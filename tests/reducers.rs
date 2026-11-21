//! Each built-in reducer against its own fixture (`tests/fixtures/reducers/`),
//! with no Core installed, plus the contract tests for behaviour versioning.
#![cfg(feature = "reducers")]

use embarch_study_designer::reducers::*;
use serde_json::Value;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * (1.0 + b.abs())
}

fn run_fixture(name: &str) {
    let path = format!("{}/tests/fixtures/reducers/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let reg = Registry::builtin();
    for case in doc["cases"].as_array().unwrap() {
        let label = format!("{name}: {}", case["name"]);
        let input: Input = serde_json::from_value(case["input"].clone()).unwrap();
        let m = reg.reduce(name, 1, &input).unwrap();
        let e = &case["expect"];
        match e["value"].as_f64() {
            Some(v) => assert!(close(m.value.unwrap(), v), "{label}: value {:?} != {v}", m.value),
            None => assert!(m.value.is_none(), "{label}: value should be null"),
        }
        let inc: Vec<Incomplete> = serde_json::from_value(e["incomplete"].clone()).unwrap();
        assert_eq!(m.incomplete, inc, "{label}: incomplete");
        let us: USource = serde_json::from_value(e["u_source"].clone()).unwrap();
        assert_eq!(m.u_source, us, "{label}: u_source");
        match (e.get("U").and_then(Value::as_f64), &m.uncertainty) {
            (Some(u), Some(got)) => {
                assert!(close(got.u_expanded, u), "{label}: U {} != {u}", got.u_expanded);
                assert_eq!(got.basis, Basis::Derived);
            }
            (None, None) => {}
            (w, g) => panic!("{label}: uncertainty {g:?} vs expected U {w:?}"),
        }
        // The type-A term is checked wherever the fixture states one, and
        // must be absent wherever it does not.
        let got_a = m.uncertainty.as_ref().and_then(|u| u.type_a);
        match e.get("type_a").and_then(Value::as_f64) {
            Some(a) => assert!(close(got_a.unwrap(), a), "{label}: type_a {got_a:?} != {a}"),
            None => assert!(got_a.is_none(), "{label}: type_a should be absent"),
        }
        assert_eq!(m.method.name, name);
        assert_eq!(m.method.kind, "reducer");
        assert_eq!(m.method.behaviour, 1);
        assert!(UNITS.contains(&m.unit.as_str()), "{label}: unit {}", m.unit);
    }
}

#[test]
fn count_fixture() {
    run_fixture("count");
}
#[test]
fn rate_fixture() {
    run_fixture("rate");
}
#[test]
fn min_fixture() {
    run_fixture("min");
}
#[test]
fn max_fixture() {
    run_fixture("max");
}
#[test]
fn mean_fixture() {
    run_fixture("mean");
}
#[test]
fn percentile_fixture() {
    run_fixture("percentile");
}
#[test]
fn interval_fixture() {
    run_fixture("interval");
}
#[test]
fn value_at_marker_fixture() {
    run_fixture("value_at_marker");
}

#[test]
fn every_name_has_a_fixture_and_behaviour_one() {
    let reg = Registry::builtin();
    for n in NAMES {
        assert_eq!(reg.behaviours(n), vec![1], "{n}");
        let path = format!("{}/tests/fixtures/reducers/{n}.json", env!("CARGO_MANIFEST_DIR"));
        assert!(std::path::Path::new(&path).exists(), "{n} has no fixture");
    }
}

#[test]
fn sqrt_is_exact_on_squares_and_close_elsewhere() {
    assert_eq!(sqrt(0.0), 0.0);
    assert_eq!(sqrt(4.0), 2.0);
    assert_eq!(sqrt(1e-150 * 1e-150), 1e-150);
    for x in [2.0f64, 3.0, 0.1, 12345.678, 1e-320, 1e300] {
        assert!((sqrt(x) - x.sqrt()).abs() <= 2.0 * f64::EPSILON * x.sqrt(), "{x}");
    }
    assert!(sqrt(-1.0).is_nan());
}

fn mean_input() -> Input {
    serde_json::from_str(
        r#"{"unit":"V","u":0.1,"points":[{"t_ms":0,"value":1.0},{"t_ms":1,"value":2.0},{"t_ms":2,"value":6.0}]}"#,
    )
    .unwrap()
}

/// A stand-in for a future change: the population standard deviation instead
/// of the sample one in `mean`'s type-A term.
fn mean_v2(i: &Input) -> Measurement {
    let mut m = Registry::builtin().reduce("mean", 1, i).unwrap();
    let n = i.points.len() as f64;
    let mean = m.value.unwrap();
    let ss: f64 = i.points.iter().map(|p| (p.value - mean) * (p.value - mean)).sum();
    let a = sqrt(ss / n) / sqrt(n);
    let u = i.u.unwrap();
    m.uncertainty = Some(Uncertainty {
        u_expanded: i.k * sqrt(u * u + a * a),
        k: i.k,
        basis: Basis::Derived,
        type_a: Some(a),
    });
    m.method.behaviour = 2;
    m
}

#[test]
fn a_behaviour_change_bumps_the_version_and_the_old_one_stays_callable() {
    let mut reg = Registry::builtin();
    let before = reg.reduce("mean", 1, &mean_input()).unwrap();
    assert_eq!(reg.latest("mean"), Some(1));
    reg.register("mean", 2, mean_v2);
    assert_eq!(reg.latest("mean"), Some(2));
    assert_eq!(reg.behaviours("mean"), vec![1, 2]);
    // Behaviour 1 is byte-for-byte what it was; 2 differs, and says so.
    assert_eq!(reg.reduce("mean", 1, &mean_input()).unwrap(), before);
    let after = reg.reduce_latest("mean", &mean_input()).unwrap();
    assert_eq!(after.method.behaviour, 2);
    assert_ne!(after.uncertainty, before.uncertainty);
    assert_eq!(after.value, before.value);
}

#[test]
fn an_unshipped_behaviour_or_name_is_refused_not_guessed() {
    let reg = Registry::builtin();
    assert_eq!(
        reg.reduce("mean", 7, &mean_input()),
        Err(ReduceError::UnknownBehaviour { name: "mean".into(), behaviour: 7 })
    );
    assert_eq!(reg.reduce("median", 1, &mean_input()), Err(ReduceError::UnknownReducer("median".into())));
}

#[test]
fn a_measurement_serialises_as_hil_shapes_fixes_it() {
    let m = Registry::builtin().reduce("mean", 1, &mean_input()).unwrap();
    let v = serde_json::to_value(&m).unwrap();
    assert_eq!(v["method"]["kind"], "reducer");
    assert_eq!(v["method"]["behaviour"], 1);
    assert_eq!(v["method"]["window"], serde_json::json!({}));
    assert_eq!(v["u_source"], "derived");
    assert!(v["uncertainty"]["U"].is_number() && v["uncertainty"]["type_a"].is_number());
    assert_eq!(v["uncertainty"]["basis"], "derived");
    assert_eq!(v["incomplete"], serde_json::json!([]));
}

#[test]
fn a_non_finite_sample_is_a_parse_failure_not_a_number() {
    let mut i = mean_input();
    i.points[1].value = f64::NAN;
    for n in ["min", "max", "mean"] {
        let m = Registry::builtin().reduce(n, 1, &i).unwrap();
        assert_eq!(m.value, None, "{n}");
        assert_eq!(m.incomplete, vec![Incomplete::ParseFailed], "{n}");
    }
}
