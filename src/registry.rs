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

use crate::decoder::{ScalarType, StructField, StructLayout};
use crate::ids::Uuid;
use crate::limits::{MAX_DECODER_NAME_LEN, MAX_STRUCT_FIELDS, MAX_STRUCT_FIELD_NAME_LEN};

use heapless::String as HString;
use heapless::Vec as HVec;

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
    /// A `study-structs.toml` field declares a scalar type this crate has no
    /// spelling for — named rather than defaulted to a plausible width
    /// (design.md §3 decision 52).
    UnknownScalarType { layout_name: String, field_name: String, declared: String },
    /// A tap references a layout no `study-structs.toml` defines. Caught at
    /// authoring time, where the author can fix it, rather than at render
    /// time, where it is a study that ran and produced no CSV.
    UnknownStructLayout { name: String },
    /// A name or field list that doesn't fit this crate's wire bounds
    /// ([`crate::limits`]). Explicit rather than truncating: a truncated
    /// column header renders a CSV whose columns don't say what they hold.
    StructLayoutTooLarge { layout_name: String, what: &'static str, max: usize },
    /// Two `[[struct]]` entries share a name, so a tap referencing it would
    /// resolve to whichever happened to come first.
    DuplicateStructLayout { name: String },
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
            RegistryError::UnknownScalarType { layout_name, field_name, declared } => write!(
                f,
                "struct '{layout_name}' field '{field_name}' declares type '{declared}', which is \
                 not one of u8/i8/u16le/u16be/i16le/i16be/u32le/u32be/i32le/i32be/u64le/u64be/\
                 i64le/i64be/f32le/f32be/f64le/f64be"
            ),
            RegistryError::UnknownStructLayout { name } => {
                write!(f, "no struct named '{name}' in study-structs.toml")
            }
            RegistryError::StructLayoutTooLarge { layout_name, what, max } => {
                write!(f, "struct '{layout_name}': {what} exceeds the wire limit of {max}")
            }
            RegistryError::DuplicateStructLayout { name } => write!(
                f,
                "two structs are both named '{name}'; a tap referencing it could resolve to \
                 either"
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

/// `<firmware-repo>/embarch/study-structs.toml` — sibling to
/// `study-actions.toml` and to `embarch.toml`, for the same reason: it is
/// engineer-authored knowledge about *this* DUT, so it travels with the
/// firmware repo and is shared across engineers exactly as those already are.
pub fn struct_registry_path(firmware_repo_root: &Path) -> PathBuf {
    firmware_repo_root.join("embarch").join("study-structs.toml")
}

/// One `[[struct]]` entry as the TOML file spells it — design.md §3
/// decision 52.
///
/// Deliberately a plain-`String` mirror of [`crate::decoder::StructLayout`]
/// rather than that type deserialized directly. A hand-edited TOML file's
/// mistakes — a name one character too long, a type spelled `u24le` — become
/// a named [`RegistryError`] here; deserializing the bounded wire type
/// straight would surface them as a `toml` parse error pointing at a
/// `heapless::String` capacity, which tells an engineer nothing about what
/// to fix. The same reason `ActionFieldValue` stores literal bytes rather
/// than a number this module would have to encode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructDef {
    pub name: String,
    /// Read once at offset 0.
    #[serde(default)]
    pub header: Vec<StructFieldDef>,
    /// Read repeatedly across whatever follows the header, producing one CSV
    /// row per repetition. Absent means "no repeating part".
    #[serde(default)]
    pub repeat: Vec<StructFieldDef>,
}

/// One named scalar in a [`StructDef`]. `ty` is the spelling
/// [`ScalarType::as_str`] produces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructFieldDef {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

/// Every payload layout one firmware repo has declared — design.md §3
/// decision 52.
///
/// **This never says what a characteristic is *for*.** It says how wide its
/// fields are and what order the bytes come in, under names the engineer
/// chose — the same line [`ActionRegistry`] draws, applied to the read
/// direction. Nothing here or anywhere else in this crate infers a layout
/// from observed bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructRegistry {
    #[serde(default, rename = "struct")]
    pub structs: Vec<StructDef>,
}

impl StructRegistry {
    /// Loads `<firmware_repo_root>/embarch/study-structs.toml`. A missing
    /// file is an empty registry, not an error — same reasoning as
    /// [`ActionRegistry::load`]'s.
    pub fn load(firmware_repo_root: &Path) -> Result<StructRegistry, RegistryError> {
        let path = struct_registry_path(firmware_repo_root);
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(StructRegistry::default())
            }
            Err(e) => return Err(RegistryError::Io(e)),
        };
        let registry: StructRegistry = toml::from_str(&raw).map_err(RegistryError::Parse)?;
        registry.validate()?;
        Ok(registry)
    }

    /// Writes the registry back, creating `embarch/` if needed.
    pub fn save(&self, firmware_repo_root: &Path) -> Result<(), RegistryError> {
        self.validate()?;
        let path = struct_registry_path(firmware_repo_root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(RegistryError::Io)?;
        }
        let raw = toml::to_string_pretty(self).map_err(RegistryError::Serialize)?;
        fs::write(&path, raw).map_err(RegistryError::Io)
    }

    /// Confirms every entry resolves and no two share a name. Pure/offline,
    /// same posture as [`ActionRegistry::validate`].
    pub fn validate(&self) -> Result<(), RegistryError> {
        for (index, def) in self.structs.iter().enumerate() {
            if self.structs[..index].iter().any(|earlier| earlier.name == def.name) {
                return Err(RegistryError::DuplicateStructLayout { name: def.name.clone() });
            }
            def.to_layout()?;
        }
        Ok(())
    }

    /// The resolved layout named `name`, ready to be placed in
    /// `Study.decoders`.
    pub fn resolve(&self, name: &str) -> Result<StructLayout, RegistryError> {
        self.structs
            .iter()
            .find(|d| d.name == name)
            .ok_or_else(|| RegistryError::UnknownStructLayout { name: name.to_string() })?
            .to_layout()
    }
}

impl StructDef {
    /// Converts this hand-editable entry into the bounded wire type,
    /// naming every way it can fail to fit.
    pub fn to_layout(&self) -> Result<StructLayout, RegistryError> {
        let name = HString::try_from(self.name.as_str()).map_err(|_| {
            RegistryError::StructLayoutTooLarge {
                layout_name: self.name.clone(),
                what: "name",
                max: MAX_DECODER_NAME_LEN,
            }
        })?;
        Ok(StructLayout {
            name,
            header: self.group(&self.header, "header")?,
            repeat: self.group(&self.repeat, "repeat")?,
        })
    }

    fn group(
        &self,
        fields: &[StructFieldDef],
        what: &'static str,
    ) -> Result<HVec<StructField, MAX_STRUCT_FIELDS>, RegistryError> {
        let mut out: HVec<StructField, MAX_STRUCT_FIELDS> = HVec::new();
        for field in fields {
            let ty = ScalarType::parse(&field.ty).ok_or_else(|| {
                RegistryError::UnknownScalarType {
                    layout_name: self.name.clone(),
                    field_name: field.name.clone(),
                    declared: field.ty.clone(),
                }
            })?;
            let name = HString::try_from(field.name.as_str()).map_err(|_| {
                RegistryError::StructLayoutTooLarge {
                    layout_name: self.name.clone(),
                    what: "a field name",
                    max: MAX_STRUCT_FIELD_NAME_LEN,
                }
            })?;
            out.push(StructField { name, ty }).map_err(|_| {
                RegistryError::StructLayoutTooLarge {
                    layout_name: self.name.clone(),
                    what,
                    max: MAX_STRUCT_FIELDS,
                }
            })?;
        }
        Ok(out)
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

#[cfg(test)]
mod struct_registry_tests {
    use super::*;

    const SAMPLE: &str = r#"
[[struct]]
name = "ppg_packet"
header = [
    { name = "seq", type = "u16le" },
    { name = "timestamp", type = "u32le" },
]
repeat = [
    { name = "green", type = "i32le" },
    { name = "red", type = "i32le" },
]

[[struct]]
name = "battery"
header = [{ name = "percent", type = "u8" }]
"#;

    #[test]
    fn a_hand_written_file_resolves_into_the_wire_type() {
        let registry: StructRegistry = toml::from_str(SAMPLE).unwrap();
        registry.validate().unwrap();
        let ppg = registry.resolve("ppg_packet").unwrap();
        assert_eq!(ppg.name.as_str(), "ppg_packet");
        assert_eq!(ppg.header_width(), 6);
        assert_eq!(ppg.repeat_width(), 8);
        assert_eq!(
            ppg.column_header().unwrap().as_str(),
            "rep_index,seq,timestamp,green,red"
        );
        let battery = registry.resolve("battery").unwrap();
        assert_eq!(battery.repeat_width(), 0);
        assert_eq!(battery.row_count(&[42]).unwrap(), 1);
    }

    #[test]
    fn round_trips_through_toml() {
        let registry: StructRegistry = toml::from_str(SAMPLE).unwrap();
        let raw = toml::to_string_pretty(&registry).unwrap();
        let parsed: StructRegistry = toml::from_str(&raw).unwrap();
        assert_eq!(registry, parsed);
    }

    #[test]
    fn a_mistyped_scalar_is_named_rather_than_defaulted_to_a_plausible_width() {
        // The file is hand-edited. Silently reading `u24le` as some nearby
        // width would render a CSV full of plausible, wrong numbers — the
        // exact failure this crate keeps refusing to produce.
        let raw = r#"
[[struct]]
name = "t"
header = [{ name = "v", type = "u24le" }]
"#;
        let registry: StructRegistry = toml::from_str(raw).unwrap();
        match registry.validate() {
            Err(RegistryError::UnknownScalarType { layout_name, field_name, declared }) => {
                assert_eq!(layout_name, "t");
                assert_eq!(field_name, "v");
                assert_eq!(declared, "u24le");
            }
            other => panic!("expected an UnknownScalarType, got {other:?}"),
        }
    }

    #[test]
    fn two_structs_with_one_name_are_refused() {
        let raw = r#"
[[struct]]
name = "t"
header = [{ name = "v", type = "u8" }]

[[struct]]
name = "t"
header = [{ name = "w", type = "u8" }]
"#;
        let registry: StructRegistry = toml::from_str(raw).unwrap();
        assert!(matches!(
            registry.validate(),
            Err(RegistryError::DuplicateStructLayout { .. })
        ));
    }

    #[test]
    fn a_tap_naming_a_layout_that_is_not_there_is_named() {
        let registry: StructRegistry = toml::from_str(SAMPLE).unwrap();
        match registry.resolve("ecg_packet") {
            Err(RegistryError::UnknownStructLayout { name }) => assert_eq!(name, "ecg_packet"),
            other => panic!("expected UnknownStructLayout, got {other:?}"),
        }
    }

    #[test]
    fn a_name_or_field_list_past_the_wire_bounds_is_named_not_truncated() {
        // A truncated column header renders a CSV whose columns don't say
        // what they hold, which is worse than refusing to build the study.
        let long = "x".repeat(MAX_DECODER_NAME_LEN + 1);
        let def = StructDef { name: long, header: Vec::new(), repeat: Vec::new() };
        assert!(matches!(
            def.to_layout(),
            Err(RegistryError::StructLayoutTooLarge { what: "name", .. })
        ));

        let too_many = StructDef {
            name: "t".to_string(),
            header: (0..MAX_STRUCT_FIELDS + 1)
                .map(|i| StructFieldDef { name: std::format!("f{i}"), ty: "u8".to_string() })
                .collect(),
            repeat: Vec::new(),
        };
        assert!(matches!(
            too_many.to_layout(),
            Err(RegistryError::StructLayoutTooLarge { what: "header", .. })
        ));
    }

    #[test]
    fn load_of_a_missing_file_is_an_empty_registry_not_an_error() {
        let dir = std::env::temp_dir().join(std::format!(
            "embarch-study-designer-structs-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert_eq!(StructRegistry::load(&dir).unwrap(), StructRegistry::default());
    }

    #[test]
    fn save_then_load_round_trips_through_the_real_path() {
        let dir = std::env::temp_dir().join(std::format!(
            "embarch-study-designer-structs-rt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let registry: StructRegistry = toml::from_str(SAMPLE).unwrap();
        registry.save(&dir).unwrap();
        assert!(struct_registry_path(&dir).exists());
        assert_eq!(StructRegistry::load(&dir).unwrap(), registry);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
