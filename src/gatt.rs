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

/// Which way a transcript entry travelled — design.md §3 decision 36, §4.3b.
///
/// `Local` covers dev-bench's own internal milestones (discovery starting,
/// a subscription being armed) that aren't an ATT PDU in either direction
/// but are exactly what makes a transcript readable after the fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GattDirection {
    /// dev-bench -> DUT.
    Out,
    /// DUT -> dev-bench.
    In,
    /// Neither — a dev-bench-side event.
    Local,
}

impl GattDirection {
    /// Lowercase column text for `gatt.csv` (design.md §4.3b).
    pub fn as_str(self) -> &'static str {
        match self {
            GattDirection::Out => "out",
            GattDirection::In => "in",
            GattDirection::Local => "local",
        }
    }
}

/// What a transcript entry records — design.md §3 decision 36, §4.3b.
///
/// Append-only, the same discipline [`crate::protocol::DevBenchMessage`]
/// follows (design.md §3 decision 10): postcard encodes this as a varint
/// discriminant, so a new kind goes on the end and an existing one is never
/// reordered or removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GattEventKind {
    Connected,
    Disconnected,
    DiscoveryStarted,
    ServiceDiscovered,
    CharacteristicDiscovered,
    Subscribed,
    Unsubscribed,
    WriteRequest,
    WriteResponse,
    ReadRequest,
    ReadResponse,
    Notification,
    Indication,
    /// A protocol-level failure: `att_status` carries the ATT error code
    /// where there was one, `payload` whatever context dev-bench had.
    Error,
}

impl GattEventKind {
    /// Lowercase column text for `gatt.csv` (design.md §4.3b).
    pub fn as_str(self) -> &'static str {
        match self {
            GattEventKind::Connected => "connected",
            GattEventKind::Disconnected => "disconnected",
            GattEventKind::DiscoveryStarted => "discovery_started",
            GattEventKind::ServiceDiscovered => "service_discovered",
            GattEventKind::CharacteristicDiscovered => "characteristic_discovered",
            GattEventKind::Subscribed => "subscribed",
            GattEventKind::Unsubscribed => "unsubscribed",
            GattEventKind::WriteRequest => "write_request",
            GattEventKind::WriteResponse => "write_response",
            GattEventKind::ReadRequest => "read_request",
            GattEventKind::ReadResponse => "read_response",
            GattEventKind::Notification => "notification",
            GattEventKind::Indication => "indication",
            GattEventKind::Error => "error",
        }
    }
}

/// One line of the exhaustive GATT transcript — design.md §3 decision 36,
/// §4.3b.
///
/// Deliberately *not* a second [`GattActivityRecord`]: that type exists to
/// summarize a single `GattMonitorAll` step inline in `events.json`, is
/// capped at [`crate::limits::MAX_GATT_ACTIVITY_RECORDS`] per step, and
/// records inbound notifications only. This type is streamed one entry at a
/// time over its own [`crate::protocol::DevBenchMessage`] variant, so it is
/// bounded by nothing but the study's own duration, and it records what
/// dev-bench *sent* as well as what it received. Both survive: the inline
/// summary stays useful for a quick pass/fail read without opening a large
/// capture file, exactly the reason `data.csv` is kept out of `events.json`
/// (design.md §5.2).
///
/// UUIDs are carried in full rather than as a `characteristic_index` into a
/// step's `gatt_services` (the compression `GattActivityRecord` uses),
/// because a transcript spans steps — including steps that ran no discovery
/// at all — so there is no single flattened table for an index to mean
/// anything against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GattTranscriptEntry {
    /// dev-bench's own capture-time timestamp, same convention as
    /// `Sample.rx_utc_ms` (design.md §4.7).
    pub rx_utc_ms: u64,
    pub direction: GattDirection,
    pub kind: GattEventKind,
    /// `None` for entries with no characteristic context (a connect, a
    /// disconnect, discovery starting).
    pub service_uuid: Option<crate::ids::Uuid>,
    pub characteristic_uuid: Option<crate::ids::Uuid>,
    /// Raw ATT error code, `0` when there was none — passed through
    /// unchanged, not re-encoded, matching this module's stance on
    /// `GattCharacteristicInfo.properties`.
    pub att_status: u8,
    pub payload: Vec<u8, { crate::limits::MAX_PAYLOAD_LEN }>,
}

#[cfg(feature = "std")]
impl GattTranscriptEntry {
    /// The `gatt.csv` header (design.md §4.3b). `core_rx_utc_ms` is appended
    /// by Core itself, not by this crate — its own receipt time, not part of
    /// the wire type, the same split `Sample::csv_header` already uses
    /// (design.md §3 decision 30).
    pub fn csv_header() -> &'static str {
        "rx_utc_ms,step_index,step_name,direction,kind,service_uuid,characteristic_uuid,att_status,payload_len,payload_hex,payload_ascii"
    }

    /// Renders one row. Returns `None` if `step_name` plus this entry's own
    /// payload can't fit within [`crate::limits::MAX_GATT_CSV_ROW_LEN`] —
    /// Core logs and skips that row rather than truncating it, the same
    /// failure posture `Sample::to_csv_row` already takes (a truncated CSV
    /// row is a worse failure mode than a dropped one).
    ///
    /// The payload appears twice by design: `payload_hex` is the exact
    /// bytes, and `payload_ascii` renders printable bytes as themselves and
    /// everything else as `.` — so a shell/NUS transcript is directly
    /// readable without decoding hex by hand, while nothing is lost for a
    /// binary protocol.
    pub fn to_csv_row(
        &self,
        step_index: u32,
        step_name: &str,
    ) -> Option<heapless::String<{ crate::limits::MAX_GATT_CSV_ROW_LEN }>> {
        use core::fmt::Write as _;

        let mut row: heapless::String<{ crate::limits::MAX_GATT_CSV_ROW_LEN }> =
            heapless::String::new();
        write!(
            row,
            "{},{},{},{},{},{},{},{},{},",
            self.rx_utc_ms,
            step_index,
            csv_escape_ok(step_name)?,
            self.direction.as_str(),
            self.kind.as_str(),
            self.service_uuid.map(|u| u.to_hyphenated()).unwrap_or_default(),
            self.characteristic_uuid.map(|u| u.to_hyphenated()).unwrap_or_default(),
            self.att_status,
            self.payload.len(),
        )
        .ok()?;

        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in &self.payload {
            row.push(HEX[(byte >> 4) as usize] as char).ok()?;
            row.push(HEX[(byte & 0x0f) as usize] as char).ok()?;
        }
        row.push(',').ok()?;
        for byte in &self.payload {
            // Printable ASCII only; `,`/`"` would break the column and are
            // rendered as `.` alongside every other non-printable rather
            // than quoted, keeping this column a lossy-but-readable
            // companion to `payload_hex`, which is the lossless one.
            let c = match byte {
                0x20..=0x7e if *byte != b',' && *byte != b'"' => *byte as char,
                _ => '.',
            };
            row.push(c).ok()?;
        }
        Some(row)
    }
}

/// Rejects a step name that would break the CSV shape rather than quoting
/// it — step names come from `Step.name`, authored in the Study Designer UI,
/// where a comma or quote has no reason to appear.
#[cfg(feature = "std")]
fn csv_escape_ok(name: &str) -> Option<&str> {
    if name.contains(',') || name.contains('"') || name.contains('\n') {
        None
    } else {
        Some(name)
    }
}
