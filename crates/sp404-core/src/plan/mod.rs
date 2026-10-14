//! Declarative arrangements.
//!
//! A plan is data: pads, triggers, effect chains, and automation. A Rust
//! program can build one with [`PlanBuilder`], or an agent can write the same
//! JSON and diff it. [`Arrangement::render`] turns any frame range into
//! samples. See `render` for why those samples do not depend on chunking.
//!
//! ```json
//! {
//!   "version": 1,
//!   "sample_rate": 44100,
//!   "tempo": 100,
//!   "duration": { "beats": 4 },
//!   "triggers": [{ "pad": 0, "at": { "beats": 0 } }],
//!   "pads": [{ "pad": 0, "sample": { "kind": "demo", "name": "kick" } }]
//! }
//! ```

mod render;
mod track;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::buffer::AudioBuffer;
use crate::chain::{ChainDesc, EffectChain, EffectDesc};
use crate::effect::effect_schema;
use crate::param::EffectSchema;
use crate::sampler::PAD_COUNT;
use crate::Error;

pub use render::{frame_at, render_arrangement, PlanRenderer};

/// Arrangement document version written by [`Arrangement::to_json`].
pub const ARRANGEMENT_VERSION: u32 = 1;

/// Frames between deterministic state checkpoints.
///
/// Rendering `[start, end)` restores the checkpoint at
/// `start - start % CHECKPOINT_FRAMES` and replays from there. The interval is
/// part of the algorithm: every renderer uses the same boundaries.
pub const CHECKPOINT_FRAMES: u64 = 4_096;

/// A whole piece: samples, triggers, chains, and automation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Arrangement {
    /// Optional label for humans and diffs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Document version. Only [`ARRANGEMENT_VERSION`] is accepted.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Render sample rate in hertz.
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    /// Beats per minute, used by beat times and per-beat LFOs.
    #[serde(default = "default_tempo")]
    pub tempo: f64,
    /// Nominal length. The default render covers `[0, duration)`.
    pub duration: PlanTime,
    /// Master linear gain. Defaults to unity.
    #[serde(default = "unity_track")]
    pub master: Track,
    /// Pad programs. Each pad index appears at most once.
    #[serde(default)]
    pub pads: Vec<PadPlan>,
    /// Chain and automation after the pad sum.
    #[serde(default)]
    pub bus: BusPlan,
    /// Pad triggers. Order among equal times is the order in this list.
    #[serde(default)]
    pub triggers: Vec<Trigger>,
}

/// One pad's sample, playback, insert chain, and parameter automation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PadPlan {
    /// Pad index, `0..16`.
    pub pad: usize,
    /// Where the sample comes from.
    pub sample: SampleSource,
    /// Linear gain over time, before the trigger gain.
    #[serde(default = "unity_track")]
    pub volume: Track,
    /// Playback transpose in semitones.
    #[serde(default = "zero_track")]
    pub pitch: Track,
    /// Normalized start point.
    #[serde(default)]
    pub start: f32,
    /// Normalized end point.
    #[serde(default = "default_end")]
    pub end: f32,
    /// Insert chain. The same JSON shape as a standalone [`ChainDesc`].
    #[serde(default)]
    pub chain: ChainDesc,
    /// Curves for parameters of `chain`, addressed by slot index.
    #[serde(default)]
    pub automation: Vec<Automation>,
}

/// The bus chain and its automation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BusPlan {
    /// Effects after the pad sum.
    #[serde(default)]
    pub chain: ChainDesc,
    /// Curves for parameters of `chain`.
    #[serde(default)]
    pub automation: Vec<Automation>,
}

/// A sample loaded onto a pad.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SampleSource {
    /// A built-in demo, rendered at the arrangement sample rate.
    Demo {
        /// Name from [`AudioBuffer::demo_names`](crate::AudioBuffer::demo_names).
        name: String,
    },
    /// Interleaved PCM carried in the plan itself.
    Pcm {
        /// Sample rate in hertz.
        sample_rate: f32,
        /// 1 or 2.
        channels: u8,
        /// Interleaved samples.
        samples: Vec<f32>,
    },
    /// A WAV path. The core does not open it; the host inlines the samples.
    File {
        /// Path the host can read. Relative paths are resolved by that host.
        path: String,
    },
}

/// One pad hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trigger {
    /// Pad to trigger.
    pub pad: usize,
    /// When the hit starts. The time is rounded to the nearest frame.
    pub at: PlanTime,
    /// Linear gain for this voice until the next trigger on the same pad.
    #[serde(default = "unity_gain")]
    pub gain: f32,
}

/// A curve bound to one effect parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Automation {
    /// Slot index in the pad or bus chain.
    pub effect: usize,
    /// Parameter id from the effect schema.
    pub param: String,
    /// Curve. It replaces the chain's static value for that parameter.
    pub track: Track,
}

/// A moment on the arrangement clock.
///
/// JSON is `{ "seconds": 1.5 }` or `{ "beats": 2 }`, never both.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "TimeFields", into = "TimeFields")]
pub enum PlanTime {
    /// Seconds from the start of the arrangement.
    Seconds(f64),
    /// Beats at the arrangement tempo.
    Beats(f64),
}

/// How to travel from the previous keyframe into this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    /// Constant slope.
    #[default]
    Linear,
    /// Stay on the previous value until this key, then jump.
    Hold,
    /// Quintic smootherstep.
    Smooth,
    /// Cubic ease in.
    In,
    /// Cubic ease out.
    Out,
    /// Smoothstep.
    InOut,
}

/// One point on a [`Track::Keyframes`] curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Time of this point.
    pub at: PlanTime,
    /// Parameter value in real units.
    pub value: f32,
    /// Curve used to arrive here from the previous point.
    #[serde(default)]
    pub ease: Ease,
}

/// Oscillator shape for [`Track::Lfo`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LfoShape {
    /// Sine, starting at zero when the phase is zero.
    Sine,
    /// Bipolar triangle.
    Triangle,
    /// Bipolar square.
    Square,
    /// One deterministic value per cycle, held until the next cycle.
    Hold,
}

/// A pure function from time to a parameter value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Track {
    /// The same value at every time.
    Constant {
        /// Real-unit value.
        value: f32,
    },
    /// Piecewise curve through sorted keys. The ends hold.
    Keyframes {
        /// Points, strictly increasing in time.
        keys: Vec<Keyframe>,
    },
    /// Periodic modulation around `center`.
    Lfo {
        /// Wave shape.
        shape: LfoShape,
        /// Cycles per second. Set this or `per_beat`, not both.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rate_hz: Option<f64>,
        /// Cycles per beat. Set this or `rate_hz`, not both.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        per_beat: Option<f64>,
        /// Peak deviation from `center`.
        depth: f32,
        /// Value at wave zero.
        center: f32,
        /// Cycle offset. `0.25` is a quarter-cycle.
        #[serde(default)]
        phase: f64,
    },
    /// Analytic damped spring released at `at` with zero velocity.
    ///
    /// `bounce` 0 is critically damped. `response` is the settle time. Before
    /// `at` the value is `from`.
    Spring {
        /// Value at the release, and before it.
        from: f32,
        /// Rest value.
        to: f32,
        /// When the spring is released.
        at: PlanTime,
        /// Approximate settle time.
        response: PlanTime,
        /// 0 is critical damping. 1 is underdamped.
        #[serde(default)]
        bounce: f32,
    },
}

/// What an automation track drives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Master linear gain.
    Master,
    /// Pad linear gain.
    Volume {
        /// Pad index.
        pad: usize,
    },
    /// Pad transpose in semitones.
    Pitch {
        /// Pad index.
        pad: usize,
    },
    /// One parameter of a pad insert.
    Pad {
        /// Pad index.
        pad: usize,
        /// Slot in the pad chain.
        effect: usize,
        /// Parameter id.
        param: String,
    },
    /// One parameter of the bus chain.
    Bus {
        /// Slot in the bus chain.
        effect: usize,
        /// Parameter id.
        param: String,
    },
}

/// Builds an [`Arrangement`] in code. [`PlanBuilder::finish`] validates it.
#[derive(Debug, Clone)]
pub struct PlanBuilder {
    name: String,
    sample_rate: u32,
    tempo: f64,
    duration: Option<PlanTime>,
    master: Track,
    pads: BTreeMap<usize, PadDraft>,
    bus: BusPlan,
    triggers: Vec<Trigger>,
}

#[derive(Debug, Clone)]
struct PadDraft {
    sample: Option<SampleSource>,
    volume: Track,
    pitch: Track,
    start: f32,
    end: f32,
    chain: ChainDesc,
    automation: Vec<Automation>,
}

impl Arrangement {
    /// Parse JSON and validate it.
    pub fn from_json(text: &str) -> Result<Self, Error> {
        let plan: Arrangement = serde_json::from_str(text)
            .map_err(|err| Error::InvalidPlan(format!("invalid JSON: {err}")))?;
        plan.validate()?;
        Ok(plan)
    }

    /// Pretty JSON, suitable for an agent to diff.
    pub fn to_json(&self) -> Result<String, Error> {
        serde_json::to_string_pretty(self).map_err(|err| Error::InvalidPlan(err.to_string()))
    }

    /// Check ids, ranges, times, and chain parameters.
    pub fn validate(&self) -> Result<(), Error> {
        if self.version != ARRANGEMENT_VERSION {
            return Err(invalid(format!(
                "unsupported version {} (this crate reads version {ARRANGEMENT_VERSION})",
                self.version
            )));
        }
        if !(8_000..=192_000).contains(&self.sample_rate) {
            return Err(invalid("sample rate must be from 8000 to 192000 Hz"));
        }
        if !self.tempo.is_finite() || self.tempo <= 0.0 || self.tempo > 1_000.0 {
            return Err(invalid(
                "tempo must be a finite bpm above 0 and at most 1000",
            ));
        }
        let frames = self.duration_frames()?;
        if frames == 0 {
            return Err(invalid("duration is empty"));
        }
        if frames > u64::from(self.sample_rate).saturating_mul(600) {
            return Err(invalid("duration is longer than 10 minutes"));
        }
        self.master.validate("master", self.tempo)?;

        let mut seen = [false; PAD_COUNT];
        for pad in &self.pads {
            if pad.pad >= PAD_COUNT {
                return Err(Error::PadOutOfRange { index: pad.pad });
            }
            if seen[pad.pad] {
                return Err(invalid(format!("pad {} is listed twice", pad.pad)));
            }
            seen[pad.pad] = true;
            validate_sample(&pad.sample)?;
            if !pad.start.is_finite() || !pad.end.is_finite() {
                return Err(invalid(format!(
                    "pad {} start or end is not finite",
                    pad.pad
                )));
            }
            pad.volume
                .validate(&format!("pad {} volume", pad.pad), self.tempo)?;
            pad.pitch
                .validate(&format!("pad {} pitch", pad.pad), self.tempo)?;
            let chain = pad.chain.instantiate()?;
            validate_automation(
                &pad.automation,
                chain_ids(&chain),
                &format!("pad {}", pad.pad),
                self.tempo,
            )?;
        }

        let bus = self.bus.chain.instantiate()?;
        validate_automation(&self.bus.automation, chain_ids(&bus), "bus", self.tempo)?;

        for (index, trigger) in self.triggers.iter().enumerate() {
            if trigger.pad >= PAD_COUNT || !seen[trigger.pad] {
                return Err(invalid(format!(
                    "trigger {index} targets pad {} which has no sample",
                    trigger.pad
                )));
            }
            if !trigger.gain.is_finite() {
                return Err(invalid(format!("trigger {index} gain is not finite")));
            }
            let frame = trigger.at.to_frame(self.tempo, self.sample_rate)?;
            if frame >= frames {
                return Err(invalid(format!(
                    "trigger {index} is at or past the end of the arrangement"
                )));
            }
        }
        Ok(())
    }

    /// Nominal length in frames, rounding the duration to the nearest frame.
    pub fn duration_frames(&self) -> Result<u64, Error> {
        self.duration.to_frame(self.tempo, self.sample_rate)
    }

    /// Render `[start_frame, end_frame)` from a fresh renderer.
    ///
    /// The samples match any other chunking of the same plan. See the `plan`
    /// module docs and `render`.
    pub fn render(&self, start_frame: u64, end_frame: u64) -> Result<AudioBuffer, Error> {
        render_arrangement(self, start_frame, end_frame)
    }
}

impl PlanTime {
    /// A time in seconds from the start.
    pub fn seconds(seconds: f64) -> Self {
        Self::Seconds(seconds)
    }

    /// A time in beats at the arrangement tempo.
    pub fn beats(beats: f64) -> Self {
        Self::Beats(beats)
    }

    /// Convert to seconds. Beat times use `tempo_bpm`.
    pub fn to_seconds(self, tempo_bpm: f64) -> Result<f64, Error> {
        let seconds = match self {
            PlanTime::Seconds(seconds) => seconds,
            PlanTime::Beats(beats) => {
                if !tempo_bpm.is_finite() || tempo_bpm <= 0.0 {
                    return Err(invalid("tempo must be positive to convert beats"));
                }
                beats * 60.0 / tempo_bpm
            }
        };
        if !seconds.is_finite() {
            return Err(invalid("time is not finite"));
        }
        Ok(seconds)
    }

    /// Nearest frame at `sample_rate`. Negative times are rejected.
    ///
    /// Halfway cases round away from zero, matching `f64::round`.
    pub fn to_frame(self, tempo_bpm: f64, sample_rate: u32) -> Result<u64, Error> {
        if sample_rate == 0 {
            return Err(invalid("sample rate is zero"));
        }
        let frames = self.to_seconds(tempo_bpm)? * f64::from(sample_rate);
        if !(0.0..=1.0e12).contains(&frames) {
            return Err(invalid("time does not land on a usable frame"));
        }
        Ok(frames.round() as u64)
    }
}

impl Keyframe {
    /// A linear key at `at`.
    pub fn at(at: PlanTime, value: f32) -> Self {
        Self {
            at,
            value,
            ease: Ease::Linear,
        }
    }

    /// Set the curve used to arrive at this key.
    pub fn ease(mut self, ease: Ease) -> Self {
        self.ease = ease;
        self
    }
}

impl Track {
    /// A constant value.
    pub fn constant(value: f32) -> Self {
        Self::Constant { value }
    }

    /// A curve through `keys`, which must be strictly increasing in time.
    pub fn keyframes(keys: impl IntoIterator<Item = Keyframe>) -> Self {
        Self::Keyframes {
            keys: keys.into_iter().collect(),
        }
    }

    /// Sine, triangle, square, or sample-and-hold at `rate_hz`.
    pub fn lfo_hz(shape: LfoShape, rate_hz: f64, depth: f32, center: f32) -> Self {
        Self::Lfo {
            shape,
            rate_hz: Some(rate_hz),
            per_beat: None,
            depth,
            center,
            phase: 0.0,
        }
    }

    /// The same LFO, with the rate in cycles per beat.
    pub fn lfo_beats(shape: LfoShape, per_beat: f64, depth: f32, center: f32) -> Self {
        Self::Lfo {
            shape,
            rate_hz: None,
            per_beat: Some(per_beat),
            depth,
            center,
            phase: 0.0,
        }
    }

    /// Set the LFO cycle offset. `0.25` is a quarter turn.
    pub fn phase(mut self, phase: f64) -> Self {
        if let Self::Lfo { phase: slot, .. } = &mut self {
            *slot = phase;
        }
        self
    }

    /// Critically damped spring from `from` to `to`, released at `at`.
    pub fn spring(from: f32, to: f32, at: PlanTime, response: PlanTime) -> Self {
        Self::Spring {
            from,
            to,
            at,
            response,
            bounce: 0.0,
        }
    }

    /// Set spring bounce. 0 is critical damping, 1 is underdamped.
    pub fn bounce(mut self, bounce: f32) -> Self {
        if let Self::Spring { bounce: slot, .. } = &mut self {
            *slot = bounce;
        }
        self
    }
}

impl Target {
    /// Master gain.
    pub fn master() -> Self {
        Self::Master
    }

    /// Pad volume.
    pub fn volume(pad: usize) -> Self {
        Self::Volume { pad }
    }

    /// Pad pitch in semitones.
    pub fn pitch(pad: usize) -> Self {
        Self::Pitch { pad }
    }

    /// A parameter on a pad insert.
    pub fn pad(pad: usize, effect: usize, param: impl Into<String>) -> Self {
        Self::Pad {
            pad,
            effect,
            param: param.into(),
        }
    }

    /// A parameter on the bus chain.
    pub fn bus(effect: usize, param: impl Into<String>) -> Self {
        Self::Bus {
            effect,
            param: param.into(),
        }
    }
}

impl PlanBuilder {
    /// Start an empty plan at `sample_rate` and `tempo_bpm`.
    pub fn new(sample_rate: u32, tempo_bpm: f64) -> Self {
        Self {
            name: String::new(),
            sample_rate,
            tempo: tempo_bpm,
            duration: None,
            master: unity_track(),
            pads: BTreeMap::new(),
            bus: BusPlan::default(),
            triggers: Vec::new(),
        }
    }

    /// Set the arrangement name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set the nominal length.
    pub fn duration(mut self, time: PlanTime) -> Self {
        self.duration = Some(time);
        self
    }

    /// Replace the master-gain track.
    pub fn master(mut self, track: Track) -> Self {
        self.master = track;
        self
    }

    /// Load a built-in demo onto `pad`.
    pub fn pad_demo(mut self, pad: usize, name: impl Into<String>) -> Self {
        self.draft(pad).sample = Some(SampleSource::Demo { name: name.into() });
        self
    }

    /// Point `pad` at a WAV path. The core will not open it.
    pub fn pad_file(mut self, pad: usize, path: impl Into<String>) -> Self {
        self.draft(pad).sample = Some(SampleSource::File { path: path.into() });
        self
    }

    /// Embed `buffer` in the plan.
    pub fn pad_pcm(mut self, pad: usize, buffer: &AudioBuffer) -> Self {
        self.draft(pad).sample = Some(SampleSource::Pcm {
            sample_rate: buffer.sample_rate,
            channels: buffer.channels as u8,
            samples: buffer.data.clone(),
        });
        self
    }

    /// Set the pad's normalized start and end.
    pub fn pad_range(mut self, pad: usize, start: f32, end: f32) -> Self {
        let draft = self.draft(pad);
        draft.start = start;
        draft.end = end;
        self
    }

    /// Replace the pad insert chain.
    pub fn pad_chain(mut self, pad: usize, chain: ChainDesc) -> Self {
        self.draft(pad).chain = chain;
        self
    }

    /// Replace the bus chain.
    pub fn bus_chain(mut self, chain: ChainDesc) -> Self {
        self.bus.chain = chain;
        self
    }

    /// Drive `target` with `track`. A repeated target replaces the earlier curve.
    pub fn automate(mut self, target: Target, track: Track) -> Self {
        match target {
            Target::Master => self.master = track,
            Target::Volume { pad } => self.draft(pad).volume = track,
            Target::Pitch { pad } => self.draft(pad).pitch = track,
            Target::Pad { pad, effect, param } => {
                let automation = &mut self.draft(pad).automation;
                if let Some(existing) = automation
                    .iter_mut()
                    .find(|item| item.effect == effect && item.param == param)
                {
                    existing.track = track;
                } else {
                    automation.push(Automation {
                        effect,
                        param,
                        track,
                    });
                }
            }
            Target::Bus { effect, param } => {
                if let Some(existing) = self
                    .bus
                    .automation
                    .iter_mut()
                    .find(|item| item.effect == effect && item.param == param)
                {
                    existing.track = track;
                } else {
                    self.bus.automation.push(Automation {
                        effect,
                        param,
                        track,
                    });
                }
            }
        }
        self
    }

    /// Trigger `pad` at `at` with unity gain.
    pub fn trigger(self, pad: usize, at: PlanTime) -> Self {
        self.trigger_gain(pad, at, 1.0)
    }

    /// Trigger `pad` at `at` with a linear voice gain.
    pub fn trigger_gain(mut self, pad: usize, at: PlanTime, gain: f32) -> Self {
        self.triggers.push(Trigger { pad, at, gain });
        self
    }

    /// Validate and return the arrangement.
    pub fn finish(self) -> Result<Arrangement, Error> {
        let Some(duration) = self.duration else {
            return Err(invalid("arrangement has no duration"));
        };
        let mut pads = Vec::with_capacity(self.pads.len());
        for (index, draft) in self.pads {
            let Some(sample) = draft.sample else {
                return Err(invalid(format!("pad {index} has no sample")));
            };
            pads.push(PadPlan {
                pad: index,
                sample,
                volume: draft.volume,
                pitch: draft.pitch,
                start: draft.start,
                end: draft.end,
                chain: draft.chain,
                automation: draft.automation,
            });
        }
        let plan = Arrangement {
            name: self.name,
            version: ARRANGEMENT_VERSION,
            sample_rate: self.sample_rate,
            tempo: self.tempo,
            duration,
            master: self.master,
            pads,
            bus: self.bus,
            triggers: self.triggers,
        };
        plan.validate()?;
        Ok(plan)
    }

    fn draft(&mut self, pad: usize) -> &mut PadDraft {
        self.pads.entry(pad).or_insert_with(|| PadDraft {
            sample: None,
            volume: unity_track(),
            pitch: zero_track(),
            start: 0.0,
            end: 1.0,
            chain: ChainDesc::default(),
            automation: Vec::new(),
        })
    }
}

/// Seconds from the start of the arrangement.
pub fn seconds(seconds: f64) -> PlanTime {
    PlanTime::seconds(seconds)
}

/// Beats at the arrangement tempo.
pub fn beats(beats: f64) -> PlanTime {
    PlanTime::beats(beats)
}

/// Build a chain from effect descriptions. Unknown ids fail when the plan is finished.
pub fn chain(effects: impl IntoIterator<Item = EffectDesc>) -> ChainDesc {
    ChainDesc {
        effects: effects.into_iter().collect(),
    }
}

/// One effect slot, for [`chain`].
pub fn effect<'a>(id: &str, params: impl IntoIterator<Item = (&'a str, f32)>) -> EffectDesc {
    EffectDesc {
        effect: id.to_string(),
        bypass: false,
        params: params
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    }
}

/// Machine-readable description of the arrangement document and its curves.
///
/// Every effect parameter in the catalog is automatable. So are pad volume,
/// pad pitch, and the master gain.
pub fn arrangement_catalog() -> serde_json::Value {
    serde_json::json!({
        "version": ARRANGEMENT_VERSION,
        "description": "An arrangement schedules pad triggers, effect chains, and automation. render(plan, start..end) is deterministic: any slice matches the same frames of a full render.",
        "checkpoint_frames": CHECKPOINT_FRAMES,
        "automatable": "Every effect parameter in this catalog, plus each pad's volume and pitch, and the master gain.",
        "time": {
            "seconds": { "seconds": 1.5 },
            "beats": { "beats": 2.0 }
        },
        "tracks": [
            { "kind": "constant", "fields": ["value"] },
            {
                "kind": "keyframes",
                "fields": ["keys"],
                "ease": ["linear", "hold", "smooth", "in", "out", "in_out"]
            },
            {
                "kind": "lfo",
                "shapes": ["sine", "triangle", "square", "hold"],
                "rate": "exactly one of rate_hz or per_beat"
            },
            {
                "kind": "spring",
                "fields": ["from", "to", "at", "response", "bounce"],
                "note": "Closed-form damped spring released at `at` with zero velocity. bounce 0 is critically damped."
            }
        ],
        "samples": [
            { "kind": "demo", "fields": ["name"] },
            { "kind": "pcm", "fields": ["sample_rate", "channels", "samples"] },
            {
                "kind": "file",
                "fields": ["path"],
                "note": "A WAV path. The core does not open it. The CLI inlines the file before rendering."
            }
        ]
    })
}

fn validate_sample(sample: &SampleSource) -> Result<(), Error> {
    match sample {
        SampleSource::Demo { name } => {
            if AudioBuffer::demo_names().contains(&name.as_str()) {
                Ok(())
            } else {
                Err(invalid(format!("unknown demo '{name}'")))
            }
        }
        SampleSource::Pcm {
            sample_rate,
            channels,
            samples,
        } => {
            if *channels != 1 && *channels != 2 {
                return Err(invalid(format!(
                    "pcm channel count {channels} is not 1 or 2"
                )));
            }
            if !sample_rate.is_finite() || *sample_rate < 1_000.0 {
                return Err(invalid(
                    "pcm sample rate must be finite and at least 1000 Hz",
                ));
            }
            if samples.is_empty() || samples.len() % *channels as usize != 0 {
                return Err(invalid("pcm must contain a whole number of frames"));
            }
            if samples.iter().any(|sample| !sample.is_finite()) {
                return Err(invalid("pcm contains a non-finite sample"));
            }
            Ok(())
        }
        SampleSource::File { path } => {
            if path.is_empty() || path.contains('\0') {
                Err(invalid("file sample path is empty"))
            } else {
                Ok(())
            }
        }
    }
}

fn chain_ids(chain: &EffectChain) -> Vec<Option<&'static EffectSchema>> {
    (0..chain.len())
        .map(|index| chain.effect_id(index).and_then(effect_schema))
        .collect()
}

fn validate_automation(
    automation: &[Automation],
    schemas: Vec<Option<&'static EffectSchema>>,
    label: &str,
    tempo: f64,
) -> Result<(), Error> {
    let mut seen: Vec<(usize, String)> = Vec::new();
    for item in automation {
        let Some(schema) = schemas.get(item.effect).copied().flatten() else {
            return Err(Error::EffectOutOfRange { index: item.effect });
        };
        if schema.params.iter().all(|param| param.id != item.param) {
            return Err(Error::UnknownParam {
                effect: schema.id.to_string(),
                param: item.param.clone(),
            });
        }
        if seen
            .iter()
            .any(|(effect, param)| *effect == item.effect && param == &item.param)
        {
            return Err(invalid(format!(
                "{label} automates {}.{} more than once",
                item.effect, item.param
            )));
        }
        seen.push((item.effect, item.param.clone()));
        item.track
            .validate(&format!("{label} {}.{}", item.effect, item.param), tempo)?;
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimeFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    beats: Option<f64>,
}

impl TryFrom<TimeFields> for PlanTime {
    type Error = String;

    fn try_from(fields: TimeFields) -> Result<Self, Self::Error> {
        match (fields.seconds, fields.beats) {
            (Some(seconds), None) => Ok(PlanTime::Seconds(seconds)),
            (None, Some(beats)) => Ok(PlanTime::Beats(beats)),
            _ => Err("time needs exactly one of \"seconds\" or \"beats\"".into()),
        }
    }
}

impl From<PlanTime> for TimeFields {
    fn from(time: PlanTime) -> Self {
        match time {
            PlanTime::Seconds(seconds) => Self {
                seconds: Some(seconds),
                beats: None,
            },
            PlanTime::Beats(beats) => Self {
                seconds: None,
                beats: Some(beats),
            },
        }
    }
}

fn default_version() -> u32 {
    ARRANGEMENT_VERSION
}

fn default_sample_rate() -> u32 {
    44_100
}

fn default_tempo() -> f64 {
    120.0
}

fn default_end() -> f32 {
    1.0
}

fn unity_gain() -> f32 {
    1.0
}

fn unity_track() -> Track {
    Track::constant(1.0)
}

fn zero_track() -> Track {
    Track::constant(0.0)
}

pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidPlan(message.into())
}
