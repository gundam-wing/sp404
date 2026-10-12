use crate::dsp::{wet_dry, Svf};
use crate::effect::{impl_effect_accessors, Effect};
use crate::param::{EffectSchema, ParamScale, ParamSchema};
use crate::state::{EffectState, Reader, Writer};
use crate::Error;

const NONE: &[&str] = &[];

pub(crate) static SCHEMA: EffectSchema = EffectSchema {
    id: "filter",
    name: "Filter",
    description: "Resonant state-variable filter with low-pass, band-pass, and high-pass outputs.",
    params: &[
        ParamSchema {
            id: "mode",
            name: "Mode",
            description: "Which filter output is heard.",
            unit: "",
            min: 0.0,
            max: 2.0,
            default: 0.0,
            scale: ParamScale::Choice,
            choices: &["low-pass", "band-pass", "high-pass"],
        },
        ParamSchema {
            id: "cutoff",
            name: "Cutoff",
            description: "Cutoff frequency.",
            unit: "Hz",
            min: 20.0,
            max: 20_000.0,
            default: 1_800.0,
            scale: ParamScale::Logarithmic,
            choices: NONE,
        },
        ParamSchema {
            id: "resonance",
            name: "Resonance",
            description: "Emphasis at the cutoff. 0 is nearly Butterworth; 1 is high Q.",
            unit: "",
            min: 0.0,
            max: 1.0,
            default: 0.35,
            scale: ParamScale::Linear,
            choices: NONE,
        },
        ParamSchema {
            id: "mix",
            name: "Mix",
            description: "Dry/wet mix. 0 is fully dry.",
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
    Box::new(Filter::new())
}

struct Filter {
    params: crate::param::ParamBank,
    left: Svf,
    right: Svf,
}

impl Filter {
    fn new() -> Self {
        Self {
            params: crate::param::ParamBank::new(&SCHEMA),
            left: Svf::new(),
            right: Svf::new(),
        }
    }
}

impl Effect for Filter {
    impl_effect_accessors!();

    fn process(&mut self, samples: &mut [f32], sample_rate: f32) {
        if self.params.is_bypassed() {
            return;
        }
        let mode = self.params.value(0).round() as i32;
        let cutoff = self.params.value(1);
        let resonance = self.params.value(2);
        let mix = self.params.value(3);
        if mix == 0.0 {
            return;
        }
        let frames = samples.len() / 2;
        for frame in 0..frames {
            let i = frame * 2;
            let dry_l = samples[i];
            let dry_r = samples[i + 1];
            let (lp_l, bp_l, hp_l) = self.left.process(dry_l, cutoff, resonance, sample_rate);
            let (lp_r, bp_r, hp_r) = self.right.process(dry_r, cutoff, resonance, sample_rate);
            let (wet_l, wet_r) = match mode {
                1 => (bp_l, bp_r),
                2 => (hp_l, hp_r),
                _ => (lp_l, lp_r),
            };
            samples[i] = wet_dry(dry_l, wet_l, mix);
            samples[i + 1] = wet_dry(dry_r, wet_r, mix);
        }
    }

    fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
    }

    fn capture_state(&self) -> EffectState {
        let mut writer = Writer::new();
        for channel in [self.left.capture(), self.right.capture()] {
            writer.f(channel[0]);
            writer.f(channel[1]);
        }
        writer.finish()
    }

    fn restore_state(&mut self, state: &EffectState) -> Result<(), Error> {
        let mut reader = Reader::new(state);
        let mut left = [0.0; 2];
        let mut right = [0.0; 2];
        left[0] = reader.f().ok_or_else(bad_state)?;
        left[1] = reader.f().ok_or_else(bad_state)?;
        right[0] = reader.f().ok_or_else(bad_state)?;
        right[1] = reader.f().ok_or_else(bad_state)?;
        if !reader.finish() {
            return Err(bad_state());
        }
        self.left.restore(left);
        self.right.restore(right);
        Ok(())
    }
}

fn bad_state() -> Error {
    Error::StateMismatch {
        effect: "filter".into(),
        reason: "snapshot does not match a filter".into(),
    }
}
