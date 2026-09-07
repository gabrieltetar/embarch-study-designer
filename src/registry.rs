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
use crate::limits::{
    MAX_DECODER_NAME_LEN, MAX_PAYLOAD_LEN, MAX_STRUCT_FIELDS, MAX_STRUCT_FIELD_NAME_LEN,
};

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
///
/// `byte_offset + byte_len` must land inside [`MAX_PAYLOAD_LEN`]; checked by
/// [`ActionRegistry::validate`] alongside the value lengths, since the widest
/// field is what sizes the payload buffer `study_builder` allocates.
///
/// **Two fields of one action must cover disjoint byte ranges**, also checked
/// by [`ActionRegistry::validate`]. Nothing structural stops two ranges from
/// meeting, and `study_builder` writes each chosen value in declaration order,
/// so an overlap loses at least one of the engineer's picks with the UI still
/// showing both as honoured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionField {
    pub name: String,
    pub byte_offset: usize,
    pub byte_len: usize,
    pub values: Vec<ActionFieldValue>,
}

/// One engineer-registered action against a specific, already-detected
/// characteristic. `fields` is only meaningful for `operation: Write`, and
/// [`ActionRegistry::validate`] **refuses** a non-`Write` action that carries
/// any — the rule used to be prose here and nothing enforced it, so a read
/// with fields offered choices that could never be sent.
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
    /// Two `[[actions]]` entries share a name, so a row referencing it would
    /// resolve to whichever happened to come first
    /// (`study_builder.rs`'s `.find(|a| &a.name == name)`), silently
    /// building the wrong payload. The [`ActionRegistry`] half of
    /// [`RegistryError::DuplicateStructLayout`]: one hand-edit mistake, one
    /// refusal, whichever of this module's two registries it lands in.
    DuplicateRegisteredAction { name: String },
    /// A field claims a byte range ending past [`MAX_PAYLOAD_LEN`], so the
    /// payload it describes could not be sent even with every value in it
    /// the right length. Caught here rather than left to `study_builder`,
    /// where `byte_offset + byte_len` is what *sizes the buffer*: the offset
    /// is the one number in this file nothing else bounds, and a hand edit
    /// choosing how many bytes the host allocates is a different mistake
    /// from a value that is the wrong length.
    FieldRangeTooLong {
        action_name: String,
        field_name: String,
        /// `byte_offset + byte_len`, saturated at `usize::MAX` — an offset
        /// that close to the top has no honest sum to report and does not
        /// need one to be refused.
        end: usize,
        max: usize,
    },
    /// Two fields of one action cover at least one payload byte in common.
    /// `study_builder` writes each chosen value into
    /// `buffer[byte_offset..byte_offset + byte_len]` **in declaration order**,
    /// so the later field overwrites whatever the earlier one put in the
    /// shared bytes. This is worse than "the later declaration wins": where
    /// the two ranges overlap only *partly*, the earlier field's bytes end up
    /// a **splice** of both chosen values — its head from its own pick, its
    /// tail from the other's — **a byte string that appears in neither
    /// field's `values` and that the engineer therefore never registered at
    /// all**, let alone chose. (A total overlap is the milder case: the
    /// earlier pick is simply gone.) The UI shows both choices as honoured
    /// either way. Decision 35's duplicate-name rule applied to offsets
    /// instead of names: the same hand-edited file, the same "the row
    /// silently carries a payload nobody chose".
    FieldRangesOverlap {
        action_name: String,
        /// The earlier-declared of the two — the one whose bytes lose.
        first_field: String,
        second_field: String,
        /// The shared range, half-open: `[overlap_start, overlap_end)`.
        overlap_start: usize,
        overlap_end: usize,
    },
    /// A non-`Write` action carries fields. `fields` describes a write
    /// payload and no other operation sends one, so `study_builder` ignores
    /// them entirely — the registry advertises choices that can never leave
    /// the host, and the only feedback is a `NotWritable` at build time
    /// blaming the *row* for choosing what the *registry* offered it.
    FieldsOnNonWriteAction {
        action_name: String,
        field_name: String,
        operation: RegisteredOperation,
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

/// The operation's name as an error message should say it — the `serde`
/// spelling rather than the `Debug` one, so the message names the word the
/// engineer typed into `study-actions.toml`.
fn operation_word(operation: RegisteredOperation) -> &'static str {
    match operation {
        RegisteredOperation::Read => "read",
        RegisteredOperation::Write => "write",
        RegisteredOperation::Subscribe => "subscribe",
        RegisteredOperation::Notify => "notify",
        RegisteredOperation::Indicate => "indicate",
    }
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
            RegistryError::DuplicateRegisteredAction { name } => write!(
                f,
                "two actions are both named '{name}'; a row referencing it could resolve to \
                 either"
            ),
            RegistryError::FieldRangeTooLong { action_name, field_name, end, max } => write!(
                f,
                "action '{action_name}' field '{field_name}': its bytes end at offset {end}, \
                 past the {max}-byte payload limit"
            ),
            RegistryError::FieldRangesOverlap {
                action_name,
                first_field,
                second_field,
                overlap_start,
                overlap_end,
            } => write!(
                f,
                "action '{action_name}': fields '{first_field}' and '{second_field}' both cover \
                 bytes {overlap_start}..{overlap_end}; '{second_field}' is written second and \
                 overwrites them, so '{first_field}'s chosen value is not in the payload — and \
                 where the ranges only partly overlap, what is there instead is a splice of \
                 both that nobody registered"
            ),
            RegistryError::FieldsOnNonWriteAction { action_name, field_name, operation } => {
                let op = operation_word(*operation);
                write!(
                    f,
                    "action '{action_name}' is a {op} but declares field '{field_name}'; fields \
                     describe a write payload, and a {op} sends none — those choices could never \
                     leave the host"
                )
            }
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

    /// Confirms no two actions share a name, that only a `Write` action
    /// carries fields, that every field's byte range ends inside
    /// [`MAX_PAYLOAD_LEN`], that no two fields of one action cover the same
    /// byte, and that every field's every value has exactly `byte_len`
    /// bytes. Pure/offline — no I/O, callable independent of `load`/`save`;
    /// called by both, so a file this refuses can be neither read nor
    /// written.
    ///
    /// **This is the whole gate on registry shape.** `study_builder` re-checks
    /// exactly one of these rules, the `MAX_PAYLOAD_LEN` bound, and only
    /// because that number sizes an allocation it makes before any of this has
    /// necessarily run; it re-checks neither the duplicate-name rule nor the
    /// overlap one. An `ActionRegistry` assembled in memory and never passed
    /// through here can still build a study whose payload is wrong.
    pub fn validate(&self) -> Result<(), RegistryError> {
        for (index, action) in self.actions.iter().enumerate() {
            if self.actions[..index].iter().any(|earlier| earlier.name == action.name) {
                return Err(RegistryError::DuplicateRegisteredAction {
                    name: action.name.clone(),
                });
            }
            // Before anything about the ranges: a read carrying fields is one
            // mistake to name, not a range mistake inside a field that was
            // never going to be sent. `match` rather than `if` + `if let`
            // because this crate is edition 2021 and has no let-chains.
            let stray_field = match action.operation {
                RegisteredOperation::Write => None,
                _ => action.fields.first(),
            };
            if let Some(field) = stray_field {
                return Err(RegistryError::FieldsOnNonWriteAction {
                    action_name: action.name.clone(),
                    field_name: field.name.clone(),
                    operation: action.operation,
                });
            }
            for field in &action.fields {
                // Saturating, for the reason on the variant: `+` here would
                // panic in debug and wrap to a passing range in release on
                // an offset near `usize::MAX`, and this file is hand-edited.
                let end = field.byte_offset.saturating_add(field.byte_len);
                if end > MAX_PAYLOAD_LEN {
                    return Err(RegistryError::FieldRangeTooLong {
                        action_name: action.name.clone(),
                        field_name: field.name.clone(),
                        end,
                        max: MAX_PAYLOAD_LEN,
                    });
                }
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
            // Pairwise, after the bound check above, so a field reaching past
            // the payload is reported as that rather than as an overlap with
            // whatever it happens to run into. Every `end` here is therefore
            // already <= MAX_PAYLOAD_LEN; `saturating_add` regardless, since
            // this crate's stated invariant is that addition saturates and a
            // reader should not have to re-derive the earlier return to see
            // that it does. O(n^2) over one action's fields, like the
            // duplicate-name scan above it over actions.
            for (index, field) in action.fields.iter().enumerate() {
                let end = field.byte_offset.saturating_add(field.byte_len);
                for earlier in &action.fields[..index] {
                    let earlier_end = earlier.byte_offset.saturating_add(earlier.byte_len);
                    let start = field.byte_offset.max(earlier.byte_offset);
                    let stop = end.min(earlier_end);
                    // Strict: half-open ranges that merely *meet* (0..2 and
                    // 2..3) share no byte, and a zero-length field covers
                    // none at all.
                    if start < stop {
                        return Err(RegistryError::FieldRangesOverlap {
                            action_name: action.name.clone(),
                            first_field: earlier.name.clone(),
                            second_field: field.name.clone(),
                            overlap_start: start,
                            overlap_end: stop,
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

    #[test]
    fn two_actions_with_one_name_are_refused_on_load_and_on_save() {
        // The sibling of `two_structs_with_one_name_are_refused`: the same
        // hand-edit mistake, in the other registry this module holds.
        // `study_builder.rs` resolves a row's action by the first `name`
        // match, so the second of two is unreachable and the row builds a
        // payload the author never chose.
        let dir = std::env::temp_dir().join(std::format!(
            "embarch-study-designer-registry-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
                + 3
        ));
        let mut registry = sample_registry();
        let duplicate = registry.actions[0].clone();
        registry.actions.push(duplicate);

        match registry.validate() {
            Err(RegistryError::DuplicateRegisteredAction { name }) => {
                assert_eq!(name, "example_write");
            }
            other => panic!("expected DuplicateRegisteredAction, got {other:?}"),
        }

        // save() refuses too: a file that could be written and not read back
        // is worse than one rejected on the way in.
        assert!(matches!(
            registry.save(&dir),
            Err(RegistryError::DuplicateRegisteredAction { .. })
        ));
        assert!(!registry_path(&dir).exists());

        // And a file hand-edited past that refusal fails to load, with the
        // same named error rather than a silently-shadowed second action.
        std::fs::create_dir_all(dir.join("embarch")).unwrap();
        std::fs::write(registry_path(&dir), toml::to_string_pretty(&registry).unwrap()).unwrap();
        assert!(matches!(
            ActionRegistry::load(&dir),
            Err(RegistryError::DuplicateRegisteredAction { .. })
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn one_action_per_name_still_loads() {
        let dir = std::env::temp_dir().join(std::format!(
            "embarch-study-designer-registry-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
                + 4
        ));
        let mut registry = sample_registry();
        let mut second = registry.actions[0].clone();
        second.name = "example_write_2".to_string();
        registry.actions.push(second);
        registry.save(&dir).unwrap();
        assert_eq!(ActionRegistry::load(&dir).unwrap(), registry);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_hand_written_field_reaching_past_the_payload_limit_is_refused_at_load() {
        // The point of the test is *where* this fires. A registry is read
        // long before any study is built from it, and `byte_offset` is what
        // sizes the buffer the builder allocates — so a file this large has
        // to be refused on the way in, not on the way to a study.
        let dir = std::env::temp_dir().join(format!(
            "embarch-study-designer-registry-range-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("embarch")).unwrap();
        let uuid_literal = |byte: u8| {
            let items: Vec<String> = (0..16).map(|_| byte.to_string()).collect();
            format!("[{}]", items.join(", "))
        };
        let raw = format!(
            r#"
[[actions]]
name = "far_field"
service_uuid = {service}
uuid = {characteristic}
operation = "write"

[[actions.fields]]
name = "flag"
byte_offset = {offset}
byte_len = 1
values = [{{ label = "On", bytes = [1] }}]
"#,
            service = uuid_literal(0xAA),
            characteristic = uuid_literal(0xAB),
            offset = MAX_PAYLOAD_LEN,
        );
        std::fs::write(registry_path(&dir), raw).unwrap();

        match ActionRegistry::load(&dir) {
            Err(RegistryError::FieldRangeTooLong { action_name, field_name, end, max }) => {
                assert_eq!(action_name, "far_field");
                assert_eq!(field_name, "flag");
                assert_eq!(end, MAX_PAYLOAD_LEN + 1);
                assert_eq!(max, MAX_PAYLOAD_LEN);
            }
            other => panic!("expected a FieldRangeTooLong, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_offset_that_would_overflow_is_refused_rather_than_wrapping() {
        // `usize::MAX` as an offset makes `byte_offset + byte_len` panic in
        // debug and wrap to a length of 0 in release — a range that passes
        // every check and then *panics* on the slice index that follows,
        // because the allocation succeeds at length 0 and slice bounds checks
        // are never elided in either profile. There is no out-of-bounds write
        // available here; the defect is a crash on a hand edit, and the
        // un-overflowing case (a 4 GB offset, which passes and then sizes a
        // 4 GB allocation) is the larger hazard. Saturating is why this is a
        // named refusal on both profiles instead of either.
        let registry = ActionRegistry {
            actions: vec![RegisteredAction {
                name: "wrapper".to_string(),
                service_uuid: Uuid([0xAA; 16]),
                uuid: Uuid([0xAB; 16]),
                operation: RegisteredOperation::Write,
                fields: vec![ActionField {
                    name: "flag".to_string(),
                    byte_offset: usize::MAX,
                    byte_len: 1,
                    values: vec![ActionFieldValue { label: "On".to_string(), bytes: vec![1] }],
                }],
            }],
        };
        match registry.validate() {
            Err(RegistryError::FieldRangeTooLong { end, .. }) => assert_eq!(end, usize::MAX),
            other => panic!("expected a FieldRangeTooLong, got {other:?}"),
        }
    }

    #[test]
    fn a_field_ending_exactly_on_the_payload_limit_is_accepted() {
        // The bound is the end of the range, not the start of it: a field
        // whose last byte is the payload's last byte fits.
        let mut registry = sample_registry();
        registry.actions[0].fields[0].byte_offset = MAX_PAYLOAD_LEN - 1;
        registry.validate().unwrap();
    }

    /// A scratch firmware-repo root, unique per test.
    fn scratch_repo(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "embarch-study-designer-registry-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("embarch")).unwrap();
        dir
    }

    fn uuid_array(byte: u8) -> String {
        let items: Vec<String> = (0..16).map(|_| byte.to_string()).collect();
        format!("[{}]", items.join(", "))
    }

    #[test]
    fn two_fields_covering_one_byte_are_refused_at_load() {
        // Hand-written, because this is a hand-edit mistake: `header` claims
        // bytes 1..3 and `mode` claims 2..4, so they share byte 2 and nothing
        // in the file says so. The overlap is *partial* on purpose — that is
        // the case where the payload ends up holding a byte string nobody
        // registered; `study_builder`'s own
        // `an_overlapping_registry_the_builder_never_validated_splices_two_values`
        // is where that is demonstrated rather than asserted.
        let dir = scratch_repo("overlap");
        let raw = format!(
            r#"
[[actions]]
name = "set_mode"
service_uuid = {service}
uuid = {characteristic}
operation = "write"

[[actions.fields]]
name = "header"
byte_offset = 1
byte_len = 2
values = [{{ label = "V1", bytes = [0xA1, 0xA2] }}]

[[actions.fields]]
name = "mode"
byte_offset = 2
byte_len = 2
values = [{{ label = "On", bytes = [0xB1, 0xB2] }}]
"#,
            service = uuid_array(0xAA),
            characteristic = uuid_array(0xAB),
        );
        std::fs::write(registry_path(&dir), raw).unwrap();

        let err = match ActionRegistry::load(&dir) {
            Err(e) => e,
            Ok(other) => panic!("expected a FieldRangesOverlap, loaded {other:?}"),
        };
        match &err {
            RegistryError::FieldRangesOverlap {
                action_name,
                first_field,
                second_field,
                overlap_start,
                overlap_end,
            } => {
                assert_eq!(action_name, "set_mode");
                // Declaration order, not alphabetical: the first name is the
                // field whose bytes lose, which is the half a reader needs.
                assert_eq!(first_field, "header");
                assert_eq!(second_field, "mode");
                assert_eq!((*overlap_start, *overlap_end), (2, 3));
            }
            other => panic!("expected a FieldRangesOverlap, got {other:?}"),
        }
        // Asserted rather than eyeballed: the understated version of this
        // message ("the later declaration wins") is wrong for exactly the
        // case in this test, so the wording is part of the fix.
        assert_eq!(
            err.to_string(),
            "action 'set_mode': fields 'header' and 'mode' both cover bytes 2..3; 'mode' is \
             written second and overwrites them, so 'header's chosen value is not in the \
             payload — and where the ranges only partly overlap, what is there instead is a \
             splice of both that nobody registered"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fields_that_only_meet_at_a_boundary_are_accepted() {
        // The ranges are half-open, so 0..2 and 2..3 are adjacent and share
        // nothing. This is the ordinary shape of a multi-field payload and
        // the check must not refuse it.
        let mut registry = sample_registry();
        registry.actions[0].fields[0].byte_len = 2;
        registry.actions[0].fields[0].values = vec![
            ActionFieldValue { label: "Off".to_string(), bytes: vec![0x00, 0x00] },
            ActionFieldValue { label: "On".to_string(), bytes: vec![0x00, 0x01] },
        ];
        registry.actions[0].fields.push(ActionField {
            name: "flags".to_string(),
            byte_offset: 2,
            byte_len: 1,
            values: vec![ActionFieldValue { label: "None".to_string(), bytes: vec![0x00] }],
        });
        registry.validate().unwrap();
    }

    #[test]
    fn a_zero_length_field_covers_no_byte_and_so_overlaps_nothing() {
        // `byte_len = 0` is a degenerate but representable hand edit. It
        // covers no byte, so it cannot collide with one — the check is `<`,
        // not `<=`, and this is what pins that.
        let mut registry = sample_registry();
        registry.actions[0].fields.push(ActionField {
            name: "nothing".to_string(),
            byte_offset: 0,
            byte_len: 0,
            values: vec![ActionFieldValue { label: "Empty".to_string(), bytes: Vec::new() }],
        });
        registry.validate().unwrap();
    }

    #[test]
    fn a_read_action_carrying_fields_is_refused_at_load() {
        // Documented as meaningless since decision 35 and enforced by nothing
        // until now: the file offered a choice the builder discards, and the
        // engineer's only signal was a `NotWritable` at build time blaming
        // the row for picking what the registry had offered it.
        let dir = scratch_repo("read-fields");
        let raw = format!(
            r#"
[[actions]]
name = "read_status"
service_uuid = {service}
uuid = {characteristic}
operation = "read"

[[actions.fields]]
name = "mode"
byte_offset = 0
byte_len = 1
values = [{{ label = "On", bytes = [1] }}]
"#,
            service = uuid_array(0xAA),
            characteristic = uuid_array(0xAB),
        );
        std::fs::write(registry_path(&dir), raw).unwrap();

        let err = match ActionRegistry::load(&dir) {
            Err(e) => e,
            Ok(other) => panic!("expected a FieldsOnNonWriteAction, loaded {other:?}"),
        };
        match &err {
            RegistryError::FieldsOnNonWriteAction { action_name, field_name, operation } => {
                assert_eq!(action_name, "read_status");
                assert_eq!(field_name, "mode");
                assert_eq!(*operation, RegisteredOperation::Read);
            }
            other => panic!("expected a FieldsOnNonWriteAction, got {other:?}"),
        }
        // The message names the operation with the word the file spells, not
        // the `Debug` capitalisation, so it points at the line to edit.
        assert_eq!(
            err.to_string(),
            "action 'read_status' is a read but declares field 'mode'; fields describe a write \
             payload, and a read sends none — those choices could never leave the host"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_non_write_operation_refuses_a_field_and_write_keeps_them() {
        // The rule is "only a write has a payload", not "reads are special":
        // subscribe, notify and indicate send no payload either, and each has
        // its own line in the TOML an engineer might attach fields to.
        for operation in [
            RegisteredOperation::Read,
            RegisteredOperation::Subscribe,
            RegisteredOperation::Notify,
            RegisteredOperation::Indicate,
        ] {
            let mut registry = sample_registry();
            registry.actions[0].operation = operation;
            match registry.validate() {
                Err(RegistryError::FieldsOnNonWriteAction { operation: got, .. }) => {
                    assert_eq!(got, operation);
                }
                other => panic!("expected a FieldsOnNonWriteAction for {operation:?}, got {other:?}"),
            }
            // ...and the same action with no fields is fine, so what is being
            // refused is the fields and not the operation.
            registry.actions[0].fields.clear();
            registry.validate().unwrap();
        }
        assert!(sample_registry().validate().is_ok());
    }

    #[test]
    fn the_two_duplicate_name_messages_are_the_same_shape() {
        // The symmetry is the product here, not just the check: an engineer
        // who has read one of these messages has read the other.
        let action = RegistryError::DuplicateRegisteredAction { name: "t".to_string() };
        let layout = RegistryError::DuplicateStructLayout { name: "t".to_string() };
        assert_eq!(
            action.to_string(),
            "two actions are both named 't'; a row referencing it could resolve to either"
        );
        assert_eq!(
            layout.to_string(),
            "two structs are both named 't'; a tap referencing it could resolve to either"
        );
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
