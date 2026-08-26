//! The `embarch-outpost` trace wire format, and the manifest that names it.
//!
//! `embarch-outpost/design.md` §4, and that repo's own `src/outpost_priv.h`,
//! which is the other half of this contract.
//!
//! **Why this lives here and not in `embarch-core`.** Every other rendered
//! stream's row shape lives in this crate — [`crate::sample::Sample`],
//! [`crate::gatt::GattTranscriptEntry`] — precisely so Core holds no column
//! knowledge (`embarch-core/design.md` §3 decision 30). An outpost trace is
//! the third rendered encoding and gets the same treatment, which also means
//! `embarch-api` and `embarch-ui` read a trace through the same code Core
//! writes it with rather than through a second implementation.
//!
//! **This is not a dev-bench wire type**, which is the one way it differs from
//! its neighbours: `embarch-outpost/design.md` §3 decision 11 has dev-bench
//! passing outpost bytes through and interpreting nothing, so no C decoder
//! mirrors this and no both-languages pin applies to it. The mirror that does
//! exist is `embarch-outpost/scripts/decode_outpost.py`, and the firmware
//! encoder itself.
//!
//! ## Frame
//!
//! ```text
//! frame := COBS(body || crc32_ieee(body) as 4 bytes LE) || 0x00
//! body  := frame_type: u8, seq: u8, payload
//! ```
//!
//! `frame_type` 0x01 carries a postcard `Vec<OutpostRecord>` (a varint count
//! then that many records); 0x02 carries an [`OutpostHeader`]. COBS is the
//! same framing the Core<->dev-bench link uses (§3 decision 10), so the same
//! shape of code reads both.
//!
//! ## The clock is this side's
//!
//! **A record carries no timestamp.** Layout 2 took the DUT's
//! `k_cycle_get_32()` stamp off the wire entirely, because reading it happened
//! inside the context switch and inside `_isr_wrapper()` — the instrument
//! charging its cost to the path it measures (`embarch-outpost/design.md` §3
//! decision 4, reworked 2026-08-26).
//!
//! Time now comes from **whoever received the bytes**: Core stamps each frame
//! with its own receipt time as it arrives, exactly as it already stamps a
//! sample or a transcript entry (`embarch-core/design.md` §3 decision 30), and
//! every record in a frame carries that one stamp. Two consequences this
//! module's callers have to respect, both of them in decision 17:
//!
//! - **A frame is the finest interval this wire resolves.** Ordering inside a
//!   frame is real and its durations are not measurable. Nothing may spread a
//!   frame's records across an interval to make a nicer picture.
//! - **A trace can legitimately have no time at all** — nobody stamped it, or
//!   the stamps did not line up ([`OutpostRecord::to_csv_row`] takes an
//!   `Option`). An ordered, untimed trace is a real answer; a fabricated one
//!   is not.

use crc::{Crc, CRC_32_ISO_HDLC};
use heapless::String as HString;
use serde::{Deserialize, Serialize};

const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);

/// Bumped by any change to the record or frame layout. A stream reporting a
/// different one is not decoded — the shapes are not compatible and guessing
/// which fields moved is exactly the kind of plausible-and-wrong answer the
/// manifest mechanism exists to prevent.
///
/// - **2** — no timestamps: records are `{kind, a, b}` and the header carries
///   no cycle rate. The host stamps frames.
/// - **1** — an absolute `k_cycle_get_32()` in every record, `cycles_per_sec`
///   in the header. Nothing decodes it any more, deliberately: a layout-1
///   stream read as layout 2 would take each record's timestamp for its kind.
pub const RECORD_LAYOUT_VERSION: u8 = 2;

pub const FRAME_RECORDS: u8 = 0x01;
pub const FRAME_HEADER: u8 = 0x02;

/// What the firmware emits when it cannot name the active vector — an
/// architecture whose active-interrupt register this module will not guess at,
/// or `CONFIG_EMBARCH_OUTPOST_ISR_IDENTIFY=n`.
pub const IRQ_UNKNOWN: u32 = 0xFFFF_FFFF;

/// The longest string the firmware will put in a header frame
/// (`CONFIG_EMBARCH_OUTPOST_BUILD_ID_MAX`'s own range maximum).
pub const MAX_BUILD_ID_LEN: usize = 128;

/// One traced event. Fixed shape, **no timestamp**, IDs never strings —
/// `embarch-outpost/design.md` §3 decision 4.
///
/// The time a record happened is not in the record. It is the arrival stamp of
/// the frame that carried it, which is why every rendering path here takes a
/// frame index and a stamp alongside the record itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutpostRecord {
    /// [`RecordKind`] as a raw byte. Kept raw rather than as an enum on the
    /// wire type so an unknown future kind decodes and renders as itself
    /// instead of failing the whole frame.
    pub kind: u8,
    pub a: u32,
    pub b: u32,
}

/// Record kinds. The wire carries [`OutpostRecord::kind`] as a byte; this is
/// the naming, and an unrecognised byte deliberately has no variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RecordKind {
    ThreadSwitchIn = 0,
    ThreadSwitchOut = 1,
    IsrEnter = 2,
    IsrExit = 3,
    Idle = 4,
    ThreadCreate = 5,
    ThreadName = 6,
    Marker = 7,
    /// Records lost to a full ring: `a` is how many. `b` is 0 and reserved —
    /// it carried the cycle span the losses fell across through layout 1, and
    /// there is no clock on the DUT side to measure one with any more.
    ///
    /// **Always the first record of its frame**, which is what replaces that
    /// span: the losses are bounded by the arrival stamp of the previous frame
    /// and that of the frame carrying the gap. A bound of one frame, and it
    /// must be presented as a bound.
    ///
    /// This also ends layout 1's one ordering anomaly — a gap stamped earlier
    /// than the records printed after it, which is what broke the host's
    /// unwrap on the first real capture. With no timestamps there is no unwrap
    /// left to break.
    Gap = 8,
}

impl RecordKind {
    pub fn from_byte(b: u8) -> Option<Self> {
        Some(match b {
            0 => Self::ThreadSwitchIn,
            1 => Self::ThreadSwitchOut,
            2 => Self::IsrEnter,
            3 => Self::IsrExit,
            4 => Self::Idle,
            5 => Self::ThreadCreate,
            6 => Self::ThreadName,
            7 => Self::Marker,
            8 => Self::Gap,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ThreadSwitchIn => "thread_switch_in",
            Self::ThreadSwitchOut => "thread_switch_out",
            Self::IsrEnter => "isr_enter",
            Self::IsrExit => "isr_exit",
            Self::Idle => "idle",
            Self::ThreadCreate => "thread_create",
            Self::ThreadName => "thread_name",
            Self::Marker => "marker",
            Self::Gap => "gap",
        }
    }

    /// Whether `a` is a thread pointer, which is what the manifest's thread
    /// table is keyed by.
    pub fn a_is_thread(self) -> bool {
        matches!(
            self,
            Self::ThreadSwitchIn | Self::ThreadSwitchOut | Self::ThreadCreate | Self::ThreadName
        )
    }

    pub fn a_is_irq(self) -> bool {
        matches!(self, Self::IsrEnter | Self::IsrExit)
    }
}

/// Emitted at startup and repeated every
/// `CONFIG_EMBARCH_OUTPOST_HEADER_INTERVAL_MS`, so a host attaching mid-stream
/// can still decode.
///
/// **No manifest CRC.** There cannot be one: `embarch-outpost/design.md` §3
/// decision 9's rework replaced the post-link CRC patch with a compile-time
/// build ID, and a manifest generated *from the linked image* has no CRC the
/// firmware could have been built knowing. [`build_id`](Self::build_id) is
/// what a manifest is checked against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutpostHeader {
    pub record_layout_version: u8,
    /// Which hook families the running firmware actually has compiled in, so a
    /// host never has to infer that from the absence of records. See
    /// [`HeaderFlags`].
    pub flags: u8,
    pub outpost_version: HString<MAX_BUILD_ID_LEN>,
    pub build_id: HString<MAX_BUILD_ID_LEN>,
}

/// Bit positions in [`OutpostHeader::flags`].
pub struct HeaderFlags;

impl HeaderFlags {
    pub const TRACE_THREADS: u8 = 1 << 0;
    pub const TRACE_ISRS: u8 = 1 << 1;
    pub const TRACE_IDLE: u8 = 1 << 2;
    pub const TRACE_MARKERS: u8 = 1 << 3;
    pub const ISR_IDENTIFY: u8 = 1 << 4;
    pub const OVERFLOW_BLOCK: u8 = 1 << 5;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// COBS decoding failed, or the chunk was too short to hold a body and a
    /// CRC.
    Framing,
    /// The scratch buffer given to [`decode_frame`] was smaller than the
    /// decoded frame.
    ScratchTooSmall,
    /// The frame's own CRC did not match its body. Costs this frame and
    /// nothing else — which is why there is a CRC per frame.
    Crc,
    /// A frame type this decoder does not know. Reserved so a later command
    /// channel is an added type rather than a reshape.
    UnknownFrameType(u8),
    /// The body did not hold what its type says it holds.
    Malformed,
}

/// One decoded frame. Borrows the scratch buffer it was decoded into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame<'a> {
    Header { seq: u8, header: OutpostHeader },
    Records { seq: u8, records: RecordIter<'a> },
}

/// Records within one frame, decoded lazily.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordIter<'a> {
    rest: &'a [u8],
    remaining: u32,
}

impl<'a> Iterator for RecordIter<'a> {
    type Item = Result<OutpostRecord, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        match postcard::take_from_bytes::<OutpostRecord>(self.rest) {
            Ok((rec, rest)) => {
                self.rest = rest;
                Some(Ok(rec))
            }
            Err(_) => {
                self.remaining = 0;
                Some(Err(DecodeError::Malformed))
            }
        }
    }
}

/// Splits a raw capture into the COBS chunks between `0x00` delimiters,
/// skipping empty ones. Each chunk goes to [`decode_frame`].
///
/// **Their enumeration order is the frame index** arrival stamps are keyed by
/// (`embarch-outpost/design.md` §3 decision 18), so a chunk that later fails
/// its CRC still consumes an index here. Whoever stamps the bytes counts the
/// same non-empty runs between delimiters, which is what keeps the two sides
/// in step without either one decoding for the other.
pub fn chunks(raw: &[u8]) -> impl Iterator<Item = &[u8]> {
    raw.split(|b| *b == 0).filter(|c| !c.is_empty())
}

/// COBS-decodes one chunk into `scratch`, checks its CRC, and parses its body.
///
/// A failure costs exactly this frame: the caller keeps going. That is the
/// whole reason there is a CRC per frame rather than one over the capture.
pub fn decode_frame<'a>(chunk: &[u8], scratch: &'a mut [u8]) -> Result<Frame<'a>, DecodeError> {
    let len = cobs_decode(chunk, scratch)?;
    // body + 2-byte header + 4-byte CRC
    if len < 6 {
        return Err(DecodeError::Framing);
    }
    let (body, crc_bytes) = scratch[..len].split_at(len - 4);
    let want = u32::from_le_bytes([crc_bytes[0], crc_bytes[1], crc_bytes[2], crc_bytes[3]]);
    if CRC32.checksum(body) != want {
        return Err(DecodeError::Crc);
    }

    let frame_type = body[0];
    let seq = body[1];
    let payload = &body[2..];

    match frame_type {
        FRAME_HEADER => {
            let header =
                postcard::from_bytes::<OutpostHeader>(payload).map_err(|_| DecodeError::Malformed)?;
            Ok(Frame::Header { seq, header })
        }
        FRAME_RECORDS => {
            let (count, rest) =
                postcard::take_from_bytes::<u32>(payload).map_err(|_| DecodeError::Malformed)?;
            Ok(Frame::Records {
                seq,
                records: RecordIter { rest, remaining: count },
            })
        }
        other => Err(DecodeError::UnknownFrameType(other)),
    }
}

/// Standard COBS, the same convention `embarch-dev-bench`'s `cobs_encode`
/// uses. Bounds-checked on every write: the input is bytes off a serial link
/// and must never be able to overrun `out` regardless of what it claims.
fn cobs_decode(input: &[u8], out: &mut [u8]) -> Result<usize, DecodeError> {
    let mut read = 0usize;
    let mut write = 0usize;

    while read < input.len() {
        let code = input[read] as usize;
        if code == 0 || (read + code > input.len() && code != 1) {
            return Err(DecodeError::Framing);
        }
        read += 1;
        for _ in 1..code {
            if read >= input.len() {
                return Err(DecodeError::Framing);
            }
            if write >= out.len() {
                return Err(DecodeError::ScratchTooSmall);
            }
            out[write] = input[read];
            write += 1;
            read += 1;
        }
        if code != 0xFF && read < input.len() {
            if write >= out.len() {
                return Err(DecodeError::ScratchTooSmall);
            }
            out[write] = 0;
            write += 1;
        }
    }
    Ok(write)
}

#[cfg(feature = "alloc")]
mod manifest {
    use super::{RecordKind, RECORD_LAYOUT_VERSION};
    use alloc::collections::BTreeMap;
    use alloc::format;
    use alloc::string::{String, ToString};
    use serde::Deserialize;

    /// `outpost-manifest.json`, as `embarch-outpost/scripts/gen_outpost_manifest.py`
    /// emits it from the linked image.
    ///
    /// Every field is an ELF read, not a derivation: marker IDs from the
    /// application's own registration table, thread names from
    /// `_k_thread_obj_*`, ISR names from `_sw_isr_table[]` at exactly the index
    /// the firmware reports. Where a fact was unavailable the generator emits
    /// nothing for it and says why in [`notes`](Self::notes) — it never fills a
    /// gap with a plausible answer.
    #[derive(Debug, Clone, Default, Deserialize)]
    pub struct OutpostManifest {
        pub schema: u32,
        /// What a stream's header frame must match for this manifest to be
        /// applied. `embarch-outpost/design.md` §3 decision 9.
        pub build_id: String,
        #[serde(default)]
        pub outpost_version: String,
        pub record_layout_version: u8,
        #[serde(default)]
        pub markers: BTreeMap<String, String>,
        #[serde(default)]
        pub threads: BTreeMap<String, String>,
        #[serde(default)]
        pub isrs: BTreeMap<String, String>,
        /// The handler behind a shared dispatcher, where the table's `arg`
        /// resolved to a function. On Nordic this is the common case, not the
        /// exception — most IRQs dispatch through one `nrfx_isr`.
        #[serde(default)]
        pub isr_args: BTreeMap<String, String>,
        /// A content fingerprint for telling two manifests apart. **Not** what
        /// the firmware reports; see [`OutpostHeader`](super::OutpostHeader).
        #[serde(default)]
        pub manifest_crc: u32,
        #[serde(default)]
        pub notes: alloc::vec::Vec<String>,
    }

    /// Why a manifest was not applied to a stream. Each variant is a case
    /// where rendering anyway would produce a trace that is entirely readable
    /// and entirely wrong.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ManifestRefusal {
        /// No manifest reached Core with the flash that put this image on the
        /// DUT.
        None,
        BuildIdMismatch { manifest: String, firmware: String },
        LayoutVersion { manifest: u8, firmware: u8, decoder: u8 },
    }

    impl ManifestRefusal {
        pub fn describe(&self) -> String {
            match self {
                Self::None => "no manifest was stored with this study's flash".to_string(),
                Self::BuildIdMismatch { manifest, firmware } => format!(
                    "manifest build_id {manifest:?} != firmware build_id {firmware:?}"
                ),
                Self::LayoutVersion { manifest, firmware, decoder } => format!(
                    "record layout version mismatch (manifest {manifest}, firmware {firmware}, \
                     decoder {decoder})"
                ),
            }
        }
    }

    impl OutpostManifest {
        /// Whether this manifest describes the firmware that produced a given
        /// header, or why not.
        ///
        /// Both checks, not either: the build ID says this is the same source
        /// tree and marker list, the layout version says the two agree about
        /// what a record *is*.
        pub fn check(
            &self,
            firmware_build_id: &str,
            firmware_layout: u8,
        ) -> Result<(), ManifestRefusal> {
            if self.build_id != firmware_build_id {
                return Err(ManifestRefusal::BuildIdMismatch {
                    manifest: self.build_id.clone(),
                    firmware: firmware_build_id.to_string(),
                });
            }
            if self.record_layout_version != firmware_layout
                || firmware_layout != RECORD_LAYOUT_VERSION
            {
                return Err(ManifestRefusal::LayoutVersion {
                    manifest: self.record_layout_version,
                    firmware: firmware_layout,
                    decoder: RECORD_LAYOUT_VERSION,
                });
            }
            Ok(())
        }

        /// The human-readable name for a record's `a` field, or an empty
        /// string when this manifest does not name it. **Never a guess** — an
        /// unnamed thread pointer or vector number renders as the number it
        /// is.
        pub fn label(&self, kind: Option<RecordKind>, a: u32) -> String {
            let Some(kind) = kind else {
                return String::new();
            };
            if kind.a_is_thread() {
                return self
                    .threads
                    .get(&format!("0x{a:08x}"))
                    .cloned()
                    .unwrap_or_default();
            }
            if kind.a_is_irq() {
                if a == super::IRQ_UNKNOWN {
                    return String::new();
                }
                let key = a.to_string();
                let Some(handler) = self.isrs.get(&key) else {
                    return String::new();
                };
                // A shared trampoline's own name says nothing about which
                // peripheral fired; the handler it was given does.
                return match self.isr_args.get(&key) {
                    Some(inner) => format!("{handler}({inner})"),
                    None => handler.clone(),
                };
            }
            if kind == RecordKind::Marker {
                return self.markers.get(&a.to_string()).cloned().unwrap_or_default();
            }
            String::new()
        }
    }
}

#[cfg(feature = "alloc")]
pub use manifest::{ManifestRefusal, OutpostManifest};

#[cfg(feature = "alloc")]
mod render {
    use super::{OutpostManifest, OutpostRecord, RecordKind};
    use alloc::format;
    use alloc::string::String;

    /// The column list for a rendered `*.trace.csv`.
    ///
    /// **`rx_utc_ms` is the row's only clock, and it is the host's** — the same
    /// wall clock `core_rx_utc_ms` carries on a sample or transcript row
    /// (`embarch-core/design.md` §3 decision 30), which is what finally makes a
    /// trace laid alongside a power capture an alignment rather than a guess.
    /// It is a *frame's* stamp, so consecutive rows repeating one value is the
    /// normal case, not a defect: see [`OutpostRecord::to_csv_row`].
    ///
    /// `frame_index` and `frame_seq` are both here because they answer
    /// different questions. The index is this capture's own frame ordinal —
    /// monotonic, unbounded, and what an arrival stamp is keyed by (§3 decision
    /// 17). The seq is the firmware's own wrapping byte, so a reader can see
    /// frames the wire lost without decoding anything.
    pub fn csv_header() -> &'static str {
        "frame_index,frame_seq,rx_utc_ms,kind,a,b,name"
    }

    impl OutpostRecord {
        /// One rendered row.
        ///
        /// `frame_index`/`frame_seq` identify the frame this record arrived in
        /// and `rx_utc_ms` is when that whole frame arrived — so **every record
        /// of a frame renders with the same time**, deliberately. Interpolating
        /// them across the interval would manufacture a resolution the wire
        /// does not have (`embarch-outpost/design.md` §3 decision 17).
        ///
        /// `rx_utc_ms` is `None` when nobody stamped the capture, or when the
        /// stamps could not be lined up with the frames; the column is then
        /// empty rather than filled with something plausible. `manifest` may be
        /// absent the same way — a trace decodes into structure with no
        /// manifest at all, it just has no names in it.
        pub fn to_csv_row(
            &self,
            frame_index: u64,
            frame_seq: u8,
            rx_utc_ms: Option<u64>,
            manifest: Option<&OutpostManifest>,
        ) -> String {
            let kind = RecordKind::from_byte(self.kind);
            let kind_name = match kind {
                Some(k) => String::from(k.as_str()),
                None => format!("unknown_{}", self.kind),
            };
            let rx = match rx_utc_ms {
                Some(ms) => format!("{ms}"),
                None => String::new(),
            };
            let name = manifest.map(|m| m.label(kind, self.a)).unwrap_or_default();
            format!(
                "{frame_index},{frame_seq},{rx},{kind_name},{},{},{name}",
                self.a, self.b
            )
        }
    }
}

#[cfg(feature = "alloc")]
pub use render::csv_header;

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes captured from the real firmware encoder, not produced by
    /// inverting this decoder: a round trip through one implementation's own
    /// inverse agrees with itself no matter what the other two do.
    ///
    /// This is one header frame, taken verbatim from the first native_sim
    /// capture (`embarch-outpost/tests/native_sim_stream`).
    const HEADER_FRAME: &[u8] = &[
        0x02, 0x02, 0x3d, 0x02, 0x0f, 0x0d, b'd', b'4', b'9', b'0', b'f', b'c', b'3', b'-', b'd',
        b'i', b'r', b't', b'y', 0x27, b'd', b'4', b'9', b'0', b'f', b'c', b'3', b'-', b'd', b'i',
        b'r', b't', b'y', b'+', b'o', b'p', b'd', b'4', b'9', b'0', b'f', b'c', b'3', b'-', b'd',
        b'i', b'r', b't', b'y', b'+', b'm', b'a', b'0', b'b', b'd', b'4', b'8', b'e', b'a', 0x40,
        0xcc, b'L', 0xfd, 0x00,
    ];

    /// One **records** frame from the same capture, frame index 2: sixteen
    /// records in 97 framed bytes, which is the layout-2 record shape as the C
    /// encoder actually writes it.
    ///
    /// Pinned separately from the header because the record is the half that
    /// changed: a layout-1 decoder pointed at these bytes would read each
    /// record's `kind` as a timestamp and produce sixteen plausible, wrong
    /// rows. That is the failure the layout version exists to make impossible,
    /// and this is the test that would catch it.
    const RECORDS_FRAME: &[u8] = &[
        0x09, 0x01, 0x02, 0x10, 0x01, 0xa0, 0xf3, 0x95, 0x40, 0x01, 0x05, 0xa0, 0x81, 0x96,
        0x40, 0x02, 0x04, 0x01, 0x07, 0x02, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x03, 0x07, 0x02,
        0x07, 0x03, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x02, 0x04, 0x01, 0x07, 0x02, 0xff, 0xff,
        0xff, 0xff, 0x0f, 0x03, 0x07, 0x02, 0x07, 0x03, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x06,
        0x01, 0xa0, 0x81, 0x96, 0x40, 0x01, 0x05, 0xe0, 0x85, 0x96, 0x40, 0x06, 0x01, 0xe0,
        0x85, 0x96, 0x40, 0x01, 0x05, 0xe0, 0x85, 0x96, 0x40, 0x06, 0x01, 0xe0, 0x85, 0x96,
        0x40, 0x01, 0x05, 0xa0, 0xf3, 0x95, 0x40, 0x05, 0x2b, 0x54, 0x89, 0x78, 0x00,
    ];

    #[test]
    fn a_real_header_frame_decodes() {
        let chunk = chunks(HEADER_FRAME).next().expect("one chunk");
        let mut scratch = [0u8; 256];
        match decode_frame(chunk, &mut scratch).expect("decodes") {
            Frame::Header { seq, header } => {
                assert_eq!(seq, 0);
                assert_eq!(header.record_layout_version, RECORD_LAYOUT_VERSION);
                assert_eq!(header.outpost_version.as_str(), "d490fc3-dirty");
                assert_eq!(
                    header.build_id.as_str(),
                    "d490fc3-dirty+opd490fc3-dirty+ma0bd48ea"
                );
                // Every hook family compiled in; ISR identify off, because
                // native_sim is not Cortex-M.
                assert_eq!(header.flags, 0x0f);
                assert_eq!(header.flags & HeaderFlags::ISR_IDENTIFY, 0);
            }
            other => panic!("expected a header frame, got {other:?}"),
        }
    }

    #[test]
    fn a_corrupted_frame_costs_only_itself() {
        let mut corrupt = HEADER_FRAME.to_vec();
        corrupt[10] ^= 0xFF;
        let chunk = chunks(&corrupt).next().expect("one chunk");
        let mut scratch = [0u8; 256];
        assert_eq!(decode_frame(chunk, &mut scratch), Err(DecodeError::Crc));
    }

    #[test]
    fn a_scratch_buffer_that_is_too_small_is_an_error_not_an_overrun() {
        let chunk = chunks(HEADER_FRAME).next().expect("one chunk");
        let mut scratch = [0u8; 8];
        assert_eq!(
            decode_frame(chunk, &mut scratch),
            Err(DecodeError::ScratchTooSmall)
        );
    }

    /// The layout-2 record shape, against bytes the C encoder wrote.
    #[test]
    fn a_real_records_frame_decodes_into_untimed_records() {
        let chunk = chunks(RECORDS_FRAME).next().expect("one chunk");
        let mut scratch = [0u8; 256];
        match decode_frame(chunk, &mut scratch).expect("decodes") {
            Frame::Records { seq, records } => {
                assert_eq!(seq, 2);
                // Checked in place rather than collected: this crate is
                // `no_std` without `alloc` by default, and a wire-format pin
                // has no business needing an allocator.
                let mut count = 0usize;
                for (i, rec) in records.enumerate() {
                    let rec = rec.expect("every record decodes");
                    count += 1;
                    match i {
                        // A switch out of one thread and into another, both
                        // carrying nothing but a pointer.
                        0 => {
                            assert_eq!(rec.kind, RecordKind::ThreadSwitchOut as u8);
                            assert_eq!(rec.a, 134_576_544);
                            assert_eq!(rec.b, 0);
                        }
                        1 => {
                            assert_eq!(rec.kind, RecordKind::ThreadSwitchIn as u8);
                            assert_eq!(rec.a, 134_578_336);
                        }
                        // native_sim cannot name its active vector, and the
                        // firmware says so rather than reporting a number it
                        // guessed.
                        3 => {
                            assert_eq!(rec.kind, RecordKind::IsrEnter as u8);
                            assert_eq!(rec.a, IRQ_UNKNOWN);
                        }
                        4 => {
                            assert_eq!(rec.kind, RecordKind::Marker as u8);
                            assert_eq!(rec.a, 2);
                        }
                        _ => {}
                    }
                }
                assert_eq!(count, 16);
            }
            other => panic!("expected a records frame, got {other:?}"),
        }
    }

    /// A frame's stamp lands on every record in it, and an unstamped capture
    /// renders an empty column rather than a fabricated time.
    ///
    /// `alloc`-gated because the rendering half is: a `no_std` consumer of this
    /// crate decodes the wire and never formats a CSV row.
    #[cfg(feature = "alloc")]
    #[test]
    fn a_frames_stamp_is_every_row_in_it() {
        let rec = OutpostRecord { kind: RecordKind::Marker as u8, a: 3, b: 77 };
        assert_eq!(
            rec.to_csv_row(9, 41, Some(1_700_000_000_123), None),
            "9,41,1700000000123,marker,3,77,"
        );
        assert_eq!(rec.to_csv_row(9, 41, None, None), "9,41,,marker,3,77,");
        assert_eq!(
            csv_header(),
            "frame_index,frame_seq,rx_utc_ms,kind,a,b,name",
            "the column list is the contract embarch-ui refuses to guess at"
        );
    }

    #[test]
    fn record_kinds_round_trip_their_bytes() {
        for b in 0u8..=8 {
            let k = RecordKind::from_byte(b).expect("known kind");
            assert_eq!(k as u8, b);
            assert!(!k.as_str().is_empty());
        }
        assert!(RecordKind::from_byte(9).is_none());
    }
}
