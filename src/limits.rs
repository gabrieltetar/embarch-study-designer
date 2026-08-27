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
/// `DevBenchMessage::HelloAck.hardware_id` (design.md §3 decision 47,
/// `embarch-core/design.md` §3 decision 35) — dev-bench's own factory-unique
/// chip ID, hex-encoded lowercase, so Core can confirm the board answering
/// on the serial link is the same silicon its JTAG probe just verified.
///
/// 32 chars fits 16 raw bytes hex-encoded. Both IDs this suite reads over
/// JTAG today are 8 bytes (`{a:08x}{b:08x}` — 16 chars), so this is double
/// the known case: dev-bench reports what Zephyr's `hwinfo_get_device_id`
/// gives it, whose length is a per-SoC driver decision rather than something
/// this crate gets to fix.
pub const MAX_HARDWARE_ID_LEN: usize = 32;
/// `Provenance.overrides` (design.md §3 decision 40, §4.5) — how many
/// version requirements one run can have had waved through. Exactly two,
/// and not a knob: there are two requirements (`dev_bench_version`,
/// `firmware_version`) and an override names one of them, so this is the
/// arity of the thing rather than a capacity guess like the constants
/// around it.
pub const MAX_VERSION_OVERRIDES: usize = 2;
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
/// `Action::GattMonitorSelected`/`GattMonitorSelectedStart.targets`
/// (design.md §3 decision 53) — how many characteristics one selective
/// monitor step may name. Sized against the largest real DUT this suite has
/// walked (`reference-dut-fw`: 10 notify/indicate-capable
/// characteristics across two services, 7 services in total once an
/// encrypted link reaches the rest of the table), with headroom. A study
/// wanting more than this wants `GattMonitorAll`, which is what that action
/// is for.
pub const MAX_MONITOR_TARGETS: usize = 16;
/// `Study.decoders` (design.md §3 decision 52) — named payload layouts one
/// study resolves out of the firmware repo's `study-structs.toml`. Bounded
/// by what a study can actually reference: a decoder is only reachable
/// through a tap's `StreamEncoding::Struct`, and there are at most
/// [`MAX_STREAMS_PER_STUDY`] taps.
pub const MAX_DECODERS_PER_STUDY: usize = MAX_STREAMS_PER_STUDY;
/// `StructLayout.name` (design.md §4.8a) — the name a tap's decoder is
/// referenced by in `study-structs.toml`.
pub const MAX_DECODER_NAME_LEN: usize = 24;
/// `StructLayout.header`/`repeat` (design.md §4.8a) — scalars in one group.
/// Placeholder-but-concrete, same posture as every other constant here; a
/// real notification packet's header is a handful of fields and its
/// repeating element smaller still.
pub const MAX_STRUCT_FIELDS: usize = 12;
/// `StructField.name` (design.md §4.8a) — becomes a CSV column header.
pub const MAX_STRUCT_FIELD_NAME_LEN: usize = 20;
/// One rendered decoded-struct CSV row's *decoded columns*, and the header
/// naming them (design.md §4.8a). Bounded by
/// [`MAX_STRUCT_FIELDS`] × 2 groups × (a name or a rendered scalar), with
/// room for the separators — not by [`MAX_GATT_CSV_ROW_LEN`], which sizes a
/// row carrying a whole payload rendered twice.
pub const MAX_STRUCT_CSV_ROW_LEN: usize = 640;
/// One rendered `gatt.csv` row (design.md §3 decision 36, §4.3b) — sized to
/// hold a `MAX_PAYLOAD_LEN` payload rendered *twice* (hex, then a printable-
/// ASCII column) alongside two hyphenated UUIDs and the fixed columns. Far
/// larger than `MAX_CSV_ROW_LEN` because a GATT transcript row carries a raw
/// payload, where a `Sample` row carries one `f32`; the two file formats are
/// deliberately not sized against the same constant.
pub const MAX_GATT_CSV_ROW_LEN: usize = 1792;

// --- `.eap` protocol manifests (design.md §3 decisions 58-62) ------------
//
// **These bound a value dev-bench executes, not one it walks past.** A
// `ProtocolDef` is the compiled, guard-reachable half of an `.eap` manifest
// (§3 decision 59's split) and rides in `Study.protocols` all the way to the
// firmware, so every constant below costs real ESP32-C5 SRAM in
// `struct dev_bench_message`'s union. They are sized against the two worked
// protocols in §4.9 with headroom, deliberately *not* against the generous
// ceilings the host-side constants use — and dev-bench remains free to cap
// tighter still with its own internal limit, exactly as
// `DBM_MAX_STEPS_PER_STUDY` already does at 16 against
// [`MAX_STEPS_PER_STUDY`]'s 64 (`embarch-dev-bench/design.md` §3 decision 27).
//
// **The SRAM cost has not been measured.** These are placeholder-but-concrete
// in the same posture as every other constant here; the real number comes
// from a `west build` ram_report, which is `embarch-dev-bench`'s own scope
// along with the interpreter itself.

/// `Study.protocols` (design.md §3 decision 58) — `.eap` protocol blocks
/// resolved into one study at build time. A protocol is only reachable
/// through an [`crate::study::Action::RunProtocol`] step, and a study
/// running more than a couple of distinct handshakes is describing two
/// studies.
pub const MAX_PROTOCOLS_PER_STUDY: usize = 2;
/// `ProtocolDef.name` — the `protocol <name> { … }` identifier, which is
/// also how a `.eap` file's blocks are told apart.
pub const MAX_PROTOCOL_NAME_LEN: usize = 32;
/// `ProtocolDef.sources` (design.md §3 decision 58) — characteristic
/// aliases one protocol block declares for itself. Sized against the real
/// BDS download's three (`ctrl`/`status`/`data`, `embarch-study-designer/design.md`
/// §3 decision 57) with room for a protocol spanning two services.
pub const MAX_SOURCES_PER_PROTOCOL: usize = 6;
/// `ProtocolSource.name` — the alias a `write`/`frame` refers to.
pub const MAX_SOURCE_NAME_LEN: usize = 24;
/// `ProtocolDef.frames` — declared frame shapes one protocol can dispatch
/// on. Only frames a state machine actually reacts to live here; a frame
/// that exists solely to render a capture is a decision-52 `StructLayout`
/// and never reaches dev-bench.
pub const MAX_FRAMES_PER_PROTOCOL: usize = 8;
/// `FrameDef.name` — referenced by `on_event <frame>` and by field paths.
pub const MAX_FRAME_NAME_LEN: usize = 32;
/// `FrameDef.fields` — the **guard-reachable** scalar reads of one frame
/// (design.md §3 decision 59). Not every field of the real packet: only the
/// ones a `when`, a `remember` or a `write` names. A GWF1 batch record has
/// thirty channel descriptors and none of them is reachable from a guard.
pub const MAX_FRAME_FIELDS: usize = 8;
/// `FrameDef.spans` — declared byte spans of one frame. A span's *bytes*
/// never reach an expression (design.md §3 decision 60 removed byte-span
/// concatenation outright); only its `len()` does, which is what a
/// flow-controlled pump loop actually counts.
pub const MAX_FRAME_SPANS: usize = 4;
/// `ScalarRead.name` / `SpanRead.name` — a field's identifier within its
/// frame. Shares [`MAX_STRUCT_FIELD_NAME_LEN`]'s size on purpose: the same
/// `.eap` frame can also be lowered into a decision-52 `StructLayout`, and
/// a name that fit one and not the other would be a silent authoring trap.
pub const MAX_EAP_FIELD_NAME_LEN: usize = MAX_STRUCT_FIELD_NAME_LEN;
/// `FrameMatch.eq` (design.md §3 decision 59's `select_if`) — the literal
/// byte run a frame is selected by. Sized for a four-byte format magic
/// (`GWF1`, `BSS\x03`) with headroom.
pub const MAX_SELECT_MATCH_LEN: usize = 8;
/// `ProtocolDef.session` — named integer variables one protocol run carries
/// (design.md §3 decision 60). Integers only: the `bytes` session variable
/// the draft carried is gone with `++`.
pub const MAX_SESSION_VARS: usize = 6;
/// `SessionVarDef.name` — referenced as `session.<name>`.
pub const MAX_SESSION_VAR_NAME_LEN: usize = 24;
/// `ProtocolDef.states` — named states in one protocol's machine, terminal
/// states included. The real BDS download uses six.
pub const MAX_STATES_PER_PROTOCOL: usize = 12;
/// `StateDef.name`, and `ProtocolOutcome.final_state` (design.md §3
/// decision 62) — the one string a protocol run reports back.
pub const MAX_STATE_NAME_LEN: usize = 24;
/// `ActiveState.on_event` — distinct frames one state reacts to.
pub const MAX_EVENT_ARMS_PER_STATE: usize = 4;
/// `EventArm.when` — guarded transitions in one arm, first match winning.
/// More than one so an author can express a small dispatch without
/// inventing intermediate states; small enough that a real branch stays
/// legible.
pub const MAX_GUARDS_PER_ARM: usize = 2;
/// `EventArm.remember` — session-variable updates one arm performs before
/// its guards are evaluated.
pub const MAX_REMEMBER_PER_ARM: usize = 2;
/// `WriteAction.fields` — typed fields one `on_enter` write assembles
/// (design.md §3 decision 61). A control-point write is a one-byte opcode
/// and occasionally an argument; this is sized for the argument.
pub const MAX_WRITE_FIELDS: usize = 6;
