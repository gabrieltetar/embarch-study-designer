//! One run's records into its verdict, and a test's runs into the test's
//! (decision 112, `interfaces/hil-verdicts.md`). The rules are
//! [`crate::verdicts`]'s; this composes them for one test so Core's run
//! executor and `verify` cannot compose them differently. Host-only, behind
//! `hil-test` (which turns on `verdicts`): the composition reads a `run.json`
//! test entry, and `verdicts` itself stays `alloc`-only (decision 96).
//!
//! Pure: no file, clock or Core. What a step addressed ([`Target`]) and where a
//! measurement came from ([`Origin`]) are given by the caller, never by the
//! study.

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::hil_expand::{RunDecisionRule, RunLimit, RunTest};
use crate::verdicts::{
    Counts, DecisionRule, Evidence, Judged, Limit, Qualifier, Reason, RuleError, Rules, StepOutcome, Target, Verdict,
};

/// `RunDecisionRule::kind` for `w = 0`.
pub const DECISION_SIMPLE: &str = "simple_acceptance";
/// `RunDecisionRule::kind` for `w = w_multiple * U`.
pub const DECISION_GUARD_BAND: &str = "guard_band";

/// Why a run or a test could not be judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JudgeError {
    /// A verdict rule could not run (an unpinned or unshipped behaviour).
    Rule(RuleError),
    /// `decision_rule.kind` is neither `simple_acceptance` nor `guard_band`.
    UnknownDecisionRule(String),
    /// [`skip`] was given a reason that is not a `skipped` one.
    NotASkipReason(Reason),
}

impl From<RuleError> for JudgeError {
    fn from(e: RuleError) -> Self {
        JudgeError::Rule(e)
    }
}

/// One step's outcome, with what the step addressed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepRecord {
    pub step: String,
    pub target: Target,
    pub outcome: StepOutcome,
}

/// What state the bench board's firmware was in when it produced a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirmwareState {
    /// A released image, verified: no qualifier.
    Released,
    Unreleased,
    Unverified,
    FlashedByAgent,
}

/// Where a measured value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum Origin {
    /// A bench board running firmware in the given state.
    BenchBoard { firmware: FirmwareState },
    /// A calibrated instrument.
    Instrument,
    /// The DUT's own report of itself.
    DutReport,
    /// A person's observation, answered at a prompt.
    PersonObservation,
}

/// Who declared the measurement's uncertainty or the source's numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclaredBy {
    /// Confirmed by a person: no qualifier.
    Person,
    Agent,
    Study,
}

/// A measured value: a number, or an observation's choice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum MeasuredValue {
    Number(f64),
    Choice(String),
}

/// One measurement of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeasurementRecord {
    pub measurement: String,
    pub value: MeasuredValue,
    /// The measurement's expanded uncertainty `U`, for the guard band's `w`.
    pub u: f64,
    #[serde(default)]
    pub evidence: Evidence,
    pub origin: Origin,
    pub declared_by: DeclaredBy,
    /// The instrument's or board's calibration was never declared.
    #[serde(default)]
    pub calibration_undeclared: bool,
}

/// A test's verdict with the counts of its runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestVerdict {
    pub judged: Judged,
    pub counts: Counts,
}

/// The decision rule `run.json` states, as the rule the verdicts take.
pub fn decision_rule(rule: &RunDecisionRule) -> Result<DecisionRule, JudgeError> {
    match rule.kind.as_str() {
        DECISION_SIMPLE => Ok(DecisionRule::Simple),
        DECISION_GUARD_BAND => Ok(DecisionRule::GuardBand { w_multiple: rule.w_multiple }),
        other => Err(JudgeError::UnknownDecisionRule(other.into())),
    }
}

fn limit_of(l: &RunLimit) -> Option<Limit> {
    match (l.lower, l.upper) {
        (Some(lo), Some(hi)) => Some(Limit::Band { lo, hi }),
        (None, Some(max)) => Some(Limit::AtMost { max }),
        (Some(min), None) => Some(Limit::AtLeast { min }),
        (None, None) => None,
    }
}

fn judged(verdict: Verdict, reason: Option<Reason>) -> Judged {
    Judged { verdict, reason, qualifiers: Vec::new() }
}

/// The qualifiers one measurement's provenance gives.
fn measurement_qualifiers(m: &MeasurementRecord) -> Vec<Qualifier> {
    let mut q = Vec::new();
    match m.origin {
        Origin::BenchBoard { firmware } => match firmware {
            FirmwareState::Released => {}
            FirmwareState::Unreleased => q.push(Qualifier::BenchFirmwareUnreleased),
            FirmwareState::Unverified => q.push(Qualifier::BenchFirmwareUnverified),
            FirmwareState::FlashedByAgent => q.push(Qualifier::BenchFirmwareFlashedByAgent),
        },
        Origin::Instrument => {}
        Origin::DutReport => q.push(Qualifier::RestsOnDutReport),
        Origin::PersonObservation => q.push(Qualifier::RestsOnPersonObservation),
    }
    if m.calibration_undeclared {
        q.push(Qualifier::CalibrationUndeclared);
    }
    match m.declared_by {
        DeclaredBy::Person => {}
        DeclaredBy::Agent => q.push(Qualifier::DeclaredByAgent),
        DeclaredBy::Study => q.push(Qualifier::DeclaredByStudy),
    }
    q
}

/// A limit against its measurement. A missing measurement is `record_gap`: the
/// run produced no value to judge (a step that failed earlier says why, and
/// outranks this in the combine).
fn judge_limit(rules: &Rules, dr: DecisionRule, l: &RunLimit, m: Option<&MeasurementRecord>) -> Result<Judged, JudgeError> {
    let Some(m) = m else {
        return Ok(judged(Verdict::Inconclusive, Some(Reason::RecordGap)));
    };
    if let Some(expect) = &l.expect {
        // Evidence decides before the choice does, as it does for a number.
        let order = [Reason::TruncatedCapture, Reason::TruncatedReply, Reason::RecordGap, Reason::ParseFailed];
        if let Some(r) = order.iter().find(|r| m.evidence.incomplete.contains(r)) {
            return Ok(judged(Verdict::Inconclusive, Some(*r)));
        }
        return Ok(match &m.value {
            MeasuredValue::Choice(c) if c == expect => judged(Verdict::Pass, None),
            MeasuredValue::Choice(_) => judged(Verdict::Fail, Some(Reason::Limit)),
            MeasuredValue::Number(_) => judged(Verdict::Inconclusive, Some(Reason::ParseFailed)),
        });
    }
    let Some(limit) = limit_of(l) else {
        return Ok(judged(Verdict::Pass, None));
    };
    let MeasuredValue::Number(value) = m.value else {
        return Ok(judged(Verdict::Inconclusive, Some(Reason::ParseFailed)));
    };
    Ok(rules.limit_verdict(value, limit, dr.width(m.u), &m.evidence)?)
}

/// One run's verdict: each step through `step_verdict` with its target, each
/// limit through `limit_verdict` on its measurement (`w` from the test's
/// decision rule and the measurement's `U`), an observation's choice against
/// its `expect` (a different one is `fail`, `limit`, plan choice 27), then
/// `combine`. Qualifiers come from every measurement's provenance.
pub fn judge_run(
    test: &RunTest,
    steps: &[StepRecord],
    measurements: &[MeasurementRecord],
    rules: &Rules,
) -> Result<Judged, JudgeError> {
    let dr = decision_rule(&test.decision_rule)?;
    let mut items = Vec::new();
    for s in steps {
        items.push(rules.step_verdict(s.target, s.outcome)?);
    }
    for l in &test.limits {
        let m = measurements.iter().find(|m| m.measurement == l.measurement);
        items.push(judge_limit(rules, dr, l, m)?);
    }
    let summary = rules.combine(&items)?;
    let reason = items.iter().find(|i| i.verdict == summary.verdict).and_then(|i| i.reason);
    let mut qualifiers = summary.qualifiers;
    for m in measurements {
        qualifiers.extend(measurement_qualifiers(m));
    }
    qualifiers.sort_unstable();
    qualifiers.dedup();
    Ok(Judged { verdict: summary.verdict, reason, qualifiers })
}

/// A test's verdict: its worst run, by `combine`, with the counts of its runs.
/// The reason is the first worst run's.
pub fn judge_test(runs: &[Judged], rules: &Rules) -> Result<TestVerdict, JudgeError> {
    let summary = rules.combine(runs)?;
    let reason = runs.iter().find(|r| r.verdict == summary.verdict).and_then(|r| r.reason);
    Ok(TestVerdict {
        judged: Judged { verdict: summary.verdict, reason, qualifiers: summary.qualifiers },
        counts: summary.counts,
    })
}

/// A test that did not run: `skipped` with its reason, one of `requires_not_met`,
/// `stopped_by_error`, `stopped_after_fail`, `bench_missing`, `hook_unmapped`,
/// `not_applicable`, `not_selected`.
pub fn skip(why: Reason) -> Result<Judged, JudgeError> {
    if why.verdict() != Verdict::Skipped {
        return Err(JudgeError::NotASkipReason(why));
    }
    Ok(judged(Verdict::Skipped, Some(why)))
}

/// Adds the `unvalidated` qualifier: a premade test whose evidence is absent.
pub fn unvalidated(mut j: Judged) -> Judged {
    j.qualifiers.push(Qualifier::Unvalidated);
    j.qualifiers.sort_unstable();
    j.qualifiers.dedup();
    j
}

/// The behaviours `judge_run` and `judge_test` used, for the record.
pub fn behaviours_used(rules: &Rules) -> Vec<(&'static str, u32)> {
    const USED: [&str; 3] = ["step_verdict", "limit_verdict", "combine"];
    rules.behaviours().iter().filter(|(n, _)| USED.contains(n)).copied().collect()
}
