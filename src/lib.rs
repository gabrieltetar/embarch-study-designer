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
pub mod ids;
pub mod limits;
pub mod protocol;
pub mod result;
pub mod sample;
pub mod schema_version;
#[cfg(feature = "core-validation")]
pub mod signal;
pub mod study;
pub mod validation;

pub use crc::{steps_crc, StepTooLargeError};
pub use ids::{BleAddress, BleAddressKind, Uuid};
pub use protocol::{DevBenchMessage, StreamChannel};
pub use result::{Outcome, StepResult, StudyResult};
pub use sample::Sample;
pub use schema_version::STUDY_DESIGNER_SCHEMA_VERSION;
pub use study::{Action, BleRole, GattOperation, PowerSampleWindow, Step, Study};
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
            steps_crc: 0xDEAD_BEEF,
        };
        let mut buf = [0u8; 64];
        let encoded = postcard::to_slice(&hello, &mut buf).unwrap();
        let decoded: DevBenchMessage = postcard::from_bytes(encoded).unwrap();
        assert_eq!(hello, decoded);

        let chunk = DevBenchMessage::StreamChunk { sample: Sample { rx_utc_ms: 42, value: 3.3 } };
        let encoded = postcard::to_slice(&chunk, &mut buf).unwrap();
        let decoded: DevBenchMessage = postcard::from_bytes(encoded).unwrap();
        assert_eq!(chunk, decoded);
    }

    #[test]
    fn sample_csv_row_matches_header_shape() {
        let sample = Sample { rx_utc_ms: 1_753_000_000_123, value: 3.301 };
        let row = sample.to_csv_row("advertise").unwrap();
        assert_eq!(row.as_str(), "1753000000123,advertise,3.301");
        assert_eq!(Sample::csv_header(), "rx_utc_ms,step_name,value");
    }

    #[test]
    fn oversized_step_name_does_not_fit() {
        let buf = [b'x'; limits::MAX_NAME_LEN + 1];
        let too_long = core::str::from_utf8(&buf).unwrap();
        assert!(heapless::String::<{ limits::MAX_NAME_LEN }>::try_from(too_long).is_err());
    }
}
