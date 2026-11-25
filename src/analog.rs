//! The analog-capture block and start header, and the pure functions that
//! decode and check them (EMB-474, decisions 104 to 106).
//!
//! **One definition for every reader.** Core's samples-link decoder, the WAV
//! export and the tests all call the functions here, so nothing in the suite
//! holds a second copy of an offset. `no_std`, no allocator, no panics: every
//! function takes a byte slice and returns a value or a reason.
//!
//! # The wire layout (all integers little-endian)
//!
//! The owner's decisions (EMB-469, 7 and 8) fix the *fields*; the byte values
//! below (magics, field order, widths, the event word's bits) are this
//! crate's choice, and are what `embarch-dev-bench` has to emit.
//!
//! **Start header**, once per session, [`START_HEADER_BYTES`] long:
//!
//! | offset | bytes | field |
//! |---:|---:|---|
//! | 0 | 4 | magic, `EAH1` |
//! | 4 | 4 | `sample_rate_hz` |
//! | 8 | 1 | `resolution_bits` |
//! | 9 | 1 | `gain_numerator` |
//! | 10 | 1 | `gain_denominator` |
//! | 11 | 1 | `reference` ([`AnalogReference`]) |
//! | 12 | 1 | `channel_count` |
//! | 13 | 2 | `vdd_mv`, the VDD measured once at start |
//! | 15 | 16 | `input_name`, UTF-8, NUL-padded |
//! | 31 | 24 | `board`, UTF-8, NUL-padded |
//! | 55 | 16 | `image_version`, UTF-8, NUL-padded |
//! | 71 | 4 | CRC-32/ISO-HDLC over bytes 0..71 |
//!
//! **Sample block**, `16 + 2 * channels * samples_per_block` bytes:
//!
//! | offset | bytes | field |
//! |---:|---:|---|
//! | 0 | 4 | magic, `EAB1` |
//! | 4 | 4 | `index`, the sample index (per channel) of the block's first frame |
//! | 8 | 4 | event word ([`EventWord`]) |
//! | 12 | `2 * channels * samples_per_block` | `i16` raw ADC codes, **frame-interleaved**: frame 0 channel 0, frame 0 channel 1, ..., frame 1 channel 0, ... |
//! | end - 4 | 4 | CRC-32/ISO-HDLC over every preceding byte of the block |
//!
//! Interleaved because that is the order a scanning ADC fills its buffer in,
//! so the firmware copies a buffer without reshuffling it. The channel count
//! and the block's frame count are **declared by the study** (a
//! [`crate::records::RecordFraming::AnalogBlocks`]) and the channel count is
//! repeated in the start header, which is how a reader cross-checks them; a
//! block carries neither.
//!
//! **Event word**: bits 0..=7 `edge_offset` (frame offset in the block of the
//! first marker edge), bit 8 `edge_level` (marker level after that edge, 1 =
//! high), bits 16..=23 `edge_count` (edges in this block; 0 means the other
//! fields are meaningless). Every other bit is reserved and is **kept, not
//! refused**, in [`EventWord::raw`].

use heapless::{String, Vec};

use crate::limits::MAX_GAPS_REPORTED;
use crate::records::RecordReport;

/// The first four bytes of a sample block.
pub const BLOCK_MAGIC: [u8; 4] = *b"EAB1";
/// The first four bytes of a start header.
pub const START_MAGIC: [u8; 4] = *b"EAH1";
/// Frames per block the owner fixed (decision 7): 256 samples per channel.
pub const DEFAULT_SAMPLES_PER_BLOCK: u16 = 256;

/// Bytes before a block's samples: magic, index, event word.
pub const BLOCK_PREFIX_BYTES: usize = 12;
/// The trailing CRC-32.
pub const CRC_BYTES: usize = 4;
/// Width of the `input_name` field.
pub const INPUT_NAME_BYTES: usize = 16;
/// Width of the `board` field.
pub const BOARD_BYTES: usize = 24;
/// Width of the `image_version` field.
pub const IMAGE_VERSION_BYTES: usize = 16;
/// Length of the whole start header, CRC included.
pub const START_HEADER_BYTES: usize = 4 + 4 + 1 + 1 + 1 + 1 + 1 + 2
    + INPUT_NAME_BYTES
    + BOARD_BYTES
    + IMAGE_VERSION_BYTES
    + CRC_BYTES;

const _: () = assert!(START_HEADER_BYTES == 75);

/// CRC-32/ISO-HDLC of `bytes` — the checksum every block and the header carry.
pub fn crc32(bytes: &[u8]) -> u32 {
    crate::crc::crc32_ieee(bytes)
}

/// Length in bytes of one block, or `None` for zero channels, zero frames or
/// a size that overflows.
pub const fn block_len(channels: u8, samples_per_block: u16) -> Option<usize> {
    if channels == 0 || samples_per_block == 0 {
        return None;
    }
    Some(BLOCK_PREFIX_BYTES + CRC_BYTES + 2 * (channels as usize) * (samples_per_block as usize))
}

fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The marker information of one block (see the module docs for the bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventWord(u32);

impl EventWord {
    /// Builds the word. `edge_offset` and `edge_count` use 8 bits each.
    pub const fn new(edge_count: u8, edge_offset: u8, edge_level: bool) -> Self {
        EventWord(((edge_count as u32) << 16) | ((edge_level as u32) << 8) | edge_offset as u32)
    }
    /// Wraps a word as read off the wire, reserved bits included.
    pub const fn from_raw(raw: u32) -> Self {
        EventWord(raw)
    }
    /// The word as carried, reserved bits included.
    pub const fn raw(self) -> u32 {
        self.0
    }
    /// Marker edges in the block.
    pub const fn edge_count(self) -> u8 {
        (self.0 >> 16) as u8
    }
    /// Frame offset of the first edge, `None` when the block has none.
    pub const fn edge_offset(self) -> Option<u8> {
        if self.edge_count() == 0 {
            None
        } else {
            Some(self.0 as u8)
        }
    }
    /// Marker level after the first edge, `None` when the block has none.
    pub const fn edge_level(self) -> Option<bool> {
        if self.edge_count() == 0 {
            None
        } else {
            Some(self.0 & 0x100 != 0)
        }
    }
}

/// Why a block was refused. Every variant says what was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// The declared channel count or frame count is zero.
    BadLayout,
    /// The slice is not exactly one block long.
    WrongLength { expected: usize, got: usize },
    /// The first four bytes are not [`BLOCK_MAGIC`].
    BadMagic,
    /// The trailing CRC does not match the bytes before it.
    CrcMismatch { stored: u32, computed: u32 },
}

impl core::fmt::Display for BlockError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BlockError::BadLayout => write!(f, "a block layout needs at least one channel and one frame"),
            BlockError::WrongLength { expected, got } => {
                write!(f, "a block is {expected} bytes for this layout; got {got}")
            }
            BlockError::BadMagic => write!(f, "the block does not begin with the EAB1 magic"),
            BlockError::CrcMismatch { stored, computed } => write!(
                f,
                "the block's CRC-32 is {stored:#010x} but its bytes compute {computed:#010x}"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for BlockError {}

/// One decoded, CRC-verified block. Borrows the bytes it was decoded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalogBlock<'a> {
    /// Sample index (per channel) of the block's first frame.
    pub index: u32,
    pub events: EventWord,
    pub channels: u8,
    pub frames: u16,
    samples: &'a [u8],
}

impl AnalogBlock<'_> {
    /// The code of `channel` at `frame`, `None` outside the block.
    pub fn sample(&self, frame: u16, channel: u8) -> Option<i16> {
        if frame >= self.frames || channel >= self.channels {
            return None;
        }
        let at = 2 * (usize::from(frame) * usize::from(self.channels) + usize::from(channel));
        Some(i16::from_le_bytes([self.samples[at], self.samples[at + 1]]))
    }

    /// Every code, frame-interleaved (`frame 0 ch 0, frame 0 ch 1, ...`).
    pub fn codes(&self) -> impl Iterator<Item = i16> + '_ {
        self.samples
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
    }
}

/// Decodes one block and checks its CRC-32/ISO-HDLC. `bytes` must be exactly
/// one block ([`block_len`]); a longer or shorter slice is refused, not
/// trimmed.
pub fn decode_block(bytes: &[u8], channels: u8, samples_per_block: u16) -> Result<AnalogBlock<'_>, BlockError> {
    let expected = block_len(channels, samples_per_block).ok_or(BlockError::BadLayout)?;
    if bytes.len() != expected {
        return Err(BlockError::WrongLength { expected, got: bytes.len() });
    }
    if bytes[..4] != BLOCK_MAGIC {
        return Err(BlockError::BadMagic);
    }
    let body = expected - CRC_BYTES;
    let stored = le_u32(bytes, body);
    let computed = crc32(&bytes[..body]);
    if stored != computed {
        return Err(BlockError::CrcMismatch { stored, computed });
    }
    Ok(AnalogBlock {
        index: le_u32(bytes, 4),
        events: EventWord(le_u32(bytes, 8)),
        channels,
        frames: samples_per_block,
        samples: &bytes[BLOCK_PREFIX_BYTES..body],
    })
}

/// Writes one block into `out` (fixtures, tests, and a firmware-side check),
/// returning its length. `codes` is frame-interleaved and must hold exactly
/// `channels * samples_per_block` values.
pub fn encode_block(
    out: &mut [u8],
    index: u32,
    events: EventWord,
    codes: &[i16],
    channels: u8,
    samples_per_block: u16,
) -> Result<usize, BlockError> {
    let len = block_len(channels, samples_per_block).ok_or(BlockError::BadLayout)?;
    if out.len() < len {
        return Err(BlockError::WrongLength { expected: len, got: out.len() });
    }
    if codes.len() != (len - BLOCK_PREFIX_BYTES - CRC_BYTES) / 2 {
        return Err(BlockError::WrongLength { expected: len, got: BLOCK_PREFIX_BYTES + CRC_BYTES + 2 * codes.len() });
    }
    out[..4].copy_from_slice(&BLOCK_MAGIC);
    out[4..8].copy_from_slice(&index.to_le_bytes());
    out[8..12].copy_from_slice(&events.raw().to_le_bytes());
    for (i, code) in codes.iter().enumerate() {
        let at = BLOCK_PREFIX_BYTES + 2 * i;
        out[at..at + 2].copy_from_slice(&code.to_le_bytes());
    }
    let crc = crc32(&out[..len - CRC_BYTES]);
    out[len - CRC_BYTES..len].copy_from_slice(&crc.to_le_bytes());
    Ok(len)
}

/// What the ADC's reference was. Raw value kept for anything not named here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalogReference {
    /// 0: the chip's internal reference.
    Internal,
    /// 1: VDD divided by 4 (decision 2's configuration).
    VddDiv4,
    /// Any other code, carried unchanged.
    Other(u8),
}

impl AnalogReference {
    pub const fn from_code(code: u8) -> Self {
        match code {
            0 => AnalogReference::Internal,
            1 => AnalogReference::VddDiv4,
            other => AnalogReference::Other(other),
        }
    }
    pub const fn code(self) -> u8 {
        match self {
            AnalogReference::Internal => 0,
            AnalogReference::VddDiv4 => 1,
            AnalogReference::Other(c) => c,
        }
    }
}

/// The decoded start header (decision 8). Raw codes on the wire; volts are
/// derived from this by the reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartHeader {
    pub sample_rate_hz: u32,
    pub resolution_bits: u8,
    pub gain_numerator: u8,
    pub gain_denominator: u8,
    pub reference: AnalogReference,
    pub channel_count: u8,
    /// VDD measured once at session start, in millivolts.
    pub vdd_mv: u16,
    pub input_name: String<INPUT_NAME_BYTES>,
    pub board: String<BOARD_BYTES>,
    pub image_version: String<IMAGE_VERSION_BYTES>,
}

/// Why a start header was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderError {
    WrongLength { expected: usize, got: usize },
    /// The first four bytes are not [`START_MAGIC`].
    BadMagic,
    CrcMismatch { stored: u32, computed: u32 },
    /// A text field is not UTF-8, or has a non-NUL byte after its first NUL.
    BadText { field: &'static str },
    /// A rate, channel count or resolution of zero, or a zero gain
    /// denominator: a session that could not have sampled anything.
    ZeroField { field: &'static str },
}

impl core::fmt::Display for HeaderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HeaderError::WrongLength { expected, got } => {
                write!(f, "a start header is {expected} bytes; got {got}")
            }
            HeaderError::BadMagic => write!(f, "the start header does not begin with the EAH1 magic"),
            HeaderError::CrcMismatch { stored, computed } => write!(
                f,
                "the start header's CRC-32 is {stored:#010x} but its bytes compute {computed:#010x}"
            ),
            HeaderError::BadText { field } => {
                write!(f, "the start header's {field} is not NUL-padded UTF-8")
            }
            HeaderError::ZeroField { field } => write!(f, "the start header declares {field} as zero"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for HeaderError {}

fn padded_text<const N: usize>(field: &'static str, raw: &[u8]) -> Result<String<N>, HeaderError> {
    let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
    if raw[end..].iter().any(|b| *b != 0) {
        return Err(HeaderError::BadText { field });
    }
    let text = core::str::from_utf8(&raw[..end]).map_err(|_| HeaderError::BadText { field })?;
    String::try_from(text).map_err(|_| HeaderError::BadText { field })
}

/// Decodes the start header and checks its CRC-32/ISO-HDLC. `bytes` must be
/// exactly [`START_HEADER_BYTES`] long.
pub fn decode_start_header(bytes: &[u8]) -> Result<StartHeader, HeaderError> {
    if bytes.len() != START_HEADER_BYTES {
        return Err(HeaderError::WrongLength { expected: START_HEADER_BYTES, got: bytes.len() });
    }
    if bytes[..4] != START_MAGIC {
        return Err(HeaderError::BadMagic);
    }
    let body = START_HEADER_BYTES - CRC_BYTES;
    let stored = le_u32(bytes, body);
    let computed = crc32(&bytes[..body]);
    if stored != computed {
        return Err(HeaderError::CrcMismatch { stored, computed });
    }
    let sample_rate_hz = le_u32(bytes, 4);
    let (resolution_bits, gain_numerator, gain_denominator, channel_count) =
        (bytes[8], bytes[9], bytes[10], bytes[12]);
    for (field, zero) in [
        ("sample_rate_hz", sample_rate_hz == 0),
        ("resolution_bits", resolution_bits == 0),
        ("gain_numerator", gain_numerator == 0),
        ("gain_denominator", gain_denominator == 0),
        ("channel_count", channel_count == 0),
    ] {
        if zero {
            return Err(HeaderError::ZeroField { field });
        }
    }
    let name_at = 15;
    let board_at = name_at + INPUT_NAME_BYTES;
    let image_at = board_at + BOARD_BYTES;
    Ok(StartHeader {
        sample_rate_hz,
        resolution_bits,
        gain_numerator,
        gain_denominator,
        reference: AnalogReference::from_code(bytes[11]),
        channel_count,
        vdd_mv: u16::from_le_bytes([bytes[13], bytes[14]]),
        input_name: padded_text("input_name", &bytes[name_at..board_at])?,
        board: padded_text("board", &bytes[board_at..image_at])?,
        image_version: padded_text("image_version", &bytes[image_at..body])?,
    })
}

/// Writes a start header into `out`, returning its length. Text longer than
/// its field is refused, never cut.
pub fn encode_start_header(out: &mut [u8], header: &StartHeader) -> Result<usize, HeaderError> {
    if out.len() < START_HEADER_BYTES {
        return Err(HeaderError::WrongLength { expected: START_HEADER_BYTES, got: out.len() });
    }
    out[..START_HEADER_BYTES].fill(0);
    out[..4].copy_from_slice(&START_MAGIC);
    out[4..8].copy_from_slice(&header.sample_rate_hz.to_le_bytes());
    out[8] = header.resolution_bits;
    out[9] = header.gain_numerator;
    out[10] = header.gain_denominator;
    out[11] = header.reference.code();
    out[12] = header.channel_count;
    out[13..15].copy_from_slice(&header.vdd_mv.to_le_bytes());
    let name_at = 15;
    let board_at = name_at + INPUT_NAME_BYTES;
    let image_at = board_at + BOARD_BYTES;
    out[name_at..name_at + header.input_name.len()].copy_from_slice(header.input_name.as_bytes());
    out[board_at..board_at + header.board.len()].copy_from_slice(header.board.as_bytes());
    out[image_at..image_at + header.image_version.len()].copy_from_slice(header.image_version.as_bytes());
    let body = START_HEADER_BYTES - CRC_BYTES;
    let crc = crc32(&out[..body]);
    out[body..START_HEADER_BYTES].copy_from_slice(&crc.to_le_bytes());
    Ok(START_HEADER_BYTES)
}

/// How a block's index compares with the one expected after the last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexStep {
    /// The first block seen, or exactly the expected index.
    InOrder,
    /// Blocks were skipped: `missing_samples` frames per channel are absent.
    Gap { expected: u32, got: u32, missing_samples: u32 },
    /// The index went backwards or repeated.
    Backwards { expected: u32, got: u32 },
}

/// Follows block indices and says where samples are missing. Pure state, so
/// Core can feed it live and a test can feed it a list.
#[derive(Debug, Clone, Copy, Default)]
pub struct IndexTracker {
    next: Option<u32>,
}

impl IndexTracker {
    pub const fn new() -> Self {
        IndexTracker { next: None }
    }

    /// Records a block that starts at `index` and holds `frames` frames.
    pub fn observe(&mut self, index: u32, frames: u16) -> IndexStep {
        let step = match self.next {
            None => IndexStep::InOrder,
            Some(expected) if expected == index => IndexStep::InOrder,
            Some(expected) if index > expected => {
                IndexStep::Gap { expected, got: index, missing_samples: index - expected }
            }
            Some(expected) => IndexStep::Backwards { expected, got: index },
        };
        self.next = Some(index.wrapping_add(u32::from(frames)));
        step
    }
}

/// One place samples are missing, by index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexGap {
    /// The index the next block should have started at.
    pub expected: u32,
    /// The index it started at.
    pub got: u32,
}

/// What [`verify_blocks`] found in one captured byte stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockReport {
    /// Header and blocks counted like any other record check: `total` is the
    /// header plus every block-shaped span, `verified` those whose CRC held.
    pub records: RecordReport,
    /// Whether the stream began with a start header that decoded.
    pub header_ok: bool,
    /// Gaps between verified blocks, capped at [`MAX_GAPS_REPORTED`].
    pub gaps: Vec<IndexGap, MAX_GAPS_REPORTED>,
    /// All gaps found; `gaps` is capped, this is not.
    pub gap_count: u32,
    /// Frames per channel missing across all gaps.
    pub missing_samples: u64,
    /// Index went backwards or repeated this many times.
    pub backwards_count: u32,
    /// Marker edges counted from verified blocks' event words.
    pub marker_edges: u32,
}

fn find_block_magic(bytes: &[u8], from: usize) -> Option<usize> {
    if from >= bytes.len() {
        return None;
    }
    bytes[from..]
        .windows(BLOCK_MAGIC.len())
        .position(|w| w == BLOCK_MAGIC)
        .map(|p| from + p)
}

fn clamp_u32(v: usize) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

/// Checks a whole capture: the start header, then every block, placing
/// missing samples by index (decision 105). Pure and allocation-free.
///
/// After a block that does not verify the scan moves to the next block magic,
/// so one damaged block costs one block. The damaged block's own index is
/// unknown, so its loss shows as a gap in the index sequence of the blocks
/// around it.
pub fn verify_blocks(bytes: &[u8], channels: u8, samples_per_block: u16) -> BlockReport {
    let mut report = BlockReport {
        records: RecordReport { total: 0, verified: 0, bad_offsets: Vec::new(), leading_bytes: 0 },
        header_ok: false,
        gaps: Vec::new(),
        gap_count: 0,
        missing_samples: 0,
        backwards_count: 0,
        marker_edges: 0,
    };
    let Some(len) = block_len(channels, samples_per_block) else {
        report.records.leading_bytes = clamp_u32(bytes.len());
        return report;
    };

    let mut pos;
    if bytes.len() >= START_MAGIC.len() && bytes[..4] == START_MAGIC {
        report.records.total += 1;
        let ok = bytes.len() >= START_HEADER_BYTES
            && decode_start_header(&bytes[..START_HEADER_BYTES])
                .is_ok_and(|h| h.channel_count == channels);
        if ok {
            report.records.verified += 1;
            report.header_ok = true;
            pos = START_HEADER_BYTES;
        } else {
            let _ = report.records.bad_offsets.push(0);
            pos = find_block_magic(bytes, 1).unwrap_or(bytes.len());
        }
    } else {
        let first = find_block_magic(bytes, 0).unwrap_or(bytes.len());
        report.records.leading_bytes = clamp_u32(first);
        pos = first;
    }

    let mut tracker = IndexTracker::new();
    while pos < bytes.len() {
        report.records.total += 1;
        let end = pos + len;
        let decoded = if bytes[pos..].starts_with(&BLOCK_MAGIC) && end <= bytes.len() {
            decode_block(&bytes[pos..end], channels, samples_per_block).ok()
        } else {
            None
        };
        match decoded {
            Some(block) => {
                report.records.verified += 1;
                report.marker_edges += u32::from(block.events.edge_count());
                match tracker.observe(block.index, samples_per_block) {
                    IndexStep::InOrder => {}
                    IndexStep::Gap { expected, got, missing_samples } => {
                        report.gap_count += 1;
                        report.missing_samples += u64::from(missing_samples);
                        let _ = report.gaps.push(IndexGap { expected, got });
                    }
                    IndexStep::Backwards { .. } => report.backwards_count += 1,
                }
                pos = end;
            }
            None => {
                let _ = report.records.bad_offsets.push(clamp_u32(pos));
                pos = find_block_magic(bytes, pos + 1).unwrap_or(bytes.len());
            }
        }
    }
    report
}

/// Why a study's analog-capture declarations do not fit together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalogCaptureError {
    /// An `AnalogInput` tap no `AnalogBlocks` record check names, so nothing
    /// would say what its bytes are or check them.
    NoBlockCheck { tap: usize },
    /// An `AnalogBlocks` record check names a `stream_id` no tap has.
    CheckNamesNoTap { check: usize, stream_id: u8 },
    /// An `AnalogBlocks` record check names a tap that is not `Raw`.
    CheckTapNotRaw { check: usize, stream_id: u8 },
    /// An `AnalogInput` tap with no `Study.session`: a recording with no cap
    /// runs at about 64 kB/s until something stops it.
    NeedsSession { tap: usize },
    /// The declared session is unusable.
    Session(crate::session::SessionError),
}

impl core::fmt::Display for AnalogCaptureError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AnalogCaptureError::NoBlockCheck { tap } => write!(
                f,
                "streams[{tap}] is an analog input but no record_checks entry declares its \
                 AnalogBlocks layout"
            ),
            AnalogCaptureError::CheckNamesNoTap { check, stream_id } => {
                write!(f, "record_checks[{check}] names stream id {stream_id}, which no tap has")
            }
            AnalogCaptureError::CheckTapNotRaw { check, stream_id } => write!(
                f,
                "record_checks[{check}] declares analog blocks for streams[{stream_id}], whose \
                 encoding is not Raw"
            ),
            AnalogCaptureError::NeedsSession { tap } => write!(
                f,
                "streams[{tap}] is an analog input, so the study must declare a session with a \
                 time cap"
            ),
            AnalogCaptureError::Session(e) => write!(f, "{e}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for AnalogCaptureError {}

impl crate::study::Study {
    /// Cross-checks a study's analog-capture declarations and validates its
    /// session (decisions 104 to 106). Run after
    /// [`validate_taps`](crate::streams::validate_taps) and
    /// [`validate_record_checks`](crate::records::validate_record_checks),
    /// which own the per-tap and per-layout rules. A study with no analog
    /// tap, no `AnalogBlocks` check and no session passes. Additive: nothing
    /// calls it implicitly.
    pub fn validate_analog_capture(&self) -> Result<(), AnalogCaptureError> {
        use crate::records::RecordFraming;
        use crate::streams::{StreamEncoding, StreamSource};

        for (check, c) in self.record_checks.iter().enumerate() {
            if !matches!(c.framing, RecordFraming::AnalogBlocks { .. }) {
                continue;
            }
            let tap = self
                .streams
                .iter()
                .find(|t| t.id == c.stream_id)
                .ok_or(AnalogCaptureError::CheckNamesNoTap { check, stream_id: c.stream_id })?;
            if tap.encoding != StreamEncoding::Raw {
                return Err(AnalogCaptureError::CheckTapNotRaw { check, stream_id: c.stream_id });
            }
        }
        for (index, tap) in self.streams.iter().enumerate() {
            if !matches!(tap.source, StreamSource::AnalogInput { .. }) {
                continue;
            }
            let declared = self.record_checks.iter().any(|c| {
                c.stream_id == tap.id && matches!(c.framing, RecordFraming::AnalogBlocks { .. })
            });
            if !declared {
                return Err(AnalogCaptureError::NoBlockCheck { tap: index });
            }
            if self.session.is_none() {
                return Err(AnalogCaptureError::NeedsSession { tap: index });
            }
        }
        if let Some(session) = &self.session {
            crate::session::validate_session(session).map_err(AnalogCaptureError::Session)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CH: u8 = 1;
    const N: u16 = DEFAULT_SAMPLES_PER_BLOCK;

    /// `heapless`, not `std::vec`: a plain `cargo test` runs the no_std path.
    type Cap = Vec<u8, 32768>;

    fn block(index: u32, events: EventWord, seed: i16) -> Cap {
        let mut codes: Vec<i16, 256> = Vec::new();
        for i in 0..N as i32 {
            codes.push(seed.wrapping_add(i as i16)).unwrap();
        }
        let mut out = Cap::new();
        out.resize(block_len(CH, N).unwrap(), 0).unwrap();
        encode_block(&mut out, index, events, &codes, CH, N).unwrap();
        out
    }

    fn header() -> StartHeader {
        StartHeader {
            sample_rate_hz: 32_000,
            resolution_bits: 12,
            gain_numerator: 1,
            gain_denominator: 4,
            reference: AnalogReference::VddDiv4,
            channel_count: CH,
            vdd_mv: 3_300,
            input_name: String::try_from("A0").unwrap(),
            board: String::try_from("dev kit").unwrap(),
            image_version: String::try_from("v1.0.0-abc").unwrap(),
        }
    }

    fn header_bytes() -> Cap {
        let mut out = Cap::new();
        out.resize(START_HEADER_BYTES, 0).unwrap();
        encode_start_header(&mut out, &header()).unwrap();
        out
    }

    #[test]
    fn a_one_channel_block_is_528_bytes() {
        assert_eq!(block_len(1, 256), Some(528));
        assert_eq!(block_len(0, 256), None);
        assert_eq!(block_len(1, 0), None);
    }

    #[test]
    fn a_block_round_trips_and_places_its_fields() {
        let b = block(512, EventWord::new(1, 17, true), -100);
        let d = decode_block(&b, CH, N).unwrap();
        assert_eq!(d.index, 512);
        assert_eq!(d.events.edge_count(), 1);
        assert_eq!(d.events.edge_offset(), Some(17));
        assert_eq!(d.events.edge_level(), Some(true));
        assert_eq!(d.sample(0, 0), Some(-100));
        assert_eq!(d.sample(255, 0), Some(155));
        assert_eq!(d.sample(256, 0), None);
        assert_eq!(d.sample(0, 1), None);
        assert_eq!(d.codes().count(), 256);
    }

    #[test]
    fn a_block_is_pinned_byte_for_byte() {
        // Two frames, two channels, so the interleave order is fixed by a test.
        let mut out = [0u8; 16 + 2 * 2 * 2];
        let n = encode_block(&mut out, 0x0102_0304, EventWord::new(0, 0, false), &[1, -2, 3, 4], 2, 2).unwrap();
        assert_eq!(n, 24);
        assert_eq!(&out[..4], b"EAB1");
        assert_eq!(&out[4..8], &[0x04, 0x03, 0x02, 0x01]);
        assert_eq!(&out[8..12], &[0, 0, 0, 0]);
        assert_eq!(&out[12..20], &[1, 0, 0xFE, 0xFF, 3, 0, 4, 0]);
        assert_eq!(le_u32(&out, 20), crc32(&out[..20]));
    }

    #[test]
    fn the_crc_is_iso_hdlc() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn a_corrupted_crc_is_refused_with_both_values() {
        let mut b = block(0, EventWord::new(0, 0, false), 0);
        let last = b.len() - 1;
        b[last] ^= 0x01;
        match decode_block(&b, CH, N) {
            Err(BlockError::CrcMismatch { stored, computed }) => assert_ne!(stored, computed),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_flipped_sample_byte_fails_the_crc() {
        let mut b = block(0, EventWord::new(0, 0, false), 0);
        b[40] ^= 0x80;
        assert!(matches!(decode_block(&b, CH, N), Err(BlockError::CrcMismatch { .. })));
    }

    #[test]
    fn a_wrong_length_or_magic_is_refused() {
        let b = block(0, EventWord::new(0, 0, false), 0);
        assert_eq!(decode_block(&b[..b.len() - 1], CH, N), Err(BlockError::WrongLength { expected: 528, got: 527 }));
        assert_eq!(decode_block(&b, 0, N), Err(BlockError::BadLayout));
        let mut bad = b.clone();
        bad[0] = b'X';
        assert_eq!(decode_block(&bad, CH, N), Err(BlockError::BadMagic));
    }

    #[test]
    fn an_event_word_without_an_edge_names_none_and_keeps_reserved_bits() {
        let w = EventWord::from_raw(0xFF00_0000 | 0x0000_01FF);
        assert_eq!(w.edge_count(), 0);
        assert_eq!(w.edge_offset(), None);
        assert_eq!(w.edge_level(), None);
        assert_eq!(w.raw(), 0xFF00_01FF);
    }

    #[test]
    fn a_start_header_round_trips() {
        let b = header_bytes();
        assert_eq!(b.len(), 75);
        assert_eq!(&b[..4], b"EAH1");
        assert_eq!(decode_start_header(&b).unwrap(), header());
    }

    #[test]
    fn a_start_header_is_pinned_byte_for_byte() {
        let b = header_bytes();
        assert_eq!(&b[4..8], &32_000u32.to_le_bytes());
        assert_eq!(&b[8..13], &[12, 1, 4, 1, 1]);
        assert_eq!(&b[13..15], &3_300u16.to_le_bytes());
        assert_eq!(&b[15..17], b"A0");
        assert!(b[17..31].iter().all(|x| *x == 0));
        assert_eq!(&b[31..38], b"dev kit");
        assert_eq!(&b[55..65], b"v1.0.0-abc");
    }

    #[test]
    fn a_corrupted_or_malformed_header_is_refused() {
        let mut b = header_bytes();
        b[20] ^= 1;
        assert!(matches!(decode_start_header(&b), Err(HeaderError::CrcMismatch { .. })));
        assert_eq!(decode_start_header(&b[..74]), Err(HeaderError::WrongLength { expected: 75, got: 74 }));

        let mut b = header_bytes();
        b[0] = 0;
        assert_eq!(decode_start_header(&b), Err(HeaderError::BadMagic));

        // text after a NUL, with the CRC repaired so only the text is wrong
        let mut b = header_bytes();
        b[18] = b'x';
        let crc = crc32(&b[..71]);
        b[71..].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode_start_header(&b), Err(HeaderError::BadText { field: "input_name" }));

        let mut h = header();
        h.sample_rate_hz = 0;
        let mut b = Cap::new();
        b.resize(75, 0).unwrap();
        encode_start_header(&mut b, &h).unwrap();
        assert_eq!(decode_start_header(&b), Err(HeaderError::ZeroField { field: "sample_rate_hz" }));
    }

    #[test]
    fn the_tracker_places_a_skipped_index() {
        let mut t = IndexTracker::new();
        assert_eq!(t.observe(0, 256), IndexStep::InOrder);
        assert_eq!(t.observe(256, 256), IndexStep::InOrder);
        assert_eq!(t.observe(768, 256), IndexStep::Gap { expected: 512, got: 768, missing_samples: 256 });
        assert_eq!(t.observe(512, 256), IndexStep::Backwards { expected: 1024, got: 512 });
    }

    fn capture(parts: &[&[u8]]) -> Cap {
        let mut out = Cap::new();
        for p in parts {
            out.extend_from_slice(p).unwrap();
        }
        out
    }

    #[test]
    fn a_clean_capture_verifies_with_no_gaps() {
        let hdr = header_bytes();
        let b0 = block(0, EventWord::new(0, 0, false), 0);
        let b1 = block(256, EventWord::new(2, 9, true), 1);
        let b2 = block(512, EventWord::new(0, 0, false), 2);
        let r = verify_blocks(&capture(&[&hdr, &b0, &b1, &b2]), CH, N);
        assert!(r.header_ok);
        assert_eq!((r.records.total, r.records.verified), (4, 4));
        assert!(r.records.all_verified());
        assert_eq!((r.gap_count, r.missing_samples, r.marker_edges), (0, 0, 2));
    }

    #[test]
    fn a_skipped_block_is_a_gap_by_index_and_nothing_is_bad() {
        let hdr = header_bytes();
        let b0 = block(0, EventWord::new(0, 0, false), 0);
        let b2 = block(512, EventWord::new(0, 0, false), 2);
        let r = verify_blocks(&capture(&[&hdr, &b0, &b2]), CH, N);
        assert!(r.records.all_verified());
        assert_eq!(r.gap_count, 1);
        assert_eq!(r.missing_samples, 256);
        assert_eq!(r.gaps.as_slice(), &[IndexGap { expected: 256, got: 512 }]);
    }

    #[test]
    fn a_corrupted_block_is_named_by_offset_and_shows_as_a_gap() {
        let hdr = header_bytes();
        let b0 = block(0, EventWord::new(0, 0, false), 0);
        let mut b1 = block(256, EventWord::new(0, 0, false), 1);
        let b2 = block(512, EventWord::new(0, 0, false), 2);
        let last = b1.len() - 1;
        b1[last] ^= 0xFF;
        let r = verify_blocks(&capture(&[&hdr, &b0, &b1, &b2]), CH, N);
        assert_eq!((r.records.total, r.records.verified), (4, 3));
        assert_eq!(r.records.bad_offsets.as_slice(), &[(75 + 528) as u32]);
        assert!(!r.records.all_verified());
        assert_eq!((r.gap_count, r.missing_samples), (1, 256));
    }

    #[test]
    fn a_truncated_tail_is_one_bad_record() {
        let hdr = header_bytes();
        let b0 = block(0, EventWord::new(0, 0, false), 0);
        let b1 = block(256, EventWord::new(0, 0, false), 1);
        let r = verify_blocks(&capture(&[&hdr, &b0, &b1[..100]]), CH, N);
        assert_eq!((r.records.total, r.records.verified), (3, 2));
        assert_eq!(r.records.bad_offsets.as_slice(), &[(75 + 528) as u32]);
    }

    #[test]
    fn a_capture_with_no_header_counts_its_leading_bytes() {
        let b0 = block(0, EventWord::new(0, 0, false), 0);
        let r = verify_blocks(&capture(&[&[1, 2, 3], &b0]), CH, N);
        assert!(!r.header_ok);
        assert_eq!(r.records.leading_bytes, 3);
        assert!(!r.records.all_verified());
        assert_eq!(r.records.verified, 1);
    }

    #[test]
    fn a_header_that_disagrees_on_channels_is_bad() {
        let hdr = header_bytes();
        let r = verify_blocks(&hdr, 2, N);
        assert!(!r.header_ok);
        assert_eq!(r.records.bad_offsets.as_slice(), &[0]);
    }

    #[test]
    fn the_gap_list_caps_but_the_count_does_not() {
        let hdr = header_bytes();
        let mut cap = hdr.clone();
        for i in 0..(MAX_GAPS_REPORTED as u32 + 8) {
            cap.extend_from_slice(&block(i * 512, EventWord::new(0, 0, false), 0)).unwrap();
        }
        let r = verify_blocks(&cap, CH, N);
        assert_eq!(r.gaps.len(), MAX_GAPS_REPORTED);
        assert_eq!(r.gap_count, MAX_GAPS_REPORTED as u32 + 7);
    }

    use crate::records::{RecordCheck, RecordFraming};
    use crate::session::{RunField, SessionSpec};
    use crate::streams::{StreamEncoding, StreamScope, StreamSource, StreamTap, validate_taps};
    use crate::study::Study;

    fn analog_tap(id: u8) -> StreamTap {
        StreamTap {
            id,
            name: String::try_from("mic").unwrap(),
            source: StreamSource::AnalogInput { sample_hz: 32_000, channel: 0 },
            encoding: StreamEncoding::Raw,
            scope: StreamScope::WholeStudy,
        }
    }

    fn blocks_check(stream_id: u8) -> RecordCheck {
        RecordCheck {
            stream_id,
            framing: RecordFraming::AnalogBlocks { channels: CH, samples_per_block: N },
        }
    }

    fn a_session() -> SessionSpec {
        let mut run_fields = Vec::new();
        run_fields
            .push(RunField {
                name: String::try_from("sensor").unwrap(),
                label: String::try_from("Sensor serial").unwrap(),
                required: true,
            })
            .unwrap();
        SessionSpec {
            start_text: String::try_from("Start").unwrap(),
            run_fields,
            done_text: String::try_from("Done").unwrap(),
            max_duration_ms: 60_000,
            export_dir: String::try_from("exports").unwrap(),
        }
    }

    fn study_with(tap: Option<StreamTap>, check: Option<RecordCheck>, session: bool) -> Study {
        let mut s = crate::tests::sample_study();
        s.streams = Default::default();
        if let Some(t) = tap {
            s.streams.push(t).unwrap();
        }
        if let Some(c) = check {
            s.record_checks.push(c).unwrap();
        }
        s.session = session.then(a_session);
        s
    }

    #[test]
    fn a_study_declaring_the_whole_session_validates() {
        let s = study_with(Some(analog_tap(0)), Some(blocks_check(0)), true);
        assert_eq!(validate_taps(&s.streams, s.steps.len() as u32, 0), Ok(()));
        assert_eq!(crate::records::validate_record_checks(&s.record_checks), Ok(()));
        assert_eq!(s.validate_analog_capture(), Ok(()));
        // and a study with none of it is untouched
        assert_eq!(study_with(None, None, false).validate_analog_capture(), Ok(()));
    }

    #[test]
    fn the_study_level_cross_checks_refuse_with_a_reason() {
        assert_eq!(
            study_with(Some(analog_tap(0)), None, true).validate_analog_capture(),
            Err(AnalogCaptureError::NoBlockCheck { tap: 0 })
        );
        assert_eq!(
            study_with(Some(analog_tap(0)), Some(blocks_check(0)), false).validate_analog_capture(),
            Err(AnalogCaptureError::NeedsSession { tap: 0 })
        );
        assert_eq!(
            study_with(Some(analog_tap(0)), Some(blocks_check(3)), true).validate_analog_capture(),
            Err(AnalogCaptureError::CheckNamesNoTap { check: 0, stream_id: 3 })
        );
        let mut text = analog_tap(0);
        text.source = StreamSource::DevBenchLog;
        text.encoding = StreamEncoding::Text;
        assert_eq!(
            study_with(Some(text), Some(blocks_check(0)), true).validate_analog_capture(),
            Err(AnalogCaptureError::CheckTapNotRaw { check: 0, stream_id: 0 })
        );
        let mut s = study_with(Some(analog_tap(0)), Some(blocks_check(0)), true);
        s.session.as_mut().unwrap().max_duration_ms = 300_001;
        assert!(matches!(s.validate_analog_capture(), Err(AnalogCaptureError::Session(_))));
    }

    #[test]
    fn a_malformed_tap_or_layout_is_refused() {
        use crate::streams::StreamTapError as E;
        let check = |t: StreamTap| {
            let mut v = heapless::Vec::<StreamTap, { crate::limits::MAX_STREAMS_PER_STUDY }>::new();
            v.push(t).unwrap();
            validate_taps(&v, 2, 0)
        };
        let mut t = analog_tap(0);
        t.source = StreamSource::AnalogInput { sample_hz: 0, channel: 0 };
        assert_eq!(check(t), Err(E::AnalogRateOutOfRange { index: 0, sample_hz: 0 }));
        let mut t = analog_tap(0);
        t.source = StreamSource::AnalogInput { sample_hz: 200_001, channel: 0 };
        assert!(matches!(check(t), Err(E::AnalogRateOutOfRange { .. })));
        let mut t = analog_tap(0);
        t.source = StreamSource::AnalogInput { sample_hz: 32_000, channel: 8 };
        assert_eq!(check(t), Err(E::AnalogChannelOutOfRange { index: 0, channel: 8 }));
        let mut t = analog_tap(0);
        t.encoding = StreamEncoding::Text;
        assert_eq!(check(t), Err(E::AnalogNotRaw { index: 0 }));
        let mut t = analog_tap(0);
        t.scope = StreamScope::Steps { from: 0, to: 1 };
        assert_eq!(check(t), Err(E::AnalogNotWholeStudy { index: 0 }));

        let mut two = heapless::Vec::<StreamTap, { crate::limits::MAX_STREAMS_PER_STUDY }>::new();
        two.push(analog_tap(0)).unwrap();
        let mut second = analog_tap(1);
        second.name = String::try_from("mic2").unwrap();
        two.push(second).unwrap();
        assert_eq!(validate_taps(&two, 2, 0), Err(E::SecondAnalogInput { index: 1, first: 0 }));

        for (channels, samples_per_block, what) in
            [(0, 256, "channels"), (9, 256, "channels"), (1, 0, "samples_per_block"), (1, 4097, "samples_per_block")]
        {
            let bad = RecordCheck { stream_id: 0, framing: RecordFraming::AnalogBlocks { channels, samples_per_block } };
            assert_eq!(
                crate::records::validate_record_checks(&[bad]),
                Err(crate::records::RecordCheckError::BadAnalogLayout { index: 0, what })
            );
        }
    }

    #[test]
    fn verify_records_dispatches_an_analog_layout() {
        let hdr = header_bytes();
        let b0 = block(0, EventWord::new(0, 0, false), 0);
        let cap = capture(&[&hdr, &b0]);
        let framing = RecordFraming::AnalogBlocks { channels: CH, samples_per_block: N };
        let r = crate::records::verify_records(&cap, &framing);
        assert!(r.all_verified());
        assert_eq!(r, verify_blocks(&cap, CH, N).records);
    }

    #[test]
    fn a_study_with_a_session_and_an_analog_tap_round_trips() {
        let s = study_with(Some(analog_tap(0)), Some(blocks_check(0)), true);
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Study>(&json).unwrap(), s);
        let mut buf = [0u8; 4096];
        let bytes = postcard::to_slice(&s, &mut buf).unwrap();
        assert_eq!(postcard::from_bytes::<Study>(bytes).unwrap(), s);
    }

    #[test]
    fn a_study_saved_before_the_session_field_still_loads() {
        let s = study_with(None, None, false);
        let mut v: serde_json::Value = serde_json::to_value(&s).unwrap();
        v.as_object_mut().unwrap().remove("session");
        let back: Study = serde_json::from_value(v).unwrap();
        assert_eq!(back.session, None);
    }

    #[test]
    fn an_empty_capture_verifies_nothing() {
        let r = verify_blocks(&[], CH, N);
        assert_eq!(r.records.total, 0);
        assert!(!r.records.all_verified());
    }
}
