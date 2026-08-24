//! Static GATT-config extraction — design.md §3 decision 33.
//!
//! Answers "what UUIDs does this DUT firmware actually expose" from source,
//! ahead of ever connecting to real hardware — distinct from the live
//! `Action::GattDiscover`/`Action::GattMonitorAll` (§3 decisions 31/32),
//! which answer the same question over an actual BLE connection. `std`-only
//! (needs filesystem access via `std::path::Path`, and `regex` for the
//! text-scan) — gated behind the `gatt-extract` feature, never linked by
//! dev-bench firmware or embarch-core/embarch-api's plain Cargo-dependency
//! use (design.md §2's scope note).
//!
//! Text-scan, not a full C parser (embarch-study-designer/milestone-9.md
//! §3.6): this deliberately doesn't evaluate preprocessor conditionals (e.g.
//! `IF_ENABLED(CONFIG_AIR_TEMP_ENABLE, (...))` in `reference-dut-fw`'s
//! `ble_def.h`) — a characteristic wrapped in one is reported regardless of
//! whether that Kconfig option is actually set for a given build. Fails
//! loudly (a named `ExtractError` variant) on an unrecognized identifier or
//! `BT_GATT_CHRC_*` token rather than silently under-extracting — the
//! defensive posture milestone-9.md §5 calls for.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use regex::Regex;

use crate::gatt::{GattCharacteristicInfo, GattServiceInfo};
use crate::ids::Uuid;
use crate::limits::MAX_DISCOVERED_SERVICES;

/// Names the specific failure rather than surfacing a raw I/O/parse error
/// (design.md §3 decisions 18/23's "name the specific failure" discipline,
/// applied here per milestone-9.md §3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractError {
    /// A required source file didn't exist or couldn't be read.
    FileNotFound(PathBuf),
    /// A `BT_UUID_INIT_128(...)`/`BT_GATT_PRIMARY_SERVICE(...)`/
    /// `BT_GATT_CHARACTERISTIC(...)` call referenced a macro, constant, or
    /// variable this scanner never found a definition for.
    MacroNotFound(String),
    /// A `BT_UUID_128_ENCODE(...)` invocation didn't have the expected
    /// 5-argument shape, or one of its arguments wasn't a parseable hex
    /// literal or known constant.
    UnparseableUuidEncode(String),
    /// A characteristic's properties expression contained a token that
    /// isn't one of the recognized `BT_GATT_CHRC_*` macros — reported rather
    /// than silently treated as contributing zero bits.
    UnparseableProperties(String),
    /// A `BT_GATT_SERVICE_DEFINE(...)`/`BT_GATT_CHARACTERISTIC(...)` call's
    /// parentheses never balanced within the source text.
    UnbalancedCall(&'static str),
    /// More services/characteristics were extracted than `limits` allows.
    CapacityExceeded(&'static str),
}

impl core::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ExtractError::FileNotFound(p) => write!(f, "file not found: {}", p.display()),
            ExtractError::MacroNotFound(name) => {
                write!(f, "macro, constant, or variable not found: {name}")
            }
            ExtractError::UnparseableUuidEncode(msg) => {
                write!(f, "unparseable BT_UUID_128_ENCODE invocation: {msg}")
            }
            ExtractError::UnparseableProperties(tok) => {
                write!(f, "unrecognized characteristic-properties token: {tok}")
            }
            ExtractError::UnbalancedCall(which) => write!(f, "unbalanced parentheses in {which}"),
            ExtractError::CapacityExceeded(which) => write!(f, "extracted data exceeds {which}"),
        }
    }
}

impl std::error::Error for ExtractError {}

/// Extracts a DUT firmware's GATT table from its source, ahead of ever
/// connecting to real hardware (design.md §3 decision 33). Generic at the
/// trait boundary, narrow at the implementation, per the user's explicit
/// call during Milestone 3's design pass — a second firmware project's own
/// extractor is a new `impl`, not a redesign of this trait or its output
/// shape.
pub trait GattConfigExtractor {
    /// `repo_root` is the checked-out firmware repo's root directory.
    /// Returns the same [`GattServiceInfo`] shape a live
    /// `Action::GattDiscover`/`Action::GattMonitorAll` result uses, so a
    /// static extraction and a live discovery are byte-for-byte comparable.
    fn extract(
        &self,
        repo_root: &Path,
    ) -> Result<heapless::Vec<GattServiceInfo, MAX_DISCOVERED_SERVICES>, ExtractError>;
}

/// Scoped narrowly to `reference-dut-fw`'s actual conventions
/// (`lib/ble/ble_def.h`'s `..._UUID_VAL` macros, `lib/ble/ble.c`'s
/// `BT_GATT_SERVICE_DEFINE`/`BT_GATT_PRIMARY_SERVICE`/
/// `BT_GATT_CHARACTERISTIC(&uuid, PROPS, ...)` calls) — confirmed against
/// that real source, not guessed against a generic Zephyr BLE peripheral
/// layout other projects might use differently (design.md §3 decision 33).
#[derive(Debug, Clone, Copy, Default)]
pub struct ZephyrBleDefExtractor;

impl GattConfigExtractor for ZephyrBleDefExtractor {
    fn extract(
        &self,
        repo_root: &Path,
    ) -> Result<heapless::Vec<GattServiceInfo, MAX_DISCOVERED_SERVICES>, ExtractError> {
        let def_path = repo_root.join("lib/ble/ble_def.h");
        let def_src = std::fs::read_to_string(&def_path)
            .map_err(|_| ExtractError::FileNotFound(def_path))?;
        let c_path = repo_root.join("lib/ble/ble.c");
        let c_src =
            std::fs::read_to_string(&c_path).map_err(|_| ExtractError::FileNotFound(c_path))?;

        extract_from_sources(&def_src, &c_src)
    }
}

/// The pure, file-I/O-free core of [`ZephyrBleDefExtractor::extract`] —
/// split out so this crate's own tests can exercise the text-scan against
/// literal fixture strings without touching the filesystem.
fn extract_from_sources(
    def_src: &str,
    c_src: &str,
) -> Result<heapless::Vec<GattServiceInfo, MAX_DISCOVERED_SERVICES>, ExtractError> {
    let consts = parse_scalar_constants(def_src);
    let uuid_macros = parse_uuid_macros(def_src, &consts)?;
    let var_uuids = parse_uuid_vars(c_src, &uuid_macros)?;
    parse_gatt_services(c_src, &var_uuids)
}

/// Simple `#define NAME 0x...` scalar constants (e.g. `ES_BASE`), used to
/// resolve `BT_UUID_128_ENCODE` arguments that reference a named constant
/// rather than a literal.
fn parse_scalar_constants(def_src: &str) -> HashMap<String, u64> {
    let flattened = join_backslash_continuations(def_src);
    let re = Regex::new(r"#define\s+(\w+)\s+0[xX]([0-9A-Fa-f]+)(?:[uUlL]*)\b").unwrap();
    let mut consts = HashMap::new();
    for caps in re.captures_iter(&flattened) {
        if let Ok(value) = u64::from_str_radix(&caps[2], 16) {
            consts.insert(caps[1].to_string(), value);
        }
    }
    consts
}

/// `#define <NAME>_UUID_VAL \` `BT_UUID_128_ENCODE(w32, w1, w2, w3, w48)`
/// macros → each resolved to 16 raw bytes, big-endian (design.md §4's
/// documented `Uuid` byte order — the same order the macro's own arguments
/// are written in, `w32-w1-w2-w3-w48`).
fn parse_uuid_macros(
    def_src: &str,
    consts: &HashMap<String, u64>,
) -> Result<HashMap<String, [u8; 16]>, ExtractError> {
    let flattened = join_backslash_continuations(def_src);
    let re =
        Regex::new(r"#define\s+(\w+_UUID_VAL)\s+BT_UUID_128_ENCODE\(([^)]*)\)").unwrap();
    let mut macros = HashMap::new();
    for caps in re.captures_iter(&flattened) {
        let name = caps[1].to_string();
        let args: Vec<&str> = caps[2].split(',').map(str::trim).collect();
        if args.len() != 5 {
            return Err(ExtractError::UnparseableUuidEncode(format!(
                "{name}: expected 5 BT_UUID_128_ENCODE arguments, found {}",
                args.len()
            )));
        }
        let w32 = parse_hex_or_const(args[0], consts, &name)?;
        let w1 = parse_hex_or_const(args[1], consts, &name)?;
        let w2 = parse_hex_or_const(args[2], consts, &name)?;
        let w3 = parse_hex_or_const(args[3], consts, &name)?;
        let w48 = parse_hex_or_const(args[4], consts, &name)?;

        let mut bytes = [0u8; 16];
        bytes[0..4].copy_from_slice(&(w32 as u32).to_be_bytes());
        bytes[4..6].copy_from_slice(&(w1 as u16).to_be_bytes());
        bytes[6..8].copy_from_slice(&(w2 as u16).to_be_bytes());
        bytes[8..10].copy_from_slice(&(w3 as u16).to_be_bytes());
        bytes[10..16].copy_from_slice(&w48.to_be_bytes()[2..8]);

        macros.insert(name, bytes);
    }
    Ok(macros)
}

fn parse_hex_or_const(
    token: &str,
    consts: &HashMap<String, u64>,
    macro_name: &str,
) -> Result<u64, ExtractError> {
    let token = token.trim();
    if let Some(hex) = token.strip_prefix("0x").or_else(|| token.strip_prefix("0X")) {
        let hex = hex.trim_end_matches(['u', 'U', 'l', 'L']);
        u64::from_str_radix(hex, 16).map_err(|_| {
            ExtractError::UnparseableUuidEncode(format!(
                "{macro_name}: bad hex literal {token:?}"
            ))
        })
    } else {
        consts
            .get(token)
            .copied()
            .ok_or_else(|| ExtractError::MacroNotFound(token.to_string()))
    }
}

/// `static struct bt_uuid_128 <var> = BT_UUID_INIT_128(<NAME>_UUID_VAL);` →
/// C variable name to that macro's already-resolved UUID bytes.
fn parse_uuid_vars(
    c_src: &str,
    uuid_macros: &HashMap<String, [u8; 16]>,
) -> Result<HashMap<String, [u8; 16]>, ExtractError> {
    let re = Regex::new(r"struct\s+bt_uuid_128\s+(\w+)\s*=\s*BT_UUID_INIT_128\((\w+)\)").unwrap();
    let mut vars = HashMap::new();
    for caps in re.captures_iter(c_src) {
        let var = caps[1].to_string();
        let macro_name = &caps[2];
        let bytes = uuid_macros
            .get(macro_name)
            .copied()
            .ok_or_else(|| ExtractError::MacroNotFound(macro_name.to_string()))?;
        vars.insert(var, bytes);
    }
    Ok(vars)
}

/// Every `BT_GATT_SERVICE_DEFINE(...)` block in `c_src`, each producing one
/// [`GattServiceInfo`] (design.md §4.3a) — services/characteristics in
/// source (== discovery) order, matching a live `GattDiscover`'s own
/// ordering convention.
fn parse_gatt_services(
    c_src: &str,
    var_uuids: &HashMap<String, [u8; 16]>,
) -> Result<heapless::Vec<GattServiceInfo, MAX_DISCOVERED_SERVICES>, ExtractError> {
    let primary_re = Regex::new(r"BT_GATT_PRIMARY_SERVICE\(\s*&(\w+)\s*\)").unwrap();
    let chrc_re =
        Regex::new(r"BT_GATT_CHARACTERISTIC\(\s*&(\w+)(?:\.uuid)?\s*,\s*([^,]+),").unwrap();

    let mut services: heapless::Vec<GattServiceInfo, MAX_DISCOVERED_SERVICES> = heapless::Vec::new();
    let marker = "BT_GATT_SERVICE_DEFINE(";
    let mut search_from = 0usize;
    while let Some(rel_idx) = c_src[search_from..].find(marker) {
        let paren_start = search_from + rel_idx + marker.len() - 1;
        let block = extract_balanced_call(c_src, paren_start)
            .ok_or(ExtractError::UnbalancedCall("BT_GATT_SERVICE_DEFINE"))?;
        search_from = paren_start + block.len();
        let inner = &block[1..block.len() - 1];

        let service_var = primary_re
            .captures(inner)
            .map(|c| c[1].to_string())
            .ok_or_else(|| ExtractError::MacroNotFound("BT_GATT_PRIMARY_SERVICE".to_string()))?;
        let service_uuid = *var_uuids
            .get(&service_var)
            .ok_or_else(|| ExtractError::MacroNotFound(service_var.clone()))?;

        let mut characteristics: heapless::Vec<GattCharacteristicInfo, { crate::limits::MAX_CHARS_PER_SERVICE }> =
            heapless::Vec::new();
        for caps in chrc_re.captures_iter(inner) {
            let char_var = caps[1].to_string();
            let props_expr = caps[2].trim();
            let uuid = *var_uuids
                .get(&char_var)
                .ok_or_else(|| ExtractError::MacroNotFound(char_var.clone()))?;

            let mut properties = 0u8;
            for token in props_expr.split('|') {
                let token = token.trim();
                properties |= chrc_property_bit(token)
                    .ok_or_else(|| ExtractError::UnparseableProperties(token.to_string()))?;
            }

            characteristics
                .push(GattCharacteristicInfo { uuid: Uuid(uuid), properties })
                .map_err(|_| ExtractError::CapacityExceeded("MAX_CHARS_PER_SERVICE"))?;
        }

        services
            .push(GattServiceInfo { uuid: Uuid(service_uuid), characteristics })
            .map_err(|_| ExtractError::CapacityExceeded("MAX_DISCOVERED_SERVICES"))?;
    }

    Ok(services)
}

/// Bluetooth Core Spec characteristic-properties bits — the same encoding
/// design.md §4.3a documents for the live `GattDiscover` path (bit 0 =
/// broadcast ... bit 7 = extended-properties). Returns `None` on an
/// unrecognized token so the caller can fail loudly rather than silently
/// contribute zero bits (milestone-9.md §5's defensive-posture call).
fn chrc_property_bit(token: &str) -> Option<u8> {
    match token {
        "BT_GATT_CHRC_BROADCAST" => Some(0x01),
        "BT_GATT_CHRC_READ" => Some(0x02),
        "BT_GATT_CHRC_WRITE_WITHOUT_RESP" => Some(0x04),
        "BT_GATT_CHRC_WRITE" => Some(0x08),
        "BT_GATT_CHRC_NOTIFY" => Some(0x10),
        "BT_GATT_CHRC_INDICATE" => Some(0x20),
        "BT_GATT_CHRC_AUTH" => Some(0x40),
        "BT_GATT_CHRC_EXT_PROP" => Some(0x80),
        _ => None,
    }
}

/// Returns the slice of `src` starting at `src[open_paren_idx] == '('`
/// through its matching close, inclusive of both parens — tracking nesting
/// depth so a call containing its own nested parenthesized arguments (as
/// every `BT_GATT_SERVICE_DEFINE` block does) is captured whole.
fn extract_balanced_call(src: &str, open_paren_idx: usize) -> Option<&str> {
    let bytes = src.as_bytes();
    if bytes.get(open_paren_idx) != Some(&b'(') {
        return None;
    }
    let mut depth = 0i32;
    let mut i = open_paren_idx;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&src[open_paren_idx..=i]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Joins a `#define`'s backslash-newline continuation lines into one
/// logical line, so the argument-capturing regexes above don't need to
/// reason about where a multi-line macro invocation happens to wrap.
fn join_backslash_continuations(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('\n') => {
                    chars.next();
                    out.push(' ');
                    continue;
                }
                Some('\r') => {
                    chars.next();
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    out.push(' ');
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal fixture mirroring reference-dut-fw's real
    // lib/ble/ble_def.h / lib/ble/ble.c conventions (confirmed against that
    // actual checkout, embarch-study-designer/milestone-9.md §3.6) — enough
    // to exercise every parsing stage without touching the filesystem or a
    // sibling repo this crate has no dependency on.
    const FIXTURE_DEF_H: &str = r#"
#define ES_BASE  0xE58100000000ULL

#define SDS_SERVICE_UUID_VAL \
    BT_UUID_128_ENCODE(0x00000001, 0x853F, 0x4A00, 0x8000, ES_BASE)
#define SDS_HRM_RRM_CHAR_UUID_VAL \
    BT_UUID_128_ENCODE(0x00000002, 0x853F, 0x4A00, 0x8000, ES_BASE)

#define DMS_SERVICE_UUID_VAL \
    BT_UUID_128_ENCODE(0x00000010, 0x853F, 0x4A00, 0x8000, ES_BASE)
#define DMS_SENSOR_CFG_UUID_VAL \
    BT_UUID_128_ENCODE(0x00000011, 0x853F, 0x4A00, 0x8000, ES_BASE)
"#;

    const FIXTURE_BLE_C: &str = r#"
static struct bt_uuid_128 sds_service_uuid      = BT_UUID_INIT_128(SDS_SERVICE_UUID_VAL);
static struct bt_uuid_128 sds_hrm_rrm_char_uuid = BT_UUID_INIT_128(SDS_HRM_RRM_CHAR_UUID_VAL);
static struct bt_uuid_128 dms_service_uuid      = BT_UUID_INIT_128(DMS_SERVICE_UUID_VAL);
static struct bt_uuid_128 dms_sensor_cfg_uuid   = BT_UUID_INIT_128(DMS_SENSOR_CFG_UUID_VAL);

BT_GATT_SERVICE_DEFINE(s11_sds,
    BT_GATT_PRIMARY_SERVICE(&sds_service_uuid),

    BT_GATT_CHARACTERISTIC(&sds_hrm_rrm_char_uuid.uuid,
                           BT_GATT_CHRC_NOTIFY,
                           BT_GATT_PERM_NONE,
                           NULL, NULL, NULL),
    BT_GATT_CCC(hrm_rrm_ccc_changed, BT_GATT_PERM_READ_ENCRYPT | BT_GATT_PERM_WRITE_ENCRYPT),
);

BT_GATT_SERVICE_DEFINE(s11_dms,
    BT_GATT_PRIMARY_SERVICE(&dms_service_uuid),

    BT_GATT_CHARACTERISTIC(&dms_sensor_cfg_uuid.uuid,
                           BT_GATT_CHRC_READ | BT_GATT_CHRC_WRITE | BT_GATT_CHRC_NOTIFY,
                           BT_GATT_PERM_READ_ENCRYPT | BT_GATT_PERM_WRITE_ENCRYPT,
                           sensor_cfg_read_handler, sensor_cfg_write_handler, NULL),
    BT_GATT_CCC(sensor_cfg_ccc_changed, BT_GATT_PERM_READ_ENCRYPT | BT_GATT_PERM_WRITE_ENCRYPT),
);
"#;

    #[test]
    fn extracts_expected_services_and_uuids_from_fixture() {
        let services = extract_from_sources(FIXTURE_DEF_H, FIXTURE_BLE_C).unwrap();
        assert_eq!(services.len(), 2);

        let sds = &services[0];
        assert_eq!(
            sds.uuid.0,
            [0x00, 0x00, 0x00, 0x01, 0x85, 0x3F, 0x4A, 0x00, 0x80, 0x00, 0xE5, 0x81, 0x00, 0x00, 0x00, 0x00]
        );
        assert_eq!(sds.characteristics.len(), 1);
        assert_eq!(sds.characteristics[0].properties, 0x10); // NOTIFY

        let dms = &services[1];
        assert_eq!(dms.characteristics.len(), 1);
        assert_eq!(dms.characteristics[0].properties, 0x02 | 0x08 | 0x10); // READ|WRITE|NOTIFY
    }

    #[test]
    fn missing_file_reports_file_not_found() {
        let extractor = ZephyrBleDefExtractor;
        let err = extractor.extract(Path::new("/nonexistent/repo/root")).unwrap_err();
        assert!(matches!(err, ExtractError::FileNotFound(_)));
    }

    #[test]
    fn unrecognized_property_token_fails_loudly() {
        let bad_c = FIXTURE_BLE_C.replace("BT_GATT_CHRC_NOTIFY,\n                           BT_GATT_PERM_NONE", "BT_GATT_CHRC_BOGUS,\n                           BT_GATT_PERM_NONE");
        let err = extract_from_sources(FIXTURE_DEF_H, &bad_c).unwrap_err();
        assert!(matches!(err, ExtractError::UnparseableProperties(_)));
    }

    #[test]
    fn unknown_macro_reference_is_reported() {
        let bad_def = FIXTURE_DEF_H.replace("ES_BASE", "UNKNOWN_BASE");
        // Rewriting every ES_BASE occurrence also rewrites the #define
        // itself into `#define UNKNOWN_BASE 0x...` — undo that one so the
        // lookup genuinely fails to resolve `UNKNOWN_BASE`.
        let bad_def = bad_def.replacen("#define UNKNOWN_BASE", "#define ES_BASE", 1);
        let err = extract_from_sources(&bad_def, FIXTURE_BLE_C).unwrap_err();
        assert!(matches!(err, ExtractError::MacroNotFound(_)));
    }
}
