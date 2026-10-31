//! HTTP routes. Jobs live in memory; a restart drops them.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::async_trait;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, FromRequestParts, Multipart, Path, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::json;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::task::spawn_blocking;

use crate::config::Config;
use crate::error::AppError;
use crate::pipeline::{render_upload, JobRequest};
use sp404_song::{ChopMode, Style};

struct AppState {
    config: Config,
    jobs: Mutex<HashMap<String, Job>>,
    slots: Arc<Semaphore>,
    next_id: AtomicU64,
}

struct Job {
    id: String,
    status: JobStatus,
    style: String,
    seed: u64,
    bars: u32,
    mode: String,
    phrase_bars: Option<u32>,
    error: Option<String>,
    plan_json: Option<String>,
    wav: Option<Vec<u8>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

impl JobStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}

#[derive(Serialize)]
struct JobBody {
    id: String,
    status: &'static str,
    style: String,
    seed: u64,
    bars: u32,
    mode: String,
    phrase_bars: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl Job {
    fn view(&self) -> JobBody {
        JobBody {
            id: self.id.clone(),
            status: self.status.as_str(),
            style: self.style.clone(),
            seed: self.seed,
            bars: self.bars,
            mode: self.mode.clone(),
            phrase_bars: self.phrase_bars,
            error: self.error.clone(),
        }
    }
}

/// Router for one configuration. Jobs are not shared across routers.
pub fn router(config: Config) -> Router {
    let slots = Arc::new(Semaphore::new(config.max_concurrent_jobs));
    let state = Arc::new(AppState {
        config,
        jobs: Mutex::new(HashMap::new()),
        slots,
        next_id: AtomicU64::new(1),
    });
    let limit = state.config.max_upload_bytes;
    Router::new()
        .route("/health", get(health))
        .route("/jobs", post(create_job))
        .route("/jobs/:id", get(job_status))
        .route("/jobs/:id/plan", get(job_plan))
        .route("/jobs/:id/audio", get(job_audio))
        .layer(DefaultBodyLimit::max(limit))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
}

struct Authed;

#[async_trait]
impl FromRequestParts<Arc<AppState>> for Authed {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        if authorized(&parts.headers, &state.config.api_key) {
            Ok(Self)
        } else {
            Err(AppError::unauthorized())
        }
    }
}

fn authorized(headers: &HeaderMap, expected: &str) -> bool {
    let Some(value) = headers.get(AUTHORIZATION) else {
        return false;
    };
    let Ok(text) = value.to_str() else {
        return false;
    };
    let Some(token) = text.strip_prefix("Bearer ") else {
        return false;
    };
    keys_match(token, expected)
}

fn keys_match(provided: &str, expected: &str) -> bool {
    let provided = provided.as_bytes();
    let expected = expected.as_bytes();
    let mut diff = provided.len() ^ expected.len();
    let length = provided.len().max(expected.len());
    for index in 0..length {
        let left = provided.get(index).copied().unwrap_or(0);
        let right = expected.get(index).copied().unwrap_or(0);
        diff |= usize::from(left ^ right);
    }
    diff == 0
}

async fn create_job(
    State(state): State<Arc<AppState>>,
    _auth: Authed,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let form = read_form(&mut multipart).await?;
    let phrase_bars = match form.mode.as_str() {
        "phrase" => Some(form.phrase_bars.unwrap_or(1)),
        _ => {
            if form.phrase_bars.is_some() {
                return Err(AppError::bad_request(
                    "phrase_bars applies only when mode is phrase",
                ));
            }
            None
        }
    };
    if form.bars == 0 {
        return Err(AppError::bad_request("bars must be at least 1"));
    }
    if form.bars > state.config.max_bars {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "bars {} exceeds SP404_MAX_BARS {}",
                form.bars, state.config.max_bars
            ),
        ));
    }
    let style = Style::parse(&form.style)
        .ok_or_else(|| AppError::bad_request("style must be house, boom-bap, or techno"))?;
    let mode = match form.mode.as_str() {
        "transient" => ChopMode::Transient,
        "beat-grid" => ChopMode::BeatGrid,
        "phrase" => {
            let bars = phrase_bars.unwrap_or(1);
            if !matches!(bars, 1 | 2 | 4) {
                return Err(AppError::bad_request("phrase bars must be 1, 2, or 4"));
            }
            ChopMode::Phrase { bars }
        }
        _ => {
            return Err(AppError::bad_request(
                "mode must be transient, beat-grid, or phrase",
            ))
        }
    };
    let permit =
        state.slots.clone().try_acquire_owned().map_err(|_| {
            AppError::new(StatusCode::TOO_MANY_REQUESTS, "too many jobs in progress")
        })?;
    let id = format!("job-{}", state.next_id.fetch_add(1, Ordering::Relaxed));
    let job = Job {
        id: id.clone(),
        status: JobStatus::Queued,
        style: style.as_str().to_string(),
        seed: form.seed,
        bars: form.bars,
        mode: mode.as_str().to_string(),
        phrase_bars: mode.phrase_bars(),
        error: None,
        plan_json: None,
        wav: None,
    };
    let body = job.view();
    state.jobs.lock().await.insert(id.clone(), job);
    let request = JobRequest {
        style,
        seed: form.seed,
        bars: form.bars,
        mode,
    };
    let wav = form.audio;
    let runner = Arc::clone(&state);
    let job_id = id.clone();
    tokio::spawn(async move {
        run_job(runner, job_id, wav, request, permit).await;
    });
    Ok((StatusCode::ACCEPTED, Json(body)).into_response())
}

struct Form {
    audio: Vec<u8>,
    style: String,
    seed: u64,
    bars: u32,
    mode: String,
    phrase_bars: Option<u32>,
}

async fn read_form(multipart: &mut Multipart) -> Result<Form, AppError> {
    let mut audio = None;
    let mut style = None;
    let mut seed = None;
    let mut bars = None;
    let mut mode = None;
    let mut phrase_bars = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|parse| AppError::bad_request(format!("invalid multipart body: {parse}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "audio" if audio.is_none() => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|read| AppError::bad_request(format!("reading audio: {read}")))?;
                audio = Some(bytes.to_vec());
            }
            "style" if style.is_none() => {
                style = Some(field_text(field).await?);
            }
            "seed" if seed.is_none() => {
                let text = field_text(field).await?;
                seed = Some(
                    text.parse::<u64>()
                        .map_err(|_| AppError::bad_request(format!("invalid seed '{text}'")))?,
                );
            }
            "bars" if bars.is_none() => {
                let text = field_text(field).await?;
                bars = Some(
                    text.parse::<u32>()
                        .map_err(|_| AppError::bad_request(format!("invalid bars '{text}'")))?,
                );
            }
            "mode" if mode.is_none() => {
                mode = Some(field_text(field).await?);
            }
            "phrase_bars" if phrase_bars.is_none() => {
                let text = field_text(field).await?;
                phrase_bars =
                    Some(text.parse::<u32>().map_err(|_| {
                        AppError::bad_request(format!("invalid phrase_bars '{text}'"))
                    })?);
            }
            _ => {}
        }
    }
    let audio = audio.ok_or_else(|| AppError::bad_request("missing audio file"))?;
    if audio.is_empty() {
        return Err(AppError::bad_request("audio file is empty"));
    }
    Ok(Form {
        audio,
        style: style.ok_or_else(|| AppError::bad_request("missing style"))?,
        seed: seed.ok_or_else(|| AppError::bad_request("missing seed"))?,
        bars: bars.ok_or_else(|| AppError::bad_request("missing bars"))?,
        mode: mode.ok_or_else(|| AppError::bad_request("missing mode"))?,
        phrase_bars,
    })
}

async fn field_text(field: axum::extract::multipart::Field<'_>) -> Result<String, AppError> {
    field
        .text()
        .await
        .map_err(|read| AppError::bad_request(format!("reading form field: {read}")))
}

async fn run_job(
    state: Arc<AppState>,
    id: String,
    wav: Vec<u8>,
    request: JobRequest,
    permit: OwnedSemaphorePermit,
) {
    set_status(&state, &id, JobStatus::Running).await;
    let outcome = spawn_blocking(move || render_upload(&wav, &request)).await;
    let result = match outcome {
        Ok(Ok(rendered)) => Ok(rendered),
        Ok(Err(message)) => Err(message),
        Err(_) => Err("job panicked".to_string()),
    };
    {
        let mut jobs = state.jobs.lock().await;
        if let Some(job) = jobs.get_mut(&id) {
            match result {
                Ok(rendered) => {
                    job.status = JobStatus::Succeeded;
                    job.plan_json = Some(rendered.plan_json);
                    job.wav = Some(rendered.wav);
                    job.error = None;
                }
                Err(message) => {
                    job.status = JobStatus::Failed;
                    job.error = Some(message);
                }
            }
        }
    }
    drop(permit);
}

async fn set_status(state: &AppState, id: &str, status: JobStatus) {
    if let Some(job) = state.jobs.lock().await.get_mut(id) {
        job.status = status;
    }
}

async fn job_status(
    State(state): State<Arc<AppState>>,
    _auth: Authed,
    Path(id): Path<String>,
) -> Result<Json<JobBody>, AppError> {
    let jobs = state.jobs.lock().await;
    let job = jobs.get(&id).ok_or_else(unknown_job)?;
    Ok(Json(job.view()))
}

async fn job_plan(
    State(state): State<Arc<AppState>>,
    _auth: Authed,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let jobs = state.jobs.lock().await;
    let job = jobs.get(&id).ok_or_else(unknown_job)?;
    let plan = ready(job)?.plan_json.clone().unwrap_or_default();
    Ok(([(CONTENT_TYPE, "application/json")], plan).into_response())
}

async fn job_audio(
    State(state): State<Arc<AppState>>,
    _auth: Authed,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let jobs = state.jobs.lock().await;
    let job = jobs.get(&id).ok_or_else(unknown_job)?;
    let wav = ready(job)?.wav.clone().unwrap_or_default();
    Ok(([(CONTENT_TYPE, "audio/wav")], Body::from(wav)).into_response())
}

fn ready(job: &Job) -> Result<&Job, AppError> {
    if job.status == JobStatus::Succeeded {
        Ok(job)
    } else {
        Err(AppError::new(StatusCode::CONFLICT, "job has not succeeded"))
    }
}

fn unknown_job() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "unknown job")
}
