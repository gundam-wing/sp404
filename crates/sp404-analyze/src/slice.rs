//! Slice boundaries from snapped onset times.
//!
//! This stage only describes slices. It does not write chopped audio. Each
//! slice carries the fade a later chop can apply at its edges.

use serde::Serialize;

/// One region between snapped onsets.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Slice {
    /// Start time in seconds.
    pub start: f64,
    /// End time in seconds.
    pub end: f64,
    /// Fade-in length in seconds, already clamped to the slice.
    pub fade_in: f64,
    /// Fade-out length in seconds, already clamped to the slice.
    pub fade_out: f64,
}

/// Build slices from sorted onset times.
///
/// The first slice starts at the first onset. Each later onset closes the
/// previous slice and opens the next. The last slice runs to `duration`.
/// With no onsets, the whole buffer is one slice. Fades are the requested
/// length or a quarter of the slice, whichever is shorter, so the two fades
/// leave a sustain region.
pub fn from_onsets(onset_times: &[f64], duration: f64, fade_seconds: f64) -> Vec<Slice> {
    if !(duration.is_finite() && duration > 0.0) {
        return Vec::new();
    }
    let fade_seconds = if fade_seconds.is_finite() && fade_seconds > 0.0 {
        fade_seconds
    } else {
        0.0
    };
    let mut starts: Vec<f64> = onset_times
        .iter()
        .copied()
        .filter(|time| time.is_finite() && *time >= 0.0 && *time < duration)
        .collect();
    if starts.is_empty() {
        starts.push(0.0);
    }
    let mut slices = Vec::with_capacity(starts.len());
    for i in 0..starts.len() {
        let start = starts[i];
        let end = starts.get(i + 1).copied().unwrap_or(duration);
        if end <= start {
            continue;
        }
        let fade = fade_seconds.min((end - start) * 0.25);
        slices.push(Slice {
            start,
            end,
            fade_in: fade,
            fade_out: fade,
        });
    }
    slices
}
