//! GATT discovery types — design.md §4.3a (§3 decisions 31/32/33).
//!
//! Shared by both a live `Action::GattDiscover`/`Action::GattMonitorAll`
//! result and a static `GattConfigExtractor` extraction (§3 decision 33,
//! [`crate::gatt_extract`]) — same shape, so the two are directly comparable
//! rather than needing separate diffing logic.

use heapless::Vec;
use serde::{Deserialize, Serialize};

use crate::ids::Uuid;
use crate::limits::MAX_CHARS_PER_SERVICE;

/// One characteristic, as discovered live or extracted from source.
///
/// `properties` is the raw ATT characteristic-properties byte (bit 0 =
/// broadcast, 1 = read, 2 = write-without-response, 3 = write, 4 = notify,
/// 5 = indicate, 6 = authenticated-signed-writes, 7 = extended-properties,
/// per the Bluetooth Core Spec's own characteristic-declaration encoding) —
/// passed through unchanged, not re-encoded into a crate-invented bitflag
/// enum, matching this crate's existing "raw, not symbolic" stance on UUIDs
/// (design.md §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GattCharacteristicInfo {
    pub uuid: Uuid,
    pub properties: u8,
}

/// One primary service and its characteristics, in discovery order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GattServiceInfo {
    pub uuid: Uuid,
    pub characteristics: Vec<GattCharacteristicInfo, MAX_CHARS_PER_SERVICE>,
}

/// One captured notification/indication from `Action::GattMonitorAll`.
///
/// `characteristic_index` indexes into that same step's `gatt_services`,
/// flattened in service-then-characteristic order (service 0's
/// characteristics first, then service 1's, and so on) — not a repeated
/// 16-byte UUID per record. `rx_utc_ms` is dev-bench's own capture-time
/// timestamp, same convention as `Sample.rx_utc_ms` (design.md §4.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GattActivityRecord {
    pub rx_utc_ms: u64,
    pub characteristic_index: u16,
    pub payload: Vec<u8, { crate::limits::MAX_PAYLOAD_LEN }>,
}
