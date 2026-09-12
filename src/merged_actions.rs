//! The merged action list a Study Designer UI row picks from — decisions 34/35.
//!
//! `std`-only (uses `std::collections::HashMap`), gated behind the
//! `study-ui` feature. Pure and offline: takes whatever `GattServiceInfo`
//! results a caller already has (from a live `GattDiscover`, from
//! `gatt_extract`, or neither) plus a loaded [`crate::registry::ActionRegistry`],
//! and produces one deduplicated list — no I/O, no BLE, no filesystem
//! access of its own, which is what makes it unit-testable without a UI,
//! hardware, or a real firmware repo.

use std::collections::HashMap;

use serde::Serialize;

use crate::gatt::GattServiceInfo;
use crate::ids::Uuid;
use crate::registry::{ActionRegistry, RegisteredAction};

/// The built-in vocabulary is [`crate::study_builder::BuiltInActionKind`],
/// and it lives there because that is the side the browser submits back —
/// see that type's own doc comment for why this module no longer declares a
/// second one (`suite/017`).
pub use crate::study_builder::BuiltInActionKind;

/// Which discovery source(s) reported a given, not-yet-registered
/// characteristic — shown in the UI so an engineer can tell "found live and
/// in source" from "only ever seen one way" before deciding whether to
/// register an action against it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct DiscoverySources {
    pub live: bool,
    pub static_extraction: bool,
}

/// One entry in the merged list a Study Designer table row picks from.
/// Externally tagged over the wire (serde's default) — `{"BuiltIn":
/// {"which": "ble_connect", "label": "…"}}`, `{"Registered": {...}}`,
/// `{"Unregistered": {...}}`.
///
/// `BuiltIn` carried a bare string until `suite/017`; it carries its label
/// now, because the browser renders what it is served rather than a
/// hardcoded list of its own. That is a wire change with no stale consumer
/// to break: the previous served built-ins were filtered out and discarded
/// by the only client there is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum MergedAction {
    /// `label` is [`BuiltInActionKind::label`] — served rather than
    /// restated in the browser, for the reason `embarch-ui` decision 17
    /// gives about browser-side copies.
    BuiltIn { which: BuiltInActionKind, label: &'static str },
    /// A characteristic with at least one engineer-registered action
    /// against it (`registry::RegisteredAction`) — shown by name, with its
    /// fields'/values' labels ready to click, never raw bytes.
    Registered(RegisteredAction),
    /// A characteristic some discovery source found, but nobody has
    /// registered an action against yet — the UI's own prompt to route the
    /// engineer to the registration form, not
    /// something a Study row can be built from directly. `service_uuid` is
    /// carried alongside `uuid` since registering an action against this
    /// characteristic needs both (`Action::DataExchange` requires a
    /// `service_uuid` and a `characteristic_uuid`, not the characteristic
    /// alone).
    Unregistered { service_uuid: Uuid, uuid: Uuid, properties: u8, sources: DiscoverySources },
    /// A characteristic of a **vendor-defined** service from
    /// [`crate::vendor`] — decision 41.
    ///
    /// Always listed, whether or not any discovery source saw it, because
    /// the table is a compile-time fact rather than an observation;
    /// `sources` says whether this bench actually found it, so the UI can
    /// distinguish "Nordic defines this" from "your DUT has this". Never
    /// routed to the registration form: transcribing a UUID Zephyr itself
    /// publishes into a per-repo registry is exactly the busywork decision
    /// 39 removes.
    Vendor {
        service_id: &'static str,
        service_name: &'static str,
        characteristic_id: &'static str,
        characteristic_name: &'static str,
        service_uuid: Uuid,
        uuid: Uuid,
        /// The vendor's declared properties byte. When a discovery source
        /// also saw this characteristic and reported different properties,
        /// `discovered_properties` carries what the hardware said and this
        /// stays the vendor's claim — the two disagreeing is a real finding
        /// (a modified service build), not something to paper over by
        /// picking one.
        properties: u8,
        discovered_properties: Option<u8>,
        sources: DiscoverySources,
    },
}

/// Merges built-in actions, live discovery, static extraction, and the
/// registry into one list. Order: built-ins first, then every registered
/// action (in registry order), then every detected-but-unregistered
/// characteristic (in first-seen order, live results before static ones) —
/// stable and deterministic so the same inputs always render the same list.
pub fn merge_actions(
    live: Option<&[GattServiceInfo]>,
    static_extraction: Option<&[GattServiceInfo]>,
    registry: &ActionRegistry,
) -> Vec<MergedAction> {
    let mut sources_by_uuid: HashMap<Uuid, (Uuid, u8, DiscoverySources)> = HashMap::new();
    let mut order: Vec<Uuid> = Vec::new();

    let mut record = |services: &[GattServiceInfo], mark: fn(&mut DiscoverySources)| {
        for service in services {
            for chrc in &service.characteristics {
                let entry = sources_by_uuid.entry(chrc.uuid).or_insert_with(|| {
                    order.push(chrc.uuid);
                    (service.uuid, chrc.properties, DiscoverySources::default())
                });
                mark(&mut entry.2);
            }
        }
    };

    if let Some(live) = live {
        record(live, |s| s.live = true);
    }
    if let Some(static_extraction) = static_extraction {
        record(static_extraction, |s| s.static_extraction = true);
    }

    let registered_uuids: std::collections::HashSet<Uuid> =
        registry.actions.iter().map(|a| a.uuid).collect();

    let mut result: Vec<MergedAction> = BuiltInActionKind::ALL
        .iter()
        .copied()
        .map(|which| MergedAction::BuiltIn { which, label: which.label() })
        .collect();

    // Vendor-defined services, before the registry: they're the entries an
    // engineer is least likely to want to author by hand, so they come
    // first among the UUID-bearing choices.
    let mut vendor_uuids: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    for service in crate::vendor::ALL {
        for chrc in service.characteristics {
            vendor_uuids.insert(chrc.uuid);
            let seen = sources_by_uuid.get(&chrc.uuid);
            result.push(MergedAction::Vendor {
                service_id: service.id,
                service_name: service.name,
                characteristic_id: chrc.id,
                characteristic_name: chrc.name,
                service_uuid: service.uuid,
                uuid: chrc.uuid,
                properties: chrc.properties,
                discovered_properties: seen.map(|(_, properties, _)| *properties),
                sources: seen.map(|(_, _, sources)| *sources).unwrap_or_default(),
            });
        }
    }

    result.extend(registry.actions.iter().cloned().map(MergedAction::Registered));

    for uuid in order {
        if registered_uuids.contains(&uuid) {
            // Already surfaced via MergedAction::Registered above — a
            // registered characteristic doesn't also show up as an
            // unregistered prompt, even though it was independently
            // detected too.
            continue;
        }
        if vendor_uuids.contains(&uuid) {
            // Same rule, for the vendor table: a discovered NUS
            // characteristic is already listed as `Vendor` above, and
            // prompting an engineer to *register* a UUID Nordic publishes
            // is the exact busywork decision 41 exists to remove.
            continue;
        }
        let (service_uuid, properties, sources) = sources_by_uuid[&uuid];
        result.push(MergedAction::Unregistered { service_uuid, uuid, properties, sources });
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gatt::GattCharacteristicInfo;
    use crate::registry::{RegisteredOperation};

    fn service(uuid: Uuid, chars: Vec<(Uuid, u8)>) -> GattServiceInfo {
        GattServiceInfo {
            uuid,
            characteristics: chars
                .into_iter()
                .map(|(uuid, properties)| GattCharacteristicInfo { uuid, properties })
                .collect(),
        }
    }

    fn uuid(byte: u8) -> Uuid {
        Uuid([byte; 16])
    }

    #[test]
    fn built_ins_always_present_even_with_nothing_else() {
        let merged = merge_actions(None, None, &ActionRegistry::default());
        // Pinned against `BuiltInActionKind::ALL` rather than a bare
        // literal, so adding a built-in (decision 36 added two) updates this
        // in one place. Vendor entries (decision 41) are also unconditional,
        // and counted the same way for the same reason.
        let vendor_count: usize =
            crate::vendor::ALL.iter().map(|s| s.characteristics.len()).sum();
        assert_eq!(merged.len(), BuiltInActionKind::ALL.len() + vendor_count);
        assert_eq!(
            merged.iter().filter(|a| matches!(a, MergedAction::BuiltIn { .. })).count(),
            BuiltInActionKind::ALL.len()
        );
        assert!(merged
            .iter()
            .all(|a| matches!(a, MergedAction::BuiltIn { .. } | MergedAction::Vendor { .. })));
    }

    /// The defect `suite/017` closed: the served list held seven built-ins
    /// while the submitted enum held nine, and nothing failed because the
    /// browser discarded the served ones. Both sides are one enum now, so
    /// this asserts the thing that was actually wrong — that every built-in
    /// a row can submit is one a row can be offered — rather than a count.
    #[test]
    fn every_submittable_built_in_is_offered_with_a_label() {
        let merged = merge_actions(None, None, &ActionRegistry::default());
        for kind in BuiltInActionKind::ALL {
            let offered = merged.iter().find(|a| matches!(a, MergedAction::BuiltIn { which, .. } if *which == kind));
            let Some(MergedAction::BuiltIn { label, .. }) = offered else {
                panic!("{kind:?} can be submitted but is never offered");
            };
            assert!(!label.is_empty(), "{kind:?} is offered with no label to render");
        }
    }

    #[test]
    fn a_characteristic_seen_live_and_statically_reports_both_sources_once() {
        let live = vec![service(uuid(1), vec![(uuid(2), 0x02)])];
        let static_ext = vec![service(uuid(1), vec![(uuid(2), 0x02)])];
        let merged = merge_actions(Some(&live), Some(&static_ext), &ActionRegistry::default());
        let unregistered: Vec<_> = merged
            .iter()
            .filter_map(|a| match a {
                MergedAction::Unregistered { service_uuid, uuid, properties, sources } => {
                    Some((*service_uuid, *uuid, *properties, *sources))
                }
                _ => None,
            })
            .collect();
        assert_eq!(unregistered.len(), 1);
        let (found_service_uuid, found_uuid, properties, sources) = unregistered[0];
        assert_eq!(found_service_uuid, uuid(1));
        assert_eq!(found_uuid, uuid(2));
        assert_eq!(properties, 0x02);
        assert!(sources.live);
        assert!(sources.static_extraction);
    }

    #[test]
    fn a_registered_characteristic_shows_as_registered_not_unregistered() {
        let live = vec![service(uuid(1), vec![(uuid(2), 0x08)])];
        let registry = ActionRegistry {
            actions: vec![RegisteredAction {
                name: "do_the_thing".to_string(),
                service_uuid: uuid(1),
                uuid: uuid(2),
                operation: RegisteredOperation::Write,
                fields: vec![],
            }],
        };
        let merged = merge_actions(Some(&live), None, &registry);
        let registered: Vec<_> = merged
            .iter()
            .filter_map(|a| match a {
                MergedAction::Registered(r) => Some(r.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(registered, vec!["do_the_thing"]);
        assert!(merged.iter().all(|a| !matches!(a, MergedAction::Unregistered { .. })));
    }

    #[test]
    fn a_registered_action_with_no_matching_discovery_still_appears() {
        // The engineer's registry is the source of truth once an action is
        // named -- it doesn't disappear just because this particular run
        // has no live connection and no static extraction to confirm it.
        let registry = ActionRegistry {
            actions: vec![RegisteredAction {
                name: "known_from_before".to_string(),
                service_uuid: uuid(8),
                uuid: uuid(9),
                operation: RegisteredOperation::Read,
                fields: vec![],
            }],
        };
        let merged = merge_actions(None, None, &registry);
        assert!(merged.iter().any(
            |a| matches!(a, MergedAction::Registered(r) if r.name == "known_from_before")
        ));
    }

    #[test]
    fn order_is_deterministic_across_two_identical_calls() {
        let live = vec![service(uuid(1), vec![(uuid(2), 0x02), (uuid(3), 0x10)])];
        let a = merge_actions(Some(&live), None, &ActionRegistry::default());
        let b = merge_actions(Some(&live), None, &ActionRegistry::default());
        assert_eq!(a, b);
    }
}
