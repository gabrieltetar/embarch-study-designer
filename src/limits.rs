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
/// `StepResult.power_samples_ref`, `StepResult.waveform_ref`.
pub const MAX_RESULT_REF_LEN: usize = 64;
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
/// `DevBenchMessage::StreamChunkBatch.values` (design.md §3 decision 25);
/// placeholder, chosen without real dev-bench UART throughput/framing
/// numbers to size against (same caveat as this module's other constants).
pub const MAX_BATCH_SAMPLES: usize = 32;
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
