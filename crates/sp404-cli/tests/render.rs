use std::process::Command;

use sp404_core::decode_wav;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sp404"))
}

#[test]
fn catalog_mentions_filter_and_presets() {
    let output = bin().arg("catalog").output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("\"version\": 1"));
    assert!(text.contains("\"id\": \"filter\""));
    assert!(text.contains("\"id\": \"lofi\""));
    assert!(text.contains("dub-delay"));
}

#[test]
fn render_demo_through_a_chain_writes_a_wav() {
    let dir = std::env::temp_dir().join(format!("pad404-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let chain = dir.join("chain.json");
    let wav = dir.join("kick.wav");
    std::fs::write(
        &chain,
        r#"{"effects":[
            {"effect":"lofi","params":{"bit_depth":6,"downsample":4,"mix":1}},
            {"effect":"delay","params":{"time_ms":120,"feedback":0.3,"mix":0.4}}
        ]}"#,
    )
    .unwrap();
    let output = bin()
        .args([
            "render",
            "--demo",
            "kick",
            "--chain",
            chain.to_str().unwrap(),
            "--output",
            wav.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = std::fs::read(&wav).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    let decoded = decode_wav(&bytes).unwrap();
    assert!(decoded.peak() > 0.05, "peak {}", decoded.peak());
    assert!(decoded.data.iter().all(|sample| sample.is_finite()));
    assert!(decoded.frames() > 1_000);

    let dry_path = dir.join("dry.wav");
    let dry = bin()
        .args([
            "render",
            "--demo",
            "kick",
            "--output",
            dry_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(dry.status.success());
    let dry_audio = decode_wav(&std::fs::read(&dry_path).unwrap()).unwrap();
    assert_ne!(dry_audio.data, decoded.data);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_input_fails() {
    let output = bin()
        .args([
            "render",
            "--input",
            "no-such-file.wav",
            "--output",
            "out.wav",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
}
