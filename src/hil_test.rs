//! The HIL test, suite manifest and plan (`interfaces/hil-tests.md`, decision
//! 109): the files a Test run is defined by, the hash a confirmation binds, and
//! the diff a derived test shows against its premade original. Host-only, behind
//! the `hil-test` feature.
//!
//! JSON, UTF-8, `snake_case` keys, an integer `schema`, and **unknown keys
//! refused** (`deny_unknown_fields`), as every HIL file is
//! (`interfaces/hil-shapes.md`, "Conventions"). A test is a study plus verdict
//! rules: it names its study by file and hash and adds no step format.
//!
//! **The definition hash is the SHA-256 of the canonical JSON (RFC 8785) of the
//! file as this crate serialises it**, so key order and a defaulted field's
//! spelling in the source file do not change it, and a changed limit does.
//! These types check shape only; the definition-time checks (a bench with no
//! DUT, a limit narrower than its `U` allows, a study that does not hash to
//! `study.hash`) are a later unit's.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::verdicts::{DecisionRule, Limit, Source};

/// `schema` of a test, a suite manifest and a plan, each starting at 1.
pub const SCHEMA: u32 = 1;

fn schema_1() -> u32 {
    SCHEMA
}

fn one() -> u32 {
    1
}

// ---- a test --------------------------------------------------------------

/// A study, by file name and hash (SHA-256 of the file's bytes as written).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudyRef {
    pub file: String,
    pub hash: String,
}

/// The spec revisions a test applies to; `null` is open on that side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionRange {
    pub from: Option<String>,
    pub to: Option<String>,
}

/// The bench a test needs and the config of it, by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchRef {
    pub name: String,
    pub config: String,
}

/// The state a test starts from. The default is `reset`, and a file writes it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Precondition {
    Reset,
    PowerCycle,
    Detach,
    None,
}

/// A limit on one measurement. `expect` is an observation's choice (plan
/// choice 27): the measurement's string value must equal `value`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LimitSpec {
    Band { lo: f64, hi: f64 },
    AtMost { max: f64 },
    AtLeast { min: f64 },
    Expect { value: String },
}

impl LimitSpec {
    /// The verdict rules' numeric limit, or `None` for `expect`.
    pub fn numeric(&self) -> Option<Limit> {
        match *self {
            LimitSpec::Band { lo, hi } => Some(Limit::Band { lo, hi }),
            LimitSpec::AtMost { max } => Some(Limit::AtMost { max }),
            LimitSpec::AtLeast { min } => Some(Limit::AtLeast { min }),
            LimitSpec::Expect { .. } => None,
        }
    }
}

/// The decision rule a test states; [`DecisionRuleSpec::rule`] is the verdict
/// rules' own type, which does not refuse unknown keys itself.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DecisionRuleSpec {
    Simple,
    GuardBand { w_multiple: f64 },
}

impl DecisionRuleSpec {
    /// The verdict rules' decision rule.
    pub fn rule(self) -> DecisionRule {
        match self {
            DecisionRuleSpec::Simple => DecisionRule::Simple,
            DecisionRuleSpec::GuardBand { w_multiple } => DecisionRule::GuardBand { w_multiple },
        }
    }
}

/// One limit of a test, on the study's measurement `measurement`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestLimit {
    pub measurement: String,
    pub limit: LimitSpec,
}

/// What a test declares for a `dut` measurement it limits: only the test can
/// say them (`notes/hil/02-study.md`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DutSource {
    pub measurement: String,
    pub unit: String,
    pub range_min: f64,
    pub range_max: f64,
    pub resolution: f64,
    /// Expanded uncertainty, in `unit`.
    pub u: f64,
    /// Its coverage factor.
    pub k: f64,
}

impl DutSource {
    /// The acceptance-width rule's view of it.
    pub fn as_source(&self) -> Source {
        Source { range_min: self.range_min, range_max: self.range_max, resolution: self.resolution, u: self.u }
    }
}

/// A declared setup term (a cable's drop, a fixture's offset): an uncertainty
/// term and never a value, a person's act like an instrument's `U`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupTerm {
    pub name: String,
    pub unit: String,
    /// Expanded uncertainty, in `unit`.
    pub u: f64,
    pub k: f64,
}

/// A template test's capability: its study lines, parse patterns and limits may
/// hold `{capability.<field>}` placeholders, filled at expansion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    pub capability: String,
}

/// One test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestDef {
    #[serde(default = "schema_1")]
    pub schema: u32,
    pub test_id: String,
    pub title: String,
    pub study: StudyRef,
    /// The suite's own stable slugs, never section numbers ([`is_requirement_slug`]).
    pub requirements: Vec<String>,
    pub revision_range: RevisionRange,
    pub bench: BenchRef,
    pub precondition: Precondition,
    /// Test ids this one depends on.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Runs; each gets its own verdict, the test's being the worst run's.
    #[serde(default = "one")]
    pub run_count: u32,
    pub limits: Vec<TestLimit>,
    pub decision_rule: DecisionRuleSpec,
    #[serde(default)]
    pub setup_terms: Vec<SetupTerm>,
    #[serde(default)]
    pub sources: Vec<DutSource>,
    /// Selection tags; not verdict-deciding.
    #[serde(default)]
    pub tags: Vec<String>,
    /// `<test id>@<suite version>` of the premade test this is a copy of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<Template>,
    /// Applicability: every condition must hold over the DUT's capabilities
    /// for the test to apply (`hil_expand`, decision 111). Empty applies always.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applies_when: Vec<Condition>,
    /// The hooks the test needs mapped; one with no mapping gives `hook_unmapped`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hooks: Vec<String>,
    /// Set on a measurable capability's check: the capability whose declared
    /// or decoded value this test compares with the measured one. Every test
    /// expanded from that capability `requires` it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<String>,
}

/// One applicability condition over a DUT capability (by `name`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Condition {
    /// The capability is present.
    Present { capability: String },
    /// The capability's value equals `value`.
    Equals { capability: String, value: Value },
    /// The capability's value is an array holding `value`.
    Contains { capability: String, value: Value },
}

/// A refusal of a file's own header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShapeError {
    /// `schema` is not [`SCHEMA`].
    Schema(u32),
    /// `run_count` is 0.
    RunCount,
    /// A bench's `description` (by bench name) is not a JSON object.
    Description(String),
}

impl TestDef {
    /// `schema` and `run_count`; every other check is the definition-time one.
    pub fn check(&self) -> Result<(), ShapeError> {
        if self.schema != SCHEMA {
            return Err(ShapeError::Schema(self.schema));
        }
        if self.run_count == 0 {
            return Err(ShapeError::RunCount);
        }
        Ok(())
    }

    /// The hash a confirmation binds: see [`definition_hash`].
    pub fn hash(&self) -> String {
        definition_hash(self)
    }
}

/// Whether `s` is a requirement slug: a lowercase letter, then lowercase
/// letters, digits, `-`, `_` or `.`. A section number such as `5.1.2` is not one.
pub fn is_requirement_slug(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some('a'..='z'))
        && c.all(|ch| matches!(ch, 'a'..='z' | '0'..='9' | '-' | '_' | '.'))
}

// ---- a suite manifest ----------------------------------------------------

/// A suite's test, by file and hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteTest {
    pub test_id: String,
    pub file: String,
    pub hash: String,
}

/// What a bench board is told and what it can do, by the commands of its own
/// firmware (plan choice 4: carried here, not shipped with the board).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchBoard {
    /// The board's part id in the saved-bench description.
    pub part: String,
    pub idle: String,
    pub hello: String,
    pub read_only: Vec<String>,
    /// The preconditions this board can perform.
    pub preconditions: Vec<Precondition>,
}

/// A suite's bench: a saved-bench description (embarch-ui decision 73) and its
/// boards' lines. `description` is held as the JSON object embarch-ui owns and
/// checks; this crate refuses anything that is not an object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteBench {
    pub name: String,
    pub description: Value,
    pub boards: Vec<BenchBoard>,
}

/// A suite manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteManifest {
    #[serde(default = "schema_1")]
    pub schema: u32,
    pub id: String,
    pub version: String,
    /// The EmbArch release the suite was validated with.
    pub validated_with: String,
    pub tests: Vec<SuiteTest>,
    pub benches: Vec<SuiteBench>,
}

impl SuiteManifest {
    /// `schema`, and every bench `description` an object.
    pub fn check(&self) -> Result<(), ShapeError> {
        if self.schema != SCHEMA {
            return Err(ShapeError::Schema(self.schema));
        }
        match self.benches.iter().find(|b| !b.description.is_object()) {
            Some(b) => Err(ShapeError::Description(b.name.clone())),
            None => Ok(()),
        }
    }

    /// The manifest hash a plan pinned to a suite the Core lacks confirms.
    pub fn hash(&self) -> String {
        definition_hash(self)
    }
}

// ---- a plan --------------------------------------------------------------

/// The suite a plan pins: by id, version and EmbArch commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuitePin {
    pub id: String,
    pub version: String,
    pub commit: String,
}

/// Which tests a plan runs; a test is selected by its id or by any of its tags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Selection {
    All,
    Some { tests: Vec<String>, tags: Vec<String> },
}

impl Selection {
    /// Whether a test with this id and these tags is selected.
    pub fn selects(&self, test_id: &str, tags: &[String]) -> bool {
        match self {
            Selection::All => true,
            Selection::Some { tests, tags: want } => {
                tests.iter().any(|t| t == test_id) || tags.iter().any(|t| want.contains(t))
            }
        }
    }
}

/// Where a capability's value came from (`notes/hil/03-test.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    Measured,
    Decoded,
    Confirmed,
    Declared,
}

/// A DUT capability and where it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub name: String,
    pub value: Value,
    pub provenance: Provenance,
}

/// What a hook is mapped to: a DUT shell command or an operator prompt. A hook
/// with no entry skips its test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HookTarget {
    Shell { command: String },
    Prompt { text: String },
}

/// A hook and its mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookMapping {
    pub hook: String,
    pub target: HookTarget,
}

/// The DUT's inputs: what decides applicability, and the hook mappings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DutInputs {
    pub capabilities: Vec<Capability>,
    pub hooks: Vec<HookMapping>,
}

/// A test plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    #[serde(default = "schema_1")]
    pub schema: u32,
    pub suite: SuitePin,
    pub select: Selection,
    pub dut: DutInputs,
    pub stop_on_fail: bool,
    /// The order benches run in, by name; a bench not listed follows, in the
    /// suite's order. Empty is the suite's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bench_order: Vec<String>,
}

impl Plan {
    /// `schema` only.
    pub fn check(&self) -> Result<(), ShapeError> {
        if self.schema != SCHEMA {
            return Err(ShapeError::Schema(self.schema));
        }
        Ok(())
    }
}

// ---- the definition hash ---------------------------------------------------

pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// The ES6 `Number::toString` of a finite double, as RFC 8785 requires.
fn es_number(f: f64) -> String {
    if f == 0.0 {
        return "0".to_string();
    }
    let sign = if f < 0.0 { "-" } else { "" };
    // The shortest round-trip digits: `d.ddde<x>`.
    let sci = format!("{:e}", f.abs());
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exp.parse::<i32>().unwrap_or(0) + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let es = if e < 0 { format!("-{}", -e) } else { format!("+{e}") };
        if k == 1 {
            format!("{digits}e{es}")
        } else {
            format!("{}.{}e{es}", &digits[..1], &digits[1..])
        }
    };
    format!("{sign}{body}")
}

fn jcs_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn jcs(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            // Integers in these files stay below 2^53, where the double
            // spelling and the integer spelling are the same string.
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                out.push_str(&es_number(n.as_f64().unwrap_or(0.0)));
            }
        }
        Value::String(s) => jcs_string(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                jcs(x, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                jcs_string(k, out);
                out.push(':');
                jcs(&m[k], out);
            }
            out.push('}');
        }
    }
}

/// The canonical JSON (RFC 8785) of a value: keys sorted by UTF-16 code unit,
/// no whitespace, numbers in ES6 form.
pub fn canonical_json(v: &Value) -> String {
    let mut s = String::new();
    jcs(v, &mut s);
    s
}

/// The definition hash: 64 lowercase hex characters of SHA-256 over the
/// canonical JSON of `def` as this crate serialises it. A confirmation binds it.
pub fn definition_hash<T: Serialize>(def: &T) -> String {
    let v = serde_json::to_value(def).unwrap_or(Value::Null);
    hex_lower(&Sha256::digest(canonical_json(&v).as_bytes()))
}

// ---- the derived diff ------------------------------------------------------

/// One verdict-deciding field that differs between a copy and its premade test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldDiff {
    /// A top-level field, or `limits.<measurement>` / `sources.<measurement>`.
    pub field: String,
    /// The premade value; `null` where the premade test has none.
    pub old: Value,
    /// The copy's value; `null` where the copy has none.
    pub new: Value,
}

/// The fields a diff reads: everything verdict-deciding. `test_id`, `title`,
/// `tags`, `schema` and `derived_from` do not decide a verdict.
const DECIDING: &[&str] = &[
    "study",
    "requirements",
    "revision_range",
    "bench",
    "precondition",
    "requires",
    "run_count",
    "decision_rule",
    "setup_terms",
    "template",
    "applies_when",
    "hooks",
    "checks",
];

fn keyed(v: &Value) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    if let Some(a) = v.as_array() {
        for x in a {
            if let Some(name) = x.get("measurement").and_then(Value::as_str) {
                m.insert(name.to_string(), x.clone());
            }
        }
    }
    m
}

/// Every verdict-deciding field of `copy` that differs from `premade`, by name,
/// old (premade) and new (copy). Limits and sources are compared per measurement.
pub fn derived_diff(copy: &TestDef, premade: &TestDef) -> Vec<FieldDiff> {
    let c = serde_json::to_value(copy).unwrap_or(Value::Null);
    let p = serde_json::to_value(premade).unwrap_or(Value::Null);
    let mut out = Vec::new();
    let get = |v: &Value, k: &str| v.get(k).cloned().unwrap_or(Value::Null);
    for f in DECIDING {
        let (old, new) = (get(&p, f), get(&c, f));
        if old != new {
            out.push(FieldDiff { field: (*f).to_string(), old, new });
        }
    }
    for f in ["limits", "sources"] {
        let (po, co) = (keyed(&get(&p, f)), keyed(&get(&c, f)));
        let mut names: Vec<&String> = po.keys().chain(co.keys()).collect();
        names.sort();
        names.dedup();
        for n in names {
            let old = po.get(n).cloned().unwrap_or(Value::Null);
            let new = co.get(n).cloned().unwrap_or(Value::Null);
            if old != new {
                out.push(FieldDiff { field: format!("{f}.{n}"), old, new });
            }
        }
    }
    out
}
