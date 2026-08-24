//! Post-hoc validation types — design.md §4.6, §3 decision 19.
//!
//! Split from the real-time, device-observed `Outcome` (§4.5): these types
//! answer "was the captured data actually correct", evaluated by Core only
//! if/when a `Study` reaches `"completed"` status. Never transmitted to
//! dev-bench (§3 decision 17) — `Study.validations` has nothing to do with it.

use heapless::Vec;
use serde::{Deserialize, Serialize};

use crate::limits::{MAX_FAIL_REASON_LEN, MAX_PAYLOAD_LEN};

/// One entry per post-hoc check an author wants run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostHocValidation {
    pub source: ValidationSource,
    pub check: PostHocCheck,
}

/// Which step's data to check, and which of that step's data channels. No
/// ordering constraint on `step_index` (design.md §3 decision 14) —
/// evaluation only ever happens after the whole study has finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationSource {
    pub step_index: u32,
    pub channel: DataChannel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataChannel {
    /// A `DataExchange` step's `StepResult.captured_data`.
    CapturedData,
    /// A step's `PowerSampleWindow` output, streamed into `data.csv`.
    PowerSamples,
    /// A `GattOperation::StreamCapture` step's output, streamed into
    /// `waveform.csv` (design.md §3 decision 21).
    SensorWaveform,
    /// A `GattMonitorAll` step's `StepResult.gatt_activity` (design.md §3
    /// decision 32). Added alongside that decision for future use — no
    /// `PostHocCheck` against it is authored yet (Milestone 3's own
    /// Definition of Done only requires the data to land in `events.json`,
    /// not a content assertion against it).
    GattActivity,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PostHocCheck {
    Simple(ExpectedValue),
    Signal(SignalCheck),
}

/// Byte-level check, unchanged in shape from the removed `Validate` `Action`
/// (design.md §3 decision 22) — the natural fit for `DataChannel::CapturedData`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExpectedValue {
    Equals(Vec<u8, MAX_PAYLOAD_LEN>),
    Contains(Vec<u8, MAX_PAYLOAD_LEN>),
    InRange { min: f32, max: f32 },
}

/// A growing, `serde`-derived enum of richer check kinds needed for
/// time-series/waveform data, which byte equality can't express.
/// Illustrative placeholder set (design.md §7) — append-only, same
/// discipline as `DevBenchMessage` (§3 decision 10). Every consumer can
/// serialize/deserialize/display any variant; only the `core-validation`
/// feature (§3 decision 19, [`crate::signal`]) evaluates one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SignalCheck {
    MeanInRange { min: f32, max: f32 },
    NoGlitchAbove { threshold: f32 },
    FftPeakNear { hz: f32, tolerance_hz: f32 },
}

/// Deliberately not a reuse of `Outcome` — `TimedOut` has no meaning for a
/// desktop-side comparison run well after the study finished.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ContentValidity {
    Valid,
    Invalid { reason: heapless::String<MAX_FAIL_REASON_LEN> },
}

/// One per `Study.validations` entry, landing in `StudyResult.validations`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationResult {
    pub step_index: u32,
    pub channel: DataChannel,
    pub result: ContentValidity,
}
