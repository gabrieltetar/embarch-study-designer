//! `STUDY_DESIGNER_SCHEMA_VERSION` — design.md §3 decision 12.
//!
//! Bumped by hand only when a wire-relevant type in this crate changes,
//! independent of `Cargo.toml`'s own semver. Checked at both connection
//! points: the Core<->dev-bench serial `Hello`/`HelloAck` handshake
//! ([`crate::protocol::DevBenchMessage`]), and Core's `/status` HTTP
//! response (a field `embarch-core` adds itself, outside this crate).

/// Bump this alongside any change to a type in [`crate::study`],
/// [`crate::result`], [`crate::validation`], [`crate::sample`], or
/// [`crate::protocol`] that changes its wire representation.
///
/// v2 (embarch-dev-bench/design.md §3 decisions 7/18): added
/// `DevBenchMessage::LogLine` and `HelloAck.firmware_version`.
///
/// v3 (decisions 24/25/27): `Hello` loses `steps_crc` (moved to
/// `StudyStart`); added `DevBenchMessage::StudyStart`/`StepResult`/
/// `StudyDone`/`StreamChunkBatch`; `Sample` gained `unit`/`channel_id`.
///
/// v4 (decisions 31/32): added `Action::GattDiscover`/`GattMonitorAll` and
/// the matching `StepResult.gatt_services`/`gatt_activity` fields (§4.3a).
/// One bump covering both new `Action` variants and both new `StepResult`
/// fields together, same one-bump-per-pass discipline v2's
/// `LogLine`+`firmware_version` pairing already established.
/// v5 (decision 36): added `Action::GattMonitorStart`/`GattMonitorStop` (a
/// capture window that outlives its own step), `DevBenchMessage::
/// GattTranscriptRecord`, the `GattTranscriptEntry`/`GattDirection`/
/// `GattEventKind` types it carries (§4.3b), and `DataChannel::
/// GattTranscript`. Same one-bump-per-pass discipline as v4.
///
/// v6 (decision 42): `Step` gained a trailing `delay_before_ms` — the "when"
/// half of authoring a stimulus, and the first `Step` field added since this
/// constant existed. Appended rather than inserted precisely because
/// postcard carries no field names and dev-bench hand-decodes `Step` in C,
/// but appended is still a wire change: a v5 decoder reads a v6 `Step` and
/// then finds an unconsumed trailing varint, and a v6 decoder runs off the
/// end of a v5 `Step`. The `Hello`/`HelloAck` handshake rejecting the
/// mismatch outright is the point.
///
/// Note what did *not* bump this: `vendor.rs` (decision 41) is a table of
/// compile-time constants with no wire representation of its own — a
/// vendor-defined characteristic resolves into an ordinary
/// `Action::DataExchange` carrying plain UUIDs before anything is encoded,
/// so dev-bench firmware never needs to know the table exists.
///
/// v7 (decision 43): `Action::BleConnect` gained a trailing `target_name`,
/// so a study can name the DUT it means instead of taking whichever
/// peripheral advertises first. Same append-don't-insert discipline as v6,
/// and the same reason it's still a wire break.
///
/// v8 (decisions 39 **and** 40, one bump for the pair — neither had shipped,
/// and one bump per pass is the standing discipline). The largest single
/// wire change since this constant existed, and the first that *removes*
/// rather than appends:
///
/// - Decision 39, one generic inbound stream pipeline. `Study` gained
///   `streams: Vec<StreamTap, _>` (§4.8) and `StudyResult` gained
///   `streams: Vec<StreamRef, _>`. `DevBenchMessage`'s four stream variants
///   collapsed into the `StreamOpen`/`StreamChunkBatch`/`StreamClose`
///   triple, carrying `{ rx_utc_ms, bytes }` records instead of `Sample`s —
///   `StreamChunk` and `StreamChunkBatch` had both been live, and both
///   handled by Core in separate match arms, a duplication decision 25 was
///   believed to have removed. `StreamChannel`,
///   `DevBenchMessage::GattTranscriptRecord`,
///   `GattOperation::StreamCapture`, and `StepResult`'s
///   `power_samples_ref`/`waveform_ref` are all retired. `StudyStart` gained
///   `streams`, because four of the five sources are dev-bench-mediated and
///   dev-bench has to know which taps to open.
///
///   **The `data.csv`/`waveform.csv`/`gatt.csv` row shapes are unchanged** —
///   they survive as declared *encodings* over a generic tap rather than as
///   their own message classes, which is the whole of what "collapse the
///   pipelines" meant. `GattTranscriptEntry` and its CSV renderer are
///   untouched.
///
///   The three retired stream variants' discriminants (2, 3, 4) are reused
///   by the new triple rather than left as holes: postcard encodes a serde
///   enum by declaration index, so a hole would need a placeholder variant
///   that exists only to be un-constructible. Reuse is safe here for the
///   same reason the whole reshape was affordable — the `Hello`/`HelloAck`
///   handshake refuses a version mismatch outright, and no dev-bench
///   firmware carrying the old shapes has ever been flashed.
///
/// - Decision 40, versions a study declares and a result records. `Study`
///   gained `requires: Requirements { dev_bench_version, firmware_version }`
///   (both mandatory, `"any"` an explicit legal value, **host-side only —
///   it never crosses the wire to dev-bench**, exactly as `validations`
///   doesn't), and `StudyResult` gained
///   `provenance: Provenance { .., dev_bench_source, firmware_source }`
///   recording *how* each version was established.
///
/// **Note this is v8, not the v6 decisions 39 and 40 were written against.**
/// Both were recorded on 2026-08-25 against a v5 crate; decisions 42 and 43
/// then landed first and took v6 and v7. The substance is unchanged — one
/// bump, covering both decisions together — only the arithmetic moved.
pub const STUDY_DESIGNER_SCHEMA_VERSION: u32 = 8;
