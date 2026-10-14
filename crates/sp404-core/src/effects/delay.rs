use crate::dsp::{flush_denormal, read_frac, sanitize, wet_dry, OnePole};
use crate::effect::{impl_effect_accessors, Effect};
use crate::param::{EffectSchema, ParamScale, ParamSchema};
use crate::state::{EffectState, Reader, Writer};
use crate::Error;

const NONE: &[&str] = &[];

pub(crate) static SCHEMA: EffectSchema = EffectSchema {
    id: "delay",
    name: "Delay",
    description: "Stereo delay with tone control in the feedback path and a ping-pong crossfeed.",
    params: &[
        ParamSchema {
            id: "time_ms",
            name: "Time",
            description: "Delay time.",
            unit: "ms",
            min: 1.0,
            max: 2_000.0,
            default: 250.0,
            scale: ParamScale::Logarithmic,
            choices: NONE,
        },
        ParamSchema {
            id: "feedback",
            name: "Feedback",
            description: "How much of the delayed signal returns to the input. The feedback path is soft-clipped.",
            unit: "",
            min: 0.0,
            max: 0.95,
            default: 0.35,
            scale: ParamScale::Linear,
            choices: NONE,
        },
        ParamSchema {
            id: "tone",
            name: "Tone",
            description: "Brightness of the feedback. 0 is dark, 1 is bright.",
            unit: "",
            min: 0.0,
            max: 1.0,
            default: 0.55,
            scale: ParamScale::Linear,
            choices: NONE,
        },
        ParamSchema {
            id: "ping_pong",
            name: "Ping-Pong",
            description: "Crossfeed between the channels. 0 is independent delays; 1 swaps the feedback.",
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
            default: 0.3,
            scale: ParamScale::Linear,
            choices: NONE,
        },
    ],
};

pub(crate) fn create() -> Box<dyn Effect> {
    Box::new(Delay::new())
}

struct Delay {
    params: crate::param::ParamBank,
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    write: usize,
    tone_l: OnePole,
    tone_r: OnePole,
    prepared_sr: f32,
}

impl Delay {
    fn new() -> Self {
        Self {
            params: crate::param::ParamBank::new(&SCHEMA),
            buf_l: Vec::new(),
            buf_r: Vec::new(),
            write: 0,
            tone_l: OnePole::new(),
            tone_r: OnePole::new(),
            prepared_sr: 0.0,
        }
    }

    fn prepare(&mut self, sample_rate: f32) {
        if (self.prepared_sr - sample_rate).abs() < 0.5 && !self.buf_l.is_empty() {
            return;
        }
        let len = (sample_rate * 2.5) as usize + 8;
        self.buf_l = vec![0.0; len];
        self.buf_r = vec![0.0; len];
        self.write = 0;
        self.tone_l.reset();
        self.tone_r.reset();
        self.prepared_sr = sample_rate;
    }
}

impl Effect for Delay {
    impl_effect_accessors!();

    fn process(&mut self, samples: &mut [f32], sample_rate: f32) {
        if self.params.is_bypassed() {
            return;
        }
        self.prepare(sample_rate);
        let time_ms = self.params.value(0);
        let feedback = self.params.value(1);
        let tone = self.params.value(2);
        let ping = self.params.value(3);
        let mix = self.params.value(4);
        let delay_samples =
            (time_ms * 0.001 * sample_rate).clamp(1.0, (self.buf_l.len() - 2) as f32);
        let cutoff = 400.0 * (14_000.0_f32 / 400.0).powf(tone.clamp(0.0, 1.0));
        let len = self.buf_l.len();
        let frames = samples.len() / 2;
        for frame in 0..frames {
            let i = frame * 2;
            let dry_l = sanitize(samples[i]);
            let dry_r = sanitize(samples[i + 1]);
            let delayed_l = read_frac(&self.buf_l, self.write as f32 - delay_samples);
            let delayed_r = read_frac(&self.buf_r, self.write as f32 - delay_samples);
            let fb_l = self.tone_l.lowpass(delayed_l, cutoff, sample_rate);
            let fb_r = self.tone_r.lowpass(delayed_r, cutoff, sample_rate);
            let send_l = fb_l * (1.0 - ping) + fb_r * ping;
            let send_r = fb_r * (1.0 - ping) + fb_l * ping;
            self.buf_l[self.write] = flush_denormal(dry_l + (send_l * feedback).tanh());
            self.buf_r[self.write] = flush_denormal(dry_r + (send_r * feedback).tanh());
            self.write += 1;
            if self.write >= len {
                self.write = 0;
            }
            if mix != 0.0 {
                samples[i] = wet_dry(dry_l, delayed_l, mix);
                samples[i + 1] = wet_dry(dry_r, delayed_r, mix);
            }
        }
    }

    fn reset(&mut self) {
        self.buf_l.fill(0.0);
        self.buf_r.fill(0.0);
        self.write = 0;
        self.tone_l.reset();
        self.tone_r.reset();
    }

    fn capture_state(&self) -> EffectState {
        let mut writer = Writer::new();
        writer.u(self.write as u32);
        writer.u(self.buf_l.len() as u32);
        writer.f(self.prepared_sr);
        writer.f(self.tone_l.z());
        writer.f(self.tone_r.z());
        writer.samples(&self.buf_l);
        writer.samples(&self.buf_r);
        writer.finish()
    }

    fn restore_state(&mut self, state: &EffectState) -> Result<(), Error> {
        let mut reader = Reader::new(state);
        let write = reader.u().ok_or_else(bad_state)? as usize;
        let len = reader.u().ok_or_else(bad_state)? as usize;
        let prepared_sr = reader.f().ok_or_else(bad_state)?;
        let tone_l = reader.f().ok_or_else(bad_state)?;
        let tone_r = reader.f().ok_or_else(bad_state)?;
        let left = reader.samples(len).ok_or_else(bad_state)?.to_vec();
        let right = reader.samples(len).ok_or_else(bad_state)?.to_vec();
        if !reader.finish() || !head_in_range(write, len) {
            return Err(bad_state());
        }
        self.write = write;
        self.prepared_sr = prepared_sr;
        self.tone_l.set_z(tone_l);
        self.tone_r.set_z(tone_r);
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
        effect: "delay".into(),
        reason: "snapshot does not match a delay".into(),
    }
}
