//! Automation tracks are closed-form functions of absolute time.
//!
//! Nothing here integrates from the previous sample. A keyframe curve, an
//! LFO, and a spring all return the same value at the same second no matter
//! which frame was rendered before it.

use super::{invalid, Ease, Keyframe, LfoShape, Track};
use crate::Error;

impl Track {
    /// Value at `seconds` from the start of the arrangement.
    ///
    /// `tempo_bpm` converts beat-stamped keyframes and springs. The result is
    /// one `f32` cast from `f64` math, so the same inputs always yield the
    /// same bits.
    pub fn sample(&self, seconds: f64, tempo_bpm: f64) -> Result<f32, Error> {
        if !seconds.is_finite() {
            return Err(invalid("automation time is not finite"));
        }
        let value = match self {
            Track::Constant { value } => f64::from(*value),
            Track::Keyframes { keys } => sample_keys(keys, seconds, tempo_bpm)?,
            Track::Lfo {
                shape,
                rate_hz,
                per_beat,
                depth,
                center,
                phase,
            } => {
                let rate = lfo_rate(*rate_hz, *per_beat, tempo_bpm)?;
                let cycles = *phase + rate * seconds;
                let wave = lfo_wave(*shape, cycles);
                f64::from(*center) + f64::from(*depth) * wave
            }
            Track::Spring {
                from,
                to,
                at,
                response,
                bounce,
            } => {
                let at = at.to_seconds(tempo_bpm)?;
                let response = response.to_seconds(tempo_bpm)?;
                spring(
                    f64::from(*from),
                    f64::from(*to),
                    seconds - at,
                    response,
                    f64::from(*bounce),
                )
            }
        };
        if !value.is_finite() {
            return Err(invalid("automation produced a non-finite value"));
        }
        Ok(value as f32)
    }

    pub(crate) fn validate(&self, label: &str, tempo_bpm: f64) -> Result<(), Error> {
        match self {
            Track::Constant { value } => finite_f32(label, *value)?,
            Track::Keyframes { keys } => {
                if keys.is_empty() {
                    return Err(invalid(format!("{label} keyframes are empty")));
                }
                let mut previous: Option<f64> = None;
                for key in keys {
                    finite_f32(label, key.value)?;
                    let time = key.at.to_seconds(tempo_bpm)?;
                    if let Some(previous) = previous {
                        if time <= previous {
                            return Err(invalid(format!(
                                "{label} keyframes must be strictly increasing in time"
                            )));
                        }
                    }
                    previous = Some(time);
                }
            }
            Track::Lfo {
                rate_hz,
                per_beat,
                depth,
                center,
                phase,
                ..
            } => {
                match (rate_hz, per_beat) {
                    (Some(rate), None) if rate.is_finite() && *rate >= 0.0 => {}
                    (None, Some(rate)) if rate.is_finite() && *rate >= 0.0 => {}
                    _ => {
                        return Err(invalid(format!(
                            "{label} LFO needs exactly one non-negative rate_hz or per_beat"
                        )));
                    }
                }
                finite_f32(label, *depth)?;
                finite_f32(label, *center)?;
                if !phase.is_finite() {
                    return Err(invalid(format!("{label} LFO phase is not finite")));
                }
            }
            Track::Spring {
                from,
                to,
                at,
                response,
                bounce,
            } => {
                finite_f32(label, *from)?;
                finite_f32(label, *to)?;
                finite_f32(label, *bounce)?;
                let _ = at.to_seconds(tempo_bpm)?;
                let response = response.to_seconds(tempo_bpm)?;
                if response < 0.0 {
                    return Err(invalid(format!("{label} spring response is negative")));
                }
            }
        }
        Ok(())
    }
}

fn finite_f32(label: &str, value: f32) -> Result<(), Error> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(invalid(format!("{label} has a non-finite value")))
    }
}

fn sample_keys(keys: &[Keyframe], seconds: f64, tempo_bpm: f64) -> Result<f64, Error> {
    if keys.is_empty() {
        return Err(invalid("keyframes are empty"));
    }
    let first = keys[0].at.to_seconds(tempo_bpm)?;
    if seconds <= first {
        return Ok(f64::from(keys[0].value));
    }
    for index in 1..keys.len() {
        let end = keys[index].at.to_seconds(tempo_bpm)?;
        if seconds < end {
            let start = keys[index - 1].at.to_seconds(tempo_bpm)?;
            let span = end - start;
            let u = if span == 0.0 {
                0.0
            } else {
                (seconds - start) / span
            };
            return Ok(ease(
                keys[index].ease,
                f64::from(keys[index - 1].value),
                f64::from(keys[index].value),
                u,
            ));
        }
    }
    Ok(f64::from(keys[keys.len() - 1].value))
}

fn ease(curve: Ease, from: f64, to: f64, u: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    let shaped = match curve {
        Ease::Hold => 0.0,
        Ease::Linear => u,
        Ease::Smooth => u * u * u * (u * (u * 6.0 - 15.0) + 10.0),
        Ease::In => u * u * u,
        Ease::Out => {
            let v = 1.0 - u;
            1.0 - v * v * v
        }
        Ease::InOut => u * u * (3.0 - 2.0 * u),
    };
    from + (to - from) * shaped
}

fn lfo_rate(rate_hz: Option<f64>, per_beat: Option<f64>, tempo_bpm: f64) -> Result<f64, Error> {
    match (rate_hz, per_beat) {
        (Some(rate), None) => Ok(rate),
        (None, Some(per_beat)) => {
            if !tempo_bpm.is_finite() || tempo_bpm <= 0.0 {
                return Err(invalid("tempo must be positive to convert an LFO rate"));
            }
            Ok(per_beat * tempo_bpm / 60.0)
        }
        _ => Err(invalid(
            "LFO needs exactly one non-negative rate_hz or per_beat",
        )),
    }
}

fn lfo_wave(shape: LfoShape, cycles: f64) -> f64 {
    match shape {
        LfoShape::Sine => (std::f64::consts::TAU * cycles).sin(),
        LfoShape::Triangle => {
            let f = cycles - cycles.floor();
            if f < 0.5 {
                f * 4.0 - 1.0
            } else {
                3.0 - f * 4.0
            }
        }
        LfoShape::Square => {
            let f = cycles - cycles.floor();
            if f < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        LfoShape::Hold => hold_wave(cycles.floor() as i64),
    }
}

fn hold_wave(cycle: i64) -> f64 {
    let mut x = (cycle as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    let bits = ((x >> 40) as u32) & 0x00FF_FFFF;
    (f64::from(bits) / 16_777_215.0) * 2.0 - 1.0
}

fn spring(from: f64, to: f64, elapsed: f64, response: f64, bounce: f64) -> f64 {
    if elapsed <= 0.0 {
        return from;
    }
    if response <= 1.0e-4 {
        return to;
    }
    let bounce = bounce.clamp(0.0, 1.0);
    let zeta = 1.0 - 0.55 * bounce;
    let omega = 8.0 / response;
    let y0 = from - to;
    let decay = (-zeta * omega * elapsed).exp();
    let y = if (zeta - 1.0).abs() < 1.0e-6 {
        y0 * (1.0 + omega * elapsed) * decay
    } else {
        let wd = omega * (1.0 - zeta * zeta).sqrt();
        let cos = (wd * elapsed).cos();
        let sin = (wd * elapsed).sin();
        decay * (y0 * cos + (zeta * omega * y0 / wd) * sin)
    };
    to + y
}
