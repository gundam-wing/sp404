use crate::demos;
use crate::Error;

/// Interleaved audio owned by the caller.
///
/// `channels` is 1 or 2. Frames are stored back to back: for stereo, index
/// `frame * 2` is left and `frame * 2 + 1` is right. The sampler engine and
/// the effect chain always process stereo; mono buffers are duplicated on the
/// way in.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioBuffer {
    /// Sample rate in hertz.
    pub sample_rate: f32,
    /// 1 for mono, 2 for stereo.
    pub channels: u16,
    /// Interleaved samples.
    pub data: Vec<f32>,
}

impl AudioBuffer {
    /// Build a buffer after checking the layout.
    ///
    /// Samples are not required to be finite; the processors replace non-finite
    /// values when they run. The layout itself must be usable.
    pub fn new(sample_rate: f32, channels: u16, data: Vec<f32>) -> Result<Self, Error> {
        if !(sample_rate.is_finite() && sample_rate >= 1_000.0) {
            return Err(Error::InvalidBuffer(
                "sample rate must be finite and at least 1000 Hz".into(),
            ));
        }
        if channels != 1 && channels != 2 {
            return Err(Error::InvalidBuffer(format!(
                "unsupported channel count {channels}"
            )));
        }
        if data.is_empty() || data.len() % channels as usize != 0 {
            return Err(Error::InvalidBuffer(
                "buffer must contain a whole number of frames".into(),
            ));
        }
        Ok(Self {
            sample_rate,
            channels,
            data,
        })
    }

    /// Number of frames (not samples).
    pub fn frames(&self) -> usize {
        self.data.len() / self.channels as usize
    }

    /// Peak absolute sample.
    pub fn peak(&self) -> f32 {
        self.data
            .iter()
            .fold(0.0, |peak, sample| peak.max(sample.abs()))
    }

    /// Root-mean-square level.
    pub fn rms(&self) -> f32 {
        if self.data.is_empty() {
            return 0.0;
        }
        let sum: f32 = self.data.iter().map(|sample| sample * sample).sum();
        (sum / self.data.len() as f32).sqrt()
    }

    /// A named built-in one-shot or test signal, already stereo.
    ///
    /// Names are listed by [`AudioBuffer::demo_names`]. Unknown names return
    /// `None`.
    pub fn demo(name: &str, sample_rate: f32) -> Option<Self> {
        demos::render(name, sample_rate)
    }

    /// Names accepted by [`AudioBuffer::demo`].
    pub fn demo_names() -> &'static [&'static str] {
        demos::NAMES
    }

    /// Convert to stereo at `sample_rate`, resampling with linear interpolation
    /// when the rate differs.
    pub fn to_stereo(&self, sample_rate: f32) -> Result<Self, Error> {
        if !(sample_rate.is_finite() && sample_rate >= 1_000.0) {
            return Err(Error::InvalidBuffer(
                "target sample rate must be finite and at least 1000 Hz".into(),
            ));
        }
        if self.channels == 2 && (self.sample_rate - sample_rate).abs() < 0.5 {
            return Ok(self.clone());
        }
        let src_frames = self.frames();
        if src_frames == 0 {
            return Err(Error::InvalidBuffer("buffer has no frames".into()));
        }
        let dst_frames = ((src_frames as f64) * (sample_rate as f64) / (self.sample_rate as f64))
            .round()
            .max(1.0) as usize;
        let mut data = Vec::with_capacity(dst_frames * 2);
        let channels = self.channels as usize;
        let last = src_frames - 1;
        for frame in 0..dst_frames {
            let pos = frame as f64 * (self.sample_rate as f64) / (sample_rate as f64);
            let i0 = (pos.floor() as usize).min(last);
            let i1 = (i0 + 1).min(last);
            let frac = (pos - i0 as f64) as f32;
            let (l0, r0) = frame_at(&self.data, channels, i0);
            let (l1, r1) = frame_at(&self.data, channels, i1);
            data.push(l0 * (1.0 - frac) + l1 * frac);
            data.push(r0 * (1.0 - frac) + r1 * frac);
        }
        Ok(Self {
            sample_rate,
            channels: 2,
            data,
        })
    }
}

fn frame_at(data: &[f32], channels: usize, frame: usize) -> (f32, f32) {
    if channels == 1 {
        let s = data[frame];
        (s, s)
    } else {
        (data[frame * 2], data[frame * 2 + 1])
    }
}
