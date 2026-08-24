//! Shared data types — and the narrow set of tools to work with them — for
//! EmbArch hardware-in-the-loop studies. Compiled independently by
//! `embarch-api`, `embarch-core`, and `embarch-dev-bench` firmware so a
//! `Study` crossing between them can't drift into three independently
//! maintained, slowly-diverging definitions.
//!
//! See `embarch-doc`'s `embarch-study-designer/design.md` for the full
//! architecture record this crate is a mechanical translation of. Section
//! references in doc comments throughout this crate (`§4.1`, `§3 decision
//! 17`, etc.) point back into that document.
//!
//! `#![no_std]` by default (design.md §3 decision 5) — the `std` feature
//! lifts that, solely so the `core-validation` feature (§3 decision 19) can
//! use floating-point/statistics helpers. Every sequence/string field uses
//! fixed-capacity `heapless` collections, not `alloc` (§3 decision 15) — this
//! crate never requires a global allocator, with or without `std`.
#![cfg_attr(not(feature = "std"), no_std)]
// `Box`-ing large enum variants would need `alloc`, which decision 15
// deliberately rules out end to end — `MAX_PAYLOAD_LEN`-sized variants being
// large on the stack is the accepted trade-off for staying allocator-free.
#![allow(clippy::large_enum_variant)]

pub mod crc;
#[cfg(feature = "ffi")]
pub mod ffi;
pub mod gatt;
#[cfg(feature = "gatt-extract")]
pub mod gatt_extract;
pub mod ids;
pub mod limits;
#[cfg(feature = "study-ui")]
pub mod merged_actions;
pub mod protocol;
#[cfg(feature = "study-ui")]
pub mod registry;
pub mod result;
pub mod sample;
pub mod schema_version;
#[cfg(feature = "core-validation")]
pub mod signal;
pub mod study;
#[cfg(feature = "study-ui")]
pub mod study_builder;
pub mod validation;

pub use crc::{steps_crc, StepTooLargeError};
pub use gatt::{GattActivityRecord, GattCharacteristicInfo, GattServiceInfo};
#[cfg(feature = "gatt-extract")]
pub use gatt_extract::{ZephyrBleDefExtractor, ExtractError, GattConfigExtractor};
pub use ids::{BleAddress, BleAddressKind, Uuid};
#[cfg(feature = "study-ui")]
pub use merged_actions::{merge_actions, BuiltInAction, DiscoverySources, MergedAction};
pub use protocol::{DevBenchMessage, StreamChannel};
#[cfg(feature = "study-ui")]
pub use registry::{
    ActionField, ActionFieldValue, ActionRegistry, RegisteredAction, RegisteredOperation,
    RegistryError,
};
pub use result::{Outcome, StepResult, StudyResult};
pub use sample::{Sample, Unit};
pub use schema_version::STUDY_DESIGNER_SCHEMA_VERSION;
pub use study::{Action, BleRole, GattOperation, PowerSampleWindow, Step, Study};
#[cfg(feature = "study-ui")]
pub use study_builder::{build_study, BuildStudyError, BuiltInActionKind, RoleChoice, RowAction, TableRow};
pub use validation::{
    ContentValidity, DataChannel, ExpectedValue, PostHocCheck, PostHocValidation, SignalCheck,
    ValidationResult, ValidationSource,
};

#[cfg(test)]
mod tests {
    use super::*;
    use heapless::Vec as HVec;

    fn sample_study() -> Study {
        let mut steps: HVec<Step, { limits::MAX_STEPS_PER_STUDY }> = HVec::new();
        steps
            .push(Step {
                name: heapless::String::try_from("connect").unwrap(),
                action: Action::BleConnect { role: BleRole::Central, target_address: None },
                timeout_ms: 5_000,
                power_sample: None,
                continue_on_fail: false,
            })
            .unwrap();
        steps
            .push(Step {
                name: heapless::String::try_from("read-battery").unwrap(),
                action: Action::DataExchange {
                    service_uuid: Uuid([0u8; 16]),
                    characteristic_uuid: Uuid([1u8; 16]),
                    operation: GattOperation::Read,
                },
                timeout_ms: 2_000,
                power_sample: Some(PowerSampleWindow { sample_rate_hz: 1_000 }),
                continue_on_fail: true,
            })
            .unwrap();

        let steps_crc = crc::steps_crc(&steps).unwrap();

        Study {
            name: heapless::String::try_from("smoke-test").unwrap(),
            steps,
            validations: HVec::new(),
            steps_crc,
        }
    }

    #[test]
    fn study_round_trips_through_postcard() {
        let study = sample_study();
        let mut buf = [0u8; 1024];
        let encoded = postcard::to_slice(&study, &mut buf).unwrap();
        let decoded: Study = postcard::from_bytes(encoded).unwrap();
        assert_eq!(study, decoded);
    }

    #[test]
    fn steps_crc_matches_after_round_trip() {
        let study = sample_study();
        let mut buf = [0u8; 1024];
        let encoded = postcard::to_slice(&study, &mut buf).unwrap();
        let decoded: Study = postcard::from_bytes(encoded).unwrap();

        assert_eq!(crc::steps_crc(&decoded.steps).unwrap(), decoded.steps_crc);
    }

    #[test]
    fn dev_bench_message_round_trips() {
        let hello = DevBenchMessage::Hello {
            schema_version: STUDY_DESIGNER_SCHEMA_VERSION,
            host_utc_ms: 1_753_000_000_000,
        };
        let mut buf = [0u8; 64];
        let encoded = postcard::to_slice(&hello, &mut buf).unwrap();
        let decoded: DevBenchMessage = postcard::from_bytes(encoded).unwrap();
        assert_eq!(hello, decoded);

        let chunk = DevBenchMessage::StreamChunk {
            sample: Sample { rx_utc_ms: 42, value: 3.3, unit: Unit::Milliamps, channel_id: 0 },
        };
        let encoded = postcard::to_slice(&chunk, &mut buf).unwrap();
        let decoded: DevBenchMessage = postcard::from_bytes(encoded).unwrap();
        assert_eq!(chunk, decoded);

        let hello_ack = DevBenchMessage::HelloAck {
            schema_version: STUDY_DESIGNER_SCHEMA_VERSION,
            compatible: true,
            firmware_version: heapless::String::try_from("nrf54l15dk-g1a2b3c").unwrap(),
        };
        let encoded = postcard::to_slice(&hello_ack, &mut buf).unwrap();
        let decoded: DevBenchMessage = postcard::from_bytes(encoded).unwrap();
        assert_eq!(hello_ack, decoded);

        let log_line = DevBenchMessage::LogLine { text: heapless::String::try_from("ble: connected").unwrap() };
        let encoded = postcard::to_slice(&log_line, &mut buf).unwrap();
        let decoded: DevBenchMessage = postcard::from_bytes(encoded).unwrap();
        assert_eq!(log_line, decoded);

        let study = sample_study();
        let mut big_buf = [0u8; 2048];
        let study_start =
            DevBenchMessage::StudyStart { steps: study.steps.clone(), steps_crc: study.steps_crc };
        let encoded = postcard::to_slice(&study_start, &mut big_buf).unwrap();
        let decoded: DevBenchMessage = postcard::from_bytes(encoded).unwrap();
        assert_eq!(study_start, decoded);

        let step_result = DevBenchMessage::StepResult {
            step_index: 0,
            result: StepResult {
                step_name: heapless::String::try_from("connect").unwrap(),
                outcome: Outcome::Pass,
                captured_data: None,
                power_samples_ref: None,
                waveform_ref: None,
                gatt_services: None,
                gatt_activity: None,
            },
        };
        let encoded = postcard::to_slice(&step_result, &mut buf).unwrap();
        let decoded: DevBenchMessage = postcard::from_bytes(encoded).unwrap();
        assert_eq!(step_result, decoded);

        let study_done = DevBenchMessage::StudyDone { completed: true };
        let encoded = postcard::to_slice(&study_done, &mut buf).unwrap();
        let decoded: DevBenchMessage = postcard::from_bytes(encoded).unwrap();
        assert_eq!(study_done, decoded);

        let mut values: HVec<f32, { limits::MAX_BATCH_SAMPLES }> = HVec::new();
        values.push(1.0).unwrap();
        values.push(2.0).unwrap();
        let chunk_batch = DevBenchMessage::StreamChunkBatch {
            base_utc_ms: 1_753_000_000_000,
            sample_interval_ms: 10,
            unit: Unit::Volts,
            channel_id: 1,
            values,
        };
        let encoded = postcard::to_slice(&chunk_batch, &mut buf).unwrap();
        let decoded: DevBenchMessage = postcard::from_bytes(encoded).unwrap();
        assert_eq!(chunk_batch, decoded);
    }

    #[test]
    fn sample_csv_row_matches_header_shape() {
        let sample = Sample {
            rx_utc_ms: 1_753_000_000_123,
            value: 3.301,
            unit: Unit::Milliamps,
            channel_id: 2,
        };
        let row = sample.to_csv_row("advertise").unwrap();
        assert_eq!(row.as_str(), "1753000000123,advertise,3.301,milliamps,2");
        assert_eq!(Sample::csv_header(), "rx_utc_ms,step_name,value,unit,channel_id");
    }

    #[test]
    fn oversized_step_name_does_not_fit() {
        let buf = [b'x'; limits::MAX_NAME_LEN + 1];
        let too_long = core::str::from_utf8(&buf).unwrap();
        assert!(heapless::String::<{ limits::MAX_NAME_LEN }>::try_from(too_long).is_err());
    }

    // design.md §3 decisions 31/32/33, embarch-study-designer/milestone-9.md
    // §3.1-3.4: the GATT-discovery types, the two new `Action` variants, and
    // `StepResult`'s two new fields all round-trip through both postcard
    // (the Core<->dev-bench wire format) and serde_json (the
    // embarch-api/events.json path, §3 decision 3's format-agnostic stance).

    fn sample_gatt_services() -> HVec<crate::gatt::GattServiceInfo, { limits::MAX_DISCOVERED_SERVICES }> {
        let mut chars: HVec<crate::gatt::GattCharacteristicInfo, { limits::MAX_CHARS_PER_SERVICE }> =
            HVec::new();
        chars
            .push(crate::gatt::GattCharacteristicInfo { uuid: Uuid([1u8; 16]), properties: 0x12 })
            .unwrap();
        chars
            .push(crate::gatt::GattCharacteristicInfo { uuid: Uuid([2u8; 16]), properties: 0x0a })
            .unwrap();

        let mut services: HVec<crate::gatt::GattServiceInfo, { limits::MAX_DISCOVERED_SERVICES }> =
            HVec::new();
        services
            .push(crate::gatt::GattServiceInfo { uuid: Uuid([0u8; 16]), characteristics: chars })
            .unwrap();
        services
    }

    #[test]
    fn gatt_service_info_round_trips_through_postcard_and_json() {
        let services = sample_gatt_services();

        let mut buf = [0u8; 512];
        let encoded = postcard::to_slice(&services, &mut buf).unwrap();
        let decoded: HVec<crate::gatt::GattServiceInfo, { limits::MAX_DISCOVERED_SERVICES }> =
            postcard::from_bytes(encoded).unwrap();
        assert_eq!(services, decoded);

        let json = serde_json::to_string(&services).unwrap();
        let decoded: HVec<crate::gatt::GattServiceInfo, { limits::MAX_DISCOVERED_SERVICES }> =
            serde_json::from_str(&json).unwrap();
        assert_eq!(services, decoded);
    }

    #[test]
    fn gatt_activity_record_round_trips_through_postcard_and_json() {
        let mut payload: HVec<u8, { limits::MAX_PAYLOAD_LEN }> = HVec::new();
        payload.extend_from_slice(&[0xAB, 0xCD, 0xEF]).unwrap();
        let record = crate::gatt::GattActivityRecord {
            rx_utc_ms: 1_753_000_000_500,
            characteristic_index: 3,
            payload,
        };

        let mut buf = [0u8; 512];
        let encoded = postcard::to_slice(&record, &mut buf).unwrap();
        let decoded: crate::gatt::GattActivityRecord = postcard::from_bytes(encoded).unwrap();
        assert_eq!(record, decoded);

        let json = serde_json::to_string(&record).unwrap();
        let decoded: crate::gatt::GattActivityRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, decoded);
    }

    #[test]
    fn gatt_discover_and_monitor_all_actions_round_trip() {
        for action in [Action::GattDiscover {}, Action::GattMonitorAll {}] {
            let mut buf = [0u8; 64];
            let encoded = postcard::to_slice(&action, &mut buf).unwrap();
            let decoded: Action = postcard::from_bytes(encoded).unwrap();
            assert_eq!(action, decoded);

            let json = serde_json::to_string(&action).unwrap();
            let decoded: Action = serde_json::from_str(&json).unwrap();
            assert_eq!(action, decoded);
        }
    }

    #[test]
    fn step_result_gatt_fields_round_trip_and_default_on_missing_json() {
        let result = StepResult {
            step_name: heapless::String::try_from("discover").unwrap(),
            outcome: Outcome::Pass,
            captured_data: None,
            power_samples_ref: None,
            waveform_ref: None,
            gatt_services: Some(sample_gatt_services()),
            gatt_activity: None,
        };

        let mut buf = [0u8; 1024];
        let encoded = postcard::to_slice(&result, &mut buf).unwrap();
        let decoded: StepResult = postcard::from_bytes(encoded).unwrap();
        assert_eq!(result, decoded);

        let json = serde_json::to_string(&result).unwrap();
        let decoded: StepResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, decoded);

        // A `StepResult` JSON predating decision 31/32 (no `gatt_services`/
        // `gatt_activity` keys at all) still deserializes, per `#[serde(default)]`
        // — this is the specific claim milestone-9.md §2's scope note makes.
        let legacy_json = r#"{
            "step_name": "connect",
            "outcome": "Pass",
            "captured_data": null,
            "power_samples_ref": null,
            "waveform_ref": null
        }"#;
        let decoded: StepResult = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(decoded.gatt_services, None);
        assert_eq!(decoded.gatt_activity, None);
    }

    #[test]
    fn data_channel_gatt_activity_round_trips() {
        let channel = DataChannel::GattActivity;
        let mut buf = [0u8; 16];
        let encoded = postcard::to_slice(&channel, &mut buf).unwrap();
        let decoded: DataChannel = postcard::from_bytes(encoded).unwrap();
        assert_eq!(channel, decoded);

        let json = serde_json::to_string(&channel).unwrap();
        let decoded: DataChannel = serde_json::from_str(&json).unwrap();
        assert_eq!(channel, decoded);
    }
}
