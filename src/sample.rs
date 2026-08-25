//! `Sample` — streamed power/waveform data, design.md §4.7.

use core::fmt::Write as _;

use heapless::String;
use serde::{Deserialize, Serialize};

use crate::limits::MAX_CSV_ROW_LEN;

/// One decoded scalar sample. `data.csv` and `waveform.csv` (design.md §5.2)
/// share this identical row shape rather than each inventing their own
/// columns.
///
/// **No longer a wire type as of schema v8** (design.md §3 decision 39): the
/// wire carries arrival-stamped bytes
/// ([`StreamRecord`](crate::streams::StreamRecord)), and a `Sample` is what
/// [`samples_in`](crate::streams::samples_in) produces from those bytes
/// against a tap's declared
/// [`StreamEncoding::Samples`](crate::streams::StreamEncoding::Samples)
/// layout. The row shape below is deliberately untouched by that move — the
/// CSV columns survive the reshape unchanged, only the path the numbers take
/// to reach them changed.
///
/// `rx_utc_ms` is the record's own arrival stamp, taken by whichever node
/// received the bytes — dev-bench's local clock for a dev-bench-mediated tap,
/// seeded and periodically resynced from Core's `host_utc_ms` on every
/// `Hello` (design.md §3 decision 12); Core's own clock for a
/// `StreamSource::Signal` tap it reads directly.
///
/// `value`'s real-world shape (a single scalar vs. multiple hardware-specific
/// fields, e.g. separate current/voltage) is still open per design.md §7;
/// appending fields later is a version-bumped wire change like any other.
///
/// `unit`/`channel_id` (design.md §3 decision 27) disambiguate `value`: which
/// physical quantity it is, and which of possibly several concurrent
/// hardware channels it came from (e.g. more than one power rail sampled at
/// once). Both are declared once per tap now, on `StreamEncoding::Samples`,
/// rather than repeated per wire message.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub rx_utc_ms: u64,
    pub value: f32,
    pub unit: Unit,
    pub channel_id: u8,
}

/// The physical quantity `Sample::value` is measured in (design.md §3
/// decision 27). Append-only, same wire-compatibility rule as
/// `DevBenchMessage` (design.md §3 decision 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Milliamps,
    Volts,
    Milliwatts,
    Raw,
}

impl Unit {
    /// This `Unit`'s CSV cell rendering: its variant name, lowercased, with
    /// no quoting (design.md §4.7 leaves the exact formatting to this crate;
    /// lowercasing the variant name is the simplest choice consistent with
    /// how `to_csv_row` renders every other field verbatim).
    const fn as_csv_str(self) -> &'static str {
        match self {
            Unit::Milliamps => "milliamps",
            Unit::Volts => "volts",
            Unit::Milliwatts => "milliwatts",
            Unit::Raw => "raw",
        }
    }
}

impl Sample {
    /// Header row for `data.csv`/`waveform.csv` (design.md §5.2), written
    /// once per file.
    pub const fn csv_header() -> &'static str {
        "rx_utc_ms,step_name,value,unit,channel_id"
    }

    /// Renders this sample as one CSV row, given the step name Core already
    /// knows from the currently-open stream (design.md §3 decision 20). This
    /// is the crate's own CSV-rendering tool (§1, §3 decision 2): Core's job
    /// writing `data.csv`/`waveform.csv` is to run a `StreamRecord` through
    /// [`samples_in`](crate::streams::samples_in), call this on each
    /// `Sample`, and append the result — no column or layout knowledge lives
    /// in Core itself.
    ///
    /// Returns `None` if `step_name` doesn't fit alongside the rest of the
    /// row within `MAX_CSV_ROW_LEN` — the caller (Core) is expected to log
    /// and skip such a row rather than truncate it silently, since a
    /// truncated CSV row is worse than a dropped one.
    pub fn to_csv_row(&self, step_name: &str) -> Option<String<MAX_CSV_ROW_LEN>> {
        let mut row = String::new();
        write!(
            row,
            "{},{},{},{},{}",
            self.rx_utc_ms,
            step_name,
            self.value,
            self.unit.as_csv_str(),
            self.channel_id
        )
        .ok()?;
        Some(row)
    }
}
