//! Offline renderer for an [`Arrangement`](super::Arrangement).
//!
//! `render(plan, start..end)` is a pure function of the plan and the frame
//! range. Stateful effects make that non-obvious: a filter memory at frame
//! 10_000 depends on every earlier sample. The renderer keeps one recurrence
//! and reaches any frame by restoring a checkpoint and replaying.
//!
//! Checkpoint `C` is the engine state *before* frame `C`. `C` is always a
//! multiple of [`CHECKPOINT_FRAMES`](super::CHECKPOINT_FRAMES), including 0.
//! The snapshot is an exact copy of voice positions and effect memory. It is
//! derived from the plan, so a cache only saves work. Rendering slices in any
//! order, or one sample at a time, follows the same per-frame transition and
//! therefore writes the same bits.

use std::collections::BTreeMap;

use crate::buffer::AudioBuffer;
use crate::plan::{invalid, Arrangement, PlanTime, CHECKPOINT_FRAMES};
use crate::sampler::{Engine, PlaybackState, PAD_COUNT};
use crate::Error;

use super::SampleSource;

/// Plays an arrangement from checkpoints so any frame range can be sought.
///
/// A renderer may cache checkpoints. Two renderers, and any order of calls on
/// one renderer, still agree on every sample.
pub struct PlanRenderer {
    plan: Arrangement,
    engine: Engine,
    triggers: Vec<Fired>,
    checkpoints: BTreeMap<u64, Checkpoint>,
    /// Frame the engine is about to process.
    cursor: u64,
    voice_gain: [f32; PAD_COUNT],
    duration: u64,
}

#[derive(Clone)]
struct Checkpoint {
    playback: PlaybackState,
    voice_gain: [f32; PAD_COUNT],
}

struct Fired {
    frame: u64,
    pad: usize,
    gain: f32,
}

impl PlanRenderer {
    /// Validate `plan`, load its samples, and remember the state at frame 0.
    pub fn new(plan: &Arrangement) -> Result<Self, Error> {
        plan.validate()?;
        let duration = plan.duration_frames()?;
        let engine = load_engine(plan)?;
        let mut triggers = Vec::with_capacity(plan.triggers.len());
        for trigger in &plan.triggers {
            triggers.push(Fired {
                frame: trigger.at.to_frame(plan.tempo, plan.sample_rate)?,
                pad: trigger.pad,
                gain: trigger.gain.clamp(0.0, 2.0),
            });
        }
        triggers.sort_by(|left, right| left.frame.cmp(&right.frame));
        let voice_gain = [1.0; PAD_COUNT];
        let mut renderer = Self {
            plan: plan.clone(),
            engine,
            triggers,
            checkpoints: BTreeMap::new(),
            cursor: 0,
            voice_gain,
            duration,
        };
        renderer.checkpoints.insert(0, renderer.capture());
        Ok(renderer)
    }

    /// Nominal length of the arrangement, in frames.
    pub fn duration_frames(&self) -> u64 {
        self.duration
    }

    /// Sample rate of the rendered buffer.
    pub fn sample_rate(&self) -> u32 {
        self.plan.sample_rate
    }

    /// Render the half-open frame range `[start, end)`.
    ///
    /// The range does not have to sit inside the arrangement. Frames at or
    /// past the duration keep running the same recurrence, so a tail can be
    /// bounced. The range must be non-empty and at most ten minutes long.
    pub fn render(&mut self, start: u64, end: u64) -> Result<AudioBuffer, Error> {
        if end < start {
            return Err(invalid("render end is before the start"));
        }
        if start == end {
            return Err(invalid("render range is empty"));
        }
        let count = end - start;
        let limit = u64::from(self.plan.sample_rate).saturating_mul(600);
        if count > limit {
            return Err(invalid("render range is longer than 10 minutes"));
        }
        self.seek(start)?;
        let frames = count as usize;
        let mut data = vec![0.0_f32; frames * 2];
        for frame in 0..frames {
            let sample = self.step()?;
            data[frame * 2] = sample[0];
            data[frame * 2 + 1] = sample[1];
        }
        AudioBuffer::new(self.plan.sample_rate as f32, 2, data)
    }

    fn seek(&mut self, frame: u64) -> Result<(), Error> {
        if self.cursor == frame {
            return Ok(());
        }
        let checkpoint = frame - frame % CHECKPOINT_FRAMES;
        self.materialize(checkpoint)?;
        self.install(checkpoint)?;
        while self.cursor < frame {
            self.step()?;
        }
        Ok(())
    }

    fn materialize(&mut self, frame: u64) -> Result<(), Error> {
        if self.checkpoints.contains_key(&frame) {
            return Ok(());
        }
        let origin = self
            .checkpoints
            .range(..=frame)
            .next_back()
            .map(|(key, _)| *key)
            .ok_or_else(|| invalid("missing origin checkpoint"))?;
        self.install(origin)?;
        while self.cursor < frame {
            self.step()?;
        }
        if !self.checkpoints.contains_key(&frame) {
            return Err(invalid("checkpoint was not recorded"));
        }
        Ok(())
    }

    fn install(&mut self, frame: u64) -> Result<(), Error> {
        let checkpoint = self
            .checkpoints
            .get(&frame)
            .cloned()
            .ok_or_else(|| invalid("missing checkpoint"))?;
        self.engine.restore_playback(&checkpoint.playback)?;
        self.voice_gain = checkpoint.voice_gain;
        self.cursor = frame;
        Ok(())
    }

    fn capture(&self) -> Checkpoint {
        Checkpoint {
            playback: self.engine.capture_playback(),
            voice_gain: self.voice_gain,
        }
    }

    fn step(&mut self) -> Result<[f32; 2], Error> {
        let frame = self.cursor;
        let seconds = frame as f64 / f64::from(self.plan.sample_rate);
        for trigger in triggers_at(&self.triggers, frame) {
            let _ = self.engine.trigger(trigger.pad);
            self.voice_gain[trigger.pad] = trigger.gain;
        }
        self.apply(seconds)?;
        let mut buf = [0.0; 2];
        self.engine.process(&mut buf);
        self.cursor = frame + 1;
        if self.cursor % CHECKPOINT_FRAMES == 0 && !self.checkpoints.contains_key(&self.cursor) {
            self.checkpoints.insert(self.cursor, self.capture());
        }
        Ok(buf)
    }

    fn apply(&mut self, seconds: f64) -> Result<(), Error> {
        let tempo = self.plan.tempo;
        let pad_count = self.plan.pads.len();
        for index in 0..pad_count {
            let pad_index = self.plan.pads[index].pad;
            let gain = self.voice_gain[pad_index];
            let volume = self.plan.pads[index].volume.sample(seconds, tempo)? * gain;
            let pitch = self.plan.pads[index].pitch.sample(seconds, tempo)?;
            self.engine.set_pad_volume(pad_index, volume)?;
            self.engine.set_pad_pitch(pad_index, pitch)?;
            let automation_count = self.plan.pads[index].automation.len();
            for slot in 0..automation_count {
                let effect = self.plan.pads[index].automation[slot].effect;
                let value = self.plan.pads[index].automation[slot]
                    .track
                    .sample(seconds, tempo)?;
                let param = self.plan.pads[index].automation[slot].param.clone();
                self.engine
                    .set_pad_param(pad_index, effect, &param, value)?;
            }
        }
        let bus_count = self.plan.bus.automation.len();
        for slot in 0..bus_count {
            let effect = self.plan.bus.automation[slot].effect;
            let value = self.plan.bus.automation[slot]
                .track
                .sample(seconds, tempo)?;
            let param = self.plan.bus.automation[slot].param.clone();
            self.engine.set_bus_param(effect, &param, value)?;
        }
        let master = self.plan.master.sample(seconds, tempo)?;
        self.engine.set_master(master);
        Ok(())
    }
}

fn triggers_at(triggers: &[Fired], frame: u64) -> &[Fired] {
    let start = triggers.partition_point(|trigger| trigger.frame < frame);
    let end = triggers.partition_point(|trigger| trigger.frame <= frame);
    &triggers[start..end]
}

fn load_engine(plan: &Arrangement) -> Result<Engine, Error> {
    let mut engine = Engine::new(plan.sample_rate as f32);
    for pad in &plan.pads {
        let (buffer, name) = match &pad.sample {
            SampleSource::Demo { name } => {
                let buffer = AudioBuffer::demo(name, plan.sample_rate as f32)
                    .ok_or_else(|| invalid(format!("unknown demo '{name}'")))?;
                (buffer, name.clone())
            }
            SampleSource::Pcm {
                sample_rate,
                channels,
                samples,
            } => {
                let buffer = AudioBuffer::new(*sample_rate, u16::from(*channels), samples.clone())?;
                (buffer, format!("pad {}", pad.pad))
            }
        };
        engine.load_pad(pad.pad, buffer, name)?;
        engine.set_pad_range(pad.pad, pad.start, pad.end)?;
        engine.set_pad_chain(pad.pad, &pad.chain)?;
    }
    engine.set_bus_chain(&plan.bus.chain)?;
    Ok(engine)
}

/// Render `[start, end)` with a fresh renderer.
///
/// This is the function an agent should call. It does not consult any earlier
/// render, and it does not touch the filesystem.
pub fn render_arrangement(
    plan: &Arrangement,
    start_frame: u64,
    end_frame: u64,
) -> Result<AudioBuffer, Error> {
    PlanRenderer::new(plan)?.render(start_frame, end_frame)
}

/// Convert a plan time to a frame index using the plan tempo and sample rate.
pub fn frame_at(plan: &Arrangement, time: PlanTime) -> Result<u64, Error> {
    time.to_frame(plan.tempo, plan.sample_rate)
}
