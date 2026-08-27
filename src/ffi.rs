//! Minimal `extern "C"` surface for dev-bench firmware (design.md §3
//! decisions 7, 23).
//!
//! This is a representative slice of the eventual FFI surface, enough to
//! lock in the calling convention — integer status codes, buffer+length
//! params, no panic ever crosses this boundary (a Rust panic unwinding into
//! C firmware is undefined behavior) — not the full surface. The field-level
//! accessors dev-bench firmware actually needs to build/read a `Study` are
//! deferred until that firmware exists to define real requirements against
//! (design.md §7). Combined with `panic = "abort"` in `Cargo.toml`'s
//! `[profile.release]`, every exported function here is safe to call from C
//! even on malformed input: it returns a status code, never panics.

use core::slice;

use crate::crc::steps_crc;
use crate::limits::{MAX_LOCAL_NAME_LEN, MAX_NAME_LEN, MAX_STEPS_PER_STUDY, MAX_STUDY_NAME_LEN};
use crate::schema_version::DEV_BENCH_WIRE_SCHEMA_VERSION;
use crate::study::{Action, Study};

/// `embarch-dev-bench/design.md` §3 decision 8: when this crate is
/// cross-compiled as the bare-metal `--crate-type staticlib` dev-bench
/// firmware links (`target_os = "none"` — true for `thumbv8m.main-none-eabihf`,
/// false for every host target this crate is also built for, including
/// `cargo test --features ffi` on the host), nothing else in that build
/// provides a `#[panic_handler]` the way `std` does for a host binary — one
/// must exist somewhere in the crate graph or the link fails outright. Gated
/// on `target_os = "none"` rather than `not(feature = "std")` specifically so
/// it does NOT fire for `cargo test --features ffi` on a host target (which
/// also has `std` off but is linked by the host's own test harness, which
/// already provides a panic handler via libstd — defining a second one there
/// would conflict). In practice this should never actually run: every
/// exported function in this module returns a status code rather than
/// panicking (this module's own doc comment, decision 23) — this is a
/// backstop for the rest of the crate's `debug_assert!`/indexing panics, not
/// an expected code path.
#[cfg(all(feature = "ffi", target_os = "none"))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EssdStatus {
    Ok = 0,
    /// A required pointer argument was null — the caller's bug.
    NullPointer = -1,
    /// `input` wasn't a valid postcard-encoded `Study`, or a single `Step`
    /// inside it was too large for `steps_crc`'s internal scratch buffer
    /// (should be unreachable given `limits`, design.md §3 decision 15).
    DecodeError = -2,
    /// The `Study` decoded, but its `steps_crc` didn't match the recomputed
    /// value (design.md §3 decision 17).
    CrcMismatch = -3,
    /// A step's Action isn't BleAdvertise -- this decode surface is scoped to
    /// BleAdvertise-only steps for now (embarch-dev-bench/design.md §3 decision
    /// 21's initial pass); BleConnect/DataExchange dispatch needs a larger
    /// decode surface, deliberately deferred (see embarch-dev-bench/design.md §4).
    UnsupportedAction = -4,
}

/// Mirrors `Action::BleAdvertise` (design.md §4.3), scoped to the fields
/// dev-bench's initial dispatch pass actually needs (design.md §3 decision
/// 21). `service_uuids` is deliberately not carried into this struct -- an
/// accepted v1 gap, not a silent bug: advertising still exercises the radio
/// without needing UUIDs for this milestone's self-test (which submits
/// `service_uuids: []` anyway); a `Study` whose `BleAdvertise` step sets
/// non-empty `service_uuids` still decodes and dispatches successfully here,
/// it just loses that field.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EssdBleAdvertiseAction {
    pub local_name: [u8; MAX_LOCAL_NAME_LEN],
    pub local_name_len: u8,
    pub has_local_name: bool,
    pub adv_interval_ms: u16,
}

impl EssdBleAdvertiseAction {
    const fn zeroed() -> Self {
        Self {
            local_name: [0; MAX_LOCAL_NAME_LEN],
            local_name_len: 0,
            has_local_name: false,
            adv_interval_ms: 0,
        }
    }
}

/// Mirrors `Step` (design.md §4.2), `action` narrowed to
/// [`EssdBleAdvertiseAction`] -- see `essd_study_decode_full`'s doc comment.
/// `delay_before_ms` is not carried into this struct (it isn't needed for
/// `BleAdvertise` dispatch; same accepted-gap posture as `service_uuids`
/// above). It is still *decoded* — postcard
/// deserialization happens against the real `Step`, so a v6 study parses
/// correctly here; the field is simply not projected across this `repr(C)`
/// boundary, which keeps `study_ffi.h`'s mirror of it unchanged. Nothing is
/// lost in practice: the study-execution path that honours the delay is
/// dev-bench's own C decoder in `serial_protocol.c`, not this narrowed
/// BleAdvertise-only view.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EssdStep {
    pub name: [u8; MAX_NAME_LEN],
    pub name_len: u8,
    pub timeout_ms: u32,
    pub continue_on_fail: bool,
    pub action: EssdBleAdvertiseAction,
}

impl EssdStep {
    const fn zeroed() -> Self {
        Self {
            name: [0; MAX_NAME_LEN],
            name_len: 0,
            timeout_ms: 0,
            continue_on_fail: false,
            action: EssdBleAdvertiseAction::zeroed(),
        }
    }
}

/// Mirrors `Study` (design.md §4.1), `steps` narrowed to [`EssdStep`] -- see
/// `essd_study_decode_full`'s doc comment. `steps_crc` is not carried into
/// this struct: it has already been checked by the time this struct is
/// populated.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EssdStudy {
    pub name: [u8; MAX_STUDY_NAME_LEN],
    pub name_len: u8,
    pub steps: [EssdStep; MAX_STEPS_PER_STUDY],
    pub steps_len: u32,
}

impl EssdStudy {
    const fn zeroed() -> Self {
        Self {
            name: [0; MAX_STUDY_NAME_LEN],
            name_len: 0,
            steps: [EssdStep::zeroed(); MAX_STEPS_PER_STUDY],
            steps_len: 0,
        }
    }
}

/// Copies `src` into `dst`, truncating to `dst`'s capacity (should never
/// actually truncate given `limits`, design.md §3 decision 15 -- `dst` is
/// always sized to match the `heapless::String`/array this is copying from).
/// Returns the copied length.
fn copy_bytes(src: &[u8], dst: &mut [u8]) -> u8 {
    let len = src.len().min(dst.len());
    dst[..len].copy_from_slice(&src[..len]);
    len as u8
}

/// This crate's [`DEV_BENCH_WIRE_SCHEMA_VERSION`] (design.md §3 decision 12
/// and its 2026-08-25 amendment), for dev-bench firmware to embed in its own
/// `HelloAck`.
///
/// The **wire** constant specifically, not the host one: this is the number
/// compared at `Hello`/`HelloAck`, and a host-side-only reshape must not
/// move what firmware reports about itself. The host constant has no FFI
/// surface at all, deliberately — dev-bench is not a party to that hop.
#[no_mangle]
pub extern "C" fn essd_schema_version() -> u32 {
    DEV_BENCH_WIRE_SCHEMA_VERSION
}

/// Decodes a postcard-encoded `Study` from `input[0..input_len]` and
/// recomputes `steps_crc` over its `steps`, writing whether that matches the
/// `Study`'s own `steps_crc` field to `*out_crc_matches` (design.md §3
/// decision 17). Returns a status code rather than panicking on malformed
/// input (design.md §3 decision 23).
///
/// **`streams_crc` (§3 decision 39's 2026-08-25 amendment) is deliberately
/// not checked here.** A single `out_crc_matches` bool cannot say *which* of
/// two seals failed, which is the property having two of them exists for —
/// so folding both into it would quietly destroy the thing being added.
/// Widening the C ABI instead would extend a surface that has no caller
/// anywhere (design.md §7's `BleAdvertise`-scoped FFI decode surface), which
/// is the same posture `embarch-topology/design.md` §3 decision 18's
/// amendment takes toward `validate_signal`. The real Core<->dev-bench check
/// is dev-bench's own C decoder in `serial_protocol.c`, which computes both
/// seals over the spans it walks.
///
/// # Safety
/// `input` must point to `input_len` readable bytes, and `out_crc_matches`
/// must point to a valid, writable `bool` — both for the duration of this
/// call. A null `input` or `out_crc_matches` is handled (returns
/// `NullPointer`) rather than dereferenced.
#[no_mangle]
pub unsafe extern "C" fn essd_study_decode_and_verify(
    input: *const u8,
    input_len: usize,
    out_crc_matches: *mut bool,
) -> EssdStatus {
    if input.is_null() || out_crc_matches.is_null() {
        return EssdStatus::NullPointer;
    }
    let bytes = slice::from_raw_parts(input, input_len);
    let study: Study = match postcard::from_bytes(bytes) {
        Ok(study) => study,
        Err(_) => return EssdStatus::DecodeError,
    };
    let recomputed = match steps_crc(&study.steps) {
        Ok(crc) => crc,
        Err(_) => return EssdStatus::DecodeError,
    };
    *out_crc_matches = recomputed == study.steps_crc;
    EssdStatus::Ok
}

/// Decodes a postcard-encoded `Study` and verifies its `steps_crc`
/// (superseding neither `essd_study_decode_and_verify` nor decision 19's
/// existing check -- this is a second, additive entry point for dev-bench's
/// real per-`Study` dispatch, decision 21), then copies every step into
/// `*out_study` as a C-friendly, fixed-layout struct so C code can iterate
/// steps/read fields without touching Rust-owned memory directly.
///
/// Scoped to `Action::BleAdvertise` steps only for this pass -- a `Study`
/// containing any other action kind fails whole with `UnsupportedAction`
/// rather than silently skipping or truncating it, since a caller dispatching
/// a partially-decoded `Study` would be running something other than what
/// was actually submitted. `steps_crc` is checked before the action-kind
/// check, so a corrupted `Study` is reported as corrupted (`CrcMismatch`),
/// not as unsupported.
///
/// # Safety
/// `input` must point to `input_len` readable bytes, and `out_study` must
/// point to a valid, writable `EssdStudy` -- both for the duration of this
/// call. A null `input` or `out_study` is handled (returns `NullPointer`)
/// rather than dereferenced. `*out_study` is left unwritten on any non-`Ok`
/// return -- never a partial decode.
#[no_mangle]
pub unsafe extern "C" fn essd_study_decode_full(
    input: *const u8,
    input_len: usize,
    out_study: *mut EssdStudy,
) -> EssdStatus {
    if input.is_null() || out_study.is_null() {
        return EssdStatus::NullPointer;
    }
    let bytes = slice::from_raw_parts(input, input_len);
    let study: Study = match postcard::from_bytes(bytes) {
        Ok(study) => study,
        Err(_) => return EssdStatus::DecodeError,
    };
    let recomputed = match steps_crc(&study.steps) {
        Ok(crc) => crc,
        Err(_) => return EssdStatus::DecodeError,
    };
    // `steps_crc` only, for the reason `essd_study_decode_and_verify`'s doc
    // comment gives: `EssdStatus::CrcMismatch` is one code with nothing to
    // say about which of a study's two seals failed.
    if recomputed != study.steps_crc {
        return EssdStatus::CrcMismatch;
    }

    let mut out = EssdStudy::zeroed();
    out.name_len = copy_bytes(study.name.as_bytes(), &mut out.name);

    for (i, step) in study.steps.iter().enumerate() {
        let Action::BleAdvertise { local_name, service_uuids: _, adv_interval_ms } = &step.action
        else {
            return EssdStatus::UnsupportedAction;
        };

        let mut essd_step = EssdStep::zeroed();
        essd_step.name_len = copy_bytes(step.name.as_bytes(), &mut essd_step.name);
        essd_step.timeout_ms = step.timeout_ms;
        essd_step.continue_on_fail = step.continue_on_fail;
        if let Some(local_name) = local_name {
            essd_step.action.local_name_len =
                copy_bytes(local_name.as_bytes(), &mut essd_step.action.local_name);
            essd_step.action.has_local_name = true;
        }
        essd_step.action.adv_interval_ms = *adv_interval_ms;

        out.steps[i] = essd_step;
    }
    out.steps_len = study.steps.len() as u32;

    *out_study = out;
    EssdStatus::Ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::study::{Action, BleRole, Study};
    use heapless::{String, Vec};

    #[test]
    fn rejects_null_pointers_without_panicking() {
        let mut out = false;
        let status = unsafe { essd_study_decode_and_verify(core::ptr::null(), 0, &mut out) };
        assert_eq!(status, EssdStatus::NullPointer);

        let status = unsafe { essd_study_decode_and_verify([0u8; 1].as_ptr(), 1, core::ptr::null_mut()) };
        assert_eq!(status, EssdStatus::NullPointer);
    }

    #[test]
    fn rejects_garbage_bytes_without_panicking() {
        let garbage = [0xFFu8; 16];
        let mut out = false;
        let status = unsafe { essd_study_decode_and_verify(garbage.as_ptr(), garbage.len(), &mut out) };
        assert_eq!(status, EssdStatus::DecodeError);
    }

    #[test]
    fn detects_crc_match_and_mismatch() {
        let mut steps = crate::bounded::StepList::new();
        steps
            .push(crate::study::Step {
                name: String::try_from("connect").unwrap(),
                action: Action::BleConnect { role: BleRole::Central, target_address: None , target_name: None },
                timeout_ms: 1_000,
                continue_on_fail: false,
                delay_before_ms: 0,
            })
            .unwrap();
        let steps_crc = crate::crc::steps_crc(&steps).unwrap();

        let good = Study {
            protocols: Default::default(),
            protocols_crc: 0,
            decoders: Default::default(),
            name: String::try_from("t").unwrap(),
            requires: crate::study::Requirements::any(),
            steps: steps.clone(),
            streams: Vec::new(),
            steps_crc,
            streams_crc: 0,
            dev_bench_log_level: crate::study::DevBenchLogLevel::default(),
        };
        let mut buf = [0u8; 256];
        let encoded = postcard::to_slice(&good, &mut buf).unwrap();
        let mut out = false;
        let status = unsafe { essd_study_decode_and_verify(encoded.as_ptr(), encoded.len(), &mut out) };
        assert_eq!(status, EssdStatus::Ok);
        assert!(out);

        let mut corrupted = good;
        corrupted.steps_crc ^= 1;
        let encoded = postcard::to_slice(&corrupted, &mut buf).unwrap();
        let status = unsafe { essd_study_decode_and_verify(encoded.as_ptr(), encoded.len(), &mut out) };
        assert_eq!(status, EssdStatus::Ok);
        assert!(!out);
    }

    fn ble_advertise_study() -> Study {
        let mut steps = crate::bounded::StepList::new();
        steps
            .push(crate::study::Step {
                name: String::try_from("advertise-1").unwrap(),
                action: Action::BleAdvertise {
                    local_name: Some(String::try_from("embarch-dev-bench").unwrap()),
                    service_uuids: Vec::new(),
                    adv_interval_ms: 100,
                },
                timeout_ms: 5_000,
                continue_on_fail: false,
                delay_before_ms: 0,
            })
            .unwrap();
        steps
            .push(crate::study::Step {
                name: String::try_from("advertise-2").unwrap(),
                action: Action::BleAdvertise {
                    local_name: None,
                    service_uuids: Vec::new(),
                    adv_interval_ms: 250,
                },
                timeout_ms: 2_000,
                continue_on_fail: true,
                delay_before_ms: 0,
            })
            .unwrap();
        let steps_crc = crate::crc::steps_crc(&steps).unwrap();

        Study {
            protocols: Default::default(),
            protocols_crc: 0,

            decoders: Default::default(),
            name: String::try_from("ble-advertise-study").unwrap(),
            requires: crate::study::Requirements::any(),
            steps,
            streams: Vec::new(),
            steps_crc,
            streams_crc: 0,
            dev_bench_log_level: crate::study::DevBenchLogLevel::default(),
        }
    }

    #[test]
    fn decode_full_round_trips_ble_advertise_only_study() {
        let study = ble_advertise_study();
        let mut buf = [0u8; 1024];
        let encoded = postcard::to_slice(&study, &mut buf).unwrap();

        let mut out = EssdStudy::zeroed();
        let status = unsafe { essd_study_decode_full(encoded.as_ptr(), encoded.len(), &mut out) };
        assert_eq!(status, EssdStatus::Ok);

        assert_eq!(out.steps_len, 2);
        assert_eq!(&out.name[..out.name_len as usize], b"ble-advertise-study");

        let step0 = &out.steps[0];
        assert_eq!(&step0.name[..step0.name_len as usize], b"advertise-1");
        assert_eq!(step0.timeout_ms, 5_000);
        assert!(!step0.continue_on_fail);
        assert!(step0.action.has_local_name);
        assert_eq!(
            &step0.action.local_name[..step0.action.local_name_len as usize],
            b"embarch-dev-bench"
        );
        assert_eq!(step0.action.adv_interval_ms, 100);

        let step1 = &out.steps[1];
        assert_eq!(&step1.name[..step1.name_len as usize], b"advertise-2");
        assert_eq!(step1.timeout_ms, 2_000);
        assert!(step1.continue_on_fail);
        assert!(!step1.action.has_local_name);
        assert_eq!(step1.action.adv_interval_ms, 250);
    }

    #[test]
    fn decode_full_detects_crc_mismatch() {
        let mut study = ble_advertise_study();
        study.steps_crc ^= 1;
        let mut buf = [0u8; 1024];
        let encoded = postcard::to_slice(&study, &mut buf).unwrap();

        let mut out = EssdStudy::zeroed();
        let status = unsafe { essd_study_decode_full(encoded.as_ptr(), encoded.len(), &mut out) };
        assert_eq!(status, EssdStatus::CrcMismatch);
    }

    #[test]
    fn decode_full_rejects_unsupported_action() {
        let mut steps = crate::bounded::StepList::new();
        steps
            .push(crate::study::Step {
                name: String::try_from("connect").unwrap(),
                action: Action::BleConnect { role: BleRole::Central, target_address: None , target_name: None },
                timeout_ms: 1_000,
                continue_on_fail: false,
                delay_before_ms: 0,
            })
            .unwrap();
        let steps_crc = crate::crc::steps_crc(&steps).unwrap();
        let study = Study {
            protocols: Default::default(),
            protocols_crc: 0,
            decoders: Default::default(),
            name: String::try_from("t").unwrap(),
            requires: crate::study::Requirements::any(),
            steps,
            streams: Vec::new(),
            steps_crc,
            streams_crc: 0,
            dev_bench_log_level: crate::study::DevBenchLogLevel::default(),
        };

        let mut buf = [0u8; 256];
        let encoded = postcard::to_slice(&study, &mut buf).unwrap();

        let mut out = EssdStudy::zeroed();
        let status = unsafe { essd_study_decode_full(encoded.as_ptr(), encoded.len(), &mut out) };
        assert_eq!(status, EssdStatus::UnsupportedAction);
    }

    #[test]
    fn decode_full_rejects_null_pointers_without_panicking() {
        let mut out = EssdStudy::zeroed();
        let status = unsafe { essd_study_decode_full(core::ptr::null(), 0, &mut out) };
        assert_eq!(status, EssdStatus::NullPointer);

        let status = unsafe { essd_study_decode_full([0u8; 1].as_ptr(), 1, core::ptr::null_mut()) };
        assert_eq!(status, EssdStatus::NullPointer);
    }
}
