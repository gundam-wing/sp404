use crate::buffer::AudioBuffer;
use crate::chain::ChainDesc;
use crate::sampler::Engine;
use crate::Error;

/// One pad trigger during an offline performance render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteEvent {
    /// Frame offset from the start of the render.
    pub frame: u64,
    /// Pad to trigger.
    pub pad: usize,
}

/// Render `input` through `chain` at the input's sample rate.
///
/// The result is stereo and has the same frame count as `input`. Mono input
/// is duplicated before the chain runs. This is the library call an agent
/// should use; it does not open files or devices.
pub fn render(input: &AudioBuffer, chain: &ChainDesc) -> Result<AudioBuffer, Error> {
    let stereo = input.to_stereo(input.sample_rate)?;
    let mut data = stereo.data;
    let mut live = chain.instantiate()?;
    live.process(&mut data, stereo.sample_rate);
    for sample in &mut data {
        if !sample.is_finite() {
            *sample = 0.0;
        }
    }
    AudioBuffer::new(stereo.sample_rate, 2, data)
}

/// [`render`] with the chain supplied as JSON.
///
/// The JSON may be a chain object or a bare array of effect objects.
pub fn render_json(input: &AudioBuffer, chain_json: &str) -> Result<AudioBuffer, Error> {
    let chain = ChainDesc::from_json(chain_json)?;
    render(input, &chain)
}

/// Bounce a pad performance to an interleaved stereo buffer.
///
/// `events` do not need to be sorted. Triggers that fall past `frames` are
/// ignored. The engine's current samples, chains, and sample rate are used,
/// so the caller loads the bank first.
pub fn render_performance(engine: &mut Engine, events: &[NoteEvent], frames: usize) -> Vec<f32> {
    let mut output = vec![0.0; frames.saturating_mul(2)];
    if frames == 0 {
        return output;
    }
    let mut events: Vec<NoteEvent> = events
        .iter()
        .copied()
        .filter(|event| event.frame < frames as u64)
        .collect();
    events.sort_by_key(|event| event.frame);
    let mut index = 0;
    let mut frame = 0u64;
    while frame < frames as u64 {
        while index < events.len() && events[index].frame <= frame {
            let _ = engine.trigger(events[index].pad);
            index += 1;
        }
        let next = events
            .get(index)
            .map(|event| event.frame)
            .unwrap_or(frames as u64)
            .min(frames as u64);
        let count = (next - frame) as usize;
        if count == 0 {
            break;
        }
        let start = frame as usize * 2;
        let end = start + count * 2;
        engine.process(&mut output[start..end]);
        frame = next;
    }
    output
}
