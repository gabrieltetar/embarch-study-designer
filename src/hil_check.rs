//! The definition-time checks of a HIL test (`interfaces/hil-tests.md`, "The
//! definition-time checks", decision 110): one function, [`check_test`], that
//! refuses a test no run could judge, so embarch-api and Core refuse the same
//! test for the same reason. Host-only, behind the `hil-test` feature.
//!
//! The checks read the test, its study, its bench and the metrology of every
//! source that measures a limited value. Instrument metrology does not exist
//! in `embarch-topology` yet (EMB-358), so a caller passes it in [`Measured`]
//! and this crate reads no topology type.
//!
//! Every refusal names the test, the field and the reason, and carries the
//! rule that made it ([`Rule`]) so a caller can match on it.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::hil_test::{LimitSpec, Precondition, SuiteBench, TestDef};
use crate::host_study::{HostAction, HostStep, HostStudy, PromptKind};
use crate::verdicts::{Limit, Rules, Source, WidthBasis, WidthCheck};

/// The saved-bench part role that marks a DUT (embarch-ui decision 73).
const DUT_ROLE: &str = "dut";

/// The study a test names, as far as the checks need it.
#[derive(Debug, Clone, Copy)]
pub enum StudyKind<'a> {
    /// A host-run study, Core's own steps.
    Host(&'a HostStudy),
    /// A dev-bench study: not in this phase (plan choice 18).
    DevBench,
}

/// Who measured a value (`notes/hil/02-study.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    /// A bench board or an instrument.
    Bench,
    /// The DUT's own word; its numbers come from the test's `sources`.
    Dut,
    /// A person's reading.
    Person,
}

/// What a source declares about itself; `None` is not declared.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Metrology {
    pub range_min: Option<f64>,
    pub range_max: Option<f64>,
    pub resolution: Option<f64>,
    /// Expanded uncertainty, in the measurement's unit.
    pub u: Option<f64>,
}

/// One measurement the study makes, as the caller resolved it.
#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    pub by: By,
    /// The instrument, bench board or person that measures it, for a refusal
    /// to name; unused for a `dut` measurement (the test's `sources` name it).
    pub source: String,
    /// An instrument's declaration or a bench board's characterisation;
    /// ignored for a `dut` measurement.
    pub metrology: Metrology,
}

/// The rule a refusal comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// The bench the test names is not the one given.
    WrongBench,
    /// The bench holds no DUT.
    BenchHasNoDut,
    /// A limited measurement no source is known for.
    UnknownMeasurement,
    /// A source with no declared `U`, resolution or range.
    SourceUndeclared,
    /// No result could be judged against the limit.
    LimitUnjudgeable,
    /// A prompt with no kind.
    PromptHasNoKind,
    /// An action prompt whose confirming measurement cannot fail the test.
    ActionUnconfirmed,
    /// An energising step after a setup or action prompt, with no clearance.
    EnergisesWithoutClearance,
    /// A precondition the bench does not say how to perform.
    PreconditionUndeclared,
    /// A dev-bench study.
    DevBenchStudy,
    /// Two limits on one measurement.
    DuplicateLimit,
}

/// One refusal: the test, the field and the reason.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub test: String,
    pub field: String,
    pub rule: Rule,
    pub reason: String,
}

impl core::fmt::Display for Refusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}: {}: {}", self.test, self.field, self.reason)
    }
}

/// Whether `step` energises on `bench`: an instrument set, a bench-board line
/// the bench does not list as read-only, and every DUT line, since Core cannot
/// classify one. A signal is a bench board's when it is named by the board's
/// `part`; any other signal is the DUT's. A step that contains one that
/// energises does too.
pub fn energises(step: &HostStep, bench: &SuiteBench) -> bool {
    match &step.action {
        HostAction::Exchange { signal, line, .. } => match bench.boards.iter().find(|b| b.part == *signal) {
            Some(b) => !b.read_only.iter().any(|r| r.trim() == line.trim()),
            None => true,
        },
        HostAction::Instrument { query, .. } => !*query,
        HostAction::Prompt { .. } | HostAction::Wait { .. } => false,
        HostAction::Repeat { steps, .. } => steps.iter().any(|s| energises(s, bench)),
    }
}

/// Whether the saved-bench description holds a DUT: a part with the `dut`
/// role, or, in the legacy shape with no `parts` key, a non-null `dut`.
fn has_dut(bench: &SuiteBench) -> bool {
    match bench.description.get("parts").and_then(|p| p.as_array()) {
        Some(parts) => parts.iter().any(|p| p.get("role").and_then(|r| r.as_str()) == Some(DUT_ROLE)),
        None => bench.description.get("dut").is_some_and(|d| !d.is_null()),
    }
}

fn declared(x: Option<f64>) -> Option<f64> {
    x.filter(|v| v.is_finite() && *v > 0.0)
}

/// The metrology as the width rule reads it, or the names of what is missing.
fn source_of(m: &Metrology) -> Result<Source, Vec<&'static str>> {
    let (u, res) = (declared(m.u), declared(m.resolution));
    let range = match (m.range_min, m.range_max) {
        (Some(a), Some(b)) if a.is_finite() && b.is_finite() && a < b => Some((a, b)),
        _ => None,
    };
    match (u, res, range) {
        (Some(u), Some(resolution), Some((range_min, range_max))) => {
            Ok(Source { range_min, range_max, resolution, u })
        }
        _ => {
            let mut missing = Vec::new();
            if u.is_none() {
                missing.push("U");
            }
            if res.is_none() {
                missing.push("resolution");
            }
            if range.is_none() {
                missing.push("range");
            }
            Err(missing)
        }
    }
}

struct Ctx<'a> {
    test: &'a TestDef,
    bench: &'a SuiteBench,
    out: Vec<Refusal>,
}

impl Ctx<'_> {
    fn refuse(&mut self, field: String, rule: Rule, reason: String) {
        self.out.push(Refusal { test: self.test.test_id.clone(), field, rule, reason });
    }
}

fn limit_of(spec: &LimitSpec) -> Option<Limit> {
    spec.numeric()
}

fn check_limits(c: &mut Ctx, measured: &BTreeMap<String, Measured>) {
    let rules = Rules::latest();
    let mut seen: Vec<&str> = Vec::new();
    for (i, l) in c.test.limits.iter().enumerate() {
        let field = format!("limits[{i}] ({})", l.measurement);
        if seen.contains(&l.measurement.as_str()) {
            c.refuse(
                field,
                Rule::DuplicateLimit,
                format!("a second limit on `{}`: the derived-test diff keys limits by measurement, so it would hide a change to the first", l.measurement),
            );
            continue;
        }
        seen.push(&l.measurement);
        let Some(m) = measured.get(&l.measurement) else {
            c.refuse(field, Rule::UnknownMeasurement, format!("`{}` is limited but no source is known to measure it", l.measurement));
            continue;
        };
        let Some(limit) = limit_of(&l.limit) else { continue };
        let (name, metrology) = match m.by {
            By::Dut => match c.test.sources.iter().find(|s| s.measurement == l.measurement) {
                Some(s) => (
                    format!("the test's source `{}`", s.measurement),
                    Metrology {
                        range_min: Some(s.range_min),
                        range_max: Some(s.range_max),
                        resolution: Some(s.resolution),
                        u: Some(s.u),
                    },
                ),
                None => {
                    c.refuse(
                        field,
                        Rule::SourceUndeclared,
                        format!("`{}` is a dut measurement and the test's `sources` declares no U, resolution or range for it", l.measurement),
                    );
                    continue;
                }
            },
            By::Bench | By::Person => (format!("`{}`", m.source), m.metrology),
        };
        let src = match source_of(&metrology) {
            Ok(s) => s,
            Err(missing) => {
                c.refuse(field, Rule::SourceUndeclared, format!("{name} declares no {}", missing.join(", ")));
                continue;
            }
        };
        let w = c.test.decision_rule.rule().width(src.u);
        let reason = match rules.acceptance_width(limit, w, &src) {
            Ok(WidthCheck::Accepted) => continue,
            Ok(WidthCheck::BandTooNarrow { basis, width, needed }) => {
                format!("a band {width} wide is not wider than {needed}, 2 x max(w, {}) of {name}", basis_text(basis))
            }
            Ok(WidthCheck::LimitTooCloseToRange { basis, clearance, needed }) => {
                format!("the limit is {clearance} inside the range of {name}, not more than max(w, {}) = {needed}", basis_text(basis))
            }
            Ok(WidthCheck::Malformed) => format!("the limit, the decision rule or the metrology of {name} is not a finite, ordered number"),
            Err(e) => format!("the acceptance-width rule could not run: {e:?}"),
        };
        c.refuse(field, Rule::LimitUnjudgeable, reason);
    }
}

fn basis_text(b: WidthBasis) -> &'static str {
    match b {
        WidthBasis::Uncertainty => "U",
        WidthBasis::Resolution => "resolution",
    }
}

fn kind_text(k: PromptKind) -> &'static str {
    match k {
        PromptKind::Setup => "setup",
        PromptKind::Action => "action",
        PromptKind::Observation => "observation",
        PromptKind::Clearance => "clearance",
    }
}

/// Walks the steps in order, carrying the handling prompt not yet cleared.
fn walk(c: &mut Ctx, steps: &[HostStep], path: &str, pending: &mut Option<(String, PromptKind)>, measured: &BTreeMap<String, Measured>) {
    for (i, s) in steps.iter().enumerate() {
        let here = format!("{path}[{i}]");
        match &s.action {
            HostAction::Prompt { kind, confirmed_by, .. } => match kind {
                None => c.refuse(here, Rule::PromptHasNoKind, "a prompt declares its kind: setup, action, observation or clearance".to_string()),
                Some(k) => {
                    match k {
                        PromptKind::Clearance => *pending = None,
                        PromptKind::Setup | PromptKind::Action => *pending = Some((here.clone(), *k)),
                        PromptKind::Observation => {}
                    }
                    if *k == PromptKind::Action {
                        check_action(c, &here, confirmed_by.as_deref(), measured);
                    }
                }
            },
            HostAction::Repeat { count, values, steps: inner } => {
                let passes = if count.unwrap_or(0) > 1 || values.len() > 1 { 2 } else { 1 };
                for _ in 0..passes {
                    walk(c, inner, &format!("{here}.steps"), pending, measured);
                }
            }
            _ => {
                if energises(s, c.bench) {
                    if let Some((at, k)) = pending.take() {
                        c.refuse(
                            here,
                            Rule::EnergisesWithoutClearance,
                            format!("energises after the {} prompt at {at} with no clearance prompt between them", kind_text(k)),
                        );
                    }
                }
            }
        }
    }
}

fn check_action(c: &mut Ctx, field: &str, confirmed_by: Option<&str>, measured: &BTreeMap<String, Measured>) {
    let bad = |c: &mut Ctx, why: String| c.refuse(field.to_string(), Rule::ActionUnconfirmed, why);
    let Some(name) = confirmed_by else {
        return bad(c, "an action prompt names the measurement that confirms it".to_string());
    };
    match measured.get(name) {
        None => bad(c, format!("`{name}` is no measurement of the study")),
        Some(m) if m.by != By::Bench => {
            bad(c, format!("`{name}` is not by: bench, so a forged \"done\" would not cost the test a fail or an error"))
        }
        Some(_) if !c.test.limits.iter().any(|l| l.measurement == name) => {
            bad(c, format!("`{name}` feeds no limit of the test, so its absence cannot fail it"))
        }
        Some(_) => {}
    }
}

/// Every refusal of `test` at definition, or `Ok`. `study` is the study the
/// test names, `bench` the suite's bench of that name, and `measured` the
/// source of each measurement the test limits, by measurement name.
pub fn check_test(
    test: &TestDef,
    study: StudyKind,
    bench: &SuiteBench,
    measured: &BTreeMap<String, Measured>,
) -> Result<(), Vec<Refusal>> {
    let mut c = Ctx { test, bench, out: Vec::new() };
    if bench.name != test.bench.name {
        c.refuse("bench.name".into(), Rule::WrongBench, format!("the test names bench `{}`, not `{}`", test.bench.name, bench.name));
    }
    if !has_dut(bench) {
        c.refuse(
            "bench.name".into(),
            Rule::BenchHasNoDut,
            format!("bench `{}` holds no DUT; a Test judges a unit, and a study may use a bench with none", bench.name),
        );
    }
    let how = |p: Precondition| bench.boards.iter().any(|b| b.preconditions.contains(&p));
    for (p, name) in [(Precondition::PowerCycle, "power_cycle"), (Precondition::Detach, "detach")] {
        if test.precondition == p && !how(p) {
            c.refuse("precondition".into(), Rule::PreconditionUndeclared, format!("`{name}` needs bench `{}` to say how, and no board lists it", bench.name));
        }
    }
    match study {
        StudyKind::DevBench => c.refuse("study".into(), Rule::DevBenchStudy, "a dev-bench study is not in this phase; a Test runs a host study".to_string()),
        StudyKind::Host(h) => {
            let mut pending = None;
            walk(&mut c, &h.steps, "study.steps", &mut pending, measured);
        }
    }
    check_limits(&mut c, measured);
    let mut seen: Vec<(String, Rule)> = Vec::new();
    c.out.retain(|r| {
        let key = (r.field.clone(), r.rule);
        let fresh = !seen.contains(&key);
        seen.push(key);
        fresh
    });
    if c.out.is_empty() {
        Ok(())
    } else {
        Err(c.out)
    }
}
