use crate::dsp::{db_to_lin, flush_denormal, sanitize, wet_dry};
use crate::effect::{impl_effect_accessors, Effect};
use crate::param::{EffectSchema, ParamScale, ParamSchema};

const NONE: &[&str] = &[];

pub(crate) static SCHEMA: EffectSchema = EffectSchema {
    id: "compressor",
    name: "Compressor",
    description:
        "Stereo-linked compressor with a soft knee. Ratio 1 and 0 dB makeup is unity gain.",
    params: &[
        ParamSchema {
            id: "threshold_db",
            name: "Threshold",
            description: "Level where gain reduction begins.",
            unit: "dB",
            min: -60.0,
            max: 0.0,
            default: -18.0,
            scale: ParamScale::Linear,
            choices: NONE,
        },
        ParamSchema {
            id: "ratio",
            name: "Ratio",
            description: "Amount of gain reduction above the threshold.",
            unit: ":1",
            min: 1.0,
            max: 20.0,
            default: 4.0,
            scale: ParamScale::Linear,
            choices: NONE,
        },
        ParamSchema {
            id: "attack_ms",
            name: "Attack",
            description: "How fast the detector follows rising level.",
            unit: "ms",
            min: 0.1,
            max: 200.0,
            default: 10.0,
            scale: ParamScale::Logarithmic,
            choices: NONE,
        },
        ParamSchema {
            id: "release_ms",
            name: "Release",
            description: "How fast the detector follows falling level.",
            unit: "ms",
            min: 5.0,
            max: 2_000.0,
            default: 120.0,
            scale: ParamScale::Logarithmic,
            choices: NONE,
        },
        ParamSchema {
            id: "makeup_db",
            name: "Makeup",
            description: "Gain applied after reduction.",
            unit: "dB",
            min: 0.0,
            max: 24.0,
            default: 0.0,
            scale: ParamScale::Linear,
            choices: NONE,
        },
        ParamSchema {
            id: "mix",
            name: "Mix",
            description: "Dry/wet mix for parallel compression. 0 is fully dry.",
            unit: "",
            min: 0.0,
            max: 1.0,
            default: 1.0,
            scale: ParamScale::Linear,
            choices: NONE,
        },
    ],
};

pub(crate) fn create() -> Box<dyn Effect> {
    Box::new(Compressor::new())
}

struct Compressor {
    params: crate::param::ParamBank,
    env: f32,
}

impl Compressor {
    fn new() -> Self {
        Self {
            params: crate::param::ParamBank::new(&SCHEMA),
            env: 0.0,
        }
    }
}

fn coef(time_ms: f32, sample_rate: f32) -> f32 {
    let seconds = (time_ms.max(0.05) / 1_000.0).max(1.0 / sample_rate);
    (-1.0 / (seconds * sample_rate)).exp()
}

impl Effect for Compressor {
    impl_effect_accessors!();

    fn process(&mut self, samples: &mut [f32], sample_rate: f32) {
        if self.params.is_bypassed() {
            return;
        }
        let threshold = self.params.value(0);
        let ratio = self.params.value(1).max(1.0);
        let attack = coef(self.params.value(2), sample_rate);
        let release = coef(self.params.value(3), sample_rate);
        let makeup = self.params.value(4);
        let mix = self.params.value(5);
        let slope = 1.0 - 1.0 / ratio;
        let knee = 6.0;
        let frames = samples.len() / 2;
        for frame in 0..frames {
            let i = frame * 2;
            let dry_l = sanitize(samples[i]);
            let dry_r = sanitize(samples[i + 1]);
            let peak = dry_l.abs().max(dry_r.abs());
            let follow = if peak > self.env { attack } else { release };
            self.env = flush_denormal(follow * self.env + (1.0 - follow) * peak);
            let level_db = if self.env <= 1.0e-8 {
                -160.0
            } else {
                20.0 * self.env.log10()
            };
            let over = level_db - threshold;
            let reduction = if slope == 0.0 || over <= -knee * 0.5 {
                0.0
            } else if over >= knee * 0.5 {
                over * slope
            } else {
                let x = over + knee * 0.5;
                (x * x) / (2.0 * knee) * slope
            };
            let gain = db_to_lin(-reduction + makeup);
            samples[i] = wet_dry(dry_l, dry_l * gain, mix);
            samples[i + 1] = wet_dry(dry_r, dry_r * gain, mix);
        }
    }

    fn reset(&mut self) {
        self.env = 0.0;
    }
}
