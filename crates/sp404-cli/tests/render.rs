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
    assert!(text.contains("\"automatable\": true"));
    assert!(text.contains("\"kind\": \"keyframes\""));
    assert!(text.contains("\"kind\": \"spring\""));
    assert!(text.contains("\"kind\": \"file\""));
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
fn plan_validate_and_render_write_a_wav() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plans");
    let sweep = root.join("filter-sweep.json");
    let throw = root.join("dub-throw.json");
    let validated = bin()
        .args(["plan", "validate", sweep.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        validated.status.success(),
        "{}",
        String::from_utf8_lossy(&validated.stderr)
    );
    let summary: serde_json::Value =
        serde_json::from_slice(&validated.stdout).expect("validate prints json");
    assert_eq!(summary["ok"], true);
    assert_eq!(summary["name"], "filter-sweep");
    assert!(summary["frames"].as_u64().unwrap() > 10_000);

    let dir = std::env::temp_dir().join(format!("pad404-plan-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("sweep.wav");
    let rendered = bin()
        .args([
            "plan",
            "render",
            sweep.to_str().unwrap(),
            "--output",
            wav.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        rendered.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    let decoded = decode_wav(&std::fs::read(&wav).unwrap()).unwrap();
    assert_eq!(&std::fs::read(&wav).unwrap()[..4], b"RIFF");
    assert!(decoded.peak() > 0.02, "peak {}", decoded.peak());
    assert!(decoded.data.iter().all(|sample| sample.is_finite()));
    assert_eq!(decoded.frames() as u64, summary["frames"].as_u64().unwrap());

    let ranged = dir.join("throw.wav");
    let partial = bin()
        .args([
            "plan",
            "render",
            throw.to_str().unwrap(),
            "--output",
            ranged.to_str().unwrap(),
            "--start-beat",
            "2",
            "--end-beat",
            "4",
        ])
        .output()
        .unwrap();
    assert!(
        partial.status.success(),
        "{}",
        String::from_utf8_lossy(&partial.stderr)
    );
    let tail = decode_wav(&std::fs::read(&ranged).unwrap()).unwrap();
    assert!(tail.frames() > 1_000);
    assert!(tail.frames() < decoded.frames());
    assert!(tail.data.iter().all(|sample| sample.is_finite()));
    assert!(tail.peak() > 0.01, "peak {}", tail.peak());

    let bad = bin()
        .args(["plan", "validate", wav.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!bad.status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn plan_inlines_a_wav_beside_the_plan() {
    let dir = std::env::temp_dir().join(format!("pad404-file-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sample = sp404_core::AudioBuffer::demo("snare", 16_000.0).unwrap();
    std::fs::write(
        dir.join("take.wav"),
        sp404_core::encode_wav(&sample).unwrap(),
    )
    .unwrap();
    let plan = dir.join("plan.json");
    std::fs::write(
        &plan,
        r#"{
            "name": "file-take",
            "sample_rate": 16000,
            "tempo": 120,
            "duration": { "seconds": 0.4 },
            "pads": [{ "pad": 0, "sample": { "kind": "file", "path": "take.wav" } }],
            "triggers": [{ "pad": 0, "at": { "seconds": 0.0 } }]
        }"#,
    )
    .unwrap();

    let validated = bin()
        .args(["plan", "validate", plan.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        validated.status.success(),
        "{}",
        String::from_utf8_lossy(&validated.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&validated.stdout).unwrap();
    assert_eq!(summary["name"], "file-take");

    let wav = dir.join("out.wav");
    let rendered = bin()
        .args([
            "plan",
            "render",
            plan.to_str().unwrap(),
            "--output",
            wav.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        rendered.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    let decoded = decode_wav(&std::fs::read(&wav).unwrap()).unwrap();
    assert!(decoded.peak() > 0.05, "peak {}", decoded.peak());
    assert!(decoded.frames() > 1_000);

    let missing = dir.join("missing.json");
    std::fs::write(
        &missing,
        r#"{
            "sample_rate": 16000,
            "tempo": 120,
            "duration": { "seconds": 0.2 },
            "pads": [{ "pad": 0, "sample": { "kind": "file", "path": "nope.wav" } }],
            "triggers": [{ "pad": 0, "at": { "seconds": 0 } }]
        }"#,
    )
    .unwrap();
    let bad = bin()
        .args(["plan", "validate", missing.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!bad.status.success());
    let err = String::from_utf8_lossy(&bad.stderr);
    assert!(err.contains("nope.wav"), "{err}");
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
