//! `StudyResult`/`StepResult`/`Outcome` — design.md §4.5.

use heapless::{String, Vec};
use serde::{Deserialize, Serialize};

use crate::gatt::{GattActivityRecord, GattServiceInfo};
use crate::limits::{
    MAX_DISCOVERED_SERVICES, MAX_FAIL_REASON_LEN, MAX_GATT_ACTIVITY_RECORDS, MAX_NAME_LEN,
    MAX_PAYLOAD_LEN, MAX_RESULT_REF_LEN, MAX_STEPS_PER_STUDY, MAX_STUDY_NAME_LEN,
    MAX_VALIDATIONS_PER_STUDY,
};
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
    /// A reference into `data.csv` (design.md §5.2), not raw samples inline.
    pub power_samples_ref: Option<String<MAX_RESULT_REF_LEN>>,
    /// A reference into `waveform.csv` (design.md §5.2); populated only for
    /// a `GattOperation::StreamCapture` step.
    pub waveform_ref: Option<String<MAX_RESULT_REF_LEN>>,
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
