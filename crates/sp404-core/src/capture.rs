//! Turn a host-recorded buffer into a pad sample.
//!
//! Nothing here opens a microphone. A native host copies interleaved frames
//! out of an input callback. A browser host would copy the same frames out of
//! an AudioWorklet fed by `getUserMedia`. [`ingest_take`] maps channels,
//! optionally trims silence, and resamples to the engine rate.

use crate::buffer::AudioBuffer;
use crate::dsp::sanitize;
use crate::Error;

/// Interleaved frames captured by a host, at the device rate.
///
/// `channels` may be greater than two. An interface with many inputs keeps
/// the first pair; the rest are ignored when the take is ingested.
#[derive(Debug, Clone, PartialEq)]
pub struct Take {
    /// Device sample rate in hertz.
    pub sample_rate: f32,
    /// Channel count of `samples`.
    pub channels: u16,
    /// Interleaved frames. Non-finite values are replaced when ingested.
    pub samples: Vec<f32>,
}

/// How [`ingest_take`] turns a recording into a pad sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TakeOptions {
    /// Engine rate. The result is stereo at this rate.
    pub target_rate: f32,
    /// Drop leading and trailing silence when true.
    pub trim_silence: bool,
    /// Linear level. A frame at or above this is sound.
    pub trim_threshold: f32,
    /// Seconds to keep before the first loud frame.
    pub pre_roll: f32,
    /// Seconds to keep after the last loud frame.
    pub post_roll: f32,
}

impl Default for TakeOptions {
    fn default() -> Self {
        Self {
            target_rate: 44_100.0,
            trim_silence: true,
            trim_threshold: 0.002,
            pre_roll: 0.01,
            post_roll: 0.02,
        }
    }
}

impl Take {
    /// Store interleaved frames after checking the layout.
    ///
    /// Samples may be non-finite; ingestion replaces those. The layout must
    /// describe a whole number of frames.
    pub fn new(sample_rate: f32, channels: u16, samples: Vec<f32>) -> Result<Self, Error> {
        check_layout(sample_rate, channels, samples.len())?;
        Ok(Self {
            sample_rate,
            channels,
            samples,
        })
    }

    /// Map, trim, and resample this take into a stereo pad sample.
    pub fn ingest(&self, options: TakeOptions) -> Result<AudioBuffer, Error> {
        ingest_take(self.sample_rate, self.channels, &self.samples, options)
    }
}

/// Map `interleaved` onto a stereo buffer at `options.target_rate`.
///
/// One channel is copied to both sides. Two or more channels keep the first
/// pair and drop the rest. When `trim_silence` is set and every frame is
/// under the threshold, the take is kept whole so a quiet recording is not
/// discarded.
pub fn ingest_take(
    sample_rate: f32,
    channels: u16,
    interleaved: &[f32],
    options: TakeOptions,
) -> Result<AudioBuffer, Error> {
    check_layout(sample_rate, channels, interleaved.len())?;
    if !(options.target_rate.is_finite() && options.target_rate >= 1_000.0) {
        return Err(Error::InvalidBuffer(
            "target sample rate must be finite and at least 1000 Hz".into(),
        ));
    }
    if !options.trim_threshold.is_finite() || options.trim_threshold < 0.0 {
        return Err(Error::InvalidBuffer(
            "trim threshold must be finite and at least 0".into(),
        ));
    }
    let mapped = map_channels(interleaved, channels as usize);
    let channel_count = if channels == 1 { 1 } else { 2 };
    let mut buffer = AudioBuffer::new(sample_rate, channel_count, mapped)?;
    if options.trim_silence {
        buffer = trim_silence(&buffer, &options);
    }
    buffer.to_stereo(options.target_rate)
}

fn check_layout(sample_rate: f32, channels: u16, len: usize) -> Result<(), Error> {
    if !(sample_rate.is_finite() && sample_rate >= 1_000.0) {
        return Err(Error::InvalidBuffer(
            "sample rate must be finite and at least 1000 Hz".into(),
        ));
    }
    if channels == 0 || channels > 32 {
        return Err(Error::InvalidBuffer(format!(
            "unsupported channel count {channels}"
        )));
    }
    if len == 0 || len % channels as usize != 0 {
        return Err(Error::InvalidBuffer(
            "recording must contain a whole number of frames".into(),
        ));
    }
    Ok(())
}

fn map_channels(interleaved: &[f32], channels: usize) -> Vec<f32> {
    let frames = interleaved.len() / channels;
    if channels == 1 {
        return interleaved.iter().copied().map(sanitize).collect();
    }
    let mut out = Vec::with_capacity(frames * 2);
    for frame in 0..frames {
        let base = frame * channels;
        out.push(sanitize(interleaved[base]));
        out.push(sanitize(interleaved[base + 1]));
    }
    out
}

fn trim_silence(buffer: &AudioBuffer, options: &TakeOptions) -> AudioBuffer {
    let channels = buffer.channels as usize;
    let frames = buffer.frames();
    let loud = |frame: usize| {
        let base = frame * channels;
        (0..channels).any(|channel| buffer.data[base + channel].abs() >= options.trim_threshold)
    };
    let Some(first) = (0..frames).find(|frame| loud(*frame)) else {
        return buffer.clone();
    };
    let last = (0..frames)
        .rev()
        .find(|frame| loud(*frame))
        .unwrap_or(first);
    let pre = seconds_to_frames(options.pre_roll, buffer.sample_rate);
    let post = seconds_to_frames(options.post_roll, buffer.sample_rate);
    let start = first.saturating_sub(pre);
    let end = (last + 1 + post).min(frames);
    if start == 0 && end == frames {
        return buffer.clone();
    }
    let data = buffer.data[start * channels..end * channels].to_vec();
    AudioBuffer {
        sample_rate: buffer.sample_rate,
        channels: buffer.channels,
        data,
    }
}

fn seconds_to_frames(seconds: f32, sample_rate: f32) -> usize {
    if !seconds.is_finite() || seconds <= 0.0 {
        0
    } else {
        (f64::from(seconds) * f64::from(sample_rate)).round() as usize
    }
}
