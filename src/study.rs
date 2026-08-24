//! `Study`/`Step`/`Action`/`PowerSampleWindow` — design.md §4.1-§4.4.

use heapless::{String, Vec};
use serde::{Deserialize, Serialize};

use crate::ids::{BleAddress, Uuid};
use crate::limits::{
    MAX_LOCAL_NAME_LEN, MAX_NAME_LEN, MAX_PAYLOAD_LEN, MAX_SERVICE_UUIDS, MAX_STEPS_PER_STUDY,
    MAX_STUDY_NAME_LEN, MAX_VALIDATIONS_PER_STUDY,
};
use crate::validation::PostHocValidation;

/// design.md §4.1. `steps_crc` is design.md §3 decision 17's integrity seal
/// over `steps` specifically — see [`crate::crc::steps_crc`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Study {
    /// Human-readable identifier; not required to be unique.
    pub name: String<MAX_STUDY_NAME_LEN>,
    /// Run in order. Entirely static once submitted for v1.
    pub steps: Vec<Step, MAX_STEPS_PER_STUDY>,
    /// Never transmitted to dev-bench (§3 decision 17) — Core-only,
    /// evaluated post-hoc once the study reaches `"completed"` (§4.6).
    pub validations: Vec<PostHocValidation, MAX_VALIDATIONS_PER_STUDY>,
    /// CRC-32 over `steps` (design.md §3 decision 17), computed by whoever
    /// submits this `Study` via [`crate::crc::steps_crc`].
    pub steps_crc: u32,
}

/// design.md §4.2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// Label surfaced in results (`StepResult.step_name`); never used for
    /// machine correlation (design.md §3 decision 14 uses array position).
    pub name: String<MAX_NAME_LEN>,
    pub action: Action,
    /// Max wall-clock time dev-bench allows this step before reporting
    /// `Outcome::TimedOut`.
    pub timeout_ms: u32,
    /// Optional power measurement bounded by this step's own start/end (or
    /// `timeout_ms`, whichever comes first).
    pub power_sample: Option<PowerSampleWindow>,
    /// `false` (default) aborts the `Study` on this step's `Fail`/`TimedOut`;
    /// `true` continues to the next step regardless. design.md §3 decision 13.
    #[serde(default)]
    pub continue_on_fail: bool,
}

/// design.md §4.3. Content validation is handled entirely post-hoc by Core
/// (§3 decision 19) — there is no on-device validation `Action` variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Action {
    BleAdvertise {
        local_name: Option<String<MAX_LOCAL_NAME_LEN>>,
        service_uuids: Vec<Uuid, MAX_SERVICE_UUIDS>,
        adv_interval_ms: u16,
    },
    BleConnect {
        role: BleRole,
        /// `None` accepts/connects to whichever DUT shows up first.
        target_address: Option<BleAddress>,
    },
    DataExchange {
        service_uuid: Uuid,
        characteristic_uuid: Uuid,
        operation: GattOperation,
    },
    /// Walks the connected DUT's entire GATT table via wildcard discovery
    /// (every primary service, every characteristic, each characteristic's
    /// raw ATT properties byte) rather than requiring a caller to already
    /// know a `service_uuid`/`characteristic_uuid` pair. Reports its result
    /// in `StepResult.gatt_services` (§4.3a); doesn't subscribe or capture
    /// anything itself. design.md §3 decision 31.
    GattDiscover {},
    /// Runs the same discovery as `GattDiscover` internally, then subscribes
    /// to every characteristic whose discovered properties include Notify or
    /// Indicate, then captures every notification/indication that arrives
    /// until the step's `timeout_ms` expires. Reports both `gatt_services`
    /// and `gatt_activity` (§4.3a) — self-sufficient, doesn't depend on a
    /// preceding `GattDiscover` step's result. design.md §3 decision 32.
    GattMonitorAll {},
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BleRole {
    Central,
    Peripheral,
}

/// design.md §4.3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GattOperation {
    Read,
    Write {
        payload: Vec<u8, MAX_PAYLOAD_LEN>,
    },
    /// Wait for a notification, bounded independently of the step's own
    /// `timeout_ms`.
    Notify {
        timeout_ms: u32,
    },
    /// Same wait-for-a-pushed-value shape as `Notify`, over BLE's
    /// acknowledged indication mechanism instead.
    Indicate {
        timeout_ms: u32,
    },
    /// Enable notifications/indications without waiting for one.
    Subscribe,
    /// Continuous capture of whatever the characteristic streams (e.g. a PPG
    /// waveform) for the duration of the step, landing in the
    /// `SensorWaveform` data channel rather than `StepResult.captured_data`.
    /// design.md §3 decisions 20/21.
    StreamCapture,
}

/// design.md §4.4. No separate duration field — bound by the step's own
/// `timeout_ms`/completion, so a window can't silently outlive or fall short
/// of the step it characterizes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PowerSampleWindow {
    pub sample_rate_hz: u32,
}
