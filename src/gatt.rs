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

/// One characteristic a study names explicitly — design.md §3 decision 53.
///
/// Both UUIDs, not the characteristic's alone: subscribing needs the service
/// to discover within, exactly as `Action::DataExchange` has always needed
/// both. It is also the pair a `StreamSource::GattNotify` tap already
/// carries, so a characteristic named as a monitor target and a
/// characteristic given its own decoded file are addressed identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GattTarget {
    pub service_uuid: Uuid,
    pub characteristic_uuid: Uuid,
}

// `GattActivityRecord` was here and is **retired** by design.md §3 decision
// 54, along with `StepResult.gatt_activity`. It capped a step's captured
// notifications at 32 — by a `MAX_GATT_ACTIVITY_RECORDS` that went with it,
// and is now only a tombstone in `interfaces/limits.md` — inline in
// `events.json`, which is the wrong shape for what it recorded: a capture is
// unbounded and streamed, and the tap pipeline (§4.8) already writes exactly
// that, incrementally, to a file. Keeping a second, capped, in-memory copy
// meant a study could look like it had captured everything while holding 32
// of several thousand records — the "nothing captured, no error" family of
// failure this suite has now arrived at from four directions.

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
/// **The only record of GATT activity this crate carries.** It was once
/// contrasted here with `GattActivityRecord`, a per-step summary capped at
/// 32 inbound notifications inline in `events.json`; that type and its cap
/// are **retired** by §3 decision 54, so the contrast is history and this
/// type is not one of two. It is streamed one entry at a time over its own
/// [`crate::protocol::DevBenchMessage`] variant, so it is bounded by nothing
/// but the study's own duration, and it records what dev-bench *sent* as
/// well as what it received — which is what made the bounded in-memory copy
/// removable rather than merely redundant.
///
/// UUIDs are carried in full rather than as a `characteristic_index` into a
/// step's `gatt_services` (the compression the retired summary used),
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
