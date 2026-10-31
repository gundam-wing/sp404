use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sp404"))
}

fn click_wav(dir: &std::path::Path) -> std::path::PathBuf {
    let sr = 22_050.0f32;
    let duration = 4.0f32;
    let frames = (sr * duration) as usize;
    let mut data = vec![0.0f32; frames];
    let interval = sr * 0.5;
    let mut at = interval;
    while at + 8.0 < frames as f32 {
        let start = at.round() as usize;
        let decay = 0.0015 * sr;
        for i in 0..(0.012 * sr) as usize {
            if start + i >= data.len() {
                break;
            }
            let env = (-(i as f32) / decay).exp();
            let phase = std::f32::consts::FRAC_PI_2;
            data[start + i] +=
                env * (2.0 * std::f32::consts::PI * 1_000.0 * (i as f32) / sr + phase).sin();
        }
        at += interval;
    }
    let buffer = sp404_core::AudioBuffer::new(sr, 1, data).unwrap();
    let path = dir.join("clicks.wav");
    std::fs::write(&path, sp404_core::encode_wav(&buffer).unwrap()).unwrap();
    path
}

#[test]
fn analyze_prints_stable_json_for_a_click_track() {
    let dir = std::env::temp_dir().join(format!("pad404-analyze-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wav = click_wav(&dir);
    let run = |extra: &[&str]| {
        let mut command = bin();
        command.arg("analyze").arg(wav.to_str().unwrap());
        command.args(extra);
        command.output().unwrap()
    };
    let output = run(&["--min-bpm", "40", "--max-bpm", "280", "--hop-size", "512"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&text).expect("analyze prints json");
    for field in [
        "sample_rate",
        "duration",
        "bpm",
        "beat_times",
        "onset_times",
        "key",
        "key_confidence",
        "slices",
    ] {
        assert!(doc.get(field).is_some(), "missing {field} in {text}");
    }
    let bpm = doc["bpm"].as_f64().unwrap();
    assert!((bpm - 120.0).abs() < 12.0, "bpm {bpm} stdout {text}");
    assert!(doc["onset_times"].as_array().unwrap().len() >= 4);
    let slice = &doc["slices"][0];
    assert!(slice["start"].is_number());
    assert!(slice["end"].is_number());
    assert!(slice["fade_in"].is_number());
    assert!(slice["fade_out"].is_number());
    assert!(String::from_utf8_lossy(&output.stderr).contains("bpm"));

    let again = run(&["--min-bpm", "40", "--max-bpm", "280", "--hop-size", "512"]);
    assert_eq!(output.stdout, again.stdout);

    let missing = bin()
        .args(["analyze", dir.join("nope.wav").to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn analyze_usage_is_in_help() {
    let output = bin().arg("--help").output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("sp404 analyze"));
    assert!(text.contains("--hop-size"));
    assert!(text.contains("--min-bpm"));
    assert!(text.contains("--max-bpm"));
}
