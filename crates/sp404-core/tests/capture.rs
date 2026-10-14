//! Recording ingestion: channel map, resample, and silence trim. No devices.

use sp404_core::plan::{seconds, Arrangement, PlanBuilder};
use sp404_core::{ingest_take, AudioBuffer, Take, TakeOptions};

fn opts(rate: f32, trim: bool) -> TakeOptions {
    TakeOptions {
        target_rate: rate,
        trim_silence: trim,
        trim_threshold: 0.01,
        pre_roll: 50.0 / 16_000.0,
        post_roll: 50.0 / 16_000.0,
    }
}

#[test]
fn mono_is_duplicated_and_resampled_to_the_engine_rate() {
    let frames = 480;
    let mut mono = Vec::with_capacity(frames);
    for i in 0..frames {
        let t = i as f32 / 48_000.0;
        mono.push((t * 440.0 * std::f32::consts::TAU).sin() * 0.5);
    }
    let out = ingest_take(48_000.0, 1, &mono, opts(44_100.0, false)).unwrap();
    assert_eq!(out.channels, 2);
    assert_eq!(out.sample_rate, 44_100.0);
    assert_eq!(out.frames(), 441);
    for frame in 0..out.frames() {
        assert_eq!(
            out.data[frame * 2].to_bits(),
            out.data[frame * 2 + 1].to_bits()
        );
    }
    assert!(out.peak() > 0.2);
    assert!(out.data.iter().all(|sample| sample.is_finite()));
}

#[test]
fn stereo_pair_is_kept_and_extra_channels_are_dropped() {
    let mut wide = Vec::new();
    for _ in 0..8 {
        wide.extend_from_slice(&[0.25, -0.5, 0.9, -0.9]);
    }
    let out = ingest_take(16_000.0, 4, &wide, opts(16_000.0, false)).unwrap();
    assert_eq!(out.frames(), 8);
    for frame in 0..8 {
        assert!((out.data[frame * 2] - 0.25).abs() < 1.0e-6);
        assert!((out.data[frame * 2 + 1] + 0.5).abs() < 1.0e-6);
    }
}

#[test]
fn same_rate_stereo_is_unchanged() {
    let samples = vec![0.1, -0.2, 0.3, -0.4];
    let out = ingest_take(44_100.0, 2, &samples, opts(44_100.0, false)).unwrap();
    assert_eq!(out.data, samples);
}

#[test]
fn non_finite_samples_become_silence() {
    let samples = vec![f32::NAN, 0.25, f32::INFINITY, -0.5];
    let out = ingest_take(16_000.0, 2, &samples, opts(16_000.0, false)).unwrap();
    assert_eq!(out.data, vec![0.0, 0.25, 0.0, -0.5]);
}

#[test]
fn trim_keeps_a_pad_around_the_loud_region() {
    let mut mono = vec![0.0; 1_000];
    mono.extend(std::iter::repeat_n(0.5, 200));
    mono.extend(std::iter::repeat_n(0.0, 800));
    let out = ingest_take(16_000.0, 1, &mono, opts(16_000.0, true)).unwrap();
    assert_eq!(out.frames(), 300);
    assert!(out.data[0].abs() < 1.0e-6);
    assert!((out.data[50 * 2] - 0.5).abs() < 1.0e-5);
    assert!((out.data[249 * 2] - 0.5).abs() < 1.0e-5);
    assert!(out.data[250 * 2].abs() < 1.0e-6);
}

#[test]
fn a_silent_take_is_kept_when_trim_finds_nothing() {
    let mono = vec![0.0; 400];
    let out = ingest_take(16_000.0, 1, &mono, opts(16_000.0, true)).unwrap();
    assert_eq!(out.frames(), 400);
    assert_eq!(out.peak(), 0.0);
}

#[test]
fn bad_layouts_are_rejected() {
    let options = opts(16_000.0, false);
    assert!(ingest_take(100.0, 1, &[0.0, 0.0], options).is_err());
    assert!(ingest_take(16_000.0, 0, &[0.0], options).is_err());
    assert!(ingest_take(16_000.0, 2, &[0.1, 0.2, 0.3], options).is_err());
    assert!(ingest_take(16_000.0, 40, &[0.0; 40], options).is_err());
    let mut bad = opts(16_000.0, true);
    bad.trim_threshold = f32::NAN;
    assert!(ingest_take(16_000.0, 1, &[0.2, 0.2], bad).is_err());
    assert!(Take::new(16_000.0, 1, vec![]).is_err());
}

#[test]
fn ingested_take_loads_onto_a_pad_and_round_trips_as_wav() {
    let take = Take::new(
        22_050.0,
        1,
        vec![0.0; 100].into_iter().chain(vec![0.4; 200]).collect(),
    )
    .unwrap();
    let sample = take.ingest(opts(44_100.0, true)).unwrap();
    assert_eq!(sample.channels, 2);
    assert!(sample.frames() > 100);
    let mut engine = sp404_core::Engine::new(44_100.0);
    engine.load_pad(3, sample.clone(), "take").unwrap();
    engine.set_pad_range(3, 0.0, 1.0).unwrap();
    assert!(engine.trigger(3));
    let mut output = vec![0.0; sample.frames() * 2];
    engine.process(&mut output);
    assert!(output.iter().any(|value| value.abs() > 0.05));

    let bytes = sp404_core::encode_wav(&sample).unwrap();
    let decoded = sp404_core::decode_wav(&bytes).unwrap();
    assert_eq!(decoded.frames(), sample.frames());
    assert!(decoded.peak() > 0.2);
}

#[test]
fn a_file_sample_validates_without_opening_it_and_will_not_render() {
    let plan = PlanBuilder::new(16_000, 120.0)
        .duration(seconds(0.1))
        .pad_file(0, "takes/pad01.wav")
        .trigger(0, seconds(0.0))
        .finish()
        .unwrap();
    assert!(matches!(
        plan.pads[0].sample,
        sp404_core::SampleSource::File { .. }
    ));
    let err = plan.render(0, 16).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("takes/pad01.wav"), "{message}");
    assert!(message.contains("inline"), "{message}");

    let json = r#"{"sample_rate":16000,"tempo":120,"duration":{"seconds":0.1},"pads":[{"pad":0,"sample":{"kind":"file","path":""}}],"triggers":[{"pad":0,"at":{"seconds":0}}]}"#;
    assert!(Arrangement::from_json(json).is_err());
}

#[test]
fn demo_buffer_still_ingests_as_a_resample_of_the_master() {
    let master = AudioBuffer::demo("kick", 44_100.0).unwrap();
    let again = ingest_take(
        master.sample_rate,
        master.channels,
        &master.data,
        TakeOptions {
            target_rate: 48_000.0,
            trim_silence: false,
            ..TakeOptions::default()
        },
    )
    .unwrap();
    assert_eq!(again.sample_rate, 48_000.0);
    assert_eq!(again.channels, 2);
    let expected = ((master.frames() as f64) * 48_000.0 / 44_100.0).round() as usize;
    assert_eq!(again.frames(), expected.max(1));
    assert!(again.peak() > 0.2);
}
