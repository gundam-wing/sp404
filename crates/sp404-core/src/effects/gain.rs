use crate::dsp::{db_to_lin, sanitize};
use crate::effect::{impl_effect_accessors, Effect};
use crate::param::{EffectSchema, ParamScale, ParamSchema};

const NONE: &[&str] = &[];

pub(crate) static SCHEMA: EffectSchema = EffectSchema {
    id: "gain",
    name: "Gain",
    description: "Utility level control. 0 dB leaves the signal unchanged.",
    params: &[ParamSchema {
        id: "gain_db",
        name: "Gain",
        description: "Level change applied to both channels.",
        unit: "dB",
        min: -24.0,
        max: 24.0,
        default: 0.0,
        scale: ParamScale::Linear,
        choices: NONE,
    }],
};

pub(crate) fn create() -> Box<dyn Effect> {
    Box::new(Gain::default())
}

struct Gain {
    params: crate::param::ParamBank,
}

impl Gain {
    fn new() -> Self {
        Self {
            params: crate::param::ParamBank::new(&SCHEMA),
        }
    }
}

impl Default for Gain {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for Gain {
    impl_effect_accessors!();

    fn process(&mut self, samples: &mut [f32], _sample_rate: f32) {
        if self.params.is_bypassed() {
            return;
        }
        let db = self.params.value(0);
        if db == 0.0 {
            return;
        }
        let gain = db_to_lin(db);
        for sample in samples.iter_mut() {
            *sample = sanitize(*sample) * gain;
        }
    }

    fn reset(&mut self) {}
}
