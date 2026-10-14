//! Sixteen-pad sampler.
//!
//! Each pad owns a sample, a playback rate, and an insert [`EffectChain`].
//! A bus chain sits after the sum. The engine is [`Send`] and is meant to be
//! wrapped in a mutex when an audio callback and a UI share it. `process` is
//! the only method the audio thread needs to call; it allocates only if the
//! caller asks for a larger block than [`Engine::prepare`] reserved.

use crate::buffer::AudioBuffer;
use crate::chain::{ChainDesc, EffectChain};
use crate::state::EffectState;
use crate::Error;

/// Pads in one bank. Matches the classic 4×4 layout.
pub const PAD_COUNT: usize = 16;

/// A copy of the pad state a UI or agent can display.
///
/// The sample itself is not included; use [`Engine::pad_sample`] or
/// [`Engine::waveform`] for that.
#[derive(Debug, Clone, PartialEq)]
pub struct PadView {
    /// Index into the bank.
    pub index: usize,
    /// Display name, usually the file or demo name.
    pub name: String,
    /// Linear gain, typically 0 to 1.
    pub volume: f32,
    /// Playback transpose in semitones.
    pub pitch_semitones: f32,
    /// Normalized start point, 0 to 1.
    pub start: f32,
    /// Normalized end point, 0 to 1.
    pub end: f32,
    /// Whether the voice is currently playing or ringing a tail.
    pub active: bool,
    /// Whether a sample is loaded.
    pub has_sample: bool,
    /// Sample length in frames, at the engine sample rate.
    pub frames: usize,
    /// Insert chain currently on the pad.
    pub chain: ChainDesc,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Off,
    Play,
    Tail,
}

struct Pad {
    name: String,
    sample: Option<AudioBuffer>,
    volume: f32,
    pitch_semitones: f32,
    start: f32,
    end: f32,
    chain: EffectChain,
    phase: Phase,
    pos: f64,
    tail_left: u32,
}

impl Pad {
    fn empty() -> Self {
        Self {
            name: String::new(),
            sample: None,
            volume: 1.0,
            pitch_semitones: 0.0,
            start: 0.0,
            end: 1.0,
            chain: EffectChain::new(),
            phase: Phase::Off,
            pos: 0.0,
            tail_left: 0,
        }
    }

    fn view(&self, index: usize) -> PadView {
        PadView {
            index,
            name: self.name.clone(),
            volume: self.volume,
            pitch_semitones: self.pitch_semitones,
            start: self.start,
            end: self.end,
            active: self.phase != Phase::Off,
            has_sample: self.sample.is_some(),
            frames: self.sample.as_ref().map(|s| s.frames()).unwrap_or(0),
            chain: self.chain.to_desc(),
        }
    }
}

/// Voice position and effect memory for one engine.
///
/// Samples, chains, and parameter values are not included. The arrangement
/// renderer reapplies those from the plan before the next frame.
#[derive(Clone, Debug)]
pub(crate) struct PlaybackState {
    pads: Vec<PadVoice>,
    bus: Vec<EffectState>,
}

#[derive(Clone, Debug)]
struct PadVoice {
    phase: u8,
    pos_bits: u64,
    tail_left: u32,
    chain: Vec<EffectState>,
}

/// The sampler: sixteen pads, a bus chain, and a master gain.
pub struct Engine {
    sample_rate: f32,
    pads: [Pad; PAD_COUNT],
    bus: EffectChain,
    master: f32,
    scratch: Vec<f32>,
}

impl Engine {
    /// Create an idle engine. `sample_rate` below 8 kHz or non-finite becomes 44.1 kHz.
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = if sample_rate.is_finite() && sample_rate >= 8_000.0 {
            sample_rate
        } else {
            44_100.0
        };
        Self {
            sample_rate,
            pads: std::array::from_fn(|_| Pad::empty()),
            bus: EffectChain::new(),
            master: 1.0,
            scratch: Vec::new(),
        }
    }

    /// Reserve the scratch buffer so the first audio callback does not allocate.
    pub fn prepare(&mut self, max_frames: usize) {
        let samples = max_frames.saturating_mul(2);
        if self.scratch.len() < samples {
            self.scratch.resize(samples, 0.0);
        }
    }

    /// Engine sample rate in hertz.
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Master linear gain.
    pub fn master(&self) -> f32 {
        self.master
    }

    /// Set the master linear gain. Non-finite values are ignored.
    pub fn set_master(&mut self, master: f32) {
        if master.is_finite() {
            self.master = master.clamp(0.0, 2.0);
        }
    }

    /// Replace every pad with a built-in demo sound.
    pub fn load_demo_bank(&mut self) {
        let sr = self.sample_rate;
        let plan: [(&str, &str, f32); PAD_COUNT] = [
            ("kick", "Kick", 0.0),
            ("snare", "Snare", 0.0),
            ("hat", "Hat", 0.0),
            ("hat_open", "Open Hat", 0.0),
            ("tom", "Tom", 0.0),
            ("tom_hi", "Tom Hi", 0.0),
            ("bass", "Bass", 0.0),
            ("perc", "Perc", 0.0),
            ("chord", "Chord", 0.0),
            ("pluck", "Pluck", 0.0),
            ("zap", "Zap", 0.0),
            ("rim", "Rim", 0.0),
            ("kick", "Kick +7", 7.0),
            ("snare", "Snare -5", -5.0),
            ("bass", "Bass +12", 12.0),
            ("chord", "Chord -12", -12.0),
        ];
        for (index, (demo, name, pitch)) in plan.into_iter().enumerate() {
            if let Some(sample) = AudioBuffer::demo(demo, sr) {
                let _ = self.load_pad(index, sample, name);
                let _ = self.set_pad_pitch(index, pitch);
            }
        }
    }

    /// Load `sample` onto a pad, resampling to the engine rate.
    ///
    /// The insert chain is kept and reset, so a new sample does not inherit a tail.
    pub fn load_pad(
        &mut self,
        index: usize,
        sample: AudioBuffer,
        name: impl Into<String>,
    ) -> Result<(), Error> {
        let pad = self
            .pads
            .get_mut(index)
            .ok_or(Error::PadOutOfRange { index })?;
        let stereo = sample.to_stereo(self.sample_rate)?;
        pad.sample = Some(stereo);
        pad.name = name.into();
        pad.phase = Phase::Off;
        pad.pos = 0.0;
        pad.tail_left = 0;
        pad.chain.reset();
        Ok(())
    }

    /// Remove the sample and silence the voice.
    pub fn clear_pad(&mut self, index: usize) -> Result<(), Error> {
        let pad = self
            .pads
            .get_mut(index)
            .ok_or(Error::PadOutOfRange { index })?;
        *pad = Pad::empty();
        Ok(())
    }

    /// A clone of the loaded sample, already at the engine rate.
    pub fn pad_sample(&self, index: usize) -> Result<Option<AudioBuffer>, Error> {
        let pad = self.pads.get(index).ok_or(Error::PadOutOfRange { index })?;
        Ok(pad.sample.clone())
    }

    /// Start the pad from its start point. Returns false if the pad is empty
    /// or the index is out of range.
    pub fn trigger(&mut self, index: usize) -> bool {
        let Some(pad) = self.pads.get_mut(index) else {
            return false;
        };
        let Some(sample) = pad.sample.as_ref() else {
            return false;
        };
        let frames = sample.frames() as f64;
        pad.pos = (pad.start.clamp(0.0, 1.0) as f64) * frames;
        pad.phase = Phase::Play;
        pad.tail_left = (self.sample_rate * 2.0) as u32;
        true
    }

    /// Silence every voice and clear effect tails.
    pub fn stop_all(&mut self) {
        for pad in &mut self.pads {
            pad.phase = Phase::Off;
            pad.chain.reset();
        }
        self.bus.reset();
    }

    /// Set the pad's linear volume.
    pub fn set_pad_volume(&mut self, index: usize, volume: f32) -> Result<(), Error> {
        let pad = self
            .pads
            .get_mut(index)
            .ok_or(Error::PadOutOfRange { index })?;
        if volume.is_finite() {
            pad.volume = volume.clamp(0.0, 2.0);
        }
        Ok(())
    }

    /// Set playback transpose in semitones.
    pub fn set_pad_pitch(&mut self, index: usize, semitones: f32) -> Result<(), Error> {
        let pad = self
            .pads
            .get_mut(index)
            .ok_or(Error::PadOutOfRange { index })?;
        if semitones.is_finite() {
            pad.pitch_semitones = semitones.clamp(-24.0, 24.0);
        }
        Ok(())
    }

    /// Set the normalized start and end points. They are ordered if crossed.
    pub fn set_pad_range(&mut self, index: usize, start: f32, end: f32) -> Result<(), Error> {
        let pad = self
            .pads
            .get_mut(index)
            .ok_or(Error::PadOutOfRange { index })?;
        if start.is_finite() && end.is_finite() {
            let mut start = start.clamp(0.0, 1.0);
            let mut end = end.clamp(0.0, 1.0);
            if end < start {
                std::mem::swap(&mut start, &mut end);
            }
            if end - start < 0.001 {
                end = (start + 0.001).min(1.0);
            }
            pad.start = start;
            pad.end = end;
        }
        Ok(())
    }

    /// Replace the pad insert chain. Delay and reverb state starts empty.
    pub fn set_pad_chain(&mut self, index: usize, desc: &ChainDesc) -> Result<(), Error> {
        let chain = desc.instantiate()?;
        let pad = self
            .pads
            .get_mut(index)
            .ok_or(Error::PadOutOfRange { index })?;
        pad.chain = chain;
        Ok(())
    }

    /// Edit one parameter of one insert without rebuilding the chain.
    pub fn set_pad_param(
        &mut self,
        index: usize,
        effect: usize,
        id: &str,
        value: f32,
    ) -> Result<(), Error> {
        self.pads
            .get_mut(index)
            .ok_or(Error::PadOutOfRange { index })?
            .chain
            .set_param(effect, id, value)
    }

    /// Bypass one insert on a pad.
    pub fn set_pad_bypass(
        &mut self,
        index: usize,
        effect: usize,
        bypassed: bool,
    ) -> Result<(), Error> {
        self.pads
            .get_mut(index)
            .ok_or(Error::PadOutOfRange { index })?
            .chain
            .set_bypassed(effect, bypassed)
    }

    /// Replace the bus chain.
    pub fn set_bus_chain(&mut self, desc: &ChainDesc) -> Result<(), Error> {
        self.bus = desc.instantiate()?;
        Ok(())
    }

    /// Edit one bus parameter without rebuilding the chain.
    pub fn set_bus_param(&mut self, effect: usize, id: &str, value: f32) -> Result<(), Error> {
        self.bus.set_param(effect, id, value)
    }

    /// Bypass one bus effect.
    pub fn set_bus_bypass(&mut self, effect: usize, bypassed: bool) -> Result<(), Error> {
        self.bus.set_bypassed(effect, bypassed)
    }

    /// Current bus description.
    pub fn bus_chain(&self) -> ChainDesc {
        self.bus.to_desc()
    }

    /// Display snapshot of one pad.
    pub fn pad_view(&self, index: usize) -> Result<PadView, Error> {
        self.pads
            .get(index)
            .map(|pad| pad.view(index))
            .ok_or(Error::PadOutOfRange { index })
    }

    /// Peak absolute amplitude in each of `bins` windows, for drawing.
    pub fn waveform(&self, index: usize, bins: usize) -> Result<Vec<f32>, Error> {
        let bins = bins.max(1);
        let pad = self.pads.get(index).ok_or(Error::PadOutOfRange { index })?;
        let Some(sample) = pad.sample.as_ref() else {
            return Ok(vec![0.0; bins]);
        };
        let frames = sample.frames().max(1);
        let mut out = vec![0.0; bins];
        for (bin, slot) in out.iter_mut().enumerate() {
            let start = bin * frames / bins;
            let end = ((bin + 1) * frames / bins).max(start + 1).min(frames);
            let mut peak = 0.0_f32;
            for frame in start..end {
                peak = peak.max(sample.data[frame * 2].abs());
                peak = peak.max(sample.data[frame * 2 + 1].abs());
            }
            *slot = peak;
        }
        Ok(out)
    }

    /// Snapshot voice positions and effect memory.
    pub(crate) fn capture_playback(&self) -> PlaybackState {
        PlaybackState {
            pads: self
                .pads
                .iter()
                .map(|pad| PadVoice {
                    phase: match pad.phase {
                        Phase::Off => 0,
                        Phase::Play => 1,
                        Phase::Tail => 2,
                    },
                    pos_bits: pad.pos.to_bits(),
                    tail_left: pad.tail_left,
                    chain: pad.chain.capture_state(),
                })
                .collect(),
            bus: self.bus.capture_state(),
        }
    }

    /// Restore a snapshot from this engine's chain layout.
    pub(crate) fn restore_playback(&mut self, state: &PlaybackState) -> Result<(), Error> {
        if state.pads.len() != PAD_COUNT {
            return Err(Error::StateMismatch {
                effect: "engine".into(),
                reason: format!(
                    "expected {PAD_COUNT} pads, snapshot has {}",
                    state.pads.len()
                ),
            });
        }
        for (pad, voice) in self.pads.iter_mut().zip(&state.pads) {
            pad.phase = match voice.phase {
                0 => Phase::Off,
                1 => Phase::Play,
                2 => Phase::Tail,
                other => {
                    return Err(Error::StateMismatch {
                        effect: "engine".into(),
                        reason: format!("unknown voice phase {other}"),
                    });
                }
            };
            pad.pos = f64::from_bits(voice.pos_bits);
            pad.tail_left = voice.tail_left;
            pad.chain.restore_state(&voice.chain)?;
        }
        self.bus.restore_state(&state.bus)?;
        Ok(())
    }

    /// Render interleaved stereo into `output`.
    ///
    /// The slice length must be even. Active pads are mixed, then the bus
    /// chain runs, then the master gain is applied. Non-finite samples are
    /// replaced with silence.
    pub fn process(&mut self, output: &mut [f32]) {
        let frames = output.len() / 2;
        output[..frames * 2].fill(0.0);
        if self.scratch.len() < frames * 2 {
            self.scratch.resize(frames * 2, 0.0);
        }
        for index in 0..PAD_COUNT {
            if self.pads[index].phase == Phase::Off {
                continue;
            }
            render_pad(
                &mut self.pads[index],
                &mut self.scratch[..frames * 2],
                frames,
            );
            let sr = self.sample_rate;
            self.pads[index]
                .chain
                .process(&mut self.scratch[..frames * 2], sr);
            let volume = self.pads[index].volume;
            for (out, src) in output.iter_mut().zip(self.scratch.iter()) {
                *out += *src * volume;
            }
        }
        self.bus
            .process(&mut output[..frames * 2], self.sample_rate);
        let master = self.master;
        for sample in &mut output[..frames * 2] {
            let scaled = *sample * master;
            *sample = if scaled.is_finite() { scaled } else { 0.0 };
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new(44_100.0)
    }
}

fn render_pad(pad: &mut Pad, scratch: &mut [f32], frames: usize) {
    scratch.fill(0.0);
    let total = match pad.sample.as_ref() {
        Some(sample) if sample.frames() > 0 => sample.frames(),
        _ => {
            pad.phase = Phase::Off;
            return;
        }
    };
    let total_f = total as f64;
    let start = (pad.start as f64) * total_f;
    let mut end = (pad.end as f64) * total_f;
    if end < start + 1.0 {
        end = (start + 1.0).min(total_f);
    }
    let rate = 2.0_f64.powf(pad.pitch_semitones as f64 / 12.0);
    let chain_empty = pad.chain.is_empty();
    for frame in 0..frames {
        match pad.phase {
            Phase::Off => break,
            Phase::Play => {
                if pad.pos < start {
                    pad.pos = start;
                }
                if pad.pos >= end {
                    if chain_empty {
                        pad.phase = Phase::Off;
                        break;
                    }
                    pad.phase = Phase::Tail;
                    continue;
                }
                let (left, right) = {
                    let data = &pad.sample.as_ref().expect("sample checked").data;
                    let i0 = (pad.pos.floor() as usize).min(total - 1);
                    let i1 = (i0 + 1).min(total - 1);
                    let frac = (pad.pos - i0 as f64) as f32;
                    let left = data[i0 * 2] * (1.0 - frac) + data[i1 * 2] * frac;
                    let right = data[i0 * 2 + 1] * (1.0 - frac) + data[i1 * 2 + 1] * frac;
                    (left, right)
                };
                scratch[frame * 2] = left;
                scratch[frame * 2 + 1] = right;
                pad.pos += rate;
            }
            Phase::Tail => {
                if pad.tail_left == 0 {
                    pad.phase = Phase::Off;
                    break;
                }
                pad.tail_left -= 1;
            }
        }
    }
}
