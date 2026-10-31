//! Onset-strength autocorrelation, an octave guard, and Ellis beat tracking.
//!
//! Tempo follows Dan Ellis, "Beat Tracking by Dynamic Programming", Journal
//! of New Music Research, 2007: autocorrelation of the onset envelope, then a
//! dynamic program whose transitions prefer the detected period. The guard
//! checks the half-time and double-time candidates against the median spacing
//! of the snapped onsets so a regular click track is not reported an octave
//! away.

/// Tempo in BPM. Zero means the envelope had no usable period.
pub fn estimate_bpm(
    flux: &[f32],
    frame_rate: f64,
    onset_times: &[f64],
    min_bpm: f64,
    max_bpm: f64,
) -> f64 {
    let candidate = autocorr_bpm(flux, frame_rate, min_bpm, max_bpm);
    if candidate <= 0.0 {
        return bpm_from_onsets(onset_times)
            .filter(|bpm| *bpm >= min_bpm && *bpm <= max_bpm)
            .unwrap_or(0.0);
    }
    guard_half_double(candidate, onset_times, min_bpm, max_bpm)
}

/// Choose among `bpm`, `bpm / 2`, and `bpm * 2`.
///
/// The winner is the candidate whose period is closest, in log distance, to
/// the median inter-onset interval. Candidates outside `[min_bpm, max_bpm]`
/// are dropped. A tie keeps the autocorrelation candidate.
pub fn guard_half_double(
    candidate_bpm: f64,
    onset_times: &[f64],
    min_bpm: f64,
    max_bpm: f64,
) -> f64 {
    if !(candidate_bpm.is_finite() && candidate_bpm > 0.0) {
        return 0.0;
    }
    let Some(ioi) = median_ioi(onset_times) else {
        return clamp_bpm(candidate_bpm, min_bpm, max_bpm);
    };
    let mut best = clamp_bpm(candidate_bpm, min_bpm, max_bpm);
    let mut best_dist = f64::MAX;
    for (scale, bias) in [(0.5, 0.0), (1.0, -1.0e-6), (2.0, 0.0)] {
        let bpm = candidate_bpm * scale;
        if bpm < min_bpm || bpm > max_bpm {
            continue;
        }
        let period = 60.0 / bpm;
        let dist = (period / ioi).ln().abs() + bias;
        if dist < best_dist {
            best_dist = dist;
            best = bpm;
        }
    }
    best
}

/// Ellis dynamic program. Times are seconds from the start of the buffer.
pub fn track_beats(
    flux: &[f32],
    bpm: f64,
    sample_rate: f64,
    hop: usize,
    fft_size: usize,
    duration: f64,
) -> Vec<f64> {
    if flux.is_empty() || hop == 0 || !(bpm.is_finite() && bpm > 0.0) || sample_rate <= 0.0 {
        return Vec::new();
    }
    let frame_rate = sample_rate / hop as f64;
    let period = (60.0 * frame_rate / bpm).round().max(1.0) as usize;
    let max_env = flux.iter().copied().fold(0.0f32, f32::max);
    if max_env <= 1e-8 {
        return Vec::new();
    }
    let local: Vec<f64> = flux
        .iter()
        .map(|value| f64::from(*value) / f64::from(max_env))
        .collect();
    let n = local.len();
    let tightness = 100.0;
    let min_off = (period / 2).max(1);
    let max_off = (period.saturating_mul(2)).max(min_off + 1);
    let mut cum = vec![0.0f64; n];
    let mut back = vec![0usize; n];
    for i in 0..n {
        if i < min_off {
            cum[i] = local[i];
            back[i] = i;
            continue;
        }
        let max_look = i.min(max_off);
        let mut best_score = f64::NEG_INFINITY;
        let mut best_j = i;
        for off in min_off..=max_look {
            let j = i - off;
            let ratio = off as f64 / period as f64;
            let log_period = ratio.ln();
            let score = cum[j] - tightness * log_period * log_period;
            if score > best_score {
                best_score = score;
                best_j = j;
            }
        }
        cum[i] = local[i] + best_score;
        back[i] = best_j;
    }

    let search_from = n.saturating_sub(period);
    let mut end = search_from;
    let mut best = f64::NEG_INFINITY;
    for (i, score) in cum.iter().enumerate().skip(search_from) {
        if *score > best {
            best = *score;
            end = i;
        }
    }
    let mut frames = vec![end];
    let mut steps = 0usize;
    while back[frames[frames.len() - 1]] < frames[frames.len() - 1] {
        frames.push(back[*frames.last().unwrap()]);
        steps += 1;
        if steps > n {
            break;
        }
    }
    frames.reverse();

    let mut times = Vec::with_capacity(frames.len());
    for frame in frames {
        let center = frame as f64 * hop as f64 + fft_size as f64 * 0.5;
        let time = (center / sample_rate).clamp(0.0, duration.max(0.0));
        if times
            .last()
            .map(|previous| time > *previous + 1.0e-4)
            .unwrap_or(true)
        {
            times.push(time);
        }
    }
    times
}

fn autocorr_bpm(flux: &[f32], frame_rate: f64, min_bpm: f64, max_bpm: f64) -> f64 {
    if flux.len() < 4 || frame_rate <= 0.0 || min_bpm <= 0.0 || max_bpm < min_bpm {
        return 0.0;
    }
    let min_lag = (60.0 * frame_rate / max_bpm).round().max(1.0) as usize;
    let mut max_lag = (60.0 * frame_rate / min_bpm).round().max(min_lag as f64) as usize;
    max_lag = max_lag.min(flux.len() / 2);
    if max_lag < min_lag {
        return 0.0;
    }
    let ac = autocorrelate(flux, max_lag);
    if ac[0] <= 1e-12 {
        return 0.0;
    }
    let center = if min_bpm <= 120.0 && max_bpm >= 120.0 {
        120.0
    } else {
        (min_bpm * max_bpm).sqrt()
    };
    let mut best_lag = min_lag;
    let mut best = f64::NEG_INFINITY;
    for lag in min_lag..=max_lag {
        let bpm = 60.0 * frame_rate / lag as f64;
        let octaves = (bpm / center).log2();
        let prior = (-0.5 * octaves * octaves).exp();
        let score = f64::from(ac[lag] / ac[0]) * prior;
        if score > best {
            best = score;
            best_lag = lag;
        }
    }
    if !best.is_finite() {
        return 0.0;
    }
    60.0 * frame_rate / best_lag as f64
}

fn autocorrelate(env: &[f32], max_lag: usize) -> Vec<f32> {
    let n = env.len();
    let max_lag = max_lag.min(n.saturating_sub(1));
    let mean = env.iter().sum::<f32>() / n as f32;
    let centered: Vec<f32> = env.iter().map(|value| value - mean).collect();
    let mut ac = vec![0.0f32; max_lag + 1];
    for lag in 0..=max_lag {
        let mut sum = 0.0f32;
        let count = n - lag;
        for i in 0..count {
            sum += centered[i] * centered[i + lag];
        }
        ac[lag] = sum / count as f32;
    }
    ac
}

fn bpm_from_onsets(onset_times: &[f64]) -> Option<f64> {
    median_ioi(onset_times).map(|ioi| 60.0 / ioi)
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

fn clamp_bpm(bpm: f64, min_bpm: f64, max_bpm: f64) -> f64 {
    if bpm < min_bpm {
        min_bpm
    } else if bpm > max_bpm {
        max_bpm
    } else {
        bpm
    }
}

#[cfg(test)]
mod tests {
    use super::guard_half_double;

    fn click_times() -> Vec<f64> {
        (0..8).map(|i| i as f64 * 0.5).collect()
    }

    #[test]
    fn half_double_guard_rejects_octave_errors_on_a_click_track() {
        let times = click_times();
        let from_half = guard_half_double(60.0, &times, 40.0, 280.0);
        let from_double = guard_half_double(240.0, &times, 40.0, 280.0);
        let from_true = guard_half_double(120.0, &times, 40.0, 280.0);
        assert!(
            (from_half - 120.0).abs() < 1.0,
            "half-time candidate stayed at {from_half}"
        );
        assert!(
            (from_double - 120.0).abs() < 1.0,
            "double-time candidate stayed at {from_double}"
        );
        assert!(
            (from_true - 120.0).abs() < 1.0,
            "true tempo moved to {from_true}"
        );
    }
}
