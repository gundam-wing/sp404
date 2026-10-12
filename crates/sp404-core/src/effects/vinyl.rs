use crate::dsp::{flush_denormal, read_frac, sanitize, wet_dry, OnePole, TAU};
use crate::effect::{impl_effect_accessors, Effect};
use crate::param::{EffectSchema, ParamScale, ParamSchema};
use crate::state::{EffectState, Reader, Writer};
use crate::Error;

const NONE: &[&str] = &[];

pub(crate) static SCHEMA: EffectSchema = EffectSchema {
    id: "vinyl",
    name: "Vinyl",
    description: "Wow, flutter, crackle, and a worn high-frequency rolloff. Zero amounts leave the signal unchanged.",
    params: &[
        ParamSchema {
            id: "wow",
            name: "Wow",
            description: "Slow pitch drift, implemented as a modulated short delay.",
            unit: "",
            min: 0.0,
            max: 1.0,
            default: 0.3,
            scale: ParamScale::Linear,
            choices: NONE,
        },
        ParamSchema {
            id: "flutter",
            name: "Flutter",
            description: "Faster pitch drift layered on the wow.",
            unit: "",
            min: 0.0,
            max: 1.0,
            default: 0.15,
            scale: ParamScale::Linear,
            choices: NONE,
        },
        ParamSchema {
            id: "crackle",
            name: "Crackle",
            description: "Density of dust impulses.",
            unit: "",
            min: 0.0,
            max: 1.0,
            default: 0.25,
            scale: ParamScale::Linear,
            choices: NONE,
        },
        ParamSchema {
            id: "wear",
            name: "Wear",
            description: "High-cut and soft saturation, as on a played record.",
            unit: "",
            min: 0.0,
            max: 1.0,
            default: 0.3,
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
            default: 0.85,
            scale: ParamScale::Linear,
            choices: NONE,
        },
    ],
};

pub(crate) fn create() -> Box<dyn Effect> {
    Box::new(Vinyl::new())
}

struct Rng(u32);

impl Rng {
    fn next_unit(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((self.0 >> 8) & 0x00FF_FFFF) as f32 / 16_777_215.0
    }

    fn bipolar(&mut self) -> f32 {
        self.next_unit() * 2.0 - 1.0
    }
}

struct Vinyl {
    params: crate::param::ParamBank,
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    write: usize,
    wow_phase: f32,
    flutter_phase: f32,
    wear_l: OnePole,
    wear_r: OnePole,
    crackle_l: f32,
    crackle_r: f32,
    rng: Rng,
    prepared_sr: f32,
}

impl Vinyl {
    fn new() -> Self {
        Self {
            params: crate::param::ParamBank::new(&SCHEMA),
            buf_l: Vec::new(),
            buf_r: Vec::new(),
            write: 0,
            wow_phase: 0.0,
            flutter_phase: 1.3,
            wear_l: OnePole::new(),
            wear_r: OnePole::new(),
            crackle_l: 0.0,
            crackle_r: 0.0,
            rng: Rng(0x5044_0404),
            prepared_sr: 0.0,
        }
    }

    fn prepare(&mut self, sample_rate: f32) {
        if (self.prepared_sr - sample_rate).abs() < 0.5 && !self.buf_l.is_empty() {
            return;
        }
        let len = (sample_rate * 0.05) as usize + 16;
        self.buf_l = vec![0.0; len];
        self.buf_r = vec![0.0; len];
        self.write = 0;
        self.prepared_sr = sample_rate;
    }
}

impl Effect for Vinyl {
    impl_effect_accessors!();

    fn process(&mut self, samples: &mut [f32], sample_rate: f32) {
        if self.params.is_bypassed() {
            return;
        }
        self.prepare(sample_rate);
        let wow = self.params.value(0);
        let flutter = self.params.value(1);
        let crackle = self.params.value(2);
        let wear = self.params.value(3);
        let mix = self.params.value(4);
        let len = self.buf_l.len();
        let frames = samples.len() / 2;
        let wear_cutoff = 8_000.0 - wear * 6_200.0;
        for frame in 0..frames {
            let i = frame * 2;
            let dry_l = sanitize(samples[i]);
            let dry_r = sanitize(samples[i + 1]);
            self.buf_l[self.write] = dry_l;
            self.buf_r[self.write] = dry_r;

            let delay = if wow == 0.0 && flutter == 0.0 {
                0.0
            } else {
                self.wow_phase += TAU * 0.45 / sample_rate;
                self.flutter_phase += TAU * 7.5 / sample_rate;
                if self.wow_phase > TAU {
                    self.wow_phase -= TAU;
                }
                if self.flutter_phase > TAU {
                    self.flutter_phase -= TAU;
                }
                let mod_delay = self.wow_phase.sin() * wow * 0.0035
                    + self.flutter_phase.sin() * flutter * 0.00045;
                (mod_delay.max(0.0) * sample_rate).min((len - 2) as f32)
            };

            let mut wet_l = read_frac(&self.buf_l, self.write as f32 - delay);
            let mut wet_r = read_frac(&self.buf_r, self.write as f32 - delay);
            if wear > 0.0 {
                wet_l = self.wear_l.lowpass(wet_l, wear_cutoff, sample_rate);
                wet_r = self.wear_r.lowpass(wet_r, wear_cutoff, sample_rate);
                let drive = 1.0 + wear * 3.0;
                wet_l = (wet_l * drive).tanh();
                wet_r = (wet_r * drive).tanh();
            }
            if crackle > 0.0 {
                self.crackle_l = flush_denormal(self.crackle_l * 0.90);
                self.crackle_r = flush_denormal(self.crackle_r * 0.90);
                let density = crackle * 0.0025;
                if self.rng.next_unit() < density {
                    self.crackle_l = self.rng.bipolar() * (0.25 + 0.75 * crackle);
                }
                if self.rng.next_unit() < density {
                    self.crackle_r = self.rng.bipolar() * (0.25 + 0.75 * crackle);
                }
                wet_l += self.crackle_l;
                wet_r += self.crackle_r;
            }
            self.write += 1;
            if self.write >= len {
                self.write = 0;
            }
            if mix != 0.0 {
                samples[i] = wet_dry(dry_l, wet_l, mix);
                samples[i + 1] = wet_dry(dry_r, wet_r, mix);
            }
        }
    }

    fn reset(&mut self) {
        self.buf_l.fill(0.0);
        self.buf_r.fill(0.0);
        self.write = 0;
        self.wow_phase = 0.0;
        self.flutter_phase = 1.3;
        self.wear_l.reset();
        self.wear_r.reset();
        self.crackle_l = 0.0;
        self.crackle_r = 0.0;
    }

    fn capture_state(&self) -> EffectState {
        let mut writer = Writer::new();
        writer.u(self.write as u32);
        writer.u(self.buf_l.len() as u32);
        writer.u(self.rng.0);
        writer.f(self.prepared_sr);
        writer.f(self.wow_phase);
        writer.f(self.flutter_phase);
        writer.f(self.wear_l.z());
        writer.f(self.wear_r.z());
        writer.f(self.crackle_l);
        writer.f(self.crackle_r);
        writer.samples(&self.buf_l);
        writer.samples(&self.buf_r);
        writer.finish()
    }

    fn restore_state(&mut self, state: &EffectState) -> Result<(), Error> {
        let mut reader = Reader::new(state);
        let write = reader.u().ok_or_else(bad_state)? as usize;
        let len = reader.u().ok_or_else(bad_state)? as usize;
        let rng = reader.u().ok_or_else(bad_state)?;
        let prepared_sr = reader.f().ok_or_else(bad_state)?;
        let wow = reader.f().ok_or_else(bad_state)?;
        let flutter = reader.f().ok_or_else(bad_state)?;
        let wear_l = reader.f().ok_or_else(bad_state)?;
        let wear_r = reader.f().ok_or_else(bad_state)?;
        let crackle_l = reader.f().ok_or_else(bad_state)?;
        let crackle_r = reader.f().ok_or_else(bad_state)?;
        let left = reader.samples(len).ok_or_else(bad_state)?.to_vec();
        let right = reader.samples(len).ok_or_else(bad_state)?.to_vec();
        if !reader.finish() || !head_in_range(write, len) {
            return Err(bad_state());
        }
        self.write = write;
        self.rng.0 = rng;
        self.prepared_sr = prepared_sr;
        self.wow_phase = wow;
        self.flutter_phase = flutter;
        self.wear_l.set_z(wear_l);
        self.wear_r.set_z(wear_r);
        self.crackle_l = crackle_l;
        self.crackle_r = crackle_r;
        self.buf_l = left;
        self.buf_r = right;
        Ok(())
    }
}

fn head_in_range(write: usize, len: usize) -> bool {
    if len == 0 {
        write == 0
    } else {
        write < len
    }
}

fn bad_state() -> Error {
    Error::StateMismatch {
        effect: "vinyl".into(),
        reason: "snapshot does not match a vinyl simulator".into(),
    }
}
