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

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::limits::{
    MAX_HOST_EXPANDED_STEPS, MAX_HOST_LINE_COUNTS, MAX_HOST_LINE_LEN, MAX_HOST_NAME_LEN,
    MAX_HOST_PATTERN_LEN, MAX_HOST_PROMPT_LEN, MAX_HOST_REPEAT_COUNT, MAX_HOST_REPEAT_STEPS,
    MAX_HOST_STEPS, MAX_HOST_TEARDOWN_STEPS, MAX_HOST_VALUE_LEN, MAX_STREAMS_PER_STUDY,
};
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
    },
    /// Block until an operator answers, or the timeout.
    Prompt { text: String },
    /// One SCPI line; with `query` the reply is recorded.
    Instrument {
        instrument: String,
        line: String,
        #[serde(default)]
        query: bool,
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
        match &self.action {
            HostAction::Exchange { signal, line: l, until, refused_on, .. } => {
                blank(signal, "signal")?;
                line(l)?;
                blank(until, "until")?;
                if let Some(r) = refused_on {
                    blank(r, "refused_on")?;
                }
            }
            HostAction::Prompt { text } => {
                blank(text, "text")?;
                if text.len() > MAX_HOST_PROMPT_LEN {
                    return Err(E::FieldTooLong { list, index, what: "text" });
                }
            }
            HostAction::Instrument { instrument, line: l, .. } => {
                blank(instrument, "instrument")?;
                line(l)?;
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

    #[test]
    fn streams_are_signal_taps_only() {
        let mut s = pd();
        s.streams[0].source = StreamSource::PowerFrontEnd { sample_hz: 10 };
        assert_eq!(s.validate(), Err(HostStudyError::StreamNotSignal { index: 0 }));
    }
}
