//! Scheduler-backed short text-to-speech synthesis.
//!
//! Attune owns request validation and the browser-facing WAV response. Model
//! selection, ORT execution, and hardware admission remain scheduler-owned.

use std::sync::TryLockError;
use std::time::Duration;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;
use attune_core::edge_cloud::SchedulerErrorKind;
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Json;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const TTS_TASK: &str = "kb.speech.synthesize";
pub(crate) const TTS_ROUTE: &str = "/api/v1/tts/synthesize";
const TTS_SCHEMA_VERSION: &str = "speech_audio.v1";
pub(crate) const TTS_ENGINE: &str = "tts-default";
const TTS_POLL_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_TEXT_CHARS: usize = 128;
const MAX_TEXT_BYTES: usize = 4 * 1024;
// Scheduler short-audio contract: canonical 44-byte WAV header plus at most
// 720_000 bytes of PCM payload; the retained JSON/base64 envelope stays <=1 MiB.
const MAX_AUDIO_BYTES: usize = 720_044;
pub(crate) const MAX_PUBLIC_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_DURATION_MS: u64 = 15_000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TtsRequest {
    pub text: String,
    #[serde(default, deserialize_with = "deserialize_non_null_option")]
    pub voice: Option<String>,
    #[serde(default, deserialize_with = "deserialize_non_null_option")]
    pub language: Option<String>,
    #[serde(default, deserialize_with = "deserialize_non_null_option")]
    pub speed: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_non_null_option")]
    pub output_format: Option<String>,
}

fn deserialize_non_null_option<'de, D, T>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Debug, Serialize, PartialEq)]
struct SchedulerTtsRequest {
    text: String,
    voice: String,
    language: String,
    speed: f64,
    output_format: String,
}

#[derive(Debug)]
struct TtsSettings {
    enabled: bool,
    provider: String,
    task: String,
    voice: String,
    language: String,
    speed: f64,
    format: String,
}

impl Default for TtsSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: "local_scheduler".to_string(),
            task: TTS_TASK.to_string(),
            voice: "auto".to_string(),
            language: "auto".to_string(),
            speed: 1.0,
            format: "wav".to_string(),
        }
    }
}

impl TtsSettings {
    fn from_value(settings: &serde_json::Value) -> Result<Self, String> {
        let Some(tts) = settings.get("tts") else {
            return Ok(Self::default());
        };
        let obj = tts
            .as_object()
            .ok_or_else(|| "tts settings must be an object".to_string())?;
        let defaults = Self::default();
        Ok(Self {
            enabled: optional_bool(obj, "enabled")?.unwrap_or(defaults.enabled),
            provider: optional_string(obj, "provider")?.unwrap_or(defaults.provider),
            task: optional_string(obj, "task")?.unwrap_or(defaults.task),
            voice: optional_string(obj, "voice")?.unwrap_or(defaults.voice),
            language: optional_string(obj, "language")?.unwrap_or(defaults.language),
            speed: optional_f64(obj, "speed")?.unwrap_or(defaults.speed),
            format: optional_string(obj, "format")?.unwrap_or(defaults.format),
        })
    }

    fn validate(&self) -> Result<(), String> {
        if self.provider != "local_scheduler" {
            return Err("tts.provider must be local_scheduler".to_string());
        }
        if self.task != TTS_TASK {
            return Err(format!("tts.task must be {TTS_TASK}"));
        }
        validate_voice(&self.voice)?;
        validate_language(&self.language)?;
        validate_speed(self.speed)?;
        validate_format(&self.format)
    }
}

fn optional_bool(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<bool>, String> {
    obj.get(key)
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| format!("tts.{key} must be a boolean"))
        })
        .transpose()
}

fn optional_string(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<String>, String> {
    obj.get(key)
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("tts.{key} must be a string"))
        })
        .transpose()
}

fn optional_f64(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<f64>, String> {
    obj.get(key)
        .map(|value| {
            value
                .as_f64()
                .ok_or_else(|| format!("tts.{key} must be a number"))
        })
        .transpose()
}

fn load_tts_settings(state: &SharedState) -> Result<TtsSettings, AppError> {
    let vault = match state.vault.try_lock() {
        Ok(vault) => vault,
        Err(TryLockError::Poisoned(error)) => error.into_inner(),
        Err(TryLockError::WouldBlock) => {
            tracing::debug!(
                task = TTS_TASK,
                "TTS settings read rejected while the vault is busy"
            );
            return Err(settings_busy_error());
        }
    };
    let settings = crate::settings_store::load_settings(&vault)
        .map_err(|error| {
            tracing::error!(error = %error, "Failed to load TTS settings");
            AppError::detailed(
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({
                    "error": "text-to-speech settings unavailable",
                    "code": "tts-settings-unavailable"
                }),
            )
        })?
        .unwrap_or_else(|| serde_json::json!({}));
    TtsSettings::from_value(&settings).map_err(|error| {
        AppError::detailed(
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({
                "error": error,
                "code": "tts-invalid-settings"
            }),
        )
    })
}

pub(crate) fn settings_busy_error() -> AppError {
    AppError::detailed(
        StatusCode::SERVICE_UNAVAILABLE,
        serde_json::json!({
            "error": "text-to-speech settings are temporarily busy",
            "code": "tts-settings-busy",
            "retryable": true,
            "may_degrade": false,
            "degradation_allowed": false,
            "degradation_policy": "honest_failure",
            "task": TTS_TASK,
            "operation": "tts_synthesize",
            "component": "tts"
        }),
    )
}

fn invalid_request(message: impl Into<String>) -> AppError {
    AppError::detailed(
        StatusCode::BAD_REQUEST,
        serde_json::json!({
            "error": message.into(),
            "code": "invalid-tts-request"
        }),
    )
}

fn sanitized_tts_scheduler_failure(kind: SchedulerErrorKind) -> (StatusCode, serde_json::Value) {
    let (status, code, scheduler_error, retryable) = match kind {
        SchedulerErrorKind::Busy => (
            StatusCode::SERVICE_UNAVAILABLE,
            "local-scheduler-busy",
            "busy",
            true,
        ),
        SchedulerErrorKind::Oversize => (
            StatusCode::PAYLOAD_TOO_LARGE,
            "local-scheduler-oversize",
            "oversize",
            false,
        ),
        SchedulerErrorKind::RateLimited => (
            StatusCode::TOO_MANY_REQUESTS,
            "local-scheduler-rate-limited",
            "rate-limited",
            true,
        ),
        SchedulerErrorKind::Unavailable | SchedulerErrorKind::Transport => (
            StatusCode::SERVICE_UNAVAILABLE,
            "local-scheduler-unavailable",
            kind.as_str(),
            true,
        ),
        SchedulerErrorKind::Delayed => (
            StatusCode::GATEWAY_TIMEOUT,
            "local-scheduler-delayed",
            "delayed",
            true,
        ),
        SchedulerErrorKind::Cancelled => (
            StatusCode::CONFLICT,
            "local-scheduler-cancelled",
            "cancelled",
            false,
        ),
        SchedulerErrorKind::Expired => (
            StatusCode::GONE,
            "local-scheduler-expired",
            "expired",
            false,
        ),
        SchedulerErrorKind::JobFailed => (
            StatusCode::BAD_GATEWAY,
            "local-scheduler-job-failed",
            "job-failed",
            false,
        ),
        SchedulerErrorKind::InvalidJson => (
            StatusCode::BAD_GATEWAY,
            "local-scheduler-invalid-response",
            "invalid-json",
            false,
        ),
        SchedulerErrorKind::Http(status) if (500..600).contains(&status) => (
            StatusCode::BAD_GATEWAY,
            "local-scheduler-upstream-error",
            "http-error",
            true,
        ),
        SchedulerErrorKind::Http(_) => (
            StatusCode::BAD_REQUEST,
            "local-scheduler-request-rejected",
            "http-error",
            false,
        ),
    };
    (
        status,
        serde_json::json!({
            "error": "text-to-speech scheduler task failed",
            "code": code,
            "scheduler_error": scheduler_error,
            "retryable": retryable,
            "may_degrade": false,
            "degradation_allowed": false,
            "degradation_policy": "honest_failure",
            "task": TTS_TASK,
            "operation": "tts_synthesize",
            "component": "tts"
        }),
    )
}

fn resolve_request(
    request: TtsRequest,
    settings: &TtsSettings,
) -> Result<SchedulerTtsRequest, AppError> {
    // Reject controls on the original input. Checking only after trimming
    // would silently accept boundary newlines/tabs and weaken the public
    // semantic contract.
    if request.text.chars().any(char::is_control) {
        return Err(invalid_request("text must not contain control characters"));
    }
    let text = request.text.trim_matches(' ').to_string();
    if text.is_empty() {
        return Err(invalid_request("text must not be empty"));
    }
    if text.chars().count() > MAX_TEXT_CHARS || text.len() > MAX_TEXT_BYTES {
        return Err(AppError::detailed(
            StatusCode::PAYLOAD_TOO_LARGE,
            serde_json::json!({
                "error": format!(
                    "text exceeds the short TTS budget ({MAX_TEXT_CHARS} characters / {MAX_TEXT_BYTES} UTF-8 bytes)"
                ),
                "code": "tts-text-too-large"
            }),
        ));
    }
    let resolved = SchedulerTtsRequest {
        text,
        voice: request.voice.unwrap_or_else(|| settings.voice.clone()),
        language: request
            .language
            .unwrap_or_else(|| settings.language.clone()),
        speed: request.speed.unwrap_or(settings.speed),
        output_format: request
            .output_format
            .unwrap_or_else(|| settings.format.clone()),
    };
    validate_voice(&resolved.voice).map_err(invalid_request)?;
    validate_language(&resolved.language).map_err(invalid_request)?;
    validate_speed(resolved.speed).map_err(invalid_request)?;
    validate_format(&resolved.output_format).map_err(invalid_request)?;
    Ok(resolved)
}

pub(crate) fn validate_voice(voice: &str) -> Result<(), String> {
    if voice.is_empty()
        || voice.len() > 64
        || !voice.is_ascii()
        || !voice
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
    {
        return Err(
            "voice must match [A-Za-z0-9_.-]{1,64} (use auto or default when unsure)".to_string(),
        );
    }
    Ok(())
}

pub(crate) fn validate_language(language: &str) -> Result<(), String> {
    if !matches!(language, "auto" | "zh-CN" | "en-US") {
        return Err("language must be auto, zh-CN, or en-US".to_string());
    }
    Ok(())
}

pub(crate) fn validate_speed(speed: f64) -> Result<(), String> {
    if !speed.is_finite() || !(0.5..=2.0).contains(&speed) {
        return Err("speed must be a finite number from 0.5 through 2.0".to_string());
    }
    Ok(())
}

pub(crate) fn validate_format(format: &str) -> Result<(), String> {
    if format != "wav" {
        return Err("output_format must be wav".to_string());
    }
    Ok(())
}

pub async fn synthesize(
    State(state): State<SharedState>,
    payload: Result<Json<TtsRequest>, JsonRejection>,
) -> AppResult<Response> {
    let request = payload
        .map_err(|error| {
            tracing::debug!(error = %error, "Rejected invalid TTS JSON request");
            invalid_request("invalid TTS JSON request")
        })?
        .0;
    let settings = load_tts_settings(&state)?;
    settings.validate().map_err(|error| {
        AppError::detailed(
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({"error": error, "code": "tts-invalid-settings"}),
        )
    })?;
    if !settings.enabled {
        return Err(AppError::detailed(
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({"error": "text-to-speech is disabled", "code": "tts-disabled"}),
        ));
    }
    let scheduler_request = resolve_request(request, &settings)?;
    let expected_voice = scheduler_request.voice.clone();
    let expected_language = scheduler_request.language.clone();
    let expected_format = scheduler_request.output_format.clone();

    let outputs =
        crate::scheduler_tasks::submit_tts_task_final(&state, scheduler_request, TTS_POLL_TIMEOUT)
            .await
            .map_err(|error| {
                tracing::warn!(
                    task = TTS_TASK,
                    scheduler_error = error.kind().as_str(),
                    error = %error,
                    "Scheduler TTS synthesis failed"
                );
                let (status, body) = sanitized_tts_scheduler_failure(error.kind());
                AppError::detailed(status, body)
            })?;

    let audio = validate_speech_audio(
        &outputs,
        &expected_voice,
        &expected_language,
        &expected_format,
    )
    .map_err(|error| {
        tracing::warn!(
            task = TTS_TASK,
            validation_error = %error,
            "Scheduler returned invalid TTS audio output"
        );
        AppError::detailed(
            StatusCode::BAD_GATEWAY,
            serde_json::json!({
                "error": "invalid scheduler TTS output",
                "code": "invalid-tts-output",
                "task": TTS_TASK,
                "retryable": false
            }),
        )
    })?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "audio/wav")
        .header(header::CONTENT_LENGTH, audio.len().to_string())
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(audio))
        .map_err(|error| {
            tracing::error!(error = %error, "Failed to build TTS response");
            AppError::Internal("failed to build text-to-speech response".to_string())
        })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpeechAudioEnvelope {
    schema_version: String,
    task: String,
    status: String,
    language: String,
    voice: String,
    engine: String,
    degraded: bool,
    audio: SpeechAudioPayload,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpeechAudioPayload {
    encoding: String,
    data: String,
    mime_type: String,
    format: String,
    sample_rate_hz: u32,
    channels: u16,
    sample_format: String,
    duration_ms: u64,
    byte_length: usize,
    sha256: String,
}

fn validate_speech_audio(
    outputs: &serde_json::Value,
    expected_voice: &str,
    expected_language: &str,
    expected_format: &str,
) -> Result<Vec<u8>, String> {
    let public_size = serde_json::to_vec(outputs)
        .map_err(|error| format!("cannot measure scheduler output: {error}"))?
        .len();
    if public_size > MAX_PUBLIC_OUTPUT_BYTES {
        return Err(format!(
            "speech_audio.v1 envelope exceeds {MAX_PUBLIC_OUTPUT_BYTES} bytes"
        ));
    }
    let envelope: SpeechAudioEnvelope = serde_json::from_value(outputs.clone())
        .map_err(|error| format!("speech_audio.v1 schema mismatch: {error}"))?;
    if envelope.schema_version != TTS_SCHEMA_VERSION {
        return Err(format!(
            "schema_version must be {TTS_SCHEMA_VERSION}, got {}",
            envelope.schema_version
        ));
    }
    if envelope.task != TTS_TASK || envelope.status != "ok" {
        return Err("task/status invariant failed".to_string());
    }
    if envelope.engine != TTS_ENGINE || envelope.degraded {
        return Err("engine must be tts-default and degraded must be false".to_string());
    }
    validate_voice(&envelope.voice)?;
    validate_language(&envelope.language)?;
    if !matches!(expected_voice, "auto" | "default") && envelope.voice != expected_voice {
        return Err("resolved voice does not match the requested profile".to_string());
    }
    if expected_language != "auto" && envelope.language != expected_language {
        return Err("resolved language does not match the request".to_string());
    }

    let payload = envelope.audio;
    if payload.encoding != "base64"
        || payload.mime_type != "audio/wav"
        || payload.format != "wav"
        || payload.format != expected_format
        || payload.channels != 1
        || payload.sample_format != "pcm_s16le"
    {
        return Err("audio must be base64 audio/wav, wav, mono, pcm_s16le".to_string());
    }
    if payload.byte_length == 0 || payload.byte_length > MAX_AUDIO_BYTES {
        return Err(format!("audio byte_length must be 1..={MAX_AUDIO_BYTES}"));
    }
    if !(8_000..=48_000).contains(&payload.sample_rate_hz) {
        return Err("audio sample_rate_hz must be 8000..=48000".to_string());
    }
    if payload.duration_ms == 0 || payload.duration_ms > MAX_DURATION_MS {
        return Err(format!("audio duration_ms must be 1..={MAX_DURATION_MS}"));
    }
    let bytes = BASE64_STANDARD
        .decode(payload.data.as_bytes())
        .map_err(|error| format!("audio.data is not canonical base64: {error}"))?;
    if BASE64_STANDARD.encode(&bytes) != payload.data {
        return Err("audio.data is not canonical padded base64".to_string());
    }
    if bytes.len() != payload.byte_length {
        return Err("decoded byte length does not match audio.byte_length".to_string());
    }
    let digest = hex::encode(Sha256::digest(&bytes));
    if payload.sha256.len() != 64
        || !payload
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || digest != payload.sha256
    {
        return Err("audio.sha256 does not match decoded WAV bytes".to_string());
    }
    validate_pcm16_mono_wav(&bytes, payload.sample_rate_hz, payload.duration_ms)?;
    Ok(bytes)
}

fn validate_pcm16_mono_wav(
    bytes: &[u8],
    expected_sample_rate: u32,
    expected_duration_ms: u64,
) -> Result<(), String> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("audio.data is not a RIFF/WAVE file".to_string());
    }
    let riff_size = read_u32_le(bytes, 4)? as usize;
    if riff_size.checked_add(8) != Some(bytes.len()) {
        return Err("RIFF size does not match decoded byte length".to_string());
    }

    // Canonical contract deliberately excludes LIST/JUNK/extended fmt chunks:
    // fixed PCM fmt chunk at byte 12 and data immediately at byte 36.
    if &bytes[12..16] != b"fmt " || read_u32_le(bytes, 16)? != 16 || &bytes[36..40] != b"data" {
        return Err("WAV is not the canonical 44-byte-header PCM layout".to_string());
    }
    let audio_format = read_u16_le(bytes, 20)?;
    let channels = read_u16_le(bytes, 22)?;
    let sample_rate = read_u32_le(bytes, 24)?;
    let byte_rate = read_u32_le(bytes, 28)?;
    let block_align = read_u16_le(bytes, 32)?;
    let bits_per_sample = read_u16_le(bytes, 34)?;
    let data_len = read_u32_le(bytes, 40)? as usize;
    if data_len.checked_add(44) != Some(bytes.len()) {
        return Err("WAV data chunk length does not match decoded bytes".to_string());
    }
    if audio_format != 1
        || channels != 1
        || bits_per_sample != 16
        || block_align != 2
        || sample_rate != expected_sample_rate
        || sample_rate.checked_mul(2) != Some(byte_rate)
        || data_len == 0
        || data_len % 2 != 0
    {
        return Err("WAV fmt/data metadata is not PCM16 mono".to_string());
    }
    let sample_count = (data_len / 2) as u64;
    let sample_rate = u64::from(sample_rate);
    let computed_duration_ms = sample_count
        .checked_mul(1_000)
        .and_then(|numerator| numerator.checked_add(sample_rate / 2))
        .and_then(|rounded| rounded.checked_div(sample_rate))
        .ok_or_else(|| "invalid WAV sample rate".to_string())?;
    if computed_duration_ms != expected_duration_ms {
        return Err("WAV duration does not match audio.duration_ms".to_string());
    }
    Ok(())
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| "truncated WAV u16 field".to_string())?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "truncated WAV u32 field".to_string())?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}
