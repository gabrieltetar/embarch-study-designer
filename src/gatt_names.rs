//! Naming a characteristic something a human recognizes — design.md §3
//! decision 56.
//!
//! Every picker in the Study Designer that asks "which characteristic?"
//! labelled its options with the head of a 128-bit UUID (`00000002`,
//! `6e400003`), because a UUID was the only thing anything in this crate
//! could tell a UI about a discovered characteristic. That is the correct
//! *identity* and a poor *label*: on the DUT this suite was built against,
//! eighteen characteristics differ only in the fourth byte.
//!
//! Two name sources exist, and this module is the one place that decides
//! between them:
//!
//! 1. [`crate::vendor`] — a vendor-published characteristic already carries
//!    the vendor's own name for itself. Authoritative for the same reason
//!    the UUID is: it is published, identical on every device implementing
//!    the service, and not this bench's guess.
//! 2. [`crate::gatt_extract`] — the C identifier the firmware repo declared
//!    the characteristic under. Covers everything custom, which on a real
//!    DUT is nearly everything, and costs the engineer nothing: it is
//!    already in the source the extractor reads.
//!
//! Vendor wins where both apply, because source is one repo's spelling of a
//! thing the vendor has already named.
//!
//! # What a name here is not
//!
//! **A label, never semantics.** This module answers "what is this
//! characteristic called", not "what does it do", "what do its bytes mean",
//! or "what happens if you write to it". A name is carried with the source
//! it came from ([`GattName::source`]) and the exact text it was derived
//! from ([`GattName::origin`]) precisely so a UI can show a name without
//! anyone downstream mistaking it for a claim about behavior — the same
//! identity-only line [`crate::vendor`] and [`crate::registry`] both hold.
//!
//! `std`-only (`HashMap`, `String`), gated behind the `std` feature — both
//! `gatt-extract` and `study-ui` enable it, and dev-bench firmware carries
//! none of it.

use std::collections::HashMap;

use serde::Serialize;

use crate::ids::Uuid;

/// Where a [`GattName`] came from, so a UI can render provenance rather than
/// presenting two very different kinds of fact identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GattNameSource {
    /// [`crate::vendor`]'s table — the vendor's published name.
    Vendor,
    /// The C identifier the firmware repo declared the characteristic under,
    /// recovered by [`crate::gatt_extract`].
    FirmwareSymbol,
}

/// One characteristic's display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GattName {
    /// What a picker's option shows.
    pub label: String,
    pub source: GattNameSource,
    /// The unabbreviated text `label` was derived from — the full C
    /// identifier, or the vendor's own full name for the characteristic.
    /// Carried so a UI can put the underived form in a tooltip and nothing
    /// is lost to the shortening.
    pub origin: String,
}

/// Every name known for the characteristics of one open project.
///
/// Built per request from whatever the caller already has, rather than
/// holding a reference to a discovery result: the vendor half is a
/// compile-time table and the source half is a small map, so there is
/// nothing here worth a lifetime.
#[derive(Debug, Clone, Default)]
pub struct GattNameBook {
    symbols: HashMap<Uuid, String>,
    /// Kept apart from `symbols` rather than merged into it: naming a
    /// service resolves against a different half of [`crate::vendor`]'s
    /// table ([`crate::vendor::find_service_by_uuid`], not
    /// [`crate::vendor::find_by_uuid`]), so one map would have to guess
    /// which lookup a UUID wanted (§3 decision 57).
    service_symbols: HashMap<Uuid, String>,
}

impl GattNameBook {
    /// A book with no source symbols — vendor names still resolve. What a
    /// project with no static extractor configured gets.
    pub fn new() -> GattNameBook {
        GattNameBook::default()
    }

    /// Adds the identifiers an extraction recovered
    /// ([`crate::gatt_extract::ExtractedGatt::symbols`]).
    ///
    /// First writer wins on a repeated UUID: a characteristic declared twice
    /// under two identifiers is a real oddity in the source, and picking the
    /// first keeps the answer stable across requests instead of depending on
    /// iteration order.
    pub fn with_symbols(
        mut self,
        symbols: impl IntoIterator<Item = (Uuid, String)>,
    ) -> GattNameBook {
        for (uuid, identifier) in symbols {
            self.symbols.entry(uuid).or_insert(identifier);
        }
        self
    }

    /// Adds the service identifiers an extraction recovered
    /// ([`crate::gatt_extract::ExtractedGatt::service_symbols`]).
    ///
    /// Same first-writer-wins rule as [`Self::with_symbols`].
    pub fn with_service_symbols(
        mut self,
        symbols: impl IntoIterator<Item = (Uuid, String)>,
    ) -> GattNameBook {
        for (uuid, identifier) in symbols {
            self.service_symbols.entry(uuid).or_insert(identifier);
        }
        self
    }

    /// This *service*'s name, or `None` — the group-header counterpart to
    /// [`Self::get`] (§3 decision 57). Same two sources, same precedence:
    /// a vendor-published service name beats one repo's spelling of it.
    pub fn service(&self, service_uuid: Uuid) -> Option<GattName> {
        if let Some(service) = crate::vendor::find_service_by_uuid(service_uuid) {
            return Some(GattName {
                label: service.name.to_string(),
                source: GattNameSource::Vendor,
                origin: service.name.to_string(),
            });
        }
        self.service_symbols.get(&service_uuid).map(|identifier| GattName {
            label: label_from_service_symbol(identifier),
            source: GattNameSource::FirmwareSymbol,
            origin: identifier.clone(),
        })
    }

    /// This characteristic's name, or `None` when neither source knows one —
    /// which is an ordinary state (a live-only characteristic on a repo with
    /// no extractor), not an error. A caller renders the UUID in that case,
    /// exactly as everything did before decision 56.
    pub fn get(&self, characteristic_uuid: Uuid) -> Option<GattName> {
        if let Some((_service, chrc)) = crate::vendor::find_by_uuid(characteristic_uuid) {
            return Some(GattName {
                label: chrc.short_name.to_string(),
                source: GattNameSource::Vendor,
                origin: chrc.name.to_string(),
            });
        }
        self.symbols.get(&characteristic_uuid).map(|identifier| GattName {
            label: label_from_symbol(identifier),
            source: GattNameSource::FirmwareSymbol,
            origin: identifier.clone(),
        })
    }
}

/// Trims the suffix a C identifier carries because it names a *variable*, not
/// a characteristic: `sds_hrm_rrm_char_uuid` -> `sds_hrm_rrm`,
/// `dms_batt_status_uuid` -> `dms_batt_status`.
///
/// Deliberately nothing else. No title-casing, no `_` -> ` `, no expanding
/// `hrm`/`sds`/`rrm` into words — every one of those would be this crate
/// deciding what a firmware team's abbreviation stands for, and being wrong
/// about it in a label an engineer then trusts. Trimming a suffix is
/// mechanical and reversible, and [`GattName::origin`] keeps the untrimmed
/// identifier anyway.
///
/// An identifier that is *only* a suffix is returned verbatim: a label of ""
/// is worse than a redundant one.
pub fn label_from_symbol(identifier: &str) -> String {
    // Longest first, so `_char_uuid` isn't left as a trailing `_char` by a
    // shorter match winning.
    const SUFFIXES: [&str; 5] = ["_char_uuid", "_chrc_uuid", "_uuid", "_char", "_chrc"];
    let trimmed = identifier.trim();
    for suffix in SUFFIXES {
        if let Some(stem) = trimmed.strip_suffix(suffix) {
            if !stem.is_empty() {
                return stem.to_string();
            }
        }
    }
    trimmed.to_string()
}

/// Trims the suffix a service's C identifier carries because it names a
/// *variable*: `sds_service_uuid` -> `sds_service`, `bds_service_uuid` ->
/// `bds_service`.
///
/// Same deliberately-minimal rule as [`label_from_symbol`], with `_service`
/// deliberately **kept**: a group header reading `sds_service` says what it
/// is, and stripping it to `sds` would be this crate deciding that `sds`
/// means something on its own.
pub fn label_from_service_symbol(identifier: &str) -> String {
    let trimmed = identifier.trim();
    match trimmed.strip_suffix("_uuid") {
        Some(stem) if !stem.is_empty() => stem.to_string(),
        _ => trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(text: &str) -> Uuid {
        Uuid::parse(text).unwrap()
    }

    #[test]
    fn a_variable_suffix_is_trimmed_and_nothing_else_is() {
        assert_eq!(label_from_symbol("sds_hrm_rrm_char_uuid"), "sds_hrm_rrm");
        assert_eq!(label_from_symbol("dms_batt_status_uuid"), "dms_batt_status");
        assert_eq!(label_from_symbol("bds_ctrl_char"), "bds_ctrl");
        // Not this crate's business what `hrm` stands for, or whether an
        // underscore should have been a space.
        assert_eq!(label_from_symbol("sds_imu_char_uuid"), "sds_imu");
        // No suffix to trim, and an identifier that is only a suffix.
        assert_eq!(label_from_symbol("heart_rate"), "heart_rate");
        assert_eq!(label_from_symbol("_uuid"), "_uuid");
    }

    #[test]
    fn a_firmware_symbol_names_a_custom_characteristic() {
        let book = GattNameBook::new().with_symbols([(
            uuid("00000002-853f-4a00-8000-e58100000000"),
            "sds_hrm_rrm_char_uuid".to_string(),
        )]);
        let name = book.get(uuid("00000002-853f-4a00-8000-e58100000000")).unwrap();
        assert_eq!(name.label, "sds_hrm_rrm");
        assert_eq!(name.source, GattNameSource::FirmwareSymbol);
        // The untrimmed identifier survives, so a tooltip can show exactly
        // what the source says.
        assert_eq!(name.origin, "sds_hrm_rrm_char_uuid");
    }

    /// A DUT that exposes NUS *and* declares it in its own source gets the
    /// vendor's name, not the repo's spelling of it.
    #[test]
    fn vendor_wins_over_a_firmware_symbol() {
        let nus_tx = uuid("6e400003-b5a3-f393-e0a9-e50e24dcca9e");
        let book = GattNameBook::new()
            .with_symbols([(nus_tx, "some_local_nus_tx_alias_uuid".to_string())]);
        let name = book.get(nus_tx).unwrap();
        assert_eq!(name.label, "NUS TX");
        assert_eq!(name.source, GattNameSource::Vendor);
    }

    /// Vendor names resolve with no extraction at all — a project with no
    /// `static_extractor` configured still gets them.
    #[test]
    fn vendor_names_need_no_symbols() {
        let name = GattNameBook::new()
            .get(uuid("6e400002-b5a3-f393-e0a9-e50e24dcca9e"))
            .unwrap();
        assert_eq!(name.label, "NUS RX");
    }

    #[test]
    fn an_unknown_characteristic_has_no_name_rather_than_a_made_up_one() {
        assert!(GattNameBook::new().get(uuid("0000dead-0000-1000-8000-00805f9b34fb")).is_none());
    }

    /// §3 decision 57: a service gets a name the same way a characteristic
    /// does, from the identifier `parse_gatt_services` already had in hand.
    #[test]
    fn a_service_is_named_from_its_declaring_identifier() {
        let svc = uuid("00000020-853f-4a00-8000-e58100000000");
        let book = GattNameBook::new()
            .with_service_symbols([(svc, "bds_service_uuid".to_string())]);
        let name = book.service(svc).unwrap();
        // `_service` survives the trim — `bds` alone would be this crate
        // deciding what the abbreviation stands for.
        assert_eq!(name.label, "bds_service");
        assert_eq!(name.source, GattNameSource::FirmwareSymbol);
        assert_eq!(name.origin, "bds_service_uuid");
    }

    /// A vendor-published service beats one repo's spelling of it, exactly
    /// as it does one level down.
    #[test]
    fn vendor_wins_for_a_service_too() {
        let nus = uuid("6e400001-b5a3-f393-e0a9-e50e24dcca9e");
        let book = GattNameBook::new()
            .with_service_symbols([(nus, "local_nus_alias_uuid".to_string())]);
        let name = book.service(nus).unwrap();
        assert_eq!(name.source, GattNameSource::Vendor);
        assert_eq!(name.label, crate::vendor::find_service_by_uuid(nus).unwrap().name);
    }

    /// Service and characteristic names live in separate maps: a
    /// characteristic symbol must not turn up as a service name, or a
    /// grouped picker would invent a group.
    #[test]
    fn a_characteristic_symbol_does_not_name_a_service() {
        let u = uuid("00000021-853f-4a00-8000-e58100000000");
        let book = GattNameBook::new().with_symbols([(u, "bds_data_char_uuid".to_string())]);
        assert!(book.get(u).is_some());
        assert!(book.service(u).is_none());
    }

    #[test]
    fn the_first_identifier_wins_on_a_repeated_uuid() {
        let u = uuid("00000002-853f-4a00-8000-e58100000000");
        let book = GattNameBook::new()
            .with_symbols([(u, "first_uuid".to_string()), (u, "second_uuid".to_string())]);
        assert_eq!(book.get(u).unwrap().label, "first");
    }
}
