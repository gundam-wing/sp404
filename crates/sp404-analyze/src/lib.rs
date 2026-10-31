//! Onset, tempo, key, and slice analysis for Pad404.
//!
//! This crate is native-only. `sp404-core` does not depend on it, and the UI
//! does not either, so a `wasm32` build of the sampler core never links
//! `rustfft` or this code.
//!
//! The same input and the same [`AnalyzeOptions`] always produce the same
//! [`Analysis`]. There is no randomness and no thread pool.
//!
//! # Detectors
//!
//! * **Onsets.** Half-wave spectral flux, a peak pick, then a snap of each
//!   peak to a nearby zero crossing. The snap is what a later chop uses so a
//!   slice does not start on a waveform peak.
//! * **Tempo.** Autocorrelation of that onset strength, then Ellis-style
//!   dynamic-programming beat tracking. A half-time / double-time guard
//!   compares the estimate with the median onset spacing.
//! * **Key.** Peak-picked chroma correlated with the Krumhansl–Schmuckler
//!   major and minor profiles. The name is `"C major"` or `"F# minor"`.
//!   Silence is `"unknown"` with confidence 0.
//! * **Slices.** One slice per snapped onset: `start`, `end`, `fade_in`, and
//!   `fade_out`, in seconds. No audio is written.
//!
//! # JSON
//!
//! [`Analysis::to_json`] writes the document the CLI prints. The fields are
//! `sample_rate`, `duration`, `bpm`, `beat_times`, `onset_times`, `key`,
//! `key_confidence`, and `slices`. Times are seconds. `bpm` is 0 when no
//! period could be estimated.
//!
//! ```
//! use sp404_analyze::{analyze, AnalyzeOptions};
//! let sr = 22_050.0;
//! let mut samples = vec![0.0; sr as usize];
//! samples[sr as usize / 4] = 1.0;
//! let analysis = analyze(&samples, sr, &AnalyzeOptions::default()).unwrap();
//! assert!(analysis.duration > 0.9 && analysis.duration < 1.1);
//! assert!((0.0..=1.0).contains(&analysis.key_confidence));
//! ```

#![forbid(unsafe_code)]

mod error;
mod key;
mod onset;
mod slice;
mod spectrum;
mod tempo;

pub use error::Error;
pub use slice::Slice;

use serde::Serialize;

/// Knobs for [`analyze`]. Defaults match the `sp404 analyze` CLI.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalyzeOptions {
    /// STFT hop in samples. Default 512.
    pub hop_size: usize,
    /// STFT size in samples. Power of two, at least 32, at most 65536.
    /// Default 2048.
    pub fft_size: usize,
    /// Slowest tempo the autocorrelation may return. Default 60.
    pub min_bpm: f64,
    /// Fastest tempo the autocorrelation may return. Default 180.
    pub max_bpm: f64,
    /// Nominal slice fade in milliseconds, clamped per slice. Default 5.
    pub fade_ms: f64,
    /// How far, in milliseconds, an onset may move to reach a zero crossing.
    /// Default 20.
    pub zero_cross_ms: f64,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        Self {
            hop_size: 512,
            fft_size: 2048,
            min_bpm: 60.0,
            max_bpm: 180.0,
            fade_ms: 5.0,
            zero_cross_ms: 20.0,
        }
    }
}

/// Deterministic analysis of one mono buffer.
///
/// `samples` is mono `f32`. Stereo should be mixed by the caller (the CLI
/// averages channels). Non-finite samples are treated as silence. Times in
/// the result are seconds from the start of this buffer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Analysis {
    /// Sample rate in hertz.
    pub sample_rate: f64,
    /// Buffer length in seconds.
    pub duration: f64,
    /// Estimated tempo. `0` when the signal has no reliable period.
    pub bpm: f64,
    /// Beat times in seconds, from the Ellis tracker.
    pub beat_times: Vec<f64>,
    /// Snapped onset times in seconds.
    pub onset_times: Vec<f64>,
    /// `"C major"`, `"A minor"`, or `"unknown"`.
    pub key: String,
    /// Pearson correlation of the winning profile, in 0..1.
    pub key_confidence: f64,
    /// Slice boundaries derived from `onset_times`.
    pub slices: Vec<Slice>,
}

impl Analysis {
    /// Pretty JSON with the stable field names.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("analysis values are finite")
    }
}

/// Analyze mono `samples` at `sample_rate`.
pub fn analyze(
    samples: &[f32],
    sample_rate: f32,
    options: &AnalyzeOptions,
) -> Result<Analysis, Error> {
    validate(samples, sample_rate, options)?;
    let mono: Vec<f32> = samples
        .iter()
        .map(|sample| if sample.is_finite() { *sample } else { 0.0 })
        .collect();
    let spec = spectrum::analyze(&mono, sample_rate, options.fft_size, options.hop_size);
    let zero_cross = ((options.zero_cross_ms / 1_000.0) * f64::from(sample_rate))
        .round()
        .max(0.0) as usize;
    let onset_samples = onset::detect(
        &mono,
        &spec.flux,
        sample_rate,
        options.hop_size,
        options.fft_size,
        zero_cross,
    );
    let mut onset_times: Vec<f64> = onset_samples
        .iter()
        .map(|&sample| round_secs(sample as f64 / f64::from(sample_rate)))
        .collect();
    onset_times.dedup_by(|a, b| (*a - *b).abs() < 1.0e-6);

    let (key, confidence) = key::match_chroma(&spec.chroma);
    let frame_rate = f64::from(sample_rate) / options.hop_size as f64;
    let bpm = round_bpm(tempo::estimate_bpm(
        &spec.flux,
        frame_rate,
        &onset_times,
        options.min_bpm,
        options.max_bpm,
    ));
    let duration = round_secs(mono.len() as f64 / f64::from(sample_rate));
    let beat_times = if bpm > 0.0 {
        tempo::track_beats(
            &spec.flux,
            bpm,
            f64::from(sample_rate),
            options.hop_size,
            options.fft_size,
            duration,
        )
    } else {
        Vec::new()
    };
    let beat_times = round_times(beat_times);
    let fade = options.fade_ms / 1_000.0;
    let slices = slice::from_onsets(&onset_times, duration, fade)
        .into_iter()
        .map(|slice| Slice {
            start: round_secs(slice.start),
            end: round_secs(slice.end),
            fade_in: round_secs(slice.fade_in),
            fade_out: round_secs(slice.fade_out),
        })
        .filter(|slice| slice.end > slice.start)
        .collect();

    Ok(Analysis {
        sample_rate: round_secs(f64::from(sample_rate)),
        duration,
        bpm,
        beat_times,
        onset_times,
        key,
        key_confidence: round_unit(confidence),
        slices,
    })
}

fn validate(samples: &[f32], sample_rate: f32, options: &AnalyzeOptions) -> Result<(), Error> {
    if samples.is_empty() {
        return Err(Error::InvalidInput("audio is empty".into()));
    }
    if !(sample_rate.is_finite() && sample_rate >= 1_000.0) {
        return Err(Error::InvalidInput(
            "sample rate must be finite and at least 1000 Hz".into(),
        ));
    }
    if options.hop_size == 0 {
        return Err(Error::InvalidOption("hop size must be at least 1".into()));
    }
    if options.fft_size < 32 || options.fft_size > 65_536 || !options.fft_size.is_power_of_two() {
        return Err(Error::InvalidOption(
            "fft size must be a power of two from 32 to 65536".into(),
        ));
    }
    if options.hop_size > options.fft_size {
        return Err(Error::InvalidOption(
            "hop size must be less than or equal to fft size".into(),
        ));
    }
    if !(options.min_bpm.is_finite() && options.min_bpm >= 1.0) {
        return Err(Error::InvalidOption(
            "min bpm must be finite and at least 1".into(),
        ));
    }
    if !(options.max_bpm.is_finite() && options.max_bpm > options.min_bpm) {
        return Err(Error::InvalidOption(
            "max bpm must be greater than min bpm".into(),
        ));
    }
    if !(options.fade_ms.is_finite() && options.fade_ms >= 0.0) {
        return Err(Error::InvalidOption(
            "fade must be finite and non-negative".into(),
        ));
    }
    if !(options.zero_cross_ms.is_finite() && options.zero_cross_ms >= 0.0) {
        return Err(Error::InvalidOption(
            "zero-cross window must be finite and non-negative".into(),
        ));
    }
    Ok(())
}

fn round_secs(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn round_bpm(value: f64) -> f64 {
    if !value.is_finite() || value <= 0.0 {
        return 0.0;
    }
    (value * 100.0).round() / 100.0
}

fn round_unit(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    (value.clamp(0.0, 1.0) * 10_000.0).round() / 10_000.0
}

fn round_times(times: Vec<f64>) -> Vec<f64> {
    let mut times: Vec<f64> = times.into_iter().map(round_secs).collect();
    times.dedup_by(|a, b| (*a - *b).abs() < 1.0e-6);
    times
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clicks(sr: f32, times: &[f64]) -> Vec<f32> {
        let n = (sr * (times.last().copied().unwrap_or(0.0) as f32 + 1.0)) as usize;
        let mut mono = vec![0.0f32; n.max(1)];
        for &time in times {
            let at = (time as f32 * sr).round() as usize;
            if at < mono.len() {
                mono[at] = 1.0;
            }
        }
        mono
    }

    fn burst_track(sr: f32, bpm: f64, duration: f64) -> (Vec<f32>, Vec<f64>) {
        let n = (sr as f64 * duration).round() as usize;
        let mut mono = vec![0.0f32; n];
        let mut times = Vec::new();
        let interval = 60.0 / bpm;
        let mut time = 0.5;
        while time < duration - 0.25 {
            times.push(time);
            add_burst(&mut mono, sr, time);
            time += interval;
        }
        (mono, times)
    }

    fn add_burst(mono: &mut [f32], sr: f32, time: f64) {
        let at = (time * f64::from(sr)).round() as usize;
        let freq = 1_000.0f32;
        let decay = 0.0015 * sr;
        let length = (0.012 * sr) as usize;
        for i in 0..length {
            let index = at + i;
            if index >= mono.len() {
                break;
            }
            let env = (-(i as f32) / decay).exp();
            let phase = std::f32::consts::FRAC_PI_2;
            mono[index] +=
                env * (2.0 * std::f32::consts::PI * freq * (i as f32) / sr + phase).sin();
        }
    }

    fn tone(sr: f32, duration: f32, midis: &[i32]) -> Vec<f32> {
        let n = (sr * duration) as usize;
        let mut out = vec![0.0f32; n];
        let fade = (0.02 * sr) as usize;
        for &midi in midis {
            let freq = 440.0 * 2.0f32.powf((midi as f32 - 69.0) / 12.0);
            for (i, sample) in out.iter_mut().enumerate() {
                let env = if i < fade {
                    i as f32 / fade as f32
                } else if i + fade > n {
                    (n - i) as f32 / fade.max(1) as f32
                } else {
                    1.0
                };
                let t = i as f32 / sr;
                *sample += env * (2.0 * std::f32::consts::PI * freq * t).sin();
            }
        }
        let peak = out
            .iter()
            .copied()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        if peak > 0.0 {
            for sample in &mut out {
                *sample /= peak;
            }
        }
        out
    }

    fn wide_tempo() -> AnalyzeOptions {
        AnalyzeOptions {
            min_bpm: 40.0,
            max_bpm: 280.0,
            ..AnalyzeOptions::default()
        }
    }

    #[test]
    fn impulses_land_near_known_clicks_and_on_zero_crossings() {
        let sr = 44_100.0f32;
        let clicks_at = [0.30, 0.70, 1.10, 1.50];
        let mono = clicks(sr, &clicks_at);
        let analysis = analyze(&mono, sr, &AnalyzeOptions::default()).unwrap();
        assert_eq!(
            analysis.onset_times.len(),
            clicks_at.len(),
            "onsets {:?}",
            analysis.onset_times
        );
        for &click in &clicks_at {
            let nearest = analysis
                .onset_times
                .iter()
                .map(|time| (time - click).abs())
                .fold(f64::MAX, f64::min);
            assert!(
                nearest < 0.03,
                "click {click} nearest onset error {nearest}"
            );
        }
        for &time in &analysis.onset_times {
            let index = (time * f64::from(sr)).round() as usize;
            assert!(index < mono.len());
            assert!(
                mono[index].abs() < 0.2,
                "onset at {index} stayed on the impulse ({})",
                mono[index]
            );
            let impulse = clicks_at
                .iter()
                .map(|click| ((click * f64::from(sr)).round() as usize, click))
                .min_by_key(|(sample, _)| sample.abs_diff(index))
                .unwrap();
            assert!(mono[impulse.0].abs() > 0.9);
            assert!(index != impulse.0, "onset did not leave the impulse peak");
        }
    }

    #[test]
    fn click_track_bpm_is_not_an_octave_error() {
        let sr = 22_050.0f32;
        let (mono, clicks_at) = burst_track(sr, 120.0, 5.0);
        let analysis = analyze(&mono, sr, &wide_tempo()).unwrap();
        assert!(
            analysis.onset_times.len() >= clicks_at.len() - 1,
            "onsets {:?} clicks {:?}",
            analysis.onset_times,
            clicks_at
        );
        for &click in &clicks_at {
            let nearest = analysis
                .onset_times
                .iter()
                .map(|time| (time - click).abs())
                .fold(f64::MAX, f64::min);
            assert!(nearest < 0.03, "click {click} nearest {nearest}");
        }
        assert!(
            (analysis.bpm - 120.0).abs() < 12.0,
            "bpm {} is outside the click-track neighborhood",
            analysis.bpm
        );
        assert!(
            (analysis.bpm - 60.0).abs() > 20.0 && (analysis.bpm - 240.0).abs() > 20.0,
            "octave error bpm {}",
            analysis.bpm
        );
        assert!(
            analysis.beat_times.len() >= 4,
            "beats {:?}",
            analysis.beat_times
        );
        let mut intervals: Vec<f64> = analysis
            .beat_times
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .collect();
        intervals.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = intervals[intervals.len() / 2];
        assert!(
            (median - 0.5).abs() < 0.05,
            "beat interval {median}, beats {:?}",
            analysis.beat_times
        );
    }

    #[test]
    fn slices_follow_snapped_onsets_and_record_fades() {
        let sr = 22_050.0f32;
        let (mono, _) = burst_track(sr, 120.0, 4.0);
        let options = AnalyzeOptions {
            fade_ms: 5.0,
            ..wide_tempo()
        };
        let analysis = analyze(&mono, sr, &options).unwrap();
        assert_eq!(analysis.slices.len(), analysis.onset_times.len());
        for (slice, onset) in analysis.slices.iter().zip(&analysis.onset_times) {
            assert_eq!(slice.start, *onset);
            assert!(slice.end > slice.start);
            assert!(slice.fade_in > 0.0 && slice.fade_in <= 0.005 + 1.0e-6);
            assert_eq!(slice.fade_in, slice.fade_out);
        }
        let last = analysis.slices.last().unwrap();
        assert!((last.end - analysis.duration).abs() < 1.0e-6);
    }

    #[test]
    fn harmonic_triad_prefers_its_major_key() {
        let sr = 22_050.0f32;
        let options = AnalyzeOptions {
            fft_size: 4096,
            hop_size: 1024,
            ..AnalyzeOptions::default()
        };
        let c_major = tone(sr, 2.0, &[72, 76, 79]);
        let analysis = analyze(&c_major, sr, &options).unwrap();
        assert_eq!(analysis.key, "C major");
        assert!(
            analysis.key_confidence > 0.5,
            "confidence {}",
            analysis.key_confidence
        );

        let d_major = tone(sr, 2.0, &[74, 78, 81]);
        let rotated = analyze(&d_major, sr, &options).unwrap();
        assert_eq!(rotated.key, "D major");
        assert!(
            rotated.key_confidence > 0.5,
            "confidence {}",
            rotated.key_confidence
        );
    }

    #[test]
    fn repeated_analysis_matches() {
        let sr = 22_050.0f32;
        let (mono, _) = burst_track(sr, 100.0, 3.0);
        let options = AnalyzeOptions::default();
        let first = analyze(&mono, sr, &options).unwrap();
        let second = analyze(&mono, sr, &options).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.to_json(), second.to_json());
    }

    #[test]
    fn json_field_names_stay_stable() {
        let sr = 22_050.0f32;
        let (mono, _) = burst_track(sr, 120.0, 3.0);
        let analysis = analyze(&mono, sr, &AnalyzeOptions::default()).unwrap();
        let value = serde_json::to_value(&analysis).unwrap();
        for field in [
            "sample_rate",
            "duration",
            "bpm",
            "beat_times",
            "onset_times",
            "key",
            "key_confidence",
            "slices",
        ] {
            assert!(value.get(field).is_some(), "missing {field}");
        }
        let slice = &value["slices"][0];
        for field in ["start", "end", "fade_in", "fade_out"] {
            assert!(slice.get(field).is_some(), "missing slice.{field}");
        }
    }

    #[test]
    fn bad_options_fail() {
        let samples = vec![0.0f32; 2_000];
        let inverted = AnalyzeOptions {
            min_bpm: 180.0,
            max_bpm: 60.0,
            ..AnalyzeOptions::default()
        };
        assert!(analyze(&samples, 44_100.0, &inverted).is_err());
        let bad_fft = AnalyzeOptions {
            fft_size: 1000,
            ..AnalyzeOptions::default()
        };
        assert!(analyze(&samples, 44_100.0, &bad_fft).is_err());
        assert!(analyze(&[], 44_100.0, &AnalyzeOptions::default()).is_err());
    }
}
