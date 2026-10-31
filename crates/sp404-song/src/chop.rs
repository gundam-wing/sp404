//! Turn analyzed boundaries into faded slice audio.
//!
//! This module does not open files. The CLI writes the WAVs and the manifest.

use sp404_core::AudioBuffer;

use crate::error::{err, Error};
use crate::label::{label_slice, SliceLabel};

/// How `sp404 chop` places slice boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChopMode {
    /// One slice per analysis slice (the snapped onsets).
    Transient,
    /// One slice between consecutive beat times.
    BeatGrid,
    /// Slices of 1, 2, or 4 bars, aligned to the beat grid.
    Phrase {
        /// 1, 2, or 4.
        bars: u32,
    },
}

impl ChopMode {
    /// Manifest spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transient => "transient",
            Self::BeatGrid => "beat-grid",
            Self::Phrase { .. } => "phrase",
        }
    }

    /// Phrase length in bars, if this mode has one.
    pub fn phrase_bars(self) -> Option<u32> {
        match self {
            Self::Phrase { bars } => Some(bars),
            _ => None,
        }
    }

    /// `transient`, `beat-grid`, or `phrase`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "transient" => Some(Self::Transient),
            "beat-grid" => Some(Self::BeatGrid),
            "phrase" => Some(Self::Phrase { bars: 1 }),
            _ => None,
        }
    }
}

/// A boundary from analysis. Times are seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliceMark {
    /// Start in the source.
    pub start: f64,
    /// End in the source.
    pub end: f64,
    /// Fade-in recorded by analysis.
    pub fade_in: f64,
    /// Fade-out recorded by analysis.
    pub fade_out: f64,
}

/// What chop needs from an analysis document. The CLI fills this in.
#[derive(Debug, Clone, Copy)]
pub struct ChopSource<'a> {
    /// Mono samples.
    pub samples: &'a [f32],
    /// Sample rate in hertz.
    pub sample_rate: f32,
    /// Buffer length in seconds.
    pub duration: f64,
    /// Analyzed tempo. `0` if none.
    pub bpm: f64,
    /// Beat times in seconds.
    pub beat_times: &'a [f64],
    /// Onset slices from analysis. Used by [`ChopMode::Transient`].
    pub slices: &'a [SliceMark],
    /// Key name.
    pub key: &'a str,
    /// Key confidence.
    pub key_confidence: f64,
}

/// One faded slice, still in memory.
#[derive(Debug, Clone, PartialEq)]
pub struct ChoppedSlice {
    /// Index in the chop, starting at 0.
    pub index: usize,
    /// Heuristic label.
    pub label: SliceLabel,
    /// Start in the source, seconds.
    pub start: f64,
    /// End in the source, seconds.
    pub end: f64,
    /// Fade-in that was applied, seconds.
    pub fade_in: f64,
    /// Fade-out that was applied, seconds.
    pub fade_out: f64,
    /// Mono audio after the fades.
    pub audio: AudioBuffer,
}

/// The result of [`chop`].
#[derive(Debug, Clone, PartialEq)]
pub struct Chopped {
    /// Copied from the source.
    pub sample_rate: f64,
    /// Copied from the source.
    pub duration: f64,
    /// Copied from the source.
    pub bpm: f64,
    /// Copied from the source.
    pub key: String,
    /// Copied from the source.
    pub key_confidence: f64,
    /// Mode that produced `slices`.
    pub mode: ChopMode,
    /// Faded slices in time order.
    pub slices: Vec<ChoppedSlice>,
}

/// Slice `source` and label each piece.
pub fn chop(source: ChopSource<'_>, mode: ChopMode) -> Result<Chopped, Error> {
    if source.samples.is_empty() {
        return Err(err("audio is empty"));
    }
    if !(source.sample_rate.is_finite() && source.sample_rate >= 1_000.0) {
        return Err(err("sample rate must be finite and at least 1000 Hz"));
    }
    if !(source.duration.is_finite() && source.duration > 0.0) {
        return Err(err("duration must be positive"));
    }
    if let ChopMode::Phrase { bars } = mode {
        if !matches!(bars, 1 | 2 | 4) {
            return Err(err("phrase bars must be 1, 2, or 4"));
        }
    }
    let fade = default_fade(source.slices);
    let marks = match mode {
        ChopMode::Transient => transient_marks(source.slices, source.duration, fade),
        ChopMode::BeatGrid => beat_marks(source.beat_times, source.duration, fade)?,
        ChopMode::Phrase { bars } => {
            phrase_marks(source.bpm, source.beat_times, source.duration, bars, fade)?
        }
    };
    let mut slices = Vec::new();
    for mark in marks {
        let raw = extract(source.samples, source.sample_rate, mark.start, mark.end);
        if raw.is_empty() {
            continue;
        }
        let label = label_slice(&raw, source.sample_rate);
        let mut faded = raw;
        apply_fades(&mut faded, source.sample_rate, mark.fade_in, mark.fade_out);
        let audio = AudioBuffer::new(source.sample_rate, 1, faded)
            .map_err(|audio_err| err(audio_err.to_string()))?;
        slices.push(ChoppedSlice {
            index: slices.len(),
            label,
            start: mark.start,
            end: mark.end,
            fade_in: mark.fade_in,
            fade_out: mark.fade_out,
            audio,
        });
    }
    if slices.is_empty() {
        return Err(err("chop produced no slices"));
    }
    Ok(Chopped {
        sample_rate: f64::from(source.sample_rate),
        duration: source.duration,
        bpm: source.bpm,
        key: source.key.to_string(),
        key_confidence: source.key_confidence,
        mode,
        slices,
    })
}

fn default_fade(slices: &[SliceMark]) -> f64 {
    let mut fades: Vec<f64> = slices
        .iter()
        .map(|slice| slice.fade_in)
        .filter(|fade| fade.is_finite() && *fade > 0.0)
        .collect();
    if fades.is_empty() {
        return 0.005;
    }
    fades.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    fades[fades.len() / 2]
}

fn clamp_fade(fade: f64, length: f64) -> f64 {
    if fade.is_finite() && fade > 0.0 && length > 0.0 {
        fade.min(length * 0.25)
    } else {
        0.0
    }
}

fn transient_marks(slices: &[SliceMark], duration: f64, fade: f64) -> Vec<SliceMark> {
    let marks: Vec<SliceMark> = slices
        .iter()
        .copied()
        .filter(|slice| slice.end > slice.start && slice.start < duration)
        .map(|mut slice| {
            slice.end = slice.end.min(duration);
            slice.fade_in = clamp_fade(slice.fade_in, slice.end - slice.start);
            slice.fade_out = clamp_fade(slice.fade_out, slice.end - slice.start);
            slice
        })
        .collect();
    if marks.is_empty() {
        let fade = clamp_fade(fade, duration);
        vec![SliceMark {
            start: 0.0,
            end: duration,
            fade_in: fade,
            fade_out: fade,
        }]
    } else {
        marks
    }
}

fn beat_marks(beat_times: &[f64], duration: f64, fade: f64) -> Result<Vec<SliceMark>, Error> {
    let beats = clean_beats(beat_times, duration);
    if beats.is_empty() {
        return Err(err("beat-grid needs at least one beat time"));
    }
    let mut marks = Vec::new();
    for (index, start) in beats.iter().copied().enumerate() {
        let end = beats.get(index + 1).copied().unwrap_or(duration);
        if end <= start {
            continue;
        }
        let fade = clamp_fade(fade, end - start);
        marks.push(SliceMark {
            start,
            end,
            fade_in: fade,
            fade_out: fade,
        });
    }
    if marks.is_empty() {
        return Err(err("beat-grid produced no slices"));
    }
    Ok(marks)
}

fn phrase_marks(
    bpm: f64,
    beat_times: &[f64],
    duration: f64,
    bars: u32,
    fade: f64,
) -> Result<Vec<SliceMark>, Error> {
    let beat = if bpm.is_finite() && bpm > 0.0 {
        60.0 / bpm
    } else {
        median_ioi(beat_times)
            .ok_or_else(|| err("phrase chop needs a positive bpm or at least two beat times"))?
    };
    if !beat.is_finite() || beat <= 0.0 {
        return Err(err("phrase chop could not find a beat length"));
    }
    let phrase = beat * 4.0 * f64::from(bars);
    let origin = clean_beats(beat_times, duration)
        .first()
        .copied()
        .unwrap_or(0.0);
    let mut marks = Vec::new();
    let mut start = origin;
    while start < duration - 1.0e-4 {
        let end = (start + phrase).min(duration);
        if end - start > 1.0e-3 {
            let fade = clamp_fade(fade, end - start);
            marks.push(SliceMark {
                start,
                end,
                fade_in: fade,
                fade_out: fade,
            });
        }
        if end >= duration - 1.0e-9 {
            break;
        }
        start += phrase;
    }
    if marks.is_empty() {
        return Err(err("phrase chop produced no slices"));
    }
    Ok(marks)
}

fn clean_beats(beat_times: &[f64], duration: f64) -> Vec<f64> {
    let mut beats: Vec<f64> = beat_times
        .iter()
        .copied()
        .filter(|time| time.is_finite() && *time >= 0.0 && *time < duration - 1.0e-4)
        .collect();
    beats.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    beats.dedup_by(|a, b| (*a - *b).abs() < 1.0e-4);
    beats
}

fn median_ioi(times: &[f64]) -> Option<f64> {
    if times.len() < 2 {
        return None;
    }
    let mut diffs: Vec<f64> = times
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .filter(|diff| diff.is_finite() && *diff > 1.0e-4)
        .collect();
    if diffs.is_empty() {
        return None;
    }
    diffs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(diffs[diffs.len() / 2])
}

fn extract(mono: &[f32], sample_rate: f32, start: f64, end: f64) -> Vec<f32> {
    let n = mono.len() as isize;
    let i0 = ((start * f64::from(sample_rate)).round() as isize).clamp(0, n) as usize;
    let i1 = ((end * f64::from(sample_rate)).round() as isize).clamp(0, n) as usize;
    if i1 <= i0 {
        return Vec::new();
    }
    mono[i0..i1]
        .iter()
        .map(|sample| if sample.is_finite() { *sample } else { 0.0 })
        .collect()
}

fn apply_fades(samples: &mut [f32], sample_rate: f32, fade_in: f64, fade_out: f64) {
    let n = samples.len();
    if n == 0 {
        return;
    }
    let rate = f64::from(sample_rate);
    let fade_in_n = (fade_in * rate).max(0.0);
    let fade_out_n = (fade_out * rate).max(0.0);
    for (index, sample) in samples.iter_mut().enumerate() {
        let mut gain = 1.0f32;
        if fade_in_n > 0.0 {
            let ramp = (index as f64 / fade_in_n) as f32;
            if ramp < 1.0 {
                gain *= ramp.clamp(0.0, 1.0);
            }
        }
        if fade_out_n > 0.0 {
            let remaining = (n - 1 - index) as f64;
            let ramp = (remaining / fade_out_n) as f32;
            if ramp < 1.0 {
                gain *= ramp.clamp(0.0, 1.0);
            }
        }
        *sample *= gain;
    }
}

#[cfg(test)]
mod tests {
    use super::{chop, ChopMode, ChopSource, SliceMark};

    fn source<'a>(
        samples: &'a [f32],
        beats: &'a [f64],
        slices: &'a [SliceMark],
        duration: f64,
    ) -> ChopSource<'a> {
        ChopSource {
            samples,
            sample_rate: 8_000.0,
            duration,
            bpm: 120.0,
            beat_times: beats,
            slices,
            key: "C major",
            key_confidence: 0.5,
        }
    }

    #[test]
    fn transient_uses_onset_fades() {
        let samples = vec![0.8f32; 8_000];
        let slices = [
            SliceMark {
                start: 0.0,
                end: 0.5,
                fade_in: 0.01,
                fade_out: 0.01,
            },
            SliceMark {
                start: 0.5,
                end: 1.0,
                fade_in: 0.01,
                fade_out: 0.01,
            },
        ];
        let chopped = chop(source(&samples, &[], &slices, 1.0), ChopMode::Transient).unwrap();
        assert_eq!(chopped.slices.len(), 2);
        let audio = &chopped.slices[0].audio.data;
        assert!(audio[0].abs() < 1.0e-4, "fade-in left {}", audio[0]);
        assert!(audio[audio.len() / 2].abs() > 0.5);
        assert!(
            audio.last().unwrap().abs() < 1.0e-3,
            "fade-out left {}",
            audio.last().unwrap()
        );
        assert!((chopped.slices[0].fade_in - 0.01).abs() < 1.0e-9);
    }

    #[test]
    fn beat_grid_follows_beat_times() {
        let samples = vec![0.2f32; 16_000];
        let beats = [0.5, 1.0, 1.5];
        let chopped = chop(source(&samples, &beats, &[], 2.0), ChopMode::BeatGrid).unwrap();
        let starts: Vec<f64> = chopped.slices.iter().map(|slice| slice.start).collect();
        assert_eq!(starts, vec![0.5, 1.0, 1.5]);
        assert!((chopped.slices.last().unwrap().end - 2.0).abs() < 1.0e-9);
    }

    #[test]
    fn beat_grid_without_beats_fails() {
        let samples = vec![0.2f32; 800];
        let failed = chop(source(&samples, &[], &[], 0.1), ChopMode::BeatGrid);
        assert!(failed.is_err());
    }

    #[test]
    fn phrase_lengths_follow_bars() {
        let samples = vec![0.2f32; 64_000];
        let beats: Vec<f64> = (0..16).map(|i| i as f64 * 0.5).collect();
        for bars in [1, 2, 4] {
            let chopped = chop(
                source(&samples, &beats, &[], 8.0),
                ChopMode::Phrase { bars },
            )
            .unwrap();
            let expected = 2.0 * f64::from(bars);
            assert!(
                (chopped.slices[0].end - chopped.slices[0].start - expected).abs() < 1.0e-6,
                "bars {bars} span {}",
                chopped.slices[0].end - chopped.slices[0].start
            );
            assert_eq!(chopped.slices.len(), (8.0 / expected) as usize);
        }
    }
}
