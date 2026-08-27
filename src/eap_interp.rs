//! Host-side reference interpreter for a [`ProtocolDef`] — design.md §3
//! decision 60, §4.9.
//!
//! # This is the reference, not the executor
//!
//! §3 decision 60 puts the real interpreter **on dev-bench**: the loop closes
//! against the DUT's own BLE connection interval, Core sends nothing
//! mid-study, and `main.c`'s receive-then-run model is unchanged. That
//! interpreter is hand-written C and is [`embarch-dev-bench`]'s own scope, the
//! same way §3 decisions 31/32 shipped `GattDiscover`/`GattMonitorAll`'s wire
//! types here and left live BLE dispatch there.
//!
//! What lives here is the **semantics those two have to agree on**, executable:
//!
//! - it is what the worked protocols in §4.9 are tested against, so the
//!   primitive set is *proven* sufficient for a real DUT's handshake rather
//!   than illustrated as if it were;
//! - it is what a cross-language wire-byte contract test pins the C against,
//!   the same shape `pc_skip_stream_tap` is pinned by today;
//! - it replays a captured `.bin` offline, so a run that ended in the wrong
//!   state can be re-examined without a bench.
//!
//! It is driven by [`Event`]s rather than by a radio: nothing in this module
//! opens a connection, subscribes to anything, or knows what time it is.
//! A caller feeds it arrivals and timer expiries and reads back the writes it
//! wants performed.

use heapless::Vec as HVec;

use crate::eap::{
    encode_write, eval_condition, eval_expr, select_frame, ActiveState, ProtocolDef, StateKind,
    TerminalOutcome,
};
use crate::limits::{MAX_PAYLOAD_LEN, MAX_SESSION_VARS};
use crate::result::{Outcome, ProtocolOutcome};

/// Something that happened, from the interpreter's point of view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event<'a> {
    /// A notification arrived on the characteristic bound to this source
    /// index, carrying these bytes.
    Notify { source: u8, payload: &'a [u8] },
    /// The current state's `on_timeout` deadline expired.
    Timeout,
}

/// What the caller should do next.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Perform a GATT write of `payload` to `source`, then wait.
    ///
    /// **A write's own ATT response is never fed back in.** There is no
    /// `Event` for one, deliberately (§3 decision 60): on the DUT this was
    /// designed against, a control-point write's response confirms only that
    /// the write was accepted, and the authoritative answer arrives later as
    /// an independent notification on a different characteristic. `Step` says
    /// what to send; only a [`Event::Notify`] or a [`Event::Timeout`] moves
    /// the machine.
    Write { source: u8, payload: HVec<u8, MAX_PAYLOAD_LEN>, with_response: bool },
    /// Wait for the next event. `deadline_ms` is the current state's own
    /// timeout, if it declared one — the caller arms a timer for it and
    /// sends [`Event::Timeout`] when it expires.
    Wait { deadline_ms: Option<u32> },
    /// The machine reached a terminal state and is done.
    Done(ProtocolOutcome),
}

/// A protocol run in progress.
pub struct Run<'p> {
    def: &'p ProtocolDef,
    state: u8,
    session: HVec<i64, MAX_SESSION_VARS>,
    /// Retries already spent in the current state, reset on every entry.
    retries: u8,
    finished: bool,
}

impl<'p> Run<'p> {
    /// Start a run at `entry_state`, initialising every session variable to
    /// its declared value.
    ///
    /// Returns `None` if `entry_state` is out of range — which
    /// [`crate::eap::validate_protocol`] and Core's pre-flight both check
    /// before this can be reached, so it is a belt-and-braces `Option`
    /// rather than the real defence.
    pub fn start(def: &'p ProtocolDef, entry_state: u8) -> Option<Self> {
        if entry_state as usize >= def.states.len() {
            return None;
        }
        let mut session = HVec::new();
        for v in &def.session {
            session.push(v.initial).ok()?;
        }
        Some(Run { def, state: entry_state, session, retries: 0, finished: false })
    }

    /// The state the run is currently in.
    pub fn state(&self) -> u8 {
        self.state
    }

    /// Session variables, in declaration order. Exposed for tests and for
    /// offline replay; **not** reported in [`ProtocolOutcome`] — see §3
    /// decision 62 for why that stayed out of the result.
    pub fn session(&self) -> &[i64] {
        &self.session
    }

    /// What to do on entering the current state: its `on_enter` write if it
    /// has one, otherwise a wait. Call once after [`start`](Self::start),
    /// and again after each transition.
    pub fn enter(&mut self) -> Step {
        self.retries = 0;
        self.on_entered()
    }

    fn on_entered(&mut self) -> Step {
        match &self.def.states[self.state as usize].kind {
            StateKind::Terminal(t) => {
                self.finished = true;
                Step::Done(self.outcome(match t {
                    TerminalOutcome::Pass => Outcome::Pass,
                    TerminalOutcome::Fail => Outcome::Fail {
                        reason: fail_reason(&self.def.states[self.state as usize].name),
                    },
                }))
            }
            StateKind::Active(a) => match &a.on_enter {
                Some(w) => {
                    let mut payload = HVec::new();
                    // An unresolvable operand does not become a zero: the
                    // write is not sent, and the run ends saying so. A
                    // control-point opcode carrying a silently wrong
                    // argument is worse than a step that fails.
                    match encode_write(w, &self.session, None, &mut payload) {
                        Some(()) => Step::Write {
                            source: w.source,
                            payload,
                            with_response: w.with_response,
                        },
                        None => {
                            self.finished = true;
                            Step::Done(self.outcome(Outcome::Fail {
                                reason: heapless::String::try_from(
                                    "protocol write operand did not resolve",
                                )
                                .unwrap_or_default(),
                            }))
                        }
                    }
                }
                None => Step::Wait { deadline_ms: a.on_timeout.as_ref().map(|t| t.after_ms) },
            },
        }
    }

    /// Feed one event and get the next instruction.
    ///
    /// An event no arm of the current state names is **ignored**, not an
    /// error: two states legitimately care about different subsets of what a
    /// DUT is sending, and a machine that failed on the first unrelated
    /// notification could not survive a real connection.
    pub fn on_event(&mut self, ev: Event<'_>) -> Step {
        if self.finished {
            return Step::Done(self.outcome(Outcome::Pass));
        }
        let StateKind::Active(active) = &self.def.states[self.state as usize].kind else {
            return self.on_entered();
        };
        match ev {
            Event::Timeout => self.on_timeout(active),
            Event::Notify { source, payload } => self.on_notify(active, source, payload),
        }
    }

    fn on_timeout(&mut self, active: &ActiveState) -> Step {
        let Some(t) = active.on_timeout.as_ref() else {
            // No declared timeout means this state has no deadline of its
            // own; the step's own `timeout_ms` still bounds the whole run.
            return Step::Wait { deadline_ms: None };
        };
        if self.retries < t.retry {
            self.retries += 1;
            // Re-run `on_enter` and wait again — which is what makes
            // `retry` mean "send it again", not "wait longer".
            return self.on_entered();
        }
        let goto = t.goto;
        self.state = goto;
        self.enter()
    }

    fn on_notify(&mut self, active: &ActiveState, source: u8, payload: &[u8]) -> Step {
        let Some(fi) = select_frame(self.def, source, payload) else {
            return Step::Wait {
                deadline_ms: active.on_timeout.as_ref().map(|t| t.after_ms),
            };
        };
        let Some(arm) = active.on_event.iter().find(|a| a.frame == fi) else {
            return Step::Wait {
                deadline_ms: active.on_timeout.as_ref().map(|t| t.after_ms),
            };
        };
        let frame = &self.def.frames[fi as usize];
        let ctx = Some((frame, payload));

        // `remember`s apply before the guards, in declaration order, so
        // `remember received = received + len(chunk.payload)` followed by
        // `when received >= expect_total` compares the value *including*
        // this frame — which is what an author writing those two lines
        // together means.
        for r in &arm.remember {
            if let Some(v) = eval_expr(r.value, &self.session, ctx) {
                if let Some(slot) = self.session.get_mut(r.var as usize) {
                    *slot = v;
                }
            }
        }

        let target = arm
            .when
            .iter()
            .find(|g| eval_condition(g.cond, &self.session, ctx))
            .map(|g| g.goto)
            .or(arm.otherwise);

        match target {
            Some(next) => {
                self.state = next;
                self.enter()
            }
            // No guard matched and no `otherwise`: the frame is consumed,
            // its `remember`s stand, and the machine stays put *without
            // re-entering* — no `on_enter` re-send, and the timeout keeps
            // running from the original entry. A pump loop wants the
            // opposite and says so with `otherwise: goto <itself>`, which
            // re-sends the flow-control ack and restarts the watchdog.
            None => Step::Wait {
                deadline_ms: active.on_timeout.as_ref().map(|t| t.after_ms),
            },
        }
    }

    /// End the run because the step's own `timeout_ms` expired before the
    /// machine reached a terminal state.
    ///
    /// Reports [`Outcome::TimedOut`] — the one outcome no manifest can
    /// declare and only a run can produce.
    pub fn abandon(&mut self) -> ProtocolOutcome {
        self.finished = true;
        self.outcome(Outcome::TimedOut)
    }

    fn outcome(&self, outcome: Outcome) -> ProtocolOutcome {
        ProtocolOutcome {
            final_state: self.def.states[self.state as usize].name.clone(),
            outcome,
        }
    }
}

fn fail_reason(state: &str) -> heapless::String<{ crate::limits::MAX_FAIL_REASON_LEN }> {
    let mut s: heapless::String<{ crate::limits::MAX_FAIL_REASON_LEN }> =
        heapless::String::new();
    // No comma and no quote, the rule `gatt::csv_escape_ok` sets for every
    // string this crate renders into a column.
    let _ = s.push_str("protocol reached terminal state ");
    let _ = s.push_str(state);
    s
}
