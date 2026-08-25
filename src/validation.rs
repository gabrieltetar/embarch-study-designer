//! Post-hoc validation types — design.md §4.6, §3 decision 19.
//!
//! Split from the real-time, device-observed `Outcome` (§4.5): these types
//! answer "was the captured data actually correct", evaluated by Core only
//! if/when a `Study` reaches `"completed"` status. Never transmitted to
//! dev-bench (§3 decision 17) — `Study.validations` has nothing to do with it.

use heapless::Vec;
use serde::{Deserialize, Serialize};

use crate::limits::{MAX_FAIL_REASON_LEN, MAX_PAYLOAD_LEN, MAX_STREAM_NAME_LEN};

/// One entry per post-hoc check an author wants run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostHocValidation {
    pub source: ValidationSource,
    pub check: PostHocCheck,
}

/// What a post-hoc check reads — design.md §4.6, reshaped 2026-08-25 by §3
/// decision 19's amendment.
///
/// Two shapes, because there are two genuinely different things to address:
/// data that belongs to one step and lands inline in `events.json`, and data
/// that belongs to a declared [`StreamTap`](crate::streams::StreamTap) whose
/// `scope` may outlive any step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidationSource {
    /// One step's own inline result data. No ordering constraint on
    /// `step_index` (design.md §3 decision 14) — evaluation only ever
    /// happens after the whole study has finished.
    Step { step_index: u32, channel: DataChannel },
    /// One declared tap's captured stream, named by
    /// [`StreamTap::name`](crate::streams::StreamTap) — which is literally
    /// what §4.8 already meant by a tap's name being "the post-hoc
    /// validation source".
    ///
    /// **Carries no `step_index`, deliberately.** When a tap is open is a
    /// property of its declared
    /// [`StreamScope`](crate::streams::StreamScope), not of a step — the
    /// same thing the wire already says by carrying no `step_index` on
    /// `StreamOpen`/`StreamClose`. This also gives `Raw`/`Text`/
    /// `OutpostTrace` taps a validation target, which no [`DataChannel`]
    /// variant ever had.
    Tap { name: heapless::String<MAX_STREAM_NAME_LEN> },
}

/// The per-step data channels — the two that really are per-step and land
/// inline in `events.json`.
///
/// **`PowerSamples`, `SensorWaveform`, and `GattTranscript` were here** and
/// are retired by §3 decision 19's 2026-08-25 amendment. Each named one of
/// the three fixed CSV files Phase B replaces with `streams/<tap name>`
/// (`embarch-core/design.md` §3 decision 30), so leaving them would have
/// left three variants pointing at files nothing writes. They become tap
/// names — [`ValidationSource::Tap`] above.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataChannel {
    /// A `DataExchange` step's `StepResult.captured_data`.
    CapturedData,
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
///
/// Carries the whole [`ValidationSource`] rather than a flattened
/// `step_index`/`channel` pair (which is what §4.6 described before decision
/// 19's 2026-08-25 amendment): a tap-sourced check has no `step_index` to
/// report, and inventing one for the result of a check that didn't have one
/// is exactly the mislabelling the amendment removed from the source side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationResult {
    pub source: ValidationSource,
    pub result: ContentValidity,
}
