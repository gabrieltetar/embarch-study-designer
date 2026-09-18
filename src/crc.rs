//! `steps_crc` and `streams_crc` — decision 17's integrity seal
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

use crate::limits::MAX_STREAMS_PER_STUDY;
use crate::streams::StreamTap;
use crate::study::Step;

const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);

/// A single `Step`'s postcard encoding didn't fit the internal scratch
/// buffer. Should be unreachable given `limits` (decision 15),
/// but this returns an error rather than panicking — this crate avoids
/// panics as a matter of course, not just at the FFI boundary (decision 23).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepTooLargeError;

/// A single `StreamTap`'s postcard encoding didn't fit the internal scratch
/// buffer — [`StepTooLargeError`]'s counterpart for [`streams_crc`], and
/// equally unreachable given `limits`. A distinct type rather than a shared
/// one so a caller's error message can name which of a study's two seals
/// failed to compute, matching the reason there are two seals at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamTapTooLargeError;

/// A single `ProtocolDef`'s postcard encoding didn't fit the internal
/// scratch buffer — the third sibling of [`StepTooLargeError`] and
/// [`StreamTapTooLargeError`], and a distinct type for the same reason: a
/// caller's error message should name which of a study's **three** seals
/// failed to compute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolTooLargeError;

/// CRC-32 (the common "CRC-32"/Ethernet/zip polynomial) over a `Study`'s
/// `steps`, decision 17. Computed once by whoever submits a
/// `Study` (`embarch-api`, or a human via the CLI); checked again,
/// independently, at both the API<->Core hop and the Core<->dev-bench hop.
///
/// Streams each step's postcard encoding through the CRC digest one at a
/// time rather than buffering the whole `steps` list at once, so a
/// constrained dev-bench MCU only ever needs a stack buffer sized for one
/// `Step`, not for `MAX_STEPS_PER_STUDY` of them.
/// Takes a slice rather than a concrete collection (decision 46):
/// `Study.steps`' backing store now differs per feature, and this
/// function only ever iterates, so a slice is both the honest signature and
/// the one that works for either shape. `&study.steps` coerces via `Deref`.
pub fn steps_crc(steps: &[Step]) -> Result<u32, StepTooLargeError> {
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

/// CRC-32 over a `Study`'s `streams`, decision 39's 2026-08-25
/// amendment — a **sibling** of [`steps_crc`], not a widening of it.
///
/// Same algorithm, same one-item-at-a-time digest, and the same
/// checked-independently-at-both-hops posture: a constrained MCU needs stack
/// for one [`StreamTap`], not for `MAX_STREAMS_PER_STUDY` of them.
///
/// **A study with no taps seals to `0`**, which is not a sentinel — it is the
/// genuine CRC-32/ISO-HDLC of zero bytes (init and xorout are both
/// `0xFFFF_FFFF`, which cancel). That is what makes `Study.streams_crc`'s
/// `#[serde(default)]` honest for a saved study (decision 38) authored
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

/// CRC-32 over a `Study`'s `protocols` (decision 58) — the
/// third seal, and the first one added since `streams_crc`.
///
/// **Why a protocol is sealed when a decoder is not.** `Study.decoders`
/// (decision 52) is covered by neither CRC, because a layout only decides how
/// the host *renders* a byte that was already captured, and re-rendering a
/// capture with a corrected layout must leave it the same study. A
/// `ProtocolDef` is the opposite: dev-bench **executes** it, so it is
/// exactly the class of value `steps_crc` exists for. Corrupting one in
/// flight would have a firmware writing different bytes to a DUT's control
/// point than the study said it should.
///
/// Same three choices as its two siblings, deliberately unchanged: the `crc`
/// crate's CRC-32/ISO-HDLC, and one element streamed through the digest at a
/// time so a constrained dev-bench MCU needs stack for one `ProtocolDef`
/// rather than for `MAX_PROTOCOLS_PER_STUDY` of them.
///
/// **This is also the in-frame `crc32` primitive's algorithm** (decision 59).
/// CRC-32/ISO-HDLC *is* the CRC-32 this crate's protocol grammar named by its
/// seed:
/// init `0xFFFFFFFF`, reflected in and out, final XOR `0xFFFFFFFF` — bit for
/// bit what Zephyr's `crc32_ieee` computes. So the manifest-identity seal and
/// the checksum inside a DUT's own frames run through one implementation,
/// which is what the design asked for, rather than through two that agree
/// until one of them is edited. See [`crate::eap_parse`] for why the seed is
/// therefore **not** an author-declared parameter.
pub fn protocols_crc(protocols: &[crate::eap::ProtocolDef]) -> Result<u32, ProtocolTooLargeError> {
    // A `ProtocolDef` is by far the largest of the three sealed elements: up
    // to 12 states, each with a write and four event arms, plus frames and
    // sources carrying two 16-byte UUIDs apiece. Sized with the same
    // generous margin its siblings use rather than tuned to the two worked
    // protocols, per decision 15's posture on every constant here.
    const SCRATCH_LEN: usize = 8192;

    let mut digest = CRC32.digest();
    let mut scratch = [0u8; SCRATCH_LEN];
    for p in protocols.iter() {
        let encoded = postcard::to_slice(p, &mut scratch).map_err(|_| ProtocolTooLargeError)?;
        digest.update(encoded);
    }
    Ok(digest.finalize())
}

/// How many bytes a `Study`'s `protocols` field occupies on the wire to
/// dev-bench, **length prefix included**.
///
/// Not a seal and not a limit of this crate's: it exists so a host can
/// compare a study it has built against
/// [`crate::limits::DEV_BENCH_MAX_PROTOCOLS_WIRE_LEN`], the one advisory
/// dev-bench cap that mirrors no count here and therefore cannot be checked
/// by counting anything. The comparison is a **warning**, never a gate — see
/// that constant.
///
/// **The whole field, not the sum of its elements.** [`protocols_crc`]
/// digests each `ProtocolDef` separately and never encodes the sequence
/// header, because a seal only has to be reproducible. The 3 KB cap bounds
/// the encoded `protocols` *span* of a `StudyStart` frame, which is
/// postcard's varint element count followed by the elements — so this
/// serializes the slice itself. Serializing the elements and summing would
/// under-report by the prefix, which is the kind of off-by-a-header that
/// makes an advisory read "within caps" right up to the frame dev-bench
/// refuses.
///
/// Measured with postcard's `Size` flavor rather than into a scratch buffer:
/// at this crate's ceilings the encoding is ~15 KB, and a 16 KB stack array
/// on a constrained target to learn a length is the wrong trade. `Size`
/// counts bytes as they are produced and allocates nothing.
pub fn protocols_wire_len(
    protocols: &[crate::eap::ProtocolDef],
) -> Result<usize, ProtocolTooLargeError> {
    postcard::serialize_with_flavor(&protocols, postcard::ser_flavors::Size::default())
        .map_err(|_| ProtocolTooLargeError)
}

/// CRC-32/ISO-HDLC over an arbitrary byte run — the same algorithm
/// decision 59's grammar names for its in-frame `crc32` primitive, and the
/// same digest the three study seals use. That primitive itself is parsed
/// and pinned but has no render consumer yet: `eap_parse`'s
/// `ResolvedProtocol::render_layout` refuses it by name
/// (`RenderUnimplemented`, decision 71) rather than silently rendering a
/// flat, unchecked field.
///
/// Exposed rather than kept private because a captured record's own
/// CRC-32 is checked host-side by [`crate::records`]'s `RecordCheck`
/// (decision 70) — a different module from the one sealing a `Study` —
/// and this crate's answer to "two places need the identical computation"
/// is one function, not two (decision 2).
pub fn crc32_ieee(bytes: &[u8]) -> u32 {
    CRC32.checksum(bytes)
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
        let mut a: crate::bounded::StepList = crate::bounded::StepList::new();
        a.push(step("connect")).unwrap();
        let mut b: crate::bounded::StepList = crate::bounded::StepList::new();
        b.push(step("connect")).unwrap();

        assert_eq!(steps_crc(&a).unwrap(), steps_crc(&b).unwrap());
    }

    #[test]
    fn different_steps_produce_different_crc() {
        let mut a: crate::bounded::StepList = crate::bounded::StepList::new();
        a.push(step("connect")).unwrap();
        let mut b: crate::bounded::StepList = crate::bounded::StepList::new();
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
        let mut steps: crate::bounded::StepList = crate::bounded::StepList::new();
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

#[cfg(test)]
mod wire_len_tests {
    use super::*;
    use crate::eap::{ActiveState, ProtocolDef, StateDef, StateKind, TerminalOutcome};

    fn protocol(name: &str) -> ProtocolDef {
        let mut states = crate::bounded::Bounded::new();
        states
            .push(StateDef {
                name: heapless::String::try_from("go").unwrap(),
                kind: StateKind::Active(ActiveState {
                    on_enter: None,
                    on_event: Vec::new(),
                    on_timeout: None,
                }),
            })
            .unwrap();
        states
            .push(StateDef {
                name: heapless::String::try_from("done").unwrap(),
                kind: StateKind::Terminal(TerminalOutcome::Pass),
            })
            .unwrap();
        ProtocolDef {
            name: heapless::String::try_from(name).unwrap(),
            sources: Vec::new(),
            frames: Vec::new(),
            session: Vec::new(),
            states,
        }
    }

    /// The empty field still costs its length prefix. A `protocols_wire_len`
    /// that summed element encodings would answer 0 here, which is the shape
    /// of the off-by-a-header this function's doc comment is about.
    #[test]
    fn the_empty_field_is_one_byte_of_length_prefix() {
        assert_eq!(protocols_wire_len(&[]).unwrap(), 1);
    }

    /// Whole-field, not sum-of-elements: two protocols encode to the sum of
    /// their own encodings **plus** the one-byte varint count.
    #[test]
    fn the_prefix_is_included() {
        let a = protocol("a");
        let b = protocol("bb");
        let one =
            postcard::serialize_with_flavor(&a, postcard::ser_flavors::Size::default()).unwrap();
        let two =
            postcard::serialize_with_flavor(&b, postcard::ser_flavors::Size::default()).unwrap();
        assert_eq!(protocols_wire_len(&[a, b]).unwrap(), one + two + 1);
    }

    /// A real-shaped manifest is nowhere near the advisory cap — the
    /// comparison the cap exists for is meaningful, not permanently true or
    /// permanently false.
    #[test]
    fn a_small_manifest_is_far_under_the_advisory_cap() {
        let len = protocols_wire_len(&[protocol("bds")]).unwrap();
        assert!(len < crate::limits::DEV_BENCH_MAX_PROTOCOLS_WIRE_LEN);
        assert!(len > 1);
    }
}
