//! The expansion of a test selection into the resolved run (`interfaces/hil-run.md`,
//! decision 111): one pure, deterministic function, [`expand`], that embarch-api
//! calls to write `run.json` and Core calls to re-expand it, refusing one it does
//! not reproduce. Host-only, behind the `hil-test` feature.
//!
//! The same inputs give the same bytes and hash: every iteration is over a
//! sorted map, so the order of the suite's tests in the file changes nothing.
//! `run_id` is an argument, never generated here (no clock, no randomness).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::hil_test::{
    canonical_json, definition_hash, hex_lower, Capability, Condition, DecisionRuleSpec, DutInputs, HookTarget,
    LimitSpec, Plan, Precondition, Provenance, RevisionRange, SuiteBench, SuiteManifest, TestDef, SCHEMA,
};
use crate::host_study::{HostAction, HostStep, HostStudy, PromptKind};

/// The `kind` every study of a run has until a dev-bench study is allowed
/// (plan choice 18).
const STUDY_KIND_HOST: &str = "host";

/// A suite as loaded: its manifest, every test it lists (found by `test_id`)
/// and the studies those tests name (by `study.file`).
#[derive(Debug, Clone, Copy)]
pub struct SuiteContent<'a> {
    pub manifest: &'a SuiteManifest,
    pub tests: &'a [TestDef],
    pub studies: &'a BTreeMap<String, HostStudy>,
}

/// Where a confirmation was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Ui,
    Cli,
}

/// A confirmation Core holds: the hash of exactly what was confirmed, what it
/// was a confirmation of (`capability:<name>` or `hook:<name>`), when and where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Confirmation {
    pub scope: String,
    pub hash: String,
    pub at_utc_ms: u64,
    pub surface: Surface,
}

/// The standard and revision the engineer confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Standard {
    pub name: String,
    pub revision: String,
}

/// The unit's identity as the caller read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DutIdentity {
    /// `None` where read-out protection blocks the read.
    pub hardware_id: Option<String>,
    pub declared_serial: String,
    pub board_revision: String,
}

/// Everything beside the suite, the plan and the DUT's inputs.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpandContext {
    /// A fresh random id the caller wrote; Core refuses one it already holds.
    pub run_id: String,
    /// The plan's hash; `None` for an unsaved selection.
    pub plan_hash: Option<String>,
    pub standard: Standard,
    pub identity: DutIdentity,
    /// The `interrupted` run this one resumes.
    pub resumes: Option<String>,
    /// The benches that exist, by name; a test on another is `bench_missing`.
    pub benches_present: Vec<String>,
    pub confirmations: Vec<Confirmation>,
}

// ---- the selection record ----------------------------------------------------

/// What became of one test of the suite (`notes/hil/03-test.md`, "A run", item 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionStatus {
    Selected,
    NotApplicable,
    HookUnmapped,
    NotSelected,
    BenchMissing,
}

/// One test's status; `reason` names the condition, hook or bench, or says a
/// `requires` pulled the test in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionEntry {
    pub test_id: String,
    pub status: SelectionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// For a selected template, the ids it expanded to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expanded: Vec<String>,
}

/// The selection as expanded: every test of the suite, by `test_id`. Named
/// apart from `hil_test::Selection`, which is a plan's picks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionRecord {
    pub entries: Vec<SelectionEntry>,
}

// ---- run.json ------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSuite {
    pub id: String,
    pub version: String,
    pub commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunCapability {
    pub name: String,
    pub value: Value,
    pub provenance: Provenance,
    pub confirmed: Option<Confirmation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunHook {
    pub name: String,
    /// `{"shell": cmd}`, `{"prompt": text}` or `null` for unmapped.
    pub maps_to: Value,
    pub confirmed: Option<Confirmation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunDut {
    pub hardware_id: Option<String>,
    pub declared_serial: String,
    pub board_revision: String,
    pub capabilities: Vec<RunCapability>,
    pub hooks: Vec<RunHook>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOptions {
    pub stop_on_fail: bool,
}

/// A bench board's lines, from the suite's bench.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunBoard {
    pub part: String,
    pub idle: String,
    pub hello: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunBench {
    pub name: String,
    pub tests: Vec<String>,
    pub boards: Vec<RunBoard>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStudy {
    pub kind: String,
    pub hash: String,
    pub file: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpandedFrom {
    pub capability: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunBenchRef {
    pub name: String,
    pub config: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunDecisionRule {
    /// `simple_acceptance` or `guard_band`.
    pub kind: String,
    /// `0` under simple acceptance.
    pub w_multiple: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunLimit {
    pub measurement: String,
    /// From the study's parse or the test's source for the measurement.
    pub unit: Option<String>,
    pub lower: Option<f64>,
    pub upper: Option<f64>,
    /// An observation's choice: the string the measurement must equal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSetupTerm {
    pub name: String,
    pub unit: String,
    #[serde(rename = "U")]
    pub u: f64,
    pub k: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSource {
    pub measurement: String,
    pub unit: String,
    pub range_min: f64,
    pub range_max: f64,
    pub resolution: f64,
    #[serde(rename = "U")]
    pub u: f64,
    pub k: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPromptKind {
    pub step: String,
    pub kind: PromptKind,
}

/// One concrete pass of a test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunTest {
    pub test_id: String,
    pub title: String,
    /// The confirmed definition's hash; a template's expansions share it.
    pub definition_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded_from: Option<ExpandedFrom>,
    pub study: RunStudy,
    pub requirements: Vec<String>,
    pub revision_range: RevisionRange,
    pub bench: RunBenchRef,
    pub precondition: Precondition,
    pub requires: Vec<String>,
    pub run_count: u32,
    pub decision_rule: RunDecisionRule,
    pub limits: Vec<RunLimit>,
    pub setup_terms: Vec<RunSetupTerm>,
    pub sources: Vec<RunSource>,
    pub prompt_kinds: Vec<RunPromptKind>,
    /// The hooks the test needs.
    pub hooks: Vec<String>,
    /// The worst case of one run, in ms: every step's timeout and delay (a
    /// prompt's timeout is its deadline), a `Repeat` once per pass, teardown
    /// included. [`duration_bound`] multiplies it by `run_count`.
    pub step_budget_ms: u64,
}

/// The resolved run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunJson {
    pub schema: u32,
    pub run_id: String,
    pub suite: RunSuite,
    pub plan_hash: Option<String>,
    pub standard: Standard,
    pub dut: RunDut,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumes: Option<String>,
    pub options: RunOptions,
    pub benches: Vec<RunBench>,
    pub tests: Vec<RunTest>,
}

impl RunJson {
    /// The canonical hash of `run.json`: SHA-256 over its RFC 8785 form.
    pub fn hash(&self) -> String {
        definition_hash(self)
    }

    /// The canonical bytes the hash is over.
    pub fn canonical(&self) -> String {
        canonical_json(&serde_json::to_value(self).unwrap_or(Value::Null))
    }
}

/// An upper bound on the run's duration in ms: every test's one-run budget
/// times its `run_count`. Preconditions and bench smoke checks are not in it.
pub fn duration_bound(run: &RunJson) -> u64 {
    run.tests.iter().map(|t| t.step_budget_ms.saturating_mul(u64::from(t.run_count))).sum()
}

/// A study written out for the caller to send with `run.json`; an expanded
/// template's is canonical JSON, so its hash is of those exact bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedStudy {
    pub test_id: String,
    pub file: String,
    pub hash: String,
    pub bytes: String,
    pub study: HostStudy,
}

/// What [`expand`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct Expansion {
    pub run: RunJson,
    pub selection: SelectionRecord,
    /// One per expanded template pass.
    pub studies: Vec<ExpandedStudy>,
}

/// A refusal to expand at all: the inputs do not describe a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpandError {
    EmptyRunId,
    /// The manifest is not the suite the plan pins.
    SuiteMismatch { plan: String, manifest: String },
    /// The manifest lists a test no definition was given for.
    TestMissing(String),
    DuplicateTest(String),
    /// A selected test's study file was not given.
    StudyMissing { test: String, file: String },
    /// A `requires` names no test of the suite.
    UnknownRequires { test: String, requires: String },
    /// `requires` cycles inside a bench.
    Cycle(String),
    /// A test requires one that runs after it.
    RequiresOrder { test: String, requires: String },
    /// A `{capability.<field>}` the capability's value has no field for.
    Placeholder { test: String, field: String },
    /// An expanded study or test no longer parses after filling.
    Filled(String),
}

/// A pass about to become a [`RunTest`]: its id, its base test, the (filled)
/// definition, the base study, its [`RunStudy`] and the capability value.
type Staged<'a> = (String, &'a str, TestDef, &'a HostStudy, RunStudy, Option<ExpandedFrom>);

// ---- expansion -------------------------------------------------------------------

fn sha256_hex(bytes: &[u8]) -> String {
    hex_lower(&Sha256::digest(bytes))
}

fn capability<'a>(dut: &'a DutInputs, name: &str) -> Option<&'a Capability> {
    dut.capabilities.iter().find(|c| c.name == name)
}

/// Why `c` does not hold, or `None` if it does.
fn unmet(c: &Condition, dut: &DutInputs) -> Option<String> {
    match c {
        Condition::Present { capability: n } => match capability(dut, n) {
            Some(_) => None,
            None => Some(format!("capability {n} is not present")),
        },
        Condition::Equals { capability: n, value } => match capability(dut, n) {
            Some(cap) if cap.value == *value => None,
            _ => Some(format!("capability {n} must equal {}", canonical_json(value))),
        },
        Condition::Contains { capability: n, value } => match capability(dut, n).and_then(|c| c.value.as_array()) {
            Some(a) if a.contains(value) => None,
            _ => Some(format!("capability {n} must contain {}", canonical_json(value))),
        },
    }
}

/// Replace every `{capability.<field>}` in the strings of `v` by `element`'s
/// field (an object's key, or `value` for a scalar). Strings are spliced raw;
/// anything else as its canonical JSON.
fn fill(v: &mut Value, element: &Value, test: &str) -> Result<(), ExpandError> {
    match v {
        Value::String(s) => {
            const OPEN: &str = "{capability.";
            let mut out = String::new();
            let mut rest = s.as_str();
            while let Some(i) = rest.find(OPEN) {
                let after = &rest[i + OPEN.len()..];
                let Some(j) = after.find('}') else { break };
                let field = &after[..j];
                let got = match element {
                    Value::Object(m) => m.get(field),
                    other if field == "value" => Some(other),
                    _ => None,
                };
                let Some(got) = got else {
                    return Err(ExpandError::Placeholder { test: test.to_string(), field: field.to_string() });
                };
                out.push_str(&rest[..i]);
                match got {
                    Value::String(t) => out.push_str(t),
                    other => out.push_str(&canonical_json(other)),
                }
                rest = &after[j + 1..];
            }
            out.push_str(rest);
            *s = out;
        }
        Value::Array(a) => {
            for x in a {
                fill(x, element, test)?;
            }
        }
        Value::Object(m) => {
            for x in m.values_mut() {
                fill(x, element, test)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn budget(steps: &[HostStep]) -> u64 {
    steps
        .iter()
        .map(|s| {
            let own = u64::from(s.delay_before_ms);
            own + match &s.action {
                HostAction::Wait { ms } => u64::from(*ms),
                HostAction::Repeat { count, values, steps } => {
                    let passes = count.map(u64::from).unwrap_or(values.len() as u64);
                    passes.saturating_mul(budget(steps))
                }
                _ => u64::from(s.timeout_ms),
            }
        })
        .sum()
}

fn study_budget(study: &HostStudy) -> u64 {
    budget(&study.steps) + budget(&study.teardown)
}

fn prompt_kinds(steps: &[HostStep], out: &mut Vec<RunPromptKind>) {
    for s in steps {
        match &s.action {
            HostAction::Prompt { kind: Some(kind), .. } => out.push(RunPromptKind { step: s.name.clone(), kind: *kind }),
            HostAction::Repeat { steps, .. } => prompt_kinds(steps, out),
            _ => {}
        }
    }
}

fn parse_units(steps: &[HostStep], out: &mut BTreeMap<String, String>) {
    for s in steps {
        match &s.action {
            HostAction::Exchange { parse: Some(p), .. } | HostAction::Instrument { parse: Some(p), .. } => {
                out.insert(p.name.clone(), p.unit.clone());
            }
            HostAction::Prompt { answer: Some(a), .. } => {
                if let Some(n) = &a.number {
                    out.insert(a.name.clone(), n.unit.clone());
                }
            }
            HostAction::Repeat { steps, .. } => parse_units(steps, out),
            _ => {}
        }
    }
}

fn run_test(def: &TestDef, id: String, study: &HostStudy, run_study: RunStudy, from: Option<ExpandedFrom>, requires: Vec<String>, def_hash: String) -> RunTest {
    let mut units = BTreeMap::new();
    parse_units(&study.steps, &mut units);
    parse_units(&study.teardown, &mut units);
    for s in &def.sources {
        units.entry(s.measurement.clone()).or_insert_with(|| s.unit.clone());
    }
    let limits = def
        .limits
        .iter()
        .map(|l| {
            let unit = units.get(&l.measurement).cloned();
            let (lower, upper, expect) = match &l.limit {
                LimitSpec::Band { lo, hi } => (Some(*lo), Some(*hi), None),
                LimitSpec::AtMost { max } => (None, Some(*max), None),
                LimitSpec::AtLeast { min } => (Some(*min), None, None),
                LimitSpec::Expect { value } => (None, None, Some(value.clone())),
            };
            RunLimit { measurement: l.measurement.clone(), unit, lower, upper, expect }
        })
        .collect();
    let mut kinds = Vec::new();
    prompt_kinds(&study.steps, &mut kinds);
    prompt_kinds(&study.teardown, &mut kinds);
    let decision_rule = match def.decision_rule {
        DecisionRuleSpec::Simple => RunDecisionRule { kind: "simple_acceptance".to_string(), w_multiple: 0.0 },
        DecisionRuleSpec::GuardBand { w_multiple } => RunDecisionRule { kind: "guard_band".to_string(), w_multiple },
    };
    RunTest {
        test_id: id,
        title: def.title.clone(),
        definition_hash: def_hash,
        derived_from: def.derived_from.clone(),
        expanded_from: from,
        study: run_study,
        requirements: def.requirements.clone(),
        revision_range: def.revision_range.clone(),
        bench: RunBenchRef { name: def.bench.name.clone(), config: def.bench.config.clone() },
        precondition: def.precondition,
        requires,
        run_count: def.run_count,
        decision_rule,
        limits,
        setup_terms: def
            .setup_terms
            .iter()
            .map(|t| RunSetupTerm { name: t.name.clone(), unit: t.unit.clone(), u: t.u, k: t.k })
            .collect(),
        sources: def
            .sources
            .iter()
            .map(|s| RunSource {
                measurement: s.measurement.clone(),
                unit: s.unit.clone(),
                range_min: s.range_min,
                range_max: s.range_max,
                resolution: s.resolution,
                u: s.u,
                k: s.k,
            })
            .collect(),
        prompt_kinds: kinds,
        hooks: def.hooks.clone(),
        step_budget_ms: study_budget(study),
    }
}

fn confirmation(list: &[Confirmation], scope: &str) -> Option<Confirmation> {
    list.iter().filter(|c| c.scope == scope).max_by_key(|c| c.at_utc_ms).cloned()
}

/// Expand a plan's selection over a suite into the resolved run.
///
/// Applicability first, then the plan's picks; every test of the suite gets one
/// [`SelectionStatus`]. A selected test's `requires` pull in the tests they name
/// that apply and can run. A template expands once per value of its capability.
pub fn expand(suite: &SuiteContent, plan: &Plan, dut: &DutInputs, ctx: &ExpandContext) -> Result<Expansion, ExpandError> {
    if ctx.run_id.is_empty() {
        return Err(ExpandError::EmptyRunId);
    }
    let m = suite.manifest;
    if m.id != plan.suite.id {
        return Err(ExpandError::SuiteMismatch { plan: plan.suite.id.clone(), manifest: m.id.clone() });
    }
    let mut defs: BTreeMap<&str, &TestDef> = BTreeMap::new();
    for d in suite.tests {
        if defs.insert(d.test_id.as_str(), d).is_some() {
            return Err(ExpandError::DuplicateTest(d.test_id.clone()));
        }
    }
    for t in &m.tests {
        if !defs.contains_key(t.test_id.as_str()) {
            return Err(ExpandError::TestMissing(t.test_id.clone()));
        }
    }
    let ids: BTreeSet<&str> = m.tests.iter().map(|t| t.test_id.as_str()).collect();
    let defs: BTreeMap<&str, &TestDef> = defs.into_iter().filter(|(k, _)| ids.contains(k)).collect();

    // A capability's check: every template of that capability requires it.
    let mut requires: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for (id, d) in &defs {
        let mut r: BTreeSet<String> = d.requires.iter().cloned().collect();
        if let Some(t) = &d.template {
            for (cid, c) in &defs {
                if c.checks.as_deref() == Some(t.capability.as_str()) && cid != id {
                    r.insert((*cid).to_string());
                }
            }
        }
        for q in &r {
            if !defs.contains_key(q.as_str()) {
                return Err(ExpandError::UnknownRequires { test: (*id).to_string(), requires: q.clone() });
            }
        }
        requires.insert(id, r);
    }

    // Per test: not applicable, the values it expands over, what blocks it.
    let live_benches: BTreeSet<&str> = ctx.benches_present.iter().map(String::as_str).collect();
    let suite_benches: BTreeMap<&str, &SuiteBench> = m.benches.iter().map(|b| (b.name.as_str(), b)).collect();
    let mapped: BTreeSet<&str> = dut.hooks.iter().map(|h| h.hook.as_str()).collect();
    let mut status: BTreeMap<&str, (SelectionStatus, Option<String>)> = BTreeMap::new();
    let mut values: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    let mut blocked: BTreeMap<&str, (SelectionStatus, String)> = BTreeMap::new();
    for (id, d) in &defs {
        if let Some(why) = d.applies_when.iter().find_map(|c| unmet(c, dut)) {
            status.insert(id, (SelectionStatus::NotApplicable, Some(why)));
            continue;
        }
        if let Some(t) = &d.template {
            match capability(dut, &t.capability) {
                None => {
                    status.insert(id, (SelectionStatus::NotApplicable, Some(format!("capability {} is not present", t.capability))));
                    continue;
                }
                Some(c) => {
                    let vs = match &c.value {
                        Value::Array(a) => a.clone(),
                        Value::Null => Vec::new(),
                        other => alloc::vec![other.clone()],
                    };
                    if vs.is_empty() {
                        status.insert(id, (SelectionStatus::NotApplicable, Some(format!("capability {} has no values", t.capability))));
                        continue;
                    }
                    values.insert(id, vs);
                }
            }
        }
        if let Some(h) = d.hooks.iter().find(|h| !mapped.contains(h.as_str())) {
            blocked.insert(id, (SelectionStatus::HookUnmapped, format!("hook {h} has no mapping")));
        } else if !live_benches.contains(d.bench.name.as_str()) || !suite_benches.contains_key(d.bench.name.as_str()) {
            blocked.insert(id, (SelectionStatus::BenchMissing, format!("bench {} does not exist", d.bench.name)));
        }
    }
    let applicable: BTreeSet<&str> = defs.keys().copied().filter(|k| !status.contains_key(k)).collect();
    let mut selected: BTreeSet<&str> = BTreeSet::new();
    for id in &applicable {
        let d = defs[id];
        if plan.select.selects(&d.test_id, &d.tags) {
            match blocked.get(id) {
                Some(b) => {
                    status.insert(id, (b.0, Some(b.1.clone())));
                }
                None => {
                    selected.insert(id);
                }
            }
        }
    }
    // Closure of `requires`.
    let mut pulled: BTreeMap<&str, String> = BTreeMap::new();
    loop {
        let mut add: Vec<(&str, String)> = Vec::new();
        for s in &selected {
            for q in &requires[s] {
                let q = q.as_str();
                if applicable.contains(q) && !blocked.contains_key(q) && !selected.contains(q) {
                    add.push((q, format!("required by {s}")));
                }
            }
        }
        if add.is_empty() {
            break;
        }
        for (q, why) in add {
            if selected.insert(q) {
                pulled.insert(q, why);
            }
        }
    }
    for id in &applicable {
        if selected.contains(id) || status.contains_key(id) {
            continue;
        }
        let e = match blocked.get(id) {
            Some(b) if plan.select.selects(&defs[id].test_id, &defs[id].tags) => (b.0, Some(b.1.clone())),
            _ => (SelectionStatus::NotSelected, None),
        };
        status.insert(id, e);
    }

    // Expand the selected tests.
    let mut studies_out: Vec<ExpandedStudy> = Vec::new();
    let mut expanded_ids: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut staged: Vec<Staged> = Vec::new();
    for id in &selected {
        let d = defs[id];
        let study = |file: &str| -> Result<&HostStudy, ExpandError> {
            suite.studies.get(file).ok_or_else(|| ExpandError::StudyMissing { test: (*id).to_string(), file: file.to_string() })
        };
        match (&d.template, values.get(id)) {
            (Some(t), Some(vs)) => {
                let base = study(&d.study.file)?;
                let stem = d.study.file.strip_suffix(".json").unwrap_or(&d.study.file);
                for (i, v) in vs.iter().enumerate() {
                    let n = i + 1;
                    let tid = format!("{id}#{n}");
                    let mut tv = serde_json::to_value(d).map_err(|e| ExpandError::Filled(e.to_string()))?;
                    fill(&mut tv, v, id)?;
                    let mut ex: TestDef = serde_json::from_value(tv).map_err(|e| ExpandError::Filled(e.to_string()))?;
                    let mut sv = serde_json::to_value(base).map_err(|e| ExpandError::Filled(e.to_string()))?;
                    fill(&mut sv, v, id)?;
                    let es: HostStudy = serde_json::from_value(sv.clone()).map_err(|e| ExpandError::Filled(e.to_string()))?;
                    let bytes = canonical_json(&sv);
                    let file = format!("{stem}.{n}.json");
                    let hash = sha256_hex(bytes.as_bytes());
                    ex.study.file = file.clone();
                    ex.study.hash = hash.clone();
                    studies_out.push(ExpandedStudy { test_id: tid.clone(), file: file.clone(), hash: hash.clone(), bytes, study: es });
                    expanded_ids.entry(id).or_default().push(tid.clone());
                    let from = ExpandedFrom { capability: t.capability.clone(), value: v.clone() };
                    staged.push((tid, id, ex, base, RunStudy { kind: STUDY_KIND_HOST.to_string(), hash, file }, Some(from)));
                }
            }
            _ => {
                let s = study(&d.study.file)?;
                let rs = RunStudy { kind: STUDY_KIND_HOST.to_string(), hash: d.study.hash.clone(), file: d.study.file.clone() };
                expanded_ids.entry(id).or_default().push((*id).to_string());
                staged.push(((*id).to_string(), id, d.clone(), s, rs, None));
            }
        }
    }
    // Expanded studies replace the base study for budget and prompt kinds.
    let by_id: BTreeMap<&str, &ExpandedStudy> = studies_out.iter().map(|s| (s.test_id.as_str(), s)).collect();

    let mut entries: BTreeMap<String, (RunTest, &str)> = BTreeMap::new();
    for (tid, base, def, study, rs, from) in staged {
        let study = by_id.get(tid.as_str()).map(|e| &e.study).unwrap_or(study);
        let mut req: BTreeSet<String> = BTreeSet::new();
        for q in &requires[base] {
            match expanded_ids.get(q.as_str()) {
                Some(ids) => req.extend(ids.iter().cloned()),
                None => {
                    req.insert(q.clone());
                }
            }
        }
        let rt = run_test(&def, tid.clone(), study, rs, from, req.into_iter().collect(), defs[base].hash());
        entries.insert(tid, (rt, base));
    }

    // Bench order, then each bench's tests in `requires` order, a capability's
    // check first.
    let mut bench_names: Vec<&str> = Vec::new();
    let used: BTreeSet<&str> = entries.values().map(|(t, _)| t.bench.name.as_str()).collect();
    for n in plan.bench_order.iter().map(String::as_str).chain(m.benches.iter().map(|b| b.name.as_str())) {
        if used.contains(n) && !bench_names.contains(&n) {
            bench_names.push(n);
        }
    }
    let mut benches: Vec<RunBench> = Vec::new();
    let mut ordered: Vec<String> = Vec::new();
    for b in &bench_names {
        let mine: Vec<&String> = entries.iter().filter(|(_, (t, _))| t.bench.name == *b).map(|(k, _)| k).collect();
        let mut placed: Vec<String> = Vec::new();
        while placed.len() < mine.len() {
            let next = mine
                .iter()
                .filter(|k| !placed.contains(k))
                .filter(|k| entries[**k].0.requires.iter().all(|q| placed.contains(q) || !mine.contains(&q)))
                .min_by_key(|k| (defs[entries[**k].1].checks.is_none(), (**k).clone()));
            match next {
                Some(k) => placed.push((*k).clone()),
                None => return Err(ExpandError::Cycle((*b).to_string())),
            }
        }
        ordered.extend(placed.iter().cloned());
        let boards = suite_benches[b].boards.iter().map(|x| RunBoard { part: x.part.clone(), idle: x.idle.clone(), hello: x.hello.clone() }).collect();
        benches.push(RunBench { name: (*b).to_string(), tests: placed, boards });
    }
    let pos: BTreeMap<&str, usize> = ordered.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    for (i, k) in ordered.iter().enumerate() {
        for q in &entries[k].0.requires {
            if let Some(p) = pos.get(q.as_str()) {
                if *p > i {
                    return Err(ExpandError::RequiresOrder { test: k.clone(), requires: q.clone() });
                }
            }
        }
    }
    let tests: Vec<RunTest> = ordered.iter().map(|k| entries[k].0.clone()).collect();

    // The DUT block: capabilities and the hooks a test needs, by name.
    let mut caps: Vec<RunCapability> = dut
        .capabilities
        .iter()
        .map(|c| RunCapability {
            name: c.name.clone(),
            value: c.value.clone(),
            provenance: c.provenance,
            confirmed: confirmation(&ctx.confirmations, &format!("capability:{}", c.name)),
        })
        .collect();
    caps.sort_by(|a, b| a.name.cmp(&b.name));
    let mut hook_names: BTreeSet<String> = dut.hooks.iter().map(|h| h.hook.clone()).collect();
    for t in &tests {
        hook_names.extend(t.hooks.iter().cloned());
    }
    for (id, (s, _)) in &status {
        if matches!(s, SelectionStatus::HookUnmapped) {
            hook_names.extend(defs[id].hooks.iter().cloned());
        }
    }
    let hooks = hook_names
        .into_iter()
        .map(|n| {
            let maps_to = match dut.hooks.iter().find(|h| h.hook == n).map(|h| &h.target) {
                Some(HookTarget::Shell { command }) => json!({ "shell": command }),
                Some(HookTarget::Prompt { text }) => json!({ "prompt": text }),
                None => Value::Null,
            };
            RunHook { confirmed: confirmation(&ctx.confirmations, &format!("hook:{n}")), name: n, maps_to }
        })
        .collect();

    let run = RunJson {
        schema: SCHEMA,
        run_id: ctx.run_id.clone(),
        suite: RunSuite { id: m.id.clone(), version: m.version.clone(), commit: Some(plan.suite.commit.clone()) },
        plan_hash: ctx.plan_hash.clone(),
        standard: ctx.standard.clone(),
        dut: RunDut {
            hardware_id: ctx.identity.hardware_id.clone(),
            declared_serial: ctx.identity.declared_serial.clone(),
            board_revision: ctx.identity.board_revision.clone(),
            capabilities: caps,
            hooks,
        },
        resumes: ctx.resumes.clone(),
        options: RunOptions { stop_on_fail: plan.stop_on_fail },
        benches,
        tests,
    };

    let mut rec: Vec<SelectionEntry> = Vec::new();
    for id in defs.keys() {
        let (st, reason) = if selected.contains(id) {
            (SelectionStatus::Selected, pulled.get(id).cloned())
        } else {
            status[id].clone()
        };
        rec.push(SelectionEntry {
            test_id: (*id).to_string(),
            status: st,
            reason,
            expanded: if values.contains_key(id) && selected.contains(id) { expanded_ids[id].clone() } else { Vec::new() },
        });
    }
    studies_out.sort_by(|a, b| a.test_id.cmp(&b.test_id));
    Ok(Expansion { run, selection: SelectionRecord { entries: rec }, studies: studies_out })
}
