//! `core-validation` feature: `SignalCheck` evaluation — design.md §3 decision 19.
//!
//! Only compiled when a consumer's build enables the `core-validation`
//! feature (in practice, only `embarch-core` does). `embarch-api` and
//! dev-bench firmware link this crate without it, so they can
//! serialize/deserialize/display every `SignalCheck` variant but never
//! compile the code that evaluates one against real sample data.

use heapless::String;

use crate::validation::{ContentValidity, SignalCheck};

/// Evaluates a `SignalCheck` against a slice of already-decoded sample
/// values (e.g. every `Sample::value` captured for the channel named by this
/// check's `ValidationSource`).
///
/// `sample_rate_hz` is required for `SignalCheck::FftPeakNear`'s
/// frequency-domain check and unused otherwise — pass whatever the
/// originating step's `PowerSampleWindow` specified for a `PowerSamples`
/// channel. `SensorWaveform`'s sample rate isn't modeled yet (design.md §7).
pub fn evaluate(check: &SignalCheck, samples: &[f32], sample_rate_hz: f32) -> ContentValidity {
    if samples.is_empty() {
        return invalid("no samples captured for this channel");
    }
    match *check {
        SignalCheck::MeanInRange { min, max } => {
            let mean = samples.iter().sum::<f32>() / samples.len() as f32;
            if (min..=max).contains(&mean) {
                ContentValidity::Valid
            } else {
                invalid("mean outside expected range")
            }
        }
        SignalCheck::NoGlitchAbove { threshold } => {
            if samples.iter().any(|s| s.abs() > threshold) {
                invalid("a sample exceeded the glitch threshold")
            } else {
                ContentValidity::Valid
            }
        }
        SignalCheck::FftPeakNear { hz, tolerance_hz } => match peak_frequency(samples, sample_rate_hz) {
            Some(peak_hz) if (peak_hz - hz).abs() <= tolerance_hz => ContentValidity::Valid,
            Some(_) => invalid("dominant frequency outside tolerance"),
            None => invalid("sample rate must be positive to run a frequency-domain check"),
        },
    }
}

fn invalid(reason: &str) -> ContentValidity {
    ContentValidity::Invalid {
        reason: String::try_from(reason).unwrap_or_default(),
    }
}

/// Naive O(n^2) DFT magnitude search for the dominant non-DC frequency.
/// Adequate for the sample counts a single study step captures; swap for a
/// real FFT crate if profiling ever shows this matters (design.md §7 already
/// flags `SignalCheck`'s DSP implementation as provisional).
fn peak_frequency(samples: &[f32], sample_rate_hz: f32) -> Option<f32> {
    if sample_rate_hz <= 0.0 {
        return None;
    }
    let n = samples.len();
    let mut best_bin = 0usize;
    let mut best_magnitude = 0f32;
    // Skip bin 0 (DC) and only search the first half (real-signal symmetry).
    for k in 1..n / 2 {
        let (mut re, mut im) = (0f32, 0f32);
        for (t, &sample) in samples.iter().enumerate() {
            let angle = -2.0 * core::f32::consts::PI * (k as f32) * (t as f32) / (n as f32);
            re += sample * angle.cos();
            im += sample * angle.sin();
        }
        let magnitude = (re * re + im * im).sqrt();
        if magnitude > best_magnitude {
            best_magnitude = magnitude;
            best_bin = k;
        }
    }
    if best_bin == 0 {
        return None;
    }
    Some(best_bin as f32 * sample_rate_hz / n as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_in_range_passes_and_fails() {
        let samples = [1.0, 2.0, 3.0];
        let check = SignalCheck::MeanInRange { min: 1.5, max: 2.5 };
        assert_eq!(evaluate(&check, &samples, 1_000.0), ContentValidity::Valid);

        let check = SignalCheck::MeanInRange { min: 10.0, max: 20.0 };
        assert!(matches!(evaluate(&check, &samples, 1_000.0), ContentValidity::Invalid { .. }));
    }

    #[test]
    fn no_glitch_above_detects_spike() {
        let samples = [0.1, 0.2, 5.0, 0.1];
        let check = SignalCheck::NoGlitchAbove { threshold: 1.0 };
        assert!(matches!(evaluate(&check, &samples, 1_000.0), ContentValidity::Invalid { .. }));

        let check = SignalCheck::NoGlitchAbove { threshold: 10.0 };
        assert_eq!(evaluate(&check, &samples, 1_000.0), ContentValidity::Valid);
    }

    #[test]
    fn fft_peak_near_finds_dominant_tone() {
        let sample_rate_hz = 1_000.0f32;
        let tone_hz = 100.0f32;
        let n = 256;
        let samples: std::vec::Vec<f32> = (0..n)
            .map(|t| (2.0 * core::f32::consts::PI * tone_hz * t as f32 / sample_rate_hz).sin())
            .collect();

        let check = SignalCheck::FftPeakNear { hz: tone_hz, tolerance_hz: 10.0 };
        assert_eq!(evaluate(&check, &samples, sample_rate_hz), ContentValidity::Valid);

        let check = SignalCheck::FftPeakNear { hz: 300.0, tolerance_hz: 10.0 };
        assert!(matches!(evaluate(&check, &samples, sample_rate_hz), ContentValidity::Invalid { .. }));
    }
}
