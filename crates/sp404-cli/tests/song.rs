use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sp404"))
}

fn loop_wav(dir: &std::path::Path) -> std::path::PathBuf {
    let sr = 8_000.0f32;
    let frames = (sr * 4.0) as usize;
    let mut data = vec![0.0f32; frames];
    let mut at = 0.25f32 * sr;
    while (at as usize) + 64 < frames {
        let start = at as usize;
        for i in 0..48 {
            data[start + i] += 0.85 * (1.0 - i as f32 / 48.0);
        }
        at += 0.5 * sr;
    }
    let buffer = sp404_core::AudioBuffer::new(sr, 1, data).unwrap();
    let path = dir.join("loop.wav");
    std::fs::write(&path, sp404_core::encode_wav(&buffer).unwrap()).unwrap();
    path
}

#[test]
fn chop_and_generate_render_through_the_plan_command() {
    let dir = std::env::temp_dir().join(format!("pad404-song-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wav = loop_wav(&dir);
    let chops = dir.join("chops");
    let chopped = bin()
        .args([
            "chop",
            wav.to_str().unwrap(),
            "--output",
            chops.to_str().unwrap(),
            "--mode",
            "transient",
            "--min-bpm",
            "40",
            "--max-bpm",
            "280",
            "--hop-size",
            "256",
            "--fft-size",
            "1024",
        ])
        .output()
        .unwrap();
    assert!(
        chopped.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&chopped.stderr)
    );
    let manifest_path = chops.join("manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["mode"], "transient");
    let slices = manifest["slices"].as_array().unwrap();
    assert!(!slices.is_empty());
    for slice in slices {
        for field in [
            "index", "label", "start", "end", "fade_in", "fade_out", "path",
        ] {
            assert!(slice.get(field).is_some(), "missing {field}");
        }
        let label = slice["label"].as_str().unwrap();
        assert!(matches!(label, "kick" | "snare" | "hat" | "melodic"));
        let audio = chops.join(slice["path"].as_str().unwrap());
        assert!(audio.is_file(), "missing {}", audio.display());
    }

    let plan_path = dir.join("song.json");
    let generated = bin()
        .args([
            "generate",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--style",
            "house",
            "--seed",
            "7",
            "--bars",
            "2",
            "--output",
            plan_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let again = bin()
        .args([
            "generate",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--style",
            "house",
            "--seed",
            "7",
            "--bars",
            "2",
            "--output",
            dir.join("song-again.json").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(again.status.success());
    assert_eq!(
        std::fs::read(&plan_path).unwrap(),
        std::fs::read(dir.join("song-again.json")).unwrap()
    );

    let wav_out = dir.join("song.wav");
    let rendered = bin()
        .args([
            "plan",
            "render",
            plan_path.to_str().unwrap(),
            "--output",
            wav_out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        rendered.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    let decoded = sp404_core::decode_wav(&std::fs::read(&wav_out).unwrap()).unwrap();
    assert!(decoded.peak() > 0.01, "peak {}", decoded.peak());
    assert!(decoded.data.iter().all(|sample| sample.is_finite()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn song_commands_are_in_help() {
    let output = bin().arg("--help").output().unwrap();
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("sp404 chop"));
    assert!(text.contains("sp404 generate"));
    assert!(text.contains("boom-bap"));
    assert!(text.contains("kick"));
    assert!(text.contains("--phrase-bars"));
}
