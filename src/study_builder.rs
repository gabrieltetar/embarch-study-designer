//! Table-row -> `Study` conversion for the Study Designer UI — design.md §3
//! decision 34, `embarch-study-designer/milestone-11.md` §3.3/§3.7.
//!
//! `std`-only, `study-ui`-feature-gated: takes a client-submitted "table"
//! (one [`TableRow`] per Study Designer UI row) and produces a real
//! [`Study`], resolving each row's action against a loaded
//! [`ActionRegistry`]. Pure and offline — no I/O, no BLE — which is what
//! makes this the actual place `Study` schema-validity gets tested, not the
//! UI binary itself (milestone-11.md §3.7).
//!
//! A registered action's payload is assembled purely mechanically: each
//! chosen field value's literal bytes (`registry::ActionFieldValue::bytes`)
//! are spliced into a zero-initialized buffer at that field's own
//! `byte_offset`. Nothing here interprets what those bytes mean — the same
//! rule `registry.rs` states plainly applies here too.

use std::collections::HashMap;

use heapless::Vec as HVec;
use serde::{Deserialize, Serialize};

use crate::ids::Uuid;
use crate::limits::{MAX_NAME_LEN, MAX_PAYLOAD_LEN, MAX_STEPS_PER_STUDY, MAX_STUDY_NAME_LEN};
use crate::registry::{ActionRegistry, RegisteredAction, RegisteredOperation};
use crate::study::{Action, BleRole, GattOperation, Step, Study};

/// Which built-in `Action` a `RowAction::BuiltIn` row picks — a UI-facing
/// enumeration distinct from `merged_actions::BuiltInAction` only in that
/// this one is (de)serializable (it crosses the wire from the browser);
/// the two are kept in sync by hand, not shared, since one lives in this
/// crate's UI-input layer and the other in its UI-output layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltInActionKind {
    BleConnect,
    GattDiscover,
    GattMonitorAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleChoice {
    Central,
    Peripheral,
}

fn default_role() -> RoleChoice {
    RoleChoice::Central
}

/// One row's chosen action, as submitted by the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RowAction {
    BuiltIn {
        which: BuiltInActionKind,
        /// Only meaningful for `BleConnect` — defaults to `Central`, the
        /// role every real study run through this suite has ever used.
        #[serde(default = "default_role")]
        role: RoleChoice,
    },
    /// References a `RegisteredAction` by name, plus — for a `Write` — which
    /// value the engineer picked per field: field name -> that field's
    /// chosen `ActionFieldValue.label`. Still never raw bytes at this
    /// layer; resolving a label back to its bytes happens in
    /// [`build_study`], against the registry, not something the client
    /// does itself.
    Registered {
        name: String,
        #[serde(default)]
        field_choices: HashMap<String, String>,
    },
}

/// One Study Designer UI row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableRow {
    pub name: String,
    pub action: RowAction,
    pub timeout_ms: u32,
    #[serde(default)]
    pub continue_on_fail: bool,
}

/// Names the specific failure building a `Study` from a table, matching
/// this crate's existing discipline (`gatt_extract::ExtractError`,
/// `registry::RegistryError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildStudyError {
    TooManySteps { max: usize, actual: usize },
    NameTooLong { which: &'static str, value: String, max: usize },
    UnknownRegisteredAction(String),
    RegisteredActionHasNoFields { action: String },
    MissingFieldChoice { action: String, field: String },
    UnknownFieldChoice { action: String, field: String, label: String },
    NotWritable { action: String },
}

impl std::fmt::Display for BuildStudyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildStudyError::TooManySteps { max, actual } => {
                write!(f, "study has {actual} steps, but the limit is {max}")
            }
            BuildStudyError::NameTooLong { which, value, max } => {
                write!(f, "{which} '{value}' is longer than the {max}-byte limit")
            }
            BuildStudyError::UnknownRegisteredAction(name) => {
                write!(f, "no registered action named '{name}'")
            }
            BuildStudyError::RegisteredActionHasNoFields { action } => {
                write!(f, "registered action '{action}' is a Write with no fields defined")
            }
            BuildStudyError::MissingFieldChoice { action, field } => {
                write!(f, "action '{action}': no value chosen for field '{field}'")
            }
            BuildStudyError::UnknownFieldChoice { action, field, label } => {
                write!(
                    f,
                    "action '{action}' field '{field}': '{label}' isn't one of its registered values"
                )
            }
            BuildStudyError::NotWritable { action } => {
                write!(f, "action '{action}' isn't a Write operation, but field choices were given")
            }
        }
    }
}

impl std::error::Error for BuildStudyError {}

fn heapless_string<const N: usize>(
    value: &str,
    which: &'static str,
) -> Result<heapless::String<N>, BuildStudyError> {
    heapless::String::try_from(value).map_err(|_| BuildStudyError::NameTooLong {
        which,
        value: value.to_string(),
        max: N,
    })
}

/// Builds a real `Study` from a submitted table. `steps_crc` is left `0` —
/// whichever `embarch-api` call actually submits this `Study`
/// (`run-study`/`run_study`) recomputes and overwrites it unconditionally
/// regardless of what's given (design.md §3 decision 26), so there's
/// nothing for this offline function to compute it against yet.
pub fn build_study(
    study_name: &str,
    rows: &[TableRow],
    registry: &ActionRegistry,
) -> Result<Study, BuildStudyError> {
    if rows.len() > MAX_STEPS_PER_STUDY {
        return Err(BuildStudyError::TooManySteps { max: MAX_STEPS_PER_STUDY, actual: rows.len() });
    }

    let mut steps: HVec<Step, MAX_STEPS_PER_STUDY> = HVec::new();
    for row in rows {
        let action = resolve_action(&row.action, registry)?;
        let step = Step {
            name: heapless_string::<MAX_NAME_LEN>(&row.name, "step name")?,
            action,
            timeout_ms: row.timeout_ms,
            power_sample: None,
            continue_on_fail: row.continue_on_fail,
        };
        // Capacity already checked above (rows.len() <= MAX_STEPS_PER_STUDY),
        // so this can't actually fail -- .ok() rather than .unwrap() only to
        // avoid a panic if that invariant ever drifts.
        let _ = steps.push(step);
    }

    Ok(Study {
        name: heapless_string::<MAX_STUDY_NAME_LEN>(study_name, "study name")?,
        steps,
        validations: HVec::new(),
        steps_crc: 0,
    })
}

fn resolve_action(row_action: &RowAction, registry: &ActionRegistry) -> Result<Action, BuildStudyError> {
    match row_action {
        RowAction::BuiltIn { which, role } => Ok(match which {
            BuiltInActionKind::BleConnect => Action::BleConnect {
                role: match role {
                    RoleChoice::Central => BleRole::Central,
                    RoleChoice::Peripheral => BleRole::Peripheral,
                },
                target_address: None,
            },
            BuiltInActionKind::GattDiscover => Action::GattDiscover {},
            BuiltInActionKind::GattMonitorAll => Action::GattMonitorAll {},
        }),
        RowAction::Registered { name, field_choices } => {
            let registered = registry
                .actions
                .iter()
                .find(|a| &a.name == name)
                .ok_or_else(|| BuildStudyError::UnknownRegisteredAction(name.clone()))?;
            let operation = resolve_operation(registered, field_choices)?;
            Ok(Action::DataExchange {
                service_uuid: registered.service_uuid,
                characteristic_uuid: registered.uuid,
                operation,
            })
        }
    }
}

fn resolve_operation(
    registered: &RegisteredAction,
    field_choices: &HashMap<String, String>,
) -> Result<GattOperation, BuildStudyError> {
    match registered.operation {
        RegisteredOperation::Read => not_writable_if_choices_given(registered, field_choices, GattOperation::Read),
        RegisteredOperation::Subscribe => {
            not_writable_if_choices_given(registered, field_choices, GattOperation::Subscribe)
        }
        // Notify/Indicate's own wait timeout is a separate field from the
        // step's own `timeout_ms` (design.md §4.3) -- defaulted to match
        // the step's timeout rather than asking the UI for a second value,
        // a UI simplification (adjustable later), not a guess about any
        // particular DUT's protocol.
        RegisteredOperation::Notify => {
            not_writable_if_choices_given(registered, field_choices, GattOperation::Notify { timeout_ms: 0 })
        }
        RegisteredOperation::Indicate => {
            not_writable_if_choices_given(registered, field_choices, GattOperation::Indicate { timeout_ms: 0 })
        }
        RegisteredOperation::Write => {
            let payload = resolve_write_payload(registered, field_choices)?;
            Ok(GattOperation::Write { payload })
        }
    }
}

fn not_writable_if_choices_given(
    registered: &RegisteredAction,
    field_choices: &HashMap<String, String>,
    op: GattOperation,
) -> Result<GattOperation, BuildStudyError> {
    if field_choices.is_empty() {
        Ok(op)
    } else {
        Err(BuildStudyError::NotWritable { action: registered.name.clone() })
    }
}

fn resolve_write_payload(
    registered: &RegisteredAction,
    field_choices: &HashMap<String, String>,
) -> Result<heapless::Vec<u8, MAX_PAYLOAD_LEN>, BuildStudyError> {
    if registered.fields.is_empty() {
        return Err(BuildStudyError::RegisteredActionHasNoFields { action: registered.name.clone() });
    }

    let buffer_len = registered
        .fields
        .iter()
        .map(|f| f.byte_offset + f.byte_len)
        .max()
        .unwrap_or(0);
    let mut buffer = vec![0u8; buffer_len];

    for field in &registered.fields {
        let label = field_choices.get(&field.name).ok_or_else(|| BuildStudyError::MissingFieldChoice {
            action: registered.name.clone(),
            field: field.name.clone(),
        })?;
        let value = field.values.iter().find(|v| &v.label == label).ok_or_else(|| {
            BuildStudyError::UnknownFieldChoice {
                action: registered.name.clone(),
                field: field.name.clone(),
                label: label.clone(),
            }
        })?;
        buffer[field.byte_offset..field.byte_offset + field.byte_len].copy_from_slice(&value.bytes);
    }

    heapless::Vec::from_slice(&buffer).map_err(|_| BuildStudyError::TooManySteps {
        // Reused variant for "doesn't fit a fixed-capacity buffer" -- a
        // payload exceeding MAX_PAYLOAD_LEN is exactly as much an
        // over-capacity condition as too many steps is.
        max: MAX_PAYLOAD_LEN,
        actual: buffer_len,
    })
}

/// Convenience for the UI binary: the `service_uuid`/`uuid` an unregistered
/// characteristic needs supplied when the engineer registers an action
/// against it (milestone-11.md §3.4) — re-exported here rather than forcing
/// the UI binary to reach into `merged_actions` for one field pair.
pub fn characteristic_pair(service_uuid: Uuid, uuid: Uuid) -> (Uuid, Uuid) {
    (service_uuid, uuid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{ActionField, ActionFieldValue};

    fn uuid(byte: u8) -> Uuid {
        Uuid([byte; 16])
    }

    fn registry_with_write_action() -> ActionRegistry {
        ActionRegistry {
            actions: vec![RegisteredAction {
                name: "set_mode".to_string(),
                service_uuid: uuid(1),
                uuid: uuid(2),
                operation: RegisteredOperation::Write,
                fields: vec![ActionField {
                    name: "mode".to_string(),
                    byte_offset: 0,
                    byte_len: 1,
                    values: vec![
                        ActionFieldValue { label: "Off".to_string(), bytes: vec![0x00] },
                        ActionFieldValue { label: "On".to_string(), bytes: vec![0x01] },
                    ],
                }],
            }],
        }
    }

    #[test]
    fn built_in_ble_connect_defaults_to_central_role() {
        let rows = vec![TableRow {
            name: "connect".to_string(),
            action: RowAction::BuiltIn { which: BuiltInActionKind::BleConnect, role: RoleChoice::Central },
            timeout_ms: 20_000,
            continue_on_fail: false,
        }];
        let study = build_study("s", &rows, &ActionRegistry::default()).unwrap();
        match &study.steps[0].action {
            Action::BleConnect { role, target_address } => {
                assert_eq!(*role, BleRole::Central);
                assert!(target_address.is_none());
            }
            other => panic!("expected BleConnect, got {other:?}"),
        }
    }

    #[test]
    fn built_in_gatt_discover_and_monitor_all_are_fieldless() {
        let rows = vec![
            TableRow {
                name: "discover".to_string(),
                action: RowAction::BuiltIn { which: BuiltInActionKind::GattDiscover, role: RoleChoice::Central },
                timeout_ms: 15_000,
                continue_on_fail: false,
            },
            TableRow {
                name: "monitor".to_string(),
                action: RowAction::BuiltIn { which: BuiltInActionKind::GattMonitorAll, role: RoleChoice::Central },
                timeout_ms: 15_000,
                continue_on_fail: false,
            },
        ];
        let study = build_study("s", &rows, &ActionRegistry::default()).unwrap();
        assert!(matches!(study.steps[0].action, Action::GattDiscover {}));
        assert!(matches!(study.steps[1].action, Action::GattMonitorAll {}));
    }

    #[test]
    fn a_registered_write_action_splices_the_chosen_value_bytes_at_its_offset() {
        let registry = registry_with_write_action();
        let mut field_choices = HashMap::new();
        field_choices.insert("mode".to_string(), "On".to_string());
        let rows = vec![TableRow {
            name: "set-on".to_string(),
            action: RowAction::Registered { name: "set_mode".to_string(), field_choices },
            timeout_ms: 5_000,
            continue_on_fail: false,
        }];
        let study = build_study("s", &rows, &registry).unwrap();
        match &study.steps[0].action {
            Action::DataExchange { service_uuid, characteristic_uuid, operation } => {
                assert_eq!(*service_uuid, uuid(1));
                assert_eq!(*characteristic_uuid, uuid(2));
                match operation {
                    GattOperation::Write { payload } => assert_eq!(payload.as_slice(), &[0x01]),
                    other => panic!("expected Write, got {other:?}"),
                }
            }
            other => panic!("expected DataExchange, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_registered_action_name_is_a_named_error() {
        let rows = vec![TableRow {
            name: "x".to_string(),
            action: RowAction::Registered { name: "does_not_exist".to_string(), field_choices: HashMap::new() },
            timeout_ms: 1_000,
            continue_on_fail: false,
        }];
        let err = build_study("s", &rows, &ActionRegistry::default()).unwrap_err();
        assert_eq!(err, BuildStudyError::UnknownRegisteredAction("does_not_exist".to_string()));
    }

    #[test]
    fn a_missing_field_choice_is_a_named_error_not_a_default_value() {
        let registry = registry_with_write_action();
        let rows = vec![TableRow {
            name: "x".to_string(),
            action: RowAction::Registered { name: "set_mode".to_string(), field_choices: HashMap::new() },
            timeout_ms: 1_000,
            continue_on_fail: false,
        }];
        let err = build_study("s", &rows, &registry).unwrap_err();
        assert_eq!(
            err,
            BuildStudyError::MissingFieldChoice { action: "set_mode".to_string(), field: "mode".to_string() }
        );
    }

    #[test]
    fn an_unknown_field_choice_label_is_a_named_error() {
        let registry = registry_with_write_action();
        let mut field_choices = HashMap::new();
        field_choices.insert("mode".to_string(), "Sideways".to_string());
        let rows = vec![TableRow {
            name: "x".to_string(),
            action: RowAction::Registered { name: "set_mode".to_string(), field_choices },
            timeout_ms: 1_000,
            continue_on_fail: false,
        }];
        let err = build_study("s", &rows, &registry).unwrap_err();
        assert_eq!(
            err,
            BuildStudyError::UnknownFieldChoice {
                action: "set_mode".to_string(),
                field: "mode".to_string(),
                label: "Sideways".to_string()
            }
        );
    }

    #[test]
    fn too_many_steps_is_a_named_error() {
        let rows: Vec<TableRow> = (0..MAX_STEPS_PER_STUDY + 1)
            .map(|i| TableRow {
                name: format!("s{i}"),
                action: RowAction::BuiltIn { which: BuiltInActionKind::GattDiscover, role: RoleChoice::Central },
                timeout_ms: 1_000,
                continue_on_fail: false,
            })
            .collect();
        let err = build_study("s", &rows, &ActionRegistry::default()).unwrap_err();
        assert_eq!(err, BuildStudyError::TooManySteps { max: MAX_STEPS_PER_STUDY, actual: MAX_STEPS_PER_STUDY + 1 });
    }

    #[test]
    fn resulting_study_round_trips_through_serde_json_matching_the_run_study_wire_shape() {
        // Run on a dedicated, generously-sized stack: `Study` embeds a
        // `heapless::Vec<Step, MAX_STEPS_PER_STUDY>` -- a fixed-size *inline*
        // array sized for all 64 slots regardless of how many this test's
        // own one-step `Study` actually populates -- so deserializing it on
        // a debug build's default test-thread stack overflows, the exact
        // already-tracked risk `design.md` §7 documents (confirmed by
        // hitting it for real writing this test, not assumed). Same fix
        // this crate's own `self_test_fixture_round_trips_end_to_end`
        // (embarch-api's `study.rs`) already uses for the identical cause.
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(round_trip_body)
            .expect("failed to spawn test thread")
            .join()
            .expect("resulting_study_round_trips body panicked");
    }

    fn round_trip_body() {
        let rows = vec![TableRow {
            name: "connect".to_string(),
            action: RowAction::BuiltIn { which: BuiltInActionKind::BleConnect, role: RoleChoice::Central },
            timeout_ms: 20_000,
            continue_on_fail: false,
        }];
        let study = build_study("json-roundtrip", &rows, &ActionRegistry::default()).unwrap();
        let json = serde_json::to_string(&study).unwrap();
        let parsed: Study = serde_json::from_str(&json).unwrap();
        assert_eq!(study, parsed);
    }
}
