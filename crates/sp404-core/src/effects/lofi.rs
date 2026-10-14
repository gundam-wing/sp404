use crate::dsp::{sanitize, wet_dry};
use crate::effect::{impl_effect_accessors, Effect};
use crate::param::{EffectSchema, ParamScale, ParamSchema};
use crate::state::{EffectState, Reader, Writer};
use crate::Error;

const NONE: &[&str] = &[];

pub(crate) static SCHEMA: EffectSchema = EffectSchema {
    id: "lofi",
    name: "Lo-Fi",
    description: "Sample-and-hold rate reduction plus bit depth quantization.",
    params: &[
        ParamSchema {
            id: "bit_depth",
            name: "Bits",
            description: "Quantization depth. 16 is fine; lower values stair-step the waveform.",
            unit: "bits",
            min: 1.0,
            max: 16.0,
            default: 8.0,
            scale: ParamScale::Linear,
            choices: NONE,
        },
        ParamSchema {
            id: "downsample",
            name: "Downsample",
            description: "How many output samples share one held input sample.",
            unit: "x",
            min: 1.0,
            max: 32.0,
            default: 4.0,
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
    Box::new(Lofi::new())
}

struct Lofi {
    params: crate::param::ParamBank,
    hold_l: f32,
    hold_r: f32,
    counter: u32,
}

impl Lofi {
    fn new() -> Self {
        Self {
            params: crate::param::ParamBank::new(&SCHEMA),
            hold_l: 0.0,
            hold_r: 0.0,
            counter: 0,
        }
    }
}

fn quantize(x: f32, bits: f32) -> f32 {
    let steps = (2.0f32).powf(bits.clamp(1.0, 16.0) - 1.0).max(1.0);
    let x = sanitize(x).clamp(-1.0, 1.0);
    (x * steps).round() / steps
}

impl Effect for Lofi {
    impl_effect_accessors!();

    fn process(&mut self, samples: &mut [f32], _sample_rate: f32) {
        if self.params.is_bypassed() {
            return;
        }
        let bits = self.params.value(0);
        let hold_for = self.params.value(1).round().clamp(1.0, 32.0) as u32;
        let mix = self.params.value(2);
        let frames = samples.len() / 2;
        for frame in 0..frames {
            let i = frame * 2;
            let dry_l = samples[i];
            let dry_r = samples[i + 1];
            if self.counter == 0 {
                self.hold_l = quantize(dry_l, bits);
                self.hold_r = quantize(dry_r, bits);
            }
            self.counter += 1;
            if self.counter >= hold_for {
                self.counter = 0;
            }
            if mix != 0.0 {
                samples[i] = wet_dry(dry_l, self.hold_l, mix);
                samples[i + 1] = wet_dry(dry_r, self.hold_r, mix);
            }
        }
    }

    fn reset(&mut self) {
        self.hold_l = 0.0;
        self.hold_r = 0.0;
        self.counter = 0;
    }

    fn capture_state(&self) -> EffectState {
        let mut writer = Writer::new();
        writer.f(self.hold_l);
        writer.f(self.hold_r);
        writer.u(self.counter);
        writer.finish()
    }

    fn restore_state(&mut self, state: &EffectState) -> Result<(), Error> {
        let mut reader = Reader::new(state);
        let hold_l = reader.f().ok_or_else(bad_state)?;
        let hold_r = reader.f().ok_or_else(bad_state)?;
        let counter = reader.u().ok_or_else(bad_state)?;
        if !reader.finish() {
            return Err(bad_state());
        }
        self.hold_l = hold_l;
        self.hold_r = hold_r;
        self.counter = counter;
        Ok(())
    }
}

fn bad_state() -> Error {
    Error::StateMismatch {
        effect: "lofi".into(),
        reason: "snapshot does not match a lo-fi effect".into(),
    }
}
