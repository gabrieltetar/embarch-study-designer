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
use crate::schema_version::STUDY_DESIGNER_SCHEMA_VERSION;
use crate::study::Study;

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
}

/// This crate's `STUDY_DESIGNER_SCHEMA_VERSION` (design.md §3 decision 12),
/// for dev-bench firmware to embed in its own `HelloAck`.
#[no_mangle]
pub extern "C" fn essd_schema_version() -> u32 {
    STUDY_DESIGNER_SCHEMA_VERSION
}

/// Decodes a postcard-encoded `Study` from `input[0..input_len]` and
/// recomputes `steps_crc` over its `steps`, writing whether that matches the
/// `Study`'s own `steps_crc` field to `*out_crc_matches` (design.md §3
/// decision 17). Returns a status code rather than panicking on malformed
/// input (design.md §3 decision 23).
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
        let mut steps: Vec<crate::study::Step, { crate::limits::MAX_STEPS_PER_STUDY }> = Vec::new();
        steps
            .push(crate::study::Step {
                name: String::try_from("connect").unwrap(),
                action: Action::BleConnect { role: BleRole::Central, target_address: None },
                timeout_ms: 1_000,
                power_sample: None,
                continue_on_fail: false,
            })
            .unwrap();
        let steps_crc = crate::crc::steps_crc(&steps).unwrap();

        let good = Study {
            name: String::try_from("t").unwrap(),
            steps: steps.clone(),
            validations: Vec::new(),
            steps_crc,
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
}
