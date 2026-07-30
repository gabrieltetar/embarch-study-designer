//! `DevBenchMessage` — the Core↔dev-bench serial wire protocol.
//!
//! design.md §3 decisions 10, 12, 20. COBS-framed, postcard-encoded
//! (framing/encoding themselves are transport concerns handled by whichever
//! component owns the serial port, not this crate — §3 decision 2). Versioned
//! by appending variants only, never reordering or removing one, so
//! postcard's varint enum discriminant stays wire-compatible across additions.

use heapless::String;
use serde::{Deserialize, Serialize};

use crate::limits::{MAX_FIRMWARE_VERSION_LEN, MAX_LOG_LINE_LEN};
use crate::sample::Sample;

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
        /// design.md §3 decision 17's integrity seal, folded into this
        /// handshake rather than a separate message.
        steps_crc: u32,
    },
    HelloAck {
        schema_version: u32,
        compatible: bool,
        /// Identifies which dev-bench firmware build replied (embarch-dev-bench/design.md
        /// §3 decision 18) — e.g. `git describe` output, a board ID, a build timestamp, or
        /// some combination; the exact contents are dev-bench build-tooling's own concern.
        firmware_version: String<MAX_FIRMWARE_VERSION_LEN>,
    },
    /// Opens a continuous capture channel for a step. More than one channel
    /// can be open on the same step concurrently.
    StreamStart {
        step_index: u32,
        channel: StreamChannel,
    },
    /// One `Sample`, belonging to whichever channel was most recently opened
    /// by a `StreamStart` for it — carries no `step_index`/`channel` of its
    /// own, keeping per-chunk overhead minimal on a UART link.
    StreamChunk { sample: Sample },
    StreamEnd {
        step_index: u32,
        channel: StreamChannel,
    },
    /// Dev-bench's own log output (embarch-dev-bench/design.md §3 decision 7) — travels as
    /// a properly-framed message like everything else on this link rather than as raw
    /// interleaved bytes on the shared serial line, which would corrupt COBS framing.
    LogLine { text: String<MAX_LOG_LINE_LEN> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamChannel {
    Power,
    SensorWaveform,
}
