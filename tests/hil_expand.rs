//! The expansion of a selection into `run.json` (`hil_expand::expand`).
#![cfg(feature = "hil-test")]

use std::collections::BTreeMap;

use embarch_study_designer::hil_expand::*;
use embarch_study_designer::hil_test::*;
use embarch_study_designer::host_study::HostStudy;
use serde_json::{json, Value};

fn study() -> HostStudy {
    serde_json::from_value(json!({
        "name": "pdo",
        "steps": [
            { "name": "set up", "action": { "Prompt": { "text": "Wire it", "kind": "setup" } }, "timeout_ms": 60000 },
            { "name": "on", "action": { "Instrument": { "instrument": "supply", "line": "VOLT {capability.mv}" } }, "timeout_ms": 2000 },
            { "name": "read", "action": { "Exchange": { "signal": "partner", "line": "vbus {capability.mv}", "until": "ok",
                "parse": { "name": "vbus_{capability.mv}", "pattern": "([0-9.]+)", "unit": "V" } } }, "timeout_ms": 3000 },
            { "name": "again", "delay_before_ms": 100, "action": { "Repeat": { "count": 2, "steps": [
                { "name": "poll", "action": { "Exchange": { "signal": "partner", "line": "status", "until": "ok" } }, "timeout_ms": 500 } ] } } }
        ],
        "teardown": [ { "name": "off", "action": { "Instrument": { "instrument": "supply", "line": "OUTP OFF" } }, "timeout_ms": 1000 } ]
    }))
    .unwrap()
}

/// 60000 + 2000 + 3000 + (100 + 2 * 500) + 1000.
const BUDGET: u64 = 67_100;

fn def(id: &str, extra: Value) -> TestDef {
    let mut v = json!({
        "schema": 1, "test_id": id, "title": id,
        "study": { "file": "pdo.study.json", "hash": "aa" },
        "requirements": ["r"], "revision_range": { "from": null, "to": null },
        "bench": { "name": "bench-a", "config": "default" },
        "precondition": "reset", "run_count": 2,
        "limits": [ { "measurement": "vbus_{capability.mv}", "limit": { "kind": "band", "lo": 1.0, "hi": 2.0 } } ],
        "decision_rule": { "kind": "simple" }, "tags": []
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    serde_json::from_value(v).unwrap()
}

fn tests() -> Vec<TestDef> {
    vec![
        def("a-cap-check", json!({ "checks": "pdos", "tags": ["check"] })),
        def("pdo-voltage", json!({ "template": { "capability": "pdos" }, "tags": ["vbus"] })),
        def("swap", json!({ "hooks": ["dut.swap"], "tags": ["swap"] })),
        def("sink-only", json!({ "applies_when": [ { "kind": "equals", "capability": "role", "value": "sink" } ] })),
        def("far", json!({ "bench": { "name": "ghost", "config": "default" } })),
        def("late", json!({ "requires": ["pdo-voltage"], "tags": ["late"] })),
    ]
}

fn suite_manifest(tests: &[TestDef]) -> SuiteManifest {
    SuiteManifest {
        schema: 1,
        id: "s".into(),
        version: "1".into(),
        validated_with: "e".into(),
        tests: tests.iter().map(|t| SuiteTest { test_id: t.test_id.clone(), file: format!("{}.json", t.test_id), hash: "h".into() }).collect(),
        benches: ["bench-a", "ghost"]
            .iter()
            .map(|n| SuiteBench {
                name: (*n).into(),
                description: json!({}),
                boards: vec![BenchBoard { part: "partner".into(), idle: "idle".into(), hello: "hello".into(), read_only: vec![], preconditions: vec![Precondition::Reset] }],
            })
            .collect(),
    }
}

fn dut() -> DutInputs {
    DutInputs {
        capabilities: vec![
            Capability { name: "role".into(), value: json!("source"), provenance: Provenance::Declared },
            Capability { name: "pdos".into(), value: json!([{ "mv": 5000 }, { "mv": 9000 }, { "mv": 15000 }]), provenance: Provenance::Measured },
        ],
        hooks: vec![],
    }
}

fn plan(select: Selection) -> Plan {
    Plan {
        schema: 1,
        suite: SuitePin { id: "s".into(), version: "1".into(), commit: "c0".into() },
        select,
        dut: dut(),
        stop_on_fail: false,
        bench_order: vec![],
    }
}

fn ctx() -> ExpandContext {
    ExpandContext {
        run_id: "run-1".into(),
        plan_hash: None,
        standard: Standard { name: "std".into(), revision: "1".into() },
        identity: DutIdentity { hardware_id: None, declared_serial: "x".into(), board_revision: "A".into() },
        resumes: None,
        benches_present: vec!["bench-a".into()],
        confirmations: vec![Confirmation { scope: "capability:role".into(), hash: "h".into(), at_utc_ms: 5, surface: Surface::Ui }],
    }
}

fn run(ts: &[TestDef], p: &Plan, d: &DutInputs) -> Expansion {
    let m = suite_manifest(ts);
    let mut studies = BTreeMap::new();
    studies.insert("pdo.study.json".to_string(), study());
    expand(&SuiteContent { manifest: &m, tests: ts, studies: &studies }, p, d, &ctx()).unwrap()
}

fn status(e: &Expansion, id: &str) -> SelectionStatus {
    e.selection.entries.iter().find(|x| x.test_id == id).unwrap().status
}

#[test]
fn same_bytes_and_hash_whatever_the_file_order() {
    let ts = tests();
    let a = run(&ts, &plan(Selection::All), &dut());
    let mut rev = ts.clone();
    rev.reverse();
    let b = run(&rev, &plan(Selection::All), &dut());
    assert_eq!(a.run.canonical(), b.run.canonical());
    assert_eq!(a.run.hash(), b.run.hash());
    assert_eq!(a.run.hash(), run(&ts, &plan(Selection::All), &dut()).run.hash());
    assert_eq!(a.run.hash().len(), 64);
}

#[test]
fn statuses_and_template_expansion() {
    let e = run(&tests(), &plan(Selection::All), &dut());
    assert_eq!(status(&e, "pdo-voltage"), SelectionStatus::Selected);
    assert_eq!(status(&e, "swap"), SelectionStatus::HookUnmapped);
    assert_eq!(status(&e, "sink-only"), SelectionStatus::NotApplicable);
    assert_eq!(status(&e, "far"), SelectionStatus::BenchMissing);
    let ids: Vec<&str> = e.run.tests.iter().map(|t| t.test_id.as_str()).collect();
    assert_eq!(ids, ["a-cap-check", "pdo-voltage#1", "pdo-voltage#2", "pdo-voltage#3", "late"]);
    let t2 = &e.run.tests[2];
    assert_eq!(t2.expanded_from.as_ref().unwrap().value, json!({ "mv": 9000 }));
    assert_eq!(t2.limits[0].measurement, "vbus_9000");
    assert_eq!(t2.definition_hash, e.run.tests[1].definition_hash);
    assert_eq!(e.studies.len(), 3);
    assert_eq!(e.studies[1].hash, t2.study.hash);
    assert!(e.studies[1].bytes.contains("VOLT 9000"));
    assert_ne!(e.studies[0].hash, e.studies[1].hash);
    let late = &e.run.tests[4];
    assert_eq!(late.requires, ["pdo-voltage#1", "pdo-voltage#2", "pdo-voltage#3"]);
    assert_eq!(e.run.dut.hooks[0].name, "dut.swap");
    assert_eq!(e.run.dut.hooks[0].maps_to, Value::Null);
    assert!(e.run.dut.capabilities.iter().any(|c| c.name == "role" && c.confirmed.is_some()));
    assert_eq!(e.run.benches[0].boards[0].hello, "hello");
}

#[test]
fn capability_check_is_first_and_required() {
    let e = run(&tests(), &plan(Selection::Some { tests: vec!["pdo-voltage".into()], tags: vec![] }), &dut());
    assert_eq!(e.run.tests[0].test_id, "a-cap-check", "pulled in, first on its bench");
    let entry = e.selection.entries.iter().find(|x| x.test_id == "a-cap-check").unwrap();
    assert_eq!((entry.status, entry.reason.as_deref()), (SelectionStatus::Selected, Some("required by pdo-voltage")));
    for t in &e.run.tests[1..] {
        assert_eq!(t.requires, ["a-cap-check"]);
    }
    assert_eq!(status(&e, "late"), SelectionStatus::NotSelected);
}

#[test]
fn a_check_that_sorts_last_still_runs_first() {
    let mut ts = tests();
    for t in &mut ts {
        if t.test_id == "a-cap-check" {
            t.test_id = "z-cap-check".into();
        }
    }
    let e = run(&ts, &plan(Selection::All), &dut());
    assert_eq!(e.run.tests[0].test_id, "z-cap-check");
}

#[test]
fn duration_is_the_hand_summed_timeouts() {
    let e = run(&tests(), &plan(Selection::All), &dut());
    // 5 tests, each run_count 2.
    assert_eq!(e.run.tests[0].step_budget_ms, BUDGET);
    assert_eq!(duration_bound(&e.run), 5 * 2 * BUDGET);
}

#[test]
fn a_mapped_hook_selects_and_bench_order_follows_the_plan() {
    let mut d = dut();
    d.hooks.push(HookMapping { hook: "dut.swap".into(), target: HookTarget::Shell { command: "pd swap".into() } });
    let mut ts = tests();
    ts.push(def("on-ghost", json!({ "bench": { "name": "ghost", "config": "default" } })));
    let mut p = plan(Selection::All);
    let mut c = ctx();
    c.benches_present.push("ghost".into());
    let m = suite_manifest(&ts);
    let mut studies = BTreeMap::new();
    studies.insert("pdo.study.json".to_string(), study());
    let suite = SuiteContent { manifest: &m, tests: &ts, studies: &studies };
    let e = expand(&suite, &p, &d, &c).unwrap();
    assert_eq!(status(&e, "swap"), SelectionStatus::Selected);
    assert_eq!(e.run.benches[0].name, "bench-a");
    p.bench_order = vec!["ghost".into()];
    let e = expand(&suite, &p, &d, &c).unwrap();
    assert_eq!(e.run.benches[0].name, "ghost");
    assert_eq!(e.run.dut.hooks[0].maps_to, json!({ "shell": "pd swap" }));
}

#[test]
fn refusals() {
    let ts = tests();
    let m = suite_manifest(&ts);
    let studies = BTreeMap::new();
    let suite = SuiteContent { manifest: &m, tests: &ts, studies: &studies };
    assert!(matches!(expand(&suite, &plan(Selection::All), &dut(), &ctx()), Err(ExpandError::StudyMissing { .. })));
    let mut c = ctx();
    c.run_id.clear();
    assert_eq!(expand(&suite, &plan(Selection::All), &dut(), &c), Err(ExpandError::EmptyRunId));
}
