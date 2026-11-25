//! A session around a capture: a Start prompt with run fields the study
//! declares, a Done prompt, a time cap and an export folder (EMB-474,
//! decision 106; the owner's EMB-469 decision 10).
//!
//! **A dev-bench `Study` could not hold a prompt.** `Action` has no prompt
//! variant and the dev-bench wire has no message for one; `HostAction::Prompt`
//! lives only in a [`crate::host_study::HostStudy`], whose taps are `Signal`
//! taps only. So a session is not a step: it is a **host-only declaration on
//! `Study`**, like `record_checks` and `measurements` — never on the dev-bench
//! wire, sealed by neither CRC. Core shows the Start prompt before the first
//! step, collects the answers, runs the study, and shows the Done prompt when
//! the capture ends or the cap is reached.
//!
//! **No prompt kind.** The HIL run file's `prompt_kinds` (`setup`, `action`,
//! `observation`, `clearance`; `interfaces/hil-run.md`) is a design only: no
//! type for it exists in this crate. The Start prompt is a setup and the Done
//! prompt a clearance by what they are, and neither asks a person to observe
//! anything, so neither carries a kind field to be wrong.

use heapless::{String, Vec};
use serde::{Deserialize, Serialize};

use crate::limits::{
    MAX_EXPORT_DIR_LEN, MAX_RUN_FIELDS, MAX_RUN_FIELD_LABEL_LEN, MAX_RUN_FIELD_NAME_LEN,
    MAX_RUN_VALUE_LEN, MAX_SESSION_MS, MAX_SESSION_PROMPT_LEN,
};

/// One free-form value the Start prompt asks for (a sensor serial, a room, a
/// note). Free text: this crate never interprets, types or ranges an answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunField {
    /// The key the answer is stored under; unique within the session.
    pub name: String<MAX_RUN_FIELD_NAME_LEN>,
    /// What the person is asked.
    pub label: String<MAX_RUN_FIELD_LABEL_LEN>,
    /// Whether the run may not start with this left blank.
    pub required: bool,
}

/// The session a study declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSpec {
    /// Shown before the first step.
    pub start_text: String<MAX_SESSION_PROMPT_LEN>,
    /// The run fields the Start prompt collects, declared with the study and
    /// so saved with it. The answers are recorded as [`RunFieldValue`]s.
    pub run_fields: Vec<RunField, MAX_RUN_FIELDS>,
    /// Shown when the capture ends or the cap is reached.
    pub done_text: String<MAX_SESSION_PROMPT_LEN>,
    /// The longest the sampling may run, in milliseconds: `1..=`
    /// [`MAX_SESSION_MS`] (5 minutes).
    pub max_duration_ms: u32,
    /// The folder the study's exports are written to. Named by the study,
    /// resolved by Core: a relative or absolute path with no `..` component.
    pub export_dir: String<MAX_EXPORT_DIR_LEN>,
}

/// One answered run field, as Core records it with the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFieldValue {
    pub name: String<MAX_RUN_FIELD_NAME_LEN>,
    pub value: String<MAX_RUN_VALUE_LEN>,
}

/// Why a [`SessionSpec`] is unusable, or an answer does not fit it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    /// The Start or Done text is blank.
    BlankPromptText { which: &'static str },
    /// `max_duration_ms` is zero or over [`MAX_SESSION_MS`].
    DurationOutOfRange { max_duration_ms: u32 },
    /// A run field's name or label is blank, or its name holds a control character.
    BadRunField { index: usize },
    /// Two run fields share a name.
    RepeatedRunField { index: usize, first: usize },
    /// `export_dir` is blank, holds a control character, or has a `..` component.
    BadExportDir,
    /// An answer names a field the session does not declare.
    UnknownRunField { index: usize },
    /// Two answers name one field.
    RepeatedAnswer { index: usize },
    /// A required field has no answer, or a blank one.
    MissingRequired { field: usize },
}

impl core::fmt::Display for SessionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SessionError::BlankPromptText { which } => {
                write!(f, "the session's {which} text is blank, so the person would see an empty prompt")
            }
            SessionError::DurationOutOfRange { max_duration_ms } => write!(
                f,
                "the session's max_duration_ms is {max_duration_ms}; it must be 1 to {MAX_SESSION_MS} (5 minutes)"
            ),
            SessionError::BadRunField { index } => write!(
                f,
                "run_fields[{index}] has a blank name or label, or a control character in its name"
            ),
            SessionError::RepeatedRunField { index, first } => {
                write!(f, "run_fields[{index}] repeats the name of run_fields[{first}]")
            }
            SessionError::BadExportDir => write!(
                f,
                "the session's export_dir is blank, holds a control character or has a '..' component"
            ),
            SessionError::UnknownRunField { index } => {
                write!(f, "answer {index} names a run field the session does not declare")
            }
            SessionError::RepeatedAnswer { index } => {
                write!(f, "answer {index} answers a run field an earlier answer already did")
            }
            SessionError::MissingRequired { field } => {
                write!(f, "run_fields[{field}] is required and has no answer")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for SessionError {}

fn has_control(s: &str) -> bool {
    s.chars().any(char::is_control)
}

/// Pre-flight validation of a declared session.
pub fn validate_session(session: &SessionSpec) -> Result<(), SessionError> {
    if session.start_text.trim().is_empty() {
        return Err(SessionError::BlankPromptText { which: "start" });
    }
    if session.done_text.trim().is_empty() {
        return Err(SessionError::BlankPromptText { which: "done" });
    }
    if session.max_duration_ms == 0 || session.max_duration_ms > MAX_SESSION_MS {
        return Err(SessionError::DurationOutOfRange { max_duration_ms: session.max_duration_ms });
    }
    for (index, field) in session.run_fields.iter().enumerate() {
        if field.name.trim().is_empty() || field.label.trim().is_empty() || has_control(&field.name) {
            return Err(SessionError::BadRunField { index });
        }
        if let Some(first) = session.run_fields[..index].iter().position(|f| f.name == field.name) {
            return Err(SessionError::RepeatedRunField { index, first });
        }
    }
    let dir = session.export_dir.as_str();
    if dir.trim().is_empty() || has_control(dir) || dir.split(['/', '\\']).any(|part| part == "..") {
        return Err(SessionError::BadExportDir);
    }
    Ok(())
}

/// Checks the answers a person gave against the declared fields: every name
/// declared, none twice, every required field answered with something. An
/// answer's text is never judged.
pub fn validate_run_answers(session: &SessionSpec, answers: &[RunFieldValue]) -> Result<(), SessionError> {
    for (index, answer) in answers.iter().enumerate() {
        if !session.run_fields.iter().any(|f| f.name == answer.name) {
            return Err(SessionError::UnknownRunField { index });
        }
        if answers[..index].iter().any(|a| a.name == answer.name) {
            return Err(SessionError::RepeatedAnswer { index });
        }
    }
    for (field, declared) in session.run_fields.iter().enumerate() {
        if declared.required
            && !answers.iter().any(|a| a.name == declared.name && !a.value.trim().is_empty())
        {
            return Err(SessionError::MissingRequired { field });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s<const N: usize>(t: &str) -> String<N> {
        String::try_from(t).unwrap()
    }

    fn field(name: &str, required: bool) -> RunField {
        RunField { name: s(name), label: s("label"), required }
    }

    fn session() -> SessionSpec {
        let mut run_fields = Vec::new();
        run_fields.push(field("sensor", true)).unwrap();
        run_fields.push(field("note", false)).unwrap();
        SessionSpec {
            start_text: s("Place the sensor, then press OK"),
            run_fields,
            done_text: s("Recording finished"),
            max_duration_ms: 300_000,
            export_dir: s("exports/mic"),
        }
    }

    fn answer(name: &str, value: &str) -> RunFieldValue {
        RunFieldValue { name: s(name), value: s(value) }
    }

    #[test]
    fn a_declared_session_validates() {
        assert_eq!(validate_session(&session()), Ok(()));
    }

    #[test]
    fn the_cap_is_five_minutes() {
        let mut x = session();
        x.max_duration_ms = 300_001;
        assert_eq!(validate_session(&x), Err(SessionError::DurationOutOfRange { max_duration_ms: 300_001 }));
        x.max_duration_ms = 0;
        assert!(validate_session(&x).is_err());
    }

    #[test]
    fn blank_text_fields_and_dirs_are_refused_with_a_reason() {
        let mut x = session();
        x.start_text = s("  ");
        assert_eq!(validate_session(&x), Err(SessionError::BlankPromptText { which: "start" }));
        let mut x = session();
        x.done_text = s("");
        assert_eq!(validate_session(&x), Err(SessionError::BlankPromptText { which: "done" }));
        let mut x = session();
        x.run_fields[1].label = s(" ");
        assert_eq!(validate_session(&x), Err(SessionError::BadRunField { index: 1 }));
        let mut x = session();
        x.run_fields[1].name = s("sensor");
        assert_eq!(validate_session(&x), Err(SessionError::RepeatedRunField { index: 1, first: 0 }));
        for bad in ["", " ", "a\nb", "../x", "a/../b", "a\\..\\b"] {
            let mut x = session();
            x.export_dir = s(bad);
            assert_eq!(validate_session(&x), Err(SessionError::BadExportDir), "{bad:?}");
        }
        let mut x = session();
        x.export_dir = s("/home/me/recordings");
        assert_eq!(validate_session(&x), Ok(()));
        x.export_dir = s("a..b");
        assert_eq!(validate_session(&x), Ok(()));
    }

    #[test]
    fn answers_must_name_declared_fields_once_and_fill_required_ones() {
        let x = session();
        assert_eq!(validate_run_answers(&x, &[answer("sensor", "SN-1")]), Ok(()));
        assert_eq!(
            validate_run_answers(&x, &[answer("sensor", "SN-1"), answer("note", "free text, any shape")]),
            Ok(())
        );
        assert_eq!(validate_run_answers(&x, &[]), Err(SessionError::MissingRequired { field: 0 }));
        assert_eq!(
            validate_run_answers(&x, &[answer("sensor", "  ")]),
            Err(SessionError::MissingRequired { field: 0 })
        );
        assert_eq!(
            validate_run_answers(&x, &[answer("sensor", "a"), answer("other", "b")]),
            Err(SessionError::UnknownRunField { index: 1 })
        );
        assert_eq!(
            validate_run_answers(&x, &[answer("sensor", "a"), answer("sensor", "b")]),
            Err(SessionError::RepeatedAnswer { index: 1 })
        );
    }

    #[test]
    fn a_session_round_trips_through_json_and_postcard() {
        let x = session();
        let json = serde_json::to_string(&x).unwrap();
        assert_eq!(serde_json::from_str::<SessionSpec>(&json).unwrap(), x);
        let mut buf = [0u8; 1024];
        let bytes = postcard::to_slice(&x, &mut buf).unwrap();
        assert_eq!(postcard::from_bytes::<SessionSpec>(bytes).unwrap(), x);
    }
}
