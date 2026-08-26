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
//! lifts that, solely so the `gatt-extract`/`study-ui` features' authoring-time
//! tools can use the filesystem. Every sequence/string field uses
//! fixed-capacity `heapless` collections, not `alloc` (§3 decision 15) — with
//! exactly one opt-in exception, added by design decision 46: the `alloc`
//! feature backs `Study.steps` with a heap `Vec` instead of a 64-slot inline
//! array, for host consumers that have an allocator anyway and were paying
//! ~38 KB of stack per `Study` to carry two steps. It is **off by default**,
//! so dev-bench firmware and every default build of this crate still require
//! no global allocator. See [`bounded`] for the full reasoning.
#![cfg_attr(not(feature = "std"), no_std)]
// `Box`-ing large enum variants would need `alloc`, which decision 15
// deliberately rules out end to end — `MAX_PAYLOAD_LEN`-sized variants being
// large on the stack is the accepted trade-off for staying allocator-free.
#![allow(clippy::large_enum_variant)]

// `alloc` is opted into by host consumers (design.md §3 decision 46) and is
// unavailable to dev-bench firmware, so every use of it is feature-gated.
#[cfg(feature = "alloc")]
extern crate alloc;

pub mod crc;
#[cfg(feature = "ffi")]
pub mod ffi;
pub mod gatt;
#[cfg(feature = "gatt-extract")]
pub mod gatt_extract;
pub mod ids;
pub mod limits;
pub mod outpost;
#[cfg(feature = "study-ui")]
pub mod merged_actions;
pub mod protocol;
#[cfg(feature = "study-ui")]
pub mod registry;
pub mod result;
pub mod sample;
pub mod bounded;
pub mod schema_version;
pub mod streams;
pub mod study;
#[cfg(feature = "study-ui")]
pub mod study_builder;
pub mod vendor;

pub use crc::{steps_crc, streams_crc, StepTooLargeError, StreamTapTooLargeError};
pub use gatt::{
    GattActivityRecord, GattCharacteristicInfo, GattDirection, GattEventKind,
    GattServiceInfo, GattTranscriptEntry,
};
#[cfg(feature = "gatt-extract")]
pub use gatt_extract::{ZephyrBleDefExtractor, ExtractError, GattConfigExtractor};
pub use ids::{BleAddress, BleAddressKind, Uuid};
#[cfg(feature = "study-ui")]
pub use merged_actions::{merge_actions, BuiltInAction, DiscoverySources, MergedAction};
pub use protocol::DevBenchMessage;
#[cfg(feature = "study-ui")]
pub use registry::{
    ActionField, ActionFieldValue, ActionRegistry, RegisteredAction, RegisteredOperation,
    RegistryError,
};
pub use result::{
    Outcome, Provenance, StepResult, StudyResult, VersionOverride, VersionSource, VersionSubject,
};
pub use sample::{Sample, Unit};
pub use schema_version::{DEV_BENCH_WIRE_SCHEMA_VERSION, HOST_TYPE_SCHEMA_VERSION};
pub use streams::{
    dev_bench_log_tap, samples_in, validate_taps, SampleLayout, StreamEncoding, StreamRecord, StreamRef, StreamScope,
    StreamSource, StreamTap, StreamTapError, RESERVED_DEV_BENCH_STREAM_NAME,
};
pub use study::{
    requirement_satisfied, Action, BleRole, GattOperation, Requirements, RequirementsError, Step,
    Study, REQUIREMENT_ANY,
};
#[cfg(feature = "study-ui")]
pub use study_builder::{build_study, BuildStudyError, BuiltInActionKind, RoleChoice, RowAction, TableRow};
pub use vendor::{VendorCharacteristic, VendorService, NORDIC_UART_SERVICE};

#[cfg(test)]
mod tests {
    use super::*;
    use heapless::Vec as HVec;

    fn sample_study() -> Study {
        let mut steps = bounded::StepList::new();
        steps
            .push(Step {
                name: heapless::String::try_from("connect").unwrap(),
                action: Action::BleConnect { role: BleRole::Central, target_address: None , target_name: None },
                timeout_ms: 5_000,
                continue_on_fail: false,
                delay_before_ms: 0,
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
                continue_on_fail: true,
                delay_before_ms: 0,
            })
            .unwrap();

        let steps_crc = crc::steps_crc(&steps).unwrap();
        let streams: HVec<StreamTap, { limits::MAX_STREAMS_PER_STUDY }> = HVec::new();
        let streams_crc = crc::streams_crc(&streams).unwrap();

        Study {
            name: heapless::String::try_from("smoke-test").unwrap(),
            requires: Requirements::any(),
            steps,
            streams,
            steps_crc,
            streams_crc,
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
        assert_eq!(crc::streams_crc(&decoded.streams).unwrap(), decoded.streams_crc);
    }

    /// Encode/decode one message and assert it survives, in a callee frame
    /// so the caller never holds more than one at a time.
    ///
    /// Still in a callee frame after design.md §3 decision 46, for a reason
    /// that changed: `StudyStart`'s steps are no longer a 64-slot inline
    /// array (this test build has `alloc`), so `DevBenchMessage` is far
    /// smaller than the ~40 KiB it used to be — but `StepResult`'s own
    /// `gatt_activity` is still large and unmigrated, so holding a dozen
    /// messages in one frame is still not free.
    #[inline(never)]
    fn assert_round_trips(msg: &DevBenchMessage) {
        let mut buf = [0u8; 4096];
        let encoded = postcard::to_slice(msg, &mut buf).unwrap();
        let decoded: DevBenchMessage = postcard::from_bytes(encoded).unwrap();
        assert_eq!(msg, &decoded);
    }

    #[test]
    fn handshake_messages_round_trip() {
        assert_round_trips(&DevBenchMessage::Hello {
            schema_version: DEV_BENCH_WIRE_SCHEMA_VERSION,
            host_utc_ms: 1_753_000_000_000,
        });
        assert_round_trips(&DevBenchMessage::HelloAck {
            schema_version: DEV_BENCH_WIRE_SCHEMA_VERSION,
            compatible: true,
            firmware_version: heapless::String::try_from("nrf54l15dk-g1a2b3c").unwrap(),
            hardware_id: heapless::String::try_from("aaaaaaaabbbbbbbb").unwrap(),
        });
        // A bench whose build has no `hwinfo` driver reports an empty
        // string, and that has to survive the wire like any other value —
        // Core's own comparison is what decides it is unusable, not the
        // encoder silently dropping it.
        assert_round_trips(&DevBenchMessage::HelloAck {
            schema_version: DEV_BENCH_WIRE_SCHEMA_VERSION,
            compatible: true,
            firmware_version: heapless::String::try_from("nrf54l15dk-g1a2b3c").unwrap(),
            hardware_id: heapless::String::new(),
        });
        assert_round_trips(&DevBenchMessage::LogLine {
            text: heapless::String::try_from("ble: connected").unwrap(),
        });
    }

    #[test]
    fn stream_messages_round_trip() {
        assert_round_trips(&DevBenchMessage::StreamOpen { id: 0 });
        assert_round_trips(&one_record_batch(0, stream_record(1_753_000_000_000, &[0xDE, 0xAD])));
        assert_round_trips(&DevBenchMessage::StreamClose { id: 0, dropped: 7 });
    }

    #[test]
    fn study_lifecycle_messages_round_trip() {
        let study = sample_study();
        assert_round_trips(&DevBenchMessage::StudyStart {
            steps: study.steps.clone(),
            steps_crc: study.steps_crc,
            streams: study.streams.clone(),
            streams_crc: study.streams_crc,
        });
        assert_round_trips(&DevBenchMessage::StepResult {
            step_index: 0,
            result: StepResult {
                step_name: heapless::String::try_from("connect").unwrap(),
                outcome: Outcome::Pass,
                captured_data: None,
                gatt_services: None,
                gatt_activity: None,
            },
        });
        assert_round_trips(&DevBenchMessage::StudyDone { completed: true });
    }

    #[test]
    fn requires_never_crosses_the_wire_to_dev_bench() {
        // design.md §3 decision 40: `requires` is host-side only. dev-bench
        // has no use for a
        // requirement it cannot check about itself, and `steps_crc` seals
        // what dev-bench actually executes, which is unchanged.
        //
        // Asserted structurally rather than by reading the type: two studies
        // that differ *only* in `requires` must produce byte-identical
        // `StudyStart` messages, and the same `steps_crc`.
        let plain = sample_study();
        let mut demanding = sample_study();
        demanding.requires = Requirements {
            dev_bench_version: heapless::String::try_from("g-dev-bench-9f9f9f9f").unwrap(),
            firmware_version: heapless::String::try_from("g-dut-1a1a1a1a-dirty").unwrap(),
        };
        assert_ne!(plain.requires, demanding.requires, "the two studies must actually differ");
        assert_eq!(plain.steps_crc, demanding.steps_crc);

        fn encoded_study_start(study: &Study, into: &mut [u8]) -> usize {
            let msg = DevBenchMessage::StudyStart {
                steps: study.steps.clone(),
                steps_crc: study.steps_crc,
                streams: study.streams.clone(),
                streams_crc: study.streams_crc,
            };
            postcard::to_slice(&msg, into).unwrap().len()
        }

        let mut a = [0u8; 4096];
        let mut b = [0u8; 4096];
        let len_a = encoded_study_start(&plain, &mut a);
        let len_b = encoded_study_start(&demanding, &mut b);
        assert_eq!(&a[..len_a], &b[..len_b], "requires leaked into StudyStart");
    }

    #[test]
    fn a_declared_version_is_never_reported_as_a_verified_one() {
        // design.md §3 decision 40's load-bearing asymmetry: dev-bench
        // self-reports over HelloAck and is genuinely checked; the DUT
        // reports nothing at all, so an unflashed run's DUT version is an
        // assertion nobody verified. A result that rendered the two
        // identically would be the same defect in a new place.
        let provenance = Provenance {
            dev_bench_version: heapless::String::try_from("g-dev-bench-9f9f9f9f").unwrap(),
            firmware_version: heapless::String::try_from("g-dut-1a1a1a1a").unwrap(),
            dev_bench_source: VersionSource::ReportedByDevBench,
            firmware_source: VersionSource::Declared,
            overrides: HVec::new(),
        };
        assert!(provenance.dev_bench_source.is_verified());
        assert!(!provenance.firmware_source.is_verified());
        for verified in [
            VersionSource::ReportedByDevBench,
            VersionSource::ReportedByOutpost,
            VersionSource::FlashedThisRun,
        ] {
            assert!(verified.is_verified(), "{verified:?}");
        }

        let mut buf = [0u8; 256];
        let encoded = postcard::to_slice(&provenance, &mut buf).unwrap();
        let decoded: Provenance = postcard::from_bytes(encoded).unwrap();
        assert_eq!(provenance, decoded);

        let json = serde_json::to_string(&provenance).unwrap();
        let decoded: Provenance = serde_json::from_str(&json).unwrap();
        assert_eq!(provenance, decoded);
    }

    #[test]
    fn an_overridden_run_says_so_in_its_own_result() {
        // design.md §3 decision 40: an override is "recorded in the result
        // rather than silently honoured". The thing that makes the record
        // worth having is that both strings survive into it — `Study.requires`
        // never travels into a `StudyResult`, so without them a reader has no
        // way to see what was waved through.
        let mut overrides: HVec<VersionOverride, { limits::MAX_VERSION_OVERRIDES }> = HVec::new();
        overrides
            .push(VersionOverride {
                subject: VersionSubject::DevBench,
                required: heapless::String::try_from("g1a2b3c4").unwrap(),
                actual: heapless::String::try_from("gdeadbeef").unwrap(),
            })
            .unwrap();
        let provenance = Provenance {
            dev_bench_version: heapless::String::try_from("gdeadbeef").unwrap(),
            firmware_version: heapless::String::try_from("g-dut-1a1a1a1a").unwrap(),
            dev_bench_source: VersionSource::ReportedByDevBench,
            firmware_source: VersionSource::FlashedThisRun,
            overrides,
        };
        assert!(provenance.was_overridden());
        let recorded = provenance.override_for(VersionSubject::DevBench).unwrap();
        assert_eq!(recorded.required.as_str(), "g1a2b3c4");
        assert_eq!(recorded.actual.as_str(), "gdeadbeef");
        assert_eq!(VersionSubject::DevBench.field_name(), "dev_bench_version");
        assert!(provenance.override_for(VersionSubject::Firmware).is_none());

        let mut buf = [0u8; 512];
        let encoded = postcard::to_slice(&provenance, &mut buf).unwrap();
        let decoded: Provenance = postcard::from_bytes(encoded).unwrap();
        assert_eq!(provenance, decoded);

        let json = serde_json::to_string(&provenance).unwrap();
        let decoded: Provenance = serde_json::from_str(&json).unwrap();
        assert_eq!(provenance, decoded);
    }

    #[test]
    fn a_result_written_before_overrides_existed_still_reads_as_not_overridden() {
        // `events.json` files on disk predate this field, and those runs
        // predate any way to override anything — so an absent `overrides` is
        // truthfully empty rather than unknown. `#[serde(default)]` is what
        // keeps `GET /study/{id}` able to read them back at all.
        let json = r#"{
            "dev_bench_version": "gdeadbeef",
            "firmware_version": "any",
            "dev_bench_source": "ReportedByDevBench",
            "firmware_source": "Declared"
        }"#;
        let decoded: Provenance = serde_json::from_str(json).unwrap();
        assert!(decoded.overrides.is_empty());
        assert!(!decoded.was_overridden());
    }

    #[test]
    fn a_stream_ref_says_when_a_capture_is_short() {
        let full = StreamRef {
            name: heapless::String::try_from("outpost").unwrap(),
            bytes_written: 4_096,
            truncated: false,
        };
        let short = StreamRef { truncated: true, ..full.clone() };
        assert_ne!(full, short);

        let mut buf = [0u8; 128];
        let encoded = postcard::to_slice(&short, &mut buf).unwrap();
        let decoded: StreamRef = postcard::from_bytes(encoded).unwrap();
        assert_eq!(short, decoded);

        let json = serde_json::to_string(&short).unwrap();
        let decoded: StreamRef = serde_json::from_str(&json).unwrap();
        assert_eq!(short, decoded);
    }

    #[test]
    fn a_stream_tap_round_trips_through_postcard_and_json() {
        let tap = StreamTap {
            id: 0,
            name: heapless::String::try_from("outpost").unwrap(),
            source: StreamSource::Signal {
                name: heapless::String::try_from("outpost").unwrap(),
            },
            encoding: StreamEncoding::OutpostTrace,
            scope: StreamScope::WholeStudy,
        };

        let mut buf = [0u8; 256];
        let encoded = postcard::to_slice(&tap, &mut buf).unwrap();
        let decoded: StreamTap = postcard::from_bytes(encoded).unwrap();
        assert_eq!(tap, decoded);

        let json = serde_json::to_string(&tap).unwrap();
        let decoded: StreamTap = serde_json::from_str(&json).unwrap();
        assert_eq!(tap, decoded);

        let waveform = StreamTap {
            id: 1,
            name: heapless::String::try_from("waveform").unwrap(),
            source: StreamSource::GattNotify {
                service_uuid: Uuid([1u8; 16]),
                characteristic_uuid: Uuid([2u8; 16]),
            },
            encoding: StreamEncoding::Samples {
                layout: SampleLayout::F32Le,
                unit: Unit::Raw,
                channel_id: 0,
            },
            scope: StreamScope::Steps { from: 1, to: 2 },
        };
        let encoded = postcard::to_slice(&waveform, &mut buf).unwrap();
        let decoded: StreamTap = postcard::from_bytes(encoded).unwrap();
        assert_eq!(waveform, decoded);
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

    fn sample_gatt_services() -> bounded::Bounded<crate::gatt::GattServiceInfo, { limits::MAX_DISCOVERED_SERVICES }> {
        let mut chars: HVec<crate::gatt::GattCharacteristicInfo, { limits::MAX_CHARS_PER_SERVICE }> =
            HVec::new();
        chars
            .push(crate::gatt::GattCharacteristicInfo { uuid: Uuid([1u8; 16]), properties: 0x12 })
            .unwrap();
        chars
            .push(crate::gatt::GattCharacteristicInfo { uuid: Uuid([2u8; 16]), properties: 0x0a })
            .unwrap();

        let mut services: bounded::Bounded<crate::gatt::GattServiceInfo, { limits::MAX_DISCOVERED_SERVICES }> =
            bounded::Bounded::new();
        services
            .push(crate::gatt::GattServiceInfo { uuid: Uuid([0u8; 16]), characteristics: chars })
            .unwrap();
        services
    }

    #[test]
    fn gatt_service_info_round_trips_through_postcard_and_json() {
        let services = sample_gatt_services();

        // Encoded from a `Bounded` and decoded back into a plain
        // `heapless::Vec` — which is now the load-bearing assertion behind
        // §3 decisions 46/49 needing no schema bump. A host encoding this
        // field and a dev-bench build decoding it hold *different* shapes of
        // the same type, and this is what says those shapes agree on the
        // wire. Compared as slices because the two are deliberately not the
        // same Rust type.
        let mut buf = [0u8; 512];
        let encoded = postcard::to_slice(&services, &mut buf).unwrap();
        let decoded: HVec<crate::gatt::GattServiceInfo, { limits::MAX_DISCOVERED_SERVICES }> =
            postcard::from_bytes(encoded).unwrap();
        assert_eq!(&services[..], &decoded[..]);

        let json = serde_json::to_string(&services).unwrap();
        let decoded: HVec<crate::gatt::GattServiceInfo, { limits::MAX_DISCOVERED_SERVICES }> =
            serde_json::from_str(&json).unwrap();
        assert_eq!(&services[..], &decoded[..]);
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
            "captured_data": null
        }"#;
        let decoded: StepResult = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(decoded.gatt_services, None);
        assert_eq!(decoded.gatt_activity, None);
    }

    // design.md §3 decision 36, §4.3b: the streamed GATT transcript — its
    // record type, its own wire variant, the two window actions, and the
    // `gatt.csv` rendering whose column knowledge lives only in this crate.

    fn sample_transcript_entry() -> crate::gatt::GattTranscriptEntry {
        let mut payload: HVec<u8, { limits::MAX_PAYLOAD_LEN }> = HVec::new();
        payload.extend_from_slice(b"ok\r\n").unwrap();
        crate::gatt::GattTranscriptEntry {
            rx_utc_ms: 1_753_000_000_777,
            direction: crate::gatt::GattDirection::In,
            kind: crate::gatt::GattEventKind::Notification,
            service_uuid: Uuid::parse("6e400001-b5a3-f393-e0a9-e50e24dcca9e"),
            characteristic_uuid: Uuid::parse("6e400003-b5a3-f393-e0a9-e50e24dcca9e"),
            att_status: 0,
            payload,
        }
    }

    #[test]
    fn uuid_parses_every_form_an_engineer_types() {
        let full = Uuid::parse("6e400001-b5a3-f393-e0a9-e50e24dcca9e").unwrap();
        assert_eq!(full.to_hyphenated().as_str(), "6e400001-b5a3-f393-e0a9-e50e24dcca9e");
        // Same value with the hyphens stripped.
        assert_eq!(Uuid::parse("6e400001b5a3f393e0a9e50e24dcca9e").unwrap(), full);
        // Case-insensitive.
        assert_eq!(Uuid::parse("6E400001-B5A3-F393-E0A9-E50E24DCCA9E").unwrap(), full);

        // 16-bit shorthand expands against the Bluetooth SIG Base UUID, in
        // every spelling — a Core Spec fact, not a DUT-specific inference.
        let battery = Uuid::parse("180f").unwrap();
        assert_eq!(battery.to_hyphenated().as_str(), "0000180f-0000-1000-8000-00805f9b34fb");
        assert_eq!(Uuid::parse("0x180F").unwrap(), battery);
        assert_eq!(Uuid::parse("0000180f").unwrap(), battery);
        assert_eq!(Uuid::parse("0000180f-0000-1000-8000-00805f9b34fb").unwrap(), battery);

        for bad in ["", "zzzz", "6e400001-b5a3-f393-e0a9-e50e24dcca9e00", "not a uuid"] {
            assert!(Uuid::parse(bad).is_none(), "{bad} should not parse");
        }
    }

    #[test]
    fn gatt_transcript_entry_round_trips_through_postcard_and_json() {
        let entry = sample_transcript_entry();

        let mut buf = [0u8; 1024];
        let encoded = postcard::to_slice(&entry, &mut buf).unwrap();
        let decoded: crate::gatt::GattTranscriptEntry = postcard::from_bytes(encoded).unwrap();
        assert_eq!(entry, decoded);

        let json = serde_json::to_string(&entry).unwrap();
        let decoded: crate::gatt::GattTranscriptEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, decoded);
    }

    #[test]
    fn a_gatt_transcript_entry_rides_a_generic_stream_record() {
        // Decision 39 retired `DevBenchMessage::GattTranscriptRecord` as a
        // variant while keeping the entry type and its `gatt.csv` columns
        // exactly as decision 36 shipped them: the entry is now the payload
        // of a generic `StreamRecord` on a tap declared
        // `StreamEncoding::GattTranscript`.
        let entry = sample_transcript_entry();
        let mut entry_buf = [0u8; 512];
        let entry_bytes = postcard::to_slice(&entry, &mut entry_buf).unwrap();

        let mut records: HVec<StreamRecord, { limits::MAX_STREAM_RECORDS_PER_BATCH }> =
            HVec::new();
        records
            .push(StreamRecord {
                rx_utc_ms: entry.rx_utc_ms,
                bytes: HVec::from_slice(entry_bytes).unwrap(),
            })
            .unwrap();
        let msg = DevBenchMessage::StreamChunkBatch { id: 2, records };

        let mut buf = [0u8; 1024];
        let encoded = postcard::to_slice(&msg, &mut buf).unwrap();
        let decoded: DevBenchMessage = postcard::from_bytes(encoded).unwrap();
        assert_eq!(msg, decoded);

        let DevBenchMessage::StreamChunkBatch { records, .. } = decoded else {
            panic!("not a StreamChunkBatch");
        };
        let round_tripped: crate::gatt::GattTranscriptEntry =
            postcard::from_bytes(&records[0].bytes).unwrap();
        assert_eq!(round_tripped, entry);
    }

    // ---- Cross-language wire contract (design.md §3 decisions 36, 39) ----
    //
    // dev-bench firmware hand-writes its postcard encoding in C
    // (`serial_protocol.c`), and nothing in either side's own test suite
    // would notice the two drifting -- a reordered field or a u8 written as
    // a varint decodes into plausible-looking garbage, not an error. So the
    // exact bytes for every wire record are pinned twice: as a literal COBS
    // frame in dev-bench's ztest suite
    // (`app/tests/serial_protocol/src/main.c`), and as the identical
    // pre-COBS body here. Changing a shape must break both, in both
    // languages. This pairing found a real discrepancy the first time it ran
    // (decision 36), which is the argument for it.

    /// `StreamOpen { id: 2 }` (schema v8, decision 39).
    const WIRE_STREAM_OPEN: &[u8] = &[
        0x02, // tag: StreamOpen (DevBenchMessage variant 2)
        0x02, // id: 2, a raw u8 rather than a varint
    ];

    /// `StreamClose { id: 2, dropped: 5 }` (schema v8, decision 39).
    const WIRE_STREAM_CLOSE: &[u8] = &[
        0x04, // tag: StreamClose (DevBenchMessage variant 4)
        0x02, // id: 2, a raw u8
        0x05, // dropped: 5, varint
    ];

    /// `StreamChunkBatch { id: 2, records: [{ rx_utc_ms: 777, bytes: "ok\r\n" }] }`
    /// (schema v8, decision 39) — the shape that replaced both `StreamChunk`
    /// and the old `Sample`-carrying `StreamChunkBatch`.
    const WIRE_STREAM_CHUNK_BATCH: &[u8] = &[
        0x03, // tag: StreamChunkBatch (DevBenchMessage variant 3)
        0x02, // id: 2, a raw u8
        0x01, // records: 1 entry
        0x89, 0x06, // records[0].rx_utc_ms: 777, varint
        0x04, // records[0].bytes: 4 bytes
        0x6f, 0x6b, 0x0d, 0x0a, // "ok\r\n"
    ];

    /// `StepResult { step_index: 1, result: { step_name: "advertise",
    /// outcome: Pass, captured_data: Some([DE AD BE EF]), gatt_services:
    /// None, gatt_activity: None } }` (schema v9).
    ///
    /// **Pinned only at v9, and that is the point.** `StepResult` predates
    /// decision 36's both-languages rule, which applied to *new* records, so
    /// this — the message dev-bench sends most — was never pinned. In the
    /// gap, dev-bench's C encoder kept writing two `Option` bytes for
    /// `power_samples_ref`/`waveform_ref` for a whole schema version after
    /// decision 39 retired both fields, and both sides' own round-trip
    /// suites stayed green because each agreed with itself.
    const WIRE_STEP_RESULT: &[u8] = &[
        0x07, // tag: StepResult (DevBenchMessage variant 7)
        0x01, // step_index: 1, varint
        0x09, // step_name: 9 bytes
        0x61, 0x64, 0x76, 0x65, 0x72, 0x74, 0x69, 0x73, 0x65, // "advertise"
        0x00, // outcome: Pass
        0x01, // captured_data: Some
        0x04, // ...4 bytes
        0xde, 0xad, 0xbe, 0xef,
        0x00, // gatt_services: None
        0x00, // gatt_activity: None
        // Nothing between `captured_data` and `gatt_services`: the two
        // retired refs are gone from the type and must be gone from the
        // wire.
    ];

    /// `HelloAck { schema_version: 10, compatible: true, firmware_version:
    /// "g1a2b3c", hardware_id: "aaaaaaaabbbbbbbb" }` (schema v10, decision
    /// 47).
    ///
    /// **Pinned because the field is new, which is exactly decision 36's
    /// rule.** `HelloAck` had never been pinned — like `StepResult`, it
    /// predates that rule — and `StepResult`'s own history is the argument
    /// for doing it now rather than later: dev-bench's C encoder wrote two
    /// stale `Option` bytes for a whole schema version while both sides'
    /// round-trip suites stayed green, because each agreed with itself. Two
    /// self-consistent encoders are not one wire format.
    const WIRE_HELLO_ACK: &[u8] = &[
        0x01, // tag: HelloAck (DevBenchMessage variant 1)
        0x0a, // schema_version: 10, varint
        0x01, // compatible: true
        0x07, // firmware_version: 7 bytes
        0x67, 0x31, 0x61, 0x32, 0x62, 0x33, 0x63, // "g1a2b3c"
        0x10, // hardware_id: 16 bytes
        0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, // "aaaaaaaa"
        0x62, 0x62, 0x62, 0x62, 0x62, 0x62, 0x62, 0x62, // "bbbbbbbb"
    ];

    #[test]
    fn hello_ack_matches_dev_bench_firmwares_own_hand_written_encoding() {
        assert_pinned(
            WIRE_HELLO_ACK,
            &DevBenchMessage::HelloAck {
                schema_version: 10,
                compatible: true,
                firmware_version: heapless::String::try_from("g1a2b3c").unwrap(),
                hardware_id: heapless::String::try_from("aaaaaaaabbbbbbbb").unwrap(),
            },
        );
    }

    #[test]
    fn an_empty_hardware_id_is_one_zero_length_byte_not_an_absent_field() {
        // The distinction matters to the C decoder: a bench with no hwinfo
        // driver still writes the length prefix, so the frame stays walkable
        // for anything appended after it.
        let mut expected = WIRE_HELLO_ACK[..WIRE_HELLO_ACK.len() - 17].to_vec();
        expected.push(0x00);
        assert_pinned(
            &expected,
            &DevBenchMessage::HelloAck {
                schema_version: 10,
                compatible: true,
                firmware_version: heapless::String::try_from("g1a2b3c").unwrap(),
                hardware_id: heapless::String::new(),
            },
        );
    }

    fn stream_record(rx_utc_ms: u64, bytes: &[u8]) -> StreamRecord {
        StreamRecord { rx_utc_ms, bytes: HVec::from_slice(bytes).unwrap() }
    }

    fn one_record_batch(id: u8, record: StreamRecord) -> DevBenchMessage {
        let mut records: HVec<StreamRecord, { limits::MAX_STREAM_RECORDS_PER_BATCH }> =
            HVec::new();
        records.push(record).unwrap();
        DevBenchMessage::StreamChunkBatch { id, records }
    }

    /// Decodes to the expected value *and* re-encodes to exactly the same
    /// bytes — so drift in either direction breaks this, not just one.
    fn assert_pinned(wire: &[u8], expected: &DevBenchMessage) {
        let decoded: DevBenchMessage = postcard::from_bytes(wire).unwrap();
        assert_eq!(&decoded, expected, "pinned bytes decoded to the wrong value");

        let mut buf = [0u8; 256];
        let re_encoded = postcard::to_slice(expected, &mut buf).unwrap();
        assert_eq!(re_encoded, wire, "re-encoding drifted from the pinned bytes");
    }

    #[test]
    fn step_result_matches_dev_bench_firmwares_own_hand_written_encoding() {
        let mut captured: HVec<u8, { limits::MAX_PAYLOAD_LEN }> = HVec::new();
        captured.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
        assert_pinned_large(
            WIRE_STEP_RESULT,
            &DevBenchMessage::StepResult {
                step_index: 1,
                result: StepResult {
                    step_name: heapless::String::try_from("advertise").unwrap(),
                    outcome: Outcome::Pass,
                    captured_data: Some(captured),
                    gatt_services: None,
                    gatt_activity: None,
                },
            },
        );
    }

    /// [`assert_pinned`] with a buffer big enough for a `StepResult`, whose
    /// `gatt_activity` array makes the message far larger than the 256-byte
    /// scratch the stream records need. `#[inline(never)]` for the same
    /// stack reason `assert_round_trips` is.
    #[inline(never)]
    fn assert_pinned_large(wire: &[u8], expected: &DevBenchMessage) {
        let decoded: DevBenchMessage = postcard::from_bytes(wire).unwrap();
        assert_eq!(&decoded, expected, "pinned bytes decoded to the wrong value");

        let mut buf = [0u8; 4096];
        let re_encoded = postcard::to_slice(expected, &mut buf).unwrap();
        assert_eq!(re_encoded, wire, "re-encoding drifted from the pinned bytes");
    }

    #[test]
    fn stream_open_matches_dev_bench_firmwares_own_hand_written_encoding() {
        assert_pinned(WIRE_STREAM_OPEN, &DevBenchMessage::StreamOpen { id: 2 });
    }

    #[test]
    fn stream_close_matches_dev_bench_firmwares_own_hand_written_encoding() {
        assert_pinned(WIRE_STREAM_CLOSE, &DevBenchMessage::StreamClose { id: 2, dropped: 5 });
    }

    #[test]
    fn stream_chunk_batch_matches_dev_bench_firmwares_own_hand_written_encoding() {
        assert_pinned(
            WIRE_STREAM_CHUNK_BATCH,
            &one_record_batch(2, stream_record(777, b"ok\r\n")),
        );
    }

    #[test]
    fn a_stream_close_with_zero_fields_still_round_trips() {
        // Every byte of this body is 0x00 except the tag, which is what
        // makes it the interesting case for dev-bench's COBS encoder --
        // three consecutive zeros become three overhead bytes and no
        // literal data at all. Pinned as a frame on the C side; pinned as
        // the body here.
        assert_pinned(&[0x04, 0x00, 0x00], &DevBenchMessage::StreamClose { id: 0, dropped: 0 });
    }

    /// One postcard-encoded `GattTranscriptEntry` — no longer a message of
    /// its own (decision 39 retired `DevBenchMessage::GattTranscriptRecord`),
    /// now the payload of a `StreamRecord` on a tap declared
    /// `StreamEncoding::GattTranscript`. Byte-for-byte the entry half of the
    /// frame decision 36 originally pinned; the entry's own shape did not
    /// change, only what carries it.
    const WIRE_GATT_TRANSCRIPT_ENTRY: &[u8] = &[
        0x89, 0x06, // rx_utc_ms: 777, varint
        0x01, // direction: In
        0x0b, // kind: Notification
        0x01, // service_uuid: Some
        0x6e, 0x40, 0x00, 0x01, 0xb5, 0xa3, 0xf3, 0x93, 0xe0, 0xa9, 0xe5, 0x0e, 0x24, 0xdc,
        0xca, 0x9e, //
        0x01, // characteristic_uuid: Some
        0x6e, 0x40, 0x00, 0x03, 0xb5, 0xa3, 0xf3, 0x93, 0xe0, 0xa9, 0xe5, 0x0e, 0x24, 0xdc,
        0xca, 0x9e, //
        0x00, // att_status: 0, a raw u8 rather than a varint
        0x04, // payload: 4 bytes
        0x6f, 0x6b, 0x0d, 0x0a, // "ok\r\n"
    ];

    #[test]
    fn gatt_transcript_entry_matches_dev_bench_firmwares_own_hand_written_encoding() {
        // Same shape as `sample_transcript_entry`, with a small `rx_utc_ms`
        // so the pinned literal stays a readable two-byte varint rather than
        // a five-byte epoch timestamp.
        let expected =
            crate::gatt::GattTranscriptEntry { rx_utc_ms: 777, ..sample_transcript_entry() };

        let decoded: crate::gatt::GattTranscriptEntry =
            postcard::from_bytes(WIRE_GATT_TRANSCRIPT_ENTRY).unwrap();
        assert_eq!(decoded, expected);

        let mut buf = [0u8; 128];
        let re_encoded = postcard::to_slice(&expected, &mut buf).unwrap();
        assert_eq!(re_encoded, WIRE_GATT_TRANSCRIPT_ENTRY);
    }

    #[test]
    fn a_transcript_entry_carried_as_a_stream_record_matches_the_pinned_frame() {
        // The whole message dev-bench now sends for one transcript line:
        // the pinned entry above, verbatim, inside a generic record.
        let mut wire: HVec<u8, 128> = HVec::new();
        wire.extend_from_slice(&[
            0x03, // tag: StreamChunkBatch
            0x02, // id: 2
            0x01, // records: 1 entry
            0x89, 0x06, // records[0].rx_utc_ms: 777 -- the entry's own stamp
            0x2c, // records[0].bytes: 44 bytes, the entry below
        ])
        .unwrap();
        wire.extend_from_slice(WIRE_GATT_TRANSCRIPT_ENTRY).unwrap();

        let expected = one_record_batch(2, stream_record(777, WIRE_GATT_TRANSCRIPT_ENTRY));
        let decoded: DevBenchMessage = postcard::from_bytes(&wire).unwrap();
        assert_eq!(decoded, expected);

        let mut buf = [0u8; 256];
        let re_encoded = postcard::to_slice(&expected, &mut buf).unwrap();
        assert_eq!(re_encoded, wire.as_slice());
    }

    #[test]
    fn dev_bench_message_discriminants_are_pinned() {
        // postcard encodes an enum as a varint discriminant. Decision 39
        // reuses the three slots the retired stream variants held, so which
        // tag means what is now load-bearing in a way it wasn't when the
        // rule was purely append-only -- pin all nine.
        let cases: [(DevBenchMessage, u8); 4] = [
            (
                DevBenchMessage::Hello { schema_version: 8, host_utc_ms: 0 },
                0,
            ),
            (DevBenchMessage::StreamOpen { id: 0 }, 2),
            (DevBenchMessage::StreamClose { id: 0, dropped: 0 }, 4),
            (DevBenchMessage::StudyDone { completed: true }, 8),
        ];
        for (msg, expected) in cases {
            let mut buf = [0u8; 64];
            let encoded = postcard::to_slice(&msg, &mut buf).unwrap();
            assert_eq!(encoded[0], expected, "{msg:?} moved discriminant");
        }
    }

    // `GattTranscriptEntry`'s CSV renderer is itself `std`-gated (gatt.rs).
    #[cfg(feature = "std")]
    #[test]
    fn gatt_transcript_csv_row_matches_header_shape() {
        let entry = sample_transcript_entry();
        let row = entry.to_csv_row(4, "nus-write").unwrap();

        assert_eq!(
            row.as_str(),
            "1753000000777,4,nus-write,in,notification,\
             6e400001-b5a3-f393-e0a9-e50e24dcca9e,\
             6e400003-b5a3-f393-e0a9-e50e24dcca9e,0,4,6f6b0d0a,ok.."
        );
        // Header and row must agree on column count, or every consumer of
        // gatt.csv silently misreads every column past the mismatch.
        let header_cols =
            crate::gatt::GattTranscriptEntry::csv_header().split(',').count();
        assert_eq!(row.split(',').count(), header_cols);
    }

    // `GattTranscriptEntry`'s CSV renderer is itself `std`-gated (gatt.rs).
    #[cfg(feature = "std")]
    #[test]
    fn gatt_transcript_csv_row_renders_absent_uuids_as_empty_columns() {
        let entry = crate::gatt::GattTranscriptEntry {
            rx_utc_ms: 10,
            direction: crate::gatt::GattDirection::Local,
            kind: crate::gatt::GattEventKind::DiscoveryStarted,
            service_uuid: None,
            characteristic_uuid: None,
            att_status: 0,
            payload: HVec::new(),
        };
        let row = entry.to_csv_row(0, "discover").unwrap();
        assert_eq!(row.as_str(), "10,0,discover,local,discovery_started,,,0,0,,");
        assert_eq!(
            row.split(',').count(),
            crate::gatt::GattTranscriptEntry::csv_header().split(',').count()
        );
    }

    // `GattTranscriptEntry`'s CSV renderer is itself `std`-gated (gatt.rs).
    #[cfg(feature = "std")]
    #[test]
    fn gatt_transcript_csv_row_refuses_a_step_name_that_would_break_the_shape() {
        let entry = sample_transcript_entry();
        // Dropped, never truncated or silently quoted — same posture
        // `Sample::to_csv_row` takes when a row doesn't fit.
        assert!(entry.to_csv_row(0, "bad,name").is_none());
        assert!(entry.to_csv_row(0, "bad\"name").is_none());
    }

    #[test]
    fn gatt_monitor_window_actions_round_trip() {
        for action in [Action::GattMonitorStart {}, Action::GattMonitorStop {}] {
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
    fn action_discriminants_stayed_append_only() {
        // postcard encodes an enum as a varint discriminant, so appending is
        // only safe if every pre-existing variant keeps its index. This
        // pins all seven rather than trusting the declaration order to be
        // left alone (design.md §3 decision 10's append-only rule, applied
        // to `Action` as well as `DevBenchMessage`).
        let cases: [(Action, u8); 5] = [
            (
                Action::BleConnect { role: BleRole::Central, target_address: None , target_name: None },
                1,
            ),
            (Action::GattDiscover {}, 3),
            (Action::GattMonitorAll {}, 4),
            (Action::GattMonitorStart {}, 5),
            (Action::GattMonitorStop {}, 6),
        ];
        for (action, expected) in cases {
            let mut buf = [0u8; 64];
            let encoded = postcard::to_slice(&action, &mut buf).unwrap();
            assert_eq!(encoded[0], expected, "{action:?} moved discriminant");
        }
    }



}
