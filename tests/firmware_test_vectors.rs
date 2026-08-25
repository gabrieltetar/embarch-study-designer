//! The exact `steps_crc` values `embarch-dev-bench`'s own ztest suite
//! (`app/tests/serial_protocol/src/main.c`) hardcodes for its `StudyStart`
//! round-trip tests.
//!
//! Those two tests exist to prove dev-bench's hand-written C postcard
//! encoding and CRC-32 agree with *this* crate, not merely with themselves,
//! which they do by asserting a CRC this crate computed. That only works
//! while the number in the C file really is the number this crate produces —
//! and a `Step` field added here silently invalidates both, since every
//! step's encoding feeds the digest.
//!
//! So the values live here too, as a test. If this file fails, the C file's
//! constants are stale and must be updated to whatever this crate now says
//! (never the other way round: this crate is the definition).
//!
//! Pairs with the same cross-language pinning already in place for
//! `GattTranscriptRecord`'s wire bytes (schema v5).

use embarch_study_designer::limits::MAX_STEPS_PER_STUDY;
use embarch_study_designer::{steps_crc, Action, Step};
use heapless::Vec;

fn advertise(name: &str, local_name: Option<&str>, adv_interval_ms: u16, timeout_ms: u32, continue_on_fail: bool) -> Step {
    Step {
        name: heapless::String::try_from(name).unwrap(),
        action: Action::BleAdvertise {
            local_name: local_name.map(|n| heapless::String::try_from(n).unwrap()),
            service_uuids: Vec::new(),
            adv_interval_ms,
        },
        timeout_ms,
        power_sample: None,
        continue_on_fail,
        delay_before_ms: 0,
    }
}

/// `test_study_start_round_trip_one_step`.
#[test]
fn one_step_vector() {
    let mut steps: Vec<Step, MAX_STEPS_PER_STUDY> = Vec::new();
    steps.push(advertise("advertise", Some("embarch-dev-bench"), 100, 5000, false)).unwrap();
    assert_eq!(
        steps_crc(&steps).unwrap(),
        0xE83F_21EC,
        "app/tests/serial_protocol/src/main.c's one-step steps_crc is stale"
    );
}

/// `test_study_start_round_trip_two_steps`.
#[test]
fn two_step_vector() {
    let mut steps: Vec<Step, MAX_STEPS_PER_STUDY> = Vec::new();
    steps.push(advertise("advertise-1", Some("dev-bench"), 100, 5000, false)).unwrap();
    steps.push(advertise("advertise-2", None, 250, 2000, true)).unwrap();
    assert_eq!(
        steps_crc(&steps).unwrap(),
        0x9192_3654,
        "app/tests/serial_protocol/src/main.c's two-step steps_crc is stale"
    );
}

/// Dumps the exact bytes Core puts on the wire for a `StudyStart` carrying
/// the four-step stimulate-and-capture study, so dev-bench's hand-written C
/// decoder can be tested against the real thing rather than against its own
/// encoder. Printed, not asserted — the assertion lives in
/// `app/tests/serial_protocol/src/main.c`, which hardcodes these bytes.
///
/// Run with: cargo test --features study-ui -- --nocapture dump_study_start
#[test]
fn dump_study_start_wire_bytes() {
    use embarch_study_designer::protocol::DevBenchMessage;
    use embarch_study_designer::{Action, GattOperation};

    let mut steps: Vec<Step, MAX_STEPS_PER_STUDY> = Vec::new();
    steps
        .push(Step {
            name: heapless::String::try_from("connect").unwrap(),
            action: Action::BleConnect {
                role: embarch_study_designer::BleRole::Central,
                target_address: None,
                target_name: Some(heapless::String::try_from("the client S11").unwrap()),
            },
            timeout_ms: 20_000,
            power_sample: None,
            continue_on_fail: false,
            delay_before_ms: 0,
        })
        .unwrap();
    steps
        .push(Step {
            name: heapless::String::try_from("open-capture").unwrap(),
            action: Action::GattMonitorStart {},
            timeout_ms: 20_000,
            power_sample: None,
            continue_on_fail: false,
            delay_before_ms: 0,
        })
        .unwrap();
    steps
        .push(Step {
            name: heapless::String::try_from("stimulate").unwrap(),
            action: Action::DataExchange {
                service_uuid: embarch_study_designer::vendor::NORDIC_UART_SERVICE.uuid,
                characteristic_uuid: embarch_study_designer::vendor::NORDIC_UART_SERVICE
                    .characteristic("rx")
                    .unwrap()
                    .uuid,
                operation: GattOperation::Write {
                    payload: heapless::Vec::from_slice(b"kernel version\r\n").unwrap(),
                },
            },
            timeout_ms: 5_000,
            power_sample: None,
            continue_on_fail: false,
            delay_before_ms: 1_000,
        })
        .unwrap();
    steps
        .push(Step {
            name: heapless::String::try_from("close-capture").unwrap(),
            action: Action::GattMonitorStop {},
            timeout_ms: 5_000,
            power_sample: None,
            continue_on_fail: false,
            delay_before_ms: 8_000,
        })
        .unwrap();

    let crc = steps_crc(&steps).unwrap();
    let msg = DevBenchMessage::StudyStart { steps, steps_crc: crc, streams: Vec::new() };
    let mut buf = [0u8; 4096];
    let encoded = postcard::to_slice(&msg, &mut buf).unwrap();
    println!("steps_crc = {crc:#010x}");
    println!("len = {}", encoded.len());
    let hex: std::vec::Vec<std::string::String> =
        encoded.iter().map(|b| std::format!("0x{b:02x}")).collect();
    println!("{}", hex.join(", "));
}
