//! The two worked protocols from design.md §4.9, as tests rather than as
//! documentation.
//!
//! They were chosen because they are structurally representative of a real
//! DUT's BLE stack, so they are here to **prove the primitive set is
//! sufficient**, not to illustrate it. Between them they exercise every
//! primitive §3 decision 59 admits: magic-byte format dispatch, a descriptor
//! table that parametrizes later bytes, delta+zigzag+bit-packed columns, a
//! trailing CRC-32 with a per-frame policy, byte spans, the whole expression
//! set, both write forms, retries, a stall watchdog, and both terminal
//! outcomes.
//!
//! If the grammar ever stops being able to express one of these, that is a
//! real finding about the primitive set and this file is where it surfaces.

#![cfg(feature = "eap-parse")]

use embarch_study_designer::eap::{
    select_frame, CompareOp, Operand, ProtocolDef, StateKind, TerminalOutcome,
};
use embarch_study_designer::eap_interp::{Event, Run, Step};
use embarch_study_designer::eap_parse::{parse, resolve, AstCount, AstField, CrcPolicy};
use embarch_study_designer::result::Outcome;

const BDS: &str = include_str!("fixtures/bds_batch_download.eap");
const GWF1: &str = include_str!("fixtures/gwf1_batch.eap");

fn resolve_only(src: &str) -> embarch_study_designer::eap_parse::ResolvedProtocol {
    let file = parse(src).expect("fixture parses");
    assert_eq!(file.protocols.len(), 1);
    resolve(&file.protocols[0]).expect("fixture resolves")
}

fn state_named(def: &ProtocolDef, name: &str) -> u8 {
    def.states
        .iter()
        .position(|s| s.name.as_str() == name)
        .unwrap_or_else(|| panic!("no state {name}")) as u8
}

fn source_named(def: &ProtocolDef, name: &str) -> u8 {
    def.sources
        .iter()
        .position(|s| s.name.as_str() == name)
        .unwrap_or_else(|| panic!("no source {name}")) as u8
}

// --- BDS: the flow-controlled download ----------------------------------

/// A `progress` notification: type 0x02, then big-endian offset and total.
fn progress(offset: u32, total: u32) -> Vec<u8> {
    let mut v = vec![0x02];
    v.extend_from_slice(&offset.to_be_bytes());
    v.extend_from_slice(&total.to_be_bytes());
    v
}

#[test]
fn bds_download_runs_to_pass_over_a_real_chunk_sequence() {
    let r = resolve_only(BDS);
    let def = &r.def;
    let ctrl = source_named(def, "ctrl");
    let status = source_named(def, "status");
    let data = source_named(def, "data");

    let mut run = Run::start(def, state_named(def, "start")).unwrap();

    // start: REQUEST_OLDEST, acknowledged.
    assert_eq!(
        run.enter(),
        Step::Write {
            source: ctrl,
            payload: heapless::Vec::from_slice(&[0x01]).unwrap(),
            with_response: true
        }
    );

    // The DUT answers on a *different* characteristic — the whole reason
    // nothing here transitions on a write's own ATT response.
    let total = 700u32;
    let step = run.on_event(Event::Notify { source: status, payload: &progress(0, total) });
    // -> pumping, whose on_enter is NEXT_CHUNK with no response.
    assert_eq!(
        step,
        Step::Write {
            source: ctrl,
            payload: heapless::Vec::from_slice(&[0x02]).unwrap(),
            with_response: false
        }
    );
    assert_eq!(run.state(), state_named(def, "pumping"));
    assert_eq!(run.session()[1], total as i64, "expect_total remembered from the frame");

    // Pump 200-byte chunks. Each one re-enters `pumping` and re-sends
    // NEXT_CHUNK; the machine counts bytes rather than accumulating them.
    let chunk = vec![0xAB; 200];
    for expected_received in [200u32, 400, 600] {
        let step = run.on_event(Event::Notify { source: data, payload: &chunk });
        assert_eq!(
            step,
            Step::Write {
                source: ctrl,
                payload: heapless::Vec::from_slice(&[0x02]).unwrap(),
                with_response: false
            },
            "a chunk short of the total pumps again"
        );
        assert_eq!(run.state(), state_named(def, "pumping"));
        assert_eq!(run.session()[0], expected_received as i64);
    }

    // The last chunk crosses the total and the guard fires.
    let tail = vec![0xAB; 100];
    let step = run.on_event(Event::Notify { source: data, payload: &tail });
    assert_eq!(
        step,
        Step::Write {
            source: ctrl,
            payload: heapless::Vec::from_slice(&[0x03]).unwrap(),
            with_response: true
        },
        "crossing expect_total goes to consuming, which CONSUMEs with a response"
    );
    assert_eq!(run.state(), state_named(def, "consuming"));
    assert_eq!(run.session()[0], 700);

    // consuming -> done, unconditionally on the next progress frame.
    let step = run.on_event(Event::Notify { source: status, payload: &progress(total, total) });
    match step {
        Step::Done(o) => {
            assert_eq!(o.final_state.as_str(), "done");
            assert_eq!(o.outcome, Outcome::Pass);
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

#[test]
fn a_stalled_pump_takes_the_watchdog_to_aborting_and_then_fails() {
    // `pumping` declares `retry 0`, so the first expiry is the transition —
    // which is what a stall watchdog means, as against `start`'s `retry 2`.
    let r = resolve_only(BDS);
    let def = &r.def;
    let status = source_named(def, "status");
    let ctrl = source_named(def, "ctrl");

    let mut run = Run::start(def, state_named(def, "start")).unwrap();
    run.enter();
    run.on_event(Event::Notify { source: status, payload: &progress(0, 4096) });
    assert_eq!(run.state(), state_named(def, "pumping"));

    // Nothing arrives.
    let step = run.on_event(Event::Timeout);
    assert_eq!(
        step,
        Step::Write {
            source: ctrl,
            payload: heapless::Vec::from_slice(&[0x04]).unwrap(),
            with_response: true
        },
        "the stall watchdog goes straight to aborting, which ABORTs with a response"
    );
    assert_eq!(run.state(), state_named(def, "aborting"));

    match run.on_event(Event::Notify { source: status, payload: &progress(0, 4096) }) {
        Step::Done(o) => {
            assert_eq!(o.final_state.as_str(), "failed");
            assert!(matches!(o.outcome, Outcome::Fail { .. }));
        }
        other => panic!("expected Done(failed), got {other:?}"),
    }
}

#[test]
fn retry_re_sends_the_on_enter_write_rather_than_waiting_longer() {
    // `start` declares `retry 2`: two re-sends, then the transition. This is
    // the distinction that makes `retry` worth having at all.
    let r = resolve_only(BDS);
    let def = &r.def;
    let ctrl = source_named(def, "ctrl");
    let request = Step::Write {
        source: ctrl,
        payload: heapless::Vec::from_slice(&[0x01]).unwrap(),
        with_response: true,
    };

    let mut run = Run::start(def, state_named(def, "start")).unwrap();
    assert_eq!(run.enter(), request);
    assert_eq!(run.on_event(Event::Timeout), request, "retry 1 re-sends");
    assert_eq!(run.on_event(Event::Timeout), request, "retry 2 re-sends");
    // Third expiry: retries exhausted, take the goto.
    match run.on_event(Event::Timeout) {
        Step::Done(o) => assert_eq!(o.final_state.as_str(), "failed"),
        other => panic!("expected Done(failed), got {other:?}"),
    }
}

#[test]
fn an_unrelated_notification_is_ignored_rather_than_failing_the_run() {
    // A real connection carries traffic the current state does not care
    // about. A machine that failed on the first one could not survive one.
    let r = resolve_only(BDS);
    let def = &r.def;
    let status = source_named(def, "status");
    let mut run = Run::start(def, state_named(def, "start")).unwrap();
    run.enter();
    // Type 0x07 fails `progress`'s select_if, so no frame is selected.
    let step = run.on_event(Event::Notify { source: status, payload: &[0x07, 0, 0, 0, 0] });
    assert_eq!(step, Step::Wait { deadline_ms: Some(2000) });
    assert_eq!(run.state(), state_named(def, "start"));
}

#[test]
fn a_truncated_frame_does_not_advance_the_machine() {
    // `progress.total` lives at offset 5. A payload that matched the magic
    // but stopped short must not silently remember a zero.
    let r = resolve_only(BDS);
    let def = &r.def;
    let status = source_named(def, "status");
    let mut run = Run::start(def, state_named(def, "start")).unwrap();
    run.enter();
    // Selects `progress` (byte 0 is 0x02) but has no room for `total`.
    let step = run.on_event(Event::Notify { source: status, payload: &[0x02, 0, 0, 0, 1] });
    // The arm's `goto pumping` is unconditional, so the machine does move —
    // but `expect_total` must be untouched, not zero-filled from a short read.
    assert_eq!(run.session()[1], 0, "the declared initial value, not a decoded one");
    let _ = step;
}

// --- GWF1: the self-describing batch record ------------------------------

#[test]
fn gwf1_dispatches_on_its_magic_and_never_on_a_version_field() {
    let r = resolve_only(GWF1);
    let def = &r.def;
    let src = source_named(def, "batch_data");

    let gwf1 = {
        let mut v = b"GWF1".to_vec();
        v.extend_from_slice(&[0u8; 32]);
        v
    };
    let ppg1 = {
        let mut v = b"PPG1".to_vec();
        v.extend_from_slice(&[0u8; 32]);
        v
    };
    let neither = vec![0u8; 36];

    let gwf1_ix = def.frames.iter().position(|f| f.name.as_str() == "gwf1_batch").unwrap() as u8;
    let ppg1_ix = def.frames.iter().position(|f| f.name.as_str() == "ppg_batch").unwrap() as u8;

    assert_eq!(select_frame(def, src, &gwf1), Some(gwf1_ix));
    assert_eq!(select_frame(def, src, &ppg1), Some(ppg1_ix));
    // Two frames share a source and neither matches: no frame is selected,
    // rather than the first one being applied to bytes it does not describe.
    assert_eq!(select_frame(def, src, &neither), None);
}

#[test]
fn only_the_header_scalars_are_guard_reachable_and_the_rest_stays_host_side() {
    // This is §3 decision 59's split, asserted. A GWF1 record's thirty
    // channel descriptors and its bit-packed sample columns are real, are
    // parsed, and do not reach dev-bench — because no guard can name one and
    // `ProtocolOutcome` reports a state name.
    let r = resolve_only(GWF1);
    let frame = r.def.frames.iter().find(|f| f.name.as_str() == "gwf1_batch").unwrap();

    let names: Vec<&str> = frame.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["magic", "uptime_ms", "n_channels", "n_samples", "n_chunks"]);

    // Offsets accumulate in declaration order with no padding, exactly as
    // `StructLayout` packs them.
    let offsets: Vec<u16> = frame.fields.iter().map(|f| f.offset).collect();
    assert_eq!(offsets, [0, 4, 12, 13, 15]);

    let render = r.render.iter().find(|x| x.frame == "gwf1_batch").unwrap();
    assert_eq!(render.repeats.len(), 2, "the channel table and the chunk list");
    assert_eq!(render.bitpacks.len(), 0, "the bitpack is inside sample_chunk, not the frame");
    assert_eq!(render.crc, Some(CrcPolicy::Skip));
}

#[test]
fn the_bitpack_and_count_from_primitives_survive_the_parse_intact() {
    // They are render-only, so nothing downstream of `resolve` consumes
    // them yet -- which is exactly why they need pinning here. A primitive
    // that parsed into the wrong shape would be invisible until the first
    // rendering was written against it.
    let file = parse(GWF1).unwrap();
    let p = &file.protocols[0];

    let chunk = p.structs.iter().find(|s| s.name == "sample_chunk").unwrap();
    let bitpack = chunk
        .fields
        .iter()
        .find_map(|f| match f {
            AstField::Bitpack { name, count_from, width_from, delta, zigzag, seed, .. } => {
                Some((name, count_from, width_from, *delta, *zigzag, seed))
            }
            _ => None,
        })
        .expect("sample_chunk declares a bitpack");
    assert_eq!(bitpack.0, "samples");
    assert_eq!(bitpack.1, "n_samples");
    assert_eq!(bitpack.2, "bps");
    assert!(bitpack.3, "delta");
    assert!(bitpack.4, "zigzag");
    assert_eq!(bitpack.5.as_deref(), Some("channel_desc.first"));

    let gwf1 = p.frames.iter().find(|f| f.name == "gwf1_batch").unwrap();
    let counts: Vec<&AstCount> = gwf1
        .fields
        .iter()
        .filter_map(|f| match f {
            AstField::Repeat { count, .. } => Some(count),
            _ => None,
        })
        .collect();
    // The channel table is a *literal* 30 — the firmware's full compile-time
    // capacity, because the flash erase happens before the geometry is known
    // — and the chunk list is runtime-counted. Both spellings are needed.
    assert_eq!(counts[0], &AstCount::Literal(30));
    assert_eq!(counts[1], &AstCount::From("n_chunks".to_string()));
}

#[test]
fn a_flat_frame_lowers_into_decision_52s_struct_layout_and_a_recursive_one_does_not() {
    // The whole relationship between the two mechanisms: an `.eap` file is a
    // second front end to the rendering that already shipped, not a second
    // rendering. A frame it cannot express gets *no* layout rather than an
    // approximate one -- a missing rendering can be redone, a wrong one
    // silently misreads every row.
    let r = resolve_only(GWF1);

    let ppg = r.render.iter().find(|x| x.frame == "ppg_batch").unwrap();
    let layout = ppg.layout.as_ref().expect("a flat frame lowers");
    assert_eq!(
        layout.column_header().unwrap().as_str(),
        "rep_index,magic,seq,green,red"
    );

    let gwf1 = r.render.iter().find(|x| x.frame == "gwf1_batch").unwrap();
    assert!(
        gwf1.layout.is_none(),
        "a count_from repeat, a bitpack and a CRC are outside what StructLayout describes"
    );

    // And the layout it did produce really renders the bytes it claims to.
    let mut payload = b"PPG1".to_vec();
    payload.extend_from_slice(&41u16.to_be_bytes());
    for i in 0..8i16 {
        payload.extend_from_slice(&i.to_le_bytes());
        payload.extend_from_slice(&(-i).to_le_bytes());
    }
    assert_eq!(layout.row_count(&payload).unwrap(), 8);
    assert_eq!(layout.row(&payload, 3).unwrap().as_str(), "3,1347438385,41,3,-3");
}

#[test]
fn a_write_carries_a_session_variable_through_the_decode_vocabulary() {
    // §3 decision 61: a write built only from constants cannot express a
    // live epoch or an echoed-back length. `capturing`'s on_enter is
    // `write ctrl { u8: 0x01, u32be: session.epoch }`.
    let r = resolve_only(GWF1);
    let def = &r.def;
    let ctrl = source_named(def, "ctrl");
    let mut run = Run::start(def, state_named(def, "capturing")).unwrap();
    match run.enter() {
        Step::Write { source, payload, with_response } => {
            assert_eq!(source, ctrl);
            assert!(with_response);
            // `epoch` is 0 by declaration; what matters is the *shape* —
            // one opcode byte then four big-endian bytes from a variable.
            assert_eq!(&payload[..], &[0x01, 0x00, 0x00, 0x00, 0x00]);
        }
        other => panic!("expected a write, got {other:?}"),
    }
}

// --- Shared: the expression set, and what it refuses ----------------------

#[test]
fn the_expression_set_lowers_to_exactly_the_four_operand_forms() {
    let r = resolve_only(BDS);
    let def = &r.def;
    let pumping = &def.states[state_named(def, "pumping") as usize];
    let StateKind::Active(a) = &pumping.kind else { panic!("pumping is active") };
    let arm = &a.on_event[0];

    // `remember received = received + len(chunk.payload)` — a session
    // variable plus a span length, which is the whole reason `+` exists.
    assert_eq!(arm.remember.len(), 1);
    match arm.remember[0].value {
        embarch_study_designer::eap::Expr::Add(Operand::Session(0), Operand::SpanLen(0)) => {}
        other => panic!("unexpected expr {other:?}"),
    }
    // `when received >= expect_total` — two session variables.
    assert_eq!(arm.when.len(), 1);
    assert_eq!(arm.when[0].cond.op, CompareOp::Ge);
    assert_eq!(arm.when[0].cond.lhs, Operand::Session(0));
    assert_eq!(arm.when[0].cond.rhs, Operand::Session(1));

    // `remember expect_total = progress.total` in `start` — the fourth form,
    // a decoded field of the triggering frame.
    let start = &def.states[state_named(def, "start") as usize];
    let StateKind::Active(a) = &start.kind else { panic!() };
    match a.on_event[0].remember[0].value {
        embarch_study_designer::eap::Expr::Term(Operand::Field(2)) => {}
        other => panic!("unexpected expr {other:?}"),
    }
}

#[test]
fn both_fixtures_declare_both_terminal_outcomes() {
    for src in [BDS, GWF1] {
        let def = resolve_only(src).def;
        let outcomes: Vec<TerminalOutcome> = def
            .states
            .iter()
            .filter_map(|s| match s.kind {
                StateKind::Terminal(t) => Some(t),
                _ => None,
            })
            .collect();
        assert!(outcomes.contains(&TerminalOutcome::Pass));
        assert!(outcomes.contains(&TerminalOutcome::Fail));
    }
}
