//! Analyze, chop, generate, and render without leaving the process.
//!
//! The steps are the library functions. This module does not shell out and
//! does not reimplement the detectors or the pattern generator.

use sp404_analyze::{analyze, AnalyzeOptions};
use sp404_core::plan::SampleSource;
use sp404_core::{decode_wav, encode_wav, AudioBuffer};
use sp404_song::{
    chop, generate, ChopMode, ChopSource, GenerateOptions, PadVoice, SliceMark, Style,
};

use crate::error::err;

/// What the client asked for, after defaults are filled in.
#[derive(Debug, Clone)]
pub struct JobRequest {
    pub style: Style,
    pub seed: u64,
    pub bars: u32,
    pub mode: ChopMode,
}

/// Arrangement JSON and rendered WAV bytes.
pub struct RenderedJob {
    pub plan_json: String,
    pub wav: Vec<u8>,
}

/// Run the existing path on one WAV.
pub fn render_upload(wav: &[u8], request: &JobRequest) -> Result<RenderedJob, String> {
    let buffer = decode_wav(wav).map_err(|decode| decode.to_string())?;
    let mono = mix_mono(&buffer);
    let analysis = analyze(&mono, buffer.sample_rate, &AnalyzeOptions::default())
        .map_err(|e| e.to_string())?;
    if !(analysis.bpm.is_finite() && analysis.bpm > 0.0) {
        return Err(err("analysis did not find a tempo"));
    }
    let marks: Vec<SliceMark> = analysis
        .slices
        .iter()
        .map(|slice| SliceMark {
            start: slice.start,
            end: slice.end,
            fade_in: slice.fade_in,
            fade_out: slice.fade_out,
        })
        .collect();
    let chopped = chop(
        ChopSource {
            samples: &mono,
            sample_rate: buffer.sample_rate,
            duration: analysis.duration,
            bpm: analysis.bpm,
            beat_times: &analysis.beat_times,
            slices: &marks,
            key: &analysis.key,
            key_confidence: analysis.key_confidence,
        },
        request.mode,
    )
    .map_err(|chop_err| chop_err.to_string())?;
    let voices = chopped
        .slices
        .iter()
        .map(|slice| {
            let channels = u8::try_from(slice.audio.channels).unwrap_or(1);
            PadVoice {
                label: slice.label,
                sample: SampleSource::Pcm {
                    sample_rate: slice.audio.sample_rate,
                    channels,
                    samples: slice.audio.data.clone(),
                },
            }
        })
        .collect::<Vec<_>>();
    let sample_rate = analysis.sample_rate.round();
    if !(8_000.0..=192_000.0).contains(&sample_rate) {
        return Err(err(
            "sample rate must be from 8000 to 192000 Hz for an arrangement",
        ));
    }
    let plan = generate(
        &voices,
        &GenerateOptions {
            style: request.style,
            seed: request.seed,
            bars: request.bars,
            bpm: analysis.bpm,
            sample_rate: sample_rate as u32,
            name: String::new(),
        },
    )
    .map_err(|generate_err| generate_err.to_string())?;
    let plan_json = plan.to_json().map_err(|json| json.to_string())?;
    let frames = plan
        .duration_frames()
        .map_err(|frames| frames.to_string())?;
    let audio = plan
        .render(0, frames)
        .map_err(|render_err| render_err.to_string())?;
    let wav_out = encode_wav(&audio).map_err(|encode| encode.to_string())?;
    Ok(RenderedJob {
        plan_json,
        wav: wav_out,
    })
}

fn mix_mono(buffer: &AudioBuffer) -> Vec<f32> {
    let channels = buffer.channels as usize;
    let frames = buffer.frames();
    let mut mono = Vec::with_capacity(frames);
    for frame in 0..frames {
        let mut sum = 0.0;
        for channel in 0..channels {
            let sample = buffer.data[frame * channels + channel];
            sum += if sample.is_finite() { sample } else { 0.0 };
        }
        mono.push(sum / channels as f32);
    }
    mono
}
