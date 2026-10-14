//! Deterministic one-shots so the sampler is playable with an empty sample folder.

use crate::buffer::AudioBuffer;
use crate::dsp::TAU;

pub(crate) const NAMES: &[&str] = &[
    "kick", "snare", "hat", "hat_open", "tom", "tom_hi", "bass", "perc", "chord", "pluck", "zap",
    "rim", "sine", "impulse",
];

pub(crate) fn render(name: &str, sample_rate: f32) -> Option<AudioBuffer> {
    if !(sample_rate.is_finite() && sample_rate >= 1_000.0) {
        return None;
    }
    let data = match name {
        "kick" => kick(sample_rate),
        "snare" => snare(sample_rate),
        "hat" => hat(sample_rate, 0.045),
        "hat_open" => hat(sample_rate, 0.28),
        "tom" => tom(sample_rate, 90.0),
        "tom_hi" => tom(sample_rate, 150.0),
        "bass" => bass(sample_rate),
        "perc" => perc(sample_rate),
        "chord" => chord(sample_rate),
        "pluck" => karplus(sample_rate, 196.0, 0.7),
        "zap" => karplus(sample_rate, 880.0, 0.22),
        "rim" => rim(sample_rate),
        "sine" => sine(sample_rate, 440.0, 1.0, 0.35),
        "impulse" => impulse(sample_rate),
        _ => return None,
    };
    Some(AudioBuffer {
        sample_rate,
        channels: 2,
        data,
    })
}

fn stereo(mono: &[f32], width: f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(mono.len() * 2);
    let mut prev = 0.0;
    for (i, sample) in mono.iter().enumerate() {
        let side = if i % 2 == 0 { width } else { -width };
        let right = (sample * (1.0 - width) + prev * side).clamp(-1.0, 1.0);
        out.push(sample.clamp(-1.0, 1.0));
        out.push(right);
        prev = *sample;
    }
    out
}

struct Noise(u32);

impl Noise {
    fn bipolar(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let value = (self.0 >> 8) & 0x00FF_FFFF;
        value as f32 / 16_777_215.0 * 2.0 - 1.0
    }
}

fn kick(sr: f32) -> Vec<f32> {
    let n = (sr * 0.42) as usize;
    let mut phase = 0.0;
    let mut mono = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr;
        let env = (-t / 0.18).exp();
        let freq = 46.0 + 170.0 * (-t / 0.04).exp();
        phase = (phase + freq / sr) % 1.0;
        let body = (phase * TAU).sin();
        let click = if t < 0.0025 {
            (t / 0.0025 * TAU * 0.5).sin()
        } else {
            0.0
        };
        mono.push(body * env * 0.92 + click * 0.3);
    }
    stereo(&mono, 0.02)
}

fn snare(sr: f32) -> Vec<f32> {
    let n = (sr * 0.28) as usize;
    let mut noise = Noise(0x51A4_E001);
    let mut phase = 0.0;
    let mut mono = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr;
        phase = (phase + 186.0 / sr) % 1.0;
        let tone = (phase * TAU).sin() * (-t / 0.08).exp();
        let hiss = noise.bipolar() * (-t / 0.11).exp();
        let snap = noise.bipolar() * (-t / 0.02).exp();
        mono.push(tone * 0.32 + hiss * 0.45 + snap * 0.28);
    }
    stereo(&mono, 0.18)
}

fn hat(sr: f32, decay: f32) -> Vec<f32> {
    let n = (sr * (decay * 1.6 + 0.02)).clamp(sr * 0.05, sr * 0.5) as usize;
    let mut noise = Noise(0x0A7_0001);
    let mut prev = 0.0;
    let mut mono = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr;
        let white = noise.bipolar();
        let hp = white - prev;
        prev = white;
        mono.push(hp * (-t / decay).exp() * 0.55);
    }
    stereo(&mono, 0.35)
}

fn tom(sr: f32, freq: f32) -> Vec<f32> {
    let n = (sr * 0.4) as usize;
    let mut phase = 0.0;
    let mut mono = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr;
        let f = freq * (1.0 + 0.7 * (-t / 0.05).exp());
        phase = (phase + f / sr) % 1.0;
        mono.push((phase * TAU).sin() * (-t / 0.16).exp() * 0.85);
    }
    stereo(&mono, 0.06)
}

fn bass(sr: f32) -> Vec<f32> {
    let n = (sr * 0.55) as usize;
    let mut phase = 0.0;
    let mut mono = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr;
        let f = 55.0 * (1.0 + 0.4 * (-t / 0.08).exp());
        phase = (phase + f / sr) % 1.0;
        let s = (phase * TAU).sin();
        let body = (s + 0.15 * (phase * 2.0 * TAU).sin()) * (-t / 0.28).exp();
        mono.push(body * 0.8);
    }
    stereo(&mono, 0.03)
}

fn perc(sr: f32) -> Vec<f32> {
    let n = (sr * 0.16) as usize;
    let mut phase = 0.0;
    let mut noise = Noise(0x0B0B);
    let mut mono = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr;
        phase = (phase + 420.0 / sr) % 1.0;
        let tone = (phase * TAU).sin() * (-t / 0.045).exp();
        let click = noise.bipolar() * (-t / 0.012).exp();
        mono.push(tone * 0.55 + click * 0.3);
    }
    stereo(&mono, 0.2)
}

fn chord(sr: f32) -> Vec<f32> {
    let n = (sr * 0.9) as usize;
    let freqs = [261.63_f32, 329.63, 392.0];
    let mut phase = [0.0_f32; 3];
    let mut mono = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr;
        let mut s = 0.0;
        for (p, f) in phase.iter_mut().zip(freqs) {
            *p = (*p + f / sr) % 1.0;
            s += (*p * TAU).sin();
        }
        mono.push(s / 3.0 * (-t / 0.45).exp() * 0.7);
    }
    stereo(&mono, 0.12)
}

fn karplus(sr: f32, freq: f32, seconds: f32) -> Vec<f32> {
    let n = (sr * seconds) as usize;
    let len = (sr / freq).round().max(2.0) as usize;
    let mut noise = Noise(freq.to_bits() ^ 0xA5A5_1234);
    let mut buf: Vec<f32> = (0..len).map(|_| noise.bipolar() * 0.6).collect();
    let mut mono = Vec::with_capacity(n);
    for i in 0..n {
        let s = buf[i % len];
        let nxt = buf[(i + 1) % len];
        buf[i % len] = (s + nxt) * 0.5 * 0.996;
        let env = 1.0 - (i as f32 / n as f32).powi(2) * 0.15;
        mono.push(s * env);
    }
    stereo(&mono, 0.15)
}

fn rim(sr: f32) -> Vec<f32> {
    let n = (sr * 0.08) as usize;
    let mut phase = 0.0;
    let mut noise = Noise(0x51_51);
    let mut mono = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr;
        phase = (phase + 1_850.0 / sr) % 1.0;
        let square = if (phase * TAU).sin() >= 0.0 {
            1.0
        } else {
            -1.0
        };
        mono.push((square * 0.35 + noise.bipolar() * 0.2) * (-t / 0.018).exp());
    }
    stereo(&mono, 0.25)
}

fn sine(sr: f32, freq: f32, seconds: f32, amp: f32) -> Vec<f32> {
    let n = (sr * seconds) as usize;
    let mut phase = 0.0;
    let mut mono = Vec::with_capacity(n);
    for _ in 0..n {
        phase = (phase + freq / sr) % 1.0;
        mono.push((phase * TAU).sin() * amp);
    }
    stereo(&mono, 0.0)
}

fn impulse(sr: f32) -> Vec<f32> {
    let n = (sr * 0.5) as usize;
    let mut mono = vec![0.0; n.max(8)];
    mono[0] = 1.0;
    stereo(&mono, 0.0)
}
