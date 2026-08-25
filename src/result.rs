//! `StudyResult`/`StepResult`/`Outcome` — design.md §4.5.

use heapless::{String, Vec};
use serde::{Deserialize, Serialize};

use crate::gatt::{GattActivityRecord, GattServiceInfo};
use crate::limits::{
    MAX_DISCOVERED_SERVICES, MAX_FAIL_REASON_LEN, MAX_FIRMWARE_VERSION_LEN,
    MAX_GATT_ACTIVITY_RECORDS, MAX_NAME_LEN, MAX_PAYLOAD_LEN, MAX_STEPS_PER_STUDY,
    MAX_STREAMS_PER_STUDY, MAX_STUDY_NAME_LEN, MAX_VALIDATIONS_PER_STUDY,
};
use crate::streams::StreamRef;
use crate::validation::ValidationResult;

/// The aggregate outcome of running a `Study`. `steps` is a proper prefix of
/// the submitted `Study.steps` when a step with `continue_on_fail: false`
/// (the default) fails — not guaranteed to cover every submitted step.
/// `validations` is populated only once the study reaches `"completed"`
/// status; it stays empty on any `"failed"` outcome. design.md §3 decision 19.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StudyResult {
    pub study_name: String<MAX_STUDY_NAME_LEN>,
    pub steps: Vec<StepResult, MAX_STEPS_PER_STUDY>,
    pub validations: Vec<ValidationResult, MAX_VALIDATIONS_PER_STUDY>,
    /// What this run actually executed against, and how each version was
    /// established (design.md §3 decision 40, §4.5). Closes a gap wider than
    /// the one it was raised for: before this, two runs of the same study
    /// against two different firmware builds produced results that were
    /// indistinguishable after the fact.
    pub provenance: Provenance,
    /// One entry per declared tap (design.md §3 decision 39, §4.8) —
    /// replaces `StepResult`'s retired `power_samples_ref`/`waveform_ref`,
    /// which could not describe a capture whose scope outlives one step.
    pub streams: Vec<StreamRef, MAX_STREAMS_PER_STUDY>,
}

/// What a `StudyResult` ran against, and **how each version was
/// established** (design.md §3 decision 40, §4.5).
///
/// The source fields are not bookkeeping. A `Declared` DUT version is an
/// assertion nobody checked; a result that rendered it identically to a
/// verified one would reintroduce exactly the mislabelling this type exists
/// to close.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub dev_bench_version: String<MAX_FIRMWARE_VERSION_LEN>,
    pub firmware_version: String<MAX_FIRMWARE_VERSION_LEN>,
    pub dev_bench_source: VersionSource,
    pub firmware_source: VersionSource,
}

/// How a version in [`Provenance`] was established (design.md §3 decision
/// 40). Append-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VersionSource {
    /// dev-bench said so over `HelloAck` — Core read it off the live link.
    ReportedByDevBench,
    /// The DUT's own outpost stream header carried a build ID
    /// (`embarch-outpost/design.md` §3 decision 9).
    ReportedByOutpost,
    /// This run flashed it, so Core knows what it put there — the only way
    /// to *know* a DUT's firmware version rather than assert it.
    FlashedThisRun,
    /// Asserted, unverified. Render this visibly weaker than the three
    /// above (`embarch-ui/design.md` §3 decision 11); never as a fact.
    Declared,
}

impl VersionSource {
    /// Whether something actually observed this version, as opposed to it
    /// having been asserted.
    pub const fn is_verified(self) -> bool {
        !matches!(self, VersionSource::Declared)
    }
}

/// `step_name` is a denormalized copy of `Step.name`, carried purely for
/// human readability — never used for machine correlation (design.md §3
/// decision 14).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepResult {
    pub step_name: String<MAX_NAME_LEN>,
    pub outcome: Outcome,
    /// Bytes a `DataExchange` read/notify/indicate step pulled off the DUT;
    /// unused for other action kinds.
    pub captured_data: Option<Vec<u8, MAX_PAYLOAD_LEN>>,
    // `power_samples_ref`/`waveform_ref` were here and are **retired** by
    // design.md §3 decision 39: a capture is a property of the study's
    // declared taps, not of one step, so what a run captured is reported
    // once as `StudyResult::streams` rather than as two optional
    // per-step file references that no tap outliving a single step could
    // ever have filled in correctly.
    /// Populated by `Action::GattDiscover` and `Action::GattMonitorAll`
    /// (design.md §3 decisions 31/32, §4.3a). Landing inline in
    /// `events.json` like `captured_data` rather than as a CSV-file
    /// reference, since both are bounded and small enough to stay
    /// JSON-friendly (unlike the high-rate power/waveform channels).
    /// `#[serde(default)]` so a `Study`/`StudyResult` JSON predating this
    /// field still deserializes.
    #[serde(default)]
    pub gatt_services: Option<Vec<GattServiceInfo, MAX_DISCOVERED_SERVICES>>,
    /// Populated only by `Action::GattMonitorAll` (design.md §3 decision 32).
    #[serde(default)]
    pub gatt_activity: Option<Vec<GattActivityRecord, MAX_GATT_ACTIVITY_RECORDS>>,
}

/// The only on-device validation signal — did the action complete without a
/// protocol-level error or timeout. Whether the *content* was correct is a
/// separate, Core-side, post-hoc question (design.md §3 decision 19, §4.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Outcome {
    Pass,
    Fail { reason: String<MAX_FAIL_REASON_LEN> },
    TimedOut,
}
