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
