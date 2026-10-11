use crate::dsp::{flush_denormal, sanitize, wet_dry};
use crate::effect::{impl_effect_accessors, Effect};
use crate::param::{EffectSchema, ParamScale, ParamSchema};

const NONE: &[&str] = &[];

const COMBS_L: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const COMBS_R: [usize; 8] = [1139, 1211, 1300, 1379, 1445, 1514, 1580, 1640];
const ALLPASS: [usize; 4] = [556, 441, 341, 225];

pub(crate) static SCHEMA: EffectSchema = EffectSchema {
    id: "reverb",
    name: "Reverb",
    description:
        "Algorithmic room: eight damped comb filters into four all-pass diffusers, per channel.",
    params: &[
        ParamSchema {
            id: "room",
            name: "Room",
            description: "Room size, heard as feedback in the comb filters.",
            unit: "",
            min: 0.0,
            max: 1.0,
            default: 0.55,
            scale: ParamScale::Linear,
            choices: NONE,
        },
        ParamSchema {
            id: "damp",
            name: "Damp",
            description: "High-frequency absorption in the feedback path.",
            unit: "",
            min: 0.0,
            max: 1.0,
            default: 0.4,
            scale: ParamScale::Linear,
            choices: NONE,
        },
        ParamSchema {
            id: "width",
            name: "Width",
            description: "Stereo spread of the wet signal. 0 is mono.",
            unit: "",
            min: 0.0,
            max: 1.0,
            default: 0.8,
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
            default: 0.25,
            scale: ParamScale::Linear,
            choices: NONE,
        },
    ],
};

pub(crate) fn create() -> Box<dyn Effect> {
    Box::new(Reverb::new())
}

struct Comb {
    buf: Vec<f32>,
    index: usize,
    filterstore: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(2)],
            index: 0,
            filterstore: 0.0,
        }
    }

    fn process(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let output = self.buf[self.index];
        self.filterstore = flush_denormal(output * (1.0 - damp) + self.filterstore * damp);
        self.buf[self.index] = flush_denormal(input + self.filterstore * feedback);
        self.index += 1;
        if self.index >= self.buf.len() {
            self.index = 0;
        }
        output
    }
}

struct Allpass {
    buf: Vec<f32>,
    index: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(2)],
            index: 0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let bufout = self.buf[self.index];
        let output = -input + bufout;
        self.buf[self.index] = flush_denormal(input + bufout * 0.5);
        self.index += 1;
        if self.index >= self.buf.len() {
            self.index = 0;
        }
        output
    }
}

struct Reverb {
    params: crate::param::ParamBank,
    combs_l: Vec<Comb>,
    combs_r: Vec<Comb>,
    all_l: Vec<Allpass>,
    all_r: Vec<Allpass>,
    prepared_sr: f32,
}

impl Reverb {
    fn new() -> Self {
        Self {
            params: crate::param::ParamBank::new(&SCHEMA),
            combs_l: Vec::new(),
            combs_r: Vec::new(),
            all_l: Vec::new(),
            all_r: Vec::new(),
            prepared_sr: 0.0,
        }
    }

    fn prepare(&mut self, sample_rate: f32) {
        if (self.prepared_sr - sample_rate).abs() < 0.5 && !self.combs_l.is_empty() {
            return;
        }
        let scale = sample_rate / 44_100.0;
        let len = |base: usize| ((base as f32) * scale).round().max(2.0) as usize;
        self.combs_l = COMBS_L.into_iter().map(|n| Comb::new(len(n))).collect();
        self.combs_r = COMBS_R.into_iter().map(|n| Comb::new(len(n))).collect();
        self.all_l = ALLPASS.into_iter().map(|n| Allpass::new(len(n))).collect();
        self.all_r = ALLPASS.into_iter().map(|n| Allpass::new(len(n))).collect();
        self.prepared_sr = sample_rate;
    }
}

impl Effect for Reverb {
    impl_effect_accessors!();

    fn process(&mut self, samples: &mut [f32], sample_rate: f32) {
        if self.params.is_bypassed() {
            return;
        }
        self.prepare(sample_rate);
        let feedback = (0.55 + self.params.value(0) * 0.40).clamp(0.0, 0.95);
        let damp = 0.05 + self.params.value(1).clamp(0.0, 1.0) * 0.7;
        let width = self.params.value(2).clamp(0.0, 1.0);
        let mix = self.params.value(3);
        let frames = samples.len() / 2;
        for frame in 0..frames {
            let i = frame * 2;
            let dry_l = sanitize(samples[i]);
            let dry_r = sanitize(samples[i + 1]);
            let mut acc_l = 0.0;
            let mut acc_r = 0.0;
            for comb in &mut self.combs_l {
                acc_l += comb.process(dry_l, feedback, damp);
            }
            for comb in &mut self.combs_r {
                acc_r += comb.process(dry_r, feedback, damp);
            }
            acc_l *= 0.125;
            acc_r *= 0.125;
            for allpass in &mut self.all_l {
                acc_l = allpass.process(acc_l);
            }
            for allpass in &mut self.all_r {
                acc_r = allpass.process(acc_r);
            }
            let wet_l = acc_l * (width * 0.5 + 0.5) + acc_r * ((1.0 - width) * 0.5);
            let wet_r = acc_r * (width * 0.5 + 0.5) + acc_l * ((1.0 - width) * 0.5);
            if mix != 0.0 {
                samples[i] = wet_dry(dry_l, wet_l, mix);
                samples[i + 1] = wet_dry(dry_r, wet_r, mix);
            }
        }
    }

    fn reset(&mut self) {
        for comb in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            comb.buf.fill(0.0);
            comb.index = 0;
            comb.filterstore = 0.0;
        }
        for allpass in self.all_l.iter_mut().chain(self.all_r.iter_mut()) {
            allpass.buf.fill(0.0);
            allpass.index = 0;
        }
    }
}
