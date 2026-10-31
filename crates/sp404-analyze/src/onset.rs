//! Spectral-flux onsets snapped to a nearby zero crossing.

/// Sample index of a detected onset after zero-crossing snap.
#[derive(Debug, Clone, Copy)]
struct Mark {
    sample: usize,
    strength: f32,
}

/// Flux peaks, refined to the transient inside the frame, then snapped.
pub fn detect(
    mono: &[f32],
    flux: &[f32],
    sample_rate: f32,
    hop: usize,
    fft_size: usize,
    zero_cross_samples: usize,
) -> Vec<usize> {
    if mono.is_empty() || flux.is_empty() || hop == 0 {
        return Vec::new();
    }
    let frame_rate = f64::from(sample_rate) / hop as f64;
    let min_gap = ((0.05 * f64::from(sample_rate)).round() as usize).max(1);
    let mut marks = Vec::new();
    for frame in pick_flux_peaks(flux, frame_rate) {
        let refined = refine_transient(mono, frame, hop, fft_size);
        let sample = snap_to_zero_crossing(mono, refined, zero_cross_samples);
        marks.push(Mark {
            sample,
            strength: flux[frame],
        });
    }
    dedup(marks, min_gap)
}

/// Move `onset` to the closest zero crossing inside `window` samples.
///
/// Crossings at or before the onset win over later ones at the same distance,
/// so the snap does not step past the attack. The returned index is the
/// quieter sample of the crossing pair. With a zero window, or when no
/// crossing exists, `onset` is returned unchanged.
pub fn snap_to_zero_crossing(mono: &[f32], onset: usize, window: usize) -> usize {
    if mono.len() < 2 || window == 0 {
        return onset.min(mono.len().saturating_sub(1));
    }
    let onset = onset.min(mono.len() - 1);
    let lo = onset.saturating_sub(window);
    let hi = (onset + window).min(mono.len() - 1);
    let mut best_before: Option<(usize, usize)> = None;
    let mut best_after: Option<(usize, usize)> = None;
    for i in lo..hi {
        let left = mono[i];
        let right = mono[i + 1];
        if !left.is_finite() || !right.is_finite() || left * right > 0.0 {
            continue;
        }
        let crossing = if left.abs() <= right.abs() { i } else { i + 1 };
        let dist = crossing.abs_diff(onset);
        if crossing <= onset {
            if best_before.map(|(best, _)| dist < best).unwrap_or(true) {
                best_before = Some((dist, crossing));
            }
        } else if best_after.map(|(best, _)| dist < best).unwrap_or(true) {
            best_after = Some((dist, crossing));
        }
    }
    best_before
        .or(best_after)
        .map(|(_, sample)| sample)
        .unwrap_or(onset)
}

fn pick_flux_peaks(flux: &[f32], frame_rate: f64) -> Vec<usize> {
    if flux.len() < 2 {
        return Vec::new();
    }
    let max = flux.iter().copied().fold(0.0f32, f32::max);
    if max <= 1e-8 {
        return Vec::new();
    }
    let thresh = max * 0.2;
    let min_gap = ((0.05 * frame_rate).round() as usize).max(1);
    let mut peaks = Vec::new();
    let consider = |peaks: &mut Vec<usize>, i: usize| {
        if flux[i] < thresh {
            return;
        }
        if let Some(&last) = peaks.last() {
            if i.saturating_sub(last) < min_gap {
                if flux[i] > flux[last] {
                    peaks.pop();
                    peaks.push(i);
                }
                return;
            }
        }
        peaks.push(i);
    };
    if flux[0] >= flux[1] {
        consider(&mut peaks, 0);
    }
    for i in 1..flux.len() - 1 {
        if flux[i] >= flux[i - 1] && flux[i] >= flux[i + 1] {
            consider(&mut peaks, i);
        }
    }
    let last = flux.len() - 1;
    if flux[last] >= flux[last - 1] {
        consider(&mut peaks, last);
    }
    peaks
}

/// Loudest sample in the peak frame and its neighbors.
///
/// Flux is quantized to a hop. The transient that caused the peak sits inside
/// that window; taking the peak absolute sample puts the marker on the click
/// before the zero-crossing snap.
fn refine_transient(mono: &[f32], frame: usize, hop: usize, fft_size: usize) -> usize {
    let start = frame.saturating_sub(1).saturating_mul(hop);
    if start >= mono.len() {
        return mono.len().saturating_sub(1);
    }
    let end = frame
        .saturating_add(1)
        .saturating_mul(hop)
        .saturating_add(fft_size)
        .min(mono.len());
    let mut best = start;
    let mut best_abs = -1.0f32;
    for (offset, sample) in mono[start..end].iter().enumerate() {
        let abs = sample.abs();
        if abs > best_abs {
            best_abs = abs;
            best = start + offset;
        }
    }
    best
}

fn dedup(marks: Vec<Mark>, min_gap: usize) -> Vec<usize> {
    let mut marks = marks;
    marks.sort_by_key(|mark| mark.sample);
    let mut kept: Vec<Mark> = Vec::new();
    for mark in marks {
        if let Some(prev) = kept.last_mut() {
            if mark.sample.saturating_sub(prev.sample) < min_gap {
                if mark.strength > prev.strength {
                    *prev = mark;
                }
                continue;
            }
        }
        kept.push(mark);
    }
    kept.into_iter().map(|mark| mark.sample).collect()
}

#[cfg(test)]
mod tests {
    use super::snap_to_zero_crossing;

    #[test]
    fn snap_moves_a_sine_peak_toward_a_zero_crossing() {
        let sr = 8_000.0f32;
        let freq = 100.0f32;
        let mono: Vec<f32> = (0..800)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin())
            .collect();
        // sin(2π * 100 * 100 / 8000) = sin(π/2) = 1, a positive peak.
        let onset = 100usize;
        assert!(mono[onset] > 0.9, "peak {}", mono[onset]);
        let snapped = snap_to_zero_crossing(&mono, onset, 25);
        assert!(snapped < onset, "snap should prefer the earlier crossing");
        assert!(onset - snapped <= 25);
        assert!(
            mono[snapped].abs() < 0.05,
            "snapped sample {} value {}",
            snapped,
            mono[snapped]
        );
        assert!(mono[snapped].abs() < mono[onset].abs());
    }
}
