//! Arrangement plans: automation math, validation, and order-independent render.

use std::collections::BTreeMap;

use sp404_core::plan::{
    beats, chain, effect, seconds, Arrangement, Ease, Keyframe, LfoShape, PlanBuilder,
    PlanRenderer, Target, Track, CHECKPOINT_FRAMES,
};
use sp404_core::{create_effect, effect_schemas, EffectDesc, Error};

fn bits_eq(left: &[f32], right: &[f32]) {
    assert_eq!(left.len(), right.len(), "length");
    for (index, (a, b)) in left.iter().zip(right).enumerate() {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "sample {index}: {a} ({:#x}) vs {b} ({:#x})",
            a.to_bits(),
            b.to_bits()
        );
    }
}

fn stateful_plan() -> Arrangement {
    PlanBuilder::new(22_050, 120.0)
        .name("determinism")
        .duration(seconds(0.55))
        .pad_demo(0, "kick")
        .pad_demo(1, "snare")
        .pad_chain(
            0,
            chain([effect(
                "lofi",
                [("bit_depth", 6.0), ("downsample", 3.0), ("mix", 1.0)],
            )]),
        )
        .pad_chain(
            1,
            chain([
                effect("pitch", [("semitones", 3.0), ("mix", 1.0)]),
                effect(
                    "delay",
                    [
                        ("time_ms", 90.0),
                        ("feedback", 0.55),
                        ("mix", 0.45),
                        ("ping_pong", 0.6),
                    ],
                ),
            ]),
        )
        .trigger(0, seconds(0.0))
        .trigger(0, seconds(0.22))
        .trigger_gain(1, seconds(0.11), 0.8)
        .trigger(1, seconds(0.33))
        .bus_chain(chain([
            effect(
                "filter",
                [("cutoff", 400.0), ("resonance", 0.5), ("mix", 1.0)],
            ),
            effect(
                "compressor",
                [("threshold_db", -18.0), ("ratio", 6.0), ("mix", 1.0)],
            ),
            effect("reverb", [("room", 0.7), ("damp", 0.3), ("mix", 0.35)]),
        ]))
        .automate(
            Target::bus(0, "cutoff"),
            Track::keyframes([
                Keyframe::at(seconds(0.0), 240.0),
                Keyframe::at(seconds(0.55), 6_000.0).ease(Ease::Smooth),
            ]),
        )
        .automate(
            Target::bus(0, "resonance"),
            Track::lfo_hz(LfoShape::Sine, 2.0, 0.25, 0.45),
        )
        .automate(
            Target::pad(1, 1, "mix"),
            Track::spring(0.05, 0.7, seconds(0.3), seconds(0.15)).bounce(0.35),
        )
        .master(Track::constant(0.8))
        .finish()
        .unwrap()
}

fn render_fresh_chunks(plan: &Arrangement, chunk: u64) -> Vec<f32> {
    let end = plan.duration_frames().unwrap();
    let mut out = Vec::new();
    let mut frame = 0;
    while frame < end {
        let next = (frame + chunk).min(end);
        let piece = plan.render(frame, next).unwrap();
        out.extend_from_slice(&piece.data);
        frame = next;
    }
    out
}

fn render_cursor_chunks(plan: &Arrangement, chunk: u64) -> Vec<f32> {
    let end = plan.duration_frames().unwrap();
    let mut renderer = PlanRenderer::new(plan).unwrap();
    let mut out = Vec::new();
    let mut frame = 0;
    while frame < end {
        let next = (frame + chunk).min(end);
        let piece = renderer.render(frame, next).unwrap();
        out.extend_from_slice(&piece.data);
        frame = next;
    }
    out
}

#[test]
fn effect_snapshots_round_trip_every_builtin() {
    let mut noise = Vec::new();
    let mut state = 0x5044_0404_u32;
    for _ in 0..256 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let sample = ((state >> 8) as f32 / 16_777_216.0) * 2.0 - 1.0;
        noise.push(sample);
        noise.push(sample * 0.5);
    }
    for schema in effect_schemas() {
        let desc = EffectDesc {
            effect: schema.id.to_string(),
            bypass: false,
            params: BTreeMap::new(),
        };
        let mut original = create_effect(&desc).unwrap();
        let mut restored = create_effect(&desc).unwrap();
        for spec in schema.params {
            let value = if spec.id == "semitones" {
                5.0
            } else {
                (spec.min + spec.max) * 0.5
            };
            original.set_param(spec.id, value).unwrap();
            restored.set_param(spec.id, value).unwrap();
        }
        let mut warm = noise.clone();
        original.process(&mut warm, 22_050.0);
        let snapshot = original.capture_state();
        restored.restore_state(&snapshot).unwrap();
        assert_eq!(restored.capture_state(), snapshot, "{}", schema.id);

        let mut keep = noise[128..].to_vec();
        let mut again = keep.clone();
        original.process(&mut keep, 22_050.0);
        restored.process(&mut again, 22_050.0);
        bits_eq(&keep, &again);
    }

    let mut filter = create_effect(&EffectDesc {
        effect: "filter".into(),
        bypass: false,
        params: BTreeMap::new(),
    })
    .unwrap();
    let err = filter
        .restore_state(&sp404_core::EffectState {
            floats: vec![1.0],
            ints: vec![],
        })
        .unwrap_err();
    assert!(matches!(err, Error::StateMismatch { .. }));
}

#[test]
fn keyframes_lfo_and_spring_are_functions_of_time() {
    let curve = Track::keyframes([
        Keyframe::at(seconds(0.0), 0.0),
        Keyframe::at(seconds(1.0), 10.0).ease(Ease::Linear),
        Keyframe::at(seconds(2.0), 10.0).ease(Ease::Hold),
        Keyframe::at(seconds(3.0), 0.0).ease(Ease::Smooth),
    ]);
    assert_eq!(
        curve.sample(-1.0, 120.0).unwrap().to_bits(),
        0.0_f32.to_bits()
    );
    assert_eq!(curve.sample(0.5, 120.0).unwrap(), 5.0);
    assert_eq!(curve.sample(1.9, 120.0).unwrap(), 10.0);
    assert_eq!(curve.sample(2.0, 120.0).unwrap(), 10.0);
    let quarter = curve.sample(2.25, 120.0).unwrap();
    assert!(
        quarter > 8.0,
        "smooth quarter should stay near the held value, got {quarter}"
    );
    assert_eq!(curve.sample(3.0, 120.0).unwrap(), 0.0);
    assert_eq!(curve.sample(9.0, 120.0).unwrap(), 0.0);

    let lfo = Track::lfo_hz(LfoShape::Sine, 1.0, 2.0, 10.0);
    assert_eq!(lfo.sample(0.0, 120.0).unwrap(), 10.0);
    assert!(lfo.sample(0.25, 120.0).unwrap() > 11.9);
    assert!(lfo.sample(0.75, 120.0).unwrap() < 8.1);
    let square = Track::lfo_beats(LfoShape::Square, 1.0, 3.0, 0.0);
    assert_eq!(square.sample(0.0, 120.0).unwrap(), 3.0);
    let beat = 60.0 / 120.0;
    assert_eq!(square.sample(beat * 0.5, 120.0).unwrap(), -3.0);
    let held = Track::lfo_hz(LfoShape::Hold, 1.0, 1.0, 0.0);
    assert_eq!(
        held.sample(0.1, 120.0).unwrap().to_bits(),
        held.sample(0.9, 120.0).unwrap().to_bits()
    );
    assert_ne!(
        held.sample(0.1, 120.0).unwrap().to_bits(),
        held.sample(1.1, 120.0).unwrap().to_bits()
    );

    let spring = Track::spring(100.0, 2_000.0, beats(1.0), beats(1.0));
    assert_eq!(spring.sample(0.0, 120.0).unwrap(), 100.0);
    let release = 60.0 / 120.0;
    assert_eq!(spring.sample(release, 120.0).unwrap(), 100.0);
    let settled = spring.sample(release * 2.0, 120.0).unwrap();
    assert!((settled - 2_000.0).abs() < 20.0, "settled at {settled}");
    assert_eq!(
        spring.sample(release * 1.25, 120.0).unwrap().to_bits(),
        spring.sample(release * 1.25, 120.0).unwrap().to_bits()
    );
}

#[test]
fn arrangement_json_round_trips_and_rejects_bad_plans() {
    let plan = stateful_plan();
    let again = Arrangement::from_json(&plan.to_json().unwrap()).unwrap();
    assert_eq!(plan, again);

    let text = plan
        .to_json()
        .unwrap()
        .replace("\"version\": 1", "\"version\": 9");
    assert!(Arrangement::from_json(&text).is_err());

    let mut duplicate = plan.clone();
    duplicate.pads.push(duplicate.pads[0].clone());
    assert!(duplicate.validate().is_err());

    let mut late = plan.clone();
    late.triggers[0].at = seconds(5.0);
    assert!(late.validate().is_err());

    let unsorted = PlanBuilder::new(16_000, 100.0)
        .duration(seconds(1.0))
        .pad_demo(0, "kick")
        .trigger(0, seconds(0.0))
        .automate(
            Target::volume(0),
            Track::keyframes([
                Keyframe::at(seconds(0.5), 1.0),
                Keyframe::at(seconds(0.1), 0.2),
            ]),
        )
        .finish();
    assert!(unsorted.is_err());

    let both = r#"{"duration":{"seconds":1,"beats":1},"pads":[{"pad":0,"sample":{"kind":"demo","name":"kick"}}],"triggers":[{"pad":0,"at":{"seconds":0}}]}"#;
    assert!(Arrangement::from_json(both).is_err());

    let missing = PlanBuilder::new(16_000, 100.0)
        .duration(beats(1.0))
        .bus_chain(chain([effect("filter", [])]))
        .automate(Target::bus(0, "nope"), Track::constant(1.0))
        .finish();
    assert!(matches!(missing.unwrap_err(), Error::UnknownParam { .. }));
}

#[test]
fn slices_match_a_full_render_in_any_order() {
    let plan = stateful_plan();
    let frames = plan.duration_frames().unwrap();
    assert!(
        frames > CHECKPOINT_FRAMES * 2,
        "plan should cross two checkpoints, got {frames}"
    );

    let full = plan.render(0, frames).unwrap();
    assert!(full.peak() > 0.02, "peak {}", full.peak());
    assert!(full.data.iter().all(|sample| sample.is_finite()));
    let dry = PlanBuilder::new(22_050, 120.0)
        .duration(seconds(0.55))
        .pad_demo(0, "kick")
        .pad_demo(1, "snare")
        .trigger(0, seconds(0.0))
        .trigger(0, seconds(0.22))
        .trigger_gain(1, seconds(0.11), 0.8)
        .trigger(1, seconds(0.33))
        .finish()
        .unwrap()
        .render(0, frames)
        .unwrap();
    assert_ne!(full.data, dry.data);

    for chunk in [64_u64, 1000, 4096, 4097] {
        bits_eq(&render_cursor_chunks(&plan, chunk), &full.data);
    }
    for chunk in [1000_u64, 4096, 4097] {
        bits_eq(&render_fresh_chunks(&plan, chunk), &full.data);
    }
    let prefix = 128.min(frames);
    for chunk in [1_u64, 7] {
        bits_eq(
            &render_fresh_chunks_until(&plan, chunk, prefix),
            &full.data[..prefix as usize * 2],
        );
    }

    let interesting = [
        (0, 1),
        (0, CHECKPOINT_FRAMES),
        (CHECKPOINT_FRAMES, CHECKPOINT_FRAMES * 2),
        (CHECKPOINT_FRAMES * 2, frames),
        (CHECKPOINT_FRAMES - 1, CHECKPOINT_FRAMES + 1),
        (100, 8_000),
        (8_000, 9_000),
        (frames - 1, frames),
    ];
    for (start, end) in interesting {
        let piece = plan.render(start, end).unwrap();
        let from = start as usize * 2;
        let to = end as usize * 2;
        bits_eq(&piece.data, &full.data[from..to]);
    }

    let mut rng = 0x1234_5678_9ABC_DEF0_u64;
    let mut slices = Vec::new();
    for _ in 0..12 {
        let start = xor_shift(&mut rng) % frames;
        let room = frames - start;
        let len = (xor_shift(&mut rng) % room.max(1)) + 1;
        slices.push((start, start + len));
    }
    for (start, end) in &slices {
        let piece = plan.render(*start, *end).unwrap();
        bits_eq(
            &piece.data,
            &full.data[*start as usize * 2..*end as usize * 2],
        );
    }

    let mut shared = PlanRenderer::new(&plan).unwrap();
    let mut order: Vec<usize> = (0..slices.len()).collect();
    for index in 0..order.len() {
        let swap = (xor_shift(&mut rng) as usize) % order.len();
        order.swap(index, swap);
    }
    for index in order {
        let (start, end) = slices[index];
        let piece = shared.render(start, end).unwrap();
        bits_eq(
            &piece.data,
            &full.data[start as usize * 2..end as usize * 2],
        );
    }
}

fn render_fresh_chunks_until(plan: &Arrangement, chunk: u64, end: u64) -> Vec<f32> {
    let mut out = Vec::new();
    let mut frame = 0;
    while frame < end {
        let next = (frame + chunk).min(end);
        let piece = plan.render(frame, next).unwrap();
        out.extend_from_slice(&piece.data);
        frame = next;
    }
    out
}

fn xor_shift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn example_plans_load_and_the_sweep_moves() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plans");
    let sweep =
        Arrangement::from_json(&std::fs::read_to_string(root.join("filter-sweep.json")).unwrap())
            .unwrap();
    let throw =
        Arrangement::from_json(&std::fs::read_to_string(root.join("dub-throw.json")).unwrap())
            .unwrap();
    assert_eq!(sweep.name, "filter-sweep");
    assert_eq!(throw.triggers.len(), 14);
    let sweep_audio = sweep.render(0, 8_000).unwrap();
    assert!(sweep_audio.peak() > 0.01);
    assert!(sweep_audio.data.iter().all(|sample| sample.is_finite()));
    let cutoff = sweep.bus.automation[0]
        .track
        .sample(0.0, sweep.tempo)
        .unwrap();
    let later = sweep.bus.automation[0]
        .track
        .sample(sweep.duration.to_seconds(sweep.tempo).unwrap(), sweep.tempo)
        .unwrap();
    assert!(later > cutoff * 4.0, "{cutoff} -> {later}");

    let built = PlanBuilder::new(44_100, 100.0)
        .name("filter-sweep")
        .duration(beats(4.0))
        .master(Track::constant(0.9))
        .pad_demo(0, "kick")
        .pad_demo(1, "snare")
        .pad_demo(2, "hat")
        .pad_demo(8, "chord")
        .bus_chain(chain([effect(
            "filter",
            [
                ("mode", 0.0),
                ("cutoff", 280.0),
                ("resonance", 0.62),
                ("mix", 1.0),
            ],
        )]))
        .automate(
            Target::bus(0, "cutoff"),
            Track::keyframes([
                Keyframe::at(beats(0.0), 280.0),
                Keyframe::at(beats(4.0), 7_200.0).ease(Ease::Smooth),
            ]),
        )
        .trigger(0, beats(0.0))
        .trigger(0, beats(1.0))
        .trigger(0, beats(2.0))
        .trigger(0, beats(3.0))
        .trigger(1, beats(1.0))
        .trigger(1, beats(3.0))
        .trigger(2, beats(0.0))
        .trigger(2, beats(0.5))
        .trigger(2, beats(1.0))
        .trigger(2, beats(1.5))
        .trigger(2, beats(2.0))
        .trigger(2, beats(2.5))
        .trigger(2, beats(3.0))
        .trigger(2, beats(3.5))
        .trigger_gain(8, beats(0.0), 0.7)
        .finish()
        .unwrap();
    assert_eq!(built, sweep);

    let tail = throw.render(0, 4_000).unwrap();
    assert!(tail.data.iter().all(|sample| sample.is_finite()));
    assert!(tail.peak() > 0.01);
}

#[test]
fn catalog_lists_automatable_parameters_and_tracks() {
    let catalog: serde_json::Value = serde_json::from_str(&sp404_core::catalog_json()).unwrap();
    assert_eq!(catalog["version"], 1);
    assert_eq!(catalog["effects"][1]["params"][1]["automatable"], true);
    assert!(catalog["arrangement"]["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|track| track["kind"] == "spring"));
    assert!(catalog["arrangement"]["automatable"]
        .as_str()
        .unwrap()
        .contains("effect parameter"));
}
