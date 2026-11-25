//! `judge_run`, `judge_test` and `skip` against one fixture per path
//! (`tests/fixtures/hil_judge.json`), with no Core installed.
#![cfg(feature = "hil-test")]

use embarch_study_designer::hil_expand::RunTest;
use embarch_study_designer::hil_judge::*;
use embarch_study_designer::verdicts::*;
use serde_json::{json, Value};

fn fixture() -> Value {
    let path = format!("{}/tests/fixtures/hil_judge.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn test_of(rule: Value, limits: Value) -> RunTest {
    serde_json::from_value(json!({
        "test_id": "t", "title": "t", "definition_hash": "h",
        "study": {"kind": "host", "hash": "s", "file": "s.json"},
        "requirements": [], "revision_range": {"from": null, "to": null},
        "bench": {"name": "b", "config": "c"}, "precondition": "reset",
        "requires": [], "run_count": 1, "decision_rule": rule,
        "limits": limits, "setup_terms": [], "sources": [], "prompt_kinds": [],
        "hooks": [], "step_budget_ms": 0
    }))
    .unwrap()
}

#[test]
fn run_fixture() {
    let rules = Rules::latest();
    let doc = fixture();
    for case in doc["runs"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let test = test_of(case["decision_rule"].clone(), case["limits"].clone());
        let steps: Vec<StepRecord> = serde_json::from_value(case["steps"].clone()).unwrap();
        let ms: Vec<MeasurementRecord> = serde_json::from_value(case["measurements"].clone()).unwrap();
        let j = judge_run(&test, &steps, &ms, &rules).unwrap();
        assert_eq!(serde_json::to_value(j.verdict).unwrap(), case["expect"]["verdict"], "{name}: verdict");
        assert_eq!(serde_json::to_value(j.reason).unwrap(), case["expect"]["reason"], "{name}: reason");
        assert_eq!(serde_json::to_value(&j.qualifiers).unwrap(), case["expect"]["qualifiers"], "{name}: qualifiers");
    }
}

#[test]
fn two_runs_one_pass_one_fail_is_fail_with_one_each() {
    let rules = Rules::latest();
    let runs = [
        Judged { verdict: Verdict::Pass, reason: None, qualifiers: vec![] },
        Judged { verdict: Verdict::Fail, reason: Some(Reason::Limit), qualifiers: vec![Qualifier::DeclaredByStudy] },
    ];
    let t = judge_test(&runs, &rules).unwrap();
    assert_eq!(t.judged.verdict, Verdict::Fail);
    assert_eq!(t.judged.reason, Some(Reason::Limit));
    assert_eq!(t.judged.qualifiers, vec![Qualifier::DeclaredByStudy]);
    assert_eq!((t.counts.pass, t.counts.fail), (1, 1));
}

#[test]
fn skip_takes_only_skipped_reasons() {
    for r in [
        Reason::RequiresNotMet,
        Reason::StoppedByError,
        Reason::StoppedAfterFail,
        Reason::BenchMissing,
        Reason::HookUnmapped,
        Reason::NotApplicable,
        Reason::NotSelected,
    ] {
        let j = skip(r).unwrap();
        assert_eq!((j.verdict, j.reason), (Verdict::Skipped, Some(r)));
    }
    assert_eq!(skip(Reason::Limit), Err(JudgeError::NotASkipReason(Reason::Limit)));
}

#[test]
fn unvalidated_adds_its_qualifier_once() {
    let j = unvalidated(unvalidated(skip(Reason::NotSelected).unwrap()));
    assert_eq!(j.qualifiers, vec![Qualifier::Unvalidated]);
}

#[test]
fn unknown_decision_rule_and_unpinned_rules_are_errors() {
    let test = test_of(json!({"kind": "mystery", "w_multiple": 0.0}), json!([]));
    assert_eq!(
        judge_run(&test, &[], &[], &Rules::latest()),
        Err(JudgeError::UnknownDecisionRule("mystery".into()))
    );
    let test = test_of(json!({"kind": "simple_acceptance", "w_multiple": 0.0}), json!([]));
    let none = Rules::pinned(&[]).unwrap();
    assert_eq!(judge_run(&test, &[], &[], &none), Err(JudgeError::Rule(RuleError::NotPinned("combine"))));
    let step = [StepRecord { step: "s".into(), target: Target::Dut, outcome: StepOutcome::Pass }];
    assert_eq!(judge_run(&test, &step, &[], &none), Err(JudgeError::Rule(RuleError::NotPinned("step_verdict"))));
}

#[test]
fn behaviours_used_are_the_three_composed() {
    let used = behaviours_used(&Rules::latest());
    assert_eq!(used, vec![("step_verdict", 1), ("limit_verdict", 1), ("combine", 1)]);
}
