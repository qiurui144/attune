//! /api/v1/office — 办公助理入口 (OCR 同步 + ASR 异步).
//!
//! Spec: docs/superpowers/specs/2026-05-20-office-helper-design.md
//! 个人助手语义：不限并发不 reject, 信号量门控 + FIFO 排队 + 软警告.
//! 错误码契约：{error: msg, code: kebab} (per CLAUDE.md error contract).

use crate::state::SharedState;
use attune_core::ocr::{self, RawLine};
use attune_core::office_job_queue::{Job, JobError, JobStage, JobState};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

    Ok(Json(OcrResponse {
        envelope_version: "1",
        profile: profile.clone(),
        elapsed_ms: start.elapsed().as_millis() as u64,
        engine: provider.name().to_string(),
        lines,
        // D1: B 档 structured 抽取留 null；D2 补
        structured: None,
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

/// POST /api/v1/office/transcribe — 提交 ASR job, 立即返 job_id
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

    let job_id = format!("asr-{}", Uuid::new_v4().simple());
    let ws_url = format!("/api/v1/office/jobs/ws?job_id={job_id}");
    state.office_jobs.insert(Job::new(job_id.clone()));

    // Spawn ASR worker (blocking thread; whisper-cli 是子进程, tokio::spawn_blocking 是对的).
    let registry = state.office_jobs.clone();
    let job_id_for_task = job_id.clone();
    let file_path = req.file_path.clone();
    let _language = req.language.clone(); // ASR backend 自检测 language; 当前 attune-core API 不暴露 override
    let diarization = req.diarization;

    tokio::task::spawn_blocking(move || {
        registry.update(&job_id_for_task, |j| {
            j.state = JobState::Running;
            j.stage = JobStage::LoadingModel;
            j.started_at = Some(std::time::Instant::now());
        });

        let backend = match attune_core::asr::detect_asr_backend() {
            Some(b) => b,
            None => {
                registry.update(&job_id_for_task, |j| {
                    j.state = JobState::Failed;
                    j.error = Some(JobError {
                        message: "ASR backend not available (whisper-cli not installed)".into(),
                        code: "asr-engine-failed".into(),
                    });
                });
                return;
            }
        };

        registry.update(&job_id_for_task, |j| j.stage = JobStage::Transcribing);

        let diar_backend = if diarization {
            attune_core::asr::detect_diarization_backend()
        } else {
            None
        };

        let (segments, _full_text_legacy) = match attune_core::asr::transcribe_with_diarization(
            &backend,
            std::path::Path::new(&file_path),
            diar_backend.as_ref(),
        ) {
            Ok(s) => s,
            Err(e) => {
                registry.update(&job_id_for_task, |j| {
                    j.state = JobState::Failed;
                    j.error = Some(JobError {
                        message: e.to_string(),
                        code: "asr-engine-failed".into(),
                    });
                });
                return;
            }
        };

        // Aggregate speakers (TranscriptSegment uses ms granularity → 转 sec for response)
        let mut speakers_agg: std::collections::BTreeMap<String, (f64, u64)> =
            std::collections::BTreeMap::new();
        for s in &segments {
            let key = s.speaker.clone().unwrap_or_else(|| "SPEAKER_UNK".into());
            let dur = (s.end_ms as f64 - s.start_ms as f64).max(0.0) / 1000.0;
            let entry = speakers_agg.entry(key).or_insert((0.0, 0));
            entry.0 += dur;
            entry.1 += 1;
        }
        let duration_sec = segments.last().map(|s| s.end_ms as f64 / 1000.0).unwrap_or(0.0);

        let result = serde_json::json!({
            "model": backend.model_name,
            "language_detected": backend.language,
            "duration_sec": duration_sec,
            "segments": segments.iter().map(|s| serde_json::json!({
                "start_sec": s.start_ms as f64 / 1000.0,
                "end_sec": s.end_ms as f64 / 1000.0,
                "text": s.text,
                "speaker": s.speaker,
            })).collect::<Vec<_>>(),
            "speakers": speakers_agg.iter().map(|(id, (total, count))| serde_json::json!({
                "id": id,
                "total_sec": total,
                "segment_count": count,
            })).collect::<Vec<_>>(),
            "full_text": segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" "),
            "diarization_used": diar_backend.is_some(),
        });

        registry.update(&job_id_for_task, |j| {
            j.state = JobState::Done;
            j.stage = JobStage::Postprocess;
            j.progress = 1.0;
            j.result_json = Some(result.to_string());
            if let Some(start) = j.started_at {
                j.elapsed_ms = start.elapsed().as_millis() as u64;
            }
        });
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(TranscribeResponse { job_id, ws_url }),
    ))
}

/// GET /api/v1/office/jobs/{job_id}
pub async fn get_job(
    State(state): State<SharedState>,
    Path(job_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let job = state.office_jobs.get(&job_id).ok_or_else(|| {
        err("not-found", "job not found", StatusCode::NOT_FOUND)
    })?;
    let queue_pos = state.office_jobs.queue_position(&job_id);
    let elapsed = job
        .started_at
        .map(|s| s.elapsed().as_millis() as u64)
        .unwrap_or(job.elapsed_ms);
    Ok(Json(serde_json::json!({
        "job_id": job.id,
        "state": job.state,
        "stage": job.stage,
        "queue_position": queue_pos,
        "progress": job.progress,
        "elapsed_ms": elapsed,
        "eta_ms": job.eta_ms,
        "result": job.result_json.as_ref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()),
        "error": job.error,
        "warnings": job.warnings,
    })))
}

/// DELETE /api/v1/office/jobs/{job_id}
pub async fn delete_job(
    State(state): State<SharedState>,
    Path(job_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let job = state
        .office_jobs
        .get(&job_id)
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
            state
                .office_jobs
                .update(&job_id, |j| j.state = JobState::Cancelled);
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
                let Some(job) = state.office_jobs.get(&job_id) else {
                    let _ = socket.send(Message::Text(
                        serde_json::json!({
                            "type": "failed",
                            "job_id": job_id,
                            "error": {"message": "job not found", "code": "not-found"}
                        }).to_string().into()
                    )).await;
                    return;
                };
                let queue_pos = state.office_jobs.queue_position(&job_id);
                let elapsed = job.started_at.map(|s| s.elapsed().as_millis() as u64).unwrap_or(job.elapsed_ms);
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
                    _ => serde_json::json!({
                        "type": "progress",
                        "job_id": job.id,
                        "state": job.state,
                        "stage": job.stage,
                        "queue_position": queue_pos,
                        "progress": job.progress,
                        "elapsed_ms": elapsed,
                    }),
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
                        state.office_jobs.update(&job_id, |j| j.state = JobState::Cancelled);
                    }
                }
            }
        }
    }
}
