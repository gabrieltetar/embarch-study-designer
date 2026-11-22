//! A study's derived measurements, declared (decisions 99 and 100,
//! `interfaces/hil-declarations.md`): which built-in reducer Core runs over
//! which of the study's own captures when the study ends, with what window,
//! and the values a study declares for itself about that input.
//!
//! **Host-only, like `Study.record_checks`**: never transmitted to dev-bench
//! and sealed by neither `steps_crc` nor `streams_crc`. What Core computes
//! after a run changes neither what dev-bench executes nor what it captures,
//! so a declaration names its tap by [`StreamTap::id`] and never rides inside
//! `StudyStart`, and [`crate::DEV_BENCH_WIRE_SCHEMA_VERSION`] does not move.
//!
//! **Always compiled**, unlike [`crate::reducers`], which needs `alloc`: a
//! `Study` exists in every build, so its fields must too. The reducer names and
//! unit strings are therefore repeated here, and a test under the `reducers`
//! feature holds the two lists equal.
//!
//! **What [`validate_measurements`] refuses** is a declaration Core could not
//! compute, or would compute over the wrong numbers: an unknown reducer, a tap
//! the study lacks, a repeated or empty name, a window key the reducer does not
//! take or one it needs and lacks, a field or unit that does not fit the tap,
//! and a declared value that is not a finite positive number. **A marker name
//! is not checked**: what stamps a marker is Core's to say (`tasks/core/132`),
//! and a name no capture produces gives `value: null` with `parse_failed`, as
//! the reducer contract already does (decision 92).

use heapless::String;
use serde::{Deserialize, Serialize};

use crate::limits::{
    MAX_MARKER_NAME_LEN, MAX_MEASUREMENT_NAME_LEN, MAX_REDUCER_NAME_LEN,
    MAX_STRUCT_FIELD_NAME_LEN, MAX_UNIT_LEN,
};
use crate::sample::Unit;
use crate::streams::StreamEncoding;
use crate::study::Study;

/// The built-in reducers a declaration may name, as `reducers::NAMES` spells
/// them.
pub const REDUCER_NAMES: &[&str] =
    &["count", "rate", "min", "max", "mean", "percentile", "interval", "value_at_marker"];

/// The units a declaration may name, as `reducers::UNITS` spells them.
pub const UNITS: &[&str] = &["V", "mV", "A", "mA", "W", "mW", "s", "ms", "us", "Hz", "B", "1"];

/// The reducer's parameters a study may declare. Its time bounds are not among
/// them: a study is written before its run, so Core sets `from_utc_ms` and
/// `to_utc_ms` from the capture itself (`tasks/core/132`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredWindow {
    /// Percentile, 0 to 100, for `percentile`.
    #[serde(default)]
    pub percentile: Option<f64>,
    /// `interval`'s start marker, or `value_at_marker`'s only one.
    #[serde(default)]
    pub from_marker: Option<String<MAX_MARKER_NAME_LEN>>,
    /// `interval`'s end marker.
    #[serde(default)]
    pub to_marker: Option<String<MAX_MARKER_NAME_LEN>>,
}

/// Values the study declares for itself about the input, each one labelled
/// "declared by the study, unconfirmed" (`u_source: study`) when Core writes
/// the measurement. Whether Core honours one for a given source is Core's
/// (a study cannot mark a DUT's word as the bench's, `notes/hil/02-study.md`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudyDeclared {
    /// Resolution of one sample, in the declaration's unit.
    #[serde(default)]
    pub resolution: Option<f64>,
    /// Expanded uncertainty `U` of one sample, in the declaration's unit.
    #[serde(rename = "U", default)]
    pub u_expanded: Option<f64>,
    /// Coverage factor of `U`; absent is 2, the reducers' default.
    #[serde(default)]
    pub k: Option<f64>,
    /// The sample rate the study declares for the tap, Hz, which Core checks
    /// against its own count (`rate_check`).
    #[serde(default)]
    pub sample_rate_hz: Option<f64>,
}

/// One derived measurement a study declares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementDecl {
    /// The measurement's name, unique in the study: what prose cites.
    pub name: String<MAX_MEASUREMENT_NAME_LEN>,
    /// One of [`REDUCER_NAMES`]; Core runs its latest behaviour.
    pub reducer: String<MAX_REDUCER_NAME_LEN>,
    /// The tap whose capture is the input, by [`crate::streams::StreamTap::id`].
    pub tap: u8,
    /// The struct field whose value is each point, where the tap is decoded
    /// by a struct layout and the reducer reads values.
    #[serde(default)]
    pub field: Option<String<MAX_STRUCT_FIELD_NAME_LEN>>,
    /// The input's unit, one of [`UNITS`], where the reducer reads values and
    /// the tap does not carry one.
    #[serde(default)]
    pub unit: Option<String<MAX_UNIT_LEN>>,
    #[serde(default)]
    pub window: DeclaredWindow,
    #[serde(default)]
    pub declared: Option<StudyDeclared>,
}

/// What one reducer takes. `values` is whether it reads the points' values
/// (and so needs a unit), not only their count or the markers.
struct Takes {
    percentile: bool,
    from_marker: bool,
    to_marker: bool,
    values: bool,
}

fn takes(reducer: &str) -> Option<Takes> {
    let none = Takes { percentile: false, from_marker: false, to_marker: false, values: false };
    Some(match reducer {
        "count" | "rate" => none,
        "min" | "max" | "mean" => Takes { values: true, ..none },
        "percentile" => Takes { percentile: true, values: true, ..none },
        "interval" => Takes { from_marker: true, to_marker: true, ..none },
        "value_at_marker" => Takes { from_marker: true, values: true, ..none },
        _ => return None,
    })
}

/// The unit a `Samples` tap carries, `None` for raw counts.
fn sample_unit(unit: Unit) -> Option<&'static str> {
    match unit {
        Unit::Milliamps => Some("mA"),
        Unit::Volts => Some("V"),
        Unit::Milliwatts => Some("mW"),
        Unit::Raw => None,
    }
}

/// Why a study's `measurements` are unusable (refused at validation).
/// `index` is the declaration's position in `Study.measurements`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeasurementDeclError {
    EmptyName { index: usize },
    RepeatedName { index: usize, first: usize },
    UnknownReducer { index: usize },
    UnknownTap { index: usize, tap: u8 },
    /// A window key the reducer does not take.
    WindowNotTaken { index: usize, key: &'static str },
    /// A window key the reducer needs, absent or empty.
    WindowMissing { index: usize, key: &'static str },
    PercentileOutOfRange { index: usize },
    /// A struct field named where the tap is not a struct or the reducer reads
    /// no values.
    FieldNotTaken { index: usize },
    /// A value reducer over a struct tap, with no field named.
    FieldMissing { index: usize },
    /// A field the tap's struct layout does not have.
    UnknownField { index: usize },
    /// A value reducer over a tap whose records are not numbers.
    NoValues { index: usize },
    UnitNotTaken { index: usize },
    UnitMissing { index: usize },
    UnknownUnit { index: usize },
    /// A unit other than the one the `Samples` tap carries.
    UnitDisagrees { index: usize, tap_unit: &'static str },
    /// A declared value the reducer does not read (`resolution`, `U` or `k`
    /// for a reducer that reads no values, `k` without `U`).
    DeclaredNotTaken { index: usize, key: &'static str },
    /// A declared value that is not a finite number above zero.
    DeclaredNotPositive { index: usize, key: &'static str },
}

impl core::fmt::Display for MeasurementDeclError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use MeasurementDeclError::*;
        match self {
            EmptyName { index } => write!(f, "measurements[{index}] has an empty name"),
            RepeatedName { index, first } => {
                write!(f, "measurements[{index}] repeats the name of measurements[{first}]")
            }
            UnknownReducer { index } => {
                write!(f, "measurements[{index}] names a reducer that is not built in")
            }
            UnknownTap { index, tap } => {
                write!(f, "measurements[{index}] reads tap {tap}, which the study does not declare")
            }
            WindowNotTaken { index, key } => {
                write!(f, "measurements[{index}] sets window.{key}, which its reducer does not take")
            }
            WindowMissing { index, key } => {
                write!(f, "measurements[{index}] lacks window.{key}, which its reducer needs")
            }
            PercentileOutOfRange { index } => {
                write!(f, "measurements[{index}] declares a percentile outside 0 to 100")
            }
            FieldNotTaken { index } => write!(
                f,
                "measurements[{index}] names a field, but its tap is not a struct or its reducer \
                 reads no values"
            ),
            FieldMissing { index } => write!(
                f,
                "measurements[{index}] reads values from a struct tap without naming the field"
            ),
            UnknownField { index } => {
                write!(f, "measurements[{index}] names a field its tap's struct layout lacks")
            }
            NoValues { index } => write!(
                f,
                "measurements[{index}]'s reducer reads values, and its tap's records are not \
                 numbers"
            ),
            UnitNotTaken { index } => {
                write!(f, "measurements[{index}] names a unit its reducer does not read")
            }
            UnitMissing { index } => write!(
                f,
                "measurements[{index}] reads values from a tap that carries no unit, and names none"
            ),
            UnknownUnit { index } => write!(f, "measurements[{index}] names an unknown unit"),
            UnitDisagrees { index, tap_unit } => write!(
                f,
                "measurements[{index}] names a unit other than its tap's own, {tap_unit}"
            ),
            DeclaredNotTaken { index, key } => {
                write!(f, "measurements[{index}] declares {key}, which its reducer does not read")
            }
            DeclaredNotPositive { index, key } => write!(
                f,
                "measurements[{index}] declares {key} as something other than a finite number \
                 above zero"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for MeasurementDeclError {}

fn positive(value: Option<f64>, index: usize, key: &'static str) -> Result<(), MeasurementDeclError> {
    match value {
        Some(v) if !(v.is_finite() && v > 0.0) => {
            Err(MeasurementDeclError::DeclaredNotPositive { index, key })
        }
        _ => Ok(()),
    }
}

/// Pre-flight validation for `Study.measurements`, beside
/// [`validate_record_checks`](crate::records::validate_record_checks). Run
/// after [`validate_taps`](crate::streams::validate_taps): a tap's struct
/// layout is looked up by the index that one checks.
pub fn validate_measurements(study: &Study) -> Result<(), MeasurementDeclError> {
    use MeasurementDeclError::*;
    for (index, decl) in study.measurements.iter().enumerate() {
        if decl.name.is_empty() {
            return Err(EmptyName { index });
        }
        if let Some(first) = study.measurements[..index].iter().position(|d| d.name == decl.name) {
            return Err(RepeatedName { index, first });
        }
        let t = takes(decl.reducer.as_str()).ok_or(UnknownReducer { index })?;
        let tap = study
            .streams
            .iter()
            .find(|s| s.id == decl.tap)
            .ok_or(UnknownTap { index, tap: decl.tap })?;

        let w = &decl.window;
        let given = |s: &Option<String<MAX_MARKER_NAME_LEN>>| s.as_ref().is_some_and(|m| !m.is_empty());
        for (key, needs, has_key, has_value) in [
            ("percentile", t.percentile, w.percentile.is_some(), w.percentile.is_some()),
            ("from_marker", t.from_marker, w.from_marker.is_some(), given(&w.from_marker)),
            ("to_marker", t.to_marker, w.to_marker.is_some(), given(&w.to_marker)),
        ] {
            if has_key && !needs {
                return Err(WindowNotTaken { index, key });
            }
            if needs && !has_value {
                return Err(WindowMissing { index, key });
            }
        }
        if let Some(p) = w.percentile {
            if !(p.is_finite() && (0.0..=100.0).contains(&p)) {
                return Err(PercentileOutOfRange { index });
            }
        }

        let tap_unit = match tap.encoding {
            StreamEncoding::Samples { unit, .. } => {
                if decl.field.is_some() {
                    return Err(FieldNotTaken { index });
                }
                sample_unit(unit)
            }
            StreamEncoding::Struct { decoder } if t.values => {
                let field = decl.field.as_ref().ok_or(FieldMissing { index })?;
                let layout = study.decoders.get(usize::from(decoder)).ok_or(UnknownField { index })?;
                if !layout.header.iter().chain(layout.repeat.iter()).any(|f| f.name == *field) {
                    return Err(UnknownField { index });
                }
                None
            }
            _ if decl.field.is_some() => return Err(FieldNotTaken { index }),
            StreamEncoding::Struct { .. } => None,
            _ if t.values => return Err(NoValues { index }),
            _ => None,
        };

        match (&decl.unit, t.values) {
            (Some(_), false) => return Err(UnitNotTaken { index }),
            (None, true) if tap_unit.is_none() => return Err(UnitMissing { index }),
            (Some(unit), true) => {
                if !UNITS.contains(&unit.as_str()) {
                    return Err(UnknownUnit { index });
                }
                if let Some(tap_unit) = tap_unit {
                    if unit.as_str() != tap_unit {
                        return Err(UnitDisagrees { index, tap_unit });
                    }
                }
            }
            _ => {}
        }

        if let Some(d) = &decl.declared {
            for (key, value) in
                [("resolution", d.resolution), ("U", d.u_expanded), ("k", d.k)]
            {
                if value.is_some() && !t.values {
                    return Err(DeclaredNotTaken { index, key });
                }
            }
            if d.k.is_some() && d.u_expanded.is_none() {
                return Err(DeclaredNotTaken { index, key: "k" });
            }
            positive(d.resolution, index, "resolution")?;
            positive(d.u_expanded, index, "U")?;
            positive(d.k, index, "k")?;
            positive(d.sample_rate_hz, index, "sample_rate_hz")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bounded::Bounded;
    use crate::decoder::{ScalarType, StructField, StructLayout};
    use crate::limits::MAX_STREAMS_PER_STUDY;
    use crate::sample::Unit;
    use crate::streams::{SampleLayout, StreamScope, StreamSource, StreamTap};
    use crate::study::{DevBenchLogLevel, Requirements};
    use heapless::Vec;
    use MeasurementDeclError::*;

    fn s<const N: usize>(text: &str) -> String<N> {
        String::try_from(text).unwrap()
    }

    /// Tap 0 samples in mA, tap 1 a struct with field `temp`, tap 2 text.
    fn study(decls: &[MeasurementDecl]) -> Study {
        let mut streams: Vec<StreamTap, MAX_STREAMS_PER_STUDY> = Vec::new();
        let tap = |id: u8, encoding: StreamEncoding| StreamTap {
            id,
            name: s("tap"),
            source: StreamSource::DevBenchLog,
            encoding,
            scope: StreamScope::WholeStudy,
        };
        streams
            .push(tap(0, StreamEncoding::Samples { layout: SampleLayout::F32Le, unit: Unit::Milliamps, channel_id: 0 }))
            .unwrap();
        streams.push(tap(1, StreamEncoding::Struct { decoder: 0 })).unwrap();
        streams.push(tap(2, StreamEncoding::Text)).unwrap();
        let mut header = Vec::new();
        header.push(StructField { name: s("temp"), ty: ScalarType::U8 }).unwrap();
        let mut decoders = Bounded::new();
        decoders
            .push(StructLayout { name: s("reading"), header, repeat: Vec::new(), chart_field: None })
            .unwrap();
        let mut measurements = Bounded::new();
        for d in decls {
            measurements.push(d.clone()).unwrap();
        }
        Study {
            name: s("measures"),
            requires: Requirements::any(),
            steps: Bounded::new(),
            streams,
            steps_crc: 0,
            streams_crc: 0,
            protocols: Default::default(),
            protocols_crc: 0,
            dev_bench_log_level: DevBenchLogLevel::default(),
            decoders,
            record_checks: Default::default(),
            measurements,
        }
    }

    fn decl(name: &str, reducer: &str, tap: u8) -> MeasurementDecl {
        MeasurementDecl {
            name: s(name),
            reducer: s(reducer),
            tap,
            field: None,
            unit: None,
            window: DeclaredWindow::default(),
            declared: None,
        }
    }

    fn check(d: MeasurementDecl) -> Result<(), MeasurementDeclError> {
        validate_measurements(&study(&[d]))
    }

    #[test]
    fn every_reducer_validates_over_a_tap_that_fits_it() {
        let mut p = decl("p99", "percentile", 0);
        p.window.percentile = Some(99.0);
        let mut i = decl("boot", "interval", 2);
        i.window.from_marker = Some(s("reset"));
        i.window.to_marker = Some(s("ready"));
        let mut v = decl("at_ready", "value_at_marker", 1);
        v.window.from_marker = Some(s("ready"));
        v.field = Some(s("temp"));
        v.unit = Some(s("1"));
        v.declared = Some(StudyDeclared {
            resolution: Some(1.0),
            u_expanded: Some(2.0),
            k: Some(2.0),
            sample_rate_hz: Some(10.0),
        });
        let mut m = decl("mean", "mean", 0);
        m.unit = Some(s("mA"));
        let all = [decl("lines", "count", 2), decl("rate", "rate", 1), m, p, i, v];
        assert_eq!(validate_measurements(&study(&all)), Ok(()));
    }

    #[test]
    fn a_study_with_measurements_round_trips_and_one_without_the_key_loads() {
        let mut p = decl("p99", "percentile", 0);
        p.window.percentile = Some(99.5);
        p.declared = Some(StudyDeclared { u_expanded: Some(0.2), ..Default::default() });
        let with = study(&[p]);
        let json = serde_json::to_string(&with).unwrap();
        assert!(json.contains(r#""reducer":"percentile","tap":0"#), "{json}");
        assert!(json.contains(r#""U":0.2"#), "{json}");
        let back: Study = serde_json::from_str(&json).unwrap();
        assert_eq!(back, with);

        let mut buf = [0u8; 4096];
        let encoded = postcard::to_slice(&with, &mut buf).unwrap();
        let back: Study = postcard::from_bytes(encoded).unwrap();
        assert_eq!(back, with);

        let mut old = serde_json::to_value(study(&[])).unwrap();
        old.as_object_mut().unwrap().remove("measurements");
        let loaded: Study = serde_json::from_value(old).unwrap();
        assert!(loaded.measurements.is_empty());
    }

    #[test]
    fn a_time_bound_in_the_window_is_refused_at_parse() {
        let raw = r#"{"name":"m","reducer":"rate","tap":0,"window":{"from_utc_ms":1}}"#;
        assert!(serde_json::from_str::<MeasurementDecl>(raw).is_err());
    }

    #[test]
    fn an_empty_or_repeated_name_is_refused() {
        assert_eq!(check(decl("", "count", 2)), Err(EmptyName { index: 0 }));
        let twice = [decl("n", "count", 2), decl("n", "rate", 2)];
        assert_eq!(validate_measurements(&study(&twice)), Err(RepeatedName { index: 1, first: 0 }));
    }

    #[test]
    fn an_unknown_reducer_or_tap_is_refused() {
        assert_eq!(check(decl("n", "median", 0)), Err(UnknownReducer { index: 0 }));
        assert_eq!(check(decl("n", "count", 7)), Err(UnknownTap { index: 0, tap: 7 }));
    }

    #[test]
    fn a_window_key_the_reducer_does_not_take_or_lacks_is_refused() {
        let mut d = decl("n", "mean", 0);
        d.window.percentile = Some(50.0);
        assert_eq!(check(d), Err(WindowNotTaken { index: 0, key: "percentile" }));
        assert_eq!(check(decl("n", "percentile", 0)), Err(WindowMissing { index: 0, key: "percentile" }));
        let mut d = decl("n", "interval", 2);
        d.window.from_marker = Some(s("a"));
        d.window.to_marker = Some(s(""));
        assert_eq!(check(d), Err(WindowMissing { index: 0, key: "to_marker" }));
        let mut d = decl("n", "value_at_marker", 0);
        d.window.from_marker = Some(s("a"));
        d.window.to_marker = Some(s("b"));
        assert_eq!(check(d), Err(WindowNotTaken { index: 0, key: "to_marker" }));
        let mut d = decl("n", "percentile", 0);
        d.window.percentile = Some(100.5);
        assert_eq!(check(d), Err(PercentileOutOfRange { index: 0 }));
    }

    #[test]
    fn a_field_that_does_not_fit_the_tap_is_refused() {
        let mut d = decl("n", "max", 0);
        d.field = Some(s("temp"));
        assert_eq!(check(d), Err(FieldNotTaken { index: 0 }));
        let mut d = decl("n", "max", 1);
        d.unit = Some(s("1"));
        assert_eq!(check(d.clone()), Err(FieldMissing { index: 0 }));
        d.field = Some(s("humidity"));
        assert_eq!(check(d), Err(UnknownField { index: 0 }));
        let mut d = decl("n", "count", 1);
        d.field = Some(s("temp"));
        assert_eq!(check(d), Err(FieldNotTaken { index: 0 }));
        assert_eq!(check(decl("n", "mean", 2)), Err(NoValues { index: 0 }));
    }

    #[test]
    fn a_unit_that_does_not_fit_is_refused() {
        let mut d = decl("n", "count", 2);
        d.unit = Some(s("1"));
        assert_eq!(check(d), Err(UnitNotTaken { index: 0 }));
        let mut d = decl("n", "max", 1);
        d.field = Some(s("temp"));
        assert_eq!(check(d.clone()), Err(UnitMissing { index: 0 }));
        d.unit = Some(s("degC"));
        assert_eq!(check(d), Err(UnknownUnit { index: 0 }));
        let mut d = decl("n", "max", 0);
        d.unit = Some(s("V"));
        assert_eq!(check(d), Err(UnitDisagrees { index: 0, tap_unit: "mA" }));
    }

    #[test]
    fn a_declared_value_that_is_not_read_or_not_positive_is_refused() {
        let mut d = decl("n", "count", 2);
        d.declared = Some(StudyDeclared { resolution: Some(1.0), ..Default::default() });
        assert_eq!(check(d), Err(DeclaredNotTaken { index: 0, key: "resolution" }));
        let mut d = decl("n", "mean", 0);
        d.declared = Some(StudyDeclared { k: Some(2.0), ..Default::default() });
        assert_eq!(check(d.clone()), Err(DeclaredNotTaken { index: 0, key: "k" }));
        d.declared = Some(StudyDeclared { u_expanded: Some(0.0), ..Default::default() });
        assert_eq!(check(d.clone()), Err(DeclaredNotPositive { index: 0, key: "U" }));
        d.declared = Some(StudyDeclared { sample_rate_hz: Some(f64::NAN), ..Default::default() });
        assert_eq!(check(d), Err(DeclaredNotPositive { index: 0, key: "sample_rate_hz" }));
        let mut d = decl("n", "rate", 0);
        d.declared = Some(StudyDeclared { sample_rate_hz: Some(100.0), ..Default::default() });
        assert_eq!(check(d), Ok(()));
    }

    #[cfg(feature = "reducers")]
    #[test]
    fn the_names_and_units_match_the_reducers() {
        assert_eq!(REDUCER_NAMES, crate::reducers::NAMES);
        assert_eq!(UNITS, crate::reducers::UNITS);
        let registry = crate::reducers::Registry::builtin();
        for name in REDUCER_NAMES {
            assert!(registry.latest(name).is_some(), "{name} is not in the registry");
            assert!(takes(name).is_some(), "{name} has no window rule");
        }
    }
}
