//! Small DSP helpers shared by the built-in effects.
//!
//! Feedback state is flushed when it falls into the denormal range so long
//! decays cannot stall the CPU. Dry signal paths are left untouched, which
//! keeps bypass and mix-at-zero bit-exact.

pub(crate) const TAU: f32 = std::f32::consts::TAU;

#[inline]
pub(crate) fn sanitize(x: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

/// Flush subnormal state to zero. Normal samples pass through unchanged.
#[inline]
pub(crate) fn flush_denormal(x: f32) -> f32 {
    if !x.is_finite() || x.abs() < 1.0e-15 {
        0.0
    } else {
        x
    }
}

#[inline]
pub(crate) fn wet_dry(dry: f32, wet: f32, mix: f32) -> f32 {
    let mix = mix.clamp(0.0, 1.0);
    dry * (1.0 - mix) + wet * mix
}

#[inline]
pub(crate) fn db_to_lin(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

#[inline]
pub(crate) fn sane_sr(sample_rate: f32) -> f32 {
    if sample_rate.is_finite() && sample_rate >= 1_000.0 {
        sample_rate
    } else {
        44_100.0
    }
}

/// Linear interpolation of a circular buffer. `pos` may be negative.
#[inline]
pub(crate) fn read_frac(buf: &[f32], pos: f32) -> f32 {
    let len = buf.len();
    if len == 0 {
        return 0.0;
    }
    let len_f = len as f32;
    let mut p = pos % len_f;
    if p < 0.0 {
        p += len_f;
    }
    let i0 = p.floor() as usize % len;
    let frac = p - p.floor();
    let i1 = (i0 + 1) % len;
    buf[i0] * (1.0 - frac) + buf[i1] * frac
}

/// Trapezoidal state-variable filter (Zavalishin / Cytomic).
///
/// Returns `(low, band, high)`. The filter is stable at high resonance.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Svf {
    ic1eq: f32,
    ic2eq: f32,
}

impl Svf {
    pub(crate) fn new() -> Self {
        Self {
            ic1eq: 0.0,
            ic2eq: 0.0,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.ic1eq = 0.0;
        self.ic2eq = 0.0;
    }

    pub(crate) fn capture(&self) -> [f32; 2] {
        [self.ic1eq, self.ic2eq]
    }

    pub(crate) fn restore(&mut self, state: [f32; 2]) {
        self.ic1eq = state[0];
        self.ic2eq = state[1];
    }

    pub(crate) fn process(
        &mut self,
        input: f32,
        cutoff: f32,
        resonance: f32,
        sample_rate: f32,
    ) -> (f32, f32, f32) {
        let sample_rate = sane_sr(sample_rate);
        let cutoff = cutoff.clamp(10.0, sample_rate * 0.45);
        let q = 0.707 + resonance.clamp(0.0, 1.0).powi(2) * 18.0;
        let g = (std::f32::consts::PI * cutoff / sample_rate).tan();
        let k = 1.0 / q;
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;

        let v0 = sanitize(input);
        let v3 = v0 - self.ic2eq;
        let v1 = a1 * self.ic1eq + a2 * v3;
        let v2 = self.ic2eq + a2 * self.ic1eq + a3 * v3;
        self.ic1eq = flush_denormal(2.0 * v1 - self.ic1eq);
        self.ic2eq = flush_denormal(2.0 * v2 - self.ic2eq);

        let low = v2;
        let band = v1;
        let high = v0 - k * v1 - v2;
        (low, band, high)
    }
}

/// One-pole lowpass whose low and high outputs sum back to the input.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OnePole {
    z: f32,
}

impl OnePole {
    pub(crate) fn new() -> Self {
        Self { z: 0.0 }
    }

    pub(crate) fn reset(&mut self) {
        self.z = 0.0;
    }

    pub(crate) fn z(&self) -> f32 {
        self.z
    }

    pub(crate) fn set_z(&mut self, z: f32) {
        self.z = z;
    }

    pub(crate) fn split(&mut self, x: f32, cutoff: f32, sample_rate: f32) -> (f32, f32) {
        let sample_rate = sane_sr(sample_rate);
        let cutoff = cutoff.clamp(10.0, sample_rate * 0.45);
        let a = (-TAU * cutoff / sample_rate).exp();
        let x = sanitize(x);
        self.z = flush_denormal(a * self.z + (1.0 - a) * x);
        let low = self.z;
        let high = x - low;
        (low, high)
    }

    pub(crate) fn lowpass(&mut self, x: f32, cutoff: f32, sample_rate: f32) -> f32 {
        self.split(x, cutoff, sample_rate).0
    }
}
