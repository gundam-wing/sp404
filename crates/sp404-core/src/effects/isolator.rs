use crate::dsp::db_to_lin;
use crate::dsp::OnePole;
use crate::effect::{impl_effect_accessors, Effect};
use crate::param::{EffectSchema, ParamScale, ParamSchema};
use crate::state::{EffectState, Reader, Writer};
use crate::Error;

const NONE: &[&str] = &[];

pub(crate) static SCHEMA: EffectSchema = EffectSchema {
    id: "isolator",
    name: "Isolator",
    description: "Three-band isolator. 0 dB on every band reconstructs the input exactly.",
    params: &[
        ParamSchema {
            id: "low_db",
            name: "Low",
            description: "Low-band gain.",
            unit: "dB",
            min: -24.0,
            max: 12.0,
            default: 0.0,
            scale: ParamScale::Linear,
            choices: NONE,
        },
        ParamSchema {
            id: "mid_db",
            name: "Mid",
            description: "Mid-band gain.",
            unit: "dB",
            min: -24.0,
            max: 12.0,
            default: 0.0,
            scale: ParamScale::Linear,
            choices: NONE,
        },
        ParamSchema {
            id: "high_db",
            name: "High",
            description: "High-band gain.",
            unit: "dB",
            min: -24.0,
            max: 12.0,
            default: 0.0,
            scale: ParamScale::Linear,
            choices: NONE,
        },
        ParamSchema {
            id: "low_mid_hz",
            name: "Low/Mid",
            description: "Crossover between the low and mid bands.",
            unit: "Hz",
            min: 40.0,
            max: 800.0,
            default: 200.0,
            scale: ParamScale::Logarithmic,
            choices: NONE,
        },
        ParamSchema {
            id: "mid_high_hz",
            name: "Mid/High",
            description: "Crossover between the mid and high bands.",
            unit: "Hz",
            min: 800.0,
            max: 12_000.0,
            default: 3_000.0,
            scale: ParamScale::Logarithmic,
            choices: NONE,
        },
    ],
};

pub(crate) fn create() -> Box<dyn Effect> {
    Box::new(Isolator::new())
}

struct Isolator {
    params: crate::param::ParamBank,
    low_l: OnePole,
    high_l: OnePole,
    low_r: OnePole,
    high_r: OnePole,
}

impl Isolator {
    fn new() -> Self {
        Self {
            params: crate::param::ParamBank::new(&SCHEMA),
            low_l: OnePole::new(),
            high_l: OnePole::new(),
            low_r: OnePole::new(),
            high_r: OnePole::new(),
        }
    }

    fn channel(
        low_split: &mut OnePole,
        high_split: &mut OnePole,
        x: f32,
        gains: [f32; 3],
        crossovers: [f32; 2],
        sample_rate: f32,
    ) -> f32 {
        let (lowmid, high) = high_split.split(x, crossovers[1], sample_rate);
        let (low, mid) = low_split.split(lowmid, crossovers[0], sample_rate);
        low * gains[0] + mid * gains[1] + high * gains[2]
    }
}

impl Effect for Isolator {
    impl_effect_accessors!();

    fn process(&mut self, samples: &mut [f32], sample_rate: f32) {
        if self.params.is_bypassed() {
            return;
        }
        let low_gain = db_to_lin(self.params.value(0));
        let mid_gain = db_to_lin(self.params.value(1));
        let high_gain = db_to_lin(self.params.value(2));
        let mut low_mid = self.params.value(3);
        let mut mid_high = self.params.value(4);
        if low_mid > mid_high {
            std::mem::swap(&mut low_mid, &mut mid_high);
        }
        let frames = samples.len() / 2;
        for frame in 0..frames {
            let i = frame * 2;
            let gains = [low_gain, mid_gain, high_gain];
            let crossovers = [low_mid, mid_high];
            samples[i] = Self::channel(
                &mut self.low_l,
                &mut self.high_l,
                samples[i],
                gains,
                crossovers,
                sample_rate,
            );
            samples[i + 1] = Self::channel(
                &mut self.low_r,
                &mut self.high_r,
                samples[i + 1],
                gains,
                crossovers,
                sample_rate,
            );
        }
    }

    fn reset(&mut self) {
        self.low_l.reset();
        self.high_l.reset();
        self.low_r.reset();
        self.high_r.reset();
    }

    fn capture_state(&self) -> EffectState {
        let mut writer = Writer::new();
        for pole in [&self.low_l, &self.high_l, &self.low_r, &self.high_r] {
            writer.f(pole.z());
        }
        writer.finish()
    }

    fn restore_state(&mut self, state: &EffectState) -> Result<(), Error> {
        let mut reader = Reader::new(state);
        let mut values = [0.0; 4];
        for slot in &mut values {
            *slot = reader.f().ok_or_else(bad_state)?;
        }
        if !reader.finish() {
            return Err(bad_state());
        }
        self.low_l.set_z(values[0]);
        self.high_l.set_z(values[1]);
        self.low_r.set_z(values[2]);
        self.high_r.set_z(values[3]);
        Ok(())
    }
}

fn bad_state() -> Error {
    Error::StateMismatch {
        effect: "isolator".into(),
        reason: "snapshot does not match an isolator".into(),
    }
}
