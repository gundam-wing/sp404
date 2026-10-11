//! Public-API tests: identity, bounds, stability, catalog, sampler, WAV.

use std::collections::HashSet;
use std::time::Instant;

use sp404_core::{
    catalog_json, chain_presets, create_effect, decode_wav, effect_schema, effect_schemas,
    encode_wav, render, render_json, render_performance, AudioBuffer, ChainDesc, Effect,
    EffectDesc, EffectSchema, Engine, Error, NoteEvent, ParamScale, Registry, PAD_COUNT,
};

fn sine(freq: f32, frames: usize, amp: f32) -> AudioBuffer {
    let sr = 44_100.0;
    let mut data = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let t = i as f32 / sr;
        let s = (std::f32::consts::TAU * freq * t).sin() * amp;
        data.push(s);
        data.push(s * 0.85);
    }
    AudioBuffer::new(sr, 2, data).unwrap()
}

fn assert_close(actual: &[f32], expected: &[f32], eps: f32) {
    assert_eq!(actual.len(), expected.len());
    let max = actual
        .iter()
        .zip(expected)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    assert!(max <= eps, "max abs diff {max} exceeds {eps}");
}

fn assert_finite(samples: &[f32], bound: f32) {
    for (index, sample) in samples.iter().enumerate() {
        assert!(sample.is_finite(), "non-finite sample at {index}: {sample}");
        assert!(
            sample.abs() <= bound,
            "sample {sample} at {index} exceeds bound {bound}"
        );
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

#[test]
fn catalog_lists_every_builtin_with_valid_ranges() {
    let schemas = effect_schemas();
    assert!(schemas.len() >= 8);
    let mut ids = HashSet::new();
    for schema in &schemas {
        assert!(ids.insert(schema.id), "duplicate id {}", schema.id);
        assert!(!schema.name.is_empty());
        assert!(!schema.description.is_empty());
        let mut params = HashSet::new();
        for param in schema.params {
            assert!(params.insert(param.id), "{} param {}", schema.id, param.id);
            assert!(param.min <= param.default && param.default <= param.max);
            assert!(param.min < param.max);
            match param.scale {
                ParamScale::Choice => {
                    let count = (param.max - param.min).round() as usize + 1;
                    assert_eq!(param.choices.len(), count, "{}", param.id);
                }
                ParamScale::Logarithmic => assert!(param.min > 0.0, "{}", param.id),
                ParamScale::Linear => {}
            }
        }
    }
    assert!(effect_schema("filter").is_some());
    assert!(effect_schema("not-an-effect").is_none());

    let catalog: serde_json::Value = serde_json::from_str(&catalog_json()).unwrap();
    assert_eq!(catalog["version"], 1);
    let listed = catalog["effects"].as_array().unwrap();
    assert_eq!(listed.len(), schemas.len());
    for schema in &schemas {
        assert!(
            catalog_json().contains(&format!("\"id\": \"{}\"", schema.id)),
            "catalog missing {}",
            schema.id
        );
    }
}

#[test]
fn presets_instantiate_and_match_the_example_files() {
    assert!(!chain_presets().is_empty());
    for preset in chain_presets() {
        let desc = ChainDesc::from_json(preset.chain_json).unwrap_or_else(|err| {
            panic!("preset {} failed to parse: {err}", preset.id);
        });
        let chain = desc.instantiate().unwrap();
        assert_eq!(chain.len(), desc.effects.len());
    }
    let dub = include_str!("../../../examples/chains/dub-delay.json");
    let from_file = ChainDesc::from_json(dub).unwrap();
    let from_preset = chain_presets()
        .iter()
        .find(|preset| preset.id == "dub-delay")
        .unwrap();
    assert_eq!(
        from_file,
        ChainDesc::from_json(from_preset.chain_json).unwrap()
    );
}

#[test]
fn chain_json_accepts_object_array_and_id_alias() {
    let object =
        ChainDesc::from_json(r#"{"effects":[{"effect":"gain","params":{"gain_db":-3.0}}]}"#)
            .unwrap();
    let array = ChainDesc::from_json(r#"[{"id":"gain","params":{"gain_db":-3}}]"#).unwrap();
    let kind = ChainDesc::from_json(r#"[{"kind":"gain","bypass":true}]"#).unwrap();
    assert_eq!(object.effects[0].effect, "gain");
    assert_eq!(array.effects[0].effect, "gain");
    assert!(kind.effects[0].bypass);
    assert!(ChainDesc::from_json("{").is_err());
    assert!(matches!(
        ChainDesc::from_json(r#"{"effects":[{"effect":"nope"}]}"#)
            .unwrap()
            .instantiate(),
        Err(Error::UnknownEffect { .. })
    ));
}

#[test]
fn chain_round_trips_through_json() {
    let desc = ChainDesc::from_json(include_str!("../../../examples/chains/vinyl.json")).unwrap();
    let live = desc.instantiate().unwrap().to_desc();
    let restored = ChainDesc::from_json(&live.to_json().unwrap())
        .unwrap()
        .instantiate()
        .unwrap()
        .to_desc();
    assert_eq!(live, restored);
}

#[test]
fn parameters_clamp_and_reject_unknown_or_non_finite_values() {
    let mut chain = ChainDesc::from_json(r#"{"effects":[{"effect":"filter"}]}"#)
        .unwrap()
        .instantiate()
        .unwrap();
    chain.set_param(0, "cutoff", 1.0e9).unwrap();
    assert_eq!(chain.to_desc().effects[0].params["cutoff"], 20_000.0);
    chain.set_param(0, "cutoff", 1.0).unwrap();
    assert_eq!(chain.to_desc().effects[0].params["cutoff"], 20.0);
    chain.set_param(0, "mode", 1.6).unwrap();
    assert_eq!(chain.to_desc().effects[0].params["mode"], 2.0);
    chain.set_param(0, "mode", 0.4).unwrap();
    assert_eq!(chain.to_desc().effects[0].params["mode"], 0.0);
    assert!(matches!(
        chain.set_param(0, "nope", 1.0),
        Err(Error::UnknownParam { .. })
    ));
    assert!(chain.set_param(0, "cutoff", f32::NAN).is_err());
    assert!(chain.set_param(4, "cutoff", 100.0).is_err());
}

#[test]
fn empty_chain_and_bypass_are_identity() {
    let input = sine(440.0, 2_048, 0.4);
    let dry = render(&input, &ChainDesc::default()).unwrap();
    assert_close(&dry.data, &input.data, 0.0);

    for schema in effect_schemas() {
        let desc = ChainDesc {
            effects: vec![EffectDesc {
                effect: schema.id.to_string(),
                bypass: true,
                params: Default::default(),
            }],
        };
        let out = render(&input, &desc).unwrap();
        assert_close(&out.data, &input.data, 0.0);
    }
}

#[test]
fn unity_settings_leave_the_signal_unchanged() {
    let input = sine(330.0, 2_048, 0.35);
    let cases = [
        r#"{"effects":[{"effect":"gain"}]}"#,
        r#"{"effects":[{"effect":"gain","params":{"gain_db":0}}]}"#,
        r#"{"effects":[{"effect":"filter","params":{"mix":0}}]}"#,
        r#"{"effects":[{"effect":"isolator"}]}"#,
        r#"{"effects":[{"effect":"compressor","params":{"ratio":1,"makeup_db":0}}]}"#,
        r#"{"effects":[{"effect":"lofi","params":{"mix":0}}]}"#,
        r#"{"effects":[{"effect":"delay","params":{"mix":0}}]}"#,
        r#"{"effects":[{"effect":"reverb","params":{"mix":0}}]}"#,
        r#"{"effects":[{"effect":"vinyl","params":{"wow":0,"flutter":0,"crackle":0,"wear":0,"mix":1}}]}"#,
        r#"{"effects":[{"effect":"pitch"}]}"#,
        r#"{"effects":[{"effect":"pitch","params":{"semitones":0,"mix":1}}]}"#,
    ];
    for json in cases {
        let out = render_json(&input, json).unwrap();
        assert_close(&out.data, &input.data, 1.0e-5);
    }
}

#[test]
fn gain_matches_decibel_conversion() {
    let input = sine(440.0, 512, 0.2);
    let out = render_json(
        &input,
        r#"{"effects":[{"effect":"gain","params":{"gain_db":6}}]}"#,
    )
    .unwrap();
    let expected = 10.0_f32.powf(6.0 / 20.0);
    let ratio = out.data[200] / input.data[200];
    assert!((ratio - expected).abs() < 1.0e-4, "ratio {ratio}");
}

#[test]
fn lowpass_attenuates_high_frequencies() {
    let chain = r#"{"effects":[{"effect":"filter","params":{"mode":0,"cutoff":500,"resonance":0,"mix":1}}]}"#;
    let low = render_json(&sine(180.0, 8_192, 0.5), chain).unwrap();
    let high = render_json(&sine(8_000.0, 8_192, 0.5), chain).unwrap();
    let low_rms = rms(&low.data[low.data.len() / 2..]);
    let high_rms = rms(&high.data[high.data.len() / 2..]);
    assert!(low_rms > high_rms * 8.0, "low {low_rms} high {high_rms}");
}

#[test]
fn compressor_reduces_a_hot_signal() {
    let input = sine(220.0, 44_100, 0.9);
    let out = render_json(
        &input,
        r#"{"effects":[{"effect":"compressor","params":{"threshold_db":-20,"ratio":12,"attack_ms":1,"release_ms":50,"makeup_db":0,"mix":1}}]}"#,
    )
    .unwrap();
    let tail = &out.data[out.data.len() * 3 / 4..];
    let peak = tail.iter().fold(0.0_f32, |p, s| p.max(s.abs()));
    assert!(peak < 0.35, "compressed peak {peak}");
    assert!(peak > 0.01, "compressor silenced the signal");
}

#[test]
fn delay_places_an_impulse_at_the_delay_time() {
    let impulse = AudioBuffer::demo("impulse", 44_100.0).unwrap();
    let out = render_json(
        &impulse,
        r#"{"effects":[{"effect":"delay","params":{"time_ms":100,"feedback":0,"tone":1,"ping_pong":0,"mix":1}}]}"#,
    )
    .unwrap();
    assert!(out.data[0].abs() < 1.0e-4);
    let at = 4_410 * 2;
    assert!(
        (out.data[at] - 1.0).abs() < 1.0e-3,
        "delayed sample {}",
        out.data[at]
    );
}

#[test]
fn reverb_has_a_tail_after_an_impulse() {
    let impulse = AudioBuffer::demo("impulse", 44_100.0).unwrap();
    let out = render_json(
        &impulse,
        r#"{"effects":[{"effect":"reverb","params":{"room":0.85,"damp":0.2,"width":1,"mix":1}}]}"#,
    )
    .unwrap();
    let energy: f32 = out.data[4_000..16_000].iter().map(|s| s * s).sum();
    assert!(energy > 1.0e-4, "reverb tail energy {energy}");
}

#[test]
fn lofi_holds_samples_and_quantizes() {
    let mut data = Vec::new();
    for frame in 0..8 {
        let sample = if frame < 4 { 0.75 } else { -0.25 };
        data.push(sample);
        data.push(sample);
    }
    let input = AudioBuffer::new(44_100.0, 2, data).unwrap();
    let held = render_json(
        &input,
        r#"{"effects":[{"effect":"lofi","params":{"bit_depth":16,"downsample":4,"mix":1}}]}"#,
    )
    .unwrap();
    for frame in 0..4 {
        assert!((held.data[frame * 2] - 0.75).abs() < 1.0e-4);
    }
    for frame in 4..8 {
        assert!((held.data[frame * 2] + 0.25).abs() < 1.0e-4);
    }

    let constant = AudioBuffer::new(44_100.0, 2, vec![0.4; 32]).unwrap();
    let crushed = render_json(
        &constant,
        r#"{"effects":[{"effect":"lofi","params":{"bit_depth":2,"downsample":1,"mix":1}}]}"#,
    )
    .unwrap();
    for sample in &crushed.data {
        let steps = (sample * 2.0).round();
        assert!((sample * 2.0 - steps).abs() < 1.0e-4);
    }
    assert!(crushed.data.iter().any(|s| (s.abs() - 0.5).abs() < 1.0e-4));
}

#[test]
fn every_effect_stays_finite_at_parameter_extremes() {
    let mut stimulus = sine(220.0, 2_048, 0.5).data;
    stimulus[0] = 1.0;
    stimulus[1] = -1.0;
    stimulus[10] = 1.0e-30;
    stimulus[11] = -1.0e-30;
    stimulus[20] = 1.0;
    stimulus[21] = -1.0;
    let started = Instant::now();
    for schema in effect_schemas() {
        for extreme in [ParamEnd::Min, ParamEnd::Max, ParamEnd::Default] {
            let mut effect = create_effect(&EffectDesc {
                effect: schema.id.to_string(),
                bypass: false,
                params: Default::default(),
            })
            .unwrap();
            for param in schema.params {
                let value = match extreme {
                    ParamEnd::Min => param.min,
                    ParamEnd::Max => param.max,
                    ParamEnd::Default => param.default,
                };
                effect.set_param(param.id, value).unwrap();
            }
            let mut block = stimulus.clone();
            effect.process(&mut block, 44_100.0);
            // +24 dB on a full-scale sample is about 15.8. Anything past that is a blow-up.
            assert_finite(&block, 20.0);
            let mut quiet = vec![1.0e-30; 8_192];
            effect.process(&mut quiet, 44_100.0);
            assert_finite(&quiet, 1.0);
            effect.reset();
        }
    }
    assert!(
        started.elapsed().as_secs() < 10,
        "parameter sweep took {:?}; denormal blowup?",
        started.elapsed()
    );
}

enum ParamEnd {
    Min,
    Max,
    Default,
}

#[test]
fn feedback_effects_do_not_explode() {
    let mut hot = vec![0.8; 44_100 * 2];
    let mut delay = create_effect(&EffectDesc {
        effect: "delay".into(),
        bypass: false,
        params: Default::default(),
    })
    .unwrap();
    delay.set_param("feedback", 0.95).unwrap();
    delay.set_param("mix", 1.0).unwrap();
    delay.set_param("time_ms", 40.0).unwrap();
    delay.process(&mut hot, 44_100.0);
    assert_finite(&hot, 4.0);

    let mut room = vec![0.5; 16_384];
    let mut reverb = create_effect(&EffectDesc {
        effect: "reverb".into(),
        bypass: false,
        params: Default::default(),
    })
    .unwrap();
    reverb.set_param("room", 1.0).unwrap();
    reverb.set_param("mix", 1.0).unwrap();
    reverb.process(&mut room, 44_100.0);
    assert_finite(&room, 8.0);
}

#[test]
fn render_replaces_non_finite_samples() {
    let mut data = vec![0.2; 64];
    data[2] = f32::NAN;
    data[3] = f32::INFINITY;
    let input = AudioBuffer::new(44_100.0, 2, data).unwrap();
    let out = render_json(
        &input,
        r#"{"effects":[{"effect":"gain","params":{"gain_db":3}}]}"#,
    )
    .unwrap();
    assert!(out.data.iter().all(|sample| sample.is_finite()));
    assert!(out.peak() > 0.0);
}

#[test]
fn mono_input_renders_to_the_same_frame_count() {
    let mono = AudioBuffer::new(44_100.0, 1, vec![0.25, -0.5, 0.0, 1.0]).unwrap();
    let out = render(&mono, &ChainDesc::default()).unwrap();
    assert_eq!(out.channels, 2);
    assert_eq!(out.frames(), mono.frames());
    assert_close(
        &out.data,
        &[0.25, 0.25, -0.5, -0.5, 0.0, 0.0, 1.0, 1.0],
        1.0e-6,
    );
}

#[test]
fn wav_round_trip_preserves_a_sine() {
    let input = sine(440.0, 1_024, 0.5);
    let bytes = encode_wav(&input).unwrap();
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let decoded = decode_wav(&bytes).unwrap();
    assert_eq!(decoded.channels, 2);
    assert!((decoded.sample_rate - 44_100.0).abs() < 0.5);
    assert_eq!(decoded.frames(), input.frames());
    assert_close(&decoded.data, &input.data, 1.0 / 32_768.0);
    assert!(decode_wav(b"not a wav").is_err());
}

#[test]
fn external_effect_can_join_a_registry() {
    let mut registry = Registry::builtin();
    registry.register(&DOUBLE_SCHEMA, || Box::new(Double));
    let desc = ChainDesc::from_json(
        r#"{"effects":[{"effect":"double"},{"effect":"gain","params":{"gain_db":0}}]}"#,
    )
    .unwrap();
    let mut chain = registry.chain(&desc).unwrap();
    let mut data = vec![0.25, -0.5, 0.1, -0.1];
    chain.process(&mut data, 44_100.0);
    assert_close(&data, &[0.5, -1.0, 0.2, -0.2], 1.0e-6);
    assert!(registry
        .create(&EffectDesc {
            effect: "missing".into(),
            bypass: false,
            params: Default::default(),
        })
        .is_err());
}

static DOUBLE_SCHEMA: EffectSchema = EffectSchema {
    id: "double",
    name: "Double",
    description: "Test-only effect that doubles every sample.",
    params: &[],
};

struct Double;

impl Effect for Double {
    fn schema(&self) -> &'static EffectSchema {
        &DOUBLE_SCHEMA
    }
    fn process(&mut self, samples: &mut [f32], _: f32) {
        for sample in samples {
            *sample *= 2.0;
        }
    }
    fn set_param(&mut self, id: &str, _: f32) -> Result<(), Error> {
        Err(Error::UnknownParam {
            effect: "double".into(),
            param: id.into(),
        })
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn set_bypassed(&mut self, _: bool) {}
    fn is_bypassed(&self) -> bool {
        false
    }
    fn reset(&mut self) {}
}

#[test]
fn engine_plays_a_loaded_sample_without_changing_it() {
    let kick = AudioBuffer::demo("kick", 44_100.0).unwrap();
    let mut engine = Engine::new(44_100.0);
    engine.load_pad(0, kick.clone(), "kick").unwrap();
    assert!(engine.trigger(0));
    let mut output = vec![0.0; kick.frames() * 2];
    engine.process(&mut output);
    assert_close(&output, &kick.data, 1.0e-5);

    engine.stop_all();
    engine
        .set_bus_chain(
            &ChainDesc::from_json(r#"{"effects":[{"effect":"gain","params":{"gain_db":-12}}]}"#)
                .unwrap(),
        )
        .unwrap();
    assert!(engine.trigger(0));
    let mut quieter = vec![0.0; kick.frames() * 2];
    engine.process(&mut quieter);
    assert!(quieter.iter().fold(0.0_f32, |p, s| p.max(s.abs())) < kick.peak() * 0.4);
}

#[test]
fn demo_bank_performance_and_pad_errors() {
    let mut engine = Engine::new(44_100.0);
    assert!(engine
        .load_pad(PAD_COUNT, sine(100.0, 32, 0.1), "x")
        .is_err());
    assert!(!engine.trigger(1));
    engine.load_demo_bank();
    for index in 0..PAD_COUNT {
        let view = engine.pad_view(index).unwrap();
        assert!(view.has_sample, "pad {index}");
        assert!(!view.name.is_empty());
    }
    let wave = engine.waveform(0, 32).unwrap();
    assert_eq!(wave.len(), 32);
    assert!(wave.iter().any(|bin| *bin > 0.0));

    let rendered = render_performance(
        &mut engine,
        &[
            NoteEvent { frame: 0, pad: 0 },
            NoteEvent {
                frame: 4_000,
                pad: 1,
            },
            NoteEvent {
                frame: 99_999,
                pad: 2,
            },
        ],
        8_000,
    );
    assert_eq!(rendered.len(), 16_000);
    assert!(rendered.iter().any(|sample| sample.abs() > 0.05));
    assert!(rendered.iter().all(|sample| sample.is_finite()));

    for name in AudioBuffer::demo_names() {
        let demo = AudioBuffer::demo(name, 44_100.0).unwrap();
        assert!(demo.frames() > 0, "{name}");
        assert_eq!(demo.channels, 2);
        assert!(demo.data.iter().all(|sample| sample.is_finite()));
    }
    assert!(AudioBuffer::demo("nope", 44_100.0).is_none());
}
