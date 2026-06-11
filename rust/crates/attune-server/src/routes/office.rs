//! /api/v1/office — 办公助理入口 (OCR 同步 + ASR 异步).
//!
//! Spec: docs/superpowers/specs/2026-05-20-office-helper-design.md
//! 个人助手语义：不限并发不 reject, 信号量门控 + FIFO 排队 + 软警告.
//! 错误码契约：{error: msg, code: kebab} (per CLAUDE.md error contract).

use crate::state::SharedState;
use attune_core::ocr::{self, RawLine};
use attune_core::office_job_queue::{JobKind, JobRecord, JobState};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use serde::{Deserialize, Serialize};

// ─── OCR (sync) ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct OcrResponse {
    pub envelope_version: &'static str,
    pub profile: String,
    pub elapsed_ms: u64,
    pub engine: String,
    pub lines: Vec<RawLine>,
    /// `Some` = B 档结构化抽取结果（schema-tagged）；`None` = A 档 only
    /// (screenshot / contract / ancient / form 或 抽取器未实现时).
    pub structured: Option<serde_json::Value>,
    /// 软警告（per spec §2.4 个人助手语义 — 文件大 / 模型回退 等）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

const OCR_SOFT_WARN_BYTES: usize = 50 * 1024 * 1024; // 50 MB

fn err(
    code: &str,
    msg: &str,
    status: StatusCode,
) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(serde_json::json!({ "error": msg, "code": code })))
}

/// POST /api/v1/office/ocr — sync, multipart/form-data
pub async fn post_ocr(
    State(_state): State<SharedState>,
    mut multipart: Multipart,
) -> Result<Json<OcrResponse>, (StatusCode, Json<serde_json::Value>)> {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    let mut profile: Option<String> = None;
    let mut id_card_subtype: Option<String> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        err(
            "invalid-input",
            &format!("multipart parse: {e}"),
            StatusCode::BAD_REQUEST,
        )
    })? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                filename = field.file_name().map(|s| s.to_string());
                let bytes = field.bytes().await.map_err(|e| {
                    err(
                        "invalid-input",
                        &format!("file read: {e}"),
                        StatusCode::BAD_REQUEST,
                    )
                })?;
                file_bytes = Some(bytes.to_vec());
            }
            "profile" => profile = Some(field.text().await.unwrap_or_default()),
            "id_card_subtype" => id_card_subtype = Some(field.text().await.unwrap_or_default()),
            _ => {}
        }
    }

    let bytes = file_bytes.ok_or_else(|| {
        err("invalid-input", "file required", StatusCode::BAD_REQUEST)
    })?;
    if bytes.is_empty() {
        return Err(err("empty-file", "file is empty", StatusCode::BAD_REQUEST));
    }
    let profile = profile.ok_or_else(|| {
        err("invalid-input", "profile required", StatusCode::BAD_REQUEST)
    })?;

    if profile == "id_card" && id_card_subtype.as_deref().unwrap_or("").is_empty() {
        return Err(err(
            "id-card-subtype-required",
            "id_card_subtype required when profile=id_card",
            StatusCode::BAD_REQUEST,
        ));
    }

    // profile whitelist
    let allowed = [
        "document", "receipt", "table", "card", "id_card",
        "screenshot", "ancient", "form", "contract",
    ];
    if !allowed.contains(&profile.as_str()) {
        return Err(err(
            "profile-not-found",
            &format!("unknown profile: {profile}"),
            StatusCode::NOT_FOUND,
        ));
    }

    // file extension whitelist (PDF + common images)
    let ext_ok = filename
        .as_deref()
        .map(|n| {
            let l = n.to_lowercase();
            l.ends_with(".pdf") || l.ends_with(".png") || l.ends_with(".jpg")
                || l.ends_with(".jpeg") || l.ends_with(".webp") || l.ends_with(".bmp")
                || l.ends_with(".tiff") || l.ends_with(".tif") || l.ends_with(".gif")
        })
        .unwrap_or(false);
    if !ext_ok {
        return Err(err(
            "unsupported-format",
            "unsupported file extension (need .pdf/.png/.jpg/.jpeg/.webp/.bmp/.tiff/.gif)",
            StatusCode::BAD_REQUEST,
        ));
    }

    let mut warnings: Vec<String> = Vec::new();
    if bytes.len() > OCR_SOFT_WARN_BYTES {
        warnings.push(format!(
            "file is {:.1} MB; OCR may take >30s",
            bytes.len() as f64 / 1024.0 / 1024.0
        ));
    }

    let start = std::time::Instant::now();

    // write to tmp file (PP-OCR provider 接受 path)
    let tmp = tempfile::NamedTempFile::new().map_err(|e| {
        err(
            "internal-error",
            &format!("tempfile: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;
    std::fs::write(tmp.path(), &bytes).map_err(|e| {
        err(
            "internal-error",
            &format!("write tmp: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;

    let provider = ocr::detect_default_provider().ok_or_else(|| {
        err(
            "ocr-engine-failed",
            "OCR provider not available (PP-OCR models missing). Run `--bootstrap-models`.",
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;

    let is_pdf = filename
        .as_deref()
        .map(|n| n.to_lowercase().ends_with(".pdf"))
        .unwrap_or(false);

    let ocr_profile = ocr::profile_for_id(Some(&profile));

    // A 档：抽出 lines + bbox
    // D1: PDF lines 输出留空（pdftoppm 切页 + 逐页 OCR + 合并 lines 在 D2 补完）
    let lines: Vec<RawLine> = if is_pdf {
        vec![]
    } else {
        let out = provider
            .extract_structured(tmp.path(), &ocr_profile)
            .map_err(|e| {
                err(
                    "ocr-engine-failed",
                    &format!("OCR: {e}"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            })?;
        out.lines.unwrap_or_default()
    };

    // B 档结构化抽取 (D2 起 wired). A 档场景 (screenshot/contract/ancient/form) 返 None.
    let structured = if !lines.is_empty() {
        attune_core::ocr::structured::extract(&profile, &lines, id_card_subtype.as_deref())
            .and_then(|s| serde_json::to_value(s).ok())
    } else {
        None
    };

    Ok(Json(OcrResponse {
        envelope_version: "1",
        profile: profile.clone(),
        elapsed_ms: start.elapsed().as_millis() as u64,
        engine: provider.name().to_string(),
        lines,
        structured,
        warnings,
    }))
}

// ─── ASR (async) ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TranscribeRequest {
    pub file_path: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub diarization: bool,
    #[serde(default)]
    #[allow(dead_code)] // D2: 真正限制 speaker 数；当前 backend 不暴露上限
    pub max_speakers: Option<u32>,
}
fn default_language() -> String {
    "auto".into()
}
fn default_model() -> String {
    "small".into()
}

#[derive(Serialize)]
pub struct TranscribeResponse {
    pub job_id: String,
    pub ws_url: String,
}

/// ASR jobs that have not finished within this ceiling are failed by the worker's
/// timeout sweep (`job-timeout`). Generous: K3 long recordings via whisper subprocess.
const ASR_JOB_DEADLINE_MS: i64 = 60 * 60 * 1000; // 1h

/// G5: map a durable [`JobRecord`] to the office job-status JSON contract
/// (`job_id`/`state`/`stage`/`queue_position`/`progress`/`elapsed_ms`/`eta_ms`/
/// `result`/`error`/`warnings` — unchanged from the old in-memory JobRegistry shape).
/// `queue_position` defaults to 0; HTTP/WS callers overwrite it with the live value.
pub(crate) fn job_record_to_status_json(j: &JobRecord) -> serde_json::Value {
    // stage: surface the inner string of stage_json {"stage":"transcribing"};
    // fall back to the state name (pre-claim jobs were stage "queued" in the old enum).
    let stage = j
        .stage_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| v.get("stage").and_then(|x| x.as_str()).map(String::from))
        .unwrap_or_else(|| {
            match j.state {
                JobState::Queued => "queued",
                JobState::Running => "running",
                JobState::Done => "postprocess",
                JobState::Failed => "failed",
                JobState::Cancelled => "cancelled",
            }
            .to_string()
        });
    let elapsed_ms = j
        .started_ms
        .map(|s| {
            let end = j
                .finished_ms
                .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
            (end - s).max(0) as u64
        })
        .unwrap_or(0);
    serde_json::json!({
        "job_id": j.id,
        "kind": j.kind,
        "state": j.state,
        "stage": stage,
        "queue_position": 0,
        "progress": j.progress,
        "elapsed_ms": elapsed_ms,
        "eta_ms": serde_json::Value::Null,
        "result": j.result_json.as_ref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()),
        "error": j.error,
        "warnings": j.warnings,
    })
}

/// POST /api/v1/office/transcribe — 提交 ASR job, 立即返 job_id.
/// G5: enqueues to the durable `job_queue` table (survives restart — idempotent
/// ASR is requeued by `recover_on_boot`); the background job worker
/// (`crate::job_worker`) claims and runs it. HTTP contract unchanged.
pub async fn post_transcribe(
    State(state): State<SharedState>,
    Json(req): Json<TranscribeRequest>,
) -> Result<(StatusCode, Json<TranscribeResponse>), (StatusCode, Json<serde_json::Value>)> {
    let path = std::path::Path::new(&req.file_path);
    if !path.exists() {
        return Err(err(
            "invalid-input",
            &format!("file not found: {}", req.file_path),
            StatusCode::BAD_REQUEST,
        ));
    }

    let store = state.job_store().ok_or_else(|| {
        err(
            "job-store-unavailable",
            "durable job queue unavailable (store failed to open at boot)",
            StatusCode::SERVICE_UNAVAILABLE,
        )
    })?;

    // Durable payload: everything AsrJobHandler needs to (re-)run after a restart.
    // language/model are accepted for forward-compat but the backend self-detects.
    let payload = serde_json::json!({
        "file_path": req.file_path,
        "diarization": req.diarization,
    })
    .to_string();
    let deadline = chrono::Utc::now().timestamp_millis() + ASR_JOB_DEADLINE_MS;
    let job_id = {
        let s = store.lock().unwrap_or_else(|e| e.into_inner());
        s.enqueue_job(JobKind::Asr, &payload, 0, Some(deadline))
            .map_err(|e| {
                err(
                    "job-enqueue-failed",
                    &e.to_string(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            })?
    };
    let ws_url = format!("/api/v1/office/jobs/ws?job_id={job_id}");

    Ok((
        StatusCode::ACCEPTED,
        Json(TranscribeResponse { job_id, ws_url }),
    ))
}

/// Error shape of the office routes ({error, code} JSON per CLAUDE.md contract).
type OfficeErr = (StatusCode, Json<serde_json::Value>);

/// Fetch a job + its live queue position from the durable store.
fn fetch_job(state: &SharedState, job_id: &str) -> Result<Option<(JobRecord, usize)>, OfficeErr> {
    let store = state.job_store().ok_or_else(|| {
        err(
            "job-store-unavailable",
            "durable job queue unavailable (store failed to open at boot)",
            StatusCode::SERVICE_UNAVAILABLE,
        )
    })?;
    let s = store.lock().unwrap_or_else(|e| e.into_inner());
    let job = s.get_job(job_id).map_err(|e| {
        err(
            "job-read-failed",
            &e.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;
    match job {
        Some(j) => {
            let pos = s.job_queue_position(job_id).unwrap_or(0);
            Ok(Some((j, pos)))
        }
        None => Ok(None),
    }
}

/// GET /api/v1/office/jobs/{job_id}
pub async fn get_job(
    State(state): State<SharedState>,
    Path(job_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let (job, queue_pos) = fetch_job(&state, &job_id)?
        .ok_or_else(|| err("not-found", "job not found", StatusCode::NOT_FOUND))?;
    let mut v = job_record_to_status_json(&job);
    v["queue_position"] = queue_pos.into();
    Ok(Json(v))
}

/// DELETE /api/v1/office/jobs/{job_id}
pub async fn delete_job(
    State(state): State<SharedState>,
    Path(job_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let (job, _) = fetch_job(&state, &job_id)?
        .ok_or_else(|| err("not-found", "job not found", StatusCode::NOT_FOUND))?;
    match job.state {
        JobState::Done => Err(err(
            "job-already-completed",
            "job already completed",
            StatusCode::CONFLICT,
        )),
        JobState::Cancelled => Err(err(
            "job-already-cancelled",
            "job already cancelled",
            StatusCode::CONFLICT,
        )),
        JobState::Failed => Err(err(
            "job-already-completed",
            "job already failed (terminal)",
            StatusCode::CONFLICT,
        )),
        _ => {
            // cancel_job is guarded to queued/running; terminal races are 409 above.
            let store = state.job_store().ok_or_else(|| {
                err(
                    "job-store-unavailable",
                    "durable job queue unavailable",
                    StatusCode::SERVICE_UNAVAILABLE,
                )
            })?;
            let s = store.lock().unwrap_or_else(|e| e.into_inner());
            s.cancel_job(&job_id).map_err(|e| {
                err(
                    "job-cancel-failed",
                    &e.to_string(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            })?;
            Ok(StatusCode::NO_CONTENT)
        }
    }
}

// ─── WebSocket progress ──────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct WsQuery {
    pub job_id: String,
}

/// WS /api/v1/office/jobs/ws?job_id=<id>
pub async fn ws_jobs(
    ws: WebSocketUpgrade,
    State(state): State<SharedState>,
    Query(q): Query<WsQuery>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state, q.job_id))
}

async fn handle_socket(mut socket: WebSocket, state: SharedState, job_id: String) {
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let lookup = fetch_job(&state, &job_id).ok().flatten();
                let Some((job, queue_pos)) = lookup else {
                    let _ = socket.send(Message::Text(
                        serde_json::json!({
                            "type": "failed",
                            "job_id": job_id,
                            "error": {"message": "job not found", "code": "not-found"}
                        }).to_string().into()
                    )).await;
                    return;
                };
                let frame = match job.state {
                    JobState::Done => serde_json::json!({
                        "type": "done",
                        "job_id": job.id,
                        "result": job.result_json.as_ref()
                            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()),
                    }),
                    JobState::Failed => serde_json::json!({
                        "type": "failed",
                        "job_id": job.id,
                        "error": job.error,
                    }),
                    JobState::Cancelled => serde_json::json!({
                        "type": "cancelled",
                        "job_id": job.id,
                    }),
                    _ => {
                        let mut v = job_record_to_status_json(&job);
                        v["type"] = "progress".into();
                        v["queue_position"] = queue_pos.into();
                        v
                    }
                };
                if socket.send(Message::Text(frame.to_string().into())).await.is_err() {
                    return;
                }
                if matches!(job.state, JobState::Done | JobState::Failed | JobState::Cancelled) {
                    return;
                }
            }
            msg = socket.recv() => {
                let Some(Ok(Message::Text(t))) = msg else { return; };
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                    if v.get("type").and_then(|x| x.as_str()) == Some("cancel") {
                        if let Some(store) = state.job_store() {
                            let s = store.lock().unwrap_or_else(|e| e.into_inner());
                            let _ = s.cancel_job(&job_id);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use attune_core::office_job_queue::JobKind;
    use attune_core::store::Store;

    #[test]
    fn job_record_maps_to_status_json_with_stage() {
        let store = Store::open_memory().unwrap();
        let id = store
            .enqueue_job(JobKind::Asr, "{\"file_path\":\"a.wav\"}", 0, None)
            .unwrap();
        store.claim_next_job().unwrap();
        store
            .update_job_progress(&id, Some("{\"stage\":\"transcribing\"}"), 0.4)
            .unwrap();
        let j = store.get_job(&id).unwrap().unwrap();
        let v = super::job_record_to_status_json(&j);
        assert_eq!(v["state"], "running");
        assert_eq!(v["stage"], "transcribing");
        assert!((v["progress"].as_f64().unwrap() - 0.4).abs() < 1e-6);
        assert_eq!(v["job_id"], id);
    }

    #[test]
    fn queued_job_status_json_has_queued_stage_and_zero_elapsed() {
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        let j = store.get_job(&id).unwrap().unwrap();
        let v = super::job_record_to_status_json(&j);
        assert_eq!(v["state"], "queued");
        assert_eq!(v["stage"], "queued");
        assert_eq!(v["elapsed_ms"], 0);
        assert!(v["result"].is_null());
        assert!(v["error"].is_null());
    }

    #[test]
    fn failed_job_status_json_carries_error_contract() {
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.claim_next_job().unwrap();
        store.fail_job(&id, "asr-engine-failed", "whisper exited 1").unwrap();
        let j = store.get_job(&id).unwrap().unwrap();
        let v = super::job_record_to_status_json(&j);
        assert_eq!(v["state"], "failed");
        assert_eq!(v["error"]["code"], "asr-engine-failed");
        assert_eq!(v["error"]["message"], "whisper exited 1");
    }
}
