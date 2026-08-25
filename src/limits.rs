//! Fixed-capacity bounds for every `heapless` collection in this crate.
//!
//! design.md §3 decision 15. Placeholder-but-concrete: chosen without real
//! `embarch-dev-bench` hardware to size against, flagged for re-confirmation
//! once real nRF54 memory constraints are known (design.md §7). A value
//! proving too small in practice is a version-bumped (§3 decision 12)
//! breaking wire-format change, same as any other field change.

/// `Study.steps`, `StudyResult.steps`.
pub const MAX_STEPS_PER_STUDY: usize = 64;
/// `Study.name`, `StudyResult.study_name`.
pub const MAX_STUDY_NAME_LEN: usize = 64;
/// `Step.name`, `StepResult.step_name`.
pub const MAX_NAME_LEN: usize = 32;
/// `BleAdvertise.service_uuids`.
pub const MAX_SERVICE_UUIDS: usize = 4;
/// `BleAdvertise.local_name`; sized to fit inside a legacy 31-byte BLE
/// advertising PDU alongside AD-structure/flags overhead.
pub const MAX_LOCAL_NAME_LEN: usize = 26;
/// `GattOperation::Write.payload`, `StepResult.captured_data`,
/// `ExpectedValue::Equals`/`Contains`; sized above BLE 5's practical
/// extended-MTU ceiling (247-byte ATT_MTU / 251-byte L2CAP payload).
pub const MAX_PAYLOAD_LEN: usize = 512;
/// `Outcome::Fail.reason`, `ContentValidity::Invalid.reason`.
pub const MAX_FAIL_REASON_LEN: usize = 64;
/// `Study.validations`, `StudyResult.validations`.
pub const MAX_VALIDATIONS_PER_STUDY: usize = 64;
/// `Sample::to_csv_row`'s returned buffer (design.md §4.7): sized to
/// comfortably fit `rx_utc_ms` (up to 20 ASCII digits for a `u64`), a
/// `MAX_NAME_LEN`-bounded step name, a formatted `value`, and separators.
pub const MAX_CSV_ROW_LEN: usize = 96;
/// `DevBenchMessage::LogLine.text` (embarch-dev-bench/design.md §3 decision
/// 7); one free-text log line dev-bench sends to Core instead of raw
/// interleaved bytes on the shared serial line.
pub const MAX_LOG_LINE_LEN: usize = 128;
/// `DevBenchMessage::HelloAck.firmware_version`
/// (embarch-dev-bench/design.md §3 decision 18); a single free-form
/// identifier (e.g. `git describe` output) for whichever dev-bench build
/// replied to `Hello`.
pub const MAX_FIRMWARE_VERSION_LEN: usize = 32;
/// `Study.streams` / `StudyResult.streams` (design.md §3 decision 39,
/// §4.8). Deliberately small: a tap is a declared capture channel, not a
/// per-step artifact, and the four sources that exist plus a couple of
/// `Signal` taps is the whole realistic range today.
pub const MAX_STREAMS_PER_STUDY: usize = 8;
/// `StreamTap.name`, `StreamRef.name` (design.md §4.8) — also the name that
/// becomes a file under a study's `streams/` directory, so it is bounded the
/// same way a step name is.
pub const MAX_STREAM_NAME_LEN: usize = 32;
/// `StreamSource::Signal.name` (design.md §4.8) — the topology-declared
/// signal a tap names rather than the carrier that currently delivers it
/// (`embarch-topology/design.md` §3 decision 18).
pub const MAX_SIGNAL_NAME_LEN: usize = 32;
/// `StreamRecord.bytes` (design.md §4.8) — one arrival-stamped run of bytes.
/// Placeholder-but-concrete, same posture as every other constant here: no
/// real dev-bench UART throughput number and no real outpost capture exist
/// to size against yet (design.md §7, `embarch-outpost/design.md` §7).
pub const MAX_STREAM_CHUNK_BYTES: usize = 512;
/// `DevBenchMessage::StreamChunkBatch.records` (design.md §4.8) — how many
/// arrival-stamped records ride in one framed message, amortizing COBS/
/// postcard framing overhead across a burst the same way the retired
/// `MAX_BATCH_SAMPLES` did for `Sample`s.
pub const MAX_STREAM_RECORDS_PER_BATCH: usize = 4;
/// `GattServiceInfo` entries per `StepResult.gatt_services` (design.md §3
/// decisions 31/32, §4.3a); sized against real DUT firmware observed so far
/// (`reference-dut-fw`'s `lib/ble/ble_def.h`/`ble.c` declares 2
/// services today), with headroom for a DUT this crate hasn't seen yet.
pub const MAX_DISCOVERED_SERVICES: usize = 8;
/// `GattServiceInfo.characteristics` (design.md §4.3a); the same DUT's
/// larger service (Sensor Data Service) declares up to 7 characteristics
/// today (6 unconditional, 1 gated behind `CONFIG_AIR_TEMP_ENABLE`).
pub const MAX_CHARS_PER_SERVICE: usize = 16;
/// `StepResult.gatt_activity` (design.md §3 decision 32); provisional, same
/// placeholder-but-concrete posture as every other constant in this module —
/// design.md §7 carries the stack-safety risk this size implies for dev-bench.
pub const MAX_GATT_ACTIVITY_RECORDS: usize = 32;
/// One rendered `gatt.csv` row (design.md §3 decision 36, §4.3b) — sized to
/// hold a `MAX_PAYLOAD_LEN` payload rendered *twice* (hex, then a printable-
/// ASCII column) alongside two hyphenated UUIDs and the fixed columns. Far
/// larger than `MAX_CSV_ROW_LEN` because a GATT transcript row carries a raw
/// payload, where a `Sample` row carries one `f32`; the two file formats are
/// deliberately not sized against the same constant.
pub const MAX_GATT_CSV_ROW_LEN: usize = 1792;
