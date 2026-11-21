//! The built-in HIL reducers (decisions 92 to 94, `interfaces/hil-shapes.md`):
//! count, rate, min, max, mean, percentile, the interval between two markers
//! and the value at a marker. Host-only, behind the `reducers` feature.
//!
//! A reducer is a pure function from an [`Input`] to one [`Measurement`], the
//! shape `measurements.json` carries for a derived value. Each is **versioned
//! by behaviour**: [`Registry::builtin`] keeps every behaviour the crate has
//! shipped, so `verify` can ask for the behaviour a record names. Nothing here
//! reads a file, a clock or a Core; `cargo test` against the fixtures in
//! `tests/fixtures/reducers/` is the whole harness.
//!
//! **Uncertainty.** Inputs carry *standard* uncertainties (`u`, in the input's
//! unit, or milliseconds for stamps); an output's `U` is `k * u_c` with `u_c` a
//! root sum of squares (GUM), so expanded values are never combined directly.
//! An interval combines the two stamps' `u` and the clock's; a mean, a
//! percentile, a min, a max and a value-at-marker take the input's `u`
//! undivided (a systematic error does not average out); a count has none; a
//! statistical reducer (mean, rate, percentile) also returns `type_a`, the
//! standard uncertainty of its own estimate from the samples' spread, which is
//! combined by root sum of squares with the instrument term. A reducer whose
//! instrument term is undeclared (`None`) reports `uncertainty: null` and
//! `u_source: undeclared`.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

/// The unit strings `measurements.json` admits (`interfaces/hil-shapes.md`).
pub const UNITS: &[&str] = &["V", "mV", "A", "mA", "W", "mW", "s", "ms", "us", "Hz", "B", "1"];

/// Default coverage factor, a 95 % interval for a normal estimate.
pub const DEFAULT_K: f64 = 2.0;

fn default_k() -> f64 {
    DEFAULT_K
}

/// One timestamped sample, `t_ms` on the clock of `StreamRecord::rx_utc_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub t_ms: u64,
    pub value: f64,
}

/// A named stamp (a marker line's `rx_utc_ms`) and the standard uncertainty of
/// that stamp, in milliseconds; `None` is undeclared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub name: String,
    pub t_ms: u64,
    #[serde(default)]
    pub u_ms: Option<f64>,
}

/// Why a measurement says its input was short. Spelled as
/// `measurements.json` spells `incomplete`; the order here is the order
/// emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Incomplete {
    TruncatedCapture,
    TruncatedReply,
    RecordGap,
    ParseFailed,
}

/// The reducer's own parameters, recorded as `method.window`. Absent keys are
/// omitted from the JSON.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Window {
    /// First instant counted (inclusive); points before it are ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_utc_ms: Option<u64>,
    /// Last instant counted (inclusive). `rate` needs both ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_utc_ms: Option<u64>,
    /// Percentile, 0 to 100, for `percentile`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percentile: Option<f64>,
    /// Marker name: `interval`'s start, or `value_at_marker`'s only marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_marker: Option<String>,
    /// `interval`'s end marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_marker: Option<String>,
}

/// Everything a reducer reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    #[serde(default)]
    pub points: Vec<Point>,
    #[serde(default)]
    pub markers: Vec<Marker>,
    /// Unit of `points[].value`, one of [`UNITS`].
    pub unit: String,
    /// Standard uncertainty of each sample, in `unit`; `None` is undeclared.
    #[serde(default)]
    pub u: Option<f64>,
    /// Standard uncertainty of the stamps' clock, ms; `None` is undeclared.
    #[serde(default)]
    pub clock_u_ms: Option<f64>,
    #[serde(default = "default_k")]
    pub k: f64,
    /// What the caller knows about the capture: `truncated_capture`,
    /// `truncated_reply` or `record_gap`. Copied to the output.
    #[serde(default)]
    pub incomplete: Vec<Incomplete>,
    #[serde(default)]
    pub window: Window,
}

/// The five `uncertainty.basis` values; a reducer only ever emits `Derived`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    Datasheet,
    Calibration,
    Characterisation,
    Declared,
    Derived,
}

/// `{U, k, basis, type_a?}` of `interfaces/hil-shapes.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Uncertainty {
    #[serde(rename = "U")]
    pub u_expanded: f64,
    pub k: f64,
    pub basis: Basis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_a: Option<f64>,
}

/// The measurement's `u_source` as a reducer sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum USource {
    Derived,
    Undeclared,
    None,
}

/// `method` for a derived value: `{kind: "reducer", name, behaviour, window}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Method {
    pub kind: String,
    pub name: String,
    pub behaviour: u32,
    pub window: Window,
}

/// The reducer-owned keys of one measurement. Core adds `name`, `at_utc_ms`,
/// `source`, `by` and the rest around it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    pub value: Option<f64>,
    pub unit: String,
    pub uncertainty: Option<Uncertainty>,
    pub u_source: USource,
    pub method: Method,
    pub incomplete: Vec<Incomplete>,
}

/// A reducer's code: input in, measurement out. Pure and deterministic.
pub type ReducerFn = fn(&Input) -> Measurement;

/// The eight built-in reducer names, spelled as `method.name`.
pub const NAMES: &[&str] =
    &["count", "rate", "min", "max", "mean", "percentile", "interval", "value_at_marker"];

/// Why a behaviour could not be called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReduceError {
    UnknownReducer(String),
    /// The crate never shipped this behaviour of the reducer, so `verify`
    /// cannot recompute the value and says so.
    UnknownBehaviour { name: String, behaviour: u32 },
}

/// Every behaviour of every reducer, kept forever (decision 93).
pub struct Registry {
    entries: Vec<(&'static str, u32, ReducerFn)>,
}

impl Registry {
    /// The behaviours this crate has shipped.
    pub fn builtin() -> Self {
        let mut r = Registry { entries: Vec::new() };
        r.register("count", 1, count_v1);
        r.register("rate", 1, rate_v1);
        r.register("min", 1, min_v1);
        r.register("max", 1, max_v1);
        r.register("mean", 1, mean_v1);
        r.register("percentile", 1, percentile_v1);
        r.register("interval", 1, interval_v1);
        r.register("value_at_marker", 1, value_at_marker_v1);
        r
    }

    /// Adds a behaviour. A (name, behaviour) pair is never replaced.
    pub fn register(&mut self, name: &'static str, behaviour: u32, f: ReducerFn) {
        assert!(
            !self.entries.iter().any(|e| e.0 == name && e.1 == behaviour),
            "behaviour already shipped"
        );
        self.entries.push((name, behaviour, f));
    }

    /// The newest behaviour of `name`: what a new record names.
    pub fn latest(&self, name: &str) -> Option<u32> {
        self.entries.iter().filter(|e| e.0 == name).map(|e| e.1).max()
    }

    /// Every behaviour of `name`, ascending.
    pub fn behaviours(&self, name: &str) -> Vec<u32> {
        let mut v: Vec<u32> = self.entries.iter().filter(|e| e.0 == name).map(|e| e.1).collect();
        v.sort_unstable();
        v
    }

    /// Runs `name` at exactly `behaviour`.
    pub fn reduce(&self, name: &str, behaviour: u32, input: &Input) -> Result<Measurement, ReduceError> {
        match self.entries.iter().find(|e| e.0 == name && e.1 == behaviour) {
            Some(e) => Ok((e.2)(input)),
            None if self.latest(name).is_none() => Err(ReduceError::UnknownReducer(name.to_string())),
            None => Err(ReduceError::UnknownBehaviour { name: name.to_string(), behaviour }),
        }
    }

    /// Runs the newest behaviour; the result's `method.behaviour` says which.
    pub fn reduce_latest(&self, name: &str, input: &Input) -> Result<Measurement, ReduceError> {
        let b = self.latest(name).ok_or_else(|| ReduceError::UnknownReducer(name.to_string()))?;
        self.reduce(name, b, input)
    }
}

// ---- numerics ------------------------------------------------------------

/// Square root by Newton's method from a bit-pattern guess: only `+ * /`, each
/// correctly rounded, so the result is the same on every target and needs
/// neither `std` nor `libm` (decision 92).
pub fn sqrt(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 || x == f64::INFINITY {
        return x;
    }
    let mut g = f64::from_bits((x.to_bits() >> 1) + 0x1FF8_0000_0000_0000);
    for _ in 0..80 {
        let n = 0.5 * (g + x / g);
        if n == g {
            break;
        }
        g = n;
    }
    g
}

fn rss(terms: &[f64]) -> f64 {
    let mut s = 0.0;
    for t in terms {
        s += t * t;
    }
    sqrt(s)
}

fn in_window(i: &Input) -> Vec<&Point> {
    i.points
        .iter()
        .filter(|p| {
            i.window.from_utc_ms.is_none_or(|f| p.t_ms >= f) && i.window.to_utc_ms.is_none_or(|t| p.t_ms <= t)
        })
        .collect()
}

fn base(name: &str, i: &Input, unit: &str) -> Measurement {
    Measurement {
        value: None,
        unit: unit.to_string(),
        uncertainty: None,
        u_source: USource::None,
        method: Method { kind: "reducer".to_string(), name: name.to_string(), behaviour: 1, window: i.window.clone() },
        incomplete: i.incomplete.clone(),
    }
}

/// Orders `incomplete` canonically and adds `parse_failed` when the reducer
/// could not produce a value or met a non-finite input.
fn finish(mut m: Measurement, failed: bool) -> Measurement {
    if failed {
        m.value = None;
        m.uncertainty = None;
        m.u_source = USource::Derived;
        m.incomplete.push(Incomplete::ParseFailed);
    }
    let order = |x: &Incomplete| match x {
        Incomplete::TruncatedCapture => 0,
        Incomplete::TruncatedReply => 1,
        Incomplete::RecordGap => 2,
        Incomplete::ParseFailed => 3,
    };
    m.incomplete.sort_by_key(order);
    m.incomplete.dedup();
    m
}

/// Sets `uncertainty` from an instrument term and an optional type-A term.
fn set_u(m: &mut Measurement, k: f64, u_instr: Option<f64>, type_a: Option<f64>) {
    match u_instr {
        None => m.u_source = USource::Undeclared,
        Some(u) => {
            let c = match type_a {
                Some(a) => rss(&[u, a]),
                None => u,
            };
            m.u_source = USource::Derived;
            m.uncertainty = Some(Uncertainty { u_expanded: k * c, k, basis: Basis::Derived, type_a });
        }
    }
}

fn all_finite(pts: &[&Point]) -> bool {
    pts.iter().all(|p| p.value.is_finite())
}

// ---- behaviour 1 of each reducer ----------------------------------------

/// `count`: the number of points in the window. Unit `1`, no uncertainty.
/// Empty input is a count of 0, complete.
fn count_v1(i: &Input) -> Measurement {
    let mut m = base("count", i, "1");
    m.value = Some(in_window(i).len() as f64);
    finish(m, false)
}

/// `rate`: points per second over the window, `n / ((to - from) / 1000)`, Hz.
/// Needs both window ends and `to > from`. Instrument term: the window is
/// bounded by two stamps, so `rate * sqrt(2) * clock_u / T`; type A: the
/// Poisson estimate `sqrt(n) / T`.
fn rate_v1(i: &Input) -> Measurement {
    let mut m = base("rate", i, "Hz");
    let (from, to) = match (i.window.from_utc_ms, i.window.to_utc_ms) {
        (Some(f), Some(t)) if t > f => (f, t),
        _ => return finish(m, true),
    };
    let n = in_window(i).len() as f64;
    let t_s = (to - from) as f64 / 1000.0;
    let rate = n / t_s;
    m.value = Some(rate);
    let u_instr = i.clock_u_ms.map(|c| rate * sqrt(2.0) * (c / 1000.0) / t_s);
    set_u(&mut m, i.k, u_instr, Some(sqrt(n) / t_s));
    finish(m, false)
}

fn extreme(name: &str, i: &Input, want_max: bool) -> Measurement {
    let mut m = base(name, i, &i.unit);
    let pts = in_window(i);
    if pts.is_empty() || !all_finite(&pts) {
        return finish(m, true);
    }
    let mut v = pts[0].value;
    for p in &pts[1..] {
        if (want_max && p.value > v) || (!want_max && p.value < v) {
            v = p.value;
        }
    }
    m.value = Some(v);
    set_u(&mut m, i.k, i.u, None);
    finish(m, false)
}

/// `min`: the smallest value in the window; takes the input's `u` undivided.
fn min_v1(i: &Input) -> Measurement {
    extreme("min", i, false)
}

/// `max`: the largest value in the window; takes the input's `u` undivided.
fn max_v1(i: &Input) -> Measurement {
    extreme("max", i, true)
}

/// `mean`: the arithmetic mean. Instrument term: the input's `u`, undivided.
/// Type A: `s / sqrt(n)` with `s` the sample standard deviation (n - 1);
/// 0 for a single point.
fn mean_v1(i: &Input) -> Measurement {
    let mut m = base("mean", i, &i.unit);
    let pts = in_window(i);
    if pts.is_empty() || !all_finite(&pts) {
        return finish(m, true);
    }
    let n = pts.len() as f64;
    let mean = pts.iter().map(|p| p.value).sum::<f64>() / n;
    m.value = Some(mean);
    let type_a = if pts.len() < 2 {
        0.0
    } else {
        let ss: f64 = pts.iter().map(|p| (p.value - mean) * (p.value - mean)).sum();
        sqrt(ss / (n - 1.0)) / sqrt(n)
    };
    set_u(&mut m, i.k, i.u, Some(type_a));
    finish(m, false)
}

/// `percentile`: linear interpolation between order statistics at rank
/// `p/100 * (n - 1)` (0-based). Instrument term: the input's `u` undivided.
/// Type A: half the spread of the order statistics at ranks
/// `rank +- sqrt(n * q * (1 - q))`, `q = p/100`, clamped to the sample: the
/// distribution-free one-sigma interval of a quantile. `p` outside 0 to 100,
/// or absent, fails.
fn percentile_v1(i: &Input) -> Measurement {
    let mut m = base("percentile", i, &i.unit);
    let pts = in_window(i);
    let p = match i.window.percentile {
        Some(p) if (0.0..=100.0).contains(&p) => p,
        _ => return finish(m, true),
    };
    if pts.is_empty() || !all_finite(&pts) {
        return finish(m, true);
    }
    let mut v: Vec<f64> = pts.iter().map(|p| p.value).collect();
    v.sort_by(|a, b| a.total_cmp(b));
    let last = (v.len() - 1) as f64;
    let at = |rank: f64| {
        let lo = rank as usize;
        let hi = if lo + 1 < v.len() { lo + 1 } else { lo };
        v[lo] + (rank - lo as f64) * (v[hi] - v[lo])
    };
    let q = p / 100.0;
    let rank = q * last;
    m.value = Some(at(rank));
    let n = v.len() as f64;
    let d = sqrt(n * q * (1.0 - q));
    let lo = if rank - d > 0.0 { rank - d } else { 0.0 };
    let hi = if rank + d < last { rank + d } else { last };
    set_u(&mut m, i.k, i.u, Some((at(hi) - at(lo)) / 2.0));
    finish(m, false)
}

fn marker<'a>(i: &'a Input, name: &Option<String>) -> Option<&'a Marker> {
    let n = name.as_ref()?;
    i.markers.iter().find(|m| &m.name == n)
}

/// `interval`: milliseconds from `from_marker` to `to_marker`, the first
/// marker of each name. Unit `ms`. `u` is the root sum of squares of the two
/// stamps' `u_ms` and the clock's; any undeclared makes it undeclared. A
/// missing marker, or `to` before `from`, fails.
fn interval_v1(i: &Input) -> Measurement {
    let mut m = base("interval", i, "ms");
    let (a, b) = match (marker(i, &i.window.from_marker), marker(i, &i.window.to_marker)) {
        (Some(a), Some(b)) if b.t_ms >= a.t_ms => (a, b),
        _ => return finish(m, true),
    };
    m.value = Some((b.t_ms - a.t_ms) as f64);
    let u = match (a.u_ms, b.u_ms, i.clock_u_ms) {
        (Some(x), Some(y), Some(c)) => Some(rss(&[x, y, c])),
        _ => None,
    };
    set_u(&mut m, i.k, u, None);
    finish(m, false)
}

/// `value_at_marker`: the value of the latest point at or before the marker
/// named by `from_marker` (sample and hold). Takes the input's `u` undivided.
/// No marker, or no point at or before it, fails.
fn value_at_marker_v1(i: &Input) -> Measurement {
    let mut m = base("value_at_marker", i, &i.unit);
    let mk = match marker(i, &i.window.from_marker) {
        Some(x) => x,
        None => return finish(m, true),
    };
    let mut best: Option<&Point> = None;
    for p in &i.points {
        if p.t_ms <= mk.t_ms && best.is_none_or(|b| p.t_ms >= b.t_ms) {
            best = Some(p);
        }
    }
    match best {
        Some(p) if p.value.is_finite() => {
            m.value = Some(p.value);
            set_u(&mut m, i.k, i.u, None);
            finish(m, false)
        }
        _ => finish(m, true),
    }
}
