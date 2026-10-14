//! Phrase search and seeded arrangement variations.

use sp404_core::{analyze, vary, AudioBuffer, VaryRequest};

fn tone(start: f64, end: f64, freq: f32, amp: f32, buffer: &mut [f32], sample_rate: f32) {
    let channels = 2;
    for frame in 0..buffer.len() / channels {
        let time = frame as f64 / f64::from(sample_rate);
        if time < start || time >= end {
            continue;
        }
        let phase = (time as f32 * freq).fract();
        let sample = (phase * std::f32::consts::TAU).sin() * amp;
        buffer[frame * 2] = sample;
        buffer[frame * 2 + 1] = sample * 0.8;
    }
}

/// Two bursts separated by silence. The first is louder and shorter.
fn two_phrases() -> AudioBuffer {
    let sample_rate = 16_000.0;
    let frames = (sample_rate * 8.0) as usize;
    let mut data = vec![0.0; frames * 2];
    tone(0.4, 2.1, 220.0, 0.7, &mut data, sample_rate);
    tone(3.2, 6.4, 330.0, 0.28, &mut data, sample_rate);
    AudioBuffer::new(sample_rate, 2, data).unwrap()
}

#[test]
fn analysis_finds_the_bursts_and_ranks_the_louder_attack() {
    let buffer = two_phrases();
    let analysis = analyze(&buffer).unwrap();
    assert!(
        analysis.phrases.len() >= 2,
        "phrases: {:?}",
        analysis.phrases
    );
    let first = analysis
        .phrases
        .iter()
        .find(|phrase| phrase.start < 1.5)
        .expect("early burst");
    let second = analysis
        .phrases
        .iter()
        .find(|phrase| phrase.start > 2.5)
        .expect("later burst");
    assert!(
        first.end < 2.6,
        "early phrase should end before the gap: {first:?}"
    );
    assert!(
        second.start > 2.6,
        "later phrase should start after the gap: {second:?}"
    );
    assert!(
        first.score >= second.score,
        "louder attack should score at least as high: {first:?} vs {second:?}"
    );
}

#[test]
fn around_prefers_the_phrase_nearest_that_time() {
    let buffer = two_phrases();
    let early = vary(VaryRequest {
        buffer: &buffer,
        seed: 2,
        around: Some(1.0),
        source_path: None,
    })
    .unwrap();
    let late = vary(VaryRequest {
        buffer: &buffer,
        seed: 2,
        around: Some(5.0),
        source_path: None,
    })
    .unwrap();
    assert!(early.hero.start < 1.5, "{:?}", early.hero);
    assert!(late.hero.start > 2.5, "{:?}", late.hero);
    assert_ne!(early.plan.to_json().unwrap(), late.plan.to_json().unwrap());
}

#[test]
fn the_same_seed_builds_the_same_plan_and_another_seed_can_differ() {
    let buffer = two_phrases();
    let once = vary(VaryRequest {
        buffer: &buffer,
        seed: 11,
        around: None,
        source_path: None,
    })
    .unwrap();
    let again = vary(VaryRequest {
        buffer: &buffer,
        seed: 11,
        around: None,
        source_path: None,
    })
    .unwrap();
    assert_eq!(once.plan.to_json().unwrap(), again.plan.to_json().unwrap());
    let other = vary(VaryRequest {
        buffer: &buffer,
        seed: 12,
        around: None,
        source_path: None,
    })
    .unwrap();
    let audio = once
        .plan
        .render(0, once.plan.duration_frames().unwrap())
        .unwrap();
    assert!(audio.peak() > 0.02, "peak {}", audio.peak());
    assert!(audio.data.iter().all(|sample| sample.is_finite()));
    let _ = other;
    assert!(
        once.plan.to_json().unwrap() != other.plan.to_json().unwrap(),
        "adjacent seeds should not be identical arrangements"
    );
}

#[test]
fn a_file_path_keeps_the_slices_as_cuts_instead_of_embedding_samples() {
    let buffer = two_phrases();
    let variation = vary(VaryRequest {
        buffer: &buffer,
        seed: 4,
        around: Some(1.0),
        source_path: Some("takes/source.wav"),
    })
    .unwrap();
    let json = variation.plan.to_json().unwrap();
    assert!(json.contains("takes/source.wav"));
    assert!(!json.contains("\"samples\""));
    assert!(variation.hero.end > variation.hero.start);
}

#[test]
fn a_long_dense_stretch_is_split_into_loopable_phrases() {
    let sample_rate = 16_000.0;
    let frames = (sample_rate * 20.0) as usize;
    let mut data = vec![0.0; frames * 2];
    tone(0.2, 19.5, 196.0, 0.4, &mut data, sample_rate);
    // A breath near the middle, quieter than the surrounding tone.
    tone(9.4, 10.2, 196.0, 0.18, &mut data, sample_rate);
    let buffer = AudioBuffer::new(sample_rate, 2, data).unwrap();
    let analysis = analyze(&buffer).unwrap();
    assert!(
        analysis.phrases.len() >= 2,
        "expected a split, got {:?}",
        analysis.phrases
    );
    for phrase in &analysis.phrases {
        let length = phrase.end - phrase.start;
        assert!(length <= 8.0, "phrase is too long to loop: {phrase:?}");
    }
}

#[test]
fn silence_is_rejected() {
    let data = vec![0.0; 16_000 * 2];
    let buffer = AudioBuffer::new(16_000.0, 2, data).unwrap();
    let error = analyze(&buffer).unwrap_err();
    assert!(error.to_string().contains("noise floor"));
}
