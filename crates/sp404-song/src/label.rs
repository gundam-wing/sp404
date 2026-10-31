//! Heuristic drum labels.
//!
//! The same samples always get the same label. The decision order is fixed:
//! near-silence is melodic, then kick, hat, snare, and everything else is
//! melodic. Thresholds are intentionally coarse.

use serde::{Deserialize, Serialize};

/// A slice role the generator can place on a pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceLabel {
    /// Low, sparse zero crossings.
    Kick,
    /// Short and noisy, with more low end than a hat.
    Snare,
    /// Short, bright, little low end.
    Hat,
    /// Sustained tones, silence, and anything that is not a drum.
    Melodic,
}

impl SliceLabel {
    /// Stable JSON / manifest spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kick => "kick",
            Self::Snare => "snare",
            Self::Hat => "hat",
            Self::Melodic => "melodic",
        }
    }

    /// Parse a manifest label.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "kick" => Some(Self::Kick),
            "snare" => Some(Self::Snare),
            "hat" => Some(Self::Hat),
            "melodic" => Some(Self::Melodic),
            _ => None,
        }
    }

    pub(crate) fn index(self) -> usize {
        match self {
            Self::Kick => 0,
            Self::Snare => 1,
            Self::Hat => 2,
            Self::Melodic => 3,
        }
    }
}

/// Classify one mono slice.
///
/// `kick` when most of the energy sits below about 150 Hz and zero crossings
/// are sparse. `hat` when crossings are dense, the low band is quiet, and the
/// slice is short. `snare` when the slice is a short noisy hit that is not
/// low enough for a kick or thin enough for a hat. Near-silence and sustained
/// tones are `melodic`.
pub fn label_slice(samples: &[f32], sample_rate: f32) -> SliceLabel {
    let (rms, zcr, low_ratio, seconds) = features(samples, sample_rate);
    if rms < 1.0e-4 {
        return SliceLabel::Melodic;
    }
    if low_ratio >= 0.55 && zcr <= 0.12 {
        return SliceLabel::Kick;
    }
    if zcr >= 0.20 && low_ratio <= 0.30 && seconds <= 0.35 {
        return SliceLabel::Hat;
    }
    if zcr >= 0.09 && low_ratio < 0.55 && seconds <= 0.45 {
        return SliceLabel::Snare;
    }
    SliceLabel::Melodic
}

fn features(samples: &[f32], sample_rate: f32) -> (f32, f32, f32, f32) {
    let n = samples.len();
    if n < 2 || !(sample_rate.is_finite() && sample_rate > 0.0) {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let dt = 1.0 / f64::from(sample_rate);
    let alpha = dt / ((1.0 / (2.0 * std::f64::consts::PI * 150.0)) + dt);
    let mut energy = 0.0f64;
    let mut low_energy = 0.0f64;
    let mut low = 0.0f64;
    let mut crossings = 0usize;
    for (index, sample) in samples.iter().copied().enumerate() {
        let x = if sample.is_finite() {
            f64::from(sample)
        } else {
            0.0
        };
        energy += x * x;
        low += alpha * (x - low);
        low_energy += low * low;
        if index > 0 {
            let previous = samples[index - 1];
            if previous.is_finite() && previous * sample < 0.0 {
                crossings += 1;
            }
        }
    }
    let rms = (energy / n as f64).sqrt() as f32;
    let zcr = crossings as f32 / (n - 1) as f32;
    let low_ratio = if energy > 1.0e-12 {
        (low_energy / energy).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let seconds = n as f32 / sample_rate;
    (rms, zcr, low_ratio, seconds)
}

#[cfg(test)]
mod tests {
    use super::{label_slice, SliceLabel};

    fn sine(sr: f32, freq: f32, seconds: f32, amp: f32) -> Vec<f32> {
        let n = (sr * seconds) as usize;
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin())
            .collect()
    }

    fn noise(n: usize, amp: f32, seed: u64) -> Vec<f32> {
        let mut state = seed | 1;
        (0..n)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                let unit = ((state >> 33) as f32) / (1u32 << 31) as f32;
                amp * (unit * 2.0 - 1.0)
            })
            .collect()
    }

    #[test]
    fn synthetic_hits_get_the_expected_labels() {
        let sr = 16_000.0;
        assert_eq!(
            label_slice(&sine(sr, 70.0, 0.12, 0.9), sr),
            SliceLabel::Kick
        );
        assert_eq!(
            label_slice(&noise((0.04 * sr) as usize, 0.8, 7), sr),
            SliceLabel::Hat
        );
        let mut snare = sine(sr, 140.0, 0.16, 0.85);
        let burst = noise(snare.len(), 0.55, 11);
        for (sample, noise) in snare.iter_mut().zip(burst) {
            *sample += noise;
        }
        assert_eq!(label_slice(&snare, sr), SliceLabel::Snare);
        assert_eq!(
            label_slice(&sine(sr, 440.0, 0.8, 0.5), sr),
            SliceLabel::Melodic
        );
        assert_eq!(
            label_slice(&vec![0.0; 1_000], sr),
            SliceLabel::Melodic,
            "silence is not a kick"
        );
    }

    #[test]
    fn labels_are_stable() {
        let sr = 16_000.0;
        let kick = sine(sr, 70.0, 0.12, 0.9);
        assert_eq!(label_slice(&kick, sr), label_slice(&kick, sr));
    }
}
