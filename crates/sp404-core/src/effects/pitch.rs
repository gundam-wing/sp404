use crate::dsp::{read_frac, sanitize, wet_dry};
use crate::effect::{impl_effect_accessors, Effect};
use crate::param::{EffectSchema, ParamScale, ParamSchema};

const NONE: &[&str] = &[];
const WINDOW: f32 = 2_048.0;
const BUF: usize = 8_192;

pub(crate) static SCHEMA: EffectSchema = EffectSchema {
    id: "pitch",
    name: "Pitch",
    description:
        "Dual-grain delay-line pitch shifter. 0 semitones is a hard bypass of the shifter.",
    params: &[
        ParamSchema {
            id: "semitones",
            name: "Pitch",
            description: "Pitch shift in semitones. Playback length stays the same.",
            unit: "st",
            min: -12.0,
            max: 12.0,
            default: 0.0,
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
    Box::new(Pitch::new())
}

struct Head {
    buf: Vec<f32>,
    write: usize,
    delay_a: f32,
    delay_b: f32,
}

impl Head {
    fn new() -> Self {
        Self {
            buf: vec![0.0; BUF],
            write: 0,
            delay_a: 1.0,
            delay_b: 1.0 + WINDOW * 0.5,
        }
    }

    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
        self.delay_a = 1.0;
        self.delay_b = 1.0 + WINDOW * 0.5;
    }

    fn process(&mut self, input: f32, ratio: f32) -> f32 {
        self.buf[self.write] = input;
        let step = 1.0 - ratio;
        self.delay_a = wrap_delay(self.delay_a + step);
        self.delay_b = wrap_delay(self.delay_b + step);
        let a = read_frac(&self.buf, self.write as f32 - self.delay_a);
        let b = read_frac(&self.buf, self.write as f32 - self.delay_b);
        let fa = tri(self.delay_a);
        let fb = tri(self.delay_b);
        let denom = fa + fb;
        let y = if denom < 1.0e-6 {
            input
        } else {
            (a * fa + b * fb) / denom
        };
        self.write += 1;
        if self.write >= BUF {
            self.write = 0;
        }
        y
    }
}

fn wrap_delay(mut delay: f32) -> f32 {
    while delay < 1.0 {
        delay += WINDOW;
    }
    while delay >= 1.0 + WINDOW {
        delay -= WINDOW;
    }
    delay
}

fn tri(delay: f32) -> f32 {
    let x = ((delay - 1.0) / WINDOW).clamp(0.0, 1.0);
    1.0 - (2.0 * x - 1.0).abs()
}

struct Pitch {
    params: crate::param::ParamBank,
    left: Head,
    right: Head,
}

impl Pitch {
    fn new() -> Self {
        Self {
            params: crate::param::ParamBank::new(&SCHEMA),
            left: Head::new(),
            right: Head::new(),
        }
    }
}

impl Effect for Pitch {
    impl_effect_accessors!();

    fn process(&mut self, samples: &mut [f32], _sample_rate: f32) {
        if self.params.is_bypassed() {
            return;
        }
        let semitones = self.params.value(0);
        let mix = self.params.value(1);
        if semitones == 0.0 || mix == 0.0 {
            return;
        }
        let ratio = 2.0f32.powf(semitones / 12.0);
        let frames = samples.len() / 2;
        for frame in 0..frames {
            let i = frame * 2;
            let dry_l = sanitize(samples[i]);
            let dry_r = sanitize(samples[i + 1]);
            let wet_l = self.left.process(dry_l, ratio);
            let wet_r = self.right.process(dry_r, ratio);
            samples[i] = wet_dry(dry_l, wet_l, mix);
            samples[i + 1] = wet_dry(dry_r, wet_r, mix);
        }
    }

    fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
    }
}
