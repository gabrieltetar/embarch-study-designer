//! `steps_crc` and `streams_crc` — design.md §3 decision 17's integrity seal
//! over `Study.steps`, and decision 39's 2026-08-25 amendment's sibling seal
//! over `Study.streams`.
//!
//! **Two seals, not one widened one.** `steps_crc` sits *between* `steps` and
//! `streams` on `DevBenchMessage::StudyStart`, so a single CRC covering both
//! would have to digest two non-contiguous spans in dev-bench's hand-written
//! C — or `StudyStart`'s field order would have to be reshuffled, which is a
//! reshape where an append will do. Two seals also fail more usefully than
//! one: a mismatch says which half is corrupt.

use crc::{Crc, CRC_32_ISO_HDLC};
use heapless::Vec;

use crate::limits::{MAX_STEPS_PER_STUDY, MAX_STREAMS_PER_STUDY};
use crate::streams::StreamTap;
use crate::study::Step;

const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);

/// A single `Step`'s postcard encoding didn't fit the internal scratch
/// buffer. Should be unreachable given `limits` (design.md §3 decision 15),
/// but this returns an error rather than panicking — this crate avoids
/// panics as a matter of course, not just at the FFI boundary (§3 decision 23).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepTooLargeError;

/// A single `StreamTap`'s postcard encoding didn't fit the internal scratch
/// buffer — [`StepTooLargeError`]'s counterpart for [`streams_crc`], and
/// equally unreachable given `limits`. A distinct type rather than a shared
/// one so a caller's error message can name which of a study's two seals
/// failed to compute, matching the reason there are two seals at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamTapTooLargeError;

/// CRC-32 (the common "CRC-32"/Ethernet/zip polynomial) over a `Study`'s
/// `steps`, design.md §3 decision 17. Computed once by whoever submits a
/// `Study` (`embarch-api`, or a human via the CLI); checked again,
/// independently, at both the API<->Core hop and the Core<->dev-bench hop.
///
/// Streams each step's postcard encoding through the CRC digest one at a
/// time rather than buffering the whole `steps` list at once, so a
/// constrained dev-bench MCU only ever needs a stack buffer sized for one
/// `Step`, not for `MAX_STEPS_PER_STUDY` of them.
pub fn steps_crc(steps: &Vec<Step, MAX_STEPS_PER_STUDY>) -> Result<u32, StepTooLargeError> {
    // Generous margin above the worst-case single encoded `Step` (~600
    // bytes, dominated by `GattOperation::Write`'s 512-byte payload).
    const SCRATCH_LEN: usize = 768;

    let mut digest = CRC32.digest();
    let mut scratch = [0u8; SCRATCH_LEN];
    for step in steps.iter() {
        let encoded = postcard::to_slice(step, &mut scratch).map_err(|_| StepTooLargeError)?;
        digest.update(encoded);
    }
    Ok(digest.finalize())
}

/// CRC-32 over a `Study`'s `streams`, design.md §3 decision 39's 2026-08-25
/// amendment — a **sibling** of [`steps_crc`], not a widening of it.
///
/// Same algorithm, same one-item-at-a-time digest, and the same
/// checked-independently-at-both-hops posture: a constrained MCU needs stack
/// for one [`StreamTap`], not for `MAX_STREAMS_PER_STUDY` of them.
///
/// **A study with no taps seals to `0`**, which is not a sentinel — it is the
/// genuine CRC-32/ISO-HDLC of zero bytes (init and xorout are both
/// `0xFFFF_FFFF`, which cancel). That is what makes `Study.streams_crc`'s
/// `#[serde(default)]` honest for a saved study (§3 decision 38) authored
/// before taps existed: the defaulted value is the correct value, not a
/// placeholder standing in for one.
pub fn streams_crc(
    streams: &Vec<StreamTap, MAX_STREAMS_PER_STUDY>,
) -> Result<u32, StreamTapTooLargeError> {
    // Worst-case single encoded `StreamTap`: a `MAX_STREAM_NAME_LEN` name, a
    // `GattNotify` source's two 16-byte UUIDs, an encoding, and a scope —
    // comfortably under 128 bytes. Same generous margin as `steps_crc`'s.
    const SCRATCH_LEN: usize = 192;

    let mut digest = CRC32.digest();
    let mut scratch = [0u8; SCRATCH_LEN];
    for tap in streams.iter() {
        let encoded = postcard::to_slice(tap, &mut scratch).map_err(|_| StreamTapTooLargeError)?;
        digest.update(encoded);
    }
    Ok(digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::Uuid;
    use crate::sample::Unit;
    use crate::streams::{SampleLayout, StreamEncoding, StreamScope, StreamSource};
    use crate::study::{Action, BleRole};

    fn step(name: &str) -> Step {
        Step {
            name: heapless::String::try_from(name).unwrap(),
            action: Action::BleConnect { role: BleRole::Central, target_address: None , target_name: None },
            timeout_ms: 5_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        }
    }

    fn tap(id: u8, name: &str) -> StreamTap {
        StreamTap {
            id,
            name: heapless::String::try_from(name).unwrap(),
            source: StreamSource::PowerFrontEnd { sample_hz: 1_000 },
            encoding: StreamEncoding::Samples {
                layout: SampleLayout::F32Le,
                unit: Unit::Milliamps,
                channel_id: 0,
            },
            scope: StreamScope::Steps { from: 0, to: 0 },
        }
    }

    #[test]
    fn same_steps_produce_same_crc() {
        let mut a: Vec<Step, MAX_STEPS_PER_STUDY> = Vec::new();
        a.push(step("connect")).unwrap();
        let mut b: Vec<Step, MAX_STEPS_PER_STUDY> = Vec::new();
        b.push(step("connect")).unwrap();

        assert_eq!(steps_crc(&a).unwrap(), steps_crc(&b).unwrap());
    }

    #[test]
    fn different_steps_produce_different_crc() {
        let mut a: Vec<Step, MAX_STEPS_PER_STUDY> = Vec::new();
        a.push(step("connect")).unwrap();
        let mut b: Vec<Step, MAX_STEPS_PER_STUDY> = Vec::new();
        b.push(step("connect-2")).unwrap();

        assert_ne!(steps_crc(&a).unwrap(), steps_crc(&b).unwrap());
    }

    #[test]
    fn same_streams_produce_same_crc() {
        let mut a: Vec<StreamTap, MAX_STREAMS_PER_STUDY> = Vec::new();
        a.push(tap(0, "power")).unwrap();
        let mut b: Vec<StreamTap, MAX_STREAMS_PER_STUDY> = Vec::new();
        b.push(tap(0, "power")).unwrap();

        assert_eq!(streams_crc(&a).unwrap(), streams_crc(&b).unwrap());
    }

    #[test]
    fn different_streams_produce_different_crc() {
        let mut a: Vec<StreamTap, MAX_STREAMS_PER_STUDY> = Vec::new();
        a.push(tap(0, "power")).unwrap();
        let mut b: Vec<StreamTap, MAX_STREAMS_PER_STUDY> = Vec::new();
        b.push(tap(0, "power-2")).unwrap();

        assert_ne!(streams_crc(&a).unwrap(), streams_crc(&b).unwrap());
    }

    /// The two seals are genuinely independent: changing one half leaves the
    /// other's value alone, which is the whole reason a mismatch can say
    /// *which* half is corrupt.
    #[test]
    fn the_two_seals_are_independent() {
        let mut steps: Vec<Step, MAX_STEPS_PER_STUDY> = Vec::new();
        steps.push(step("connect")).unwrap();
        let mut streams: Vec<StreamTap, MAX_STREAMS_PER_STUDY> = Vec::new();
        streams.push(tap(0, "power")).unwrap();

        let steps_before = steps_crc(&steps).unwrap();
        let streams_before = streams_crc(&streams).unwrap();

        streams[0].name = heapless::String::try_from("renamed").unwrap();
        assert_eq!(steps_crc(&steps).unwrap(), steps_before);
        assert_ne!(streams_crc(&streams).unwrap(), streams_before);
    }

    /// An empty tap list seals to `0` — the real CRC of zero bytes, not a
    /// sentinel. `Study.streams_crc`'s `#[serde(default)]` leans on this.
    #[test]
    fn no_taps_seals_to_the_crc_of_nothing() {
        let empty: Vec<StreamTap, MAX_STREAMS_PER_STUDY> = Vec::new();
        assert_eq!(streams_crc(&empty).unwrap(), 0);
        // Not a special case in the code — the same value the algorithm
        // itself produces for an empty input.
        assert_eq!(CRC32.digest().finalize(), 0);
    }

    /// A `Uuid` import that would otherwise be unused keeps the
    /// `GattNotify`-shaped worst case honest: the widest source variant
    /// still fits the scratch buffer.
    #[test]
    fn the_widest_tap_fits_the_scratch_buffer() {
        let mut widest: Vec<StreamTap, MAX_STREAMS_PER_STUDY> = Vec::new();
        let name_buf = [b'x'; crate::limits::MAX_STREAM_NAME_LEN];
        let name = core::str::from_utf8(&name_buf).unwrap();
        widest
            .push(StreamTap {
                id: 0,
                name: heapless::String::try_from(name).unwrap(),
                source: StreamSource::GattNotify {
                    service_uuid: Uuid([0xAB; 16]),
                    characteristic_uuid: Uuid([0xCD; 16]),
                },
                encoding: StreamEncoding::Samples {
                    layout: SampleLayout::I16Be,
                    unit: Unit::Raw,
                    channel_id: 255,
                },
                scope: StreamScope::Steps { from: u32::MAX, to: u32::MAX },
            })
            .unwrap();
        assert!(streams_crc(&widest).is_ok());
    }
}
