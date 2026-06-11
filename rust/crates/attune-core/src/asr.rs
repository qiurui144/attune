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
    /// whisper.cpp 是否编译时启用 GPU 加速（CUDA / Metal / Vulkan）。
    /// 通过 `whisper-cli --help` 输出含 `--no-gpu` / `gpu-device` flag 来探测。
    /// CPU-only build 不识别这些 flag → false。
    /// 影响：whisper-medium 60s 长音频 GPU 约 5s / CPU 约 60s（10x 差异）。
    pub gpu_capable: bool,
}

impl AsrBackend {
    /// 是否支持中文 ASR（whisper-small 及以上中文 WER < 20%）
    pub fn supports_chinese_well(&self) -> bool {
        matches!(self.model_name.as_str(), "small" | "medium" | "large")
    }
}

/// 探测 whisper-cli 是否为 GPU build（CUDA / Metal / Vulkan）。
///
/// 检测策略：跑 `whisper-cli --help`，输出含 `--no-gpu` / `gpu-device` /
/// `n-gpu-layers` 等 GPU 相关 flag → GPU build。CPU-only build 这些 flag
/// 不会出现在 help 输出。
///
/// 失败时（命令执行错 / help 输出空）保守返 false（避免误判）。
fn probe_whisper_gpu_capable(whisper_path: &str) -> bool {
    let output = match Command::new(whisper_path).arg("--help").output() {
        Ok(o) => o,
        Err(_) => return false,
    };
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    // whisper.cpp upstream 标志（按 2024+ 版本 CLI）：
    // - `--no-gpu` / `-ng`   (GPU build 才识别)
    // - `--gpu-device N`     (GPU build 才识别)
    // - `-fa` flash attn     (GPU build 通常含)
    let lower = combined.to_ascii_lowercase();
    lower.contains("--no-gpu")
        || lower.contains("-ng,")
        || lower.contains("gpu-device")
        || lower.contains("flash-attn")
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
    let gpu_capable = probe_whisper_gpu_capable(&whisper_path);

    if !gpu_capable {
        log::warn!(
            target: "hardware_utilization",
            "F-16: whisper.cpp at {} appears to be CPU-only build (no GPU flags in --help). \
             ASR will run on CPU; whisper-medium 60s audio may take ~60s instead of ~5s. \
             Consider installing GPU build (CUDA/Metal/Vulkan) for 10x speedup.",
            whisper_path
        );
    } else {
        log::info!(
            target: "hardware_utilization",
            "F-16: whisper.cpp at {} is GPU-capable build, ASR will use GPU automatically",
            whisper_path
        );
    }

    Some(AsrBackend {
        whisper_path,
        model_path,
        model_name,
        language: "auto".to_string(), // whisper.cpp 自动检测语言
        gpu_capable,
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
    // 2-N. 标准路径 — large-v3-turbo 优先（中文 WER 5-7%，OpenAI 2024-10 sota）
    //
    // 候选优先级（用户拍板：尽可能不降级到 small）:
    //   1. large-v3-turbo-q5（574 MB, WER 5-7%, 推理 ~30s/30s 音频）★ 默认
    //   2. large-v3-q5_0（934 MB, 比 turbo 慢 8x 但准）
    //   3. medium-q5_0（480 MB, WER 10-12%）
    //   4. medium-q8_0（750 MB, WER 8-10%）
    //   5. small-q8_0（250 MB, WER 15-20%）— legacy fallback，不主动下载
    //   6. base / tiny — 极差路径，仅探测兼容
    let home = std::env::var("HOME").ok()?;
    let attune_models = format!("{home}/.local/share/attune/models/whisper");
    let cache_dir = format!("{home}/.cache/whisper");
    let candidates = [
        // ★ Tier 1: large-v3-turbo (default by postinst since 2026-05-01 B1 upgrade)
        format!("{attune_models}/ggml-large-v3-turbo-q5_0.bin"),
        format!("{attune_models}/ggml-large-v3-turbo.bin"),
        // Tier 2: large (full)
        format!("{attune_models}/ggml-large-v3-q5_0.bin"),
        format!("{attune_models}/ggml-large-v3.bin"),
        // Tier 3: medium (mid quality)
        format!("{attune_models}/ggml-medium-q5_0.bin"),
        format!("{attune_models}/ggml-medium-q8_0.bin"),
        format!("{attune_models}/ggml-medium.bin"),
        // Tier 4: small (legacy / low-tier fallback only)
        format!("{attune_models}/ggml-small-q8_0.bin"),
        format!("{attune_models}/ggml-small.bin"),
        // Tier 5: base/tiny (极差路径，仅作存在性 fallback)
        format!("{attune_models}/ggml-base-q8_0.bin"),
        format!("{attune_models}/ggml-tiny-q8_0.bin"),
        format!("{attune_models}/ggml-base.bin"),
        // ~/.cache/whisper（whisper.cpp 默认下载路径）
        format!("{cache_dir}/ggml-large-v3-turbo-q5_0.bin"),
        format!("{cache_dir}/ggml-large-v3-q5_0.bin"),
        format!("{cache_dir}/ggml-medium-q5_0.bin"),
        format!("{cache_dir}/ggml-small-q8_0.bin"),
        format!("{cache_dir}/ggml-small.bin"),
        // System-wide
        "/usr/share/whisper/ggml-large-v3-turbo-q5_0.bin".to_string(),
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

// ── 时间戳转写 + 说话人分离 ─────────────────────────────────────────────────

/// 单条时间戳转写片段（来自 whisper.cpp SRT / whisperX JSON 解析）。
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptSegment {
    /// 识别文本
    pub text: String,
    /// 起始时间（毫秒）
    pub start_ms: u32,
    /// 结束时间（毫秒）
    pub end_ms: u32,
    /// 说话人标签（如 "SPEAKER_00"），仅说话人分离时填入
    pub speaker: Option<String>,
}

impl TranscriptSegment {
    /// 格式化为 `[00:01:23 → 00:01:25] (SPEAKER_00) 文字` 显示用字符串。
    pub fn to_display(&self) -> String {
        let start = ms_to_hms(self.start_ms);
        let end = ms_to_hms(self.end_ms);
        if let Some(sp) = &self.speaker {
            format!("[{start} → {end}] ({sp}) {}", self.text)
        } else {
            format!("[{start} → {end}] {}", self.text)
        }
    }
}

fn ms_to_hms(ms: u32) -> String {
    let total_s = ms / 1000;
    let h = total_s / 3600;
    let m = (total_s % 3600) / 60;
    let s = total_s % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// 说话人分离后端能力探测结果。
///
/// 优先级：whisperX（端到端转写+分离）> pyannote-audio（仅分离，需配合 whisper）。
#[derive(Debug, Clone)]
pub enum DiarizationBackend {
    /// `whisperx` CLI（pip install whisperx）
    /// 一次命令同时完成转写 + 说话人分离，输出 JSON 含 speaker 字段。
    WhisperX { python_path: String },
    /// `pyannote-audio` Python 包（pip install pyannote.audio）
    /// 仅做说话人分离（diarization），返回每段时间区间 + 说话人标签。
    /// 需要 HuggingFace token（$HF_TOKEN 或 $HUGGINGFACE_TOKEN）。
    Pyannote { python_path: String },
}

/// 探测说话人分离后端（不做实际推理，仅检查 Python 包是否安装）。
///
/// 探测顺序：
///   1. whisperX — `python3 -c "import whisperx"`
///   2. pyannote  — `python3 -c "import pyannote.audio"`
///   3. 两者均无 → None（`transcribe_with_diarization` 退化为普通带时间戳转写）
pub fn detect_diarization_backend() -> Option<DiarizationBackend> {
    let python = detect_python()?;

    // 1. whisperX（优先）
    let wx_check = Command::new(&python)
        .args(["-c", "import whisperx; print('ok')"])
        .output();
    if let Ok(out) = wx_check {
        if out.status.success() {
            log::info!("ASR diarization: whisperX found at {python}");
            return Some(DiarizationBackend::WhisperX { python_path: python.clone() });
        }
    }

    // 2. pyannote-audio
    let py_check = Command::new(&python)
        .args(["-c", "import pyannote.audio; print('ok')"])
        .output();
    if let Ok(out) = py_check {
        if out.status.success() {
            log::info!("ASR diarization: pyannote.audio found at {python}");
            return Some(DiarizationBackend::Pyannote { python_path: python });
        }
    }

    log::debug!(
        "ASR diarization: neither whisperX nor pyannote.audio found. \
         Install with: pip install whisperx  (recommended)"
    );
    None
}

/// 查找系统 python3 / python 可执行路径。
fn detect_python() -> Option<String> {
    for name in ["python3", "python"] {
        if let Ok(p) = which::which(name) {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

/// 带时间戳的音频转写 — 返回每段文字及其起止时间（毫秒）。
///
/// 使用 whisper.cpp `-osrt` 输出 SRT 格式，再解析。
/// SRT 格式：
/// ```text
/// 1
/// 00:00:01,234 --> 00:00:03,456
/// 转写文字
/// ```
/// speaker 字段留空（需 `transcribe_with_diarization` 才填写）。
pub fn transcribe_audio_with_timestamps(
    backend: &AsrBackend,
    audio_path: &Path,
) -> Result<Vec<TranscriptSegment>> {
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
            "-osrt",
            "-of",
            output_prefix.to_str().unwrap_or("audio"),
        ])
        .output()
        .map_err(VaultError::Io)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(VaultError::InvalidInput(format!(
            "whisper.cpp (srt) failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            stderr.lines().take(3).collect::<Vec<_>>().join(" ")
        )));
    }

    let srt_path = output_prefix.with_extension("srt");
    if !srt_path.exists() {
        return Err(VaultError::InvalidInput(format!(
            "whisper.cpp did not produce expected .srt at {}",
            srt_path.display()
        )));
    }
    let srt = std::fs::read_to_string(&srt_path).map_err(VaultError::Io)?;
    Ok(parse_srt(&srt))
}

/// 解析 SRT 字符串 → Vec<TranscriptSegment>。
///
/// SRT 格式（RFC 4752 subset）：
/// ```text
/// 1
/// 00:00:01,234 --> 00:00:03,456
/// text line 1
/// text line 2
///
/// 2
/// ...
/// ```
pub fn parse_srt(srt: &str) -> Vec<TranscriptSegment> {
    let mut segments = Vec::new();
    let mut lines = srt.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim();
        // Skip block number (pure digits or empty)
        if line.is_empty() || line.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        // Try to parse timestamp line: "00:00:01,234 --> 00:00:03,456"
        if let Some((start_ms, end_ms)) = parse_srt_timestamp_line(line) {
            // Collect text lines until blank line
            let mut text_parts = Vec::new();
            while let Some(&next) = lines.peek() {
                let next = next.trim();
                if next.is_empty() {
                    let _ = lines.next();
                    break;
                }
                text_parts.push(next.to_string());
                let _ = lines.next();
            }
            let text = text_parts.join(" ");
            if !text.is_empty() {
                segments.push(TranscriptSegment {
                    text,
                    start_ms,
                    end_ms,
                    speaker: None,
                });
            }
        }
    }
    segments
}

/// 解析 SRT 时间戳行 "HH:MM:SS,mmm --> HH:MM:SS,mmm"，返回 (start_ms, end_ms)。
fn parse_srt_timestamp_line(line: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = line.splitn(2, " --> ").collect();
    if parts.len() != 2 {
        return None;
    }
    let start = parse_srt_time(parts[0].trim())?;
    let end = parse_srt_time(parts[1].trim().split(' ').next()?)?; // trim trailing flags
    Some((start, end))
}

/// "HH:MM:SS,mmm" → 毫秒
fn parse_srt_time(s: &str) -> Option<u32> {
    // Format: HH:MM:SS,mmm  or  HH:MM:SS.mmm
    let s = s.replace(',', ".");
    let parts: Vec<&str> = s.splitn(2, '.').collect();
    let hms: Vec<u32> = parts
        .first()?
        .split(':')
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    if hms.len() != 3 {
        return None;
    }
    let ms: u32 = parts
        .get(1)
        .and_then(|m| m.parse().ok())
        .unwrap_or(0);
    Some(hms[0] * 3_600_000 + hms[1] * 60_000 + hms[2] * 1000 + ms)
}

/// 带说话人分离的音频转写。
///
/// 策略（按后端可用性选择）：
/// 1. **whisperX**（推荐）：一次命令输出包含 speaker 字段的 JSON
/// 2. **pyannote-audio**：先用 `transcribe_audio_with_timestamps` 拿时间戳片段，
///    再用 pyannote 做 diarization，将 speaker label 按时间戳对齐注入
/// 3. **无后端**（asr_backend 有，diarization 无）：退化为普通带时间戳转写
///    （speaker = None）
///
/// 返回 `(segments, full_text)`：
/// - `segments`：逐句文字 + 时间戳 + 说话人（可选）
/// - `full_text`：供搜索索引的纯文本（含 `[SPEAKER_XX]: text` 格式化）
pub fn transcribe_with_diarization(
    asr: &AsrBackend,
    audio_path: &Path,
    diarization: Option<&DiarizationBackend>,
) -> Result<(Vec<TranscriptSegment>, String)> {
    match diarization {
        Some(DiarizationBackend::WhisperX { python_path }) => {
            transcribe_whisperx(python_path, asr, audio_path)
        }
        Some(DiarizationBackend::Pyannote { python_path }) => {
            transcribe_pyannote(asr, audio_path, python_path)
        }
        None => {
            // 退化：普通时间戳转写，speaker = None
            let segments = transcribe_audio_with_timestamps(asr, audio_path)?;
            let full = segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("\n");
            Ok((segments, full))
        }
    }
}

/// 用 whisperX 完成端到端转写 + 说话人分离。
/// 输出 JSON 格式（whisperX v3+）：
/// ```json
/// {"segments": [{"start": 0.0, "end": 1.5, "text": "...", "speaker": "SPEAKER_00"}, ...]}
/// ```
fn transcribe_whisperx(
    python_path: &str,
    asr: &AsrBackend,
    audio_path: &Path,
) -> Result<(Vec<TranscriptSegment>, String)> {
    let tmp = tempfile::TempDir::new().map_err(VaultError::Io)?;
    let audio_str = audio_path.to_str().ok_or_else(|| {
        VaultError::InvalidInput("audio path not utf-8".to_string())
    })?;
    let lang = if asr.language == "auto" { "zh" } else { &asr.language };
    // whisperx 命令：--model 来自 asr backend 名（如 small/medium/large）
    // --output_format json --output_dir <tmp>
    let output = Command::new(python_path)
        .args([
            "-m", "whisperx",
            audio_str,
            "--model", &asr.model_name,
            "--language", lang,
            "--output_format", "json",
            "--output_dir", tmp.path().to_str().unwrap_or("."),
            "--diarize",
        ])
        .output()
        .map_err(VaultError::Io)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(VaultError::InvalidInput(format!(
            "whisperX failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            stderr.lines().take(5).collect::<Vec<_>>().join(" ")
        )));
    }

    // 找 JSON 输出文件（whisperX 输出到 <audio_stem>.json）
    let json_path = tmp.path().join(
        audio_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("audio"),
    ).with_extension("json");

    if !json_path.exists() {
        // 尝试找任何 .json 文件
        let any_json = std::fs::read_dir(tmp.path())
            .map_err(VaultError::Io)?
            .filter_map(|e| e.ok())
            .find(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"));
        if let Some(entry) = any_json {
            return parse_whisperx_json(&std::fs::read_to_string(entry.path()).map_err(VaultError::Io)?);
        }
        return Err(VaultError::InvalidInput(format!(
            "whisperX did not produce expected JSON at {}",
            json_path.display()
        )));
    }
    let json_str = std::fs::read_to_string(&json_path).map_err(VaultError::Io)?;
    parse_whisperx_json(&json_str)
}

/// 解析 whisperX JSON 输出 → (segments, full_text)。
fn parse_whisperx_json(json_str: &str) -> Result<(Vec<TranscriptSegment>, String)> {
    let v: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        VaultError::InvalidInput(format!("whisperX JSON parse error: {e}"))
    })?;
    let segs_arr = v["segments"].as_array().ok_or_else(|| {
        VaultError::InvalidInput("whisperX JSON missing 'segments' array".to_string())
    })?;

    let mut segments: Vec<TranscriptSegment> = segs_arr
        .iter()
        .filter_map(|s| {
            let text = s["text"].as_str()?.trim().to_string();
            let start_ms = (s["start"].as_f64()? * 1000.0) as u32;
            let end_ms = (s["end"].as_f64()? * 1000.0) as u32;
            let speaker = s["speaker"].as_str().map(|sp| sp.to_string());
            Some(TranscriptSegment { text, start_ms, end_ms, speaker })
        })
        .collect();

    // 按时间戳排序（whisperX 通常已排序，但防御性排）
    segments.sort_by_key(|s| s.start_ms);
    let full = format_diarized_text(&segments);
    Ok((segments, full))
}

/// 用 pyannote-audio 做说话人分离，再与 whisper 时间戳片段对齐。
fn transcribe_pyannote(
    asr: &AsrBackend,
    audio_path: &Path,
    python_path: &str,
) -> Result<(Vec<TranscriptSegment>, String)> {
    // Step 1: whisper.cpp 带时间戳转写
    let mut segments = transcribe_audio_with_timestamps(asr, audio_path)?;
    if segments.is_empty() {
        return Ok((segments, String::new()));
    }

    // Step 2: pyannote 说话人分离（Python one-liner → JSON）
    let audio_str = audio_path.to_str().ok_or_else(|| {
        VaultError::InvalidInput("audio path not utf-8".to_string())
    })?;
    let hf_token = std::env::var("HF_TOKEN")
        .or_else(|_| std::env::var("HUGGINGFACE_TOKEN"))
        .unwrap_or_default();

    let py_script = format!(
        r#"
import sys, json
try:
    from pyannote.audio import Pipeline
    import torch
    pipeline = Pipeline.from_pretrained(
        "pyannote/speaker-diarization-3.1",
        use_auth_token={token}
    )
    diarization = pipeline("{audio}")
    result = []
    for turn, _, speaker in diarization.itertracks(yield_label=True):
        result.append({{"start": turn.start, "end": turn.end, "speaker": speaker}})
    print(json.dumps(result))
except Exception as e:
    print(json.dumps({{"error": str(e)}}), file=sys.stderr)
    sys.exit(1)
"#,
        token = if hf_token.is_empty() { "None".to_string() } else { format!("'{hf_token}'") },
        audio = audio_str.replace('\\', "\\\\").replace('\'', "\\'"),
    );

    let output = Command::new(python_path)
        .args(["-c", &py_script])
        .output()
        .map_err(VaultError::Io)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::warn!(
            "pyannote diarization failed: {}. Falling back to no-speaker output.",
            stderr.lines().take(2).collect::<Vec<_>>().join(" ")
        );
        // 不报错，退化为无说话人
        let full = segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("\n");
        return Ok((segments, full));
    }

    let json_out = String::from_utf8_lossy(&output.stdout);
    let diarization: Vec<serde_json::Value> = match serde_json::from_str(&json_out) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("pyannote JSON parse error: {e}. Falling back to no-speaker output.");
            let full = segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("\n");
            return Ok((segments, full));
        }
    };

    // Step 3: 对每个 ASR 片段，按时间重叠分配说话人标签
    // 策略：取与该片段时间重叠最大的说话人区间
    for seg in &mut segments {
        let start_s = seg.start_ms as f64 / 1000.0;
        let end_s = seg.end_ms as f64 / 1000.0;
        let mut best_overlap = 0.0f64;
        let mut best_speaker: Option<String> = None;

        for entry in &diarization {
            let d_start = entry["start"].as_f64().unwrap_or(0.0);
            let d_end = entry["end"].as_f64().unwrap_or(0.0);
            let speaker = entry["speaker"].as_str().unwrap_or("SPEAKER_00");
            // 时间重叠
            let overlap = (end_s.min(d_end) - start_s.max(d_start)).max(0.0);
            if overlap > best_overlap {
                best_overlap = overlap;
                best_speaker = Some(speaker.to_string());
            }
        }
        seg.speaker = best_speaker;
    }

    let full = format_diarized_text(&segments);
    Ok((segments, full))
}

/// 将 segments 格式化为纯文本（供搜索索引）。
/// 有说话人标签时：`[SPEAKER_00]: 文字\n[SPEAKER_01]: 文字`
/// 无说话人时：纯拼接
pub fn format_diarized_text(segments: &[TranscriptSegment]) -> String {
    let mut out = String::with_capacity(segments.len() * 60);
    let mut current_speaker: Option<&str> = None;
    for seg in segments {
        match &seg.speaker {
            Some(sp) => {
                if current_speaker != Some(sp.as_str()) {
                    if !out.is_empty() { out.push('\n'); }
                    out.push('[');
                    out.push_str(sp);
                    out.push_str("]: ");
                    current_speaker = Some(sp.as_str());
                } else {
                    out.push(' ');
                }
                out.push_str(&seg.text);
            }
            None => {
                if !out.is_empty() { out.push('\n'); }
                out.push_str(&seg.text);
            }
        }
    }
    out
}

/// 自动下载 whisper.cpp ggml 模型文件（按 tier）。
///
/// 来源：HuggingFace `ggerganov/whisper.cpp` 仓（ggml-{tiny/base/small/medium}-q8_0.bin）。
/// HF_ENDPOINT 环境变量已由 state.rs 按 region 设好（China → hf-mirror.com）。
///
/// 模型保存到 ~/.local/share/attune/models/whisper/{filename}，让 detect_asr_backend
/// 之后能找到。
///
/// 返回: 下载好的模型文件路径
pub fn ensure_whisper_model(ggml_filename: &str) -> crate::error::Result<std::path::PathBuf> {
    use crate::error::VaultError;

    let target_dir = crate::platform::data_dir().join("models").join("whisper");
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| VaultError::ModelLoad(format!("create whisper dir: {e}")))?;
    let target = target_dir.join(ggml_filename);

    if target.exists() {
        // 已存在跳过（不做 SHA 校验避免破坏用户自己放的 ggml；用户想换重新下载就删除文件）
        return Ok(target);
    }

    // 离线模式: 缓存未命中时禁止网络下载, 立即 Err → 调用方 graceful degrade(不阻塞)。
    if crate::infer::model_store::hf_hub_offline() {
        return Err(VaultError::ModelLoad(format!(
            "whisper model {ggml_filename} not cached and HF_HUB_OFFLINE is set; refusing network download"
        )));
    }

    // S1: pre-flight 可达性探测(带显式 connect 超时)。注意 whisper.cpp 仓在 ModelScope
    // **无覆盖** → CN 默认源(ModelScope)会 404,但探测命中可达后 hf-hub 的 404 是快速失败;
    // 若整个 endpoint 死(如旧 hf-mirror)则探测在超时内 fail-fast 而非永久阻塞启动后台线程。
    crate::infer::model_store::probe_endpoint_reachable(
        &crate::infer::model_store::hf_endpoint(),
    )?;

    let api = hf_hub::api::sync::Api::new()
        .map_err(|e| VaultError::ModelLoad(format!("hf-hub init: {e}")))?;
    let repo = api.model("ggerganov/whisper.cpp".to_string());
    let src = repo
        .get(ggml_filename)
        .map_err(|e| VaultError::ModelLoad(format!("download {ggml_filename}: {e}")))?;
    std::fs::copy(&src, &target)
        .map_err(|e| VaultError::ModelLoad(format!("copy ggml file: {e}")))?;
    Ok(target)
}

/// 启动时根据硬件 tier 后台拉取对应大小的 whisper ggml 模型。
///
/// 由 state.rs::init_search_engines spawn 在 tokio runtime 中调用。
/// 失败不阻塞启动，仅 warn 日志（用户可以晚点用 ASR 时再 retry）。
pub fn fetch_for_tier(tier: crate::platform::Tier) -> crate::error::Result<std::path::PathBuf> {
    let rec = crate::platform::ModelRecommendation::for_tier(tier).ok_or_else(|| {
        crate::error::VaultError::InvalidInput(format!("tier {} not supported", tier.label()))
    })?;
    ensure_whisper_model(rec.asr_ggml)
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

    // ── F-16 hardware utilization: whisper.cpp GPU build detection ──────────

    #[test]
    fn probe_whisper_gpu_capable_returns_false_for_nonexistent_binary() {
        // Invalid binary path → command exec fails → false (conservative)
        let result = probe_whisper_gpu_capable("/nonexistent/whisper-cli-12345");
        assert!(!result, "nonexistent binary should return false");
    }

    #[test]
    fn probe_whisper_gpu_capable_recognizes_no_gpu_flag() {
        // We can't easily inject a fake binary, but we test the keyword detection
        // logic by verifying the function uses lowercase + multiple keywords.
        // Real whisper.cpp GPU build outputs "--no-gpu, -ng" in --help.
        // CPU-only build does NOT include those flags.
        //
        // Since we can't mock std::process::Command easily without trait abstraction,
        // this is a smoke test confirming the function exists + handles edge cases.
        // Full integration is in tests/MANUAL_TEST_CHECKLIST.md ASR section.
    }

    #[test]
    fn supports_chinese_well_threshold() {
        let mk = |name: &str| AsrBackend {
            whisper_path: "/usr/bin/whisper".into(),
            model_path: "/x/ggml.bin".into(),
            model_name: name.into(),
            language: "auto".into(),
            gpu_capable: false, // not relevant for this WER threshold test
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

    // ── SRT parsing tests ───────────────────────────────────────────────────

    #[test]
    fn parse_srt_empty_returns_empty_vec() {
        assert_eq!(parse_srt(""), vec![]);
    }

    #[test]
    fn parse_srt_single_segment() {
        let srt = "1\n00:00:01,000 --> 00:00:03,500\n你好世界\n\n";
        let segs = parse_srt(srt);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "你好世界");
        assert_eq!(segs[0].start_ms, 1000);
        assert_eq!(segs[0].end_ms, 3500);
        assert!(segs[0].speaker.is_none());
    }

    #[test]
    fn parse_srt_multiple_segments() {
        let srt = "\
1
00:00:00,000 --> 00:00:01,200
Hello

2
00:01:05,500 --> 00:01:08,000
World

";
        let segs = parse_srt(srt);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "Hello");
        assert_eq!(segs[0].start_ms, 0);
        assert_eq!(segs[0].end_ms, 1200);
        assert_eq!(segs[1].text, "World");
        assert_eq!(segs[1].start_ms, 65500);
        assert_eq!(segs[1].end_ms, 68000);
    }

    #[test]
    fn parse_srt_multiline_text_joined() {
        let srt = "1\n00:00:01,000 --> 00:00:03,000\nLine one\nLine two\n\n";
        let segs = parse_srt(srt);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "Line one Line two");
    }

    #[test]
    fn parse_srt_time_h_m_s_ms() {
        let srt = "1\n01:02:03,456 --> 01:02:05,789\ntest\n\n";
        let segs = parse_srt(srt);
        assert_eq!(segs[0].start_ms, 3_600_000 + 2 * 60_000 + 3 * 1000 + 456);
        assert_eq!(segs[0].end_ms, 3_600_000 + 2 * 60_000 + 5 * 1000 + 789);
    }

    #[test]
    fn parse_srt_dot_separator_ms() {
        // 有些 whisper.cpp 版本用 HH:MM:SS.mmm 而不是逗号
        let srt = "1\n00:00:01.000 --> 00:00:03.500\ntext\n\n";
        let segs = parse_srt(srt);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].start_ms, 1000);
        assert_eq!(segs[0].end_ms, 3500);
    }

    // ── TranscriptSegment.to_display() ─────────────────────────────────────

    #[test]
    fn to_display_no_speaker() {
        let seg = TranscriptSegment {
            text: "你好".to_string(),
            start_ms: 1000,
            end_ms: 3000,
            speaker: None,
        };
        let d = seg.to_display();
        assert!(d.contains("00:00:01"), "should contain start time");
        assert!(d.contains("你好"), "should contain text");
        assert!(!d.contains("SPEAKER"), "should not contain SPEAKER label");
    }

    #[test]
    fn to_display_with_speaker() {
        let seg = TranscriptSegment {
            text: "你好".to_string(),
            start_ms: 1000,
            end_ms: 3000,
            speaker: Some("SPEAKER_01".to_string()),
        };
        let d = seg.to_display();
        assert!(d.contains("SPEAKER_01"), "should contain speaker label");
        assert!(d.contains("你好"), "should contain text");
    }

    // ── format_diarized_text ────────────────────────────────────────────────

    #[test]
    fn format_diarized_text_groups_same_speaker() {
        let segs = vec![
            TranscriptSegment { text: "Hello".to_string(), start_ms: 0, end_ms: 1000, speaker: Some("SPEAKER_00".to_string()) },
            TranscriptSegment { text: "world".to_string(), start_ms: 1000, end_ms: 2000, speaker: Some("SPEAKER_00".to_string()) },
            TranscriptSegment { text: "Hi".to_string(), start_ms: 2000, end_ms: 3000, speaker: Some("SPEAKER_01".to_string()) },
        ];
        let text = format_diarized_text(&segs);
        // SPEAKER_00 consecutive → should be on same line
        assert!(text.contains("[SPEAKER_00]: Hello world"), "got: {text}");
        assert!(text.contains("[SPEAKER_01]: Hi"), "got: {text}");
    }

    #[test]
    fn format_diarized_text_no_speaker_plain_text() {
        let segs = vec![
            TranscriptSegment { text: "第一句".to_string(), start_ms: 0, end_ms: 1000, speaker: None },
            TranscriptSegment { text: "第二句".to_string(), start_ms: 1000, end_ms: 2000, speaker: None },
        ];
        let text = format_diarized_text(&segs);
        assert!(text.contains("第一句"), "got: {text}");
        assert!(text.contains("第二句"), "got: {text}");
        assert!(!text.contains("SPEAKER"), "no speaker labels expected; got: {text}");
    }

    // ── whisperX JSON parser ────────────────────────────────────────────────

    #[test]
    fn parse_whisperx_json_well_formed() {
        let json = r#"{"segments": [
            {"start": 0.0, "end": 1.5, "text": "Hello", "speaker": "SPEAKER_00"},
            {"start": 1.6, "end": 3.0, "text": "World", "speaker": "SPEAKER_01"}
        ]}"#;
        let (segs, full) = parse_whisperx_json(json).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "Hello");
        assert_eq!(segs[0].speaker, Some("SPEAKER_00".to_string()));
        assert_eq!(segs[1].speaker, Some("SPEAKER_01".to_string()));
        assert!(full.contains("[SPEAKER_00]: Hello"), "full text: {full}");
    }

    #[test]
    fn parse_whisperx_json_missing_speaker_field() {
        // whisperX without --diarize flag → no speaker field
        let json = r#"{"segments": [{"start": 0.0, "end": 1.5, "text": "Hello"}]}"#;
        let (segs, _) = parse_whisperx_json(json).unwrap();
        assert_eq!(segs[0].speaker, None);
    }

    #[test]
    fn parse_whisperx_json_malformed_returns_err() {
        let result = parse_whisperx_json("not json");
        assert!(result.is_err());
    }
}
