//! BLE identifier newtypes.
//!
//! design.md §4's preamble: "exact byte/UUID representations ... are
//! implementation detail, not a design choice left open here." Raw fixed-size
//! byte arrays, not a `uuid`-crate dependency, so the crate stays dependency-lean
//! and `no_std`-compatible without needing to check that crate's own `no_std`
//! support.

use serde::{Deserialize, Serialize};

/// A 128-bit BLE UUID, raw bytes (big-endian, matching the Bluetooth SIG's
/// on-the-wire base-UUID byte order) — not symbolic, per design.md §4.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Uuid(pub [u8; 16]);

/// A BLE device address: 6 raw address bytes plus its public/random kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BleAddress {
    pub bytes: [u8; 6],
    pub kind: BleAddressKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BleAddressKind {
    Public,
    Random,
}

impl Uuid {
    /// Renders the standard hyphenated 8-4-4-4-12 form (`0000180f-0000-1000-
    /// 8000-00805f9b34fb`) from this type's big-endian bytes — the form a
    /// human reading a `gatt.csv` transcript (design.md §4.3b) or comparing
    /// against a DUT's own headers actually recognizes. `no_std`, allocation
    /// free: a fixed 36-byte `heapless::String`, never a `format!`.
    /// Parses the forms a firmware engineer actually types: the hyphenated
    /// 128-bit form, the same 32 hex digits with no hyphens, or the 16-/32-bit
    /// shorthand (`180f`, `0x180f`, `0000180f`) expanded against the
    /// Bluetooth SIG Base UUID (`0000xxxx-0000-1000-8000-00805f9b34fb`).
    ///
    /// The Base-UUID expansion is a Bluetooth Core Spec fact, not an
    /// inference about any particular DUT — a 16-bit UUID means precisely
    /// that 128-bit value by definition, so expanding it here doesn't guess
    /// at anything this crate isn't in a position to know.
    ///
    /// Returns `None` on anything else; deliberately not a `FromStr` impl
    /// with a named error type, since every caller so far only needs the
    /// yes/no.
    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim();
        let t = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);

        let mut hex: heapless::Vec<u8, 32> = heapless::Vec::new();
        for c in t.chars() {
            if c == '-' {
                continue;
            }
            hex.push(c.to_digit(16)? as u8).ok()?;
        }

        let mut bytes = [0u8; 16];
        match hex.len() {
            // Full 128-bit, with or without hyphens.
            32 => {
                for (i, pair) in hex.chunks(2).enumerate() {
                    bytes[i] = (pair[0] << 4) | pair[1];
                }
            }
            // 16-/32-bit shorthand -> Base UUID. Left-pad to 8 digits, which
            // become the leading four bytes; the trailing twelve are fixed.
            1..=8 => {
                let mut padded = [0u8; 8];
                padded[8 - hex.len()..].copy_from_slice(&hex);
                for (i, pair) in padded.chunks(2).enumerate() {
                    bytes[i] = (pair[0] << 4) | pair[1];
                }
                bytes[4..].copy_from_slice(&[
                    0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb,
                ]);
            }
            _ => return None,
        }
        Some(Uuid(bytes))
    }

    pub fn to_hyphenated(self) -> heapless::String<36> {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        // 8-4-4-4-12: a hyphen follows bytes 3, 5, 7 and 9 (0-indexed).
        let mut out: heapless::String<36> = heapless::String::new();
        for (i, byte) in self.0.iter().enumerate() {
            // Capacity is exactly 36 and this writes exactly 36 bytes, so
            // neither push can fail; `.ok()` keeps the function panic-free
            // rather than relying on that arithmetic staying true.
            out.push(HEX[(byte >> 4) as usize] as char).ok();
            out.push(HEX[(byte & 0x0f) as usize] as char).ok();
            if matches!(i, 3 | 5 | 7 | 9) {
                out.push('-').ok();
            }
        }
        out
    }
}
