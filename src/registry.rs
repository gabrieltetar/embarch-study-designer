//! The user-authored custom-action registry — design.md §3 decision 35.
//!
//! `std`-only (file I/O, `toml`), gated behind the `study-ui` feature —
//! never linked by dev-bench firmware or embarch-core/embarch-api's plain
//! Cargo-dependency use, same posture as `gatt_extract` (§3 decision 33).
//!
//! **The one rule this whole module exists to enforce: nothing in this
//! crate ever infers what a GATT action does.** A [`RegisteredAction`] is
//! never a semantic description ("this starts streaming HRM") — it's a
//! name and a set of engineer-supplied literal byte choices for a field,
//! full stop. Where the exact final bytes for a named choice come from is
//! the engineer's problem to know, not this module's to guess: a value is
//! stored as the literal bytes to send (`ActionFieldValue::bytes`), never a
//! numeric type this module would have to encode itself — encoding implies
//! an endianness/width assumption nobody here is in a position to make.
//!
//! Persisted as `<firmware-repo>/embarch/study-actions.toml`, sibling to
//! `embarch.toml` — travels with the firmware repo, shared across engineers
//! the same way that file already is (`embarch-study-designer/milestone-11.md`
//! §3.1).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ids::Uuid;

/// The GATT-level operation a [`RegisteredAction`] performs. A subset of
/// `study::GattOperation` (no `StreamCapture`, decision 35's own "doesn't
/// need it here" call) — kept as its own type rather than reusing
/// `GattOperation` directly, since a registered action doesn't carry that
/// enum's per-call timeout fields (`Notify { timeout_ms }`, `Indicate
/// { timeout_ms }` — those belong on the `Step`, not the registry entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisteredOperation {
    Read,
    Write,
    Subscribe,
    Notify,
    Indicate,
}

/// One named, clickable choice for an [`ActionField`] — the engineer's own
/// label next to the exact literal bytes to send for it. `bytes.len()` must
/// equal the owning field's `byte_len`; checked by [`ActionRegistry::validate`],
/// not enforced structurally (a TOML file is hand-editable, and a length
/// mismatch is a clearer error surfaced explicitly than a type that can't
/// represent the mistake at all).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionFieldValue {
    pub label: String,
    pub bytes: Vec<u8>,
}

/// One named byte range within a `Write` action's payload, plus every
/// choice the engineer has registered for it. Multiple fields describe a
/// payload byte-range by byte-range; a payload with only one meaningful
/// byte still gets exactly one field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionField {
    pub name: String,
    pub byte_offset: usize,
    pub byte_len: usize,
    pub values: Vec<ActionFieldValue>,
}

/// One engineer-registered action against a specific, already-detected
/// characteristic. `fields` is only meaningful for `operation: Write` —
/// empty for every other operation, per this module's own doc comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisteredAction {
    pub name: String,
    /// The primary service this characteristic belongs to — `Action::DataExchange`
    /// (the `Action` variant every registered action ultimately becomes,
    /// `src/study_builder.rs`) needs both, not `uuid` alone.
    pub service_uuid: Uuid,
    pub uuid: Uuid,
    pub operation: RegisteredOperation,
    #[serde(default)]
    pub fields: Vec<ActionField>,
}

/// The full registry, one per firmware repo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRegistry {
    #[serde(default)]
    pub actions: Vec<RegisteredAction>,
}

/// Names the specific failure rather than surfacing a raw I/O/parse error,
/// matching this crate's existing discipline (`gatt_extract::ExtractError`).
#[derive(Debug)]
pub enum RegistryError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    Serialize(toml::ser::Error),
    /// A `RegisteredAction`'s field has a value whose `bytes.len()` doesn't
    /// match that field's own declared `byte_len` — caught explicitly
    /// (§3.1's own "checked, not structurally enforced" note) rather than
    /// silently truncating or padding a mismatch a hand-edited file could
    /// easily introduce.
    FieldLengthMismatch {
        action_name: String,
        field_name: String,
        value_label: String,
        expected: usize,
        actual: usize,
    },
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::Io(e) => write!(f, "I/O error: {e}"),
            RegistryError::Parse(e) => write!(f, "failed to parse registry TOML: {e}"),
            RegistryError::Serialize(e) => write!(f, "failed to serialize registry TOML: {e}"),
            RegistryError::FieldLengthMismatch {
                action_name,
                field_name,
                value_label,
                expected,
                actual,
            } => write!(
                f,
                "action '{action_name}' field '{field_name}' value '{value_label}': \
                 declared byte_len {expected}, but bytes has length {actual}"
            ),
        }
    }
}

impl std::error::Error for RegistryError {}

/// `<firmware-repo>/embarch/study-actions.toml` — sibling to `embarch.toml`
/// (`embarch-api/design.md` §4's own convention for that file's location).
pub fn registry_path(firmware_repo_root: &Path) -> PathBuf {
    firmware_repo_root.join("embarch").join("study-actions.toml")
}

impl ActionRegistry {
    /// Loads the registry from `<firmware_repo_root>/embarch/study-actions.toml`.
    /// A missing file is an empty registry, not an error — this file has no
    /// `embarch init`-equivalent bootstrap step yet (milestone-11.md §5), so
    /// "doesn't exist" is the ordinary starting state for a firmware repo
    /// that's never registered a custom action.
    pub fn load(firmware_repo_root: &Path) -> Result<ActionRegistry, RegistryError> {
        let path = registry_path(firmware_repo_root);
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ActionRegistry::default()),
            Err(e) => return Err(RegistryError::Io(e)),
        };
        let registry: ActionRegistry = toml::from_str(&raw).map_err(RegistryError::Parse)?;
        registry.validate()?;
        Ok(registry)
    }

    /// Writes the registry to `<firmware_repo_root>/embarch/study-actions.toml`,
    /// creating the `embarch/` directory if it doesn't exist yet.
    pub fn save(&self, firmware_repo_root: &Path) -> Result<(), RegistryError> {
        self.validate()?;
        let path = registry_path(firmware_repo_root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(RegistryError::Io)?;
        }
        let raw = toml::to_string_pretty(self).map_err(RegistryError::Serialize)?;
        fs::write(&path, raw).map_err(RegistryError::Io)
    }

    /// Confirms every field's every value has exactly `byte_len` bytes.
    /// Pure/offline — no I/O, callable independent of `load`/`save`.
    pub fn validate(&self) -> Result<(), RegistryError> {
        for action in &self.actions {
            for field in &action.fields {
                for value in &field.values {
                    if value.bytes.len() != field.byte_len {
                        return Err(RegistryError::FieldLengthMismatch {
                            action_name: action.name.clone(),
                            field_name: field.name.clone(),
                            value_label: value.label.clone(),
                            expected: field.byte_len,
                            actual: value.bytes.len(),
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_registry() -> ActionRegistry {
        ActionRegistry {
            actions: vec![RegisteredAction {
                name: "example_write".to_string(),
                service_uuid: Uuid([0xAA; 16]),
                uuid: Uuid([0xAB; 16]),
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
    fn round_trips_through_toml() {
        let registry = sample_registry();
        let raw = toml::to_string_pretty(&registry).unwrap();
        let parsed: ActionRegistry = toml::from_str(&raw).unwrap();
        assert_eq!(registry, parsed);
    }

    #[test]
    fn load_of_a_missing_file_is_an_empty_registry_not_an_error() {
        let dir = std::env::temp_dir().join(format!(
            "embarch-study-designer-registry-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let registry = ActionRegistry::load(&dir).unwrap();
        assert_eq!(registry, ActionRegistry::default());
    }

    #[test]
    fn save_then_load_round_trips_on_disk() {
        let dir = std::env::temp_dir().join(format!(
            "embarch-study-designer-registry-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
                + 1
        ));
        let registry = sample_registry();
        registry.save(&dir).unwrap();
        assert!(registry_path(&dir).is_file());
        let loaded = ActionRegistry::load(&dir).unwrap();
        assert_eq!(registry, loaded);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn validate_catches_a_field_length_mismatch() {
        let mut registry = sample_registry();
        registry.actions[0].fields[0].values[0].bytes = vec![0x00, 0x01]; // declared byte_len is 1
        let err = registry.validate().unwrap_err();
        match err {
            RegistryError::FieldLengthMismatch { expected, actual, .. } => {
                assert_eq!(expected, 1);
                assert_eq!(actual, 2);
            }
            other => panic!("expected FieldLengthMismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_hand_edited_mismatched_file_fails_to_load_with_a_named_error() {
        let dir = std::env::temp_dir().join(format!(
            "embarch-study-designer-registry-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
                + 2
        ));
        let mut registry = sample_registry();
        registry.actions[0].fields[0].values[0].bytes = vec![0x00, 0x01];
        // Bypass validate() to write a genuinely bad file, the way a human
        // hand-editing study-actions.toml could.
        std::fs::create_dir_all(dir.join("embarch")).unwrap();
        std::fs::write(registry_path(&dir), toml::to_string_pretty(&registry).unwrap()).unwrap();
        let err = ActionRegistry::load(&dir).unwrap_err();
        assert!(matches!(err, RegistryError::FieldLengthMismatch { .. }));
        std::fs::remove_dir_all(&dir).ok();
    }
}
