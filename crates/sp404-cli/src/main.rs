//! Headless entry point for agents and scripts.
//!
//! `sp404 catalog` prints every effect schema and preset. `sp404 render`
//! runs a WAV or a built-in demo through a chain JSON file and writes WAV
//! bytes. `sp404 analyze` reads a WAV and prints onset, tempo, key, and slice
//! JSON. The DSP lives in `sp404-core`. Analysis lives in `sp404-analyze`.
//! This binary only does filesystem I/O.

#![forbid(unsafe_code)]

use std::env;
use std::io::{self, Write};
use std::process::ExitCode;

use sp404_analyze::{analyze, AnalyzeOptions};
use sp404_core::plan::{frame_at, Arrangement, PlanTime};
use sp404_core::SampleSource;
use sp404_core::{catalog_json, effect_schema, presets_json, render, AudioBuffer, ChainDesc};

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("catalog") | Some("effects") => {
            println!("{}", catalog_json());
            ExitCode::SUCCESS
        }
        Some("presets") => {
            println!("{}", presets_json());
            ExitCode::SUCCESS
        }
        Some("schema") => match args.next() {
            Some(id) => match effect_schema(&id) {
                Some(schema) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(schema).expect("schema is finite")
                    );
                    ExitCode::SUCCESS
                }
                None => {
                    eprintln!("unknown effect '{id}'");
                    eprintln!("try `sp404 catalog` for the list");
                    ExitCode::from(1)
                }
            },
            None => {
                eprintln!("usage: sp404 schema <effect-id>");
                ExitCode::from(2)
            }
        },
        Some("demos") => {
            for name in AudioBuffer::demo_names() {
                println!("{name}");
            }
            ExitCode::SUCCESS
        }
        Some("render") => match render_cmd(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::from(1)
            }
        },
        Some("plan") => match plan_cmd(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::from(1)
            }
        },
        Some("analyze") => match analyze_cmd(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::from(1)
            }
        },
        Some("help") | Some("--help") | Some("-h") => {
            print_usage();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown command '{other}'");
            print_usage();
            ExitCode::from(2)
        }
        None => {
            print_usage();
            ExitCode::from(2)
        }
    }
}

fn print_usage() {
    eprintln!(
        "\
sp404 — headless pad sampler

Usage:
  sp404 catalog
  sp404 presets
  sp404 schema <effect-id>
  sp404 demos
  sp404 render --output <file.wav|-> (--input <file.wav> | --demo <name>) [--chain <chain.json>] [--sample-rate <hz>]
  sp404 plan validate <plan.json>
  sp404 plan render <plan.json> --output <file.wav|-> [--start <sec>] [--end <sec>] [--start-beat <beats>] [--end-beat <beats>]
  sp404 analyze <file.wav> [--hop-size <frames>] [--fft-size <frames>] [--min-bpm <bpm>] [--max-bpm <bpm>] [--fade-ms <ms>] [--zero-cross-ms <ms>]

`catalog` prints effect schemas (ids, ranges, units, defaults, automatable) and arrangement tracks.
`render` writes 16-bit stereo WAV. Use `--output -` to write WAV bytes to stdout.
Omitting `--chain` copies the input. Mono files are expanded to stereo.
A chain file is a JSON object `{{\"effects\":[...]}}` or a bare array of effects.
`plan validate` checks an arrangement. `plan render` bounces a time range of it.
The default range is the whole plan. Start and end accept seconds or beats, not both.
`analyze` reads a WAV and prints JSON to stdout: sample_rate, duration, bpm,
beat_times, onset_times, key, key_confidence, and slices (start, end, fade_in,
fade_out). Times are seconds. Defaults: hop 512, fft 2048, bpm 60..180, fade 5 ms,
zero-cross window 20 ms. Stereo is mixed to mono. A summary line goes to stderr."
    );
}

struct RenderArgs {
    input: Option<String>,
    demo: Option<String>,
    chain: Option<String>,
    output: String,
    sample_rate: Option<f32>,
}

fn render_cmd(args: impl Iterator<Item = String>) -> Result<(), String> {
    let args = parse_render(args)?;
    if args.input.is_some() == args.demo.is_some() {
        return Err("pass exactly one of --input or --demo".into());
    }
    let mut buffer = if let Some(demo) = &args.demo {
        let sr = args.sample_rate.unwrap_or(44_100.0);
        AudioBuffer::demo(demo, sr).ok_or_else(|| {
            format!(
                "unknown demo '{demo}'. known: {}",
                AudioBuffer::demo_names().join(", ")
            )
        })?
    } else {
        let path = args.input.as_deref().unwrap();
        let bytes = std::fs::read(path).map_err(|err| format!("reading {path}: {err}"))?;
        sp404_core::decode_wav(&bytes).map_err(|err| err.to_string())?
    };

    if let Some(sr) = args.sample_rate {
        if (buffer.sample_rate - sr).abs() > 0.5 {
            buffer = buffer.to_stereo(sr).map_err(|err| err.to_string())?;
        }
    }

    let chain = match &args.chain {
        Some(path) => {
            let text =
                std::fs::read_to_string(path).map_err(|err| format!("reading {path}: {err}"))?;
            ChainDesc::from_json(&text).map_err(|err| err.to_string())?
        }
        None => ChainDesc::default(),
    };
    let rendered = render(&buffer, &chain).map_err(|err| err.to_string())?;
    let wav = sp404_core::encode_wav(&rendered).map_err(|err| err.to_string())?;
    if args.output == "-" {
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(&wav)
            .map_err(|err| format!("writing stdout: {err}"))?;
    } else {
        std::fs::write(&args.output, &wav)
            .map_err(|err| format!("writing {}: {err}", args.output))?;
    }
    eprintln!(
        "rendered {} frames at {:.0} Hz, peak {:.3}, {} effect(s) -> {}",
        rendered.frames(),
        rendered.sample_rate,
        rendered.peak(),
        chain.effects.len(),
        args.output
    );
    Ok(())
}

fn plan_cmd(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    match args.next().as_deref() {
        Some("validate") => {
            let path = args.next().ok_or("usage: sp404 plan validate <plan.json>")?;
            let text = std::fs::read_to_string(&path).map_err(|err| format!("reading {path}: {err}"))?;
            let mut plan = Arrangement::from_json(&text).map_err(|err| err.to_string())?;
            inline_files(&mut plan, std::path::Path::new(&path))?;
            let frames = plan.duration_frames().map_err(|err| err.to_string())?;
            println!(
                "{}",
                serde_json::json!({
                    "ok": true,
                    "name": plan.name,
                    "frames": frames,
                    "sample_rate": plan.sample_rate,
                    "tempo": plan.tempo,
                    "pads": plan.pads.len(),
                    "triggers": plan.triggers.len(),
                })
            );
            Ok(())
        }
        Some("render") => plan_render(args),
        Some(other) => Err(format!(
            "unknown plan command '{other}'. try `sp404 plan validate` or `sp404 plan render`"
        )),
        None => Err("usage: sp404 plan validate <plan.json> | sp404 plan render <plan.json> --output <file>".into()),
    }
}

fn inline_files(plan: &mut Arrangement, plan_path: &std::path::Path) -> Result<(), String> {
    let base = plan_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    for pad in &mut plan.pads {
        let SampleSource::File { path } = &pad.sample else {
            continue;
        };
        let full = if std::path::Path::new(path).is_absolute() {
            std::path::PathBuf::from(path)
        } else {
            base.join(path)
        };
        let bytes =
            std::fs::read(&full).map_err(|err| format!("reading {}: {err}", full.display()))?;
        let buffer = sp404_core::decode_wav(&bytes)
            .map_err(|err| format!("decoding {}: {err}", full.display()))?;
        pad.sample = SampleSource::Pcm {
            sample_rate: buffer.sample_rate,
            channels: buffer.channels as u8,
            samples: buffer.data,
        };
    }
    plan.validate().map_err(|err| err.to_string())?;
    Ok(())
}

struct PlanRenderArgs {
    path: String,
    output: String,
    start: Option<PlanTime>,
    end: Option<PlanTime>,
}

fn plan_render(args: impl Iterator<Item = String>) -> Result<(), String> {
    let args = parse_plan_render(args)?;
    let text = std::fs::read_to_string(&args.path)
        .map_err(|err| format!("reading {}: {err}", args.path))?;
    let mut plan = Arrangement::from_json(&text).map_err(|err| err.to_string())?;
    inline_files(&mut plan, std::path::Path::new(&args.path))?;
    let start = match args.start {
        Some(time) => frame_at(&plan, time).map_err(|err| err.to_string())?,
        None => 0,
    };
    let end = match args.end {
        Some(time) => frame_at(&plan, time).map_err(|err| err.to_string())?,
        None => plan.duration_frames().map_err(|err| err.to_string())?,
    };
    let rendered = plan.render(start, end).map_err(|err| err.to_string())?;
    let wav = sp404_core::encode_wav(&rendered).map_err(|err| err.to_string())?;
    if args.output == "-" {
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(&wav)
            .map_err(|err| format!("writing stdout: {err}"))?;
    } else {
        std::fs::write(&args.output, &wav)
            .map_err(|err| format!("writing {}: {err}", args.output))?;
    }
    eprintln!(
        "rendered plan '{}' frames {}..{} ({} frames) at {} Hz, peak {:.3} -> {}",
        plan.name,
        start,
        end,
        rendered.frames(),
        plan.sample_rate,
        rendered.peak(),
        args.output
    );
    Ok(())
}

fn parse_plan_render(args: impl Iterator<Item = String>) -> Result<PlanRenderArgs, String> {
    let args: Vec<String> = args.collect();
    if args.is_empty() {
        return Err("usage: sp404 plan render <plan.json> --output <file.wav>".into());
    }
    let path = args[0].clone();
    if path.starts_with("--") {
        return Err("plan path must come before flags".into());
    }
    let mut output = None;
    let mut start_sec = None;
    let mut end_sec = None;
    let mut start_beat = None;
    let mut end_beat = None;
    let mut index = 1;
    while index < args.len() {
        let flag = args[index].as_str();
        let value = args
            .get(index + 1)
            .cloned()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        let number = |flag: &str, value: &str| -> Result<f64, String> {
            value
                .parse()
                .map_err(|_| format!("invalid number '{value}' for {flag}"))
        };
        match flag {
            "--output" | "-o" => output = Some(value),
            "--start" => start_sec = Some(number(flag, &value)?),
            "--end" => end_sec = Some(number(flag, &value)?),
            "--start-beat" => start_beat = Some(number(flag, &value)?),
            "--end-beat" => end_beat = Some(number(flag, &value)?),
            other => return Err(format!("unknown flag '{other}'")),
        }
        index += 2;
    }
    if start_sec.is_some() && start_beat.is_some() {
        return Err("pass only one of --start and --start-beat".into());
    }
    if end_sec.is_some() && end_beat.is_some() {
        return Err("pass only one of --end and --end-beat".into());
    }
    let start = match (start_sec, start_beat) {
        (Some(seconds), None) => Some(PlanTime::seconds(seconds)),
        (None, Some(beats)) => Some(PlanTime::beats(beats)),
        _ => None,
    };
    let end = match (end_sec, end_beat) {
        (Some(seconds), None) => Some(PlanTime::seconds(seconds)),
        (None, Some(beats)) => Some(PlanTime::beats(beats)),
        _ => None,
    };
    let output = output.ok_or("missing --output")?;
    Ok(PlanRenderArgs {
        path,
        output,
        start,
        end,
    })
}

struct AnalyzeArgs {
    path: String,
    hop_size: Option<usize>,
    fft_size: Option<usize>,
    min_bpm: Option<f64>,
    max_bpm: Option<f64>,
    fade_ms: Option<f64>,
    zero_cross_ms: Option<f64>,
}

fn analyze_cmd(args: impl Iterator<Item = String>) -> Result<(), String> {
    let args: Vec<String> = args.collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_usage();
        return Ok(());
    }
    let args = parse_analyze(args)?;
    let bytes = std::fs::read(&args.path).map_err(|err| format!("reading {}: {err}", args.path))?;
    let buffer = sp404_core::decode_wav(&bytes).map_err(|err| err.to_string())?;
    let mono = mix_mono(&buffer);
    let mut options = AnalyzeOptions::default();
    if let Some(hop) = args.hop_size {
        options.hop_size = hop;
    }
    if let Some(fft) = args.fft_size {
        options.fft_size = fft;
    }
    if let Some(min_bpm) = args.min_bpm {
        options.min_bpm = min_bpm;
    }
    if let Some(max_bpm) = args.max_bpm {
        options.max_bpm = max_bpm;
    }
    if let Some(fade_ms) = args.fade_ms {
        options.fade_ms = fade_ms;
    }
    if let Some(zero_cross_ms) = args.zero_cross_ms {
        options.zero_cross_ms = zero_cross_ms;
    }
    let analysis = analyze(&mono, buffer.sample_rate, &options).map_err(|err| err.to_string())?;
    println!("{}", analysis.to_json());
    eprintln!(
        "analyzed {:.0} Hz, {:.3} s, {:.2} bpm, {}, {} onsets, {} beats, {} slices",
        analysis.sample_rate,
        analysis.duration,
        analysis.bpm,
        analysis.key,
        analysis.onset_times.len(),
        analysis.beat_times.len(),
        analysis.slices.len()
    );
    Ok(())
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

fn parse_analyze(args: Vec<String>) -> Result<AnalyzeArgs, String> {
    if args.is_empty() {
        return Err(
            "usage: sp404 analyze <file.wav> [--hop-size <frames>] [--fft-size <frames>] [--min-bpm <bpm>] [--max-bpm <bpm>] [--fade-ms <ms>] [--zero-cross-ms <ms>]"
                .into(),
        );
    }
    let path = args[0].clone();
    if path.starts_with("--") {
        return Err("audio path must come before flags".into());
    }
    let mut hop_size = None;
    let mut fft_size = None;
    let mut min_bpm = None;
    let mut max_bpm = None;
    let mut fade_ms = None;
    let mut zero_cross_ms = None;
    let mut index = 1;
    while index < args.len() {
        let flag = args[index].as_str();
        let value = args
            .get(index + 1)
            .cloned()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        let integer = |flag: &str, value: &str| -> Result<usize, String> {
            value
                .parse()
                .map_err(|_| format!("invalid integer '{value}' for {flag}"))
        };
        let number = |flag: &str, value: &str| -> Result<f64, String> {
            value
                .parse()
                .map_err(|_| format!("invalid number '{value}' for {flag}"))
        };
        match flag {
            "--hop-size" => hop_size = Some(integer(flag, &value)?),
            "--fft-size" => fft_size = Some(integer(flag, &value)?),
            "--min-bpm" => min_bpm = Some(number(flag, &value)?),
            "--max-bpm" => max_bpm = Some(number(flag, &value)?),
            "--fade-ms" => fade_ms = Some(number(flag, &value)?),
            "--zero-cross-ms" => zero_cross_ms = Some(number(flag, &value)?),
            other => return Err(format!("unknown flag '{other}'")),
        }
        index += 2;
    }
    Ok(AnalyzeArgs {
        path,
        hop_size,
        fft_size,
        min_bpm,
        max_bpm,
        fade_ms,
        zero_cross_ms,
    })
}

fn parse_render(args: impl Iterator<Item = String>) -> Result<RenderArgs, String> {
    let args: Vec<String> = args.collect();
    let mut input = None;
    let mut demo = None;
    let mut chain = None;
    let mut output = None;
    let mut sample_rate = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let value = args
            .get(index + 1)
            .cloned()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag {
            "--input" | "-i" => input = Some(value),
            "--demo" | "-d" => demo = Some(value),
            "--chain" | "-c" => chain = Some(value),
            "--output" | "-o" => output = Some(value),
            "--sample-rate" | "--sr" => {
                let sr: f32 = value
                    .parse()
                    .map_err(|_| format!("invalid sample rate '{value}'"))?;
                sample_rate = Some(sr);
            }
            other => return Err(format!("unknown flag '{other}'")),
        }
        index += 2;
    }
    let output = output.ok_or("missing --output")?;
    Ok(RenderArgs {
        input,
        demo,
        chain,
        output,
        sample_rate,
    })
}
