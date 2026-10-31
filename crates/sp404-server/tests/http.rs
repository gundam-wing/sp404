use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sp404_server::{router, Config};
use tower::ServiceExt;

fn config(max_bars: u32, max_upload_bytes: usize, max_concurrent_jobs: usize) -> Config {
    Config {
        api_key: "test-key".to_string(),
        max_bars,
        max_upload_bytes,
        max_concurrent_jobs,
        bind: "127.0.0.1:0".to_string(),
    }
}

fn click_wav() -> Vec<u8> {
    let sr = 8_000.0f32;
    let frames = (sr * 4.0) as usize;
    let mut data = vec![0.0f32; frames];
    let mut at = 0.25 * sr;
    while (at as usize) + 64 < frames {
        let start = at as usize;
        for (offset, sample) in data[start..start + 48].iter_mut().enumerate() {
            *sample += 0.85 * (1.0 - offset as f32 / 48.0);
        }
        at += 0.5 * sr;
    }
    let buffer = sp404_core::AudioBuffer::new(sr, 1, data).unwrap();
    sp404_core::encode_wav(&buffer).unwrap()
}

fn multipart(fields: &[(&str, &str)], file: &[u8]) -> (String, Vec<u8>) {
    let boundary = "----sp404test";
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .into_bytes(),
        );
    }
    body.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"audio\"; filename=\"loop.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .into_bytes(),
    );
    body.extend(file);
    body.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn job_form(bars: &str) -> (String, Vec<u8>) {
    multipart(
        &[
            ("style", "house"),
            ("seed", "1"),
            ("bars", bars),
            ("mode", "transient"),
        ],
        &click_wav(),
    )
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    key: Option<&str>,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> axum::http::Response<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(key) = key {
        builder = builder.header("authorization", format!("Bearer {key}"));
    }
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    app.clone()
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap()
}

async fn bytes(response: axum::http::Response<Body>) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn health_is_open_and_other_routes_require_the_key() {
    let app = router(config(64, 1024 * 1024, 2));
    let health = send(&app, "GET", "/health", None, None, Vec::new()).await;
    assert_eq!(health.status(), StatusCode::OK);
    let health_body = bytes(health).await;
    assert!(std::str::from_utf8(&health_body).unwrap().contains("ok"));

    let (content_type, body) = job_form("2");
    let missing = send(&app, "POST", "/jobs", None, Some(&content_type), body).await;
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

    let (content_type, body) = job_form("2");
    let wrong = send(
        &app,
        "POST",
        "/jobs",
        Some("nope"),
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    let status = send(&app, "GET", "/jobs/job-1", None, None, Vec::new()).await;
    assert_eq!(status.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn oversized_bars_are_413_and_a_full_queue_is_429() {
    let app = router(config(1, 1024 * 1024, 2));
    let (content_type, body) = job_form("2");
    let response = send(
        &app,
        "POST",
        "/jobs",
        Some("test-key"),
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let paused = router(config(64, 1024 * 1024, 0));
    let (content_type, body) = job_form("2");
    let response = send(
        &paused,
        "POST",
        "/jobs",
        Some("test-key"),
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn a_job_returns_the_same_plan_twice_and_a_wav() {
    let app = router(config(64, 1024 * 1024, 2));
    let (content_type, body) = job_form("2");
    let created = send(
        &app,
        "POST",
        "/jobs",
        Some("test-key"),
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(created.status(), StatusCode::ACCEPTED);
    let created_json: serde_json::Value = serde_json::from_slice(&bytes(created).await).unwrap();
    let id = created_json["id"].as_str().unwrap().to_string();
    assert_eq!(created_json["status"], "queued");
    assert_eq!(created_json["style"], "house");
    assert_eq!(created_json["seed"], 1);
    assert_eq!(created_json["bars"], 2);
    assert_eq!(created_json["mode"], "transient");

    let unknown = send(
        &app,
        "GET",
        "/jobs/missing",
        Some("test-key"),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    let mut status = String::new();
    let mut detail = String::new();
    for _ in 0..400 {
        let response = send(
            &app,
            "GET",
            &format!("/jobs/{id}"),
            Some("test-key"),
            None,
            Vec::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&bytes(response).await).unwrap();
        status = json["status"].as_str().unwrap().to_string();
        detail = json.to_string();
        if status == "succeeded" || status == "failed" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(status, "succeeded", "{detail}");

    let plan = send(
        &app,
        "GET",
        &format!("/jobs/{id}/plan"),
        Some("test-key"),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(plan.status(), StatusCode::OK);
    assert_eq!(
        plan.headers().get("content-type").unwrap(),
        "application/json"
    );
    let plan_bytes = bytes(plan).await;
    let arrangement =
        sp404_core::plan::Arrangement::from_json(std::str::from_utf8(&plan_bytes).unwrap())
            .expect("plan json");
    assert!(!arrangement.triggers.is_empty());

    let audio = send(
        &app,
        "GET",
        &format!("/jobs/{id}/audio"),
        Some("test-key"),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(audio.status(), StatusCode::OK);
    assert_eq!(audio.headers().get("content-type").unwrap(), "audio/wav");
    let wav = bytes(audio).await;
    assert_eq!(&wav[..4], b"RIFF");
    let decoded = sp404_core::decode_wav(&wav).unwrap();
    assert!(decoded.peak() > 0.01, "peak {}", decoded.peak());
    assert!(decoded.data.iter().all(|sample| sample.is_finite()));

    let (content_type, body) = job_form("2");
    let again = send(
        &app,
        "POST",
        "/jobs",
        Some("test-key"),
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(again.status(), StatusCode::ACCEPTED);
    let again_json: serde_json::Value = serde_json::from_slice(&bytes(again).await).unwrap();
    let again_id = again_json["id"].as_str().unwrap();
    let mut again_status = String::new();
    for _ in 0..400 {
        let response = send(
            &app,
            "GET",
            &format!("/jobs/{again_id}"),
            Some("test-key"),
            None,
            Vec::new(),
        )
        .await;
        let json: serde_json::Value = serde_json::from_slice(&bytes(response).await).unwrap();
        again_status = json["status"].as_str().unwrap().to_string();
        if again_status == "succeeded" || again_status == "failed" {
            detail = json.to_string();
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(again_status, "succeeded", "{detail}");
    let again_plan = send(
        &app,
        "GET",
        &format!("/jobs/{again_id}/plan"),
        Some("test-key"),
        None,
        Vec::new(),
    )
    .await;
    let again_bytes = bytes(again_plan).await;
    assert_eq!(plan_bytes, again_bytes);
}

#[tokio::test]
async fn plan_and_audio_are_409_until_the_job_succeeds() {
    let app = router(config(64, 1024 * 1024, 2));
    let (content_type, body) = multipart(
        &[
            ("style", "techno"),
            ("seed", "3"),
            ("bars", "2"),
            ("mode", "transient"),
        ],
        b"not a wav",
    );
    let created = send(
        &app,
        "POST",
        "/jobs",
        Some("test-key"),
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(created.status(), StatusCode::ACCEPTED);
    let created_json: serde_json::Value = serde_json::from_slice(&bytes(created).await).unwrap();
    let id = created_json["id"].as_str().unwrap();

    let mut status = String::new();
    let mut detail = String::new();
    for _ in 0..100 {
        let response = send(
            &app,
            "GET",
            &format!("/jobs/{id}"),
            Some("test-key"),
            None,
            Vec::new(),
        )
        .await;
        let json: serde_json::Value = serde_json::from_slice(&bytes(response).await).unwrap();
        status = json["status"].as_str().unwrap().to_string();
        detail = json.to_string();
        if status == "failed" || status == "succeeded" {
            break;
        }
        let plan = send(
            &app,
            "GET",
            &format!("/jobs/{id}/plan"),
            Some("test-key"),
            None,
            Vec::new(),
        )
        .await;
        assert_eq!(plan.status(), StatusCode::CONFLICT);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(status, "failed", "{detail}");
    let plan = send(
        &app,
        "GET",
        &format!("/jobs/{id}/plan"),
        Some("test-key"),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(plan.status(), StatusCode::CONFLICT);
    let audio = send(
        &app,
        "GET",
        &format!("/jobs/{id}/audio"),
        Some("test-key"),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(audio.status(), StatusCode::CONFLICT);
}
