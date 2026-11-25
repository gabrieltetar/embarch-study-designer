//! The HIL test, suite and plan types against their fixtures
//! (`tests/fixtures/hil-tests/`): round-trip, refused unknown keys, the
//! definition hash and the derived diff.
#![cfg(feature = "hil-test")]

use embarch_study_designer::hil_test::*;
use embarch_study_designer::verdicts::Limit;
use serde_json::{json, Value};

fn raw(name: &str) -> String {
    let path = format!("{}/tests/fixtures/hil-tests/{name}.json", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path).unwrap()
}

fn value(name: &str) -> Value {
    serde_json::from_str(&raw(name)).unwrap()
}

fn test() -> TestDef {
    serde_json::from_str(&raw("test")).unwrap()
}

#[test]
fn fixtures_round_trip() {
    let t = test();
    assert_eq!(serde_json::to_value(&t).unwrap(), value("test"));
    t.check().unwrap();
    let s: SuiteManifest = serde_json::from_str(&raw("suite")).unwrap();
    assert_eq!(serde_json::to_value(&s).unwrap(), value("suite"));
    s.check().unwrap();
    let p: Plan = serde_json::from_str(&raw("plan")).unwrap();
    assert_eq!(serde_json::to_value(&p).unwrap(), value("plan"));
    p.check().unwrap();
}

#[test]
fn unknown_keys_are_refused_at_every_level() {
    fn refused<T: serde::de::DeserializeOwned>(mut v: Value, path: &[&str]) {
        let mut at = &mut v;
        for k in path {
            at = match k.parse::<usize>() {
                Ok(i) => &mut at[i],
                Err(_) => &mut at[*k],
            };
        }
        at.as_object_mut().unwrap().insert("surprise".into(), json!(1));
        assert!(serde_json::from_value::<T>(v).is_err(), "{path:?}");
    }
    for p in [&[][..], &["study"], &["bench"], &["revision_range"], &["limits", "0"], &["limits", "0", "limit"], &["decision_rule"], &["setup_terms", "0"], &["sources", "0"]] {
        refused::<TestDef>(value("test"), p);
    }
    for p in [&[][..], &["tests", "0"], &["benches", "0"], &["benches", "0", "boards", "0"]] {
        refused::<SuiteManifest>(value("suite"), p);
    }
    for p in [&[][..], &["suite"], &["select"], &["dut"], &["dut", "capabilities", "0"], &["dut", "hooks", "0"], &["dut", "hooks", "0", "target"]] {
        refused::<Plan>(value("plan"), p);
    }
}

#[test]
fn defaults_are_the_written_out_spelling() {
    let mut v = value("test");
    for k in ["requires", "run_count", "setup_terms", "sources", "tags", "schema"] {
        v.as_object_mut().unwrap().remove(k);
    }
    let t: TestDef = serde_json::from_value(v).unwrap();
    assert_eq!((t.run_count, t.schema, t.requires.len()), (1, SCHEMA, 0));
    // Precondition is never defaulted: a file writes it out.
    let mut v = value("test");
    v.as_object_mut().unwrap().remove("precondition");
    assert!(serde_json::from_value::<TestDef>(v).is_err());
}

#[test]
fn header_checks() {
    let mut t = test();
    t.run_count = 0;
    assert_eq!(t.check(), Err(ShapeError::RunCount));
    t.run_count = 1;
    t.schema = 2;
    assert_eq!(t.check(), Err(ShapeError::Schema(2)));
    let mut s: SuiteManifest = serde_json::from_str(&raw("suite")).unwrap();
    s.benches[0].description = json!("nope");
    assert_eq!(s.check(), Err(ShapeError::Description("pd-sink-bench".into())));
}

#[test]
fn hash_ignores_key_order_and_follows_a_limit() {
    let t = test();
    let h = t.hash();
    assert_eq!(h.len(), 64);
    assert!(h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));

    // The same file with every object's keys reversed.
    fn reversed(v: &Value) -> Value {
        match v {
            Value::Object(m) => Value::Object(m.iter().rev().map(|(k, x)| (k.clone(), reversed(x))).collect()),
            Value::Array(a) => Value::Array(a.iter().map(reversed).collect()),
            x => x.clone(),
        }
    }
    let text = serde_json::to_string(&reversed(&value("test"))).unwrap();
    let again: TestDef = serde_json::from_str(&text).unwrap();
    assert_eq!(again.hash(), h);
    // Defaulted fields spelled or omitted give one hash.
    let mut v = value("test");
    v.as_object_mut().unwrap().remove("tags");
    v.as_object_mut().unwrap().insert("tags".into(), json!(["vbus", "source"]));
    assert_eq!(serde_json::from_value::<TestDef>(v).unwrap().hash(), h);

    let mut wider = test();
    wider.limits[0].limit = LimitSpec::Band { lo: 4.5, hi: 5.5 };
    assert_ne!(wider.hash(), h);
}

#[test]
fn canonical_json_follows_rfc_8785() {
    let v: Value = serde_json::from_str(
        r#"{"\u20ac":1,"a":[1.0,0.5,1e21,1e-7,-0.0,123456789012345670000,100,4.75],"\r":"\u0001\"\\","b":{"z":null,"y":true}}"#,
    )
    .unwrap();
    assert_eq!(
        canonical_json(&v),
        "{\"\\r\":\"\\u0001\\\"\\\\\",\"a\":[1,0.5,1e+21,1e-7,0,123456789012345670000,100,4.75],\"b\":{\"y\":true,\"z\":null},\"\u{20ac}\":1}"
    );
    // UTF-16 order: U+FFFF sorts after U+10000's surrogate pair.
    let v: Value = serde_json::from_str("{\"\u{ffff}\":1,\"\u{10000}\":2}").unwrap();
    assert_eq!(canonical_json(&v), "{\"\u{10000}\":2,\"\u{ffff}\":1}");
    // The RFC's sample number set.
    for (n, s) in [(1e30, "1e+30"), (0.000001, "0.000001"), (0.0000001, "1e-7"), (1.5e300, "1.5e+300")] {
        assert_eq!(canonical_json(&json!(n)), s);
    }
}

#[test]
fn a_widened_limit_is_the_one_field_reported() {
    let premade = test();
    let mut copy = premade.clone();
    copy.test_id = "my-vbus".into();
    copy.title = "mine".into();
    copy.tags.clear();
    copy.derived_from = Some("vbus-5v-source-voltage@0.1.0".into());
    assert!(derived_diff(&copy, &premade).is_empty(), "identity fields are not verdict-deciding");
    copy.limits[0].limit = LimitSpec::Band { lo: 4.5, hi: 5.5 };
    let d = derived_diff(&copy, &premade);
    assert_eq!(
        d,
        vec![FieldDiff {
            field: "limits.vbus_mean".into(),
            old: json!({"measurement":"vbus_mean","limit":{"kind":"band","lo":4.75,"hi":5.25}}),
            new: json!({"measurement":"vbus_mean","limit":{"kind":"band","lo":4.5,"hi":5.5}}),
        }]
    );
    copy.run_count = 1;
    copy.limits.pop();
    let fields: Vec<String> = derived_diff(&copy, &premade).into_iter().map(|f| f.field).collect();
    assert_eq!(fields, ["run_count", "limits.led_state", "limits.vbus_mean"]);
}

#[test]
fn slugs_selection_and_conversions() {
    assert!(is_requirement_slug("vbus-default-5v"));
    assert!(!is_requirement_slug("5.1.2"));
    assert!(!is_requirement_slug("Vbus"));
    assert!(!is_requirement_slug(""));
    let t = test();
    assert!(Selection::All.selects("x", &[]));
    let sel = Selection::Some { tests: vec!["a".into()], tags: vec!["vbus".into()] };
    assert!(sel.selects("a", &[]));
    assert!(sel.selects("b", &t.tags));
    assert!(!sel.selects("b", &[]));
    assert_eq!(t.limits[0].limit.numeric(), Some(Limit::Band { lo: 4.75, hi: 5.25 }));
    assert_eq!(t.limits[2].limit.numeric(), None);
    assert_eq!(t.sources[0].as_source().u, 0.05);
    assert_eq!(t.decision_rule.rule(), embarch_study_designer::verdicts::DecisionRule::GuardBand { w_multiple: 1.0 });
}
