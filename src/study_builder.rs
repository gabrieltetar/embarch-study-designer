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
use crate::limits::{
    MAX_LOCAL_NAME_LEN, MAX_NAME_LEN, MAX_PAYLOAD_LEN, MAX_STEPS_PER_STUDY, MAX_STUDY_NAME_LEN,
};
use crate::registry::{ActionRegistry, RegisteredAction, RegisteredOperation};
use crate::study::{Action, BleRole, GattOperation, Requirements, Step, Study};

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
    /// design.md §3 decision 36.
    GattMonitorStart,
    /// design.md §3 decision 36.
    GattMonitorStop,
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
        /// Only meaningful for `BleConnect`: the advertised local name to
        /// connect to (design.md §3 decision 43). Blank or absent leaves
        /// the study taking whichever peripheral advertises first, which is
        /// the old behavior and rarely what anyone wants — see
        /// `Action::BleConnect::target_name`.
        #[serde(default)]
        target_name: Option<String>,
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
    /// A one-off `DataExchange` against a UUID pair the engineer typed
    /// directly, with a payload they supplied as literal bytes — design.md
    /// §3 decision 37, alongside the registry rather than replacing it.
    ///
    /// This does not weaken decision 35's rule. That rule forbids this crate
    /// inventing a *semantic description* of what an action does, and
    /// forbids it *encoding* a number into bytes on an engineer's behalf
    /// (which requires assuming a width and endianness nobody here knows).
    /// `payload` here is already bytes — whatever the engineer typed, parsed
    /// client-side by the same parser the registration form uses — so this
    /// layer still never interprets or encodes anything. The registry
    /// remains the way to *name and re-use* an action; this is the way to
    /// send one you haven't named yet.
    Raw {
        service_uuid: String,
        characteristic_uuid: String,
        operation: RegisteredOperation,
        #[serde(default)]
        payload: Vec<u8>,
    },
    /// A `DataExchange` against a **vendor-defined** service from
    /// [`crate::vendor`], picked by id rather than by typing UUIDs —
    /// design.md §3 decision 41.
    ///
    /// This sits between `Registered` and `Raw` and replaces neither.
    /// `Registered` names an action an engineer authored for *their* custom
    /// characteristic; `Raw` is for a UUID pair nobody has named yet; this
    /// is for a characteristic whose UUIDs were never anyone's to author,
    /// because the silicon/stack vendor published them. Nordic's UART
    /// Service is the case in point: transcribing
    /// `6e400002-b5a3-f393-e0a9-e50e24dcca9e` into a per-repo registry, in
    /// every repo, to write to a service Zephyr itself defines is pure
    /// error surface.
    ///
    /// `payload` is still literal engineer-supplied bytes, exactly as in
    /// `Raw`. That is the whole line this variant does not cross: the
    /// vendor table supplies the *address* (which service, which
    /// characteristic, which operations are legal there), never the
    /// *content*. Nothing in this crate knows what a given DUT expects to
    /// receive on a vendor characteristic — see `vendor.rs`'s own module
    /// doc comment.
    Vendor {
        /// [`crate::vendor::VendorService::id`], e.g. `"nordic-uart"`.
        service: String,
        /// [`crate::vendor::VendorCharacteristic::id`], e.g. `"rx"`.
        characteristic: String,
        operation: RegisteredOperation,
        #[serde(default)]
        payload: Vec<u8>,
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
    /// The row's "when" — `Step::delay_before_ms` (design.md §3 decision
    /// 40). `#[serde(default)]` so a table saved before this field existed
    /// still loads, as 0 (start immediately), which is exactly what those
    /// studies did.
    #[serde(default)]
    pub delay_before_ms: u32,
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
    UnparseableUuid { which: &'static str, value: String },
    PayloadTooLong { max: usize, actual: usize },
    PayloadOnNonWrite,
    UnknownVendorService(String),
    UnknownVendorCharacteristic { service: String, characteristic: String },
    /// The chosen operation isn't among the ones the vendor declares for
    /// that characteristic. Caught here rather than on hardware, where it
    /// surfaces as an opaque ATT error code mid-study.
    OperationNotDeclared {
        service: String,
        characteristic: String,
        operation: RegisteredOperation,
        properties: u8,
    },
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
            BuildStudyError::UnparseableUuid { which, value } => write!(
                f,
                "{which} '{value}' isn't a UUID — expected the hyphenated 128-bit form, \
                 32 hex digits, or a 16-bit shorthand like 180f"
            ),
            BuildStudyError::PayloadTooLong { max, actual } => {
                write!(f, "payload is {actual} bytes, but the limit is {max}")
            }
            BuildStudyError::PayloadOnNonWrite => {
                write!(f, "a payload was given for an operation that isn't a Write")
            }
            BuildStudyError::UnknownVendorService(id) => {
                write!(f, "no vendor-defined service with id '{id}'")
            }
            BuildStudyError::UnknownVendorCharacteristic { service, characteristic } => {
                write!(f, "vendor service '{service}' has no characteristic '{characteristic}'")
            }
            BuildStudyError::OperationNotDeclared {
                service,
                characteristic,
                operation,
                properties,
            } => write!(
                f,
                "{service}/{characteristic} doesn't declare {operation:?} \
                 (its properties byte is {properties:#04x})"
            ),
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
    requires: Requirements,
    rows: &[TableRow],
    registry: &ActionRegistry,
) -> Result<Study, BuildStudyError> {
    if rows.len() > MAX_STEPS_PER_STUDY {
        return Err(BuildStudyError::TooManySteps { max: MAX_STEPS_PER_STUDY, actual: rows.len() });
    }

    let mut steps: crate::bounded::StepList = crate::bounded::StepList::new();
    for row in rows {
        let action = resolve_action(&row.action, registry)?;
        let step = Step {
            name: heapless_string::<MAX_NAME_LEN>(&row.name, "step name")?,
            action,
            timeout_ms: row.timeout_ms,
            continue_on_fail: row.continue_on_fail,
            delay_before_ms: row.delay_before_ms,
        };
        // Capacity already checked above (rows.len() <= MAX_STEPS_PER_STUDY),
        // so this can't actually fail -- .ok() rather than .unwrap() only to
        // avoid a panic if that invariant ever drifts.
        let _ = steps.push(step);
    }

    Ok(Study {
        name: heapless_string::<MAX_STUDY_NAME_LEN>(study_name, "study name")?,
        // Taken from the caller rather than defaulted here on purpose
        // (design.md §3 decision 40): "any build" is a legitimate answer that
        // has to be *said*, and this function has no idea which bench the
        // study is for. `embarch-ui`'s Study Designer is where a human says
        // it (`embarch-ui/design.md` §3 decision 11, Milestone 7 Phase D).
        requires,
        steps,
        validations: HVec::new(),
        streams: HVec::new(),
        steps_crc: 0,
        // Both seals are left at 0 here, and both are overwritten by
        // whoever submits (`embarch-api/design.md` §3 decision 26). For
        // `streams_crc` that zero happens to already be correct — this
        // builder authors no taps, and 0 is the real CRC of an empty tap
        // list (`crate::crc::streams_crc`) — but it is not written *as* a
        // correct value, for the same reason `steps_crc` isn't.
        streams_crc: 0,
    })
}

fn resolve_action(row_action: &RowAction, registry: &ActionRegistry) -> Result<Action, BuildStudyError> {
    match row_action {
        RowAction::BuiltIn { which, role, target_name } => Ok(match which {
            BuiltInActionKind::BleConnect => Action::BleConnect {
                role: match role {
                    RoleChoice::Central => BleRole::Central,
                    RoleChoice::Peripheral => BleRole::Peripheral,
                },
                target_address: None,
                target_name: match target_name.as_deref().map(str::trim) {
                    // Empty means "no filter" rather than "match the empty
                    // name": the UI's input starts blank, and an untouched
                    // field must not become a filter nothing can satisfy.
                    None | Some("") => None,
                    Some(name) => Some(heapless_string::<MAX_LOCAL_NAME_LEN>(name, "target name")?),
                },
            },
            BuiltInActionKind::GattDiscover => Action::GattDiscover {},
            BuiltInActionKind::GattMonitorAll => Action::GattMonitorAll {},
            BuiltInActionKind::GattMonitorStart => Action::GattMonitorStart {},
            BuiltInActionKind::GattMonitorStop => Action::GattMonitorStop {},
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
        RowAction::Raw { service_uuid, characteristic_uuid, operation, payload } => {
            let service_uuid = Uuid::parse(service_uuid).ok_or_else(|| {
                BuildStudyError::UnparseableUuid {
                    which: "service UUID",
                    value: service_uuid.clone(),
                }
            })?;
            let characteristic_uuid = Uuid::parse(characteristic_uuid).ok_or_else(|| {
                BuildStudyError::UnparseableUuid {
                    which: "characteristic UUID",
                    value: characteristic_uuid.clone(),
                }
            })?;

            let operation = literal_operation(*operation, payload)?;

            Ok(Action::DataExchange { service_uuid, characteristic_uuid, operation })
        }
        RowAction::Vendor { service, characteristic, operation, payload } => {
            let (vendor_service, vendor_chrc) =
                crate::vendor::find_characteristic(service, characteristic).ok_or_else(|| {
                    // Distinguish the two failures: a stale saved study
                    // naming a service this build dropped reads very
                    // differently from a typo'd characteristic id.
                    if crate::vendor::find(service).is_none() {
                        BuildStudyError::UnknownVendorService(service.clone())
                    } else {
                        BuildStudyError::UnknownVendorCharacteristic {
                            service: service.clone(),
                            characteristic: characteristic.clone(),
                        }
                    }
                })?;

            if !declares(vendor_chrc.properties, *operation) {
                return Err(BuildStudyError::OperationNotDeclared {
                    service: vendor_service.id.to_string(),
                    characteristic: vendor_chrc.id.to_string(),
                    operation: *operation,
                    properties: vendor_chrc.properties,
                });
            }

            let operation = literal_operation(*operation, payload)?;

            Ok(Action::DataExchange {
                service_uuid: vendor_service.uuid,
                characteristic_uuid: vendor_chrc.uuid,
                operation,
            })
        }
    }
}

/// Whether an ATT properties byte declares the bit a given operation needs.
///
/// The bit values are the Bluetooth Core Spec's (Vol 3, Part G), and the
/// mapping is only as specific as it can honestly be: `Write` requires the
/// Write bit rather than accepting Write-Without-Response, because dev-bench
/// issues a Write *Request* (`bt_gatt_write`) and a characteristic that
/// declares only Write-Without-Response will reject it. Checked against a
/// vendor entry's declared properties only — for a `Raw` row there's nothing
/// to check against, and live discovery is the authority regardless.
fn declares(properties: u8, operation: RegisteredOperation) -> bool {
    let needed = match operation {
        RegisteredOperation::Read => 0x02,
        RegisteredOperation::Write => 0x08,
        RegisteredOperation::Subscribe | RegisteredOperation::Notify => 0x10,
        RegisteredOperation::Indicate => 0x20,
    };
    properties & needed != 0
}

/// Turns an operation plus already-literal engineer-supplied bytes into a
/// `GattOperation`. Shared by `RowAction::Raw` and `RowAction::Vendor`,
/// which differ only in where the UUID pair comes from — the payload rule
/// (bytes pass through untouched; a payload against a non-Write is refused
/// rather than silently dropped) is identical and must stay that way.
fn literal_operation(
    operation: RegisteredOperation,
    payload: &[u8],
) -> Result<GattOperation, BuildStudyError> {
    Ok(match operation {
        RegisteredOperation::Write => {
            let bytes: HVec<u8, MAX_PAYLOAD_LEN> =
                HVec::from_slice(payload).map_err(|_| BuildStudyError::PayloadTooLong {
                    max: MAX_PAYLOAD_LEN,
                    actual: payload.len(),
                })?;
            GattOperation::Write { payload: bytes }
        }
        // A payload only means something for a Write — silently dropping one
        // the engineer typed against a Read would be worse than refusing it.
        _ if !payload.is_empty() => return Err(BuildStudyError::PayloadOnNonWrite),
        RegisteredOperation::Read => GattOperation::Read,
        RegisteredOperation::Subscribe => GattOperation::Subscribe,
        // Same defaulting as a registered Notify/Indicate: the step's own
        // `timeout_ms` is the wait budget.
        RegisteredOperation::Notify => GattOperation::Notify { timeout_ms: 0 },
        RegisteredOperation::Indicate => GattOperation::Indicate { timeout_ms: 0 },
    })
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

    /// Shadows the real [`super::build_study`] with its pre-decision-40
    /// arity. Every case in this module is about resolving a table row into
    /// an `Action`, not about which builds a study requires, so they all
    /// author [`Requirements::any`] — the explicit "doesn't matter here"
    /// value, said rather than defaulted (design.md §3 decision 40).
    fn build_study(
        study_name: &str,
        rows: &[TableRow],
        registry: &ActionRegistry,
    ) -> Result<Study, BuildStudyError> {
        super::build_study(study_name, Requirements::any(), rows, registry)
    }

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
            action: RowAction::BuiltIn { which: BuiltInActionKind::BleConnect, role: RoleChoice::Central , target_name: None },
            timeout_ms: 20_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        }];
        let study = build_study("s", &rows, &ActionRegistry::default()).unwrap();
        match &study.steps[0].action {
            Action::BleConnect { role, target_address, target_name } => {
                assert_eq!(*role, BleRole::Central);
                assert!(target_address.is_none());
                assert!(target_name.is_none());
            }
            other => panic!("expected BleConnect, got {other:?}"),
        }
    }

    #[test]
    fn built_in_gatt_discover_and_monitor_all_are_fieldless() {
        let rows = vec![
            TableRow {
                name: "discover".to_string(),
                action: RowAction::BuiltIn { which: BuiltInActionKind::GattDiscover, role: RoleChoice::Central , target_name: None },
                timeout_ms: 15_000,
                continue_on_fail: false,
                delay_before_ms: 0,
            },
            TableRow {
                name: "monitor".to_string(),
                action: RowAction::BuiltIn { which: BuiltInActionKind::GattMonitorAll, role: RoleChoice::Central , target_name: None },
                timeout_ms: 15_000,
                continue_on_fail: false,
                delay_before_ms: 0,
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
            delay_before_ms: 0,
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
            delay_before_ms: 0,
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
            delay_before_ms: 0,
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
            delay_before_ms: 0,
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
    fn raw_row_builds_a_data_exchange_write_from_typed_uuids_and_literal_bytes() {
        // design.md §3 decision 37 — the free-text path, e.g. an NUS shell
        // write, with no registry entry involved at all.
        let rows = vec![TableRow {
            name: "nus-write".into(),
            action: RowAction::Raw {
                service_uuid: "6e400001-b5a3-f393-e0a9-e50e24dcca9e".into(),
                characteristic_uuid: "6e400002-b5a3-f393-e0a9-e50e24dcca9e".into(),
                operation: RegisteredOperation::Write,
                payload: b"help\n".to_vec(),
            },
            timeout_ms: 3_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        }];

        let study = build_study("nus", &rows, &ActionRegistry::default()).unwrap();
        match &study.steps[0].action {
            Action::DataExchange { service_uuid, characteristic_uuid, operation } => {
                assert_eq!(
                    service_uuid.to_hyphenated().as_str(),
                    "6e400001-b5a3-f393-e0a9-e50e24dcca9e"
                );
                assert_eq!(
                    characteristic_uuid.to_hyphenated().as_str(),
                    "6e400002-b5a3-f393-e0a9-e50e24dcca9e"
                );
                match operation {
                    // The bytes arrive exactly as supplied — nothing in this
                    // layer re-encodes or reinterprets them.
                    GattOperation::Write { payload } => assert_eq!(&payload[..], b"help\n"),
                    other => panic!("expected Write, got {other:?}"),
                }
            }
            other => panic!("expected DataExchange, got {other:?}"),
        }
    }

    #[test]
    fn raw_row_accepts_a_16_bit_shorthand_uuid() {
        let rows = vec![TableRow {
            name: "read-batt".into(),
            action: RowAction::Raw {
                service_uuid: "180f".into(),
                characteristic_uuid: "0x2a19".into(),
                operation: RegisteredOperation::Read,
                payload: Vec::new(),
            },
            timeout_ms: 2_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        }];

        let study = build_study("batt", &rows, &ActionRegistry::default()).unwrap();
        match &study.steps[0].action {
            Action::DataExchange { service_uuid, characteristic_uuid, operation } => {
                assert_eq!(
                    service_uuid.to_hyphenated().as_str(),
                    "0000180f-0000-1000-8000-00805f9b34fb"
                );
                assert_eq!(
                    characteristic_uuid.to_hyphenated().as_str(),
                    "00002a19-0000-1000-8000-00805f9b34fb"
                );
                assert_eq!(operation, &GattOperation::Read);
            }
            other => panic!("expected DataExchange, got {other:?}"),
        }
    }

    #[test]
    fn raw_row_names_every_failure_rather_than_silently_coping() {
        let row = |op, payload: Vec<u8>, svc: &str| TableRow {
            name: "row".into(),
            action: RowAction::Raw {
                service_uuid: svc.into(),
                characteristic_uuid: "2a19".into(),
                operation: op,
                payload,
            },
            timeout_ms: 1_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        };
        let reg = ActionRegistry::default();

        // An unparseable UUID is named, not coerced to zeros.
        assert!(matches!(
            build_study("s", &[row(RegisteredOperation::Read, vec![], "nope")], &reg),
            Err(BuildStudyError::UnparseableUuid { which: "service UUID", .. })
        ));

        // A payload against a non-Write is refused rather than dropped —
        // silently discarding bytes the engineer typed is the worse failure.
        assert!(matches!(
            build_study("s", &[row(RegisteredOperation::Read, vec![1, 2], "180f")], &reg),
            Err(BuildStudyError::PayloadOnNonWrite)
        ));

        // Over-length payloads are named against the real limit.
        assert!(matches!(
            build_study(
                "s",
                &[row(RegisteredOperation::Write, vec![0u8; MAX_PAYLOAD_LEN + 1], "180f")],
                &reg
            ),
            Err(BuildStudyError::PayloadTooLong { max: MAX_PAYLOAD_LEN, .. })
        ));
    }

    #[test]
    fn monitor_window_built_ins_build_their_actions() {
        let rows = vec![
            TableRow {
                name: "open".into(),
                action: RowAction::BuiltIn {
                    which: BuiltInActionKind::GattMonitorStart,
                    role: RoleChoice::Central,
                    target_name: None,
                },
                timeout_ms: 10_000,
                continue_on_fail: false,
                delay_before_ms: 0,
            },
            TableRow {
                name: "close".into(),
                action: RowAction::BuiltIn {
                    which: BuiltInActionKind::GattMonitorStop,
                    role: RoleChoice::Central,
                    target_name: None,
                },
                timeout_ms: 5_000,
                continue_on_fail: false,
                delay_before_ms: 0,
            },
        ];
        let study = build_study("window", &rows, &ActionRegistry::default()).unwrap();
        assert_eq!(study.steps[0].action, Action::GattMonitorStart {});
        assert_eq!(study.steps[1].action, Action::GattMonitorStop {});
    }

    #[test]
    fn too_many_steps_is_a_named_error() {
        let rows: Vec<TableRow> = (0..MAX_STEPS_PER_STUDY + 1)
            .map(|i| TableRow {
                name: format!("s{i}"),
                action: RowAction::BuiltIn { which: BuiltInActionKind::GattDiscover, role: RoleChoice::Central , target_name: None },
                timeout_ms: 1_000,
                continue_on_fail: false,
                delay_before_ms: 0,
            })
            .collect();
        let err = build_study("s", &rows, &ActionRegistry::default()).unwrap_err();
        assert_eq!(err, BuildStudyError::TooManySteps { max: MAX_STEPS_PER_STUDY, actual: MAX_STEPS_PER_STUDY + 1 });
    }

    #[test]
    fn resulting_study_round_trips_through_serde_json_matching_the_run_study_wire_shape() {
        // Kept on a dedicated, generously-sized stack even though design.md
        // §3 decision 46 removed the cause: `Study.steps` is a heap `Vec`
        // under `alloc` (which `study-ui` implies via `std`), so the 64-slot
        // inline array this comment used to describe is gone here. The big
        // stack stays because `Step` itself is still large and this test
        // deserializes a whole `Study` -- it is cheap insurance, not a
        // workaround any more.
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
            action: RowAction::BuiltIn { which: BuiltInActionKind::BleConnect, role: RoleChoice::Central , target_name: None },
            timeout_ms: 20_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        }];
        let study = build_study("json-roundtrip", &rows, &ActionRegistry::default()).unwrap();
        let json = serde_json::to_string(&study).unwrap();
        let parsed: Study = serde_json::from_str(&json).unwrap();
        assert_eq!(study, parsed);
    }

    // --- decision 41: vendor-defined services ---------------------------

    fn vendor_row(characteristic: &str, operation: RegisteredOperation, payload: Vec<u8>) -> TableRow {
        TableRow {
            name: "stimulate".to_string(),
            action: RowAction::Vendor {
                service: "nordic-uart".to_string(),
                characteristic: characteristic.to_string(),
                operation,
                payload,
            },
            timeout_ms: 5_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        }
    }

    /// The whole point of decision 41: the engineer names the service, and
    /// the real UUIDs come out the other end without anyone typing them.
    #[test]
    fn vendor_row_resolves_nus_rx_uuids_without_the_engineer_typing_them() {
        let rows = vec![vendor_row("rx", RegisteredOperation::Write, b"hello\n".to_vec())];
        let study = build_study("s", &rows, &ActionRegistry::default()).unwrap();
        match &study.steps[0].action {
            Action::DataExchange { service_uuid, characteristic_uuid, operation } => {
                assert_eq!(
                    service_uuid.to_hyphenated().as_str(),
                    "6e400001-b5a3-f393-e0a9-e50e24dcca9e"
                );
                assert_eq!(
                    characteristic_uuid.to_hyphenated().as_str(),
                    "6e400002-b5a3-f393-e0a9-e50e24dcca9e"
                );
                match operation {
                    GattOperation::Write { payload } => assert_eq!(&payload[..], b"hello\n"),
                    other => panic!("expected Write, got {other:?}"),
                }
            }
            other => panic!("expected DataExchange, got {other:?}"),
        }
    }

    /// A vendor row is a `DataExchange` like any other by the time it leaves
    /// here — dev-bench never learns the table exists, which is why decision
    /// 39 needed no firmware change and no schema bump of its own.
    #[test]
    fn a_vendor_row_and_the_equivalent_raw_row_build_the_identical_action() {
        let vendor = build_study(
            "s",
            &[vendor_row("rx", RegisteredOperation::Write, b"x".to_vec())],
            &ActionRegistry::default(),
        )
        .unwrap();
        let raw = build_study(
            "s",
            &[TableRow {
                name: "stimulate".to_string(),
                action: RowAction::Raw {
                    service_uuid: "6e400001-b5a3-f393-e0a9-e50e24dcca9e".to_string(),
                    characteristic_uuid: "6e400002-b5a3-f393-e0a9-e50e24dcca9e".to_string(),
                    operation: RegisteredOperation::Write,
                    payload: b"x".to_vec(),
                },
                timeout_ms: 5_000,
                continue_on_fail: false,
                delay_before_ms: 0,
            }],
            &ActionRegistry::default(),
        )
        .unwrap();
        assert_eq!(vendor.steps[0].action, raw.steps[0].action);
    }

    #[test]
    fn vendor_tx_subscribes_and_notifies_but_cannot_be_written() {
        let ok = build_study(
            "s",
            &[vendor_row("tx", RegisteredOperation::Subscribe, vec![])],
            &ActionRegistry::default(),
        );
        assert!(ok.is_ok());

        // NUS TX is Notify-only; a Write against it would fail on hardware
        // with an opaque ATT error mid-study, so it's refused here.
        let err = build_study(
            "s",
            &[vendor_row("tx", RegisteredOperation::Write, b"x".to_vec())],
            &ActionRegistry::default(),
        )
        .unwrap_err();
        match err {
            BuildStudyError::OperationNotDeclared { service, characteristic, properties, .. } => {
                assert_eq!(service, "nordic-uart");
                assert_eq!(characteristic, "tx");
                assert_eq!(properties, 0x10);
            }
            other => panic!("expected OperationNotDeclared, got {other:?}"),
        }
    }

    /// NUS RX declares Read nowhere, so reading it is refused too — the
    /// check is against the properties byte, not a special case for writes.
    #[test]
    fn vendor_rx_cannot_be_read() {
        let err = build_study(
            "s",
            &[vendor_row("rx", RegisteredOperation::Read, vec![])],
            &ActionRegistry::default(),
        )
        .unwrap_err();
        assert!(matches!(err, BuildStudyError::OperationNotDeclared { .. }));
    }

    #[test]
    fn unknown_vendor_service_and_characteristic_are_distinguished() {
        let bad_service = build_study(
            "s",
            &[TableRow {
                name: "x".to_string(),
                action: RowAction::Vendor {
                    service: "nordic_uart".to_string(),
                    characteristic: "rx".to_string(),
                    operation: RegisteredOperation::Write,
                    payload: vec![],
                },
                timeout_ms: 1,
                continue_on_fail: false,
                delay_before_ms: 0,
            }],
            &ActionRegistry::default(),
        )
        .unwrap_err();
        assert!(matches!(bad_service, BuildStudyError::UnknownVendorService(_)));

        let bad_chrc =
            build_study("s", &[vendor_row("rxx", RegisteredOperation::Write, vec![])], &ActionRegistry::default())
                .unwrap_err();
        assert!(matches!(bad_chrc, BuildStudyError::UnknownVendorCharacteristic { .. }));
    }

    /// Same rule as `Raw`: a payload against a non-Write is refused, not
    /// silently dropped.
    #[test]
    fn vendor_payload_against_a_subscribe_is_refused() {
        let err = build_study(
            "s",
            &[vendor_row("tx", RegisteredOperation::Subscribe, b"x".to_vec())],
            &ActionRegistry::default(),
        )
        .unwrap_err();
        assert!(matches!(err, BuildStudyError::PayloadOnNonWrite));
    }

    // --- decision 42: the "when" half ------------------------------------

    #[test]
    fn delay_before_ms_reaches_the_step() {
        let rows = vec![TableRow {
            name: "wait-then-write".to_string(),
            action: RowAction::Vendor {
                service: "nordic-uart".to_string(),
                characteristic: "rx".to_string(),
                operation: RegisteredOperation::Write,
                payload: b"x".to_vec(),
            },
            timeout_ms: 5_000,
            continue_on_fail: false,
            delay_before_ms: 2_500,
        }];
        let study = build_study("s", &rows, &ActionRegistry::default()).unwrap();
        assert_eq!(study.steps[0].delay_before_ms, 2_500);
        // The delay is *not* taken out of the action's own budget.
        assert_eq!(study.steps[0].timeout_ms, 5_000);
    }

    /// A table saved before decision 42 existed has no `delay_before_ms`
    /// key at all; it must load as "start immediately", which is what those
    /// studies already did.
    #[test]
    fn a_row_saved_without_delay_before_ms_loads_as_zero() {
        let json = r#"{
            "name": "connect",
            "action": { "kind": "built_in", "which": "ble_connect", "role": "central" },
            "timeout_ms": 20000
        }"#;
        let row: TableRow = serde_json::from_str(json).unwrap();
        assert_eq!(row.delay_before_ms, 0);
        assert!(!row.continue_on_fail);
    }

    /// Two otherwise-identical studies that differ only in their delay must
    /// seal to different CRCs, or `steps_crc` would stop covering the
    /// timing an engineer authored.
    #[test]
    fn delay_before_ms_is_covered_by_steps_crc() {
        let mut row = TableRow {
            name: "connect".to_string(),
            action: RowAction::BuiltIn { which: BuiltInActionKind::BleConnect, role: RoleChoice::Central , target_name: None },
            timeout_ms: 20_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        };
        let a = build_study("s", std::slice::from_ref(&row), &ActionRegistry::default()).unwrap();
        row.delay_before_ms = 1_000;
        let b = build_study("s", std::slice::from_ref(&row), &ActionRegistry::default()).unwrap();
        assert_ne!(
            crate::crc::steps_crc(&a.steps).unwrap(),
            crate::crc::steps_crc(&b.steps).unwrap()
        );
    }

    // --- decision 43: naming the DUT to connect to -----------------------

    fn connect_row(target_name: Option<&str>) -> TableRow {
        TableRow {
            name: "connect".to_string(),
            action: RowAction::BuiltIn {
                which: BuiltInActionKind::BleConnect,
                role: RoleChoice::Central,
                target_name: target_name.map(str::to_string),
            },
            timeout_ms: 20_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        }
    }

    fn built_target_name(row: TableRow) -> Option<String> {
        let study = build_study("s", &[row], &ActionRegistry::default()).unwrap();
        match &study.steps[0].action {
            Action::BleConnect { target_name, .. } => target_name.as_ref().map(|n| n.to_string()),
            other => panic!("expected BleConnect, got {other:?}"),
        }
    }

    #[test]
    fn target_name_reaches_the_action() {
        assert_eq!(built_target_name(connect_row(Some("the client S11"))).as_deref(),
                   Some("the client S11"));
    }

    /// A blank input must mean "no filter", not "match the empty name" --
    /// the latter is a filter nothing can ever satisfy, so a study whose
    /// name field was simply never touched would never connect at all.
    #[test]
    fn blank_or_whitespace_target_name_is_no_filter() {
        assert!(built_target_name(connect_row(None)).is_none());
        assert!(built_target_name(connect_row(Some(""))).is_none());
        assert!(built_target_name(connect_row(Some("   "))).is_none());
    }

    #[test]
    fn target_name_is_trimmed() {
        assert_eq!(built_target_name(connect_row(Some("  S11  "))).as_deref(), Some("S11"));
    }

    /// The advertised-name field is bounded by `MAX_LOCAL_NAME_LEN` (26), well
    /// under Zephyr's own `CONFIG_BT_DEVICE_NAME_MAX` -- so an over-long name
    /// has to be refused with a message, not silently truncated into a filter
    /// that matches nothing.
    #[test]
    fn over_long_target_name_is_refused() {
        let long = "x".repeat(MAX_LOCAL_NAME_LEN + 1);
        let err = build_study("s", &[connect_row(Some(&long))], &ActionRegistry::default())
            .unwrap_err();
        match err {
            BuildStudyError::NameTooLong { which, max, .. } => {
                assert_eq!(which, "target name");
                assert_eq!(max, MAX_LOCAL_NAME_LEN);
            }
            other => panic!("expected NameTooLong, got {other:?}"),
        }
    }

    /// A row saved before decision 43 has no `target_name` key at all.
    #[test]
    fn a_connect_row_saved_without_target_name_loads_as_none() {
        let json = r#"{
            "name": "connect",
            "action": { "kind": "built_in", "which": "ble_connect" },
            "timeout_ms": 20000
        }"#;
        let row: TableRow = serde_json::from_str(json).unwrap();
        assert!(built_target_name(row).is_none());
    }

    /// Only `BleConnect` has anywhere to put it; naming a device on a
    /// monitor step must not silently do nothing surprising elsewhere.
    #[test]
    fn target_name_on_a_non_connect_built_in_is_ignored() {
        let row = TableRow {
            name: "open".to_string(),
            action: RowAction::BuiltIn {
                which: BuiltInActionKind::GattMonitorStart,
                role: RoleChoice::Central,
                target_name: Some("the client S11".to_string()),
            },
            timeout_ms: 10_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        };
        let study = build_study("s", &[row], &ActionRegistry::default()).unwrap();
        assert_eq!(study.steps[0].action, Action::GattMonitorStart {});
    }
}
