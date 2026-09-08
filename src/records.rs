//! Engineer-declared record framing for a stream tap, and the check Core runs
//! against it after a capture — design.md §3 decision 60.
//!
//! **This exists because a capture could be short and say it was complete.**
//! A 10 h PPG drain (`embarch-core` study `872aef3c466dd465c66e671412a97760`,
//! 2026-09-08) lost three `StreamChunkBatch` frames on the Core↔dev-bench
//! UART — 976 bytes, four 244-byte notifications — damaging 3 of its 598
//! records. Every layer's own accounting was honest and none of them noticed:
//! the DUT advanced its BDS offset only on notifies that succeeded and
//! declared 9538149 bytes in its `PROGRESS` frames, dev-bench's queues dropped
//! nothing, and Core wrote the 9537173 bytes it received. The shortfall was
//! visible from exactly one place — the CRC-32 each record already carries —
//! and nothing was looking at it.
//!
//! So this is not a new integrity mechanism. It is a declaration of where the
//! one already in the data lives, so the host can check it.
//!
//! # Why host-side, and why it names its stream directly
//!
//! A [`RecordCheck`] rides in `Study.record_checks`, a **host-only** field
//! like `Study.requires` and `Study.decoders` — never transmitted to
//! dev-bench, sealed by neither `steps_crc` nor `streams_crc`. What a captured
//! byte *means* is the knowledge decision 39 took away from dev-bench, and
//! whether the host later checks a checksum changes neither what dev-bench
//! executes nor what it captures.
//!
//! Unlike a [`crate::decoder::StructLayout`], which the tap's own
//! `StreamEncoding::Struct` indexes into, this names its tap by
//! [`crate::streams::StreamTap::id`] instead. A tap's encoding crosses the
//! wire inside `StudyStart`, so referencing this from there would make a
//! host-side-only check cost a `DEV_BENCH_WIRE_SCHEMA_VERSION` bump and a
//! firmware reflash. Naming the id keeps the whole feature on this side of
//! that hop.
//!
//! # What a verified record actually promises
//!
//! End to end, and that is the point of checking here rather than on any one
//! link. A record that verifies was read correctly off the DUT's flash,
//! survived an unacknowledged BLE notification stream, dev-bench's queues, the
//! UART, Core's deframer and the write to disk. A per-frame sequence number on
//! the UART would have caught this particular fault and none of the others.

use heapless::Vec;
use serde::{Deserialize, Serialize};

use crate::limits::{MAX_BAD_RECORDS_REPORTED, MAX_RECORD_MAGIC_LEN};

/// One declared tap's record framing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordCheck {
    /// Which declared tap this describes, by [`crate::streams::StreamTap::id`].
    pub stream_id: u8,
    pub framing: RecordFraming,
}

/// How a capture's records are delimited and checked.
///
/// Append-only, like every other enum in this crate that reaches a persisted
/// file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordFraming {
    /// Each record begins with `magic` and ends with a 4-byte CRC-32/ISO-HDLC
    /// over every preceding byte of that record, little-endian.
    ///
    /// Fits batchMgr's `GWF1` PPG records and the WDS flash spill's, which
    /// share the shape — so this is a format family, not a one-off.
    ///
    /// **Record length is not declared, and does not need to be.** Boundaries
    /// are found by scanning for the next `magic`, and the trailing CRC is
    /// what confirms the guess: a record that verifies was split correctly, by
    /// construction. A stray `magic` inside a payload would split one record
    /// in two, so a failed segment is retried against later boundaries before
    /// being called damaged — see [`verify_records`]. That is why this needs
    /// no per-format length arithmetic and stays honest anyway.
    MagicPrefixedCrc32Le {
        magic: Vec<u8, MAX_RECORD_MAGIC_LEN>,
    },
}

/// What checking one capture found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordReport {
    /// Records found, verified or not.
    pub total: u32,
    /// Records whose trailing CRC matched their contents.
    pub verified: u32,
    /// Byte offsets of records that did not verify, capped at
    /// [`MAX_BAD_RECORDS_REPORTED`] — `total - verified` is the true count and
    /// is never capped, so a capture that lost a great deal still reports how
    /// much.
    pub bad_offsets: Vec<u32, MAX_BAD_RECORDS_REPORTED>,
    /// Bytes before the first record's magic, which belong to no record.
    /// Non-zero means the capture began mid-record — a tap opened after the
    /// DUT had started sending, or a lost first frame.
    pub leading_bytes: u32,
}

impl RecordReport {
    /// Whether every record found verified and nothing was left over. The one
    /// question a caller usually has.
    pub fn all_verified(&self) -> bool {
        self.total == self.verified && self.leading_bytes == 0
    }
}

/// How many later boundaries a failed segment is retried against before it is
/// called damaged.
///
/// Covers a stray `magic` inside a payload splitting one record into a few
/// pieces. Bounded rather than unlimited so a capture whose records are
/// genuinely damaged cannot turn this into a quadratic scan of the whole file:
/// with a cap, the work is linear in records and constant per record.
const MAX_COALESCE: usize = 4;

/// Checks every record in `bytes` against `framing`.
///
/// Pure, so it is testable without a capture on disk, and callable from either
/// side of the api↔Core hop.
pub fn verify_records(bytes: &[u8], framing: &RecordFraming) -> RecordReport {
    let RecordFraming::MagicPrefixedCrc32Le { magic } = framing;

    let mut report = RecordReport {
        total: 0,
        verified: 0,
        bad_offsets: Vec::new(),
        leading_bytes: 0,
    };

    if magic.is_empty() {
        return report;
    }

    let first = match find_magic(bytes, magic, 0) {
        Some(at) => at,
        None => {
            report.leading_bytes = clamp_u32(bytes.len());
            return report;
        }
    };
    report.leading_bytes = clamp_u32(first);

    let mut start = first;
    loop {
        // Candidate ends, in order: the next few magics, then end-of-capture.
        let mut end_candidates: Vec<usize, { MAX_COALESCE + 1 }> = Vec::new();
        let mut probe = start;
        for _ in 0..MAX_COALESCE {
            match find_magic(bytes, magic, probe + 1) {
                Some(at) => {
                    let _ = end_candidates.push(at);
                    probe = at;
                }
                None => break,
            }
        }
        let _ = end_candidates.push(bytes.len());

        report.total += 1;
        let mut resolved: Option<usize> = None;
        for &end in end_candidates.iter() {
            if end < start + magic.len() + 4 {
                continue; // too short to hold a magic and a CRC
            }
            let body = &bytes[start..end - 4];
            let stored = u32::from_le_bytes([
                bytes[end - 4],
                bytes[end - 3],
                bytes[end - 2],
                bytes[end - 1],
            ]);
            if crate::crc::crc32_ieee(body) == stored {
                resolved = Some(end);
                break;
            }
        }

        let next = match resolved {
            Some(end) => {
                report.verified += 1;
                end
            }
            None => {
                let _ = report.bad_offsets.push(clamp_u32(start));
                // Advance by one boundary only: a damaged record must not
                // swallow the intact ones behind it.
                match find_magic(bytes, magic, start + 1) {
                    Some(at) => at,
                    None => bytes.len(),
                }
            }
        };

        if next >= bytes.len() {
            break;
        }
        // A verified record's end is the next record's start only if a magic
        // is actually there; at end-of-capture it is not.
        start = match bytes[next..].starts_with(magic) {
            true => next,
            false => match find_magic(bytes, magic, next) {
                Some(at) => at,
                None => break,
            },
        };
    }

    report
}

/// First occurrence of `needle` in `haystack` at or after `from`.
fn find_magic(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|at| at + from)
}

/// Offsets and counts are reported as `u32`: a single capture larger than 4 GB
/// is not a thing this suite produces, and a saturating clamp is a better
/// answer than a silently wrapped offset.
fn clamp_u32(v: usize) -> u32 {
    if v > u32::MAX as usize {
        u32::MAX
    } else {
        v as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Capture buffer for the tests. `heapless`, not `std::vec` — a plain
    /// `cargo test` here exercises the real `#![no_std]`, no-allocator path
    /// (README), so a test that reached for `std` would not compile in the one
    /// configuration this crate's own CI checks.
    type Cap = Vec<u8, 8192>;

    fn framing() -> RecordFraming {
        RecordFraming::MagicPrefixedCrc32Le { magic: Vec::from_slice(b"GWF1").unwrap() }
    }

    /// Appends one well-formed record to `cap`: magic, `body`, then the
    /// little-endian CRC over both. Returns the offset it began at.
    fn push_record(cap: &mut Cap, body: &[u8]) -> u32 {
        let at = cap.len() as u32;
        cap.extend_from_slice(b"GWF1").unwrap();
        cap.extend_from_slice(body).unwrap();
        let crc = crate::crc::crc32_ieee(&cap[at as usize..]);
        cap.extend_from_slice(&crc.to_le_bytes()).unwrap();
        at
    }

    #[test]
    fn a_clean_capture_verifies_every_record() {
        let mut cap = Cap::new();
        for i in 0..5u8 {
            push_record(&mut cap, &[i; 40]);
        }
        let r = verify_records(&cap, &framing());
        assert_eq!((r.total, r.verified), (5, 5));
        assert!(r.bad_offsets.is_empty());
        assert_eq!(r.leading_bytes, 0);
        assert!(r.all_verified());
    }

    /// The fault this exists for: one record short by a lost frame's worth of
    /// bytes, the rest intact. The damaged one must be named and the others
    /// must still verify — a checker that gave up at the first failure would
    /// have called the 10 h drain unusable instead of 595 good records.
    #[test]
    fn one_damaged_record_is_named_and_the_rest_still_verify() {
        let mut cap = Cap::new();
        push_record(&mut cap, &[1u8; 60]);

        // A record built short: the CRC is computed over the full body, then
        // twenty bytes of it never arrive — which is what losing a
        // notification mid-record does.
        let bad_at = cap.len() as u32;
        let mut whole = Cap::new();
        push_record(&mut whole, &[2u8; 60]);
        cap.extend_from_slice(&whole[..20]).unwrap();
        cap.extend_from_slice(&whole[40..]).unwrap();

        push_record(&mut cap, &[3u8; 60]);

        let r = verify_records(&cap, &framing());
        assert_eq!((r.total, r.verified), (3, 2));
        assert_eq!(r.bad_offsets.as_slice(), &[bad_at]);
        assert!(!r.all_verified());
    }

    /// A `magic` that turns up inside a payload would split one record in two
    /// and report both halves as damaged. Retrying against later boundaries is
    /// what keeps a self-delimiting scheme honest with no per-format length
    /// arithmetic anywhere.
    #[test]
    fn a_stray_magic_inside_a_payload_does_not_fail_the_record() {
        let mut body = Cap::new();
        body.extend_from_slice(&[7u8; 12]).unwrap();
        body.extend_from_slice(b"GWF1").unwrap(); // the stray
        body.extend_from_slice(&[9u8; 12]).unwrap();

        let mut cap = Cap::new();
        push_record(&mut cap, &body);
        push_record(&mut cap, &[3u8; 30]);

        let r = verify_records(&cap, &framing());
        assert_eq!((r.total, r.verified), (2, 2), "the stray magic is coalesced away");
        assert!(r.all_verified());
    }

    /// A capture that begins mid-record says so: those bytes belong to no
    /// record, and dropping them silently would report a clean capture.
    #[test]
    fn bytes_before_the_first_record_are_reported() {
        let mut cap = Cap::new();
        cap.extend_from_slice(&[0xAAu8; 17]).unwrap();
        push_record(&mut cap, &[1u8; 20]);
        let r = verify_records(&cap, &framing());
        assert_eq!(r.leading_bytes, 17);
        assert_eq!((r.total, r.verified), (1, 1));
        assert!(!r.all_verified(), "a clean record is not a clean capture");
    }

    /// An empty capture, and one with no magic at all, are both "no records"
    /// rather than a panic or a false pass.
    #[test]
    fn a_capture_with_no_records_reports_none() {
        let r = verify_records(&[], &framing());
        assert_eq!((r.total, r.verified, r.leading_bytes), (0, 0, 0));

        let r = verify_records(&[1u8, 2, 3, 4, 5], &framing());
        assert_eq!((r.total, r.verified), (0, 0));
        assert_eq!(r.leading_bytes, 5, "every byte belongs to no record");
    }

    /// The true shortfall is never capped even when the offset list is, so a
    /// badly damaged capture cannot read as a mildly damaged one.
    #[test]
    fn the_offset_list_caps_but_the_count_does_not() {
        let mut cap = Cap::new();
        let n = MAX_BAD_RECORDS_REPORTED + 6;
        for _ in 0..n {
            push_record(&mut cap, &[5u8; 40]);
            let last = cap.len() - 1;
            cap[last] ^= 0xFF; // corrupt the stored CRC
        }
        let r = verify_records(&cap, &framing());
        assert_eq!(r.verified, 0);
        assert_eq!(r.total as usize, n, "every record is counted");
        assert_eq!(r.bad_offsets.len(), MAX_BAD_RECORDS_REPORTED, "the list is capped");
    }
}
