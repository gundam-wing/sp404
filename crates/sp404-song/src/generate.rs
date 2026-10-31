//! Seeded arrangement generation on top of `sp404-core` plans.
//!
//! The output is an [`Arrangement`]. Pads, triggers, and the bus filter
//! keyframes are the existing document, not a second format.

use sp404_core::plan::{
    beats, chain, effect, Arrangement, Ease, Keyframe, PlanBuilder, SampleSource, Target, Track,
};
use sp404_core::PAD_COUNT;

use crate::error::{err, Error};
use crate::euclidean::{euclidean, rotate};
use crate::label::SliceLabel;
use crate::rng::Rng;

/// Genre template. Names are the CLI spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Four-on-the-floor, offbeat hats, light swing.
    House,
    /// Syncopated kick, backbeat snare, swung hats.
    BoomBap,
    /// Four-on-the-floor, straight 16th hats, no swing.
    Techno,
}

impl Style {
    /// `house`, `boom-bap`, or `techno`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "house" => Some(Self::House),
            "boom-bap" => Some(Self::BoomBap),
            "techno" => Some(Self::Techno),
            _ => None,
        }
    }

    /// Stable spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::House => "house",
            Self::BoomBap => "boom-bap",
            Self::Techno => "techno",
        }
    }

    fn template(self) -> Template {
        match self {
            // Swing is the fraction of an 8th-note pair taken by the first 16th.
            // 0.5 is straight. 0.5625 and 0.625 are exact in binary.
            Self::House => Template {
                swing: 0.5625,
                kick: (4, 3),
                snare: (2, 3),
                hat: (8, 0),
                melodic: (3, 0),
            },
            Self::BoomBap => Template {
                swing: 0.625,
                kick: (5, 2),
                snare: (2, 3),
                hat: (10, 0),
                melodic: (3, 0),
            },
            Self::Techno => Template {
                swing: 0.5,
                kick: (4, 3),
                snare: (2, 3),
                hat: (16, 0),
                melodic: (2, 0),
            },
        }
    }
}

/// One slice the generator may assign to a pad.
#[derive(Debug, Clone)]
pub struct PadVoice {
    /// Heuristic role.
    pub label: SliceLabel,
    /// Sample the arrangement will play. `File` paths are the host's job.
    pub sample: SampleSource,
}

/// Knobs for [`generate`].
#[derive(Debug, Clone)]
pub struct GenerateOptions {
    /// Genre template.
    pub style: Style,
    /// Pattern seed. The same seed and inputs replay byte-for-byte.
    pub seed: u64,
    /// Length in 4/4 bars.
    pub bars: u32,
    /// Arrangement tempo, from the chop manifest.
    pub bpm: f64,
    /// Render sample rate. Slice files are resampled to this by the engine.
    pub sample_rate: u32,
    /// Arrangement name. Empty becomes `{style}-seed-{seed}`.
    pub name: String,
}

struct Template {
    swing: f64,
    kick: (usize, usize),
    snare: (usize, usize),
    hat: (usize, usize),
    melodic: (usize, usize),
}

struct Section {
    name: &'static str,
    start: f64,
    end: f64,
}

struct Assigned {
    pad: usize,
    label: SliceLabel,
    sample: SampleSource,
}

/// Build an arrangement from labeled slices.
///
/// Up to four slices of each label are placed on the 16 pads: kicks on 0..4,
/// snares on 4..8, hats on 8..12, melodic on 12..16. The seed picks which pad
/// in a role plays each hit, rotates the hat and melodic Euclidean patterns,
/// and jitters gain. Kick and snare grids stay on the style template.
pub fn generate(voices: &[PadVoice], options: &GenerateOptions) -> Result<Arrangement, Error> {
    if voices.is_empty() {
        return Err(err("generate needs at least one slice"));
    }
    if options.bars == 0 || options.bars > 256 {
        return Err(err("bars must be from 1 to 256"));
    }
    if !(options.bpm.is_finite() && options.bpm > 0.0 && options.bpm <= 1_000.0) {
        return Err(err("bpm must be finite, above 0, and at most 1000"));
    }
    if !(8_000..=192_000).contains(&options.sample_rate) {
        return Err(err("sample rate must be from 8000 to 192000 Hz"));
    }
    let total_beats = f64::from(options.bars) * 4.0;
    let seconds = total_beats * 60.0 / options.bpm;
    if seconds > 600.0 {
        return Err(err("arrangement would be longer than 10 minutes"));
    }
    let assigned = assign_pads(voices);
    if assigned.is_empty() {
        return Err(err("generate could not assign any pads"));
    }

    let template = options.style.template();
    let mut hat = role_pattern(template.hat);
    let mut melodic = role_pattern(template.melodic);
    hat = rotate(hat, (options.seed % 4) as usize);
    melodic = rotate(melodic, ((options.seed / 4) % 4) as usize);
    let patterns = [
        role_pattern(template.kick),
        role_pattern(template.snare),
        hat,
        melodic,
    ];
    let sections = sections(total_beats);
    let mut pads_for = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for voice in &assigned {
        pads_for[voice.label.index()].push(voice.pad);
    }

    let mut rng = Rng::new(options.seed);
    let mut hits = Vec::new();
    for bar in 0..options.bars {
        for step in 0..16 {
            let beat = f64::from(bar) * 4.0 + swung_step(step, template.swing);
            if beat >= total_beats - 1.0e-6 {
                continue;
            }
            let section = section_at(beat, &sections);
            for (role_index, pattern) in patterns.iter().enumerate() {
                if !pattern[step] {
                    continue;
                }
                let label = match role_index {
                    0 => SliceLabel::Kick,
                    1 => SliceLabel::Snare,
                    2 => SliceLabel::Hat,
                    _ => SliceLabel::Melodic,
                };
                let role_pads = &pads_for[label.index()];
                let section_gain = section_gain(section.name, label);
                if role_pads.is_empty() || section_gain <= 0.0 {
                    continue;
                }
                let pad = role_pads[rng.index(role_pads.len())];
                let gain = quantize(base_gain(label) * section_gain * jitter(&mut rng));
                if gain > 0.0 {
                    hits.push((beat, pad, gain));
                }
            }
        }
    }

    let name = if options.name.is_empty() {
        format!("{}-seed-{}", options.style.as_str(), options.seed)
    } else {
        options.name.clone()
    };
    let mut builder = PlanBuilder::new(options.sample_rate, options.bpm)
        .name(name)
        .duration(beats(total_beats))
        .master(Track::keyframes(master_keys(&sections)))
        .bus_chain(chain([effect(
            "filter",
            [
                ("mode", 0.0),
                ("cutoff", 1_800.0),
                ("resonance", 0.35),
                ("mix", 1.0),
            ],
        )]))
        .automate(
            Target::bus(0, "cutoff"),
            Track::keyframes(filter_keys(&sections, total_beats)),
        );
    for voice in &assigned {
        builder = builder.pad_source(voice.pad, voice.sample.clone());
        if voice.label == SliceLabel::Melodic {
            builder = builder.automate(
                Target::volume(voice.pad),
                Track::keyframes(melodic_level(&sections)),
            );
        }
    }
    for (beat, pad, gain) in hits {
        builder = builder.trigger_gain(pad, beats(beat), gain);
    }
    builder.finish().map_err(|build| err(build.to_string()))
}

/// Beat position of a 16th-note `step` inside a bar, after swing.
///
/// Even steps stay on the straight grid. Odd steps fall at `swing * 0.5`
/// beats into their 8th-note pair. `swing` 0.5 is straight.
pub fn swung_step(step: usize, swing: f64) -> f64 {
    let swing = swing.clamp(0.5, 0.75);
    let pair = (step / 2) as f64;
    let start = pair * 0.5;
    if step % 2 == 0 {
        start
    } else {
        start + swing * 0.5
    }
}

fn role_pattern((pulses, rotate_by): (usize, usize)) -> Vec<bool> {
    rotate(euclidean(pulses, 16), rotate_by)
}

fn assign_pads(voices: &[PadVoice]) -> Vec<Assigned> {
    let groups = [
        (SliceLabel::Kick, 0usize),
        (SliceLabel::Snare, 4),
        (SliceLabel::Hat, 8),
        (SliceLabel::Melodic, 12),
    ];
    let mut assigned = Vec::new();
    for (label, base) in groups {
        let group: Vec<&PadVoice> = voices.iter().filter(|voice| voice.label == label).collect();
        let chosen = pick_evenly(group.len(), 4);
        for (slot, index) in chosen.into_iter().enumerate() {
            let pad = base + slot;
            if pad >= PAD_COUNT {
                break;
            }
            assigned.push(Assigned {
                pad,
                label,
                sample: group[index].sample.clone(),
            });
        }
    }
    assigned
}

fn pick_evenly(len: usize, slots: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    if len <= slots {
        return (0..len).collect();
    }
    (0..slots).map(|slot| slot * len / slots).collect()
}

fn sections(total_beats: f64) -> Vec<Section> {
    // intro / A / B / break / outro. Weights sum to 10.
    let weights = [1.0, 3.0, 3.0, 2.0, 1.0];
    let names = ["intro", "a", "b", "break", "outro"];
    let sum: f64 = weights.iter().sum();
    let mut start = 0.0;
    names
        .iter()
        .zip(weights)
        .enumerate()
        .map(|(index, (name, weight))| {
            let end = if index + 1 == names.len() {
                total_beats
            } else {
                start + total_beats * weight / sum
            };
            let section = Section { name, start, end };
            start = end;
            section
        })
        .collect()
}

fn section_at(beat: f64, sections: &[Section]) -> &Section {
    sections
        .iter()
        .find(|section| beat < section.end - 1.0e-9)
        .unwrap_or_else(|| sections.last().expect("sections are non-empty"))
}

fn section_gain(section: &str, label: SliceLabel) -> f32 {
    match (section, label) {
        ("intro", SliceLabel::Kick) => 0.85,
        ("intro", SliceLabel::Hat) => 0.45,
        ("intro", _) => 0.0,
        ("a" | "b", _) => 1.0,
        ("break", SliceLabel::Kick) => 0.0,
        ("break", SliceLabel::Snare) => 0.85,
        ("break", SliceLabel::Hat) => 0.7,
        ("break", SliceLabel::Melodic) => 0.4,
        ("outro", SliceLabel::Kick) => 0.7,
        ("outro", SliceLabel::Hat) => 0.35,
        ("outro", SliceLabel::Snare) => 0.0,
        ("outro", SliceLabel::Melodic) => 0.25,
        _ => 0.0,
    }
}

fn base_gain(label: SliceLabel) -> f32 {
    match label {
        SliceLabel::Kick => 0.95,
        SliceLabel::Snare => 0.8,
        SliceLabel::Hat => 0.42,
        SliceLabel::Melodic => 0.7,
    }
}

fn jitter(rng: &mut Rng) -> f32 {
    (0.90 + 0.10 * rng.unit()) as f32
}

fn quantize(gain: f32) -> f32 {
    (gain * 1_000.0).round() / 1_000.0
}

fn master_keys(sections: &[Section]) -> Vec<Keyframe> {
    let intro = &sections[0];
    let brk = &sections[3];
    let outro_end = sections[4].end;
    vec![
        Keyframe::at(beats(intro.start), 0.2),
        Keyframe::at(beats(intro.end), 1.0).ease(Ease::Smooth),
        Keyframe::at(beats(brk.start), 1.0).ease(Ease::Hold),
        Keyframe::at(beats(brk.start + (brk.end - brk.start) * 0.35), 0.4).ease(Ease::Smooth),
        Keyframe::at(beats(brk.end), 1.0).ease(Ease::Smooth),
        Keyframe::at(beats(outro_end), 0.0).ease(Ease::Out),
    ]
}

fn filter_keys(sections: &[Section], total_beats: f64) -> Vec<Keyframe> {
    vec![
        Keyframe::at(beats(sections[0].start), 320.0),
        Keyframe::at(beats(sections[0].end), 2_800.0).ease(Ease::Smooth),
        Keyframe::at(beats(sections[2].start), 7_200.0).ease(Ease::Smooth),
        Keyframe::at(beats(sections[3].start), 480.0).ease(Ease::In),
        Keyframe::at(beats(sections[3].end), 4_200.0).ease(Ease::Smooth),
        Keyframe::at(beats(total_beats), 220.0).ease(Ease::Out),
    ]
}

fn melodic_level(sections: &[Section]) -> Vec<Keyframe> {
    let values = [0.0, 0.75, 0.85, 0.35, 0.2];
    sections
        .iter()
        .zip(values)
        .map(|(section, value)| {
            let ease = if section.name == "break" {
                Ease::In
            } else {
                Ease::Smooth
            };
            Keyframe::at(beats(section.start), value).ease(ease)
        })
        .collect()
}

trait PadSource {
    fn pad_source(self, pad: usize, sample: SampleSource) -> Self;
}

impl PadSource for PlanBuilder {
    fn pad_source(self, pad: usize, sample: SampleSource) -> Self {
        match sample {
            SampleSource::Demo { name } => self.pad_demo(pad, name),
            SampleSource::File { path } => self.pad_file(pad, path),
            SampleSource::Pcm {
                sample_rate,
                channels,
                samples,
            } => {
                let buffer = sp404_core::AudioBuffer {
                    sample_rate,
                    channels: u16::from(channels),
                    data: samples,
                };
                self.pad_pcm(pad, &buffer)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{generate, swung_step, GenerateOptions, PadVoice, Style};
    use crate::label::SliceLabel;
    use sp404_core::plan::{Arrangement, PlanTime, SampleSource, Track};

    fn click() -> SampleSource {
        let mut samples = vec![0.0f32; 200];
        for (index, sample) in samples.iter_mut().enumerate().take(40) {
            *sample = 0.9 * (1.0 - index as f32 / 40.0);
        }
        SampleSource::Pcm {
            sample_rate: 8_000.0,
            channels: 1,
            samples,
        }
    }

    fn options(style: Style, seed: u64) -> GenerateOptions {
        GenerateOptions {
            style,
            seed,
            bars: 2,
            bpm: 120.0,
            sample_rate: 8_000,
            name: String::new(),
        }
    }

    fn kick_bank() -> Vec<PadVoice> {
        vec![PadVoice {
            label: SliceLabel::Kick,
            sample: click(),
        }]
    }

    fn hat_bank() -> Vec<PadVoice> {
        vec![PadVoice {
            label: SliceLabel::Hat,
            sample: click(),
        }]
    }

    fn beat_of(trigger: &sp404_core::plan::Trigger) -> f64 {
        match trigger.at {
            PlanTime::Beats(beat) => beat,
            PlanTime::Seconds(seconds) => seconds,
        }
    }

    #[test]
    fn swing_delays_the_off_sixteenth() {
        assert!((swung_step(0, 0.625) - 0.0).abs() < 1.0e-9);
        assert!((swung_step(1, 0.5) - 0.25).abs() < 1.0e-9);
        assert!((swung_step(1, 0.625) - 0.3125).abs() < 1.0e-9);
        assert!(swung_step(1, 0.625) > swung_step(1, 0.5));
    }

    #[test]
    fn same_seed_replays_and_a_new_seed_changes_the_plan() {
        let first = generate(&kick_bank(), &options(Style::House, 1))
            .unwrap()
            .to_json()
            .unwrap();
        let again = generate(&kick_bank(), &options(Style::House, 1))
            .unwrap()
            .to_json()
            .unwrap();
        let other = generate(&kick_bank(), &options(Style::House, 2))
            .unwrap()
            .to_json()
            .unwrap();
        assert_eq!(first, again);
        assert_ne!(first, other);
    }

    #[test]
    fn house_kicks_land_on_the_beat_and_the_plan_renders() {
        let plan = generate(&kick_bank(), &options(Style::House, 0)).unwrap();
        let loaded = Arrangement::from_json(&plan.to_json().unwrap()).unwrap();
        let beats: Vec<f64> = loaded.triggers.iter().map(beat_of).collect();
        assert!(beats.iter().any(|beat| beat.abs() < 1.0e-6), "{beats:?}");
        assert!(
            beats.iter().any(|beat| (*beat - 1.0).abs() < 1.0e-6),
            "{beats:?}"
        );
        assert!(
            beats.iter().all(|beat| (beat * 4.0).fract().abs() < 1.0e-6
                || ((beat * 4.0).fract() - 1.0).abs() < 1.0e-6),
            "house kick left the quarter-note grid: {beats:?}"
        );
        let frames = loaded.duration_frames().unwrap();
        let audio = loaded.render(0, frames).unwrap();
        assert!(audio.peak() > 0.02, "peak {}", audio.peak());
        assert!(audio.data.iter().all(|sample| sample.is_finite()));
        assert_eq!(audio.frames() as u64, frames);
    }

    #[test]
    fn sections_move_the_filter_and_the_master() {
        let plan = generate(&kick_bank(), &options(Style::Techno, 0)).unwrap();
        let Track::Keyframes { keys } = &plan.master else {
            panic!("master is not keyframes");
        };
        assert!(keys.first().unwrap().value < 0.5);
        assert!(keys.last().unwrap().value < 0.15);
        assert!(keys.iter().any(|key| (key.value - 0.4).abs() < 1.0e-3));
        let cutoff = &plan.bus.automation[0];
        assert_eq!(cutoff.param, "cutoff");
        let Track::Keyframes { keys } = &cutoff.track else {
            panic!("cutoff is not keyframes");
        };
        let values: Vec<f32> = keys.iter().map(|key| key.value).collect();
        assert!(
            values.windows(3).any(|window| window[0] > window[1]
                && window[1] < window[2]
                && window[1] < 1_000.0),
            "filter did not dip through the break: {values:?}"
        );
    }

    #[test]
    fn boom_bap_swings_and_techno_does_not() {
        let swung = generate(&hat_bank(), &options(Style::BoomBap, 0)).unwrap();
        let straight = generate(&hat_bank(), &options(Style::Techno, 0)).unwrap();
        let swung_offsets: Vec<f64> = swung
            .triggers
            .iter()
            .map(|trigger| beat_of(trigger) % 0.5)
            .collect();
        let straight_offsets: Vec<f64> = straight
            .triggers
            .iter()
            .map(|trigger| beat_of(trigger) % 0.5)
            .collect();
        assert!(
            swung_offsets
                .iter()
                .any(|offset| (*offset - 0.3125).abs() < 1.0e-4),
            "boom-bap offsets {swung_offsets:?}"
        );
        assert!(
            straight_offsets
                .iter()
                .any(|offset| (*offset - 0.25).abs() < 1.0e-4),
            "techno offsets {straight_offsets:?}"
        );
    }
}
