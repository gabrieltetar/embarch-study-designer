//! `.eap` protocol manifests, in the form dev-bench executes — design.md
//! §3 decisions 58-62, §4.9.
//!
//! **This is the write direction decision 39 left open.** A `Study` could
//! always declare where bytes come from and how to render them
//! ([`crate::streams`]); it could never say "write this, wait for that, and
//! branch on what came back." Decision 39's own proposal for it
//! (`StreamSend`/`StreamExpect`) was rejected as premature — it had no
//! conditional logic, no branching and no multi-step state, which is most of
//! what a real handshake is. [`ProtocolDef`] is that mechanism with the
//! state actually in it.
//!
//! # What is here, and what deliberately is not
//!
//! An `.eap` file's grammar (§4.9) is larger than this module. Decision 59
//! splits it in two, and the line is **what a running state machine can
//! reach**:
//!
//! - **Here, and crossing the wire to dev-bench**: `select_if` frame
//!   dispatch, integer scalar reads at fixed offsets, byte-span *lengths*,
//!   the expression set, write templates, states and transitions. Everything
//!   a `when`, a `remember` or a `write` can name.
//! - **Not here** — parsed from the same `.eap` file host-side and applied
//!   at render time over the raw bytes the tap already wrote:
//!   `repeat[count_from:]`, `bitpack … delta zigzag seed:`, `crc32`/`crc16`,
//!   and `fixed(scale, unit)`. No guard in either worked protocol references
//!   a bit-packed column, and with [`ProtocolOutcome`] reporting only a state
//!   name, dev-bench has no consumer for one. Putting a bit-unpacker in
//!   hand-written C would buy a capability nothing uses and make every future
//!   grammar addition cost a firmware reflash.
//!
//! # Nothing here is inferred, and nothing here is a program
//!
//! A [`ProtocolDef`] is authored by an engineer in the firmware repo's own
//! `embarch/protocols/<name>.eap` and **resolved into the submitted `Study`
//! at build time** — the posture §3 decision 52 settled for payload layouts,
//! for the same reason: Core cannot read that repo, so a study that named a
//! manifest rather than carrying it would run on its author's machine and
//! nowhere else. What crosses the wire is the resolved definition, indexed by
//! [`crate::study::Action::RunProtocol`].
//!
//! The expression set is three operand forms, one arithmetic operation and
//! six comparisons ([`Expr`], [`Condition`]). There is no nesting, no
//! boolean connective, no user-defined function, and no way to express a
//! loop that is not a state transition. That is a deliberate ceiling, not an
//! unfinished one — see §3 decision 60 for what each omission costs and why
//! it was judged worth paying.

use heapless::{String, Vec};
use serde::{Deserialize, Serialize};

use crate::bounded::Bounded;
use crate::decoder::ScalarType;
use crate::ids::Uuid;
use crate::limits::{
    MAX_EAP_FIELD_NAME_LEN, MAX_EVENT_ARMS_PER_STATE, MAX_FRAMES_PER_PROTOCOL, MAX_FRAME_FIELDS,
    MAX_FRAME_NAME_LEN, MAX_FRAME_SPANS, MAX_GUARDS_PER_ARM, MAX_PROTOCOL_NAME_LEN,
    MAX_REMEMBER_PER_ARM, MAX_SELECT_MATCH_LEN, MAX_SESSION_VARS, MAX_SESSION_VAR_NAME_LEN,
    MAX_SOURCES_PER_PROTOCOL, MAX_SOURCE_NAME_LEN, MAX_STATES_PER_PROTOCOL, MAX_STATE_NAME_LEN,
    MAX_WRITE_FIELDS,
};

/// One `protocol <name> { … }` block, resolved and ready to execute.
///
/// Every cross-reference inside is an **index**, never a name: a state's
/// `goto` is a `states` index, an `on_event` names a `frames` index, a
/// `write` names a `sources` index. Names survive only where a human or a
/// result has to read them back. This is what lets dev-bench's interpreter
/// dispatch without string comparison, and it is checked once — by
/// [`validate_protocol`] — on the host, before the study is ever submitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtocolDef {
    pub name: String<MAX_PROTOCOL_NAME_LEN>,
    /// Characteristic aliases this block declares for itself. A protocol is
    /// **self-contained** (design.md §3 decision 58): it does not reference
    /// the study's `StreamTap`s, so the same `.eap` protocol is portable
    /// across studies that are wired up differently.
    pub sources: Vec<ProtocolSource, MAX_SOURCES_PER_PROTOCOL>,
    /// Frame shapes the machine can dispatch on. Two frames may declare the
    /// same source; the first whose `select_if` matches wins, which is the
    /// only format-versioning mechanism there is (design.md §3 decision 59).
    pub frames: Vec<FrameDef, MAX_FRAMES_PER_PROTOCOL>,
    /// Named integer variables, initialised once when the run enters its
    /// entry state.
    pub session: Vec<SessionVarDef, MAX_SESSION_VARS>,
    pub states: Bounded<StateDef, MAX_STATES_PER_PROTOCOL>,
}

/// A characteristic this protocol writes to or reads frames from, under a
/// local alias.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolSource {
    pub name: String<MAX_SOURCE_NAME_LEN>,
    pub service_uuid: Uuid,
    pub characteristic_uuid: Uuid,
}

/// One declared frame shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameDef {
    pub name: String<MAX_FRAME_NAME_LEN>,
    /// Index into [`ProtocolDef::sources`].
    pub source: u8,
    /// `None` matches any payload arriving on that source — legal, and the
    /// right answer for a characteristic that only ever carries one format.
    /// A frame with no `select_if` must be the **last** one declared for its
    /// source, so it cannot silently shadow a more specific sibling;
    /// [`validate_protocol`] enforces that rather than leaving it to
    /// declaration luck.
    pub select_if: Option<FrameMatch>,
    /// Guard-reachable integer reads. Not the whole packet — see the module
    /// docs.
    pub fields: Vec<ScalarRead, MAX_FRAME_FIELDS>,
    /// Declared byte spans. Only [`Operand::SpanLen`] can reach one.
    pub spans: Vec<SpanRead, MAX_FRAME_SPANS>,
}

/// `select_if { offset: N, len: L, eq: … }` — a literal byte run that must
/// match for this frame to be selected.
///
/// `len` is not stored: it is `eq.len()` by construction, and carrying it
/// separately would allow a manifest where the two disagree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameMatch {
    pub offset: u16,
    pub eq: Vec<u8, MAX_SELECT_MATCH_LEN>,
}

impl FrameMatch {
    /// Whether `payload` carries this frame's magic at its declared offset.
    ///
    /// A payload too short to contain the match does **not** match — it is
    /// never treated as a partial hit, because a truncated notification and
    /// a different format are different facts.
    pub fn matches(&self, payload: &[u8]) -> bool {
        let start = self.offset as usize;
        match payload.get(start..start + self.eq.len()) {
            Some(window) => window == &self.eq[..],
            None => false,
        }
    }
}

/// One integer field a guard, a `remember` or a `write` can name.
///
/// Reuses [`ScalarType`] — §3 decision 52's own 18-variant width/signedness/
/// byte-order enum — rather than declaring a second one, so a frame lowered
/// into a `StructLayout` for rendering reads its bytes through exactly the
/// same code path that a guard does.
///
/// **Float variants are rejected at parse time, not at runtime.** The
/// expression set is integer-only ([`Operand::Literal`] is an `i64`), so an
/// `f32` field could be declared and never used; refusing it where it is
/// written is the loud failure, and a runtime one on dev-bench would not be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScalarRead {
    pub name: String<MAX_EAP_FIELD_NAME_LEN>,
    pub offset: u16,
    pub ty: ScalarType,
}

/// A declared byte span. `len: None` means "the rest of the payload".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanRead {
    pub name: String<MAX_EAP_FIELD_NAME_LEN>,
    pub offset: u16,
    pub len: Option<u16>,
}

/// A named integer carried for the length of one protocol run.
///
/// **Integers only, and that is the shape of a real decision.** The draft
/// this decision came from had a `bytes` variable accumulating a download
/// (`buffer = buffer ++ chunk.payload`) so a guard could compare its length
/// against an expected total. With dev-bench as the executor (§3 decision
/// 60) there is nowhere to put those bytes — and nowhere they are needed:
/// the chunks are already streaming out on their own tap as they arrive, so
/// the machine only has to count them. `received = received + len(chunk.payload)`
/// says the same thing in eight bytes of state instead of the whole transfer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionVarDef {
    pub name: String<MAX_SESSION_VAR_NAME_LEN>,
    pub initial: i64,
}

/// Everything an expression can name. Four forms, and adding a fifth is a
/// decision rather than a convenience.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operand {
    /// An integer literal, written in the manifest as decimal or `0x`-hex.
    Literal(i64),
    /// A field of the frame that triggered the current event, by index into
    /// that [`FrameDef::fields`]. Reachable only from an `on_event` arm —
    /// an `on_enter` write has no triggering frame, and
    /// [`validate_protocol`] refuses one that references a field.
    Field(u8),
    /// A session variable, by index into [`ProtocolDef::session`].
    Session(u8),
    /// `len(<frame>.<span>)`, by index into that [`FrameDef::spans`]. The
    /// only way a byte span's contents affect anything at all.
    SpanLen(u8),
}

/// The right-hand side of a `remember`.
///
/// One level, no parentheses, and exactly one arithmetic operation. The
/// justification for `Add` existing at all is that a flow-controlled pump
/// loop has to accumulate a count and there is no other way to say it; the
/// justification for nothing else existing is that no worked protocol needed
/// one, and every operator added here is a permanent widening of what a
/// manifest can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expr {
    Term(Operand),
    /// `<operand> + <operand>` — wrapping is not possible to express, since
    /// both operands are `i64` and the evaluator saturates rather than
    /// wrapping (see [`eval_expr`]).
    Add(Operand, Operand),
}

/// The six comparisons, and no boolean connectives.
///
/// `a && b` is expressible as two states; `!a` is expressible by swapping
/// the `when` and the `otherwise`. Neither omission costs an author a
/// protocol they could otherwise have written, and both keep a guard to one
/// comparison a reader can check at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// `<operand> <op> <operand>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Condition {
    pub lhs: Operand,
    pub op: CompareOp,
    pub rhs: Operand,
}

/// `remember <var> = <expr>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remember {
    /// Index into [`ProtocolDef::session`].
    pub var: u8,
    pub value: Expr,
}

/// One typed field of a `write` payload (design.md §3 decision 61).
///
/// The same [`ScalarType`] vocabulary decode uses, which is the whole point
/// of the decision: a write that has to echo back a decoded value or carry a
/// session variable cannot be expressed by a literal-only payload, and
/// `study-actions.toml`'s registered actions (§3 decision 35) are
/// literal-only by design. This does not change them — it applies only
/// inside a `RunProtocol` block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteField {
    pub ty: ScalarType,
    pub value: Operand,
}

/// An `on_enter:` write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteAction {
    /// Index into [`ProtocolDef::sources`].
    pub source: u8,
    /// Fields are packed in declaration order with no padding, exactly as
    /// [`crate::decoder::StructLayout`] reads them.
    pub fields: Vec<WriteField, MAX_WRITE_FIELDS>,
    /// An acknowledged (`Write Request`) write rather than a
    /// `Write Command`.
    ///
    /// **A write's own response is never a transition trigger**, whichever
    /// this is — see [`ActiveState`]. `with_response` controls the ATT
    /// operation, and nothing else.
    pub with_response: bool,
}

/// `on_event <frame>: [remember …] [when …: goto …] [otherwise: goto …]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventArm {
    /// Index into [`ProtocolDef::frames`].
    pub frame: u8,
    /// Applied **before** the guards are evaluated, in declaration order —
    /// so `remember received = received + len(chunk.payload)` followed by
    /// `when received >= expect_total` compares the value including this
    /// frame, which is what an author writing those two lines together
    /// means.
    pub remember: Vec<Remember, MAX_REMEMBER_PER_ARM>,
    /// First match wins.
    pub when: Vec<GuardedGoto, MAX_GUARDS_PER_ARM>,
    /// Taken when no guard matched. `None` means the frame is consumed —
    /// `remember`s applied — and the machine **stays in this state without
    /// re-entering it**: no `on_enter` write is re-sent and the timeout keeps
    /// running from the original entry.
    ///
    /// **That is a different thing from `otherwise: goto <this state>`, and
    /// a flow-controlled pump loop needs the latter.** A self-transition
    /// re-runs `on_enter` — re-sending the `NEXT_CHUNK` that asks for the
    /// following chunk — and restarts the stall watchdog, so each arriving
    /// chunk resets the deadline. Omitting `otherwise` there would consume
    /// every chunk correctly, ack none of them, and stall at the watchdog.
    /// Both behaviors are real and the author says which; §4.9's worked BDS
    /// download writes the self-transition explicitly for exactly this
    /// reason.
    pub otherwise: Option<u8>,
}

/// `when <cond>: goto <state>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardedGoto {
    pub cond: Condition,
    /// Index into [`ProtocolDef::states`].
    pub goto: u8,
}

/// `on_timeout <ms> [retry <n>]: goto <state>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeoutArm {
    /// Measured from the moment the state was entered, or from the last
    /// retry — not from the start of the run.
    pub after_ms: u32,
    /// How many times to re-run `on_enter` and wait again before taking
    /// `goto`. `0` means take it on the first expiry, which is what a stall
    /// watchdog wants.
    pub retry: u8,
    /// Index into [`ProtocolDef::states`], taken once `retry` is exhausted.
    pub goto: u8,
}

/// What a terminal state declares. Two variants, because a terminal state is
/// authored and cannot carry a runtime reason.
///
/// [`crate::result::Outcome`] itself is reused unchanged for what comes
/// *back* — including `Outcome::TimedOut`, which a manifest cannot declare
/// and only a run can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutcome {
    Pass,
    Fail,
}

/// A state: either one the machine acts in, or one it stops at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateKind {
    Active(ActiveState),
    Terminal(TerminalOutcome),
}

/// A non-terminal state's behavior.
///
/// **Nothing here can transition on a write's own ATT response**, and that
/// is deliberate rather than an omission (design.md §3 decision 60). On the
/// DUT this was designed against, a control-point write's response confirms
/// only that the write was *accepted*; the authoritative answer arrives
/// later as an independent notification on a different characteristic. A
/// machine that could branch on the ack would be branching on the wrong
/// fact, and would look correct while doing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveState {
    /// Performed once on entry, and again on each `on_timeout` retry.
    pub on_enter: Option<WriteAction>,
    /// A frame not named by any arm is ignored by this state — not an
    /// error. Two states legitimately care about different subsets of what a
    /// DUT is sending.
    pub on_event: Vec<EventArm, MAX_EVENT_ARMS_PER_STATE>,
    /// A state with no timeout can only be left by an event. The step's own
    /// `timeout_ms` still bounds the whole run, so this is not a way to hang
    /// forever — it is a way to say "this state has no deadline of its own".
    pub on_timeout: Option<TimeoutArm>,
}

/// One named state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateDef {
    pub name: String<MAX_STATE_NAME_LEN>,
    pub kind: StateKind,
}

// --- Validation --------------------------------------------------------
//
// Computed here rather than in `embarch-core` or in the `.eap` parser, for
// the reason `validate_taps` (§4.8) already is: there must be no second copy
// to drift. A `ProtocolDef` reaching dev-bench with an out-of-range index
// would be a hand-written C interpreter dereferencing past an array, so the
// check that it cannot happen belongs where every consumer sees the same one.

/// Why a [`ProtocolDef`] cannot be executed.
///
/// Every variant names the specific thing that is wrong, in the style §3
/// decision 18 sets for Core's pre-flight validation: a raw index failure
/// on a firmware's array is exactly the failure this exists to convert into
/// a sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolError {
    /// A protocol with no states cannot be entered.
    NoStates,
    /// `Action::RunProtocol.entry_state`, or a `goto`, names a state that
    /// does not exist.
    StateIndexOutOfRange { at_state: u8, goto: u8 },
    /// An `on_event` names a frame that does not exist.
    FrameIndexOutOfRange { at_state: u8, frame: u8 },
    /// A `write` or a `frame` names a source alias that does not exist.
    SourceIndexOutOfRange { source: u8 },
    /// An operand references a session variable that does not exist.
    SessionIndexOutOfRange { session: u8 },
    /// An operand references a field or span the triggering frame does not
    /// declare.
    FieldIndexOutOfRange { frame: u8, field: u8 },
    /// An `on_enter` write referenced a decoded field. There is no
    /// triggering frame on entry, so the reference has nothing to resolve
    /// against — caught here rather than producing a zero on the bench.
    FieldRefInEnterWrite { at_state: u8 },
    /// A `select_if`-less frame is not the last one declared for its source,
    /// so it shadows a more specific sibling that can then never match.
    UnguardedFrameShadows { frame: u8, shadowed: u8 },
    /// A scalar field declared at a float width. The expression set is
    /// integer-only.
    NonIntegerField { frame: u8, field: u8 },
    /// The entry state is a terminal one, so the run would end without
    /// doing anything.
    EntryStateIsTerminal,
    /// No terminal state exists, so no run can ever finish by itself.
    NoTerminalState,
}

impl core::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProtocolError::NoStates => write!(f, "protocol declares no states"),
            ProtocolError::StateIndexOutOfRange { at_state, goto } => {
                write!(f, "state {at_state} goes to state {goto}, which does not exist")
            }
            ProtocolError::FrameIndexOutOfRange { at_state, frame } => {
                write!(f, "state {at_state} reacts to frame {frame}, which does not exist")
            }
            ProtocolError::SourceIndexOutOfRange { source } => {
                write!(f, "source {source} does not exist")
            }
            ProtocolError::SessionIndexOutOfRange { session } => {
                write!(f, "session variable {session} does not exist")
            }
            ProtocolError::FieldIndexOutOfRange { frame, field } => {
                write!(f, "frame {frame} does not declare field {field}")
            }
            ProtocolError::FieldRefInEnterWrite { at_state } => write!(
                f,
                "state {at_state}'s on_enter write references a decoded field but no frame triggered it"
            ),
            ProtocolError::UnguardedFrameShadows { frame, shadowed } => write!(
                f,
                "frame {frame} has no select_if and precedes frame {shadowed} on the same source"
            ),
            ProtocolError::NonIntegerField { frame, field } => {
                write!(f, "frame {frame} field {field} is a float width and no guard can compare it")
            }
            ProtocolError::EntryStateIsTerminal => {
                write!(f, "entry state is terminal so the run would end immediately")
            }
            ProtocolError::NoTerminalState => {
                write!(f, "protocol declares no terminal state so no run can finish")
            }
        }
    }
}

/// Check every index and every structural rule a running interpreter would
/// otherwise have to trust.
///
/// Called by whoever builds a `Study` — before submission, before the CRC is
/// sealed, and long before any of it reaches a firmware.
pub fn validate_protocol(p: &ProtocolDef) -> Result<(), ProtocolError> {
    if p.states.is_empty() {
        return Err(ProtocolError::NoStates);
    }
    if !p.states.iter().any(|s| matches!(s.kind, StateKind::Terminal(_))) {
        return Err(ProtocolError::NoTerminalState);
    }

    // Frames: source in range, integer fields only, and no unguarded frame
    // sitting in front of a guarded sibling on the same source.
    for (fi, frame) in p.frames.iter().enumerate() {
        if frame.source as usize >= p.sources.len() {
            return Err(ProtocolError::SourceIndexOutOfRange { source: frame.source });
        }
        for (di, field) in frame.fields.iter().enumerate() {
            if !field.ty.is_integer() {
                return Err(ProtocolError::NonIntegerField { frame: fi as u8, field: di as u8 });
            }
        }
        if frame.select_if.is_none() {
            if let Some(later) = p
                .frames
                .iter()
                .enumerate()
                .skip(fi + 1)
                .find(|(_, o)| o.source == frame.source)
            {
                return Err(ProtocolError::UnguardedFrameShadows {
                    frame: fi as u8,
                    shadowed: later.0 as u8,
                });
            }
        }
    }

    let n_states = p.states.len();
    for (si, state) in p.states.iter().enumerate() {
        let si8 = si as u8;
        let active = match &state.kind {
            StateKind::Terminal(_) => continue,
            StateKind::Active(a) => a,
        };

        if let Some(w) = &active.on_enter {
            if w.source as usize >= p.sources.len() {
                return Err(ProtocolError::SourceIndexOutOfRange { source: w.source });
            }
            for wf in &w.fields {
                // No triggering frame exists on entry, so a field reference
                // has nothing to resolve against.
                if matches!(wf.value, Operand::Field(_) | Operand::SpanLen(_)) {
                    return Err(ProtocolError::FieldRefInEnterWrite { at_state: si8 });
                }
                check_operand(wf.value, p, None, si8)?;
            }
        }

        for arm in &active.on_event {
            let frame = p
                .frames
                .get(arm.frame as usize)
                .ok_or(ProtocolError::FrameIndexOutOfRange { at_state: si8, frame: arm.frame })?;
            for r in &arm.remember {
                if r.var as usize >= p.session.len() {
                    return Err(ProtocolError::SessionIndexOutOfRange { session: r.var });
                }
                for op in expr_operands(r.value) {
                    check_operand(op, p, Some((arm.frame, frame)), si8)?;
                }
            }
            for g in &arm.when {
                check_operand(g.cond.lhs, p, Some((arm.frame, frame)), si8)?;
                check_operand(g.cond.rhs, p, Some((arm.frame, frame)), si8)?;
                if g.goto as usize >= n_states {
                    return Err(ProtocolError::StateIndexOutOfRange { at_state: si8, goto: g.goto });
                }
            }
            if let Some(o) = arm.otherwise {
                if o as usize >= n_states {
                    return Err(ProtocolError::StateIndexOutOfRange { at_state: si8, goto: o });
                }
            }
        }

        if let Some(t) = &active.on_timeout {
            if t.goto as usize >= n_states {
                return Err(ProtocolError::StateIndexOutOfRange { at_state: si8, goto: t.goto });
            }
        }
    }
    Ok(())
}

/// The operands one [`Expr`] names, as a fixed-size array so this needs no
/// allocation and no iterator adapter on the `no_std` path.
fn expr_operands(e: Expr) -> [Operand; 2] {
    match e {
        Expr::Term(a) => [a, a],
        Expr::Add(a, b) => [a, b],
    }
}

fn check_operand(
    op: Operand,
    p: &ProtocolDef,
    frame: Option<(u8, &FrameDef)>,
    at_state: u8,
) -> Result<(), ProtocolError> {
    match op {
        Operand::Literal(_) => Ok(()),
        Operand::Session(i) => {
            if (i as usize) < p.session.len() {
                Ok(())
            } else {
                Err(ProtocolError::SessionIndexOutOfRange { session: i })
            }
        }
        Operand::Field(i) => match frame {
            Some((fi, f)) if (i as usize) < f.fields.len() => {
                let _ = fi;
                Ok(())
            }
            Some((fi, _)) => Err(ProtocolError::FieldIndexOutOfRange { frame: fi, field: i }),
            None => Err(ProtocolError::FieldRefInEnterWrite { at_state }),
        },
        Operand::SpanLen(i) => match frame {
            Some((fi, f)) if (i as usize) < f.spans.len() => {
                let _ = fi;
                Ok(())
            }
            Some((fi, _)) => Err(ProtocolError::FieldIndexOutOfRange { frame: fi, field: i }),
            None => Err(ProtocolError::FieldRefInEnterWrite { at_state }),
        },
    }
}

// --- Evaluation --------------------------------------------------------
//
// The reference semantics. dev-bench's C interpreter has to agree with these
// byte for byte, the same way its `StreamTap` walker agrees with
// `pc_skip_stream_tap` — this is what a cross-language contract test pins.

/// Resolve one operand against the current run state.
///
/// `payload`/`frame` are the frame that triggered the current event, absent
/// for an `on_enter` write. Returns `None` only for a reference
/// [`validate_protocol`] should already have rejected, or a payload too
/// short to contain a declared field — the latter is a real runtime case (a
/// truncated notification) and is deliberately not a zero.
pub fn eval_operand(
    op: Operand,
    session: &[i64],
    frame: Option<(&FrameDef, &[u8])>,
) -> Option<i64> {
    match op {
        Operand::Literal(v) => Some(v),
        Operand::Session(i) => session.get(i as usize).copied(),
        Operand::Field(i) => {
            let (f, payload) = frame?;
            let read = f.fields.get(i as usize)?;
            let at = read.offset as usize;
            read.ty.read_i64(payload.get(at..)?)
        }
        Operand::SpanLen(i) => {
            let (f, payload) = frame?;
            let span = f.spans.get(i as usize)?;
            let at = span.offset as usize;
            let available = payload.len().checked_sub(at)?;
            Some(match span.len {
                Some(n) if (n as usize) <= available => n as i64,
                // A declared fixed length longer than what arrived is a
                // short payload, not a silently shorter span.
                Some(_) => return None,
                None => available as i64,
            })
        }
    }
}

/// Evaluate a `remember`'s right-hand side.
///
/// `Add` **saturates**. A wrapping counter would be a plausible wrong number
/// — the failure this crate refuses elsewhere (§3 decision 52's "integers
/// render as integers") — and a saturated one stops a pump loop's guard from
/// ever passing again, which is a stall the step timeout catches and reports.
pub fn eval_expr(e: Expr, session: &[i64], frame: Option<(&FrameDef, &[u8])>) -> Option<i64> {
    match e {
        Expr::Term(a) => eval_operand(a, session, frame),
        Expr::Add(a, b) => {
            let x = eval_operand(a, session, frame)?;
            let y = eval_operand(b, session, frame)?;
            Some(x.saturating_add(y))
        }
    }
}

/// Evaluate a guard. An operand that cannot be resolved makes the guard
/// **false**, never true: a comparison against a field a truncated payload
/// did not contain must not be the thing that advances a state machine.
pub fn eval_condition(
    c: Condition,
    session: &[i64],
    frame: Option<(&FrameDef, &[u8])>,
) -> bool {
    let (Some(l), Some(r)) = (
        eval_operand(c.lhs, session, frame),
        eval_operand(c.rhs, session, frame),
    ) else {
        return false;
    };
    match c.op {
        CompareOp::Eq => l == r,
        CompareOp::Ne => l != r,
        CompareOp::Lt => l < r,
        CompareOp::Le => l <= r,
        CompareOp::Gt => l > r,
        CompareOp::Ge => l >= r,
    }
}

/// Select the frame a payload arriving on `source` belongs to — first
/// matching `select_if` wins, an unguarded frame matching anything.
///
/// Returns the frame's index, so a caller can look up an `on_event` arm by
/// the same number the manifest uses.
pub fn select_frame(p: &ProtocolDef, source: u8, payload: &[u8]) -> Option<u8> {
    p.frames.iter().enumerate().find_map(|(i, f)| {
        if f.source != source {
            return None;
        }
        match &f.select_if {
            Some(m) if m.matches(payload) => Some(i as u8),
            Some(_) => None,
            None => Some(i as u8),
        }
    })
}

/// Assemble a `write` payload from its typed fields (design.md §3 decision
/// 61).
///
/// Returns `None` if any operand fails to resolve — the write is not sent
/// with a zero substituted in, because a control-point opcode carrying a
/// silently-wrong argument is worse than a step that fails saying so.
pub fn encode_write(
    w: &WriteAction,
    session: &[i64],
    frame: Option<(&FrameDef, &[u8])>,
    out: &mut Vec<u8, { crate::limits::MAX_PAYLOAD_LEN }>,
) -> Option<()> {
    out.clear();
    for f in &w.fields {
        let v = eval_operand(f.value, session, frame)?;
        let mut buf = [0u8; 8];
        let n = f.ty.write_i64(v, &mut buf)?;
        out.extend_from_slice(&buf[..n]).ok()?;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::ScalarType;

    fn src(name: &str) -> ProtocolSource {
        ProtocolSource {
            name: String::try_from(name).unwrap(),
            service_uuid: Uuid([0x11; 16]),
            characteristic_uuid: Uuid([0x22; 16]),
        }
    }

    fn scalar(name: &str, offset: u16, ty: ScalarType) -> ScalarRead {
        ScalarRead { name: String::try_from(name).unwrap(), offset, ty }
    }

    fn frame(name: &str, source: u8, magic: Option<(u16, &[u8])>) -> FrameDef {
        FrameDef {
            name: String::try_from(name).unwrap(),
            source,
            select_if: magic.map(|(offset, b)| FrameMatch {
                offset,
                eq: Vec::from_slice(b).unwrap(),
            }),
            fields: Vec::new(),
            spans: Vec::new(),
        }
    }

    fn terminal(name: &str, o: TerminalOutcome) -> StateDef {
        StateDef { name: String::try_from(name).unwrap(), kind: StateKind::Terminal(o) }
    }

    fn minimal() -> ProtocolDef {
        let mut states = Bounded::new();
        states
            .push(StateDef {
                name: String::try_from("go").unwrap(),
                kind: StateKind::Active(ActiveState {
                    on_enter: None,
                    on_event: Vec::new(),
                    on_timeout: None,
                }),
            })
            .unwrap();
        states.push(terminal("done", TerminalOutcome::Pass)).unwrap();
        ProtocolDef {
            name: String::try_from("p").unwrap(),
            sources: Vec::new(),
            frames: Vec::new(),
            session: Vec::new(),
            states,
        }
    }

    #[test]
    fn a_protocol_round_trips_through_postcard_and_json() {
        // Both hops, like every other wire type in this crate (§3 decision
        // 3): postcard to dev-bench, JSON through embarch-api's events.json.
        let mut p = minimal();
        p.sources.push(src("ctrl")).unwrap();
        p.frames.push(frame("f", 0, Some((0, &[0x02])))).unwrap();
        p.session
            .push(SessionVarDef { name: String::try_from("n").unwrap(), initial: 7 })
            .unwrap();

        let mut buf = [0u8; 2048];
        let bytes = postcard::to_slice(&p, &mut buf).unwrap();
        let back: ProtocolDef = postcard::from_bytes(bytes).unwrap();
        assert_eq!(back, p);

        let json = serde_json::to_string(&p).unwrap();
        let back: ProtocolDef = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn a_select_if_never_partially_matches_a_short_payload() {
        // A truncated notification and a different format are different
        // facts; treating the first as a partial hit would dispatch a frame
        // against bytes that never arrived.
        let m = FrameMatch { offset: 0, eq: Vec::from_slice(b"GWF1").unwrap() };
        assert!(m.matches(b"GWF1...."));
        assert!(!m.matches(b"GWF"));
        assert!(!m.matches(b""));
        assert!(!m.matches(b"PPG1"));

        let at2 = FrameMatch { offset: 2, eq: Vec::from_slice(&[0xAB]).unwrap() };
        assert!(at2.matches(&[0, 0, 0xAB]));
        assert!(!at2.matches(&[0, 0]));
    }

    #[test]
    fn an_unguarded_frame_may_not_shadow_a_guarded_sibling() {
        // Otherwise the guarded one can never be reached, and nothing would
        // say so: the capture would just always be the wrong frame.
        let mut p = minimal();
        p.sources.push(src("s")).unwrap();
        p.frames.push(frame("catch_all", 0, None)).unwrap();
        p.frames.push(frame("specific", 0, Some((0, &[0x01])))).unwrap();
        assert_eq!(
            validate_protocol(&p),
            Err(ProtocolError::UnguardedFrameShadows { frame: 0, shadowed: 1 })
        );

        // The other order is fine: the specific frame is tried first.
        let mut q = minimal();
        q.sources.push(src("s")).unwrap();
        q.frames.push(frame("specific", 0, Some((0, &[0x01])))).unwrap();
        q.frames.push(frame("catch_all", 0, None)).unwrap();
        assert_eq!(validate_protocol(&q), Ok(()));
    }

    #[test]
    fn a_protocol_with_no_terminal_state_is_refused() {
        let mut p = minimal();
        p.states = Bounded::new();
        p.states
            .push(StateDef {
                name: String::try_from("spin").unwrap(),
                kind: StateKind::Active(ActiveState {
                    on_enter: None,
                    on_event: Vec::new(),
                    on_timeout: None,
                }),
            })
            .unwrap();
        assert_eq!(validate_protocol(&p), Err(ProtocolError::NoTerminalState));
    }

    #[test]
    fn a_float_field_is_refused_where_it_is_declared() {
        let mut p = minimal();
        p.sources.push(src("s")).unwrap();
        let mut f = frame("f", 0, None);
        f.fields.push(scalar("x", 0, ScalarType::F32Le)).unwrap();
        p.frames.push(f).unwrap();
        assert_eq!(
            validate_protocol(&p),
            Err(ProtocolError::NonIntegerField { frame: 0, field: 0 })
        );
    }

    #[test]
    fn an_on_enter_write_cannot_reference_a_decoded_field() {
        // There is no triggering frame on entry, so the reference resolves
        // to nothing. Caught here rather than becoming a zero on the bench.
        let mut p = minimal();
        p.sources.push(src("s")).unwrap();
        let mut fields = Vec::new();
        fields.push(WriteField { ty: ScalarType::U8, value: Operand::Field(0) }).unwrap();
        p.states = Bounded::new();
        p.states
            .push(StateDef {
                name: String::try_from("go").unwrap(),
                kind: StateKind::Active(ActiveState {
                    on_enter: Some(WriteAction { source: 0, fields, with_response: false }),
                    on_event: Vec::new(),
                    on_timeout: None,
                }),
            })
            .unwrap();
        p.states.push(terminal("done", TerminalOutcome::Pass)).unwrap();
        assert_eq!(
            validate_protocol(&p),
            Err(ProtocolError::FieldRefInEnterWrite { at_state: 0 })
        );
    }

    #[test]
    fn every_index_is_range_checked_before_a_c_array_ever_sees_it() {
        let mut p = minimal();
        p.frames.push(frame("f", 3, None)).unwrap();
        assert_eq!(
            validate_protocol(&p),
            Err(ProtocolError::SourceIndexOutOfRange { source: 3 })
        );
    }

    #[test]
    fn a_short_payload_yields_no_value_rather_than_a_zero() {
        // The whole reason `eval_operand` returns an `Option`: a guard
        // comparing against a field a truncated notification did not carry
        // must not be what advances a state machine.
        let mut f = frame("f", 0, None);
        f.fields.push(scalar("total", 5, ScalarType::U32Be)).unwrap();
        let short = [0x02, 0, 0, 0, 1];
        assert_eq!(eval_operand(Operand::Field(0), &[], Some((&f, &short))), None);

        let full = [0x02, 0, 0, 0, 0, 0, 0, 0x02, 0x00];
        assert_eq!(eval_operand(Operand::Field(0), &[], Some((&f, &full))), Some(512));

        // And a guard over it is false, never true.
        let c = Condition { lhs: Operand::Field(0), op: CompareOp::Ge, rhs: Operand::Literal(0) };
        assert!(!eval_condition(c, &[], Some((&f, &short))));
        assert!(eval_condition(c, &[], Some((&f, &full))));
    }

    #[test]
    fn a_span_length_is_what_arrived_and_a_fixed_span_refuses_to_shrink() {
        let mut f = frame("f", 0, None);
        f.spans
            .push(SpanRead { name: String::try_from("rest").unwrap(), offset: 2, len: None })
            .unwrap();
        f.spans
            .push(SpanRead { name: String::try_from("four").unwrap(), offset: 0, len: Some(4) })
            .unwrap();

        let p = [0u8; 10];
        assert_eq!(eval_operand(Operand::SpanLen(0), &[], Some((&f, &p))), Some(8));
        assert_eq!(eval_operand(Operand::SpanLen(1), &[], Some((&f, &p))), Some(4));

        // A declared fixed length longer than what arrived is a short
        // payload, not a silently shorter span.
        let tiny = [0u8; 3];
        assert_eq!(eval_operand(Operand::SpanLen(1), &[], Some((&f, &tiny))), None);
        assert_eq!(eval_operand(Operand::SpanLen(0), &[], Some((&f, &tiny))), Some(1));
    }

    #[test]
    fn addition_saturates_rather_than_wrapping() {
        // A wrapping counter is a plausible wrong number. A saturated one
        // stops the guard passing, which the step timeout catches and
        // reports.
        let e = Expr::Add(Operand::Session(0), Operand::Literal(1));
        assert_eq!(eval_expr(e, &[i64::MAX], None), Some(i64::MAX));
        assert_eq!(eval_expr(e, &[41], None), Some(42));
    }

    #[test]
    fn a_u64_above_i64_max_saturates_rather_than_reading_negative() {
        let mut f = frame("f", 0, None);
        f.fields.push(scalar("big", 0, ScalarType::U64Le)).unwrap();
        let payload = u64::MAX.to_le_bytes();
        assert_eq!(
            eval_operand(Operand::Field(0), &[], Some((&f, &payload))),
            Some(i64::MAX),
            "never a negative number a guard would compare the wrong way"
        );
    }

    #[test]
    fn a_write_assembles_its_fields_in_declaration_order_with_no_padding() {
        let mut fields = Vec::new();
        fields.push(WriteField { ty: ScalarType::U8, value: Operand::Literal(0x02) }).unwrap();
        fields
            .push(WriteField { ty: ScalarType::U32Be, value: Operand::Session(0) })
            .unwrap();
        fields
            .push(WriteField { ty: ScalarType::U16Le, value: Operand::Literal(0x0102) })
            .unwrap();
        let w = WriteAction { source: 0, fields, with_response: true };

        let mut out = Vec::new();
        assert_eq!(encode_write(&w, &[0xDEADBEEFu32 as i64], None, &mut out), Some(()));
        assert_eq!(&out[..], &[0x02, 0xDE, 0xAD, 0xBE, 0xEF, 0x02, 0x01]);
    }

    #[test]
    fn an_unresolvable_write_operand_produces_no_write_at_all() {
        // Not a zero-filled one: a control-point opcode carrying a silently
        // wrong argument is worse than a step that fails saying so.
        let mut fields = Vec::new();
        fields.push(WriteField { ty: ScalarType::U8, value: Operand::Session(9) }).unwrap();
        let w = WriteAction { source: 0, fields, with_response: false };
        let mut out = Vec::new();
        assert_eq!(encode_write(&w, &[], None, &mut out), None);
    }
}
