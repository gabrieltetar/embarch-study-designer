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
//! Signal, instrument and prompt names are checked against topology when Core
//! accepts a study, not here.

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::limits::{
    MAX_HOST_LINE_LEN, MAX_HOST_NAME_LEN, MAX_HOST_PROMPT_LEN, MAX_HOST_STEPS,
    MAX_HOST_TEARDOWN_STEPS, MAX_STREAMS_PER_STUDY,
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
}

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
            s.validate(HostList::Steps, i)?;
        }
        for (i, s) in self.teardown.iter().enumerate() {
            s.validate(HostList::Teardown, i)?;
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
        Ok(())
    }
}

impl HostStep {
    fn validate(&self, list: HostList, index: usize) -> Result<(), HostStudyError> {
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
        HostStudy { name: "n".into(), steps, teardown: Vec::new(), streams: Vec::new() }
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

    #[test]
    fn streams_are_signal_taps_only() {
        let mut s = pd();
        s.streams[0].source = StreamSource::PowerFrontEnd { sample_hz: 10 };
        assert_eq!(s.validate(), Err(HostStudyError::StreamNotSignal { index: 0 }));
    }
}
