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

use embarch_study_designer::limits::MAX_STREAMS_PER_STUDY;
use embarch_study_designer::{steps_crc, streams_crc, Action, Step};
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
        continue_on_fail,
        delay_before_ms: 0,
    }
}

/// `test_study_start_round_trip_one_step`.
#[test]
fn one_step_vector() {
    let mut steps = embarch_study_designer::bounded::StepList::new();
    steps.push(advertise("advertise", Some("embarch-dev-bench"), 100, 5000, false)).unwrap();
    assert_eq!(
        steps_crc(&steps).unwrap(),
        0x889F_AF61,
        "app/tests/serial_protocol/src/main.c's one-step steps_crc is stale"
    );
}

/// `test_study_start_round_trip_two_steps`.
#[test]
fn two_step_vector() {
    let mut steps = embarch_study_designer::bounded::StepList::new();
    steps.push(advertise("advertise-1", Some("dev-bench"), 100, 5000, false)).unwrap();
    steps.push(advertise("advertise-2", None, 250, 2000, true)).unwrap();
    assert_eq!(
        steps_crc(&steps).unwrap(),
        0xC366_12CC,
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

    let mut steps = embarch_study_designer::bounded::StepList::new();
    steps
        .push(Step {
            name: heapless::String::try_from("connect").unwrap(),
            action: Action::BleConnect {
                role: embarch_study_designer::BleRole::Central,
                target_address: None,
                target_name: Some(heapless::String::try_from("the client S11").unwrap()),
            },
            timeout_ms: 20_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        })
        .unwrap();
    steps
        .push(Step {
            name: heapless::String::try_from("open-capture").unwrap(),
            action: Action::GattMonitorStart {},
            timeout_ms: 20_000,
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
            continue_on_fail: false,
            delay_before_ms: 1_000,
        })
        .unwrap();
    steps
        .push(Step {
            name: heapless::String::try_from("close-capture").unwrap(),
            action: Action::GattMonitorStop {},
            timeout_ms: 5_000,
            continue_on_fail: false,
            delay_before_ms: 8_000,
        })
        .unwrap();

    let streams: Vec<embarch_study_designer::StreamTap, MAX_STREAMS_PER_STUDY> = Vec::new();
    let crc = steps_crc(&steps).unwrap();
    let streams_crc_value = streams_crc(&streams).unwrap();
    let msg = DevBenchMessage::StudyStart {
        steps,
        steps_crc: crc,
        streams,
        streams_crc: streams_crc_value,
        // Deliberately **not** the default (`Warn`, discriminant 2) — schema
        // v13's new field is pinned here precisely so an off-by-one in
        // dev-bench's decode of it cannot hide behind a value that happens to
        // match whatever the C side left in the struct. `Debug` is 4.
        dev_bench_log_level: embarch_study_designer::DevBenchLogLevel::Debug,
    };
    let mut buf = [0u8; 4096];
    let encoded = postcard::to_slice(&msg, &mut buf).unwrap();
    println!("steps_crc = {crc:#010x}");
    println!("streams_crc = {streams_crc_value:#010x}");
    println!("len = {}", encoded.len());
    let hex: std::vec::Vec<std::string::String> =
        encoded.iter().map(|b| std::format!("0x{b:02x}")).collect();
    println!("{}", hex.join(", "));
}

/// Dumps a `StudyStart` carrying **real stream taps**, so dev-bench's
/// `pc_skip_stream_tap` walker and its `streams_crc` check are pinned against
/// bytes this crate produced rather than against dev-bench's own encoder —
/// which cannot produce a non-empty tap list at all (it holds no taps).
///
/// An empty-`streams` frame proves nothing about the walker, so the taps here
/// deliberately span the variants whose *lengths* differ: a `GattNotify`
/// source (two raw 16-byte UUIDs, no length prefix), a `PowerFrontEnd` source
/// (a varint), a `Samples` encoding (two enum varints plus a raw `u8`), an
/// `OutpostTrace` encoding (a multi-byte varint), and both `StreamScope`
/// shapes. Any of those walked at the wrong width shifts `streams_crc`'s span
/// and the C-side check fails.
///
/// Run with: cargo test --test firmware_test_vectors -- --nocapture dump_study_start_with_taps
#[test]
fn dump_study_start_with_taps_wire_bytes() {
    use embarch_study_designer::protocol::DevBenchMessage;
    use embarch_study_designer::{
        SampleLayout, StreamEncoding, StreamScope, StreamSource, StreamTap, Unit, Uuid,
    };

    let mut steps = embarch_study_designer::bounded::StepList::new();
    steps.push(advertise("advertise", Some("embarch-dev-bench"), 100, 5000, false)).unwrap();

    let mut streams: Vec<StreamTap, MAX_STREAMS_PER_STUDY> = Vec::new();
    streams
        .push(StreamTap {
            id: 0,
            name: heapless::String::try_from("waveform").unwrap(),
            source: StreamSource::GattNotify {
                service_uuid: Uuid::parse("6e400001-b5a3-f393-e0a9-e50e24dcca9e").unwrap(),
                characteristic_uuid: Uuid::parse("6e400003-b5a3-f393-e0a9-e50e24dcca9e").unwrap(),
            },
            encoding: StreamEncoding::Samples {
                layout: SampleLayout::I16Le,
                unit: Unit::Raw,
                channel_id: 7,
            },
            scope: StreamScope::Steps { from: 0, to: 0 },
        })
        .unwrap();
    streams
        .push(StreamTap {
            id: 1,
            name: heapless::String::try_from("outpost").unwrap(),
            source: StreamSource::Signal {
                name: heapless::String::try_from("outpost-trace").unwrap(),
            },
            encoding: StreamEncoding::OutpostTrace,
            scope: StreamScope::WholeStudy,
        })
        .unwrap();
    streams
        .push(StreamTap {
            id: 2,
            name: heapless::String::try_from("power").unwrap(),
            source: StreamSource::PowerFrontEnd { sample_hz: 1_000 },
            encoding: StreamEncoding::Raw,
            scope: StreamScope::Steps { from: 0, to: 0 },
        })
        .unwrap();

    let crc = steps_crc(&steps).unwrap();
    let streams_crc_value = streams_crc(&streams).unwrap();
    let msg = DevBenchMessage::StudyStart {
        steps,
        steps_crc: crc,
        streams,
        streams_crc: streams_crc_value,
        // Deliberately **not** the default (`Warn`, discriminant 2) — schema
        // v13's new field is pinned here precisely so an off-by-one in
        // dev-bench's decode of it cannot hide behind a value that happens to
        // match whatever the C side left in the struct. `Debug` is 4.
        dev_bench_log_level: embarch_study_designer::DevBenchLogLevel::Debug,
    };
    let mut buf = [0u8; 4096];
    let encoded = postcard::to_slice(&msg, &mut buf).unwrap();
    println!("steps_crc = {crc:#010x}");
    println!("streams_crc = {streams_crc_value:#010x}");
    println!("len = {}", encoded.len());
    let hex: std::vec::Vec<std::string::String> =
        encoded.iter().map(|b| std::format!("0x{b:02x}")).collect();
    println!("{}", hex.join(", "));
}

/// Dumps a `StepResult` message's wire bytes.
///
/// Added at schema v9 because nothing pinned this record across the two
/// languages — it was not a *new* record when decision 36's both-languages
/// rule came in, so it was never covered — and that gap let dev-bench's C
/// encoder keep writing two `Option` bytes for `power_samples_ref`/
/// `waveform_ref` for a whole schema version after decision 39 retired both
/// fields. It is the message dev-bench sends most.
///
/// Run with: cargo test --test firmware_test_vectors -- --nocapture dump_step_result
#[test]
fn dump_step_result_wire_bytes() {
    use embarch_study_designer::protocol::DevBenchMessage;
    use embarch_study_designer::{Outcome, StepResult};

    let msg = DevBenchMessage::StepResult {
        step_index: 1,
        result: StepResult {
            step_name: heapless::String::try_from("advertise").unwrap(),
            outcome: Outcome::Pass,
            captured_data: Some(heapless::Vec::from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]).unwrap()),
            gatt_services: None,
            security_level: None,
        },
    };
    let mut buf = [0u8; 4096];
    let encoded = postcard::to_slice(&msg, &mut buf).unwrap();
    println!("len = {}", encoded.len());
    let hex: std::vec::Vec<std::string::String> =
        encoded.iter().map(|b| std::format!("0x{b:02x}")).collect();
    println!("{}", hex.join(", "));
}

/// Dumps a `StudyStart` carrying schema v12's two new actions
/// (`embarch-study-designer/design.md` §3 decisions 50/51), so dev-bench's
/// hand-written C decoder is pinned against bytes this crate produced rather
/// than against its own encoder — decision 36's both-languages rule, applied
/// to a new record the pass that adds it rather than a version later (which
/// is how `StepResult`'s own two stale bytes survived a whole schema
/// version).
///
/// `BleSecurity` is the interesting one: it is the first `Action` variant
/// since `BleConnect` to carry a field, so its tag is followed by a varint a
/// decoder must walk. `BleUnbond` is field-less and proves the tag after it
/// still lands where the encoder put it.
///
/// Run with: cargo test --test firmware_test_vectors -- --nocapture dump_study_start_with_security
#[test]
fn dump_study_start_with_security_wire_bytes() {
    use embarch_study_designer::protocol::DevBenchMessage;
    use embarch_study_designer::BleSecurityLevel;

    let mut steps = embarch_study_designer::bounded::StepList::new();
    steps
        .push(Step {
            name: heapless::String::try_from("connect").unwrap(),
            action: Action::BleConnect {
                role: embarch_study_designer::BleRole::Central,
                target_address: None,
                target_name: Some(heapless::String::try_from("the client S11").unwrap()),
            },
            timeout_ms: 20_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        })
        .unwrap();
    steps
        .push(Step {
            name: heapless::String::try_from("secure").unwrap(),
            action: Action::BleSecurity { level: BleSecurityLevel::L4 },
            // A DUT that enforces a post-connect security deadline needs the
            // elevation to start promptly and finish inside a bounded
            // window; both halves are ordinary `Step` fields (design.md §3
            // decision 42), which is why this action needed no timing field
            // of its own.
            timeout_ms: 10_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        })
        .unwrap();
    steps
        .push(Step {
            name: heapless::String::try_from("drop-bond").unwrap(),
            action: Action::BleUnbond {},
            timeout_ms: 5_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        })
        .unwrap();

    let streams: Vec<embarch_study_designer::StreamTap, MAX_STREAMS_PER_STUDY> = Vec::new();
    let crc = steps_crc(&steps).unwrap();
    let streams_crc_value = streams_crc(&streams).unwrap();
    let msg = DevBenchMessage::StudyStart {
        steps,
        steps_crc: crc,
        streams,
        streams_crc: streams_crc_value,
        // Deliberately **not** the default (`Warn`, discriminant 2) — schema
        // v13's new field is pinned here precisely so an off-by-one in
        // dev-bench's decode of it cannot hide behind a value that happens to
        // match whatever the C side left in the struct. `Debug` is 4.
        dev_bench_log_level: embarch_study_designer::DevBenchLogLevel::Debug,
    };
    let mut buf = [0u8; 4096];
    let encoded = postcard::to_slice(&msg, &mut buf).unwrap();
    println!("steps_crc = {crc:#010x}");
    println!("streams_crc = {streams_crc_value:#010x}");
    println!("len = {}", encoded.len());
    let hex: std::vec::Vec<std::string::String> =
        encoded.iter().map(|b| std::format!("0x{b:02x}")).collect();
    println!("{}", hex.join(", "));
}

/// Dumps a `StepResult` that actually *reports* a security level, so the
/// trailing `Option<BleSecurityLevel>` schema v12 appends is pinned in the
/// populated case and not only in the `None` one
/// (`dump_step_result_wire_bytes` above covers `None`).
///
/// Run with: cargo test --test firmware_test_vectors -- --nocapture dump_step_result_with_security
#[test]
fn dump_step_result_with_security_wire_bytes() {
    use embarch_study_designer::protocol::DevBenchMessage;
    use embarch_study_designer::{Outcome, BleSecurityLevel, StepResult};

    let msg = DevBenchMessage::StepResult {
        step_index: 1,
        result: StepResult {
            step_name: heapless::String::try_from("secure").unwrap(),
            outcome: Outcome::Pass,
            captured_data: None,
            gatt_services: None,
            security_level: Some(BleSecurityLevel::L4),
        },
    };
    let mut buf = [0u8; 4096];
    let encoded = postcard::to_slice(&msg, &mut buf).unwrap();
    println!("len = {}", encoded.len());
    let hex: std::vec::Vec<std::string::String> =
        encoded.iter().map(|b| std::format!("0x{b:02x}")).collect();
    println!("{}", hex.join(", "));
}

/// Dumps a `StudyStart` carrying schema v14's new wire shapes
/// (`embarch-study-designer/design.md` §3 decisions 52/53), so dev-bench's
/// hand-written C decoder is pinned against bytes this crate produced — the
/// both-languages rule, applied in the pass that adds them.
///
/// Three things here can only be walked at the right width by a decoder that
/// knows about them, and each shifts every later byte if it isn't:
///
/// * **`GattMonitorSelectedStart`'s `targets`** — the first `Action` variant
///   ever to carry a *sequence*. A decoder that walked it as a field-less
///   variant (which is what every monitor action was until now) would read
///   the length varint as the next step's name length and produce garbage
///   that still decodes.
/// * **`StreamEncoding::Struct`** — one raw `u8` after the tag, inside the
///   span `streams_crc` seals. Skipped at the wrong width and the C-side CRC
///   check fails, which is the failure this vector wants: loud, at the
///   handshake, rather than a study that runs and captures into the wrong
///   file.
/// * **A `GattMonitorStop` after both**, proving the tag after the
///   variable-length one still lands where the encoder put it — the same
///   role `BleUnbond` plays in the v12 vector.
///
/// Run with: cargo test --test firmware_test_vectors -- --nocapture dump_study_start_with_selective_monitor
#[test]
fn dump_study_start_with_selective_monitor_wire_bytes() {
    use embarch_study_designer::protocol::DevBenchMessage;
    use embarch_study_designer::{
        GattTarget, StreamEncoding, StreamScope, StreamSource, StreamTap, Uuid,
    };

    let service = Uuid::parse("6e400001-b5a3-f393-e0a9-e50e24dcca9e").unwrap();
    let tx = Uuid::parse("6e400003-b5a3-f393-e0a9-e50e24dcca9e").unwrap();
    let rx = Uuid::parse("6e400002-b5a3-f393-e0a9-e50e24dcca9e").unwrap();

    let mut targets = embarch_study_designer::bounded::Bounded::<
        GattTarget,
        { embarch_study_designer::limits::MAX_MONITOR_TARGETS },
    >::new();
    targets.push(GattTarget { service_uuid: service, characteristic_uuid: tx }).unwrap();
    targets.push(GattTarget { service_uuid: service, characteristic_uuid: rx }).unwrap();

    let mut steps = embarch_study_designer::bounded::StepList::new();
    steps
        .push(Step {
            name: heapless::String::try_from("monitor").unwrap(),
            action: Action::GattMonitorSelectedStart { targets },
            timeout_ms: 5_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        })
        .unwrap();
    steps
        .push(Step {
            name: heapless::String::try_from("stop").unwrap(),
            action: Action::GattMonitorStop {},
            timeout_ms: 1_000,
            continue_on_fail: false,
            delay_before_ms: 250,
        })
        .unwrap();

    let mut streams: Vec<StreamTap, MAX_STREAMS_PER_STUDY> = Vec::new();
    streams
        .push(StreamTap {
            id: 0,
            name: heapless::String::try_from("nus-tx").unwrap(),
            source: StreamSource::GattNotify {
                service_uuid: service,
                characteristic_uuid: tx,
            },
            // Deliberately a non-zero index, so a C decoder that skipped the
            // byte entirely rather than reading it still shifts the span.
            encoding: StreamEncoding::Struct { decoder: 1 },
            scope: StreamScope::WholeStudy,
        })
        .unwrap();

    let crc = steps_crc(&steps).unwrap();
    let streams_crc_value = streams_crc(&streams).unwrap();
    let msg = DevBenchMessage::StudyStart {
        steps,
        steps_crc: crc,
        streams,
        streams_crc: streams_crc_value,
        dev_bench_log_level: embarch_study_designer::DevBenchLogLevel::Debug,
    };
    let mut buf = [0u8; 4096];
    let encoded = postcard::to_slice(&msg, &mut buf).unwrap();
    println!("steps_crc = {crc:#010x}");
    println!("streams_crc = {streams_crc_value:#010x}");
    println!("len = {}", encoded.len());
    let hex: std::vec::Vec<std::string::String> =
        encoded.iter().map(|b| std::format!("0x{b:02x}")).collect();
    println!("{}", hex.join(", "));
}
