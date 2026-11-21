//! The HIL verdict rules (decisions 95 to 98, `interfaces/hil-verdicts.md`):
//! the six verdicts, the mapping from Core's step outcomes, the guard band on
//! both sides of a limit, the acceptance-width rule, the severity order and the
//! requirement roll-up. Host-only, behind the `verdicts` feature.
//!
//! **A verdict is decided here, never by the agent.** Every function is pure:
//! no file, clock or Core. Each rule carries a **behaviour version** kept the
//! way the reducers keep theirs (decision 93): [`Rules::latest`] pins every
//! rule at the newest behaviour, [`Rules::pinned`] pins what a record names,
//! and a behaviour the crate never shipped is [`RuleError::UnknownBehaviour`],
//! which `verify` reports as "cannot recompute", never a substitute.
//!
//! The strings are `interfaces/hil-shapes.md`'s: the six verdicts, the closed
//! `reason` list (plus `observation_unanswered`, decision 95), the twelve
//! qualifiers.

use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

/// Every rule name that carries a behaviour version, for
/// `measurements.json`'s `behaviours`.
pub const RULE_NAMES: &[&str] = &["step_verdict", "limit_verdict", "acceptance_width", "combine", "requirement"];

/// The behaviours this crate has shipped, ascending, per rule.
const SHIPPED: &[(&str, &[u32])] = &[
    ("step_verdict", &[1]),
    ("limit_verdict", &[1]),
    ("acceptance_width", &[1]),
    ("combine", &[1]),
    ("requirement", &[1]),
];

/// The six verdicts, as the strings every file and export spells them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
    Error,
    Inconclusive,
    Skipped,
    Cancelled,
}

impl Verdict {
    /// The six, in the order the per-verdict counts list them.
    pub const ALL: [Verdict; 6] =
        [Verdict::Pass, Verdict::Fail, Verdict::Error, Verdict::Inconclusive, Verdict::Skipped, Verdict::Cancelled];

    /// Severity where a file needs a number: error 5, fail 4, inconclusive 3,
    /// cancelled 2, skipped 1, pass 0.
    pub fn severity(self) -> u8 {
        match self {
            Verdict::Error => 5,
            Verdict::Fail => 4,
            Verdict::Inconclusive => 3,
            Verdict::Cancelled => 2,
            Verdict::Skipped => 1,
            Verdict::Pass => 0,
        }
    }

    /// The string a file carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Fail => "fail",
            Verdict::Error => "error",
            Verdict::Inconclusive => "inconclusive",
            Verdict::Skipped => "skipped",
            Verdict::Cancelled => "cancelled",
        }
    }
}

/// The closed `reason` list. Each belongs to one verdict ([`Reason::verdict`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    // fail
    Limit,
    MarkerRule,
    StepFailed,
    StepTimedOut,
    // error
    BenchStepFailed,
    PortLost,
    EmptyReply,
    PromptNotConfirmed,
    SmokeCheck,
    // inconclusive
    TruncatedCapture,
    TruncatedReply,
    RecordGap,
    ParseFailed,
    GuardBand,
    CalibrationExpired,
    ObservationUnanswered,
    // skipped, a gap
    HookUnmapped,
    BenchMissing,
    RequiresNotMet,
    StoppedByError,
    StoppedAfterFail,
    // skipped, no gap
    NotApplicable,
    NotSelected,
}

impl Reason {
    /// The verdict this reason belongs to; `pass` and `cancelled` have none.
    pub fn verdict(self) -> Verdict {
        use Reason::*;
        match self {
            Limit | MarkerRule | StepFailed | StepTimedOut => Verdict::Fail,
            BenchStepFailed | PortLost | EmptyReply | PromptNotConfirmed | SmokeCheck => Verdict::Error,
            TruncatedCapture | TruncatedReply | RecordGap | ParseFailed | GuardBand | CalibrationExpired
            | ObservationUnanswered => Verdict::Inconclusive,
            HookUnmapped | BenchMissing | RequiresNotMet | StoppedByError | StoppedAfterFail | NotApplicable
            | NotSelected => Verdict::Skipped,
        }
    }

    /// A `skipped` reason that is a gap: it makes a requirement `not verified`.
    pub fn is_gap(self) -> bool {
        use Reason::*;
        matches!(self, HookUnmapped | BenchMissing | RequiresNotMet | StoppedByError | StoppedAfterFail)
    }
}

/// The closed qualifier list, in the order a roll-up lists them. `n_of_m_run`
/// travels beside its `n` and `m` ([`NOfM`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Qualifier {
    BenchFirmwareUnreleased,
    BenchFirmwareUnverified,
    BenchFirmwareFlashedByAgent,
    Unvalidated,
    UnitIdentityNotRead,
    CalibrationUndeclared,
    DeclaredByAgent,
    DeclaredByStudy,
    Relayed,
    RestsOnDutReport,
    RestsOnPersonObservation,
    NOfMRun,
}

/// `n` of `m` tests run, the value beside `n_of_m_run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NOfM {
    pub n: u32,
    pub m: u32,
}

/// A verdict with its reason (where its verdict lists any) and qualifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Judged {
    pub verdict: Verdict,
    #[serde(default)]
    pub reason: Option<Reason>,
    #[serde(default)]
    pub qualifiers: Vec<Qualifier>,
}

impl Judged {
    fn bare(verdict: Verdict, reason: Option<Reason>) -> Self {
        Judged { verdict, reason, qualifiers: Vec::new() }
    }
}

/// Per-verdict counts, an object keyed by the six strings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub pass: u32,
    pub fail: u32,
    pub error: u32,
    pub inconclusive: u32,
    pub skipped: u32,
    pub cancelled: u32,
}

impl Counts {
    /// The count of one verdict.
    pub fn of(&self, v: Verdict) -> u32 {
        match v {
            Verdict::Pass => self.pass,
            Verdict::Fail => self.fail,
            Verdict::Error => self.error,
            Verdict::Inconclusive => self.inconclusive,
            Verdict::Skipped => self.skipped,
            Verdict::Cancelled => self.cancelled,
        }
    }

    fn add(&mut self, v: Verdict) {
        let slot = match v {
            Verdict::Pass => &mut self.pass,
            Verdict::Fail => &mut self.fail,
            Verdict::Error => &mut self.error,
            Verdict::Inconclusive => &mut self.inconclusive,
            Verdict::Skipped => &mut self.skipped,
            Verdict::Cancelled => &mut self.cancelled,
        };
        *slot += 1;
    }
}

// ---- a step's outcome ----------------------------------------------------

/// What a step addressed, which Core derives from the step's target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// A signal on the DUT: its failure can fail the DUT.
    Dut,
    /// A bench board or an instrument: its failure is the setup's.
    Bench,
}

/// Core's step outcomes (embarch-core decisions 95 and 105).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum StepOutcome {
    /// `until` seen, or a prompt confirmed.
    Pass,
    /// `Fail` with `refused_on` seen.
    RefusedOn,
    /// `TimedOut`.
    TimedOut,
    /// `until_not_seen`: bytes arrived, the marker did not.
    UntilNotSeen,
    /// `Fail`, `empty_reply`: no byte at all.
    EmptyReply,
    /// The port was lost.
    PortLost,
    /// `prompt_timed_out`; `observation` is whether the prompt asked a person
    /// what they saw, as against any other kind.
    PromptTimedOut { observation: bool },
    /// `prompt_cancelled`.
    PromptCancelled { observation: bool },
}

// ---- a limit -------------------------------------------------------------

/// A declared limit, the **tolerance** limit(s); the acceptance limit is
/// derived from them and `w`, never stored (decision 91).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Limit {
    /// `lo <= value <= hi`.
    Band { lo: f64, hi: f64 },
    /// `value <= max`.
    AtMost { max: f64 },
    /// `value >= min`.
    AtLeast { min: f64 },
}

/// The decision rule a report states.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecisionRule {
    /// `w = 0`.
    Simple,
    /// `w = w_multiple * U`; the premade default is 1 (ILAC-G8).
    GuardBand { w_multiple: f64 },
}

impl DecisionRule {
    /// The guard band's width for a measurement with expanded uncertainty `u`.
    pub fn width(self, u: f64) -> f64 {
        match self {
            DecisionRule::Simple => 0.0,
            DecisionRule::GuardBand { w_multiple } => w_multiple * u,
        }
    }
}

/// The state of what a limit is judged on: the measurement's evidence.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    /// The measurement's `incomplete` array, any of `truncated_capture`,
    /// `truncated_reply`, `record_gap`, `parse_failed`.
    #[serde(default)]
    pub incomplete: Vec<Reason>,
    /// The instrument's or board's calibration or characterisation had lapsed
    /// at `at_utc_ms`.
    #[serde(default)]
    pub calibration_expired: bool,
}

/// What a limit's range check looked at, when it refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidthBasis {
    /// `max(w, U)`.
    Uncertainty,
    /// `max(w, resolution)`.
    Resolution,
}

/// The result of the acceptance-width check made when a test is defined.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum WidthCheck {
    Accepted,
    /// A two-sided band no wider than `2 * max(w, x)`: `width` against the
    /// `needed` it had to exceed.
    BandTooNarrow { basis: WidthBasis, width: f64, needed: f64 },
    /// A one-sided limit not more than `max(w, x)` inside the measured
    /// range: `clearance` is its distance to the nearer range end.
    LimitTooCloseToRange { basis: WidthBasis, clearance: f64, needed: f64 },
    /// A value that is not a finite number, or a band with `lo > hi`.
    Malformed,
}

/// What the check reads: the source's range, resolution and expanded `U`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub range_min: f64,
    pub range_max: f64,
    pub resolution: f64,
    pub u: f64,
}

// ---- a requirement -------------------------------------------------------

/// A requirement's verdict: a verdict, or `not_verified`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementVerdict {
    Pass,
    Fail,
    Error,
    Inconclusive,
    Cancelled,
    NotVerified,
}

/// A requirement's roll-up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requirement {
    pub verdict: RequirementVerdict,
    /// Per-verdict counts of the tests that count (relayed manual ones left out).
    pub counts: Counts,
    /// Every qualifier of the tests that ran, in the closed list's order; a
    /// pass with tests not selected also carries `n_of_m_run`.
    pub qualifiers: Vec<Qualifier>,
    pub n_of_m: Option<NOfM>,
    /// Skipped tests, each with its reason: listed under a pass or beside a
    /// `not verified`.
    pub skipped: Vec<Reason>,
    /// Relayed manual verdicts, listed apart, counted in nothing.
    pub relayed_listed_apart: u32,
}

/// The run's summary: the worst verdict, the counts, every qualifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub verdict: Verdict,
    pub counts: Counts,
    pub qualifiers: Vec<Qualifier>,
}

// ---- versions ------------------------------------------------------------

/// Why a rule could not be run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleError {
    UnknownRule(alloc::string::String),
    /// The crate never shipped this behaviour, so `verify` cannot recompute.
    UnknownBehaviour { name: &'static str, behaviour: u32 },
    /// The record pinned no behaviour for this rule.
    NotPinned(&'static str),
}

/// The behaviour version of each rule a record names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    pinned: Vec<(&'static str, u32)>,
}

fn shipped(name: &str) -> Option<(&'static str, &'static [u32])> {
    SHIPPED.iter().find(|e| e.0 == name).map(|e| (e.0, e.1))
}

impl Rules {
    /// Every rule at its newest behaviour: what a new record names.
    pub fn latest() -> Self {
        Rules { pinned: SHIPPED.iter().map(|e| (e.0, *e.1.last().unwrap_or(&1))).collect() }
    }

    /// The behaviours a record names (`measurements.json`'s `behaviours`).
    /// A name or a behaviour the crate never shipped is an error.
    pub fn pinned(named: &[(&str, u32)]) -> Result<Self, RuleError> {
        let mut pinned = Vec::new();
        for (name, b) in named {
            let (static_name, versions) = shipped(name).ok_or_else(|| RuleError::UnknownRule((*name).into()))?;
            if !versions.contains(b) {
                return Err(RuleError::UnknownBehaviour { name: static_name, behaviour: *b });
            }
            pinned.push((static_name, *b));
        }
        Ok(Rules { pinned })
    }

    /// The pinned `(rule, behaviour)` pairs, to write into `behaviours`.
    pub fn behaviours(&self) -> &[(&'static str, u32)] {
        &self.pinned
    }

    /// Every behaviour of `name` the crate has shipped, ascending.
    pub fn shipped(name: &str) -> &'static [u32] {
        shipped(name).map(|e| e.1).unwrap_or(&[])
    }

    fn behaviour(&self, name: &'static str) -> Result<u32, RuleError> {
        self.pinned.iter().find(|e| e.0 == name).map(|e| e.1).ok_or(RuleError::NotPinned(name))
    }

    /// Core's step outcome, charged by what the step addressed.
    pub fn step_verdict(&self, target: Target, outcome: StepOutcome) -> Result<Judged, RuleError> {
        match self.behaviour("step_verdict")? {
            1 => Ok(step_v1(target, outcome)),
            b => Err(RuleError::UnknownBehaviour { name: "step_verdict", behaviour: b }),
        }
    }

    /// A numeric result against a limit: evidence first, then the limit with
    /// the guard band `w` on both sides of the tolerance limit.
    pub fn limit_verdict(&self, value: f64, limit: Limit, w: f64, evidence: &Evidence) -> Result<Judged, RuleError> {
        match self.behaviour("limit_verdict")? {
            1 => Ok(limit_v1(value, limit, w, evidence)),
            b => Err(RuleError::UnknownBehaviour { name: "limit_verdict", behaviour: b }),
        }
    }

    /// The check made when a test is defined: could any result be judged
    /// against this limit.
    pub fn acceptance_width(&self, limit: Limit, w: f64, source: &Source) -> Result<WidthCheck, RuleError> {
        match self.behaviour("acceptance_width")? {
            1 => Ok(width_v1(limit, w, source)),
            b => Err(RuleError::UnknownBehaviour { name: "acceptance_width", behaviour: b }),
        }
    }

    /// Combines verdicts (runs into a test, tests into a run's summary).
    pub fn combine(&self, items: &[Judged]) -> Result<Summary, RuleError> {
        match self.behaviour("combine")? {
            1 => Ok(combine_v1(items)),
            b => Err(RuleError::UnknownBehaviour { name: "combine", behaviour: b }),
        }
    }

    /// Rolls the tests of one requirement into its verdict.
    pub fn requirement(&self, tests: &[Judged]) -> Result<Requirement, RuleError> {
        match self.behaviour("requirement")? {
            1 => Ok(requirement_v1(tests)),
            b => Err(RuleError::UnknownBehaviour { name: "requirement", behaviour: b }),
        }
    }
}

// ---- behaviour 1 ---------------------------------------------------------

fn step_v1(target: Target, outcome: StepOutcome) -> Judged {
    use Reason::*;
    let (v, r) = match (outcome, target) {
        (StepOutcome::Pass, _) => (Verdict::Pass, None),
        (StepOutcome::RefusedOn, Target::Dut) => (Verdict::Fail, Some(StepFailed)),
        (StepOutcome::TimedOut | StepOutcome::UntilNotSeen, Target::Dut) => (Verdict::Fail, Some(StepTimedOut)),
        (StepOutcome::RefusedOn | StepOutcome::TimedOut | StepOutcome::UntilNotSeen, Target::Bench) => {
            (Verdict::Error, Some(BenchStepFailed))
        }
        (StepOutcome::EmptyReply, _) => (Verdict::Error, Some(EmptyReply)),
        (StepOutcome::PortLost, _) => (Verdict::Error, Some(PortLost)),
        (StepOutcome::PromptTimedOut { observation } | StepOutcome::PromptCancelled { observation }, _) => {
            if observation {
                (Verdict::Inconclusive, Some(ObservationUnanswered))
            } else {
                (Verdict::Error, Some(PromptNotConfirmed))
            }
        }
    };
    Judged::bare(v, r)
}

fn limit_v1(value: f64, limit: Limit, w: f64, evidence: &Evidence) -> Judged {
    // The evidence decides before the number does: a truncated capture's
    // value is not one to judge a limit by, even when it looks like a fail.
    let order = [Reason::TruncatedCapture, Reason::TruncatedReply, Reason::RecordGap, Reason::ParseFailed];
    for r in order {
        if evidence.incomplete.contains(&r) {
            return Judged::bare(Verdict::Inconclusive, Some(r));
        }
    }
    if !value.is_finite() || !w.is_finite() || w < 0.0 {
        return Judged::bare(Verdict::Inconclusive, Some(Reason::ParseFailed));
    }
    if evidence.calibration_expired {
        return Judged::bare(Verdict::Inconclusive, Some(Reason::CalibrationExpired));
    }
    // Per side: `accept` is the acceptance limit (the tolerance limit moved
    // inward by w), `outer` the tolerance limit moved outward by w.
    let (lo, hi) = match limit {
        Limit::Band { lo, hi } => (lo, hi),
        Limit::AtMost { max } => (f64::NEG_INFINITY, max),
        Limit::AtLeast { min } => (min, f64::INFINITY),
    };
    if value < lo - w || value > hi + w {
        return Judged::bare(Verdict::Fail, Some(Reason::Limit));
    }
    if value >= lo + w && value <= hi - w {
        return Judged::bare(Verdict::Pass, None);
    }
    Judged::bare(Verdict::Inconclusive, Some(Reason::GuardBand))
}

fn width_v1(limit: Limit, w: f64, s: &Source) -> WidthCheck {
    let finite = [w, s.range_min, s.range_max, s.resolution, s.u].iter().all(|x| x.is_finite());
    if !finite || w < 0.0 || s.u < 0.0 || s.resolution < 0.0 || s.range_min > s.range_max {
        return WidthCheck::Malformed;
    }
    let bases = [(WidthBasis::Uncertainty, s.u), (WidthBasis::Resolution, s.resolution)];
    match limit {
        Limit::Band { lo, hi } => {
            if !lo.is_finite() || !hi.is_finite() || lo > hi {
                return WidthCheck::Malformed;
            }
            for (basis, x) in bases {
                let needed = 2.0 * w.max(x);
                if hi - lo <= needed {
                    return WidthCheck::BandTooNarrow { basis, width: hi - lo, needed };
                }
            }
        }
        Limit::AtMost { max: x } | Limit::AtLeast { min: x } => {
            if !x.is_finite() {
                return WidthCheck::Malformed;
            }
            let clearance = (x - s.range_min).min(s.range_max - x);
            for (basis, v) in bases {
                let needed = w.max(v);
                if clearance <= needed {
                    return WidthCheck::LimitTooCloseToRange { basis, clearance, needed };
                }
            }
        }
    }
    WidthCheck::Accepted
}

fn union_qualifiers<'a>(sets: impl Iterator<Item = &'a Vec<Qualifier>>) -> Vec<Qualifier> {
    let mut out: Vec<Qualifier> = sets.flatten().copied().collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn combine_v1(items: &[Judged]) -> Summary {
    let mut counts = Counts::default();
    for i in items {
        counts.add(i.verdict);
    }
    // Nothing to combine means nothing ran: `skipped`, never a bare pass.
    let verdict = items.iter().map(|i| i.verdict).max_by_key(|v| v.severity()).unwrap_or(Verdict::Skipped);
    Summary { verdict, counts, qualifiers: union_qualifiers(items.iter().map(|i| &i.qualifiers)) }
}

fn requirement_v1(tests: &[Judged]) -> Requirement {
    let counted: Vec<&Judged> = tests.iter().filter(|t| !t.qualifiers.contains(&Qualifier::Relayed)).collect();
    let relayed_listed_apart = (tests.len() - counted.len()) as u32;
    let mut counts = Counts::default();
    for t in &counted {
        counts.add(t.verdict);
    }
    let ran: Vec<&&Judged> = counted.iter().filter(|t| t.verdict != Verdict::Skipped).collect();
    let skipped: Vec<Reason> = counted.iter().filter(|t| t.verdict == Verdict::Skipped).filter_map(|t| t.reason).collect();
    // A skipped test with no reason is not known to be a non-gap.
    let unexplained = counted.iter().filter(|t| t.verdict == Verdict::Skipped && t.reason.is_none()).count();
    let gap = unexplained > 0 || skipped.iter().any(|r| r.is_gap());
    let worst = ran.iter().map(|t| t.verdict).max_by_key(|v| v.severity());
    let mut qualifiers = union_qualifiers(ran.iter().map(|t| &t.qualifiers));
    let mut n_of_m = None;
    let verdict = match worst {
        Some(Verdict::Error) => RequirementVerdict::Error,
        Some(Verdict::Fail) => RequirementVerdict::Fail,
        Some(Verdict::Inconclusive) => RequirementVerdict::Inconclusive,
        Some(Verdict::Cancelled) => RequirementVerdict::Cancelled,
        Some(Verdict::Pass) | Some(Verdict::Skipped) | None if gap || ran.is_empty() => RequirementVerdict::NotVerified,
        _ => {
            let not_selected = skipped.iter().filter(|r| **r == Reason::NotSelected).count() as u32;
            if not_selected > 0 {
                let n = ran.len() as u32;
                n_of_m = Some(NOfM { n, m: n + not_selected });
                qualifiers.push(Qualifier::NOfMRun);
                qualifiers.sort_unstable();
                qualifiers.dedup();
            }
            RequirementVerdict::Pass
        }
    };
    Requirement { verdict, counts, qualifiers, n_of_m, skipped, relayed_listed_apart }
}
