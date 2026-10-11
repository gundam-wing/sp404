//! Headless entry point for agents and scripts.
//!
//! `sp404 catalog` prints every effect schema and preset. `sp404 render`
//! runs a WAV or a built-in demo through a chain JSON file and writes WAV
//! bytes. The DSP lives in `sp404-core`; this binary only does filesystem I/O.

#![forbid(unsafe_code)]

use std::env;
use std::io::{self, Write};
use std::process::ExitCode;

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

`catalog` prints effect schemas (ids, ranges, units, defaults) and presets.
`render` writes 16-bit stereo WAV. Use `--output -` to write WAV bytes to stdout.
Omitting `--chain` copies the input. Mono files are expanded to stereo.
A chain file is a JSON object `{{\"effects\":[...]}}` or a bare array of effects."
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
