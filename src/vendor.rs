//! Vendor-defined GATT service *identities* — design.md §3 decision 41.
//!
//! `no_std`, allocation-free, always compiled (no feature gate): dev-bench
//! firmware, `embarch-core`, `embarch-api` and a Study Designer UI all need
//! the same answer to "what are the UUIDs for the Nordic UART Service?",
//! and that answer is the same on every bench in the world.
//!
//! # Why this is not [`crate::registry`]
//!
//! [`registry`](crate::registry) holds *engineer-authored* actions, kept per
//! firmware repo in `embarch/study-actions.toml`, because what a custom
//! characteristic's bytes mean is knowledge only that repo's engineers have.
//! A vendor-defined service is the opposite kind of fact: its UUIDs are
//! published by the silicon/stack vendor (here Nordic Semiconductor, as
//! carried in Zephyr's own `include/zephyr/bluetooth/services/nus.h`), they
//! are identical for every device that implements the service, and nobody
//! should have to transcribe `6e400002-b5a3-f393-e0a9-e50e24dcca9e` by hand
//! into their own repo's registry to use one. So they live here, in the
//! crate, as constants.
//!
//! # What this module deliberately does NOT know
//!
//! **Identity only. No semantics, ever.** This is the same rule
//! `registry.rs` states, applied to a table that would be much more
//! tempting to over-fill:
//!
//! - It records that NUS RX is the characteristic you write to, and that
//!   the vendor declares it Write / Write-Without-Response. It records
//!   nothing about *what bytes to write* — no command vocabulary, no line
//!   terminator, no "send `help` to list commands". Whatever is behind a
//!   given DUT's NUS endpoint (a Zephyr shell, an application protocol, a
//!   bootloader, nothing at all) is that DUT's business and its engineers'
//!   knowledge, supplied per study as literal bytes.
//! - It records nothing about *whether a given DUT has* this service. An
//!   entry here is a conditional: *if* a device exposes this vendor service,
//!   these are its UUIDs. `Action::GattDiscover` against real hardware is
//!   still the only thing that answers "does this DUT actually have it?".
//! - It records no security or connection preconditions (pairing, bonding,
//!   MTU) — those are per-implementation and not derivable from the UUIDs.
//!
//! [`properties`](VendorCharacteristic::properties) is included because it
//! is part of the vendor's own declaration and it is what decides whether a
//! chosen operation is even legal against a characteristic — the same ATT
//! properties byte `GattDiscover` reports back from live hardware, so a UI
//! can render and check both the same way. It is still the vendor's claim,
//! not a measurement: a build that modifies the service will disagree with
//! it, and live discovery wins whenever the two differ.

use crate::ids::Uuid;

/// One characteristic of a [`VendorService`], as the vendor declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VendorCharacteristic {
    /// Stable machine key, unique within its service (`"rx"`, `"tx"`).
    pub id: &'static str,
    /// The vendor's own name for it, for display.
    pub name: &'static str,
    /// The same identity in the few characters a picker's label has room for
    /// (design.md §3 decision 56). A separate field rather than a truncation
    /// of [`name`](Self::name): `name` is the vendor's full sentence, and the
    /// place to decide what its short form is, is the table that knows both.
    pub short_name: &'static str,
    pub uuid: Uuid,
    /// The ATT characteristic-properties byte the vendor declares. Same bit
    /// layout as `GattCharacteristicInfo::properties`, so a UI can render
    /// this and a live-discovered value with one function. The vendor's
    /// claim, not a measurement — see this module's own doc comment.
    pub properties: u8,
}

/// A GATT service whose UUIDs are defined by a silicon/stack vendor rather
/// than by the engineer using it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VendorService {
    /// Stable machine key, used by a UI/wire selection rather than the
    /// display name (`"nordic-uart"`).
    pub id: &'static str,
    pub name: &'static str,
    /// Who defines it, and where this entry's numbers were read from —
    /// recorded so the provenance of a UUID in this table is auditable
    /// rather than folklore.
    pub vendor: &'static str,
    pub source: &'static str,
    pub uuid: Uuid,
    pub characteristics: &'static [VendorCharacteristic],
}

impl VendorService {
    /// The characteristic with this `id`, or `None`.
    pub fn characteristic(&self, id: &str) -> Option<&'static VendorCharacteristic> {
        self.characteristics.iter().find(|c| c.id == id)
    }
}

/// ATT characteristic-properties bits used by this table. The Bluetooth Core
/// Spec's own values (Vol 3, Part G) — the same bits
/// `merged_actions`/`gatt` already treat as spec facts.
const PROP_WRITE_WITHOUT_RESP: u8 = 0x04;
const PROP_WRITE: u8 = 0x08;
const PROP_NOTIFY: u8 = 0x10;

/// Nordic UART Service (NUS) — Nordic Semiconductor's vendor-defined
/// serial-over-GATT service, implemented upstream in Zephyr as
/// `CONFIG_BT_ZEPHYR_NUS`.
///
/// UUIDs and properties transcribed from Zephyr's own service definition
/// (`include/zephyr/bluetooth/services/nus.h` for the UUIDs,
/// `include/zephyr/bluetooth/services/nus/inst.h`'s
/// `Z_INTERNAL_BT_NUS_INST_DEFINE` for the properties), which is the
/// definition that compiles into any Zephyr device exposing the service.
///
/// Direction names are the vendor's and are stated from the *peripheral's*
/// point of view, which is worth reading twice: **RX is what a central
/// writes to** (the peripheral receives), and **TX is what the peripheral
/// notifies on** (the central receives).
pub const NORDIC_UART_SERVICE: VendorService = VendorService {
    id: "nordic-uart",
    name: "Nordic UART Service (NUS)",
    vendor: "Nordic Semiconductor",
    source: "Zephyr include/zephyr/bluetooth/services/nus.h + nus/inst.h",
    uuid: Uuid([
        0x6e, 0x40, 0x00, 0x01, 0xb5, 0xa3, 0xf3, 0x93, 0xe0, 0xa9, 0xe5, 0x0e, 0x24, 0xdc, 0xca,
        0x9e,
    ]),
    characteristics: &[
        VendorCharacteristic {
            id: "rx",
            name: "RX — written by the central, received by the peripheral",
            short_name: "NUS RX",
            uuid: Uuid([
                0x6e, 0x40, 0x00, 0x02, 0xb5, 0xa3, 0xf3, 0x93, 0xe0, 0xa9, 0xe5, 0x0e, 0x24, 0xdc,
                0xca, 0x9e,
            ]),
            properties: PROP_WRITE | PROP_WRITE_WITHOUT_RESP,
        },
        VendorCharacteristic {
            id: "tx",
            name: "TX — notified by the peripheral, received by the central",
            short_name: "NUS TX",
            uuid: Uuid([
                0x6e, 0x40, 0x00, 0x03, 0xb5, 0xa3, 0xf3, 0x93, 0xe0, 0xa9, 0xe5, 0x0e, 0x24, 0xdc,
                0xca, 0x9e,
            ]),
            properties: PROP_NOTIFY,
        },
    ],
};

/// Every vendor-defined service this crate ships. Order is stable — a UI
/// renders it as given.
pub const ALL: &[VendorService] = &[NORDIC_UART_SERVICE];

/// The service with this [`VendorService::id`], or `None`.
pub fn find(id: &str) -> Option<&'static VendorService> {
    ALL.iter().find(|s| s.id == id)
}

/// The `(service, characteristic)` pair named by a service id and a
/// characteristic id, or `None` if either is unknown. The one lookup a
/// caller resolving a UI selection into an `Action::DataExchange` needs.
pub fn find_characteristic(
    service_id: &str,
    characteristic_id: &str,
) -> Option<(&'static VendorService, &'static VendorCharacteristic)> {
    let service = find(service_id)?;
    let characteristic = service.characteristic(characteristic_id)?;
    Some((service, characteristic))
}

/// The `(service, characteristic)` pair a **UUID** belongs to, or `None`.
///
/// The lookup direction [`find_characteristic`] doesn't cover: that one
/// resolves a UI selection an engineer made *by name*, this one names a
/// characteristic something else already found *by UUID* — a live
/// `GattDiscover` result, or a static extraction. Matching on the
/// characteristic UUID alone rather than on the pair: a UUID this table
/// defines is globally unique by construction, so a device exposing NUS TX
/// under some other service is still exposing NUS TX, and saying so is more
/// useful than declining to name it.
pub fn find_by_uuid(
    characteristic_uuid: Uuid,
) -> Option<(&'static VendorService, &'static VendorCharacteristic)> {
    ALL.iter().find_map(|service| {
        service
            .characteristics
            .iter()
            .find(|c| c.uuid == characteristic_uuid)
            .map(|c| (service, c))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The UUIDs, spelled out the way an engineer reads them in Zephyr's
    /// header — so a typo in the byte arrays above fails here rather than
    /// silently writing to the wrong characteristic on real hardware.
    #[test]
    fn nus_uuids_match_nordics_published_values() {
        assert_eq!(
            NORDIC_UART_SERVICE.uuid.to_hyphenated().as_str(),
            "6e400001-b5a3-f393-e0a9-e50e24dcca9e"
        );
        assert_eq!(
            NORDIC_UART_SERVICE.characteristic("rx").unwrap().uuid.to_hyphenated().as_str(),
            "6e400002-b5a3-f393-e0a9-e50e24dcca9e"
        );
        assert_eq!(
            NORDIC_UART_SERVICE.characteristic("tx").unwrap().uuid.to_hyphenated().as_str(),
            "6e400003-b5a3-f393-e0a9-e50e24dcca9e"
        );
    }

    /// Cross-checks the byte arrays against `Uuid::parse` of the hyphenated
    /// text — a second, independent path to the same 16 bytes.
    #[test]
    fn byte_arrays_agree_with_parsed_text() {
        assert_eq!(
            NORDIC_UART_SERVICE.uuid,
            Uuid::parse("6e400001-b5a3-f393-e0a9-e50e24dcca9e").unwrap()
        );
    }

    #[test]
    fn a_uuid_lookup_names_the_characteristic_it_belongs_to() {
        let uuid = Uuid::parse("6e400003-b5a3-f393-e0a9-e50e24dcca9e").unwrap();
        let (service, chrc) = find_by_uuid(uuid).expect("NUS TX is in the table");
        assert_eq!(service.id, "nordic-uart");
        assert_eq!(chrc.id, "tx");
        // A custom UUID belongs to no vendor here, and this table says so
        // rather than guessing — the whole point of it being identity-only.
        assert!(find_by_uuid(Uuid::parse("00000002-853f-4a00-8000-e58100000000").unwrap()).is_none());
    }

    #[test]
    fn nus_properties_match_zephyrs_gatt_declaration() {
        // BT_GATT_CHRC_WRITE | BT_GATT_CHRC_WRITE_WITHOUT_RESP
        assert_eq!(NORDIC_UART_SERVICE.characteristic("rx").unwrap().properties, 0x0c);
        // BT_GATT_CHRC_NOTIFY
        assert_eq!(NORDIC_UART_SERVICE.characteristic("tx").unwrap().properties, 0x10);
    }

    #[test]
    fn find_resolves_by_id_and_rejects_unknown() {
        assert_eq!(find("nordic-uart").unwrap().id, "nordic-uart");
        assert!(find("nordic_uart").is_none());
        assert!(find("").is_none());
    }

    #[test]
    fn find_characteristic_resolves_both_ids() {
        let (service, chrc) = find_characteristic("nordic-uart", "rx").unwrap();
        assert_eq!(service.uuid, NORDIC_UART_SERVICE.uuid);
        assert_eq!(chrc.id, "rx");
        assert!(find_characteristic("nordic-uart", "nope").is_none());
        assert!(find_characteristic("nope", "rx").is_none());
    }

    /// Ids are what a saved study persists, so a duplicate would make a
    /// saved selection ambiguous on reload.
    #[test]
    fn ids_are_unique() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a.id, b.id, "duplicate service id");
            }
            for (j, c) in a.characteristics.iter().enumerate() {
                for d in &a.characteristics[j + 1..] {
                    assert_ne!(c.id, d.id, "duplicate characteristic id in {}", a.id);
                }
            }
        }
    }

    /// This table is identity-only by design (see the module doc comment).
    /// A characteristic with no declared operation would mean somebody added
    /// an entry for its *meaning* rather than its identity.
    #[test]
    fn every_characteristic_declares_at_least_one_operation() {
        for service in ALL {
            for chrc in service.characteristics {
                assert_ne!(chrc.properties, 0, "{}/{} declares no operations", service.id, chrc.id);
            }
        }
    }
}
