//! Platform audio. This module is the native boundary: `cpal` is not linked
//! into `sp404-core`, and a missing output device falls back to a silent
//! clock so the window still opens and the engine still runs.
//!
//! Input capture is also here. The core only sees interleaved frames.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use sp404_core::Engine;

const MODE_IDLE: u8 = 0;
const MODE_INPUT: u8 = 1;
const MODE_RESAMPLE: u8 = 2;
const MAX_RECORD_SECONDS: f32 = 60.0;

pub struct Session {
    pub engine: Arc<Mutex<Engine>>,
    pub peak: Arc<AtomicU32>,
    pub status: String,
    capture: Arc<Capture>,
    inputs: Vec<NamedInput>,
    selected_input: usize,
    input_error: Option<String>,
    input_channels: u16,
    input_rate: f32,
    stream: Option<cpal::Stream>,
    input_stream: Option<cpal::Stream>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

/// One input device the picker can show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedInput {
    /// Label for the picker. Duplicates get a numeric suffix.
    pub name: String,
    /// Name reported by the host, used to reopen the device.
    host_name: String,
    /// Which device with `host_name` this is, starting at 0.
    ordinal: usize,
    /// Whether this was the host's default when the list was built.
    pub is_default: bool,
}

/// Frames copied out of a recording, still at the device rate.
#[derive(Debug, Clone)]
pub struct RawTake {
    /// Device sample rate.
    pub sample_rate: f32,
    /// Channels in `samples`.
    pub channels: u16,
    /// Interleaved frames.
    pub samples: Vec<f32>,
    /// True when the 60 second cap stopped the take early.
    pub capped: bool,
}

struct Capture {
    mode: AtomicU8,
    peak: AtomicU32,
    inner: Mutex<CaptureInner>,
}

struct CaptureInner {
    samples: Vec<f32>,
    channels: u16,
    sample_rate: f32,
    capped: bool,
}

impl Capture {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            mode: AtomicU8::new(MODE_IDLE),
            peak: AtomicU32::new(0.0_f32.to_bits()),
            inner: Mutex::new(CaptureInner {
                samples: Vec::new(),
                channels: 2,
                sample_rate: 44_100.0,
                capped: false,
            }),
        })
    }

    fn level(&self) -> f32 {
        f32::from_bits(self.peak.load(Ordering::Relaxed))
    }

    fn recording(&self) -> bool {
        self.mode.load(Ordering::Relaxed) != MODE_IDLE
    }

    fn begin(&self, mode: u8, sample_rate: f32, channels: u16) {
        let mut inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        inner.samples.clear();
        inner.channels = channels.max(1);
        inner.sample_rate = sample_rate;
        inner.capped = false;
        drop(inner);
        self.mode.store(mode, Ordering::Relaxed);
    }

    fn finish(&self) -> RawTake {
        self.mode.store(MODE_IDLE, Ordering::Relaxed);
        let mut inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        RawTake {
            sample_rate: inner.sample_rate,
            channels: inner.channels,
            samples: std::mem::take(&mut inner.samples),
            capped: inner.capped,
        }
    }

    fn push(&self, mode: u8, block: &[f32]) {
        if self.mode.load(Ordering::Relaxed) != mode || block.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        if self.mode.load(Ordering::Relaxed) != mode {
            return;
        }
        let max = ((inner.sample_rate * MAX_RECORD_SECONDS) as usize)
            .saturating_mul(inner.channels as usize);
        let room = max.saturating_sub(inner.samples.len());
        if room == 0 {
            inner.capped = true;
            return;
        }
        let count = block.len().min(room);
        inner.samples.extend_from_slice(&block[..count]);
        if count < block.len() {
            inner.capped = true;
        }
    }

    fn note_input(&self, block: &[f32]) {
        let level = block
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        self.peak.store(level.to_bits(), Ordering::Relaxed);
        self.push(MODE_INPUT, block);
    }
}

pub fn start() -> Session {
    let peak = Arc::new(AtomicU32::new(0));
    let capture = Capture::new();
    let mut session = match open_output(peak.clone(), capture.clone()) {
        Ok(opened) => opened,
        Err(err) => silent_clock(peak, capture, err),
    };
    if let Err(err) = session.refresh_inputs() {
        session.input_error = Some(err);
    }
    session
}

impl Session {
    /// Input devices from the last refresh.
    pub fn inputs(&self) -> &[NamedInput] {
        &self.inputs
    }

    /// Index into [`Session::inputs`], if any device is selected.
    pub fn selected_input(&self) -> Option<usize> {
        if self.inputs.is_empty() {
            None
        } else {
            Some(self.selected_input.min(self.inputs.len() - 1))
        }
    }

    /// Why the input stream is not open, when that is the case.
    pub fn input_error(&self) -> Option<&str> {
        self.input_error.as_deref()
    }

    /// Latest input peak, linear.
    pub fn input_level(&self) -> f32 {
        self.capture.level()
    }

    /// Whether a take is currently being written.
    pub fn recording(&self) -> bool {
        self.capture.recording()
    }

    /// Re-read the host's input list and open the default device for the meter.
    pub fn refresh_inputs(&mut self) -> Result<(), String> {
        if self.recording() {
            return Err("stop recording before refreshing inputs".into());
        }
        self.input_stream.take();
        self.inputs = list_inputs()?;
        let index = self
            .inputs
            .iter()
            .position(|device| device.is_default)
            .unwrap_or(0);
        self.select_input(index)
    }

    /// Open `index` for metering. Fails while a take is recording.
    pub fn select_input(&mut self, index: usize) -> Result<(), String> {
        if self.recording() {
            return Err("stop recording before changing the input".into());
        }
        if self.inputs.is_empty() {
            self.input_error = Some("no input device".into());
            return Err("no input device".into());
        }
        let index = index.min(self.inputs.len() - 1);
        let host_name = self.inputs[index].host_name.clone();
        let ordinal = self.inputs[index].ordinal;
        match open_input(&host_name, ordinal, self.capture.clone()) {
            Ok((stream, channels, rate)) => {
                self.input_stream = Some(stream);
                self.input_channels = channels;
                self.input_rate = rate;
                self.selected_input = index;
                self.input_error = None;
                Ok(())
            }
            Err(err) => {
                self.input_stream = None;
                self.input_error = Some(err.clone());
                self.selected_input = index;
                Err(err)
            }
        }
    }

    /// Record the open input onto a buffer at the device rate.
    pub fn start_input_record(&mut self) -> Result<(), String> {
        if self.input_stream.is_none() {
            if let Some(index) = self.selected_input() {
                self.select_input(index)?;
            } else {
                return Err(self
                    .input_error
                    .clone()
                    .unwrap_or_else(|| "no input device".into()));
            }
        }
        if let Some(err) = &self.input_error {
            return Err(err.clone());
        }
        self.capture
            .begin(MODE_INPUT, self.input_rate, self.input_channels);
        Ok(())
    }

    /// Record the engine master output, already stereo at the engine rate.
    pub fn start_resample_record(&self) -> Result<(), String> {
        let rate = self
            .engine
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .sample_rate();
        self.capture.begin(MODE_RESAMPLE, rate, 2);
        Ok(())
    }

    /// Stop and return the device-rate frames. The caller ingests them.
    pub fn stop_record(&self) -> Result<RawTake, String> {
        if !self.recording() {
            return Err("not recording".into());
        }
        let take = self.capture.finish();
        if take.samples.is_empty() {
            return Err("recorded no samples".into());
        }
        Ok(take)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.capture.mode.store(MODE_IDLE, Ordering::Relaxed);
        self.input_stream.take();
        self.running.store(false, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.stream.take();
    }
}

fn list_inputs() -> Result<Vec<NamedInput>, String> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|device| device.name().ok());
    let devices = host
        .input_devices()
        .map_err(|err| explain_input(&err.to_string()))?;
    let mut out: Vec<NamedInput> = Vec::new();
    for device in devices {
        let host_name = device.name().unwrap_or_else(|_| "input".to_string());
        let is_default = default_name.as_deref() == Some(host_name.as_str());
        let ordinal = out
            .iter()
            .filter(|existing| existing.host_name == host_name)
            .count();
        let name = if ordinal == 0 {
            host_name.clone()
        } else {
            format!("{host_name} ({})", ordinal + 1)
        };
        out.push(NamedInput {
            name,
            host_name,
            ordinal,
            is_default,
        });
    }
    if out.is_empty() {
        Err("no input device".into())
    } else {
        Ok(out)
    }
}

fn open_input(
    host_name: &str,
    ordinal: usize,
    capture: Arc<Capture>,
) -> Result<(cpal::Stream, u16, f32), String> {
    let host = cpal::default_host();
    let mut found = None;
    let mut seen = 0;
    for device in host
        .input_devices()
        .map_err(|err| explain_input(&err.to_string()))?
    {
        let device_name = device.name().unwrap_or_default();
        if device_name == host_name {
            if seen == ordinal {
                found = Some(device);
                break;
            }
            seen += 1;
        }
    }
    let device = found.ok_or_else(|| format!("input '{host_name}' disappeared"))?;
    let supported = device
        .default_input_config()
        .map_err(|err| explain_input(&err.to_string()))?;
    let channels = supported.channels();
    let rate = supported.sample_rate().0 as f32;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let stream = match format {
        SampleFormat::F32 => build_input_f32(&device, &config, capture)?,
        SampleFormat::I16 => build_input_i16(&device, &config, capture)?,
        SampleFormat::U16 => build_input_u16(&device, &config, capture)?,
        other => {
            return Err(format!(
                "unsupported input format {other:?}. {}",
                permission_note()
            ))
        }
    };
    stream
        .play()
        .map_err(|err| explain_input(&err.to_string()))?;
    Ok((stream, channels, rate))
}

fn build_input_f32(
    device: &cpal::Device,
    config: &StreamConfig,
    capture: Arc<Capture>,
) -> Result<cpal::Stream, String> {
    device
        .build_input_stream(
            config,
            move |data: &[f32], _| capture.note_input(data),
            |err| eprintln!("input stream error: {err}"),
            None,
        )
        .map_err(|err| explain_input(&err.to_string()))
}

fn build_input_i16(
    device: &cpal::Device,
    config: &StreamConfig,
    capture: Arc<Capture>,
) -> Result<cpal::Stream, String> {
    device
        .build_input_stream(
            config,
            move |data: &[i16], _| {
                let converted: Vec<f32> = data.iter().copied().map(i16_to_f32).collect();
                capture.note_input(&converted);
            },
            |err| eprintln!("input stream error: {err}"),
            None,
        )
        .map_err(|err| explain_input(&err.to_string()))
}

fn build_input_u16(
    device: &cpal::Device,
    config: &StreamConfig,
    capture: Arc<Capture>,
) -> Result<cpal::Stream, String> {
    device
        .build_input_stream(
            config,
            move |data: &[u16], _| {
                let converted: Vec<f32> = data.iter().copied().map(u16_to_f32).collect();
                capture.note_input(&converted);
            },
            |err| eprintln!("input stream error: {err}"),
            None,
        )
        .map_err(|err| explain_input(&err.to_string()))
}

pub(crate) fn i16_to_f32(sample: i16) -> f32 {
    (sample as f32 / i16::MAX as f32).clamp(-1.0, 1.0)
}

pub(crate) fn u16_to_f32(sample: u16) -> f32 {
    ((sample as f32 / u16::MAX as f32) * 2.0 - 1.0).clamp(-1.0, 1.0)
}

fn explain_input(err: &str) -> String {
    let lower = err.to_lowercase();
    if lower.contains("permission")
        || lower.contains("denied")
        || lower.contains("not authorized")
        || lower.contains("not permitted")
    {
        format!("{err}. {}", permission_note())
    } else {
        err.to_string()
    }
}

fn permission_note() -> &'static str {
    "On macOS, grant microphone access to the app that launched cargo (System Settings → Privacy & Security → Microphone). A binary from `cargo run` has no Info.plist; NSMicrophoneUsageDescription is only required once the sampler is bundled as an .app."
}

fn open_output(peak: Arc<AtomicU32>, capture: Arc<Capture>) -> Result<Session, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no default output device".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|err| err.to_string())?;
    let name = device.name().unwrap_or_else(|_| "output".to_string());
    let sample_rate = supported.sample_rate().0 as f32;
    let channels = supported.channels() as usize;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let engine = Arc::new(Mutex::new(Engine::new(sample_rate)));
    engine
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .prepare(8_192);

    let tap = OutputTap {
        engine: engine.clone(),
        peak: peak.clone(),
        capture: capture.clone(),
    };
    let stream = match format {
        SampleFormat::F32 => build_f32(&device, &config, channels, tap)?,
        SampleFormat::I16 => build_i16(&device, &config, channels, tap)?,
        SampleFormat::U16 => build_u16(&device, &config, channels, tap)?,
        other => return Err(format!("unsupported sample format {other:?}")),
    };
    stream.play().map_err(|err| err.to_string())?;
    Ok(Session {
        engine,
        peak,
        status: format!("{name} · {channels} ch · {sample_rate:.0} Hz"),
        capture,
        inputs: Vec::new(),
        selected_input: 0,
        input_error: None,
        input_channels: 1,
        input_rate: sample_rate,
        stream: Some(stream),
        input_stream: None,
        running: Arc::new(AtomicBool::new(false)),
        worker: None,
    })
}

fn silent_clock(peak: Arc<AtomicU32>, capture: Arc<Capture>, err: String) -> Session {
    let sample_rate = 44_100.0;
    let engine = Arc::new(Mutex::new(Engine::new(sample_rate)));
    engine
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .prepare(8_192);
    let running = Arc::new(AtomicBool::new(true));
    let flag = running.clone();
    let engine_thread = engine.clone();
    let peak_thread = peak.clone();
    let capture_thread = capture.clone();
    let worker = thread::Builder::new()
        .name("sp404-audio".into())
        .spawn(move || {
            let block = 256;
            let mut scratch = vec![0.0; block * 2];
            let period = Duration::from_secs_f64(block as f64 / sample_rate as f64);
            while flag.load(Ordering::Relaxed) {
                let start = Instant::now();
                {
                    let mut eng = engine_thread
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner());
                    eng.process(&mut scratch);
                }
                let level = scratch
                    .iter()
                    .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
                peak_thread.store(level.to_bits(), Ordering::Relaxed);
                capture_thread.push(MODE_RESAMPLE, &scratch);
                let elapsed = start.elapsed();
                if period > elapsed {
                    thread::sleep(period - elapsed);
                }
            }
        })
        .ok();
    Session {
        engine,
        peak,
        status: format!("No output device ({err}). Silent clock at 44100 Hz — render from the CLI to hear a file."),
        capture,
        inputs: Vec::new(),
        selected_input: 0,
        input_error: None,
        input_channels: 1,
        input_rate: sample_rate,
        stream: None,
        input_stream: None,
        running,
        worker,
    }
}

struct OutputTap {
    engine: Arc<Mutex<Engine>>,
    peak: Arc<AtomicU32>,
    capture: Arc<Capture>,
}

fn render_stereo(engine: &Mutex<Engine>, scratch: &mut Vec<f32>, frames: usize) -> f32 {
    scratch.resize(frames * 2, 0.0);
    {
        let mut eng = engine.lock().unwrap_or_else(|poison| poison.into_inner());
        eng.process(&mut scratch[..frames * 2]);
    }
    scratch
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
}

fn scatter(dst: &mut [f32], scratch: &[f32], channels: usize) {
    let channels = channels.max(1);
    let frames = dst.len() / channels;
    match channels {
        2 => dst[..frames * 2].copy_from_slice(&scratch[..frames * 2]),
        1 => {
            for frame in 0..frames {
                dst[frame] = 0.5 * (scratch[frame * 2] + scratch[frame * 2 + 1]);
            }
        }
        _ => {
            for frame in 0..frames {
                let base = frame * channels;
                dst[base] = scratch[frame * 2];
                if channels > 1 {
                    dst[base + 1] = scratch[frame * 2 + 1];
                }
                for channel in 2..channels {
                    dst[base + channel] = 0.0;
                }
            }
        }
    }
}

fn build_f32(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    tap: OutputTap,
) -> Result<cpal::Stream, String> {
    let mut scratch = Vec::new();
    device
        .build_output_stream(
            config,
            move |data: &mut [f32], _| {
                let frames = data.len() / channels.max(1);
                let level = render_stereo(&tap.engine, &mut scratch, frames);
                scatter(data, &scratch, channels);
                tap.peak.store(level.to_bits(), Ordering::Relaxed);
                tap.capture.push(MODE_RESAMPLE, &scratch[..frames * 2]);
            },
            |err| eprintln!("audio stream error: {err}"),
            None,
        )
        .map_err(|err| err.to_string())
}

fn build_i16(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    tap: OutputTap,
) -> Result<cpal::Stream, String> {
    let mut scratch = Vec::new();
    let mut mixed = Vec::new();
    device
        .build_output_stream(
            config,
            move |data: &mut [i16], _| {
                let frames = data.len() / channels.max(1);
                let level = render_stereo(&tap.engine, &mut scratch, frames);
                mixed.resize(data.len(), 0.0);
                scatter(&mut mixed, &scratch, channels);
                for (dst, src) in data.iter_mut().zip(mixed.iter()) {
                    *dst = (src.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                }
                tap.peak.store(level.to_bits(), Ordering::Relaxed);
                tap.capture.push(MODE_RESAMPLE, &scratch[..frames * 2]);
            },
            |err| eprintln!("audio stream error: {err}"),
            None,
        )
        .map_err(|err| err.to_string())
}

fn build_u16(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    tap: OutputTap,
) -> Result<cpal::Stream, String> {
    let mut scratch = Vec::new();
    let mut mixed = Vec::new();
    device
        .build_output_stream(
            config,
            move |data: &mut [u16], _| {
                let frames = data.len() / channels.max(1);
                let level = render_stereo(&tap.engine, &mut scratch, frames);
                mixed.resize(data.len(), 0.0);
                scatter(&mut mixed, &scratch, channels);
                for (dst, src) in data.iter_mut().zip(mixed.iter()) {
                    let centered = src.clamp(-1.0, 1.0) * 0.5 + 0.5;
                    *dst = (centered * u16::MAX as f32) as u16;
                }
                tap.peak.store(level.to_bits(), Ordering::Relaxed);
                tap.capture.push(MODE_RESAMPLE, &scratch[..frames * 2]);
            },
            |err| eprintln!("audio stream error: {err}"),
            None,
        )
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sp404_core::{ingest_take, TakeOptions};

    #[test]
    fn integer_input_samples_map_into_the_unit_interval() {
        assert!((i16_to_f32(0) - 0.0).abs() < 1.0e-6);
        assert!((i16_to_f32(i16::MAX) - 1.0).abs() < 1.0e-6);
        assert_eq!(i16_to_f32(i16::MIN), -1.0);
        assert!((u16_to_f32(u16::MAX / 2) - 0.0).abs() < 0.001);
        assert!((u16_to_f32(u16::MAX) - 1.0).abs() < 1.0e-4);
        assert!((u16_to_f32(0) + 1.0).abs() < 1.0e-4);
    }

    #[test]
    fn capture_appends_only_while_armed_and_stops_at_sixty_seconds() {
        let capture = Capture::new();
        capture.note_input(&[0.5, -0.5]);
        assert!(!capture.recording());
        assert!(capture.finish().samples.is_empty());

        capture.begin(MODE_INPUT, 1_000.0, 1);
        capture.note_input(&[0.2, 0.3, f32::NAN]);
        let take = capture.finish();
        assert_eq!(take.samples.len(), 3);
        assert_eq!(take.samples[0], 0.2);
        assert_eq!(take.samples[1], 0.3);
        assert!(take.samples[2].is_nan());
        assert_eq!(take.channels, 1);
        assert!(!take.capped);

        capture.begin(MODE_INPUT, 1_000.0, 1);
        let block = vec![0.1; 30_000];
        capture.note_input(&block);
        capture.note_input(&block);
        capture.note_input(&block);
        let take = capture.finish();
        assert_eq!(take.samples.len(), 60_000);
        assert!(take.capped);
    }

    #[test]
    fn permission_errors_mention_the_macos_prompt() {
        let denied = explain_input("permission denied");
        assert!(denied.contains("Info.plist"), "{denied}");
        assert!(denied.contains("NSMicrophoneUsageDescription"), "{denied}");
        assert_eq!(explain_input("device lost"), "device lost");
    }

    #[test]
    fn resample_through_the_session_clock_lands_a_pad_sample() {
        let mut session = start();
        assert!(
            session.inputs().is_empty()
                || session.input_error().is_some()
                || session.input_stream.is_some(),
            "input setup should report devices, an error, or an open stream"
        );
        let input_result = if session.inputs().is_empty() {
            session
                .input_error()
                .unwrap_or("no input device")
                .to_string()
        } else {
            "input device present".into()
        };
        eprintln!("input probe: {input_result}");

        session
            .engine
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .load_demo_bank();
        session.start_resample_record().unwrap();
        assert!(session
            .engine
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .trigger(0));
        thread::sleep(Duration::from_millis(250));
        let raw = session.stop_record().unwrap();
        assert_eq!(raw.channels, 2);
        let frames = raw.samples.len() / raw.channels as usize;
        assert!(
            (4_000..30_000).contains(&frames),
            "silent clock ran off realtime: {frames} frames in 250ms, capped={}",
            raw.capped
        );
        assert!(!raw.capped);
        let pad = ingest_take(
            raw.sample_rate,
            raw.channels,
            &raw.samples,
            TakeOptions {
                target_rate: 44_100.0,
                trim_silence: false,
                ..TakeOptions::default()
            },
        )
        .unwrap();
        assert!(pad.peak() > 0.02, "peak {}", pad.peak());
        assert!(session.start_input_record().is_err() || session.input_stream.is_some());
    }
}
