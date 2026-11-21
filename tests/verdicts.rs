//! Each verdict rule against its own fixture (`tests/fixtures/verdicts/`), with
//! no Core installed, plus the contract tests for behaviour versioning.
#![cfg(feature = "verdicts")]

use embarch_study_designer::verdicts::*;
use serde_json::{json, Value};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/verdicts/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn label(case: &Value) -> String {
    case["name"].as_str().unwrap().to_string()
}

fn expect_judged(j: &Judged, e: &Value, l: &str) {
    assert_eq!(serde_json::to_value(j.verdict).unwrap(), e["verdict"], "{l}: verdict");
    assert_eq!(serde_json::to_value(j.reason).unwrap(), e["reason"], "{l}: reason");
    if let Some(r) = j.reason {
        assert_eq!(r.verdict(), j.verdict, "{l}: reason belongs to its verdict");
    }
}

#[test]
fn step_fixture() {
    let rules = Rules::latest();
    let doc = fixture("step");
    for case in doc["cases"].as_array().unwrap() {
        let target: Target = serde_json::from_value(case["target"].clone()).unwrap();
        let outcome: StepOutcome = serde_json::from_value(case["input"].clone()).unwrap();
        let j = rules.step_verdict(target, outcome).unwrap();
        expect_judged(&j, &case["expect"], &label(case));
    }
}

#[test]
fn limit_fixture() {
    let rules = Rules::latest();
    let doc = fixture("limit");
    for case in doc["cases"].as_array().unwrap() {
        let limit: Limit = serde_json::from_value(case["limit"].clone()).unwrap();
        let ev: Evidence = case.get("evidence").map(|e| serde_json::from_value(e.clone()).unwrap()).unwrap_or_default();
        let j = rules
            .limit_verdict(case["value"].as_f64().unwrap(), limit, case["w"].as_f64().unwrap(), &ev)
            .unwrap();
        expect_judged(&j, &case["expect"], &label(case));
    }
}

#[test]
fn width_fixture() {
    let rules = Rules::latest();
    let doc = fixture("width");
    for case in doc["cases"].as_array().unwrap() {
        let limit: Limit = serde_json::from_value(case["limit"].clone()).unwrap();
        let src: Source = serde_json::from_value(case["source"].clone()).unwrap();
        let got = rules.acceptance_width(limit, case["w"].as_f64().unwrap(), &src).unwrap();
        let got = serde_json::to_value(got).unwrap();
        let e = &case["expect"];
        assert_eq!(got["result"], e["result"], "{}", label(case));
        if let Some(b) = e.get("basis") {
            assert_eq!(&got["basis"], b, "{}: basis", label(case));
        }
    }
}

#[test]
fn combine_fixture() {
    let rules = Rules::latest();
    let doc = fixture("combine");
    for case in doc["cases"].as_array().unwrap() {
        let items: Vec<Judged> = serde_json::from_value(case["items"].clone()).unwrap();
        let s = rules.combine(&items).unwrap();
        let e = &case["expect"];
        let l = label(case);
        assert_eq!(serde_json::to_value(s.verdict).unwrap(), e["verdict"], "{l}: verdict");
        assert_eq!(serde_json::to_value(s.counts).unwrap(), e["counts"], "{l}: counts");
        assert_eq!(serde_json::to_value(&s.qualifiers).unwrap(), e["qualifiers"], "{l}: qualifiers");
    }
}

#[test]
fn requirement_fixture() {
    let rules = Rules::latest();
    let doc = fixture("requirement");
    for case in doc["cases"].as_array().unwrap() {
        let tests: Vec<Judged> = serde_json::from_value(case["tests"].clone()).unwrap();
        let got = serde_json::to_value(rules.requirement(&tests).unwrap()).unwrap();
        let e = &case["expect"];
        let l = label(case);
        for k in ["verdict", "counts", "qualifiers", "n_of_m", "skipped", "relayed_listed_apart"] {
            assert_eq!(got[k], e[k], "{l}: {k}");
        }
    }
}

#[test]
fn the_six_strings_and_severities_are_the_shapes() {
    let strings: Vec<&str> = Verdict::ALL.iter().map(|v| v.as_str()).collect();
    assert_eq!(strings, ["pass", "fail", "error", "inconclusive", "skipped", "cancelled"]);
    for v in Verdict::ALL {
        assert_eq!(serde_json::to_value(v).unwrap(), json!(v.as_str()));
    }
    let sev: Vec<u8> = Verdict::ALL.iter().map(|v| v.severity()).collect();
    assert_eq!(sev, [0, 4, 5, 3, 1, 2]);
}

#[test]
fn the_gap_split_is_five_and_two() {
    let gaps = ["hook_unmapped", "bench_missing", "requires_not_met", "stopped_by_error", "stopped_after_fail"];
    let nogaps = ["not_applicable", "not_selected"];
    for g in gaps {
        let r: Reason = serde_json::from_value(json!(g)).unwrap();
        assert!(r.is_gap() && r.verdict() == Verdict::Skipped, "{g}");
    }
    for g in nogaps {
        let r: Reason = serde_json::from_value(json!(g)).unwrap();
        assert!(!r.is_gap() && r.verdict() == Verdict::Skipped, "{g}");
    }
}

#[test]
fn a_reason_outside_the_closed_list_is_refused() {
    assert!(serde_json::from_value::<Reason>(json!("prompt_timed_out")).is_err());
    assert!(serde_json::from_value::<Verdict>(json!("Pass")).is_err());
    assert!(serde_json::from_value::<Qualifier>(json!("relay")).is_err());
}

#[test]
fn every_rule_ships_at_behaviour_one_and_latest_pins_all() {
    let latest = Rules::latest();
    for n in RULE_NAMES {
        assert_eq!(Rules::shipped(n), &[1], "{n}");
        assert!(latest.behaviours().contains(&(*n, 1)), "{n}");
    }
}

#[test]
fn a_behaviour_the_crate_never_shipped_cannot_be_pinned() {
    assert_eq!(
        Rules::pinned(&[("combine", 2)]),
        Err(RuleError::UnknownBehaviour { name: "combine", behaviour: 2 })
    );
    assert!(matches!(Rules::pinned(&[("nope", 1)]), Err(RuleError::UnknownRule(_))));
    let some = Rules::pinned(&[("combine", 1)]).unwrap();
    assert!(some.combine(&[]).is_ok());
    assert_eq!(some.requirement(&[]), Err(RuleError::NotPinned("requirement")));
}

#[test]
fn a_rule_version_is_a_separate_function_so_behaviour_one_never_changes() {
    // The contract is the fixtures above, which carry no behaviour number a
    // change could edit: a drift in behaviour 1 fails them. This pins the
    // names `measurements.json`'s `behaviours` will carry.
    assert_eq!(RULE_NAMES, ["step_verdict", "limit_verdict", "acceptance_width", "combine", "requirement"]);
}
