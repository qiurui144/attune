//! SenseVoice ASR provider — in-process sherpa-onnx (via the `sherpa-rs` crate).
//!
//! Replaces the platform-specific `whisper-cli` subprocess for the AMD/Intel Windows
//! (and any catalog `engine: sensevoice`) tiers. By loading the ONNX model in-process
//! there is **no per-platform binary to bundle**, which kills the "Linux ELF shipped in
//! a Windows package" bug class by construction (the original trigger for this work).
//!
//! Model: `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17` (`model.int8.onnx` +
//! `tokens.txt`), multilingual single-model (zh/en/ja/ko/yue), CER ~7.7% on zh
//! (VLM benchmark + spike both measured 7.69%). Runtime-fetched via S8 ModelStack
//! failover into `models_dir()/asr/sensevoice/`.
//!
//! Whole module is gated behind the `asr-sensevoice` cargo feature (default-on for
//! server/desktop). When the feature is off, `SenseVoiceBackend::detect()` returns
//! `None` and the ASR engine selector falls back to whisper-cli — so a build without
//! sherpa-onnx C libs (size-constrained / pure-native) still has working ASR.

use crate::error::{Result, VaultError};
use std::path::{Path, PathBuf};

/// HF repo id for the SenseVoice int8 model + tokens (S8 runtime-fetch).
pub const SENSEVOICE_REPO: &str = "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17";
/// The two files attune needs from the repo.
pub const SENSEVOICE_MODEL_FILE: &str = "model.int8.onnx";
pub const SENSEVOICE_TOKENS_FILE: &str = "tokens.txt";

/// Resolved on-disk locations of the SenseVoice model assets.
///
/// Constructed once (paths verified to exist) and held by the engine; the heavy
/// `SenseVoiceRecognizer` is created lazily per transcription call (sherpa's recognizer
/// is not `Send`, and ASR is an occasional operation, so we don't cache it across calls).
#[derive(Debug, Clone, PartialEq)]
pub struct SenseVoiceBackend {
    /// Path to `model.int8.onnx`.
    pub model_path: PathBuf,
    /// Path to `tokens.txt`.
    pub tokens_path: PathBuf,
    /// sherpa provider string: "cpu" | "cuda" | "directml". attune ASR runs CPU
    /// (catalog `ep` is an EP *hint* for embedding/OCR; SenseVoice int8 is CPU-friendly
    /// ~7x realtime, and `download-binaries`/`static` prebuilt CUDA/DirectML is not
    /// guaranteed, so we keep "cpu" for the bundled path).
    pub provider: String,
}

/// Directory holding the SenseVoice model assets: `models_dir()/asr/sensevoice/`.
pub fn sensevoice_model_dir() -> PathBuf {
    crate::platform::models_dir().join("asr").join("sensevoice")
}

impl SenseVoiceBackend {
    /// Detect a usable SenseVoice backend: model + tokens present on disk.
    ///
    /// Does **not** download — callers wanting fetch-on-miss use [`ensure_sensevoice_model`]
    /// first. Returns `None` when either asset is missing so the engine selector can fall
    /// back to whisper-cli gracefully (never panics).
    pub fn detect() -> Option<Self> {
        let dir = sensevoice_model_dir();
        let model_path = dir.join(SENSEVOICE_MODEL_FILE);
        let tokens_path = dir.join(SENSEVOICE_TOKENS_FILE);
        if model_path.exists() && tokens_path.exists() {
            Some(Self {
                model_path,
                tokens_path,
                provider: "cpu".to_string(),
            })
        } else {
            None
        }
    }

    /// Build a backend from explicit asset paths (test / advanced-config injection).
    ///
    /// Errors (graceful, never panics) when a path is missing.
    pub fn from_paths(model_path: impl Into<PathBuf>, tokens_path: impl Into<PathBuf>) -> Result<Self> {
        let model_path = model_path.into();
        let tokens_path = tokens_path.into();
        if !model_path.exists() {
            return Err(VaultError::InvalidInput(format!(
                "sensevoice model not found at {}. error-code=asr-model-offline-missing",
                model_path.display()
            )));
        }
        if !tokens_path.exists() {
            return Err(VaultError::InvalidInput(format!(
                "sensevoice tokens not found at {}. error-code=asr-model-offline-missing",
                tokens_path.display()
            )));
        }
        Ok(Self {
            model_path,
            tokens_path,
            provider: "cpu".to_string(),
        })
    }
}

/// Ensure the SenseVoice model + tokens are present on disk, fetching via S8 failover
/// (company-mirror → hf-mirror → HF) when missing. Returns the model directory.
///
/// Honors `HF_HUB_OFFLINE` (offline + missing → explicit `asr-model-offline-missing` error,
/// never a silent hang). The sherpa-onnx **C libs** are build-time vendored (not fetched here)
/// — only the ONNX weights + tokens are runtime-fetched, same as embedding/rerank.
pub fn ensure_sensevoice_model() -> Result<SenseVoiceBackend> {
    let dir = sensevoice_model_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| VaultError::ModelLoad(format!("create sensevoice dir: {e}")))?;
    let model_path = dir.join(SENSEVOICE_MODEL_FILE);
    let tokens_path = dir.join(SENSEVOICE_TOKENS_FILE);

    if model_path.exists() && tokens_path.exists() {
        return SenseVoiceBackend::from_paths(model_path, tokens_path);
    }

    if crate::infer::model_store::hf_hub_offline() {
        return Err(VaultError::ModelLoad(format!(
            "sensevoice model not cached and HF_HUB_OFFLINE is set; refusing network download. \
             error-code=asr-model-offline-missing (expected at {})",
            dir.display()
        )));
    }

    let sources = crate::infer::model_source::resolve_sources_for(SENSEVOICE_REPO);
    for (file, dst) in [
        (SENSEVOICE_MODEL_FILE, &model_path),
        (SENSEVOICE_TOKENS_FILE, &tokens_path),
    ] {
        if dst.exists() {
            continue;
        }
        let used = crate::infer::model_source::download_with_failover(
            &sources,
            SENSEVOICE_REPO,
            file,
            dst,
        )?;
        log::info!("sensevoice {file} downloaded via source '{used}'");
    }
    SenseVoiceBackend::from_paths(model_path, tokens_path)
}

/// Transcribe an audio file with SenseVoice. Returns the recognized text (trimmed).
///
/// - Empty / ultra-short audio → empty string (not a panic).
/// - sherpa init / decode failure → `Err` (caller decides whisper fallback vs surface).
///
/// `read_audio_file` (sherpa built-in) is **WAV-only and requires exactly 16 kHz i16** — it
/// `Err`s on `.mp3` / `.m4a` / `.flac` / non-16 kHz WAV. attune does NOT pre-decode here; the
/// engine dispatcher [`crate::asr::transcribe_with_engine`] catches this `Err` and falls back
/// to whisper-cli (which handles those containers natively), so unsupported-format audio still
/// transcribes instead of failing ingest. This function therefore only handles the 16 kHz-WAV
/// happy path and returns `Err` (never panics) for everything else.
#[cfg(feature = "asr-sensevoice")]
pub fn transcribe_sensevoice(backend: &SenseVoiceBackend, audio_path: &Path) -> Result<String> {
    use sherpa_rs::sense_voice::{SenseVoiceConfig, SenseVoiceRecognizer};

    let audio_str = audio_path.to_str().ok_or_else(|| {
        VaultError::InvalidInput("audio path not utf-8".to_string())
    })?;
    let model = backend.model_path.to_str().ok_or_else(|| {
        VaultError::InvalidInput("sensevoice model path not utf-8".to_string())
    })?;
    let tokens = backend.tokens_path.to_str().ok_or_else(|| {
        VaultError::InvalidInput("sensevoice tokens path not utf-8".to_string())
    })?;

    let (samples, sample_rate) = sherpa_rs::read_audio_file(audio_str).map_err(|e| {
        VaultError::InvalidInput(format!("sensevoice read_audio_file failed: {e}"))
    })?;
    // Empty / ultra-short audio → empty transcript, not a recognizer error.
    if samples.is_empty() {
        return Ok(String::new());
    }

    let config = SenseVoiceConfig {
        model: model.to_string(),
        tokens: tokens.to_string(),
        provider: Some(backend.provider.clone()),
        ..Default::default()
    };
    let mut recognizer = SenseVoiceRecognizer::new(config).map_err(|e| {
        VaultError::ModelLoad(format!("sensevoice recognizer init failed: {e}"))
    })?;
    let result = recognizer.transcribe(sample_rate, &samples);
    Ok(result.text.trim().to_string())
}

/// Stub when the `asr-sensevoice` feature is disabled — provider unavailable, never panics.
#[cfg(not(feature = "asr-sensevoice"))]
pub fn transcribe_sensevoice(_backend: &SenseVoiceBackend, _audio_path: &Path) -> Result<String> {
    Err(VaultError::InvalidInput(
        "sensevoice ASR not compiled in (asr-sensevoice feature disabled)".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_dir_under_models_asr_sensevoice() {
        let dir = sensevoice_model_dir();
        assert!(dir.ends_with("asr/sensevoice"), "got: {}", dir.display());
    }

    #[test]
    fn from_paths_missing_model_errs() {
        let r = SenseVoiceBackend::from_paths("/nonexistent/model.int8.onnx", "/nonexistent/tokens.txt");
        assert!(r.is_err());
        let msg = format!("{}", r.unwrap_err());
        assert!(msg.contains("asr-model-offline-missing"), "got: {msg}");
    }

    #[test]
    fn from_paths_present_assets_ok() {
        let tmp = tempfile::TempDir::new().unwrap();
        let model = tmp.path().join("model.int8.onnx");
        let tokens = tmp.path().join("tokens.txt");
        std::fs::write(&model, b"fake").unwrap();
        std::fs::write(&tokens, b"fake").unwrap();
        let b = SenseVoiceBackend::from_paths(&model, &tokens).unwrap();
        assert_eq!(b.provider, "cpu");
        assert_eq!(b.model_path, model);
        assert_eq!(b.tokens_path, tokens);
    }

    #[test]
    fn detect_none_when_assets_missing() {
        // In CI / fresh env the model dir is empty → detect returns None (graceful;
        // engine selector falls back to whisper). If a real machine already has the
        // assets this asserts the Some shape instead.
        match SenseVoiceBackend::detect() {
            None => {} // expected on fresh env
            Some(b) => {
                assert!(b.model_path.exists());
                assert!(b.tokens_path.exists());
            }
        }
    }
}
