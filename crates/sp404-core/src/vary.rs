//! Find phrases in a recording and turn one of them into an arrangement.
//!
//! The search is the same measurement used to chop a song by hand. A short
//! loudness envelope marks where the sound is, breaths split long stretches,
//! and each phrase is scored by its attack, its contrast, and its length.
//! [`vary`] then loops the chosen phrase, tucks the neighboring phrases
//! around it, and slows the playback. A seed changes which phrase leads and
//! how the loop is built. The same buffer and seed always build the same plan.

use serde::Serialize;

use crate::buffer::AudioBuffer;
use crate::plan::{chain, effect, seconds, Arrangement, Keyframe, LfoShape, PlanBuilder, Track};
use crate::Error;

/// One contiguous stretch of audio the search is willing to loop.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Phrase {
    /// Start, in seconds from the beginning of the buffer.
    pub start: f64,
    /// End, in seconds. Always greater than `start`.
    pub end: f64,
    /// Higher means a stronger candidate for the loop.
    pub score: f32,
    /// Mean loudness of the phrase.
    pub rms: f32,
    /// Loudest hop inside the phrase.
    pub peak: f32,
    /// How hard the phrase attacks, as a rise in loudness.
    pub attack: f32,
}

/// Phrases found in one buffer, in time order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Analysis {
    /// Sample rate of the buffer that was searched.
    pub sample_rate: f32,
    /// Length of that buffer, in seconds.
    pub duration: f64,
    /// Phrases, earliest first.
    pub phrases: Vec<Phrase>,
}

/// What [`vary`] built, plus the phrases it used.
#[derive(Debug, Clone)]
pub struct Variation {
    /// A plan that loops `hero` and mixes in `neighbors`.
    pub plan: Arrangement,
    /// The phrase the loop is built on.
    pub hero: Phrase,
    /// Phrases placed beside the hero: the one before, then the ones after.
    pub neighbors: Vec<Phrase>,
    /// Playback transpose in semitones. Negative values also slow the phrase.
    pub pitch: f32,
    /// The seed that selected this arrangement.
    pub seed: u64,
    /// Which arrangement shape the seed picked.
    pub pattern: &'static str,
}

/// Inputs for [`vary`].
pub struct VaryRequest<'a> {
    /// Recording to search and, when `source_path` is absent, to slice.
    pub buffer: &'a AudioBuffer,
    /// Chooses the phrase (unless `around` is set) and the arrangement shape.
    pub seed: u64,
    /// Prefer the phrase nearest this time in the source, in seconds.
    pub around: Option<f64>,
    /// When set, pads point at this WAV path and use start/end cuts.
    ///
    /// The core does not open the path. Leave it empty to embed each slice
    /// in the plan, which is what tests and a host without a filesystem want.
    pub source_path: Option<&'a str>,
}

/// Search `buffer` for loopable phrases.
///
/// Returns [`Error::InvalidBuffer`] when the buffer is shorter than a phrase
/// or no stretch rises above the noise floor.
pub fn analyze(buffer: &AudioBuffer) -> Result<Analysis, Error> {
    let env = envelope(buffer)?;
    let duration = env.duration;
    if duration < 0.5 {
        return Err(Error::InvalidBuffer(
            "recording is shorter than 0.5 seconds".into(),
        ));
    }
    let mut phrases = phrases_from(&env);
    if phrases.is_empty() {
        return Err(Error::InvalidBuffer(
            "recording never rises above the noise floor".into(),
        ));
    }
    phrases.sort_by(|left, right| {
        left.start
            .partial_cmp(&right.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(Analysis {
        sample_rate: buffer.sample_rate,
        duration,
        phrases,
    })
}

/// Build one slowed, looped arrangement from [`analyze`].
pub fn vary(request: VaryRequest<'_>) -> Result<Variation, Error> {
    let rate = request.buffer.sample_rate.round() as u32;
    if !(8_000..=192_000).contains(&rate) {
        return Err(Error::InvalidBuffer(
            "sample rate must be from 8000 to 192000 Hz".into(),
        ));
    }
    let analysis = analyze(request.buffer)?;
    let mut rng = Rng(request.seed);
    let hero_index = pick_hero(&analysis.phrases, request.around, &mut rng);
    let hero = analysis.phrases[hero_index].clone();
    let neighbors = neighbors_of(&analysis.phrases, hero_index);
    let pitch = PITCHES[rng.index(PITCHES.len())];
    let pattern_index = rng.index(PATTERNS.len());
    let pattern = PATTERNS[pattern_index];
    let reverb = REVERBS[rng.index(REVERBS.len())];
    let vinyl = VINYLS[rng.index(VINYLS.len())];
    let open = OPENS[rng.index(OPENS.len())];

    let stretch = 2.0_f64.powf(f64::from(-pitch) / 12.0);
    let hero_play = (hero.end - hero.start) * stretch;
    let before_play = neighbors
        .first()
        .filter(|phrase| phrase.end <= hero.start + 0.05)
        .map(|phrase| (phrase.end - phrase.start) * stretch);
    let after = neighbors
        .iter()
        .find(|phrase| phrase.start >= hero.end - 0.05);
    let after_play = after.map(|phrase| (phrase.end - phrase.start) * stretch);
    let further = neighbors
        .iter()
        .filter(|phrase| phrase.start >= hero.end - 0.05)
        .nth(1);
    let further_play = further.map(|phrase| (phrase.end - phrase.start) * stretch);

    let events = arrange(
        pattern_index,
        hero_play,
        before_play,
        after_play,
        further_play,
    );
    let last = events
        .iter()
        .map(|event| event.at + event.play)
        .fold(0.0, f64::max);
    let duration = (last + 4.0).clamp(6.0, 180.0);

    let mut builder = PlanBuilder::new(request.buffer.sample_rate.round() as u32, 80.0)
        .name(format!("vary-{}", request.seed))
        .duration(seconds(duration))
        .master(Track::constant(master_gain(hero.rms)))
        .bus_chain(chain([
            effect(
                "vinyl",
                [
                    ("wow", 0.28),
                    ("flutter", 0.12),
                    ("crackle", 0.05),
                    ("wear", 0.36),
                    ("mix", vinyl),
                ],
            ),
            effect(
                "filter",
                [
                    ("mode", 0.0),
                    ("cutoff", 1_800.0),
                    ("resonance", 0.12),
                    ("mix", 1.0),
                ],
            ),
            effect(
                "isolator",
                [
                    ("low_db", 0.4),
                    ("mid_db", 1.2),
                    ("high_db", -1.8),
                    ("low_mid_hz", 220.0),
                    ("mid_high_hz", 3_400.0),
                ],
            ),
            effect(
                "compressor",
                [
                    ("threshold_db", -16.0),
                    ("ratio", 2.0),
                    ("attack_ms", 18.0),
                    ("release_ms", 180.0),
                    ("makeup_db", 1.0),
                    ("mix", 1.0),
                ],
            ),
            effect(
                "reverb",
                [
                    ("room", 0.68),
                    ("damp", 0.4),
                    ("width", 0.74),
                    ("mix", reverb),
                ],
            ),
        ]))
        .automate(
            crate::plan::Target::bus(1, "cutoff"),
            Track::keyframes(filter_keys(duration, open)),
        );

    let stutter = stutter_window(&hero);
    let slots = [
        Some(&hero),
        neighbors
            .iter()
            .find(|phrase| phrase.end <= hero.start + 0.05),
        after,
        further,
        Some(&stutter),
    ];
    for (pad, phrase) in slots.into_iter().enumerate() {
        let Some(phrase) = phrase else {
            continue;
        };
        builder = place_pad(builder, request.buffer, request.source_path, pad, phrase);
        builder = builder.automate(
            crate::plan::Target::pitch(pad),
            Track::lfo_hz(LfoShape::Sine, 0.07, 0.07, pitch),
        );
    }
    for event in &events {
        if slots[event.pad].is_none() {
            continue;
        }
        builder = builder.trigger_gain(event.pad, seconds(event.at), event.gain);
    }

    let plan = builder.finish()?;
    Ok(Variation {
        plan,
        hero,
        neighbors,
        pitch,
        seed: request.seed,
        pattern,
    })
}

const PITCHES: [f32; 5] = [-3.0, -4.0, -5.0, -6.0, -7.0];
const REVERBS: [f32; 4] = [0.22, 0.28, 0.34, 0.42];
const VINYLS: [f32; 3] = [0.5, 0.66, 0.8];
const OPENS: [f32; 4] = [2_200.0, 3_200.0, 4_600.0, 6_000.0];
const PATTERNS: [&str; 4] = ["loop-then-expand", "lead-in", "stutter-open", "overlap"];

struct Env {
    hop: f64,
    rms: Vec<f32>,
    duration: f64,
}

struct Event {
    pad: usize,
    at: f64,
    gain: f32,
    play: f64,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        self.0
    }

    fn index(&mut self, len: usize) -> usize {
        (self.next() as usize) % len
    }
}

fn envelope(buffer: &AudioBuffer) -> Result<Env, Error> {
    let channels = buffer.channels as usize;
    let frames = buffer.frames();
    let hop = ((buffer.sample_rate * 0.02).round() as usize).max(1);
    if frames < hop * 4 {
        return Err(Error::InvalidBuffer(
            "recording is shorter than 0.5 seconds".into(),
        ));
    }
    let mut rms = Vec::new();
    let mut start = 0;
    while start + hop <= frames {
        let mut sum = 0.0_f32;
        for frame in start..start + hop {
            let mut mono = 0.0;
            for channel in 0..channels {
                mono += buffer.data[frame * channels + channel];
            }
            mono /= channels as f32;
            sum += mono * mono;
        }
        rms.push((sum / hop as f32).sqrt());
        start += hop;
    }
    if rms.len() >= 3 {
        let raw = rms.clone();
        for index in 1..rms.len() - 1 {
            rms[index] = (raw[index - 1] + raw[index] + raw[index + 1]) / 3.0;
        }
    }
    let duration = frames as f64 / f64::from(buffer.sample_rate);
    Ok(Env {
        hop: hop as f64 / f64::from(buffer.sample_rate),
        rms,
        duration,
    })
}

fn phrases_from(env: &Env) -> Vec<Phrase> {
    let gate = noise_gate(&env.rms);
    let mut runs = Vec::new();
    let mut index = 0;
    while index < env.rms.len() {
        if env.rms[index] < gate {
            index += 1;
            continue;
        }
        let start = index;
        while index < env.rms.len() && env.rms[index] >= gate {
            index += 1;
        }
        split_run(env, start, index, &mut runs);
    }
    runs.retain(|(start, end)| env.time(*end) - env.time(*start) >= 0.7);
    merge_close(env, runs)
        .into_iter()
        .filter_map(|(start, end)| score_phrase(env, start, end))
        .collect()
}

fn noise_gate(rms: &[f32]) -> f32 {
    let mut sorted = rms.to_vec();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let index = ((sorted.len() as f32) * 0.85).floor() as usize;
    let loud = sorted[index.min(sorted.len() - 1)];
    (loud * 0.18).max(0.008)
}

fn split_run(env: &Env, start: usize, end: usize, out: &mut Vec<(usize, usize)>) {
    let length = env.time(end) - env.time(start);
    if length <= 7.5 || end.saturating_sub(start) < 8 {
        out.push((start, end));
        return;
    }
    let margin = ((end - start) as f32 * 0.18) as usize;
    let lo = start + margin.max(1);
    let hi = end.saturating_sub(margin.max(1));
    let mut dip = None;
    let mut quiet = f32::MAX;
    if hi > lo {
        for index in lo..hi {
            let level = env.rms[index];
            let local_min = level <= env.rms[index - 1] && level <= env.rms[index + 1];
            if local_min && level < quiet {
                dip = Some(index);
                quiet = level;
            }
        }
    }
    // A long stretch always breaks at its quietest interior dip, even when
    // that dip is only a little softer than the rest. Leaving it whole
    // produces a phrase too long to loop.
    let Some(dip) = dip else {
        out.push((start, end));
        return;
    };
    split_run(env, start, dip, out);
    split_run(env, dip, end, out);
}

fn merge_close(env: &Env, runs: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    let mut merged = Vec::new();
    for (start, end) in runs {
        if let Some((previous_start, previous_end)) = merged.last_mut() {
            let gap = env.time(start) - env.time(*previous_end);
            let combined = env.time(end) - env.time(*previous_start);
            // A split inside one long stretch leaves the pieces touching.
            // Only glue a real rest, and only when the result still loops.
            if gap > 0.0 && gap < 0.18 && combined <= 7.5 {
                *previous_end = end;
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

fn score_phrase(env: &Env, start: usize, end: usize) -> Option<Phrase> {
    if end <= start + 1 {
        return None;
    }
    let slice = &env.rms[start..end];
    let mean = slice.iter().sum::<f32>() / slice.len() as f32;
    if mean < 0.008 {
        return None;
    }
    let peak = slice.iter().copied().fold(0.0, f32::max);
    let mut attack = 0.0_f32;
    let attack_hops = ((0.3 / env.hop).ceil() as usize).max(2);
    for index in start + 1..(start + attack_hops).min(end) {
        attack = attack.max(env.rms[index] - env.rms[index - 1]);
    }
    let length = env.time(end) - env.time(start);
    let contrast = (peak / (mean + 1.0e-4)).min(6.0) / 6.0;
    let attack_n = (attack / 0.12).min(1.0);
    let length_n = length_weight(length);
    let score = 0.4 * contrast + 0.35 * attack_n + 0.25 * length_n;
    Some(Phrase {
        start: round3(env.time(start).max(0.0)),
        end: round3(env.time(end).min(env.duration)),
        score,
        rms: mean,
        peak,
        attack,
    })
}

fn length_weight(length: f64) -> f32 {
    let distance = (length - 3.2).abs();
    (1.0 - distance / 5.0).clamp(0.0, 1.0) as f32
}

fn pick_hero(phrases: &[Phrase], around: Option<f64>, rng: &mut Rng) -> usize {
    if let Some(around) = around {
        phrases
            .iter()
            .enumerate()
            .max_by(|(left_index, left), (right_index, right)| {
                interest(left, around)
                    .partial_cmp(&interest(right, around))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(right_index.cmp(left_index))
            })
            .map(|(index, _)| index)
            .unwrap_or(0)
    } else {
        let mut ranked: Vec<usize> = (0..phrases.len()).collect();
        ranked.sort_by(|&left, &right| {
            phrases[right]
                .score
                .partial_cmp(&phrases[left].score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let pool = ranked.len().min(4);
        ranked[rng.index(pool)]
    }
}

fn interest(phrase: &Phrase, around: f64) -> f32 {
    let middle = (phrase.start + phrase.end) * 0.5;
    let distance = (middle - around).abs();
    phrase.score / (1.0 + distance as f32 * 0.35)
}

fn neighbors_of(phrases: &[Phrase], hero: usize) -> Vec<Phrase> {
    let mut neighbors = Vec::new();
    if hero > 0 {
        neighbors.push(phrases[hero - 1].clone());
    }
    if hero + 1 < phrases.len() {
        neighbors.push(phrases[hero + 1].clone());
    }
    if hero + 2 < phrases.len() {
        neighbors.push(phrases[hero + 2].clone());
    }
    neighbors
}

fn stutter_window(hero: &Phrase) -> Phrase {
    let length = (hero.end - hero.start).clamp(0.4, 8.0);
    let start = hero.start + length * 0.18;
    let end = (start + 0.42).min(hero.end);
    Phrase {
        start: round3(start),
        end: round3(end.max(start + 0.12)),
        score: hero.score,
        rms: hero.rms,
        peak: hero.peak,
        attack: hero.attack,
    }
}

fn arrange(
    pattern: usize,
    hero: f64,
    before: Option<f64>,
    after: Option<f64>,
    further: Option<f64>,
) -> Vec<Event> {
    let loop_every = (hero * 0.97).max(0.4);
    let mut events = Vec::new();
    match pattern {
        0 => {
            for index in 0..4 {
                events.push(event(0, index as f64 * loop_every, 0.94, hero));
            }
            if before.is_some() {
                events.push(event(
                    1,
                    loop_every + hero * 0.55,
                    0.42,
                    before.unwrap_or(0.0),
                ));
            }
            if after.is_some() {
                events.push(event(
                    2,
                    3.0 * loop_every + hero * 0.5,
                    0.4,
                    after.unwrap_or(0.0),
                ));
            }
            let chain = 4.0 * loop_every + 0.35;
            push_sentence(&mut events, chain, hero, before, after, further);
        }
        1 => {
            let mut cursor = 0.0;
            if let Some(play) = before {
                events.push(event(1, cursor, 0.7, play));
                cursor += play + 0.12;
            }
            for index in 0..3 {
                events.push(event(0, cursor + index as f64 * loop_every, 0.94, hero));
            }
            if let Some(play) = after {
                events.push(event(2, cursor + loop_every + hero * 0.45, 0.45, play));
            }
        }
        2 => {
            events.push(event(4, 0.0, 0.75, 0.5));
            events.push(event(4, 0.38, 0.6, 0.5));
            events.push(event(4, 0.76, 0.48, 0.5));
            let start = 1.3;
            for index in 0..4 {
                events.push(event(0, start + index as f64 * loop_every, 0.94, hero));
            }
            if let Some(play) = further.or(after) {
                let pad = if further.is_some() { 3 } else { 2 };
                events.push(event(pad, start + 2.0 * loop_every, 0.4, play));
            }
        }
        _ => {
            for index in 0..5 {
                events.push(event(0, index as f64 * loop_every, 0.92, hero));
                if index % 2 == 1 {
                    if let Some(play) = before {
                        events.push(event(1, index as f64 * loop_every + 0.15, 0.38, play));
                    }
                } else if index > 0 {
                    if let Some(play) = after {
                        events.push(event(2, index as f64 * loop_every + hero * 0.4, 0.36, play));
                    }
                }
            }
        }
    }
    events
}

fn push_sentence(
    events: &mut Vec<Event>,
    mut cursor: f64,
    hero: f64,
    before: Option<f64>,
    after: Option<f64>,
    further: Option<f64>,
) {
    if let Some(play) = before {
        events.push(event(1, cursor, 0.72, play));
        cursor += play + 0.1;
    }
    events.push(event(0, cursor, 0.94, hero));
    cursor += hero + 0.08;
    if let Some(play) = after {
        events.push(event(2, cursor, 0.62, play));
        if let Some(more) = further {
            events.push(event(3, cursor + play * 0.7, 0.36, more));
        }
    }
}

fn event(pad: usize, at: f64, gain: f32, play: f64) -> Event {
    Event {
        pad,
        at: round3(at),
        gain,
        play,
    }
}

fn place_pad(
    builder: PlanBuilder,
    buffer: &AudioBuffer,
    source_path: Option<&str>,
    pad: usize,
    phrase: &Phrase,
) -> PlanBuilder {
    let (start, end) = fractions(buffer, phrase);
    if let Some(path) = source_path {
        builder.pad_file(pad, path).pad_range(pad, start, end)
    } else {
        let slice = slice_buffer(buffer, phrase.start, phrase.end);
        builder.pad_pcm(pad, &slice).pad_range(pad, 0.0, 1.0)
    }
}

fn fractions(buffer: &AudioBuffer, phrase: &Phrase) -> (f32, f32) {
    let duration = buffer.frames() as f64 / f64::from(buffer.sample_rate);
    let start = (phrase.start / duration).clamp(0.0, 0.98) as f32;
    let end = (phrase.end / duration).clamp(f64::from(start) + 0.002, 1.0) as f32;
    (start, end)
}

fn slice_buffer(buffer: &AudioBuffer, start: f64, end: f64) -> AudioBuffer {
    let channels = buffer.channels as usize;
    let frames = buffer.frames();
    let start_frame = ((start * f64::from(buffer.sample_rate)).round() as usize).min(frames - 1);
    let mut end_frame = ((end * f64::from(buffer.sample_rate)).round() as usize).min(frames);
    if end_frame <= start_frame {
        end_frame = (start_frame + 1).min(frames);
    }
    let data = buffer.data[start_frame * channels..end_frame * channels].to_vec();
    AudioBuffer::new(buffer.sample_rate, buffer.channels, data).expect("slice keeps a valid layout")
}

fn filter_keys(duration: f64, open: f32) -> Vec<Keyframe> {
    let marks = [
        (0.0, 1_200.0),
        (0.32, 1_800.0),
        (0.62, open),
        (0.84, open * 0.55),
        (1.0, 800.0),
    ];
    let mut keys = Vec::new();
    let mut previous = -1.0;
    for (fraction, value) in marks {
        let time = round3((duration * fraction).min(duration));
        if time <= previous + 0.02 {
            continue;
        }
        let key = Keyframe::at(seconds(time), value);
        keys.push(if keys.is_empty() {
            key
        } else {
            key.ease(crate::plan::Ease::Smooth)
        });
        previous = time;
    }
    if keys.len() < 2 {
        keys.push(
            Keyframe::at(seconds(duration.max(previous + 0.05)), 800.0)
                .ease(crate::plan::Ease::Smooth),
        );
    }
    keys
}

fn master_gain(rms: f32) -> f32 {
    // Cap below unity. Several phrases play at once, and the bus compressor
    // adds makeup, so a full-scale master clips on the stacked loops.
    (0.16 / rms.max(0.02)).clamp(0.35, 0.8)
}

fn round3(value: f64) -> f64 {
    (value * 1_000.0).round() / 1_000.0
}

impl Env {
    fn time(&self, hop: usize) -> f64 {
        hop as f64 * self.hop
    }
}
