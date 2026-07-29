//! Voice input surface backed by scheduler-owned ASR tasks.

use axum::body::Bytes;
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::json;
use std::io::Write;

use crate::state::SharedState;

const ASR_TASK: &str = "kb.meeting.asr_frontend";
const VOICE_TRANSCRIBE_ROUTE: &str = "/api/v1/voice/transcribe";
const VOICE_TRANSCRIBE_FILE_ROUTE: &str = "/api/v1/voice/transcribe-file";
const OFFICE_TRANSCRIBE_ROUTE: &str = "/api/v1/office/transcribe";
pub(crate) const MAX_VOICE_AUDIO_BYTES: usize = 100 * 1024 * 1024;

pub(crate) fn scheduler_model_ready(model: &attune_core::edge_cloud::SchedulerModelStatus) -> bool {
    model.lifecycle.eq_ignore_ascii_case("ready")
        && matches!(
            model.dispatchable.trim().to_ascii_uppercase().as_str(),
            "FREE" | "BUSY" | "QUEUED"
        )
}

#[derive(Debug, Clone)]
pub(crate) struct VoiceTaskStatus {
    pub available: bool,
    pub registered: bool,
    pub model: Option<String>,
    pub model_state: Option<String>,
    pub dispatchable: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct VoiceReadiness {
    pub asr: VoiceTaskStatus,
    pub tts: VoiceTaskStatus,
}

fn task_status(
    scheduler: &crate::local_scheduler::SchedulerRuntimeProbe,
    task_name: &'static str,
    required_model: Option<&'static str>,
    missing_task_note: &'static str,
    missing_model_note: impl FnOnce(&str) -> String,
) -> VoiceTaskStatus {
    let registered = scheduler
        .tasks
        .iter()
        .any(|candidate| candidate == task_name);
    let task = scheduler.runtime_tasks.iter().find(|task| {
        task.name == task_name
            && task.async_only
            && required_model.map_or(true, |model| task.model == model)
    });
    let model_name = task
        .map(|task| task.model.as_str())
        .filter(|model| !model.trim().is_empty());
    let model =
        model_name.and_then(|name| scheduler.models.iter().find(|model| model.name == name));
    let model_ready = model.is_some_and(scheduler_model_ready);
    let available = registered && task.is_some() && model_ready;
    let note = if !registered {
        Some(missing_task_note.to_string())
    } else if !model_ready {
        Some(missing_model_note(model_name.unwrap_or("ASR")))
    } else {
        None
    };
    VoiceTaskStatus {
        available,
        registered,
        model: model_name.map(str::to_string),
        model_state: model.map(|model| model.state.clone()),
        dispatchable: model.map(|model| model.dispatchable.clone()),
        note,
    }
}

pub(crate) fn scheduler_voice_readiness(
    scheduler: &crate::local_scheduler::SchedulerRuntimeProbe,
) -> VoiceReadiness {
    VoiceReadiness {
        asr: task_status(
            scheduler,
            ASR_TASK,
            None,
            "local scheduler 未暴露 kb.meeting.asr_frontend",
            |model| format!("local scheduler 已注册 ASR task，但 {model} 模型尚未配置或不可调度"),
        ),
        tts: task_status(
            scheduler,
            crate::routes::tts::TTS_TASK,
            Some(crate::routes::tts::TTS_ENGINE),
            "local scheduler 未暴露 kb.speech.synthesize",
            |_| {
                "local scheduler 已注册 TTS task，但 tts-default 模型尚未配置或不可调度".to_string()
            },
        ),
    }
}

/// GET /api/v1/voice/status
///
/// Single Attune-facing voice input projection. The actual ASR model execution
/// remains scheduler-owned; browser/demo clients should submit captured audio
/// to these Attune endpoints instead of scheduler URLs.
pub async fn status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let scheduler_base = crate::local_scheduler::base_from_state(&state);
    let scheduler = crate::local_scheduler::probe_scheduler_runtime(scheduler_base.clone()).await;
    let readiness = scheduler_voice_readiness(&scheduler);
    state.set_tts_capability_ready(readiness.tts.available);

    Json(json!({
        "schema_version": "attune.voice.v1",
        "scheduler": {
            "endpoint": scheduler_base,
            "status": scheduler.status,
            "error": scheduler.error,
        },
        "routes": {
            "status": "/api/v1/voice/status",
            "transcribe": VOICE_TRANSCRIBE_ROUTE,
            "transcribe_file": VOICE_TRANSCRIBE_FILE_ROUTE,
            "legacy_transcribe": OFFICE_TRANSCRIBE_ROUTE,
        },
        "asr": {
            "available": readiness.asr.available,
            "registered": readiness.asr.registered,
            "task": ASR_TASK,
            "model": readiness.asr.model,
            "model_state": readiness.asr.model_state,
            "dispatchable": readiness.asr.dispatchable,
            "route": VOICE_TRANSCRIBE_ROUTE,
            "file_route": VOICE_TRANSCRIBE_FILE_ROUTE,
            "legacy_route": OFFICE_TRANSCRIBE_ROUTE,
            "engine": if readiness.asr.available { "scheduler:kb.meeting.asr_frontend" } else { "scheduler" },
            "note": readiness.asr.note,
        },
    }))
}

fn voice_err(
    code: &str,
    error: impl Into<String>,
    status: StatusCode,
) -> (StatusCode, Json<serde_json::Value>) {
    (
        status,
        Json(json!({
            "error": error.into(),
            "code": code,
        })),
    )
}

async fn require_asr_ready(
    state: &SharedState,
) -> Result<VoiceTaskStatus, (StatusCode, Json<serde_json::Value>)> {
    let scheduler_base = crate::local_scheduler::base_from_state(state);
    let scheduler = crate::local_scheduler::probe_scheduler_runtime(scheduler_base).await;
    let readiness = scheduler_voice_readiness(&scheduler);
    if !readiness.asr.available {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": readiness
                    .asr
                    .note
                    .clone()
                    .unwrap_or_else(|| "ASR scheduler model is not ready".to_string()),
                "code": "voice-asr-not-ready",
                "task": ASR_TASK,
                "model": readiness.asr.model,
                "retryable": true,
            })),
        ));
    }
    Ok(readiness.asr)
}

/// POST /api/v1/voice/transcribe
pub async fn transcribe(
    State(state): State<SharedState>,
    Json(req): Json<crate::routes::office::TranscribeRequest>,
) -> Result<
    (StatusCode, Json<crate::routes::office::TranscribeResponse>),
    (StatusCode, Json<serde_json::Value>),
> {
    require_asr_ready(&state).await?;
    crate::routes::office::post_transcribe(State(state), Json(req)).await
}

fn is_trueish(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn audio_suffix(file_name: Option<&str>, content_type: Option<&str>) -> &'static str {
    if let Some(ext) = file_name
        .and_then(|name| std::path::Path::new(name).extension())
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.trim().to_ascii_lowercase())
    {
        match ext.as_str() {
            "aac" => return ".aac",
            "flac" => return ".flac",
            "m4a" => return ".m4a",
            "mp3" => return ".mp3",
            "ogg" => return ".ogg",
            "opus" => return ".opus",
            "wav" => return ".wav",
            "webm" => return ".webm",
            _ => {}
        }
    }

    match content_type
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "audio/aac" => ".aac",
        "audio/flac" => ".flac",
        "audio/mp4" | "audio/x-m4a" => ".m4a",
        "audio/mpeg" | "audio/mp3" => ".mp3",
        "audio/ogg" => ".ogg",
        "audio/opus" => ".opus",
        "audio/wav" | "audio/wave" | "audio/x-wav" => ".wav",
        "audio/webm" => ".webm",
        _ => ".audio",
    }
}

fn persist_voice_audio(
    bytes: &[u8],
    file_name: Option<&str>,
    content_type: Option<&str>,
) -> Result<std::path::PathBuf, (StatusCode, Json<serde_json::Value>)> {
    if bytes.is_empty() {
        return Err(voice_err(
            "empty-voice-audio",
            "uploaded audio is empty",
            StatusCode::BAD_REQUEST,
        ));
    }
    if bytes.len() > MAX_VOICE_AUDIO_BYTES {
        return Err(voice_err(
            "voice-audio-too-large",
            format!("uploaded audio exceeds {} bytes", MAX_VOICE_AUDIO_BYTES),
            StatusCode::PAYLOAD_TOO_LARGE,
        ));
    }

    let dir = std::env::temp_dir().join("attune-voice-uploads");
    std::fs::create_dir_all(&dir).map_err(|err| {
        voice_err(
            "voice-upload-store-unavailable",
            format!("failed to prepare voice upload directory: {err}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;
    let suffix = audio_suffix(file_name, content_type);
    let mut temp_file = tempfile::Builder::new()
        .prefix("attune-voice-")
        .suffix(suffix)
        .tempfile_in(&dir)
        .map_err(|err| {
            voice_err(
                "voice-upload-store-unavailable",
                format!("failed to create voice upload file: {err}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
    temp_file.write_all(bytes).map_err(|err| {
        voice_err(
            "voice-upload-write-failed",
            format!("failed to write voice upload: {err}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;
    temp_file.flush().map_err(|err| {
        voice_err(
            "voice-upload-write-failed",
            format!("failed to flush voice upload: {err}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;
    let (_file, path) = temp_file.keep().map_err(|err| {
        voice_err(
            "voice-upload-store-unavailable",
            format!("failed to persist voice upload: {err}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;
    Ok(path)
}

/// POST /api/v1/voice/transcribe-file
pub async fn transcribe_file(
    State(state): State<SharedState>,
    mut multipart: Multipart,
) -> Result<
    (StatusCode, Json<crate::routes::office::TranscribeResponse>),
    (StatusCode, Json<serde_json::Value>),
> {
    require_asr_ready(&state).await?;

    let mut audio: Option<Bytes> = None;
    let mut file_name: Option<String> = None;
    let mut content_type: Option<String> = None;
    let mut language = "auto".to_string();
    let mut model = "small".to_string();
    let mut diarization = false;

    while let Some(field) = multipart.next_field().await.map_err(|err| {
        voice_err(
            "invalid-voice-upload",
            format!("failed to read multipart field: {err}"),
            StatusCode::BAD_REQUEST,
        )
    })? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" | "audio" | "audio_file" => {
                file_name = field.file_name().map(str::to_string);
                content_type = field.content_type().map(str::to_string);
                let bytes = field.bytes().await.map_err(|err| {
                    voice_err(
                        "invalid-voice-upload",
                        format!("failed to read audio field: {err}"),
                        StatusCode::BAD_REQUEST,
                    )
                })?;
                if bytes.len() > MAX_VOICE_AUDIO_BYTES {
                    return Err(voice_err(
                        "voice-audio-too-large",
                        format!("uploaded audio exceeds {} bytes", MAX_VOICE_AUDIO_BYTES),
                        StatusCode::PAYLOAD_TOO_LARGE,
                    ));
                }
                audio = Some(bytes);
            }
            "language" => {
                language = field.text().await.map_err(|err| {
                    voice_err(
                        "invalid-voice-upload",
                        format!("failed to read language field: {err}"),
                        StatusCode::BAD_REQUEST,
                    )
                })?;
            }
            "model" => {
                model = field.text().await.map_err(|err| {
                    voice_err(
                        "invalid-voice-upload",
                        format!("failed to read model field: {err}"),
                        StatusCode::BAD_REQUEST,
                    )
                })?;
            }
            "diarization" => {
                let value = field.text().await.map_err(|err| {
                    voice_err(
                        "invalid-voice-upload",
                        format!("failed to read diarization field: {err}"),
                        StatusCode::BAD_REQUEST,
                    )
                })?;
                diarization = is_trueish(&value);
            }
            _ => {}
        }
    }

    let audio = audio.ok_or_else(|| {
        voice_err(
            "invalid-voice-upload",
            "multipart upload must include a file, audio, or audio_file field",
            StatusCode::BAD_REQUEST,
        )
    })?;
    let path = persist_voice_audio(&audio, file_name.as_deref(), content_type.as_deref())?;
    let req = crate::routes::office::TranscribeRequest {
        file_path: path.to_string_lossy().to_string(),
        language,
        model,
        diarization,
        max_speakers: None,
    };
    crate::routes::office::post_transcribe(State(state), Json(req)).await
}
