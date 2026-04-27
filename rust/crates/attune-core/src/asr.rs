//! ASR (Automatic Speech Recognition) backend — whisper.cpp subprocess。
//!
//! Design philosophy: 与 ocr.rs 完全一致 — 复用系统已装的 whisper.cpp CLI / pip 包，
//! 不引入 C/C++ FFI 依赖（避免交叉编译时的 libwhisper 版本地狱）。
//!
//! - 跨平台成熟：whisper.cpp Mac/Linux/Windows 都有官方二进制
//! - 中文 WER 满足：whisper-small Q8 实测 < 20%（per CLAUDE.md ASR 决策）
//!   - whisper-tiny WER 35-40% 不达标，仅在内存极小 (<8GB) 提示用户
//! - 偶发操作：用户不天天 ingest 音频，启动子进程可接受
//!
//! 音频文件 (mp3/wav/m4a/etc) → whisper.cpp main → 文字 + (可选) 时间戳
//!
//! 可选择性使用：若 `detect_asr_backend()` 返回 None，parser.rs 自动跳过音频文件
//! 入库（不报错，仅记 warn）。

use crate::error::{Result, VaultError};
use std::path::Path;
use std::process::Command;

/// ASR backend 能力探测结果
#[derive(Debug, Clone)]
pub struct AsrBackend {
    pub whisper_path: String,
    pub model_path: String, // 已下载的 ggml model file (e.g. ggml-small.bin)
    pub model_name: String, // tiny / base / small / medium / large
    pub language: String,   // "auto" / "zh" / "en" 等
}

impl AsrBackend {
    /// 是否支持中文 ASR（whisper-small 及以上中文 WER < 20%）
    pub fn supports_chinese_well(&self) -> bool {
        matches!(self.model_name.as_str(), "small" | "medium" | "large")
    }
}

/// 探测系统是否装了 whisper.cpp + 可用的 ggml 模型
///
/// 查找顺序（per CLAUDE.md "ASR 引擎" 决策）：
/// 1. PATH 中的 `whisper` 或 `whisper-cli` 或 `main` (whisper.cpp 二进制)
/// 2. 常见模型路径（按优先级）：
///    - $ATTUNE_WHISPER_MODEL (用户自定义)
///    - ~/.local/share/attune/models/whisper/ggml-small.bin
///    - ~/.cache/whisper/ggml-small.bin
///    - /usr/share/whisper/ggml-small.bin
/// 3. 找不到 → None（parser.rs 跳过音频文件）
pub fn detect_asr_backend() -> Option<AsrBackend> {
    let whisper_path = which_bin("whisper-cli")
        .or_else(|| which_bin("whisper"))
        .or_else(|| which_bin("main"))?;
    let (model_path, model_name) = find_default_model()?;
    Some(AsrBackend {
        whisper_path,
        model_path,
        model_name,
        language: "auto".to_string(), // whisper.cpp 自动检测语言
    })
}

fn which_bin(name: &str) -> Option<String> {
    which::which(name)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// 查找默认 ggml 模型文件，返回 (path, model_name)
fn find_default_model() -> Option<(String, String)> {
    // 1. 用户显式指定
    if let Ok(env_path) = std::env::var("ATTUNE_WHISPER_MODEL") {
        if std::path::Path::new(&env_path).exists() {
            let name = extract_model_name(&env_path);
            return Some((env_path, name));
        }
    }
    // 2-N. 标准路径，优先 small（中文 WER 满足）
    let home = std::env::var("HOME").ok()?;
    let candidates = [
        format!("{home}/.local/share/attune/models/whisper/ggml-small.bin"),
        format!("{home}/.local/share/attune/models/whisper/ggml-medium.bin"),
        format!("{home}/.local/share/attune/models/whisper/ggml-base.bin"),
        format!("{home}/.cache/whisper/ggml-small.bin"),
        format!("{home}/.cache/whisper/ggml-base.bin"),
        "/usr/share/whisper/ggml-small.bin".to_string(),
    ];
    for path in &candidates {
        if std::path::Path::new(path).exists() {
            let name = extract_model_name(path);
            return Some((path.clone(), name));
        }
    }
    None
}

fn extract_model_name(path: &str) -> String {
    // ggml-small.bin → small；ggml-small-q8.bin → small；ggml-large-v3.bin → large
    let stem = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let after_ggml = stem.strip_prefix("ggml-").unwrap_or(stem);
    let name = after_ggml.split(['-', '.']).next().unwrap_or("unknown");
    name.to_string()
}

/// 音频文件 → 文字（ASR 转写）
///
/// 流程：
///   1. 调 whisper.cpp main：whisper-cli -m <model> -f <audio> -l <lang> -otxt
///   2. whisper.cpp 输出 .txt 同名文件，读取返回文本
///
/// 设计注意：
///   - 大音频（如 1 小时 mp3）耗时可能 5-30 min（CPU 推理），调用方应 spawn_blocking
///   - 临时输出走 audio 同目录或 tempdir
///   - 失败：返 Err，调用方决定 fall back 还是上报
pub fn transcribe_audio(backend: &AsrBackend, audio_path: &Path) -> Result<String> {
    let tmp = tempfile::TempDir::new().map_err(VaultError::Io)?;
    let output_prefix = tmp.path().join(
        audio_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("audio"),
    );

    let lang_arg = if backend.language == "auto" { "auto" } else { &backend.language };
    let output = Command::new(&backend.whisper_path)
        .args([
            "-m",
            &backend.model_path,
            "-f",
            audio_path.to_str().ok_or_else(|| {
                VaultError::InvalidInput("audio path not utf-8".to_string())
            })?,
            "-l",
            lang_arg,
            "-otxt",
            "-of",
            output_prefix.to_str().unwrap_or("audio"),
            "-nt", // no timestamps in output text (干净文本)
        ])
        .output()
        .map_err(VaultError::Io)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(VaultError::InvalidInput(format!(
            "whisper.cpp failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            stderr.lines().take(3).collect::<Vec<_>>().join(" ")
        )));
    }

    // whisper.cpp 输出 .txt 文件
    let txt_path = output_prefix.with_extension("txt");
    if !txt_path.exists() {
        return Err(VaultError::InvalidInput(format!(
            "whisper.cpp did not produce expected .txt at {}",
            txt_path.display()
        )));
    }
    let text = std::fs::read_to_string(&txt_path).map_err(VaultError::Io)?;
    Ok(text.trim().to_string())
}

/// 探测当前系统是否能跑 ASR（不实际转写，仅检查依赖）
pub fn is_available() -> bool {
    detect_asr_backend().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_model_name_basic() {
        assert_eq!(extract_model_name("/x/ggml-small.bin"), "small");
        assert_eq!(extract_model_name("/x/ggml-large-v3.bin"), "large");
        assert_eq!(extract_model_name("/x/ggml-base.bin"), "base");
        assert_eq!(extract_model_name("/x/ggml-tiny-q8.bin"), "tiny");
    }

    #[test]
    fn supports_chinese_well_threshold() {
        let mk = |name: &str| AsrBackend {
            whisper_path: "/usr/bin/whisper".into(),
            model_path: "/x/ggml.bin".into(),
            model_name: name.into(),
            language: "auto".into(),
        };
        assert!(!mk("tiny").supports_chinese_well(), "tiny WER 35-40% 不达标");
        assert!(!mk("base").supports_chinese_well(), "base WER 25-30% 不达标");
        assert!(mk("small").supports_chinese_well(), "small Q8 中文 WER < 20%");
        assert!(mk("medium").supports_chinese_well());
        assert!(mk("large").supports_chinese_well());
    }

    #[test]
    fn detect_returns_none_when_whisper_not_in_path() {
        // 在 CI / 大多数本地机器上 whisper.cpp 未装 → None
        // (装了的情况下这个测试会被跳过)
        if which::which("whisper-cli").is_err()
            && which::which("whisper").is_err()
            && which::which("main").is_err()
        {
            assert!(detect_asr_backend().is_none());
            assert!(!is_available());
        }
    }
}
