//! One Hann-windowed STFT shared by onset, tempo, and key.
//!
//! The same frames feed spectral flux and chroma so the document is a single
//! deterministic pass. `rustfft` is pure Rust; this crate is the only place
//! it is linked.

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

pub struct Spectrum {
    /// Half-wave spectral flux, one value per frame.
    pub flux: Vec<f32>,
    /// Summed peak-picked pitch-class energy.
    pub chroma: [f32; 12],
}

pub fn analyze(mono: &[f32], sample_rate: f32, fft_size: usize, hop: usize) -> Spectrum {
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(fft_size);
    let window = hann(fft_size);
    let bins = fft_size / 2 + 1;
    let mut buf = vec![Complex::new(0.0, 0.0); fft_size];
    let mut mag = vec![0.0f32; bins];
    let mut prev = vec![0.0f32; bins];
    let mut flux = Vec::new();
    let mut chroma = [0.0f32; 12];

    let mut frame = 0usize;
    loop {
        let start = frame.saturating_mul(hop);
        if start >= mono.len() {
            break;
        }
        for (i, bin) in buf.iter_mut().enumerate() {
            let sample = mono.get(start + i).copied().unwrap_or(0.0);
            let sample = if sample.is_finite() { sample } else { 0.0 };
            *bin = Complex::new(sample * window[i], 0.0);
        }
        fft.process(&mut buf);
        let mut frame_flux = 0.0f32;
        let mut frame_max = 0.0f32;
        for bin in 0..bins {
            let value = buf[bin].re.hypot(buf[bin].im);
            mag[bin] = value;
            frame_max = frame_max.max(value);
            let diff = value - prev[bin];
            if diff > 0.0 {
                frame_flux += diff;
            }
        }
        if frame_max > 0.0 {
            let floor = frame_max * 0.05;
            for bin in 1..bins.saturating_sub(1) {
                if mag[bin] >= floor && mag[bin] >= mag[bin - 1] && mag[bin] >= mag[bin + 1] {
                    add_chroma(&mut chroma, bin, fft_size, sample_rate, mag[bin]);
                }
            }
        }
        flux.push(frame_flux);
        prev.copy_from_slice(&mag);
        frame += 1;
    }

    Spectrum { flux, chroma }
}

fn hann(n: usize) -> Vec<f32> {
    if n <= 1 {
        return vec![1.0; n];
    }
    (0..n)
        .map(|i| {
            let phase = 2.0 * std::f32::consts::PI * (i as f32) / (n as f32 - 1.0);
            0.5 - 0.5 * phase.cos()
        })
        .collect()
}

/// Nearest-bin chroma. Bins outside the pitched range are ignored so rumble
/// and hiss do not vote in the key match.
fn add_chroma(chroma: &mut [f32; 12], bin: usize, fft_size: usize, sample_rate: f32, mag: f32) {
    let freq = bin as f64 * f64::from(sample_rate) / fft_size as f64;
    if !(65.0..=5_000.0).contains(&freq) {
        return;
    }
    let midi = 69.0 + 12.0 * (freq / 440.0).log2();
    if !midi.is_finite() {
        return;
    }
    let class = (midi.round() as i64).rem_euclid(12) as usize;
    chroma[class] += mag;
}
