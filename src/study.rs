//! `Study`/`Step`/`Action` — design.md §4.1-§4.3.
//!
//! §4.4's `PowerSampleWindow` was here and is **retired** by §3 decision
//! 39's 2026-08-25 amendment: a `StreamSource::PowerFrontEnd { sample_hz }`
//! tap scoped to a step range says the same thing, and was already the only
//! one of the two anything read.

use heapless::{String, Vec};
use serde::{Deserialize, Serialize};

use crate::ids::{BleAddress, Uuid};
use crate::limits::{
    MAX_FIRMWARE_VERSION_LEN, MAX_LOCAL_NAME_LEN, MAX_NAME_LEN, MAX_PAYLOAD_LEN, MAX_SERVICE_UUIDS,
    MAX_STREAMS_PER_STUDY, MAX_STUDY_NAME_LEN,
};
use crate::streams::StreamTap;

/// design.md §4.1. Sealed by two sibling CRCs: `steps_crc` over `steps`
/// (design.md §3 decision 17) and `streams_crc` over `streams` (decision
/// 39's 2026-08-25 amendment) — see [`crate::crc`] for why there are two
/// rather than one widened one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Study {
    /// Human-readable identifier; not required to be unique.
    pub name: String<MAX_STUDY_NAME_LEN>,
    /// The builds this study is meant to run against (design.md §3 decision
    /// 40, §4.1). **Host-side only — never transmitted to dev-bench**,
    /// dev-bench has no use for a
    /// requirement it cannot check about itself, and `steps_crc` seals what
    /// dev-bench actually executes, which is unchanged.
    ///
    /// Mandatory, with no `#[serde(default)]` on purpose: "I don't care
    /// which build" is a real and legitimate answer — a dev-bench self-test
    /// involves no DUT at all — but it has to be *said*
    /// ([`REQUIREMENT_ANY`]), not achieved by leaving a field out, because
    /// the failure this exists to prevent is precisely the one where nobody
    /// thought about it.
    pub requires: Requirements,
    /// Run in order. Entirely static once submitted for v1.
    pub steps: crate::bounded::StepList,
    /// Declared capture channels for this study (design.md §3 decision 39,
    /// §4.8) — the one generic inbound stream pipeline that replaced power,
    /// sensor-waveform, and GATT-transcript capture as three separate ones.
    ///
    /// Unlike `requires`, this **does** cross the wire to
    /// dev-bench, on `DevBenchMessage::StudyStart`: four of the five
    /// [`StreamSource`](crate::streams::StreamSource) variants are
    /// dev-bench-mediated, so dev-bench has to know which taps to open and
    /// which `id` each one answers to.
    ///
    /// `#[serde(default)]` so a saved study (design.md §3 decision 38)
    /// authored before taps existed still loads — as a study that captures
    /// nothing, which is exactly what it did.
    #[serde(default)]
    pub streams: Vec<StreamTap, MAX_STREAMS_PER_STUDY>,
    /// CRC-32 over `steps` (design.md §3 decision 17), computed by whoever
    /// submits this `Study` via [`crate::crc::steps_crc`].
    pub steps_crc: u32,
    /// CRC-32 over `streams` (design.md §3 decision 39's 2026-08-25
    /// amendment), computed by the same submitter via
    /// [`crate::crc::streams_crc`]. A **sibling** of `steps_crc`, not a
    /// widening of it: `steps_crc`'s own definition is unchanged, and each
    /// seal is checked independently at both hops, so a mismatch says which
    /// half is corrupt.
    ///
    /// `#[serde(default)]` — and, unlike `requires`, that default is
    /// *correct* rather than merely permissive: a saved study (design.md §3
    /// decision 38) authored before taps existed has no `streams`, and `0`
    /// is the genuine CRC-32/ISO-HDLC of zero bytes, not a sentinel standing
    /// in for one. Every submitter recomputes and overwrites it anyway
    /// (`embarch-api/design.md` §3 decision 26).
    #[serde(default)]
    pub streams_crc: u32,
}

/// The explicit "I don't care which build" value for either
/// [`Requirements`] field (design.md §3 decision 40).
pub const REQUIREMENT_ANY: &str = "any";

/// The dev-bench and DUT firmware builds a `Study` is meant to run against
/// (design.md §3 decision 40, §4.1).
///
/// Two free-form strings, matching the shape `HelloAck.firmware_version`
/// already uses (`embarch-dev-bench/design.md` §3 decision 18: whatever the
/// build embeds, typically `git describe --always --dirty --abbrev=8`).
/// Both are mandatory and [`REQUIREMENT_ANY`] is an explicit legal value.
///
/// **The verification asymmetry is real and cannot be designed away.**
/// dev-bench self-reports its version over `HelloAck`, so a dev-bench
/// requirement is genuinely *checked*. The DUT reports nothing at all —
/// Core flashes it through a debug probe with no readback path — so a
/// `firmware_version` requirement is only verifiable when the outpost is
/// compiled in or the run just flashed it. That is what
/// [`Provenance`](crate::result::Provenance)'s source fields exist to record
/// rather than paper over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requirements {
    pub dev_bench_version: String<MAX_FIRMWARE_VERSION_LEN>,
    pub firmware_version: String<MAX_FIRMWARE_VERSION_LEN>,
}

impl Requirements {
    /// Both fields explicitly [`REQUIREMENT_ANY`] — a dev-bench self-test
    /// with no DUT involved, said out loud.
    pub fn any() -> Self {
        let any = String::try_from(REQUIREMENT_ANY).expect("REQUIREMENT_ANY fits");
        Requirements { dev_bench_version: any.clone(), firmware_version: any }
    }

    /// `POST /study`'s pre-flight check (design.md §3 decision 18): a blank
    /// requirement is the not-thought-about case decision 40 exists to
    /// reject, and is not the same thing as [`REQUIREMENT_ANY`].
    pub fn validate(&self) -> Result<(), RequirementsError> {
        if self.dev_bench_version.trim().is_empty() {
            return Err(RequirementsError::BlankDevBenchVersion);
        }
        if self.firmware_version.trim().is_empty() {
            return Err(RequirementsError::BlankFirmwareVersion);
        }
        Ok(())
    }
}

/// Whether an actual version satisfies a declared requirement — exact match,
/// or [`REQUIREMENT_ANY`]. Lives here so Core's version gate (design.md §3
/// decision 40) holds no independent copy of the comparison rule.
pub fn requirement_satisfied(required: &str, actual: &str) -> bool {
    required == REQUIREMENT_ANY || required == actual
}

/// Why a `Study.requires` isn't usable (design.md §3 decision 40).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequirementsError {
    BlankDevBenchVersion,
    BlankFirmwareVersion,
}

impl core::fmt::Display for RequirementsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let field = match self {
            RequirementsError::BlankDevBenchVersion => "dev_bench_version",
            RequirementsError::BlankFirmwareVersion => "firmware_version",
        };
        write!(
            f,
            "requires.{field} is blank; state the build this study needs, or              '{REQUIREMENT_ANY}' if it genuinely doesn't matter"
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RequirementsError {}

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
    /// `false` (default) aborts the `Study` on this step's `Fail`/`TimedOut`;
    /// `true` continues to the next step regardless. design.md §3 decision 13.
    #[serde(default)]
    pub continue_on_fail: bool,
    /// How long dev-bench waits *before* starting this step's action —
    /// design.md §3 decision 42, the "when" half of authoring a stimulus.
    ///
    /// Steps run strictly in sequence, so until this existed the only
    /// expressible timing was "immediately after the previous step
    /// finished". That is not enough to author a stimulus: letting a DUT
    /// settle after a connect, or waiting inside an open
    /// [`Action::GattMonitorStart`] window before writing so the transcript
    /// clearly separates unsolicited traffic from the response to the
    /// write, both need a delay that isn't a side effect of some other
    /// step's `timeout_ms`.
    ///
    /// Deliberately *not* folded into `timeout_ms`: this is time spent
    /// before the action starts, so it doesn't consume the action's own
    /// budget, and a step's `Outcome::TimedOut` keeps meaning "the action
    /// took too long" rather than "the delay was too long".
    ///
    /// Declared last, and encoded last, on purpose. Postcard is a
    /// field-order-sensitive format with no field names on the wire, and
    /// dev-bench hand-decodes `Step` in C (`serial_protocol.c`); appending
    /// rather than inserting means that decoder gained one trailing varint
    /// read instead of a re-shuffled sequence. Wire-format change all the
    /// same — hence the v6 bump in
    /// [`crate::schema_version`].
    #[serde(default)]
    pub delay_before_ms: u32,
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
        /// Connect only to an advertiser whose advertised local name equals
        /// this, exactly — design.md §3 decision 43.
        ///
        /// Added because "whichever DUT shows up first" is not a usable
        /// default on a real bench. Found live running roadmap Milestone 6:
        /// with both `target_address` and this unset, consecutive runs of the
        /// *same* study connected to visibly different peripherals — one run
        /// discovered a GATT table with a `0x1910` service, the next an
        /// entirely different table carrying two Apple 128-bit services —
        /// neither of them the DUT under test. Every study then failed with
        /// "service not found on DUT", which is true and completely
        /// misleading: the service wasn't on the device dev-bench happened to
        /// reach.
        ///
        /// `target_address` already existed and remains the precise filter,
        /// but it can't be authored ahead of time for a DUT that advertises
        /// a resolvable private address, and nobody knows their DUT's MAC by
        /// heart. A name is what an engineer actually knows
        /// (`CONFIG_BT_DEVICE_NAME`). Both may be set; both must then match.
        ///
        /// Matched against the advertised name only — never against the GAP
        /// Device Name characteristic (`0x2A00`), which would require
        /// connecting first, i.e. exactly what this exists to avoid.
        #[serde(default)]
        target_name: Option<String<MAX_LOCAL_NAME_LEN>>,
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
    /// Opens a capture window that deliberately **outlives its own step**:
    /// runs the same wildcard discovery as `GattDiscover`, subscribes to
    /// every Notify/Indicate characteristic, and returns immediately, leaving
    /// every subscription armed. Every step that runs afterwards — including
    /// `DataExchange` writes that stimulate the DUT — has its GATT traffic
    /// recorded into the streamed transcript until a `GattMonitorStop`
    /// closes the window. design.md §3 decision 36.
    ///
    /// This is the one action that makes "stimulate the DUT and capture what
    /// comes back" expressible at all: `GattMonitorAll` tears its own
    /// subscriptions down when its step ends, and steps run strictly in
    /// sequence, so a write step and a `GattMonitorAll` step can never
    /// overlap.
    GattMonitorStart {},
    /// Closes the window a preceding `GattMonitorStart` opened: unsubscribes
    /// everything it armed and reports the window's own `gatt_services` plus
    /// the (capped) inline `gatt_activity` summary in this step's
    /// `StepResult`. The full, uncapped record is the streamed transcript,
    /// not this summary. A `GattMonitorStop` with no open window is a
    /// no-op `Pass`, not a `Fail` — a study that ends without one still has
    /// its window closed implicitly when the study does. design.md §3
    /// decision 36.
    GattMonitorStop {},
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
    // `StreamCapture` was here (design.md §3 decisions 20/21) and is
    // **retired** by decision 39: a continuous capture of what a
    // characteristic streams is now a declared
    // `StreamSource::GattNotify` tap (§4.8), not a per-step action kind.
    // Removed rather than kept as a dead trailing variant so nothing can
    // author one — the schema break is already paid for by v8's
    // Hello/HelloAck handshake, and a variant nothing dispatches is the
    // silently-captures-nothing failure decision 36 was opened by.
}

// `PowerSampleWindow` was here (design.md §4.4) and is **retired** by §3
// decision 39's 2026-08-25 amendment, along with `Step.power_sample` above.
// It carried one field, `sample_rate_hz`, naming dev-bench's power-sampling
// rate for a step-bounded window; a `StreamSource::PowerFrontEnd
// { sample_hz }` tap with a `StreamScope::Steps { from, to }` covering the
// same step (§4.8) expresses exactly that.
//
// Retired on evidence, not on symmetry: nothing consumed it. `embarch-core`
// took a power capture's rate from the tap, dev-bench's C encoder wrote its
// `Option` byte as `None` unconditionally while its decoder read-and-
// discarded it, and `study_builder::build_study` always emitted `None`.
// Milestone 4 — the first study that would author a power capture, and one
// that has never run — now finds one way to do it rather than two.

#[cfg(test)]
mod tests {
    // `#![no_std]` is on for the default feature set, so `std` isn't in the
    // prelude even though the test harness links it. Only the JSON test
    // below needs it.
    extern crate std;

    use super::*;

    #[test]
    fn any_is_an_explicit_legal_value_for_both_requirements() {
        let requires = Requirements::any();
        assert_eq!(requires.dev_bench_version.as_str(), REQUIREMENT_ANY);
        assert_eq!(requires.firmware_version.as_str(), REQUIREMENT_ANY);
        assert_eq!(requires.validate(), Ok(()));
    }

    #[test]
    fn a_blank_requirement_is_not_the_same_thing_as_any() {
        // Decision 40's whole point: "I don't care which build" is a real
        // answer, but it has to be *said*. Blank is the nobody-thought-about-
        // it case, and it is a pre-flight failure.
        let mut requires = Requirements::any();
        requires.firmware_version = String::try_from("  ").unwrap();
        assert_eq!(requires.validate(), Err(RequirementsError::BlankFirmwareVersion));

        let mut requires = Requirements::any();
        requires.dev_bench_version = String::new();
        assert_eq!(requires.validate(), Err(RequirementsError::BlankDevBenchVersion));
    }

    #[test]
    fn requirement_matching_is_exact_or_any() {
        assert!(requirement_satisfied("any", "g1a2b3c-dirty"));
        assert!(requirement_satisfied("g1a2b3c", "g1a2b3c"));
        assert!(!requirement_satisfied("g1a2b3c", "g1a2b3c-dirty"));
        assert!(!requirement_satisfied("g1a2b3c", ""));
        // "Any" is a value, not a wildcard syntax — nothing else matches
        // loosely, because a result attributed to the wrong firmware is
        // worse than no result.
        assert!(!requirement_satisfied("g*", "g1a2b3c"));
    }

    #[test]
    fn a_study_json_without_requires_is_rejected_rather_than_defaulted() {
        // Run on a deliberately larger stack: deserializing a `Study` at all
        // needs ~75 KiB of inline `heapless` arrays plus serde's own frames,
        // and overflows libtest's default stack in a debug build. That is
        // design.md §7's long-standing open item, not something this test
        // introduces — `embarch-api` closed its own exposure the same way
        // (`embarch-api/design.md` §3 decision 36).
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                // Mandatory on purpose (decision 40): omitting it must fail,
                // not silently become "any".
                let without = r#"{"name":"s","steps":[],"steps_crc":0}"#;
                assert!(serde_json::from_str::<Study>(without).is_err());

                let with = r#"{"name":"s","requires":{"dev_bench_version":"any",
                    "firmware_version":"any"},"steps":[],"steps_crc":0}"#;
                let study: Study = serde_json::from_str(with).unwrap();
                assert_eq!(study.requires, Requirements::any());
                // `streams`, unlike `requires`, defaults: a saved study
                // authored before taps existed captured nothing, and still
                // does.
                assert!(study.streams.is_empty());
                // And so does its sibling seal — with the defaulted value
                // being the *correct* one, since 0 really is the CRC of the
                // empty tap list this study has (crate::crc::streams_crc).
                assert_eq!(study.streams_crc, 0);
                assert_eq!(crate::crc::streams_crc(&study.streams).unwrap(), 0);
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
