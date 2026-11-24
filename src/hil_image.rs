//! An image's load segments and identity, and the bench-firmware list's shape
//! (`interfaces/hil-image.md`). Host-only, behind the `hil-image` feature.
//!
//! A chip holds load segments, not a hex or ELF file, so a readback compares
//! segments. [`image_segments`] reduces a hex, an ELF or a bin-with-a-base to
//! the maximal contiguous address ranges it writes, each with the SHA-256 of
//! its bytes; [`identity`] is the SHA-256 of the canonical JSON of that list.
//! A hex and an ELF of one build give the same list. The Intel HEX and ELF
//! subsets needed are parsed by hand; the only dependency is `sha2`.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The artifact formats [`image_segments`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    /// Intel HEX.
    Hex,
    /// ELF32 or ELF64, either byte order; its `PT_LOAD` segments at their
    /// physical addresses.
    Elf,
    /// A raw binary, placed at the `base` the caller gives.
    Bin,
}

/// One maximal contiguous address range an image writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    /// First address written.
    pub address: u64,
    /// Number of bytes written.
    pub length: u64,
    /// Lowercase hex SHA-256 of those bytes.
    pub sha256: String,
}

/// Why an image could not be reduced to segments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageError {
    /// The hex file is malformed (line number, 1-based, and why).
    Hex(usize, &'static str),
    /// The ELF file is malformed or not a supported class.
    Elf(&'static str),
    /// A bin needs a base address and none was given.
    NoBase,
    /// Two ranges the image writes overlap at this address, or the range
    /// starting here runs past the top of the 64-bit address space.
    Overlap(u64),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::Hex(n, why) => write!(f, "hex line {n}: {why}"),
            ImageError::Elf(why) => write!(f, "elf: {why}"),
            ImageError::NoBase => write!(f, "a bin needs a base address"),
            ImageError::Overlap(a) => write!(f, "image writes address {a:#x} twice"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ImageError {}

/// One entry of `bench-firmware.json`'s `images`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchImage {
    /// The firmware app (for example `dev-bench`).
    pub app: String,
    /// The board it is built for.
    pub board: String,
    /// The release version of that firmware.
    pub version: String,
    /// The source commit it was built from.
    pub commit: String,
    /// [`identity`] of `segments`.
    pub identity: String,
    /// The image's load segments.
    pub segments: Vec<Segment>,
}

/// `bench-firmware.json`: the bench firmware an installed release ships.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchFirmwareList {
    /// Schema version of this file ([`BENCH_FIRMWARE_SCHEMA`]).
    pub schema: u32,
    /// The release this list belongs to.
    pub release: String,
    /// One entry per bench firmware image.
    pub images: Vec<BenchImage>,
}

/// The `schema` value `bench-firmware.json` carries today.
pub const BENCH_FIRMWARE_SCHEMA: u32 = 1;

impl BenchImage {
    /// Whether `identity` is the identity of `segments`.
    pub fn identity_consistent(&self) -> bool {
        self.identity == identity(&self.segments)
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_lower(&Sha256::digest(bytes))
}

/// The canonical JSON of a segment list: an array of
/// `{"address":N,"length":N,"sha256":"…"}`, keys in that order, no whitespace.
pub fn canonical_json(segments: &[Segment]) -> String {
    let mut s = String::from("[");
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"address\":{},\"length\":{},\"sha256\":\"{}\"}}",
            seg.address, seg.length, seg.sha256
        ));
    }
    s.push(']');
    s
}

/// The image's identity: lowercase hex SHA-256 of [`canonical_json`].
pub fn identity(segments: &[Segment]) -> String {
    sha256_hex(canonical_json(segments).as_bytes())
}

/// Reduce an image to its load segments, sorted by address. `base` is the load
/// address of a [`ImageFormat::Bin`] and ignored for the other formats.
pub fn image_segments(
    bytes: &[u8],
    format: ImageFormat,
    base: Option<u64>,
) -> Result<Vec<Segment>, ImageError> {
    let chunks = match format {
        ImageFormat::Hex => hex_chunks(bytes)?,
        ImageFormat::Elf => elf_chunks(bytes)?,
        ImageFormat::Bin => {
            let base = base.ok_or(ImageError::NoBase)?;
            if bytes.is_empty() {
                Vec::new()
            } else {
                alloc::vec![(base, bytes.to_vec())]
            }
        }
    };
    merge(chunks)
}

/// Sort chunks, join those that touch, refuse those that overlap, hash.
fn merge(mut chunks: Vec<(u64, Vec<u8>)>) -> Result<Vec<Segment>, ImageError> {
    chunks.retain(|(_, d)| !d.is_empty());
    chunks.sort_by_key(|(a, _)| *a);
    // A range that runs past the top of the address space has no end address.
    if let Some((a, _)) = chunks
        .iter()
        .find(|(a, d)| a.checked_add(d.len() as u64).is_none())
    {
        return Err(ImageError::Overlap(*a));
    }
    let mut merged: Vec<(u64, Vec<u8>)> = Vec::new();
    for (addr, data) in chunks {
        if let Some((a, d)) = merged.last_mut() {
            let end = *a + d.len() as u64; // checked above: every chunk's end fits
            if addr < end {
                return Err(ImageError::Overlap(addr));
            }
            if addr == end {
                d.extend_from_slice(&data);
                continue;
            }
        }
        merged.push((addr, data));
    }
    Ok(merged
        .into_iter()
        .map(|(address, d)| Segment {
            address,
            length: d.len() as u64,
            sha256: sha256_hex(&d),
        })
        .collect())
}

fn nibble(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

fn hex_chunks(bytes: &[u8]) -> Result<Vec<(u64, Vec<u8>)>, ImageError> {
    let text = core::str::from_utf8(bytes).map_err(|_| ImageError::Hex(1, "not text"))?;
    let mut chunks = Vec::new();
    let mut upper: u64 = 0;
    let mut eof = false;
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if eof {
            return Err(ImageError::Hex(n, "record after end-of-file record"));
        }
        let body = line
            .strip_prefix(':')
            .ok_or(ImageError::Hex(n, "record does not start with ':'"))?
            .as_bytes();
        if body.len() % 2 != 0 || body.len() < 10 {
            return Err(ImageError::Hex(n, "bad record length"));
        }
        let mut rec = Vec::with_capacity(body.len() / 2);
        for pair in body.chunks(2) {
            match (nibble(pair[0]), nibble(pair[1])) {
                (Some(h), Some(l)) => rec.push(h << 4 | l),
                _ => return Err(ImageError::Hex(n, "not hex digits")),
            }
        }
        if rec[0] as usize + 5 != rec.len() {
            return Err(ImageError::Hex(n, "byte count does not match"));
        }
        if rec.iter().fold(0u8, |a, b| a.wrapping_add(*b)) != 0 {
            return Err(ImageError::Hex(n, "checksum mismatch"));
        }
        let offset = u16::from_be_bytes([rec[1], rec[2]]) as u64;
        let data = &rec[4..rec.len() - 1];
        match rec[3] {
            0x00 => chunks.push((upper + offset, data.to_vec())),
            0x01 => eof = true,
            0x02 if data.len() == 2 => {
                upper = (u16::from_be_bytes([data[0], data[1]]) as u64) << 4
            }
            0x04 if data.len() == 2 => {
                upper = (u16::from_be_bytes([data[0], data[1]]) as u64) << 16
            }
            0x03 | 0x05 => {}
            _ => return Err(ImageError::Hex(n, "unsupported record type")),
        }
    }
    Ok(chunks)
}

fn elf_chunks(b: &[u8]) -> Result<Vec<(u64, Vec<u8>)>, ImageError> {
    const PT_LOAD: u32 = 1;
    if b.len() < 16 || &b[..4] != b"\x7fELF" {
        return Err(ImageError::Elf("not an ELF file"));
    }
    let is64 = match b[4] {
        1 => false,
        2 => true,
        _ => return Err(ImageError::Elf("unknown class")),
    };
    let big = match b[5] {
        1 => false,
        2 => true,
        _ => return Err(ImageError::Elf("unknown byte order")),
    };
    let rd = |off: usize, len: usize| -> Result<u64, ImageError> {
        let end = off.checked_add(len).ok_or(ImageError::Elf("truncated"))?;
        let s = b.get(off..end).ok_or(ImageError::Elf("truncated"))?;
        let mut v = 0u64;
        for i in 0..len {
            let byte = if big { s[i] } else { s[len - 1 - i] };
            v = v << 8 | byte as u64;
        }
        Ok(v)
    };
    let rd_at = |p: usize, d: usize, len: usize| -> Result<u64, ImageError> {
        rd(p.checked_add(d).ok_or(ImageError::Elf("truncated"))?, len)
    };
    let (phoff, phentsize, phnum) = if is64 {
        (rd(32, 8)?, rd(54, 2)?, rd(56, 2)?)
    } else {
        (rd(28, 4)?, rd(42, 2)?, rd(44, 2)?)
    };
    let mut chunks = Vec::new();
    for i in 0..phnum {
        let p = i
            .checked_mul(phentsize)
            .and_then(|d| d.checked_add(phoff))
            .and_then(|p| usize::try_from(p).ok())
            .ok_or(ImageError::Elf("truncated"))?;
        let (ty, off, paddr, filesz) = if is64 {
            (rd_at(p, 0, 4)?, rd_at(p, 8, 8)?, rd_at(p, 24, 8)?, rd_at(p, 32, 8)?)
        } else {
            (rd_at(p, 0, 4)?, rd_at(p, 4, 4)?, rd_at(p, 12, 4)?, rd_at(p, 16, 4)?)
        };
        if ty as u32 != PT_LOAD || filesz == 0 {
            continue;
        }
        let range = off.checked_add(filesz).and_then(|end| {
            Some(usize::try_from(off).ok()?..usize::try_from(end).ok()?)
        });
        let data = range
            .and_then(|r| b.get(r))
            .ok_or(ImageError::Elf("segment outside the file"))?;
        chunks.push((paddr, data.to_vec()));
    }
    Ok(chunks)
}

/// The image's segments and identity as the JSON the `image_segments` example
/// prints: `{"identity":"…","segments":[…]}`.
pub fn describe_json(segments: &[Segment]) -> String {
    format!(
        "{{\"identity\":\"{}\",\"segments\":{}}}",
        identity(segments),
        canonical_json(segments)
    )
    .to_string()
}
