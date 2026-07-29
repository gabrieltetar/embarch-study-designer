//! `Sample` — streamed power/waveform data, design.md §4.7.

use core::fmt::Write as _;

use heapless::String;
use serde::{Deserialize, Serialize};

use crate::limits::MAX_CSV_ROW_LEN;

/// The one wire-level record type carried by both `StreamChannel::Power` and
/// `StreamChannel::SensorWaveform`. `data.csv` and `waveform.csv` (design.md
/// §5.2) share this identical row shape rather than each inventing their own
/// columns.
///
/// `rx_utc_ms` is stamped by dev-bench itself, at the moment it captures the
/// sample from its own sampling hardware — its own local clock, seeded and
/// periodically resynced from Core's `host_utc_ms` on every `Hello` (design.md
/// §3 decision 12) — not assigned by Core on arrival.
///
/// `value`'s real-world shape (a single scalar vs. multiple hardware-specific
/// fields, e.g. separate current/voltage) is still open per design.md §7;
/// appending fields later is a version-bumped wire change like any other.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub rx_utc_ms: u64,
    pub value: f32,
}

impl Sample {
    /// Header row for `data.csv`/`waveform.csv` (design.md §5.2), written
    /// once per file.
    pub const fn csv_header() -> &'static str {
        "rx_utc_ms,step_name,value"
    }

    /// Renders this sample as one CSV row, given the step name Core already
    /// knows from the currently-open stream (design.md §3 decision 20). This
    /// is the crate's own CSV-rendering tool (§1, §3 decision 2): Core's job
    /// writing `data.csv`/`waveform.csv` is to decode a `StreamChunk` into a
    /// `Sample`, call this, and append the result — no column knowledge lives
    /// in Core itself.
    ///
    /// Returns `None` if `step_name` doesn't fit alongside the rest of the
    /// row within `MAX_CSV_ROW_LEN` — the caller (Core) is expected to log
    /// and skip such a row rather than truncate it silently, since a
    /// truncated CSV row is worse than a dropped one.
    pub fn to_csv_row(&self, step_name: &str) -> Option<String<MAX_CSV_ROW_LEN>> {
        let mut row = String::new();
        write!(row, "{},{},{}", self.rx_utc_ms, step_name, self.value).ok()?;
        Some(row)
    }
}
