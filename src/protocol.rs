//! `DevBenchMessage` — the Core↔dev-bench serial wire protocol.
//!
//! design.md §3 decisions 10, 12, 20. COBS-framed, postcard-encoded
//! (framing/encoding themselves are transport concerns handled by whichever
//! component owns the serial port, not this crate — §3 decision 2). Versioned
//! by appending variants only, never reordering or removing one, so
//! postcard's varint enum discriminant stays wire-compatible across additions.

use heapless::{String, Vec};
use serde::{Deserialize, Serialize};

use crate::limits::{
    MAX_FIRMWARE_VERSION_LEN, MAX_LOG_LINE_LEN, MAX_STREAMS_PER_STUDY,
    MAX_STREAM_RECORDS_PER_BATCH,
};
use crate::result::StepResult;
use crate::streams::{StreamRecord, StreamTap};

/// Every message dev-bench sends or receives. Append-only (design.md §3
/// decision 10) — do not reorder or remove variants once this ships.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DevBenchMessage {
    /// Sent by Core when it opens the serial port, before any `Study`
    /// traffic. Unconditionally tells dev-bench to abort whatever it's
    /// currently running and clear its execution state (design.md §3
    /// decision 12/16) — a hard reset, not just a version check.
    Hello {
        schema_version: u32,
        /// Core's current UTC time in milliseconds since the epoch.
        /// Dev-bench's only clock source — it seeds/resyncs its own UTC
        /// offset from this on every `Hello` (design.md §3 decision 12),
        /// which is what makes `Sample::rx_utc_ms` meaningful.
        host_utc_ms: u64,
    },
    HelloAck {
        schema_version: u32,
        compatible: bool,
        /// Identifies which dev-bench firmware build replied (embarch-dev-bench/design.md
        /// §3 decision 18) — e.g. `git describe` output, a board ID, a build timestamp, or
        /// some combination; the exact contents are dev-bench build-tooling's own concern.
        firmware_version: String<MAX_FIRMWARE_VERSION_LEN>,
    },
    /// Opens the tap whose `id` this is — its own index in `Study.streams`
    /// (design.md §3 decision 39, §4.8). Carries no `step_index`: when a
    /// tap opens is a property of its declared
    /// [`StreamScope`](crate::streams::StreamScope), not of the wire.
    ///
    /// **Slot reuse, stated rather than buried.** This variant occupies the
    /// discriminant the retired `StreamStart` had, as `StreamChunkBatch`
    /// and `StreamClose` below do for `StreamChunk`/`StreamEnd`. Decision
    /// 10's append-only rule is about additions to a shipped protocol;
    /// decision 39 retires these three outright, and the `Hello`/`HelloAck`
    /// schema-version handshake refusing a v7 peer is what makes reusing
    /// their slots safe. No dev-bench firmware carrying the old shapes has
    /// ever been flashed, which is what made the reshape cheap at all.
    StreamOpen { id: u8 },
    /// A batch of arrival-stamped byte records for one open tap.
    ///
    /// **Bytes, never decoded values** — this is the whole of decision 39.
    /// The old `StreamChunk`/`StreamChunkBatch` pair each carried a `Sample`
    /// (one `f32` plus a unit), which is why a raw payload, a direction, or
    /// a pair of UUIDs had nowhere to go and every new capture kind grew its
    /// own message class. What the bytes mean is declared once, by the tap's
    /// [`StreamEncoding`](crate::streams::StreamEncoding), and resolved
    /// host-side.
    StreamChunkBatch {
        id: u8,
        records: Vec<StreamRecord, MAX_STREAM_RECORDS_PER_BATCH>,
    },
    /// Closes the tap whose `id` this is. `dropped` is how many records the
    /// producer lost — **a stream that lost data says so** rather than
    /// presenting a shorter, plausible capture as complete.
    StreamClose { id: u8, dropped: u32 },
    /// Dev-bench's own log output (embarch-dev-bench/design.md §3 decision 7) — travels as
    /// a properly-framed message like everything else on this link rather than as raw
    /// interleaved bytes on the shared serial line, which would corrupt COBS framing.
    LogLine { text: String<MAX_LOG_LINE_LEN> },
    /// Sent by Core exactly once, immediately after the `Hello`/`HelloAck`
    /// handshake completes — the whole `Study.steps` vector transferred in
    /// one postcard-encoded message rather than streamed step-by-step
    /// (design.md §3 decision 24). `steps_crc` travels here rather than on
    /// `Hello` (design.md §3 decision 17's integrity seal), since `Hello`
    /// itself precedes any `Study` being submitted.
    StudyStart {
        steps: crate::bounded::StepList,
        steps_crc: u32,
        /// The study's declared taps (design.md §3 decision 39, §4.8).
        ///
        /// This is the one part of a `Study` beyond `steps` that dev-bench
        /// genuinely needs: four of the five
        /// [`StreamSource`](crate::streams::StreamSource) variants are
        /// dev-bench-mediated, so dev-bench has to know which taps to open,
        /// which characteristic to subscribe for a `GattNotify` tap, and
        /// which `id` each one answers to on `StreamOpen`/
        /// `StreamChunkBatch`/`StreamClose`.
        ///
        /// `Study.validations` and `Study.requires` still never cross this
        /// hop (design.md §3 decisions 17, 40) — neither is anything
        /// dev-bench could act on. **`steps_crc` still seals `steps`
        /// alone**; `streams` has its own sibling seal, `streams_crc`
        /// below.
        streams: Vec<StreamTap, MAX_STREAMS_PER_STUDY>,
        /// CRC-32 over `streams`, design.md §3 decision 39's 2026-08-25
        /// amendment — [`crate::crc::streams_crc`], checked here
        /// independently of `steps_crc` exactly as `steps_crc` is.
        ///
        /// Carried *after* `streams` rather than beside `steps_crc` so each
        /// seal immediately follows the one contiguous span it covers.
        /// That is the whole reason this is a second CRC rather than a
        /// widened one: `steps_crc` sits between `steps` and `streams`, so
        /// widening it would mean digesting two non-contiguous spans in
        /// dev-bench's hand-written C, or reshuffling this message's field
        /// order — a reshape where an append will do.
        streams_crc: u32,
    },
    /// Sent by dev-bench as each step completes, streaming results back
    /// incrementally rather than batched at the end (design.md §3 decision
    /// 24). `step_index` correlates back to `StudyStart.steps`' array
    /// position (design.md §3 decision 14).
    StepResult {
        step_index: u32,
        result: StepResult,
    },
    /// Sent by dev-bench exactly once, after the last step it actually ran
    /// (design.md §3 decision 24) — `completed` distinguishes a `Study` that
    /// ran to its natural end from one aborted early by a failing step with
    /// `continue_on_fail: false`.
    StudyDone { completed: bool },
    // The old batched `StreamChunkBatch` (design.md §3 decision 25) and
    // `GattTranscriptRecord` (decision 36) were the last two variants here
    // and are both **retired** by decision 39. `StreamChunk` and
    // `StreamChunkBatch` had been live simultaneously, handled by Core in
    // two separate match arms — a duplication decision 25 was believed to
    // have removed — and are now genuinely one message. The GATT transcript
    // keeps every part of itself that mattered (its record type, its
    // both-directions coverage, its uncapped streaming, its `gatt.csv`
    // columns) as `StreamSource::GattTranscript` +
    // `StreamEncoding::GattTranscript`; only its dedicated message class is
    // gone.
}
