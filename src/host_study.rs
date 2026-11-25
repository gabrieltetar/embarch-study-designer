//! Host-run studies (decision 80): steps Core executes itself against
//! declared host-side signals and instruments, with an operator prompt.
//!
//! **JSON only, never on the dev-bench wire.** Nothing here is `postcard`ed,
//! sealed or sent to dev-bench, so decision 60 holds for bench studies. Gated
//! on `alloc`: these are host types and carry heap strings.
//!
//! **No action carries an expected value or a range** — a study collects and
//! the engineer interprets. The one thing a step may declare about a reply is
//! a refusal marker (`Exchange::refused_on`), because a refusal means the
//! action was not carried out, which is a failure; a value is never compared.
//! `deny_unknown_fields` is what makes that enforced rather than conventional.
//!
//! **Repeat and line counts (decision 86).** A step may be a [`HostAction::Repeat`]
//! and a study may declare [`LineCount`] views. A count is **shown, never
//! compared**: `LineCount` has no expected value, and `deny_unknown_fields`
//! refuses one. Counting happens post-run in Core, from the raw bytes on disk.
//!
//! Signal, instrument and prompt names are checked against topology when Core
//! accepts a study, not here.
//!
//! **A prompt may name a bench configuration (decision 88).** `Prompt.bench_config`
//! is a name only: no one refuses it at submission, Core forwards it, and the
//! surface showing the prompt resolves it against the bench live there.
//!
//! **A step may read one number out of its reply, and a prompt says its kind
//! (decision 108).** `Exchange.parse` and `Instrument.parse` name a
//! measurement and say how to read it ([`StepParse`]); [`parse_step_number`]
//! is the one function Core and `verify` both call. A parse is still not an
//! expected value: nothing here compares the number with anything. A prompt's
//! [`PromptKind`] decides who may answer it (`notes/hil/12-prevention.md`,
//! "Operator prompts, by kind"); an `action` prompt names the parse that
//! confirms it, and an `observation` prompt declares its answer.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::limits::{
    MAX_HOST_EXPANDED_STEPS, MAX_HOST_LINE_COUNTS, MAX_HOST_LINE_LEN, MAX_HOST_NAME_LEN,
    MAX_HOST_PATTERN_LEN, MAX_HOST_PROMPT_ANSWERS, MAX_HOST_PROMPT_LEN, MAX_HOST_REPEAT_COUNT,
    MAX_HOST_REPEAT_STEPS, MAX_HOST_STEPS, MAX_HOST_TEARDOWN_STEPS, MAX_HOST_VALUE_LEN,
    MAX_MEASUREMENT_NAME_LEN, MAX_STREAMS_PER_STUDY,
};
use crate::measures::UNITS;
use crate::result::Outcome;
use crate::streams::{StreamSource, StreamTap};

/// A host-run study. `streams` holds `Signal` taps only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostStudy {
    pub name: String,
    pub steps: Vec<HostStep>,
    /// Runs on **every** exit path: the end of the run, a failed step without
    /// `continue_on_fail`, a timeout and a cancel. A teardown step that fails
    /// is recorded and the rest still run; `continue_on_fail` has no effect
    /// inside it.
    #[serde(default)]
    pub teardown: Vec<HostStep>,
    #[serde(default)]
    pub streams: Vec<StreamTap>,
    /// Views over `streams`, counted per pass and over the whole run
    /// (decision 86). Never compared with anything.
    #[serde(default)]
    pub line_counts: Vec<LineCount>,
}

/// How a [`LineCount::pattern`] is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PatternKind {
    /// A line matches if it contains the pattern text.
    #[default]
    Substring,
    /// A line matches if the `regex` crate's syntax matches anywhere in it.
    Regex,
}

/// A count of the lines of one tap that match a pattern. A view, not a check:
/// **no field here is an expected value, a minimum or a maximum**.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineCount {
    /// Label shown with the count; unique within the study.
    pub name: String,
    /// The `name` of one of the study's `streams`.
    pub tap: String,
    pub pattern: String,
    #[serde(default)]
    pub kind: PatternKind,
}

impl LineCount {
    /// Whether this view matches `line`. Compiles a `Regex` pattern on every
    /// call; use [`LineCount::count`] for a whole capture.
    #[cfg(feature = "std")]
    pub fn matches(&self, line: &str) -> Result<bool, String> {
        Ok(self.matcher()?.is_match(line))
    }

    /// Number of `lines` this view matches.
    #[cfg(feature = "std")]
    pub fn count<'a>(&self, lines: impl IntoIterator<Item = &'a str>) -> Result<u32, String> {
        let m = self.matcher()?;
        Ok(lines.into_iter().filter(|l| m.is_match(l)).count() as u32)
    }

    #[cfg(feature = "std")]
    fn matcher(&self) -> Result<Matcher, String> {
        match self.kind {
            PatternKind::Substring => Ok(Matcher::Sub(self.pattern.clone())),
            PatternKind::Regex => regex::RegexBuilder::new(&self.pattern)
                .size_limit(1 << 20)
                .build()
                .map(Matcher::Re)
                .map_err(|e| alloc::format!("{e}")),
        }
    }
}

#[cfg(feature = "std")]
enum Matcher {
    Sub(String),
    Re(regex::Regex),
}

#[cfg(feature = "std")]
impl Matcher {
    fn is_match(&self, line: &str) -> bool {
        match self {
            Matcher::Sub(p) => line.contains(p.as_str()),
            Matcher::Re(r) => r.is_match(line),
        }
    }
}

/// The text a `Repeat` substitutes into `Exchange.line` and `Instrument.line`.
/// With `values` it is the entry; with `count` it is the 1-based pass number.
pub const VALUE_PLACEHOLDER: &str = "{value}";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostStep {
    /// A label only; position identifies a step (decision 14).
    pub name: String,
    pub action: HostAction,
    /// Budget for the action itself, not counting `delay_before_ms`.
    /// Must be non-zero for every action but `Wait`.
    #[serde(default)]
    pub timeout_ms: u32,
    #[serde(default)]
    pub continue_on_fail: bool,
    #[serde(default)]
    pub delay_before_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum HostAction {
    /// Write one line to a declared writable signal and read until `until`
    /// or the timeout (embarch-core decision 79). The echo rule is declared.
    Exchange {
        signal: String,
        line: String,
        until: String,
        #[serde(default)]
        match_after_echo: bool,
        /// Decision 80: if the reply contains this text the action was not
        /// carried out and the step is `Fail`. Never a value comparison.
        #[serde(default)]
        refused_on: Option<String>,
        /// The number this step reads out of its reply (decision 108).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parse: Option<StepParse>,
    },
    /// Block until an operator answers, or the timeout.
    Prompt {
        text: String,
        /// The bench configuration this step asks for, by name (decision 88):
        /// what the bench must look like, never which cables. **Resolved by
        /// whoever shows the prompt**, against the bench live there; Core
        /// forwards it unchanged and resolves nothing, and a name the bench
        /// lacks is not refused here or there; only its shape is checked
        /// (non-blank, at most `MAX_HOST_NAME_LEN` bytes, no control character,
        /// line separator, bidi format character or edge whitespace). Skipped
        /// when `None`, so a study written before the field serializes as it
        /// always did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bench_config: Option<String>,
        /// Who may answer it (decision 108). `None` is a study written before
        /// the field; it takes neither `confirmed_by` nor `answer`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<PromptKind>,
        /// An `action` prompt's confirming measurement: the `name` of a
        /// [`StepParse`] in the same study. Required on `action`, refused on
        /// every other kind.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confirmed_by: Option<String>,
        /// What an `observation` prompt's answer is. Required on
        /// `observation`, refused on every other kind.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer: Option<PromptAnswer>,
    },
    /// One SCPI line; with `query` the reply is recorded.
    Instrument {
        instrument: String,
        line: String,
        #[serde(default)]
        query: bool,
        /// The number this step reads out of its reply (decision 108); only a
        /// `query` has a reply to read.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parse: Option<StepParse>,
    },
    Wait { ms: u32 },
    /// Run `steps` `count` times, or once per entry of `values` (exactly one
    /// of the two). `{value}` in an inner `Exchange.line` or
    /// `Instrument.line` is replaced by the entry, or by the 1-based pass
    /// number for `count` (decision 86). One level only: an inner step may
    /// not be a `Repeat`. The step's own `timeout_ms` is unused (the inner
    /// steps carry theirs) and `continue_on_fail` has no effect (each inner
    /// step's own governs); `delay_before_ms` applies once, before pass 1.
    Repeat {
        #[serde(default)]
        count: Option<u32>,
        #[serde(default)]
        values: Vec<String>,
        steps: Vec<HostStep>,
    },
}

/// How a step reads one number out of its reply (decision 108): the first
/// match of `pattern` in the reply, its one capture group read by the crate's
/// number grammar ([`crate::number_grammar`]). Core records the number beside
/// the raw reply at step time and `verify` re-parses it from the recorded
/// reply, both through [`parse_step_number`], so they cannot disagree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepParse {
    /// The measurement's name, unique within the study across every parse and
    /// every observation prompt's answer.
    pub name: String,
    /// A `regex` crate pattern with exactly one capture group.
    pub pattern: String,
    /// One of the reducers' units ([`crate::measures::UNITS`]).
    pub unit: String,
}

/// Who may answer a prompt, by what its answer means
/// (`notes/hil/12-prevention.md`, "Operator prompts, by kind").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    /// A claim the setup is done; the person, or the agent relaying them.
    Setup,
    /// A timing mark (the person acted), confirmed by a measurement.
    Action,
    /// The measurement itself; the person's own hand only.
    Observation,
    /// A person's safety before energising; the person's own hand only.
    Clearance,
}

/// What an `observation` prompt's answer is: a measurement with `by: person`,
/// either one of `answers` or a `number`, exactly one of the two.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptAnswer {
    /// The measurement's name, in the same namespace as [`StepParse::name`].
    pub name: String,
    /// The choices offered, each unique; empty when `number` is given.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<ObservedNumber>,
}

/// A number the person reads, with the resolution and uncertainty the prompt
/// declares for it (`notes/hil/02-study.md`, "a `person` measurement").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedNumber {
    /// One of [`crate::measures::UNITS`].
    pub unit: String,
    /// The smallest step the person reads, in `unit`.
    pub resolution: f64,
    /// Expanded uncertainty `U` of one reading, in `unit`.
    #[serde(rename = "U")]
    pub u_expanded: f64,
    /// Coverage factor of `U`; absent is 2, the reducers' default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k: Option<f64>,
}

#[cfg(feature = "std")]
impl StepParse {
    /// The compiled pattern, or why it is refused: it does not compile, or it
    /// has other than exactly one capture group.
    pub fn regex(&self) -> Result<regex::Regex, String> {
        let re = regex::RegexBuilder::new(&self.pattern)
            .size_limit(1 << 20)
            .build()
            .map_err(|e| alloc::format!("{e}"))?;
        // `captures_len` counts the whole match as group 0.
        match re.captures_len() - 1 {
            1 => Ok(re),
            n => Err(alloc::format!("{n} capture groups; a parse takes exactly one")),
        }
    }
}

/// The number `parse` reads out of `reply` (decision 108): the first match of
/// its pattern, whose capture, trimmed, must be exactly one number in the
/// crate's number grammar. No match, a capture that did not take part, a
/// capture that is not one number, or a pattern the study should not have
/// passed validation with, is [`crate::reducers::Incomplete::ParseFailed`].
#[cfg(all(feature = "std", feature = "reducers"))]
pub fn parse_step_number(reply: &str, parse: &StepParse) -> Result<f64, crate::reducers::Incomplete> {
    use crate::reducers::Incomplete;
    let re = parse.regex().map_err(|_| Incomplete::ParseFailed)?;
    let text = re
        .captures(reply)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim())
        .ok_or(Incomplete::ParseFailed)?;
    match crate::number_grammar::tokens(text).as_slice() {
        [one] if one.text == text => {
            let v = one.value.to_f64();
            if v.is_finite() {
                Ok(v)
            } else {
                Err(Incomplete::ParseFailed)
            }
        }
        _ => Err(Incomplete::ParseFailed),
    }
}

/// One step's result. `reply` is raw text, cut at `MAX_HOST_REPLY_LEN` with
/// `reply_truncated` set rather than cut silently.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostStepResult {
    pub step_name: String,
    pub outcome: Outcome,
    #[serde(default)]
    pub reply: String,
    #[serde(default)]
    pub reply_truncated: bool,
    /// Who answered a `Prompt` (`confirmed`/`cancelled` sources); else `None`.
    #[serde(default)]
    pub answered_by: Option<String>,
    pub elapsed_ms: u64,
    /// Set on a step that ran inside a `Repeat`; else `None`.
    #[serde(default)]
    pub iteration: Option<IterationRef>,
}

/// Which pass of which `Repeat` a [`HostStepResult`] belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IterationRef {
    pub list: HostList,
    /// Index of the `Repeat` step in its list.
    pub repeat_index: u32,
    /// 0-based pass number.
    pub pass: u32,
}

/// One pass boundary, recorded by Core as the pass starts (decision 86). A
/// view buckets a tap's lines by `rx_utc_ms` against these: a line belongs to
/// the last marker of its `Repeat` whose `started_utc_ms` is not after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IterationMarker {
    pub list: HostList,
    pub repeat_index: u32,
    pub pass: u32,
    /// The substituted entry for a `values` repeat; `None` for `count`.
    #[serde(default)]
    pub value: Option<String>,
    /// Same clock as `StreamRecord::rx_utc_ms`.
    pub started_utc_ms: u64,
}

/// What a host study run produced: step results plus pass markers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostStudyResult {
    pub study_name: String,
    #[serde(default)]
    pub steps: Vec<HostStepResult>,
    #[serde(default)]
    pub iterations: Vec<IterationMarker>,
}

/// The pass of `(list, repeat_index)` that `t_ms` falls in, by `markers`
/// (which need not be sorted); `None` before the first pass.
pub fn pass_at(markers: &[IterationMarker], list: HostList, repeat_index: u32, t_ms: u64) -> Option<u32> {
    markers
        .iter()
        .filter(|m| m.list == list && m.repeat_index == repeat_index && m.started_utc_ms <= t_ms)
        .max_by_key(|m| (m.started_utc_ms, m.pass))
        .map(|m| m.pass)
}

/// Which list a [`HostStudyError`] is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostList {
    Steps,
    Teardown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostStudyError {
    EmptyName,
    NameTooLong,
    NoSteps,
    TooManySteps { count: usize },
    TooManyTeardownSteps { count: usize },
    EmptyStepName { list: HostList, index: usize },
    StepNameTooLong { list: HostList, index: usize },
    /// A field that must be non-empty was blank (`what` names it).
    BlankField { list: HostList, index: usize, what: &'static str },
    FieldTooLong { list: HostList, index: usize, what: &'static str },
    /// `line` holds a CR, LF or other control character; the terminator is
    /// declared on the signal or instrument, never written by the study.
    ControlInLine { list: HostList, index: usize },
    /// `Prompt.bench_config` holds a control character, a line or paragraph
    /// separator or a bidi format character, or starts or ends in whitespace
    /// (decision 88): it is printed on one line and later looked up by exact
    /// name, so none of them can be part of a name.
    BadBenchConfig { list: HostList, index: usize },
    ZeroTimeout { list: HostList, index: usize },
    ZeroWait { list: HostList, index: usize },
    TooManyStreams { count: usize },
    StreamIdIsNotItsIndex { index: usize, id: u8 },
    EmptyStreamName { index: usize },
    DuplicateStreamName { index: usize },
    /// `streams` carries `Signal` taps only.
    StreamNotSignal { index: usize },
    /// Exactly one of `count` and `values`; neither or both was given.
    RepeatNeedsCountXorValues { list: HostList, index: usize },
    /// `count` outside `1..=MAX_HOST_REPEAT_COUNT`, or `values` longer than it.
    RepeatCountOutOfRange { list: HostList, index: usize },
    /// A `values` entry is blank, too long or holds a control character.
    BadRepeatValue { list: HostList, index: usize, value_index: usize },
    NoRepeatSteps { list: HostList, index: usize },
    TooManyRepeatSteps { list: HostList, index: usize, count: usize },
    /// An inner step of a `Repeat` is itself a `Repeat`.
    NestedRepeat { list: HostList, index: usize },
    /// The inner step failed its own checks (`source`, indexed within the repeat).
    InRepeat { list: HostList, index: usize, source: Box<HostStudyError> },
    /// `{value}` in a line outside any `Repeat`.
    ValueOutsideRepeat { list: HostList, index: usize },
    /// A repeat, expanded, would run more than `MAX_HOST_EXPANDED_STEPS`
    /// steps, or a substituted line would pass `MAX_HOST_LINE_LEN`.
    RepeatTooLarge { list: HostList, index: usize },
    TooManyLineCounts { count: usize },
    BlankLineCountField { index: usize, what: &'static str },
    LineCountFieldTooLong { index: usize, what: &'static str },
    DuplicateLineCountName { index: usize },
    /// `tap` names no declared stream.
    UnknownLineCountTap { index: usize },
    /// The pattern does not compile (checked with feature `std`).
    BadLineCountPattern { index: usize, reason: String },
    /// A measurement name (a parse's, or an observation prompt's answer's) is
    /// blank, longer than `MAX_MEASUREMENT_NAME_LEN` or holds a control
    /// character (decision 108).
    BadMeasurementName { list: HostList, index: usize },
    /// A measurement name already used by an earlier parse or answer.
    DuplicateMeasurementName { list: HostList, index: usize },
    /// A parse's pattern does not compile, or has other than exactly one
    /// capture group (checked with feature `std`).
    BadParsePattern { list: HostList, index: usize, reason: String },
    /// A unit outside `measures::UNITS` (`what` names the field).
    UnknownUnit { list: HostList, index: usize, what: &'static str },
    /// A `parse` on an `Instrument` step without `query`: there is no reply.
    ParseWithoutReply { list: HostList, index: usize },
    /// A prompt field its kind does not take (`what`: `confirmed_by` or
    /// `answer`), including on a prompt with no kind.
    PromptFieldNotForKind { list: HostList, index: usize, what: &'static str },
    /// An `action` prompt with no `confirmed_by`.
    ActionWithoutConfirmedBy { list: HostList, index: usize },
    /// `confirmed_by` names no parse in the study.
    UnknownConfirmedBy { list: HostList, index: usize },
    /// An `observation` prompt with no `answer`.
    ObservationWithoutAnswer { list: HostList, index: usize },
    /// An answer declaring neither or both of `answers` and `number`, or a
    /// choice that is blank, too long, repeated or holds a control character,
    /// or too many choices, or a `resolution`, `U` or `k` that is not a finite
    /// positive number (`what` names the field).
    BadPromptAnswer { list: HostList, index: usize, what: &'static str },
}

impl HostStudy {
    /// Structural checks only; names are checked against topology by Core.
    pub fn validate(&self) -> Result<(), HostStudyError> {
        use HostStudyError as E;
        if self.name.trim().is_empty() {
            return Err(E::EmptyName);
        }
        if self.name.len() > MAX_HOST_NAME_LEN {
            return Err(E::NameTooLong);
        }
        if self.steps.is_empty() {
            return Err(E::NoSteps);
        }
        if self.steps.len() > MAX_HOST_STEPS {
            return Err(E::TooManySteps { count: self.steps.len() });
        }
        if self.teardown.len() > MAX_HOST_TEARDOWN_STEPS {
            return Err(E::TooManyTeardownSteps { count: self.teardown.len() });
        }
        for (i, s) in self.steps.iter().enumerate() {
            s.validate(HostList::Steps, i, false)?;
        }
        for (i, s) in self.teardown.iter().enumerate() {
            s.validate(HostList::Teardown, i, false)?;
        }
        self.validate_measurements()?;
        if self.streams.len() > MAX_STREAMS_PER_STUDY {
            return Err(E::TooManyStreams { count: self.streams.len() });
        }
        for (index, tap) in self.streams.iter().enumerate() {
            if usize::from(tap.id) != index {
                return Err(E::StreamIdIsNotItsIndex { index, id: tap.id });
            }
            if tap.name.trim().is_empty() {
                return Err(E::EmptyStreamName { index });
            }
            if self.streams[..index].iter().any(|t| t.name == tap.name) {
                return Err(E::DuplicateStreamName { index });
            }
            if !matches!(tap.source, StreamSource::Signal { .. }) {
                return Err(E::StreamNotSignal { index });
            }
        }
        if self.line_counts.len() > MAX_HOST_LINE_COUNTS {
            return Err(E::TooManyLineCounts { count: self.line_counts.len() });
        }
        for (index, lc) in self.line_counts.iter().enumerate() {
            for (what, v, max) in [
                ("name", &lc.name, MAX_HOST_NAME_LEN),
                ("tap", &lc.tap, MAX_HOST_NAME_LEN),
                ("pattern", &lc.pattern, MAX_HOST_PATTERN_LEN),
            ] {
                if v.trim().is_empty() {
                    return Err(E::BlankLineCountField { index, what });
                }
                if v.len() > max {
                    return Err(E::LineCountFieldTooLong { index, what });
                }
            }
            if self.line_counts[..index].iter().any(|l| l.name == lc.name) {
                return Err(E::DuplicateLineCountName { index });
            }
            if !self.streams.iter().any(|t| t.name.as_str() == lc.tap) {
                return Err(E::UnknownLineCountTap { index });
            }
            #[cfg(feature = "std")]
            if let Err(reason) = lc.matcher() {
                return Err(E::BadLineCountPattern { index, reason });
            }
        }
        Ok(())
    }

    /// Decision 108's study-wide half: every measurement name (a parse's, an
    /// observation's answer's) is used once, and an action prompt's
    /// `confirmed_by` names a parse. An inner step of a `Repeat` reports as
    /// [`HostStudyError::InRepeat`], indexed within it, as its own checks do.
    fn validate_measurements(&self) -> Result<(), HostStudyError> {
        use HostStudyError as E;
        let lists = [(HostList::Steps, flatten(&self.steps)), (HostList::Teardown, flatten(&self.teardown))];
        let at = |list: HostList, index: usize, inner: Option<usize>, e: fn(HostList, usize) -> E| match inner {
            None => e(list, index),
            Some(j) => E::InRepeat { list, index, source: Box::new(e(list, j)) },
        };
        let mut names: Vec<&str> = Vec::new();
        let mut parses: Vec<&str> = Vec::new();
        for (list, steps) in &lists {
            for &(index, inner, step) in steps {
                if let Some(name) = measurement_of(step) {
                    if names.contains(&name) {
                        return Err(at(*list, index, inner, |list, index| E::DuplicateMeasurementName { list, index }));
                    }
                    names.push(name);
                    if !matches!(step.action, HostAction::Prompt { .. }) {
                        parses.push(name);
                    }
                }
            }
        }
        for (list, steps) in &lists {
            for &(index, inner, step) in steps {
                if let HostAction::Prompt { confirmed_by: Some(c), .. } = &step.action {
                    if !parses.contains(&c.as_str()) {
                        return Err(at(*list, index, inner, |list, index| E::UnknownConfirmedBy { list, index }));
                    }
                }
            }
        }
        Ok(())
    }
}

/// A character that is not a control character but still breaks a name shown
/// on one line: U+2028 and U+2029 end the line, and the bidi format characters
/// (U+200E, U+200F, U+202A to U+202E, U+2066 to U+2069) reorder what follows,
/// so a name could display as another name it does not equal.
fn breaks_one_line_text(ch: char) -> bool {
    matches!(ch, '\u{2028}' | '\u{2029}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// A measurement name is shown on one line and looked up by exact name, as a
/// `measurements.json` row's `name` is.
fn measurement_name_ok(name: &str) -> bool {
    !name.trim().is_empty()
        && name.trim() == name
        && name.len() <= MAX_MEASUREMENT_NAME_LEN
        && !name.chars().any(|ch| ch.is_control() || breaks_one_line_text(ch))
}

fn finite_positive(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

/// An observation prompt's answer, on its own; its name's uniqueness is the
/// study's check.
fn validate_answer(a: &PromptAnswer, list: HostList, index: usize) -> Result<(), HostStudyError> {
    use HostStudyError as E;
    let bad = |what| Err(E::BadPromptAnswer { list, index, what });
    if !measurement_name_ok(&a.name) {
        return Err(E::BadMeasurementName { list, index });
    }
    match (a.answers.is_empty(), &a.number) {
        (false, None) => {
            if a.answers.len() > MAX_HOST_PROMPT_ANSWERS {
                return bad("answers");
            }
            for (i, c) in a.answers.iter().enumerate() {
                if c.trim().is_empty()
                    || c.len() > MAX_HOST_VALUE_LEN
                    || c.chars().any(char::is_control)
                    || a.answers[..i].contains(c)
                {
                    return bad("answers");
                }
            }
        }
        (true, Some(n)) => {
            if !UNITS.contains(&n.unit.as_str()) {
                return Err(E::UnknownUnit { list, index, what: "answer.number.unit" });
            }
            if !finite_positive(n.resolution) {
                return bad("resolution");
            }
            if !finite_positive(n.u_expanded) {
                return bad("U");
            }
            if n.k.is_some_and(|k| !finite_positive(k)) {
                return bad("k");
            }
        }
        _ => return bad("answers or number"),
    }
    Ok(())
}

/// The measurement a step declares, if any: its parse's name, or an
/// observation prompt's answer's.
fn measurement_of(step: &HostStep) -> Option<&str> {
    match &step.action {
        HostAction::Exchange { parse: Some(p), .. } | HostAction::Instrument { parse: Some(p), .. } => Some(&p.name),
        HostAction::Prompt { answer: Some(a), .. } => Some(&a.name),
        _ => None,
    }
}

/// Every step of `list`, an inner step of a `Repeat` with its index within it.
fn flatten(steps: &[HostStep]) -> Vec<(usize, Option<usize>, &HostStep)> {
    let mut out = Vec::new();
    for (i, s) in steps.iter().enumerate() {
        out.push((i, None, s));
        if let HostAction::Repeat { steps: inner, .. } = &s.action {
            out.extend(inner.iter().enumerate().map(|(j, t)| (i, Some(j), t)));
        }
    }
    out
}

impl HostStep {
    fn validate(&self, list: HostList, index: usize, in_repeat: bool) -> Result<(), HostStudyError> {
        use HostStudyError as E;
        if self.name.trim().is_empty() {
            return Err(E::EmptyStepName { list, index });
        }
        if self.name.len() > MAX_HOST_NAME_LEN {
            return Err(E::StepNameTooLong { list, index });
        }
        let blank = |v: &str, what| {
            if v.trim().is_empty() {
                Err(E::BlankField { list, index, what })
            } else {
                Ok(())
            }
        };
        let line = |v: &str| {
            blank(v, "line")?;
            if v.len() > MAX_HOST_LINE_LEN {
                return Err(E::FieldTooLong { list, index, what: "line" });
            }
            if v.chars().any(char::is_control) {
                return Err(E::ControlInLine { list, index });
            }
            if !in_repeat && v.contains(VALUE_PLACEHOLDER) {
                return Err(E::ValueOutsideRepeat { list, index });
            }
            Ok(())
        };
        let parse_ok = |p: &StepParse| {
            if !measurement_name_ok(&p.name) {
                return Err(E::BadMeasurementName { list, index });
            }
            blank(&p.pattern, "parse.pattern")?;
            if p.pattern.len() > MAX_HOST_PATTERN_LEN {
                return Err(E::FieldTooLong { list, index, what: "parse.pattern" });
            }
            #[cfg(feature = "std")]
            if let Err(reason) = p.regex() {
                return Err(E::BadParsePattern { list, index, reason });
            }
            if !UNITS.contains(&p.unit.as_str()) {
                return Err(E::UnknownUnit { list, index, what: "parse.unit" });
            }
            Ok(())
        };
        match &self.action {
            HostAction::Exchange { signal, line: l, until, refused_on, parse, .. } => {
                blank(signal, "signal")?;
                line(l)?;
                blank(until, "until")?;
                if let Some(r) = refused_on {
                    blank(r, "refused_on")?;
                }
                if let Some(p) = parse {
                    parse_ok(p)?;
                }
            }
            HostAction::Prompt { text, bench_config, kind, confirmed_by, answer } => {
                if confirmed_by.is_some() && *kind != Some(PromptKind::Action) {
                    return Err(E::PromptFieldNotForKind { list, index, what: "confirmed_by" });
                }
                if answer.is_some() && *kind != Some(PromptKind::Observation) {
                    return Err(E::PromptFieldNotForKind { list, index, what: "answer" });
                }
                match kind {
                    Some(PromptKind::Action) => match confirmed_by {
                        // Whether it names a parse is the study's check.
                        Some(c) => blank(c, "confirmed_by")?,
                        None => return Err(E::ActionWithoutConfirmedBy { list, index }),
                    },
                    Some(PromptKind::Observation) => match answer {
                        Some(a) => validate_answer(a, list, index)?,
                        None => return Err(E::ObservationWithoutAnswer { list, index }),
                    },
                    _ => {}
                }
                blank(text, "text")?;
                if text.len() > MAX_HOST_PROMPT_LEN {
                    return Err(E::FieldTooLong { list, index, what: "text" });
                }
                // A blank name is not an omission: absent is the way to name no config.
                if let Some(c) = bench_config {
                    blank(c, "bench_config")?;
                    if c.len() > MAX_HOST_NAME_LEN {
                        return Err(E::FieldTooLong { list, index, what: "bench_config" });
                    }
                    if c.trim() != c || c.chars().any(|ch| ch.is_control() || breaks_one_line_text(ch)) {
                        return Err(E::BadBenchConfig { list, index });
                    }
                }
            }
            HostAction::Instrument { instrument, line: l, query, parse } => {
                blank(instrument, "instrument")?;
                line(l)?;
                if let Some(p) = parse {
                    if !query {
                        return Err(E::ParseWithoutReply { list, index });
                    }
                    parse_ok(p)?;
                }
            }
            HostAction::Wait { ms } => {
                if *ms == 0 {
                    return Err(E::ZeroWait { list, index });
                }
                return Ok(());
            }
            HostAction::Repeat { count, values, steps } => {
                if in_repeat {
                    return Err(E::NestedRepeat { list, index });
                }
                let passes = match (count, values.is_empty()) {
                    (Some(c), true) => {
                        if *c == 0 || *c as usize > MAX_HOST_REPEAT_COUNT {
                            return Err(E::RepeatCountOutOfRange { list, index });
                        }
                        *c as usize
                    }
                    (None, false) => {
                        if values.len() > MAX_HOST_REPEAT_COUNT {
                            return Err(E::RepeatCountOutOfRange { list, index });
                        }
                        for (value_index, v) in values.iter().enumerate() {
                            if v.trim().is_empty()
                                || v.len() > MAX_HOST_VALUE_LEN
                                || v.chars().any(char::is_control)
                            {
                                return Err(E::BadRepeatValue { list, index, value_index });
                            }
                        }
                        values.len()
                    }
                    _ => return Err(E::RepeatNeedsCountXorValues { list, index }),
                };
                if steps.is_empty() {
                    return Err(E::NoRepeatSteps { list, index });
                }
                if steps.len() > MAX_HOST_REPEAT_STEPS {
                    return Err(E::TooManyRepeatSteps { list, index, count: steps.len() });
                }
                if passes * steps.len() > MAX_HOST_EXPANDED_STEPS {
                    return Err(E::RepeatTooLarge { list, index });
                }
                for (i, s) in steps.iter().enumerate() {
                    if matches!(s.action, HostAction::Repeat { .. }) {
                        return Err(E::NestedRepeat { list, index });
                    }
                    s.validate(list, i, true)
                        .map_err(|e| E::InRepeat { list, index, source: Box::new(e) })?;
                }
                // Worst case after substitution must still fit a line.
                let widest = values.iter().map(String::len).max().unwrap_or(10);
                for s in steps {
                    let l = match &s.action {
                        HostAction::Exchange { line, .. } | HostAction::Instrument { line, .. } => line,
                        _ => continue,
                    };
                    let n = l.matches(VALUE_PLACEHOLDER).count();
                    if l.len() - n * VALUE_PLACEHOLDER.len() + n * widest > MAX_HOST_LINE_LEN {
                        return Err(E::RepeatTooLarge { list, index });
                    }
                }
                return Ok(());
            }
        }
        if self.timeout_ms == 0 {
            return Err(E::ZeroTimeout { list, index });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::vec;

    const PD: &str = include_str!("../tests/fixtures/host_pd_source_sequence.json");

    fn pd() -> HostStudy {
        serde_json::from_str(PD).unwrap()
    }

    fn exchange(line: &str) -> HostStep {
        HostStep {
            name: "x".into(),
            action: HostAction::Exchange {
                signal: "s".into(),
                line: line.into(),
                until: "ok".into(),
                match_after_echo: false,
                refused_on: None,
                parse: None,
            },
            timeout_ms: 100,
            continue_on_fail: false,
            delay_before_ms: 0,
        }
    }

    fn study(steps: Vec<HostStep>) -> HostStudy {
        HostStudy {
            name: "n".into(),
            steps,
            teardown: Vec::new(),
            streams: Vec::new(),
            line_counts: Vec::new(),
        }
    }

    #[test]
    fn pd_fixture_validates_with_field_shape() {
        let s = pd();
        s.validate().unwrap();
        assert_eq!(s.steps.len(), 49);
        let waits = s.steps.iter().filter(|x| matches!(x.action, HostAction::Wait { .. })).count();
        let reqs = s.steps.iter().filter(|x| matches!(&x.action,
            HostAction::Exchange { line, .. } if line.starts_with("pd req") || line.starts_with("pd pps"))).count();
        assert_eq!((waits, reqs, 49 - waits - reqs), (18, 14, 17));
        assert_eq!(s.teardown.len(), 2);
        assert!(MAX_HOST_STEPS >= s.steps.len());
    }

    #[test]
    fn round_trips() {
        let s = pd();
        let back: HostStudy = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back);
        let r = HostStepResult {
            step_name: "p".into(),
            outcome: Outcome::TimedOut,
            reply: "pd: ok".into(),
            reply_truncated: false,
            answered_by: Some("ui".into()),
            elapsed_ms: 7,
            iteration: Some(IterationRef { list: HostList::Steps, repeat_index: 1, pass: 2 }),
        };
        let back: HostStepResult = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn unknown_fields_are_refused_so_no_expected_value_can_ride() {
        for extra in ["expect", "min", "max", "expected_value", "tolerance"] {
            let mut v: serde_json::Value = serde_json::from_str(PD).unwrap();
            v["steps"][2]["action"]["Exchange"][extra] = serde_json::json!("5.0");
            assert!(serde_json::from_value::<HostStudy>(v).is_err(), "{extra}");
            let mut v: serde_json::Value = serde_json::from_str(PD).unwrap();
            v["steps"][2][extra] = serde_json::json!(1);
            assert!(serde_json::from_value::<HostStudy>(v).is_err(), "{extra}");
        }
        let mut v: serde_json::Value = serde_json::from_str(PD).unwrap();
        v["bogus"] = serde_json::json!(1);
        assert!(serde_json::from_value::<HostStudy>(v).is_err());
    }

    #[test]
    fn teardown_is_validated_like_steps() {
        let mut s = pd();
        s.teardown.push(exchange("bad\nline"));
        assert_eq!(
            s.validate(),
            Err(HostStudyError::ControlInLine { list: HostList::Teardown, index: 2 })
        );
        let mut s = pd();
        s.teardown = (0..=MAX_HOST_TEARDOWN_STEPS).map(|_| exchange("a")).collect();
        assert!(matches!(s.validate(), Err(HostStudyError::TooManyTeardownSteps { .. })));
    }

    #[test]
    fn structural_refusals() {
        assert_eq!(study(vec![]).validate(), Err(HostStudyError::NoSteps));
        let mut st = exchange("a");
        st.timeout_ms = 0;
        assert!(matches!(study(vec![st]).validate(), Err(HostStudyError::ZeroTimeout { .. })));
        let w = HostStep {
            name: "w".into(),
            action: HostAction::Wait { ms: 0 },
            timeout_ms: 0,
            continue_on_fail: false,
            delay_before_ms: 0,
        };
        assert!(matches!(study(vec![w]).validate(), Err(HostStudyError::ZeroWait { .. })));
        let many = (0..=MAX_HOST_STEPS).map(|_| exchange("a")).collect();
        assert!(matches!(study(many).validate(), Err(HostStudyError::TooManySteps { .. })));
        let mut blank_marker = exchange("a");
        if let HostAction::Exchange { refused_on, .. } = &mut blank_marker.action {
            *refused_on = Some(" ".into());
        }
        assert!(matches!(study(vec![blank_marker]).validate(), Err(HostStudyError::BlankField { .. })));
    }

    const HR: &str = include_str!("../tests/fixtures/host_pd_hard_reset_counts.json");

    fn hr() -> HostStudy {
        serde_json::from_str(HR).unwrap()
    }

    fn repeat(count: Option<u32>, values: &[&str], steps: Vec<HostStep>) -> HostStep {
        HostStep {
            name: "r".into(),
            action: HostAction::Repeat {
                count,
                values: values.iter().map(|v| (*v).into()).collect(),
                steps,
            },
            timeout_ms: 0,
            continue_on_fail: false,
            delay_before_ms: 0,
        }
    }

    #[test]
    fn hard_reset_fixture_validates_with_counts_on_two_taps() {
        let s = hr();
        s.validate().unwrap();
        assert_eq!(s.streams.len(), 2);
        assert_eq!(s.line_counts.len(), 8);
        for tap in ["pd-bench console", "dut console"] {
            for p in ["Hard Reset", "contract in force", "rx overrun", "rx message error"] {
                assert!(s.line_counts.iter().any(|l| l.tap == tap && l.pattern == p));
            }
        }
        assert!(matches!(&s.steps[1].action, HostAction::Repeat { count: Some(8), .. }));
        let back: HostStudy = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn repeat_needs_exactly_one_of_count_and_values() {
        let body = || vec![exchange("a {value}")];
        for (c, v) in [(None, &[][..]), (Some(2), &["1"][..])] {
            assert!(matches!(
                study(vec![repeat(c, v, body())]).validate(),
                Err(HostStudyError::RepeatNeedsCountXorValues { .. })
            ));
        }
        assert!(study(vec![repeat(Some(1), &[], body())]).validate().is_ok());
        assert!(study(vec![repeat(None, &["5", "9"], body())]).validate().is_ok());
        for c in [0, MAX_HOST_REPEAT_COUNT as u32 + 1] {
            assert!(matches!(
                study(vec![repeat(Some(c), &[], body())]).validate(),
                Err(HostStudyError::RepeatCountOutOfRange { .. })
            ));
        }
        assert!(matches!(
            study(vec![repeat(None, &["5", " "], body())]).validate(),
            Err(HostStudyError::BadRepeatValue { value_index: 1, .. })
        ));
        assert!(matches!(
            study(vec![repeat(Some(2), &[], vec![])]).validate(),
            Err(HostStudyError::NoRepeatSteps { .. })
        ));
    }

    #[test]
    fn repeat_is_one_level_and_bounded() {
        let inner = repeat(Some(2), &[], vec![exchange("a")]);
        assert!(matches!(
            study(vec![repeat(Some(2), &[], vec![inner])]).validate(),
            Err(HostStudyError::NestedRepeat { .. })
        ));
        let big = repeat(Some(MAX_HOST_REPEAT_COUNT as u32), &[], vec![exchange("a"); 6]);
        assert!(matches!(study(vec![big]).validate(), Err(HostStudyError::RepeatTooLarge { .. })));
        let mut bad = exchange("a");
        bad.timeout_ms = 0;
        assert!(matches!(
            study(vec![repeat(Some(2), &[], vec![bad])]).validate(),
            Err(HostStudyError::InRepeat { source, .. })
                if matches!(*source, HostStudyError::ZeroTimeout { .. })
        ));
        let long = "x".repeat(MAX_HOST_LINE_LEN - 7) + "{value}";
        assert!(matches!(
            study(vec![repeat(None, &["123456789"], vec![exchange(&long)])]).validate(),
            Err(HostStudyError::RepeatTooLarge { .. })
        ));
    }

    #[test]
    fn value_placeholder_is_refused_outside_a_repeat() {
        assert!(matches!(
            study(vec![exchange("pd pps {value}")]).validate(),
            Err(HostStudyError::ValueOutsideRepeat { .. })
        ));
        let mut s = study(vec![exchange("a")]);
        s.teardown.push(exchange("pd pps {value}"));
        assert!(matches!(
            s.validate(),
            Err(HostStudyError::ValueOutsideRepeat { list: HostList::Teardown, .. })
        ));
    }

    #[test]
    fn line_count_checks_tap_and_name() {
        let mut s = hr();
        s.line_counts[0].tap = "nope".into();
        assert_eq!(s.validate(), Err(HostStudyError::UnknownLineCountTap { index: 0 }));
        let mut s = hr();
        s.line_counts[1].name = s.line_counts[0].name.clone();
        assert_eq!(s.validate(), Err(HostStudyError::DuplicateLineCountName { index: 1 }));
        let mut s = hr();
        s.line_counts[0].pattern = " ".into();
        assert!(matches!(s.validate(), Err(HostStudyError::BlankLineCountField { .. })));
    }

    #[test]
    fn line_count_refuses_an_expected_value() {
        for extra in ["expected", "min", "max", "expect", "tolerance"] {
            let mut v: serde_json::Value = serde_json::from_str(HR).unwrap();
            v["line_counts"][0][extra] = serde_json::json!(8);
            assert!(serde_json::from_value::<HostStudy>(v).is_err(), "{extra}");
        }
    }

    #[cfg(feature = "std")]
    #[test]
    fn regex_compiles_at_validate_and_counts() {
        let mut s = hr();
        s.line_counts[0].kind = PatternKind::Regex;
        s.line_counts[0].pattern = "rx (overrun|error)".into();
        s.validate().unwrap();
        let lines = ["rx overrun", "ok", "rx error x", "Hard Reset"];
        assert_eq!(s.line_counts[0].count(lines).unwrap(), 2);
        assert!(!s.line_counts[1].matches("nothing").unwrap());
        assert_eq!(s.line_counts[3].count(["rx message error"]).unwrap(), 1);
        s.line_counts[0].pattern = "rx (overrun".into();
        assert!(matches!(s.validate(), Err(HostStudyError::BadLineCountPattern { index: 0, .. })));
    }

    #[test]
    fn passes_bucket_by_marker() {
        let m = |pass, t| IterationMarker {
            list: HostList::Steps,
            repeat_index: 1,
            pass,
            value: None,
            started_utc_ms: t,
        };
        let ms = [m(1, 200), m(0, 100), m(2, 300)];
        assert_eq!(pass_at(&ms, HostList::Steps, 1, 99), None);
        assert_eq!(pass_at(&ms, HostList::Steps, 1, 250), Some(1));
        assert_eq!(pass_at(&ms, HostList::Steps, 1, 9999), Some(2));
        assert_eq!(pass_at(&ms, HostList::Steps, 0, 250), None);
        let r = HostStudyResult { study_name: "n".into(), steps: vec![], iterations: ms.to_vec() };
        let back: HostStudyResult = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(r, back);
    }

    // ---- a prompt naming a bench configuration (decision 88) ----

    /// A study exactly as the crate serialized it before `bench_config`
    /// existed (`HOST_TYPE_SCHEMA_VERSION` 20): no such key anywhere.
    const BEFORE_BENCH_CONFIG: &str = r#"{"name":"pd","steps":[{"name":"plug","action":{"Prompt":{"text":"plug in the pack"}},"timeout_ms":60000,"continue_on_fail":false,"delay_before_ms":0}],"teardown":[{"name":"unplug","action":{"Prompt":{"text":"unplug the pack"}},"timeout_ms":60000,"continue_on_fail":false,"delay_before_ms":0}],"streams":[],"line_counts":[]}"#;

    fn prompt(text: &str, bench_config: Option<&str>) -> HostStep {
        HostStep {
            name: "p".into(),
            action: HostAction::Prompt {
                text: text.into(),
                bench_config: bench_config.map(Into::into),
                kind: None,
                confirmed_by: None,
                answer: None,
            },
            timeout_ms: 60_000,
            continue_on_fail: false,
            delay_before_ms: 0,
        }
    }

    #[test]
    fn a_study_written_before_bench_config_loads_and_serializes_byte_identical() {
        let s: HostStudy = serde_json::from_str(BEFORE_BENCH_CONFIG).unwrap();
        s.validate().unwrap();
        for step in s.steps.iter().chain(&s.teardown) {
            assert!(matches!(&step.action, HostAction::Prompt { bench_config: None, .. }));
        }
        assert_eq!(serde_json::to_string(&s).unwrap(), BEFORE_BENCH_CONFIG);
        // The two field fixtures predate it too, and still load and validate.
        pd().validate().unwrap();
        hr().validate().unwrap();
    }

    #[test]
    fn a_prompt_naming_a_bench_config_round_trips() {
        let s = study(vec![prompt("move the analyzer in line", Some("pd analyzer in line"))]);
        s.validate().unwrap();
        let json = serde_json::to_string(&s).unwrap();
        assert!(
            json.contains(r#""Prompt":{"text":"move the analyzer in line","bench_config":"pd analyzer in line"}"#),
            "{json}"
        );
        let back: HostStudy = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
        // A sibling key the type does not declare is still refused.
        let mut v = serde_json::to_value(&s).unwrap();
        v["steps"][0]["action"]["Prompt"]["config"] = serde_json::json!("x");
        assert!(serde_json::from_value::<HostStudy>(v).is_err());
    }

    #[test]
    fn a_bench_config_name_is_checked_for_shape_only() {
        let what = "bench_config";
        assert_eq!(
            study(vec![prompt("t", Some(" "))]).validate(),
            Err(HostStudyError::BlankField { list: HostList::Steps, index: 0, what })
        );
        let long = "c".repeat(MAX_HOST_NAME_LEN + 1);
        assert_eq!(
            study(vec![prompt("t", Some(&long))]).validate(),
            Err(HostStudyError::FieldTooLong { list: HostList::Steps, index: 0, what })
        );
        assert!(study(vec![prompt("t", Some(&long[1..]))]).validate().is_ok());
        // One line, matched by exact name later: no control character, no edge whitespace.
        let mut bad: Vec<String> = [" pd", "pd ", "pd\tanalyzer", "pd\nanalyzer", "pd\u{7f}"].map(String::from).to_vec();
        // Line and paragraph separators, and every bidi format character.
        let format_chars = ['\u{2028}', '\u{2029}', '\u{200E}', '\u{200F}']
            .into_iter()
            .chain('\u{202A}'..='\u{202E}')
            .chain('\u{2066}'..='\u{2069}');
        bad.extend(format_chars.map(|ch| format!("pd{ch}analyzer")));
        assert_eq!(bad.len(), 5 + 13);
        for bad in &bad {
            assert_eq!(
                study(vec![prompt("t", Some(bad))]).validate(),
                Err(HostStudyError::BadBenchConfig { list: HostList::Steps, index: 0 }),
                "{bad:?}"
            );
        }
        // Not a format character: a zero-width joiner and a non-ASCII letter stay allowed.
        assert!(study(vec![prompt("t", Some("pd\u{200D}analyzer \u{e9}"))]).validate().is_ok());
        assert!(study(vec![prompt("t", Some("pd analyzer, in line"))]).validate().is_ok());
        // Checked the same in the teardown and inside a `Repeat`.
        let mut s = study(vec![prompt("t", None)]);
        s.teardown.push(prompt("t", Some("")));
        assert_eq!(s.validate(), Err(HostStudyError::BlankField { list: HostList::Teardown, index: 0, what }));
        assert!(matches!(
            study(vec![repeat(Some(2), &[], vec![prompt("t", Some("\t"))])]).validate(),
            Err(HostStudyError::InRepeat { source, .. })
                if *source == HostStudyError::BlankField { list: HostList::Steps, index: 0, what }
        ));
        // A name is never looked up here: there is no bench to look it up in.
        assert!(study(vec![prompt("t", Some("a config no bench has"))]).validate().is_ok());
    }

    #[test]
    fn streams_are_signal_taps_only() {
        let mut s = pd();
        s.streams[0].source = StreamSource::PowerFrontEnd { sample_hz: 10 };
        assert_eq!(s.validate(), Err(HostStudyError::StreamNotSignal { index: 0 }));
    }

    // ---- a step-time parse and a prompt's kind (decision 108) ----

    fn vbus() -> StepParse {
        StepParse { name: "vbus".into(), pattern: r"vbus: (\S+) mV".into(), unit: "mV".into() }
    }

    fn parsed(parse: StepParse) -> HostStep {
        let mut s = exchange("pd status");
        if let HostAction::Exchange { parse: p, .. } = &mut s.action {
            *p = Some(parse);
        }
        s
    }

    fn kinded(kind: PromptKind, confirmed_by: Option<&str>, answer: Option<PromptAnswer>) -> HostStep {
        let mut s = prompt("do it", None);
        s.action = HostAction::Prompt {
            text: "do it".into(),
            bench_config: None,
            kind: Some(kind),
            confirmed_by: confirmed_by.map(Into::into),
            answer,
        };
        s
    }

    fn choices(name: &str, answers: &[&str]) -> PromptAnswer {
        PromptAnswer { name: name.into(), answers: answers.iter().map(|a| (*a).into()).collect(), number: None }
    }

    #[cfg(all(feature = "std", feature = "reducers"))]
    #[test]
    fn a_reply_and_a_pattern_with_one_group_give_the_number() {
        let reply = "pd status\r\nvbus: 5012 mV\r\nibus: 1.25 A\r\nuart:~$ ";
        assert_eq!(parse_step_number(reply, &vbus()), Ok(5012.0));
        let ibus = StepParse { name: "ibus".into(), pattern: r"ibus:\s*([^ ]+) A".into(), unit: "A".into() };
        assert_eq!(parse_step_number(reply, &ibus), Ok(1.25));
        let neg = StepParse { name: "t".into(), pattern: r"t=(\S+)".into(), unit: "1".into() };
        assert_eq!(parse_step_number("t=-2.5e-3", &neg), Ok(-0.0025));
    }

    #[cfg(all(feature = "std", feature = "reducers"))]
    #[test]
    fn no_match_and_a_capture_that_is_not_one_number_are_parse_failed() {
        use crate::reducers::Incomplete::ParseFailed;
        assert_eq!(parse_step_number("pd: error no partner", &vbus()), Err(ParseFailed));
        assert_eq!(parse_step_number("vbus: n/a mV", &vbus()), Err(ParseFailed));
        assert_eq!(parse_step_number("vbus: 5012V mV", &vbus()), Err(ParseFailed));
        let two = StepParse { name: "v".into(), pattern: r"v=(.*);".into(), unit: "V".into() };
        assert_eq!(parse_step_number("v=1 2;", &two), Err(ParseFailed));
        let optional = StepParse { name: "v".into(), pattern: r"v=(\d+)?;".into(), unit: "V".into() };
        assert_eq!(parse_step_number("v=;", &optional), Err(ParseFailed));
    }

    #[cfg(feature = "std")]
    #[test]
    fn a_parse_is_refused_for_its_pattern_unit_or_name() {
        let refused = |p: StepParse| study(vec![parsed(p)]).validate();
        for bad in [r"vbus: \S+ mV", r"(\d+)\.(\d+)", r"vbus: (\S+"] {
            let r = refused(StepParse { pattern: bad.into(), ..vbus() });
            assert!(matches!(r, Err(HostStudyError::BadParsePattern { index: 0, .. })), "{bad}: {r:?}");
        }
        assert_eq!(
            refused(StepParse { unit: "volts".into(), ..vbus() }),
            Err(HostStudyError::UnknownUnit { list: HostList::Steps, index: 0, what: "parse.unit" })
        );
        assert_eq!(
            refused(StepParse { name: " vbus".into(), ..vbus() }),
            Err(HostStudyError::BadMeasurementName { list: HostList::Steps, index: 0 })
        );
        let mut s = exchange("x");
        s.action = HostAction::Instrument { instrument: "dmm".into(), line: "MEAS?".into(), query: false, parse: Some(vbus()) };
        assert_eq!(study(vec![s]).validate(), Err(HostStudyError::ParseWithoutReply { list: HostList::Steps, index: 0 }));
    }

    #[test]
    fn a_measurement_name_is_used_once_across_parses_and_answers() {
        let s = study(vec![parsed(vbus()), parsed(vbus())]);
        assert_eq!(s.validate(), Err(HostStudyError::DuplicateMeasurementName { list: HostList::Steps, index: 1 }));
        let s = study(vec![parsed(vbus()), kinded(PromptKind::Observation, None, Some(choices("vbus", &["yes", "no"])))]);
        assert_eq!(s.validate(), Err(HostStudyError::DuplicateMeasurementName { list: HostList::Steps, index: 1 }));
        let mut s = study(vec![parsed(vbus())]);
        s.teardown.push(parsed(vbus()));
        assert_eq!(s.validate(), Err(HostStudyError::DuplicateMeasurementName { list: HostList::Teardown, index: 0 }));
    }

    #[test]
    fn an_action_prompt_names_a_parse_of_the_same_study() {
        let ok = study(vec![kinded(PromptKind::Action, Some("vbus"), None), parsed(vbus())]);
        ok.validate().unwrap();
        let s = study(vec![kinded(PromptKind::Action, Some("ibus"), None), parsed(vbus())]);
        assert_eq!(s.validate(), Err(HostStudyError::UnknownConfirmedBy { list: HostList::Steps, index: 0 }));
        // An observation's answer is a measurement, but not one that confirms an action.
        let s = study(vec![
            kinded(PromptKind::Observation, None, Some(choices("led", &["green", "red"]))),
            kinded(PromptKind::Action, Some("led"), None),
        ]);
        assert_eq!(s.validate(), Err(HostStudyError::UnknownConfirmedBy { list: HostList::Steps, index: 1 }));
        assert_eq!(
            study(vec![kinded(PromptKind::Action, None, None)]).validate(),
            Err(HostStudyError::ActionWithoutConfirmedBy { list: HostList::Steps, index: 0 })
        );
        assert_eq!(
            study(vec![kinded(PromptKind::Setup, Some("vbus"), None), parsed(vbus())]).validate(),
            Err(HostStudyError::PromptFieldNotForKind { list: HostList::Steps, index: 0, what: "confirmed_by" })
        );
    }

    #[test]
    fn an_action_prompt_inside_a_repeat_reports_within_it() {
        let mut r = exchange("x");
        r.action = HostAction::Repeat { count: Some(2), values: Vec::new(), steps: vec![kinded(PromptKind::Action, Some("nope"), None)] };
        assert_eq!(
            study(vec![r]).validate(),
            Err(HostStudyError::InRepeat {
                list: HostList::Steps,
                index: 0,
                source: Box::new(HostStudyError::UnknownConfirmedBy { list: HostList::Steps, index: 0 }),
            })
        );
    }

    #[test]
    fn an_observation_prompt_declares_choices_or_a_number() {
        let check = |a: Option<PromptAnswer>| study(vec![kinded(PromptKind::Observation, None, a)]).validate();
        check(Some(choices("led", &["green", "red"]))).unwrap();
        let number = |unit: &str, resolution: f64, k: Option<f64>| PromptAnswer {
            name: "meter".into(),
            answers: Vec::new(),
            number: Some(ObservedNumber { unit: unit.into(), resolution, u_expanded: 0.02, k }),
        };
        check(Some(number("V", 0.01, None))).unwrap();
        let bad = |what| Err(HostStudyError::BadPromptAnswer { list: HostList::Steps, index: 0, what });
        assert_eq!(check(None), Err(HostStudyError::ObservationWithoutAnswer { list: HostList::Steps, index: 0 }));
        assert_eq!(check(Some(choices("led", &[]))), bad("answers or number"));
        let mut both = number("V", 0.01, None);
        both.answers = vec!["green".into()];
        assert_eq!(check(Some(both)), bad("answers or number"));
        assert_eq!(check(Some(choices("led", &["green", "green"]))), bad("answers"));
        assert_eq!(check(Some(number("V", 0.0, None))), bad("resolution"));
        assert_eq!(check(Some(number("V", 0.01, Some(f64::NAN)))), bad("k"));
        assert_eq!(
            check(Some(number("volts", 0.01, None))),
            Err(HostStudyError::UnknownUnit { list: HostList::Steps, index: 0, what: "answer.number.unit" })
        );
        assert_eq!(
            study(vec![kinded(PromptKind::Clearance, None, Some(choices("led", &["on"])))]).validate(),
            Err(HostStudyError::PromptFieldNotForKind { list: HostList::Steps, index: 0, what: "answer" })
        );
    }

    #[test]
    fn a_prompt_kind_reads_and_writes_in_lower_case() {
        let s = kinded(PromptKind::Observation, None, Some(choices("led", &["green"])));
        let j = serde_json::to_string(&s.action).unwrap();
        assert!(j.contains(r#""kind":"observation""#), "{j}");
        assert!(j.contains(r#""answer":{"name":"led","answers":["green"]}"#), "{j}");
        let n = ObservedNumber { unit: "V".into(), resolution: 0.01, u_expanded: 0.02, k: None };
        assert_eq!(serde_json::to_string(&n).unwrap(), r#"{"unit":"V","resolution":0.01,"U":0.02}"#);
    }

    /// A study exactly as the crate serialized it at `HOST_TYPE_SCHEMA_VERSION`
    /// 24: an exchange, a query and a prompt, none with a decision 108 field.
    const BEFORE_PARSE: &str = r#"{"name":"pd","steps":[{"name":"req","action":{"Exchange":{"signal":"pd","line":"pd status","until":"uart:~$ ","match_after_echo":true,"refused_on":"pd: error"}},"timeout_ms":3000,"continue_on_fail":false,"delay_before_ms":0},{"name":"read","action":{"Instrument":{"instrument":"dmm","line":"MEAS:VOLT?","query":true}},"timeout_ms":3000,"continue_on_fail":false,"delay_before_ms":0},{"name":"plug","action":{"Prompt":{"text":"plug in the pack","bench_config":"pd-sink"}},"timeout_ms":60000,"continue_on_fail":false,"delay_before_ms":0}],"teardown":[],"streams":[],"line_counts":[]}"#;

    #[test]
    fn a_study_written_before_the_parse_loads_validates_and_serializes_byte_identical() {
        let s: HostStudy = serde_json::from_str(BEFORE_PARSE).unwrap();
        s.validate().unwrap();
        assert!(s.steps.iter().all(|step| measurement_of(step).is_none()));
        assert!(matches!(&s.steps[2].action, HostAction::Prompt { kind: None, confirmed_by: None, answer: None, .. }));
        assert_eq!(serde_json::to_string(&s).unwrap(), BEFORE_PARSE);
        assert_eq!(crate::HOST_TYPE_SCHEMA_VERSION, 25);
    }
}
