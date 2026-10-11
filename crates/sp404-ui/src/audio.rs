//! Platform audio. This module is the native boundary: `cpal` is not linked
//! into `sp404-core`, and a missing device falls back to a silent clock so
//! the window still opens and the engine still runs.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use sp404_core::Engine;

pub struct Session {
    pub engine: Arc<Mutex<Engine>>,
    pub peak: Arc<AtomicU32>,
    pub status: String,
    stream: Option<cpal::Stream>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

pub fn start() -> Session {
    let peak = Arc::new(AtomicU32::new(0));
    match open_device(peak.clone()) {
        Ok(opened) => opened,
        Err(err) => silent_clock(peak, err),
    }
}

fn open_device(peak: Arc<AtomicU32>) -> Result<Session, String> {
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

    let stream = match format {
        SampleFormat::F32 => build_f32(&device, &config, channels, engine.clone(), peak.clone())?,
        SampleFormat::I16 => build_i16(&device, &config, channels, engine.clone(), peak.clone())?,
        SampleFormat::U16 => build_u16(&device, &config, channels, engine.clone(), peak.clone())?,
        other => return Err(format!("unsupported sample format {other:?}")),
    };
    stream.play().map_err(|err| err.to_string())?;
    Ok(Session {
        engine,
        peak,
        status: format!("{name} · {channels} ch · {sample_rate:.0} Hz"),
        stream: Some(stream),
        running: Arc::new(AtomicBool::new(false)),
        worker: None,
    })
}

fn silent_clock(peak: Arc<AtomicU32>, err: String) -> Session {
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
        stream: None,
        running,
        worker,
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.stream.take();
    }
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
    engine: Arc<Mutex<Engine>>,
    peak: Arc<AtomicU32>,
) -> Result<cpal::Stream, String> {
    let mut scratch = Vec::new();
    device
        .build_output_stream(
            config,
            move |data: &mut [f32], _| {
                let frames = data.len() / channels.max(1);
                let level = render_stereo(&engine, &mut scratch, frames);
                scatter(data, &scratch, channels);
                peak.store(level.to_bits(), Ordering::Relaxed);
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
    engine: Arc<Mutex<Engine>>,
    peak: Arc<AtomicU32>,
) -> Result<cpal::Stream, String> {
    let mut scratch = Vec::new();
    let mut mixed = Vec::new();
    device
        .build_output_stream(
            config,
            move |data: &mut [i16], _| {
                let frames = data.len() / channels.max(1);
                let level = render_stereo(&engine, &mut scratch, frames);
                mixed.resize(data.len(), 0.0);
                scatter(&mut mixed, &scratch, channels);
                for (dst, src) in data.iter_mut().zip(mixed.iter()) {
                    *dst = (src.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                }
                peak.store(level.to_bits(), Ordering::Relaxed);
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
    engine: Arc<Mutex<Engine>>,
    peak: Arc<AtomicU32>,
) -> Result<cpal::Stream, String> {
    let mut scratch = Vec::new();
    let mut mixed = Vec::new();
    device
        .build_output_stream(
            config,
            move |data: &mut [u16], _| {
                let frames = data.len() / channels.max(1);
                let level = render_stereo(&engine, &mut scratch, frames);
                mixed.resize(data.len(), 0.0);
                scatter(&mut mixed, &scratch, channels);
                for (dst, src) in data.iter_mut().zip(mixed.iter()) {
                    let centered = src.clamp(-1.0, 1.0) * 0.5 + 0.5;
                    *dst = (centered * u16::MAX as f32) as u16;
                }
                peak.store(level.to_bits(), Ordering::Relaxed);
            },
            |err| eprintln!("audio stream error: {err}"),
            None,
        )
        .map_err(|err| err.to_string())
}
