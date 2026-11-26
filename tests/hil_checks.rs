//! The definition-time checks of a HIL test (`hil_check::check_test`): the
//! worked example passes, and each refusal has a fixture that names the test,
//! the field and the reason.
#![cfg(feature = "hil-test")]

use std::collections::BTreeMap;

use embarch_study_designer::hil_check::*;
use embarch_study_designer::hil_test::*;
use embarch_study_designer::host_study::{HostAction, HostStep, HostStudy, PromptKind};
use serde_json::json;

fn raw(name: &str) -> String {
    let path = format!("{}/tests/fixtures/hil-tests/{name}.json", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path).unwrap()
}

/// The worked example, its `study.hash` set to the hash of `study.json`.
fn test() -> TestDef {
    let mut t: TestDef = serde_json::from_str(&raw("test")).unwrap();
    t.study.hash = study_hash(&study());
    t
}

fn bench() -> SuiteBench {
    let s: SuiteManifest = serde_json::from_str(&raw("suite")).unwrap();
    s.benches[0].clone()
}

fn study() -> HostStudy {
    serde_json::from_str(&raw("study")).unwrap()
}

fn metrology(res: f64, u: f64, lo: f64, hi: f64) -> Metrology {
    Metrology { range_min: Some(lo), range_max: Some(hi), resolution: Some(res), u: Some(u) }
}

fn measured() -> BTreeMap<String, Measured> {
    let mut m = BTreeMap::new();
    m.insert("vbus_mean".into(), Measured { by: By::Bench, source: "dmm".into(), metrology: metrology(0.001, 0.01, 0.0, 20.0) });
    m.insert("vbus_ripple".into(), Measured { by: By::Bench, source: "dmm".into(), metrology: metrology(0.0001, 0.005, 0.0, 0.5) });
    m.insert("led_state".into(), Measured { by: By::Person, source: "the operator".into(), metrology: Metrology::default() });
    m
}

/// The refusals of `t` against `s`, which `t` is made to name by hash, so a
/// fixture that edits the study meets only the refusal it is about.
fn refused(t: &TestDef, s: &HostStudy, b: &SuiteBench, m: &BTreeMap<String, Measured>) -> Vec<Refusal> {
    let mut t = t.clone();
    t.study.hash = study_hash(s);
    check_test(&t, StudyKind::Host(s), b, m).unwrap_err()
}

/// `t` passes every check against `s`, which it is made to name by hash.
fn accepted(t: &TestDef, s: &HostStudy, b: &SuiteBench, m: &BTreeMap<String, Measured>) {
    let mut t = t.clone();
    t.study.hash = study_hash(s);
    check_test(&t, StudyKind::Host(s), b, m).unwrap();
}

/// Exactly one refusal, of `rule`, on `field`, naming the test.
fn only(r: Vec<Refusal>, rule: Rule, field: &str) -> Refusal {
    assert_eq!(r.len(), 1, "{r:?}");
    let r = r.into_iter().next().unwrap();
    assert_eq!((r.rule, r.field.as_str()), (rule, field), "{r:?}");
    assert_eq!(r.test, "vbus-5v-source-voltage");
    assert!(!r.reason.is_empty());
    assert!(r.to_string().starts_with("vbus-5v-source-voltage: "));
    r
}

fn step(name: &str, action: HostAction) -> HostStep {
    HostStep { name: name.into(), action, timeout_ms: 1000, continue_on_fail: false, delay_before_ms: 0 }
}

fn prompt(kind: Option<PromptKind>, confirmed_by: Option<&str>) -> HostAction {
    HostAction::Prompt {
        text: "Do it".into(),
        bench_config: None,
        kind,
        confirmed_by: confirmed_by.map(String::from),
        answer: None,
    }
}

fn raw_dut_line() -> HostAction {
    HostAction::Exchange {
        signal: "unit".into(),
        line: "vbus on".into(),
        until: "ok".into(),
        match_after_echo: false,
        refused_on: None,
        parse: None,
    }
}

#[test]
fn the_worked_example_passes_every_check() {
    check_test(&test(), StudyKind::Host(&study()), &bench(), &measured()).unwrap();
}

#[test]
fn a_bench_with_no_dut_is_refused_naming_the_bench() {
    let mut b = bench();
    b.description = json!({ "parts": [ { "id": "partner", "template": "nucleo-g0b1re" } ], "configs": [] });
    let r = only(refused(&test(), &study(), &b, &measured()), Rule::BenchHasNoDut, "bench.name");
    assert!(r.reason.contains("pd-sink-bench"));
    // The legacy shape names its DUT under `dut`.
    b.description = json!({ "dut": { "board": "nrf54l15dk" } });
    check_test(&test(), StudyKind::Host(&study()), &b, &measured()).unwrap();
    b.description = json!({ "dev-bench": {} });
    only(refused(&test(), &study(), &b, &measured()), Rule::BenchHasNoDut, "bench.name");
}

#[test]
fn another_bench_than_the_one_named_is_refused() {
    let mut b = bench();
    b.name = "other".into();
    only(refused(&test(), &study(), &b, &measured()), Rule::WrongBench, "bench.name");
}

#[test]
fn a_source_with_no_declared_u_resolution_or_range_is_refused_by_name() {
    for (name, drop) in [("U", 0), ("resolution", 1), ("range", 2)] {
        let mut m = measured();
        let md = &mut m.get_mut("vbus_mean").unwrap().metrology;
        match drop {
            0 => md.u = None,
            1 => md.resolution = Some(0.0),
            _ => md.range_max = None,
        }
        let r = only(refused(&test(), &study(), &bench(), &m), Rule::SourceUndeclared, "limits[0] (vbus_mean)");
        assert!(r.reason.contains("`dmm`") && r.reason.contains(name), "{r:?}");
    }
}

#[test]
fn a_dut_measurement_takes_its_numbers_from_the_tests_sources() {
    let mut t = test();
    t.limits.push(TestLimit { measurement: "dut_vbus_report".into(), limit: LimitSpec::Band { lo: 4.5, hi: 5.5 } });
    let mut m = measured();
    m.insert("dut_vbus_report".into(), Measured { by: By::Dut, source: "unit".into(), metrology: Metrology::default() });
    check_test(&t, StudyKind::Host(&study()), &bench(), &m).unwrap();
    t.sources.clear();
    let r = only(refused(&t, &study(), &bench(), &m), Rule::SourceUndeclared, "limits[3] (dut_vbus_report)");
    assert!(r.reason.contains("sources"));
    // A declared but zero U is not declared.
    let mut t = test();
    t.limits.push(TestLimit { measurement: "dut_vbus_report".into(), limit: LimitSpec::Band { lo: 4.5, hi: 5.5 } });
    t.sources[0].u = 0.0;
    only(refused(&t, &study(), &bench(), &m), Rule::SourceUndeclared, "limits[3] (dut_vbus_report)");
}

#[test]
fn a_limit_no_result_could_be_judged_against_is_refused_by_u_then_by_resolution() {
    // 4.99 .. 5.01 is 0.02 wide; 2 x max(w = U = 0.01, 0.01) = 0.02.
    let mut t = test();
    t.limits[0].limit = LimitSpec::Band { lo: 4.99, hi: 5.01 };
    let r = only(refused(&t, &study(), &bench(), &measured()), Rule::LimitUnjudgeable, "limits[0] (vbus_mean)");
    assert!(r.reason.contains("U"), "{r:?}");
    // Wide enough for U, not for the resolution.
    let mut m = measured();
    m.get_mut("vbus_mean").unwrap().metrology = metrology(0.05, 0.001, 0.0, 20.0);
    let mut t = test();
    t.decision_rule = DecisionRuleSpec::Simple;
    t.limits[0].limit = LimitSpec::Band { lo: 4.95, hi: 5.04 };
    let r = only(refused(&t, &study(), &bench(), &m), Rule::LimitUnjudgeable, "limits[0] (vbus_mean)");
    assert!(r.reason.contains("resolution"), "{r:?}");
    // A one-sided limit too close to the range end.
    let mut t = test();
    t.limits[1].limit = LimitSpec::AtMost { max: 0.004 };
    only(refused(&t, &study(), &bench(), &measured()), Rule::LimitUnjudgeable, "limits[1] (vbus_ripple)");
    // An `expect` limit has no width.
    check_test(&test(), StudyKind::Host(&study()), &bench(), &measured()).unwrap();
}

#[test]
fn a_limit_on_a_measurement_nobody_measures_is_refused() {
    let mut m = measured();
    m.remove("vbus_ripple");
    only(refused(&test(), &study(), &bench(), &m), Rule::UnknownMeasurement, "limits[1] (vbus_ripple)");
}

#[test]
fn a_prompt_with_no_kind_is_refused() {
    let mut s = study();
    s.steps[0] = step("set up", prompt(None, None));
    only(refused(&test(), &s, &bench(), &measured()), Rule::PromptHasNoKind, "study.steps[0]");
}

#[test]
fn an_action_prompt_needs_a_bench_measurement_that_feeds_a_limit() {
    let mut t = test();
    t.limits.push(TestLimit { measurement: "attach_seen".into(), limit: LimitSpec::Expect { value: "attached".into() } });
    let mut m = measured();
    m.insert("attach_seen".into(), Measured { by: By::Bench, source: "partner".into(), metrology: Metrology::default() });
    let with = |by: Option<&str>| {
        let mut s = study();
        s.steps.insert(0, step("plug", prompt(Some(PromptKind::Action), by)));
        // A clearance prompt follows, so the energising rule stays quiet.
        s.steps.insert(1, step("clear", prompt(Some(PromptKind::Clearance), None)));
        s
    };
    accepted(&t, &with(Some("attach_seen")), &bench(), &m);
    // Names nothing.
    only(refused(&t, &with(None), &bench(), &m), Rule::ActionUnconfirmed, "study.steps[0]");
    // Names a measurement of the study that does not exist.
    only(refused(&t, &with(Some("nope")), &bench(), &m), Rule::ActionUnconfirmed, "study.steps[0]");
    // Not by: bench.
    m.get_mut("attach_seen").unwrap().by = By::Person;
    let r = only(refused(&t, &with(Some("attach_seen")), &bench(), &m), Rule::ActionUnconfirmed, "study.steps[0]");
    assert!(r.reason.contains("by: bench"));
    // Feeds no limit.
    m.get_mut("attach_seen").unwrap().by = By::Bench;
    let r = only(refused(&test(), &with(Some("attach_seen")), &bench(), &m), Rule::ActionUnconfirmed, "study.steps[0]");
    assert!(r.reason.contains("no limit"));
}

#[test]
fn an_energising_step_after_a_handling_prompt_needs_a_clearance_between() {
    for kind in [PromptKind::Setup, PromptKind::Action] {
        let confirmed = (kind == PromptKind::Action).then_some("vbus_mean");
        // The example's own clearance is removed: the setup prompt is followed
        // by an instrument set.
        let mut s = study();
        s.steps = vec![step("handle", prompt(Some(kind), confirmed)), step("on", s.steps[2].action.clone())];
        let r = only(refused(&test(), &s, &bench(), &measured()), Rule::EnergisesWithoutClearance, "study.steps[1]");
        assert!(r.reason.contains("study.steps[0]"));
        // With a clearance prompt between, it passes.
        s.steps.insert(1, step("clear", prompt(Some(PromptKind::Clearance), None)));
        accepted(&test(), &s, &bench(), &measured());
    }
}

#[test]
fn a_raw_dut_exchange_after_an_action_prompt_is_refused_unless_cleared() {
    let mut s = study();
    s.steps = vec![step("plug", prompt(Some(PromptKind::Action), Some("vbus_mean"))), step("raw", raw_dut_line())];
    only(refused(&test(), &s, &bench(), &measured()), Rule::EnergisesWithoutClearance, "study.steps[1]");
    s.steps.insert(1, step("clear", prompt(Some(PromptKind::Clearance), None)));
    accepted(&test(), &s, &bench(), &measured());
}

#[test]
fn what_energises() {
    let b = bench();
    let read = |line: &str| HostAction::Exchange {
        signal: "partner".into(),
        line: line.into(),
        until: "ok".into(),
        match_after_echo: false,
        refused_on: None,
        parse: None,
    };
    let inst = |q| HostAction::Instrument { instrument: "dmm".into(), line: "X".into(), query: q, parse: None };
    // A bench-board line the bench lists as read-only does not; one it does not list does.
    assert!(!energises(&step("a", read("status")), &b));
    assert!(!energises(&step("a", read(" vbus ")), &b));
    assert!(energises(&step("a", read("vbus_on")), &b));
    // Every DUT line does.
    assert!(energises(&step("a", raw_dut_line()), &b));
    // An instrument set does, a query does not.
    assert!(energises(&step("a", inst(false)), &b));
    assert!(!energises(&step("a", inst(true)), &b));
    assert!(!energises(&step("a", HostAction::Wait { ms: 5 }), &b));
    assert!(!energises(&step("a", prompt(Some(PromptKind::Setup), None)), &b));
    let rep = |steps| HostAction::Repeat { count: Some(2), values: vec![], steps };
    assert!(energises(&step("r", rep(vec![step("x", inst(true)), step("y", inst(false))])), &b));
    assert!(!energises(&step("r", rep(vec![step("x", inst(true))])), &b));
}

#[test]
fn a_repeat_carries_a_handling_prompt_into_its_next_pass() {
    let mut s = study();
    let body = vec![step("on", HostAction::Instrument { instrument: "supply".into(), line: "OUTP ON".into(), query: false, parse: None }), step("plug", prompt(Some(PromptKind::Setup), None))];
    s.steps = vec![step("loop", HostAction::Repeat { count: Some(3), values: vec![], steps: body.clone() })];
    only(refused(&test(), &s, &bench(), &measured()), Rule::EnergisesWithoutClearance, "study.steps[0].steps[0]");
    // One pass has nothing after the prompt.
    s.steps = vec![step("loop", HostAction::Repeat { count: Some(1), values: vec![], steps: body })];
    accepted(&test(), &s, &bench(), &measured());
}

#[test]
fn a_precondition_the_bench_does_not_declare_is_refused() {
    for (p, name) in [(Precondition::PowerCycle, "power_cycle"), (Precondition::Detach, "detach")] {
        let mut t = test();
        t.precondition = p;
        let mut b = bench();
        b.boards[0].preconditions = vec![Precondition::Reset];
        let r = only(refused(&t, &study(), &b, &measured()), Rule::PreconditionUndeclared, "precondition");
        assert!(r.reason.contains(name));
        b.boards[0].preconditions.push(p);
        check_test(&t, StudyKind::Host(&study()), &b, &measured()).unwrap();
    }
    // `reset` and `none` need no declaration.
    for p in [Precondition::Reset, Precondition::None] {
        let mut t = test();
        t.precondition = p;
        let mut b = bench();
        b.boards[0].preconditions.clear();
        check_test(&t, StudyKind::Host(&study()), &b, &measured()).unwrap();
    }
}

#[test]
fn a_dev_bench_study_is_not_in_this_phase() {
    let r = check_test(&test(), StudyKind::DevBench, &bench(), &measured()).unwrap_err();
    let r = only(r, Rule::DevBenchStudy, "study");
    assert!(r.reason.contains("not in this phase"));
}

#[test]
fn two_limits_on_one_measurement_are_refused() {
    let mut t = test();
    t.limits.push(TestLimit { measurement: "vbus_mean".into(), limit: LimitSpec::Band { lo: 4.0, hi: 6.0 } });
    let r = only(refused(&t, &study(), &bench(), &measured()), Rule::DuplicateLimit, "limits[3] (vbus_mean)");
    assert!(r.reason.contains("derived"));
}

#[test]
fn every_refusal_is_reported_not_only_the_first() {
    let mut t = test();
    t.precondition = Precondition::Detach;
    t.limits.push(TestLimit { measurement: "vbus_mean".into(), limit: LimitSpec::Band { lo: 4.0, hi: 6.0 } });
    let mut b = bench();
    b.boards[0].preconditions.clear();
    b.description = json!({ "parts": [] });
    let r = refused(&t, &study(), &b, &measured());
    let rules: Vec<Rule> = r.iter().map(|x| x.rule).collect();
    assert_eq!(rules, [Rule::BenchHasNoDut, Rule::PreconditionUndeclared, Rule::DuplicateLimit]);
}

#[test]
fn a_study_that_does_not_hash_to_study_hash_is_refused() {
    let t = test();
    accepted(&t, &study(), &bench(), &measured());
    let mut other = study();
    other.name.push('x');
    let r = check_test(&t, StudyKind::Host(&other), &bench(), &measured()).unwrap_err();
    let r = only(r, Rule::StudyHashMismatch, "study.hash");
    assert!(r.reason.contains(&t.study.file), "{r:?}");
    assert!(r.reason.contains(&t.study.hash), "{r:?}");
    assert!(r.reason.contains(&study_hash(&other)), "{r:?}");
}

#[test]
fn the_study_hash_is_pinned_and_ignores_the_files_layout() {
    // A fixed input to its hash: SHA-256 of the canonical JSON of the study.
    let s = study();
    let h = study_hash(&s);
    assert_eq!(h.len(), 64);
    assert_eq!(h, definition_hash(&s));
    assert_eq!(h, PINNED_STUDY_HASH);
    // Whitespace and key order in the file do not change it.
    let v: serde_json::Value = serde_json::from_str(&raw("study")).unwrap();
    let respaced: HostStudy = serde_json::from_str(&serde_json::to_string_pretty(&v).unwrap()).unwrap();
    assert_eq!(study_hash(&respaced), h);
    // A changed step does.
    let mut t = s.clone();
    t.name.push('x');
    assert_ne!(study_hash(&t), h);
}

const PINNED_STUDY_HASH: &str = "985e5888b3c5c8f1b7f57b05cd91a49c7780ee2d0814e92e92641ded9092c22b";
