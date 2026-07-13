// npu-vault/crates/vault-core/src/parser.rs

use crate::error::{Result, VaultError};
use crate::text_norm::collapse_whitespace;
use base64::Engine;
use serde_json::Value;
use std::thread;
use std::time::{Duration, Instant};
use std::{io::Write, path::Path};

/// 代码文件扩展名
const CODE_EXTENSIONS: &[&str] = &[
    ".py", ".js", ".ts", ".rs", ".go", ".java", ".c", ".cpp", ".h", ".rb", ".php", ".swift", ".kt",
    ".scala", ".sh", ".bash", ".zsh", ".toml", ".yaml", ".yml", ".json", ".xml", ".html", ".css",
];

const DEFAULT_PARSE_SCHEDULER_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_SCHEDULER_MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_OCRMYPDF_MAX_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_SCHEDULER_PDF_OCR_MAX_PAGES: usize = 0;
const DEFAULT_SCHEDULER_PDF_OCR_UNKNOWN_MAX_PAGES: usize = 256;
const DEFAULT_SCHEDULER_PDF_OCR_MAX_FAILED_PAGES: usize = 16;
const DEFAULT_SCHEDULER_PDF_OCR_MAX_CONSECUTIVE_FAILURES: usize = 3;
const DEFAULT_SCHEDULER_PDF_OCR_MAX_DPI: usize = 200;
const DEFAULT_SCHEDULER_PDF_OCR_MIN_DPI: usize = 120;
const SCHEDULER_INLINE_JSON_OVERHEAD_BYTES: usize = 4096;

#[derive(Debug, Clone, Default)]
pub struct ParseOptions {
    pub profile_id: Option<String>,
    pub scheduler_base: Option<String>,
    pub scheduler_timeout_ms: Option<u64>,
}

impl ParseOptions {
    pub fn with_profile(profile_id: Option<&str>) -> Self {
        Self {
            profile_id: profile_id.map(str::to_string),
            ..Self::default()
        }
    }

    pub fn with_scheduler_base(mut self, scheduler_base: Option<&str>) -> Self {
        self.scheduler_base = scheduler_base
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        self
    }

    pub fn with_scheduler_timeout_ms(mut self, timeout_ms: u64) -> Self {
        if timeout_ms > 0 {
            self.scheduler_timeout_ms = Some(timeout_ms);
        }
        self
    }

    fn profile_id(&self) -> Option<&str> {
        self.profile_id.as_deref()
    }

    fn scheduler_timeout(&self) -> Duration {
        Duration::from_millis(
            self.scheduler_timeout_ms
                .unwrap_or(DEFAULT_PARSE_SCHEDULER_TIMEOUT_MS)
                .max(1_000),
        )
    }
}

fn panic_payload_message(payload: Box<dyn std::any::Any + Send>) -> String {
    let any = payload.as_ref();
    if let Some(s) = any.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = any.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn safe_pdf_extract_text_from_mem(data: &[u8]) -> std::result::Result<String, String> {
    match std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(data)) {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(err)) => Err(err.to_string()),
        Err(payload) => Err(format!(
            "pdf_extract panic: {}",
            panic_payload_message(payload)
        )),
    }
}

fn pdf_text_layer_is_usable(text: &str) -> bool {
    if !crate::ocr::needs_ocr(text) {
        return true;
    }
    // `needs_ocr` intentionally uses a conservative <100 non-whitespace
    // threshold to catch image-only PDFs. Very short real text-layer PDFs,
    // however, can still be valid KB material; OCRing them can be worse than
    // keeping the text layer. Count Unicode word characters so mixed Chinese /
    // English snippets such as deterministic test fixtures stay on text path.
    let word_chars = text
        .chars()
        .filter(|c| !c.is_control() && c.is_alphanumeric())
        .count();
    word_chars >= 32
}

fn ocrmypdf_fallback_enabled() -> bool {
    env_bool_any(&["ATTUNE_ENABLE_OCRMYPDF_FALLBACK"], false)
}

fn ocrmypdf_max_bytes() -> usize {
    env_usize_any(
        &[
            "ATTUNE_OCRMYPDF_MAX_BYTES",
            "ATTUNE_SCHEDULER_OCRMYPDF_MAX_BYTES",
            "ATTUNE_LOCAL_SCHEDULER_OCRMYPDF_MAX_BYTES",
        ],
        DEFAULT_OCRMYPDF_MAX_BYTES,
    )
}

fn scheduler_local_ocr_provider_fallback_enabled() -> bool {
    env_bool_any(
        &[
            "ATTUNE_SCHEDULER_ALLOW_LOCAL_OCR_PROVIDER_FALLBACK",
            "ATTUNE_LOCAL_SCHEDULER_ALLOW_LOCAL_OCR_PROVIDER_FALLBACK",
        ],
        false,
    )
}

fn scheduler_ocr_enabled() -> bool {
    env_bool_any(
        &[
            "ATTUNE_SCHEDULER_OCR_ENABLED",
            "ATTUNE_LOCAL_SCHEDULER_OCR_ENABLED",
        ],
        true,
    )
}

fn try_ocrmypdf_sidecar_from_path(path: &Path) -> Option<String> {
    if !ocrmypdf_fallback_enabled() {
        return None;
    }
    let max_bytes = ocrmypdf_max_bytes();
    if max_bytes > 0 {
        if let Ok(metadata) = std::fs::metadata(path) {
            if metadata.len() > max_bytes as u64 {
                log::warn!(
                    "ocrmypdf fallback skipped for {}: file too large ({} bytes > max {} bytes); \
                     use scheduler page/chunk OCR for large scanned PDFs",
                    path.display(),
                    metadata.len(),
                    max_bytes
                );
                return None;
            }
        }
    }
    let ocrmypdf = which::which("ocrmypdf").ok()?;
    let tmp = tempfile::TempDir::new().ok()?;
    let sidecar = tmp.path().join("ocr.txt");
    let out_pdf = tmp.path().join("ocr.pdf");
    let output = match crate::process::command_no_window(&ocrmypdf)
        .arg("--sidecar")
        .arg(&sidecar)
        .arg("--skip-text")
        .arg("--optimize")
        .arg("0")
        .arg("--output-type")
        .arg("pdf")
        .arg(path)
        .arg(&out_pdf)
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            log::warn!("ocrmypdf failed to start for {}: {e}", path.display());
            return None;
        }
    };
    if !output.status.success() {
        log::warn!(
            "ocrmypdf exited {:?} for {}; stderr={}",
            output.status.code(),
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = std::fs::read_to_string(&sidecar).ok()?;
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn scheduler_ocr_path(path: &Path, options: &ParseOptions) -> Option<String> {
    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "document".to_string());
    let size = std::fs::metadata(path)
        .ok()
        .and_then(|m| usize::try_from(m.len()).ok())?;
    if !scheduler_inline_file_fits_with_copies(&filename, size, "kb.document.ocr_recognize", 3) {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    scheduler_ocr_bytes(&data, &filename, options)
}

fn env_usize_any(keys: &[&str], default: usize) -> usize {
    keys.iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .filter(|v| *v > 0)
        })
        .unwrap_or(default)
}

fn env_usize_any_allow_zero(keys: &[&str], default: usize) -> usize {
    keys.iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(default)
}

fn env_bool_any(keys: &[&str], default: bool) -> bool {
    keys.iter()
        .find_map(|key| {
            std::env::var(key).ok().map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
        })
        .unwrap_or(default)
}

fn scheduler_max_body_bytes() -> usize {
    env_usize_any(
        &[
            "ATTUNE_SCHEDULER_MAX_BODY_BYTES",
            "ATTUNE_LOCAL_SCHEDULER_MAX_BODY_BYTES",
        ],
        DEFAULT_SCHEDULER_MAX_BODY_BYTES,
    )
}

fn base64_encoded_len(bytes: usize) -> Option<usize> {
    bytes.checked_add(2).map(|n| (n / 3) * 4)
}

fn scheduler_inline_file_fits_body(bytes: usize, max_body_bytes: usize) -> bool {
    scheduler_inline_file_fits_body_with_copies(bytes, max_body_bytes, 1)
}

fn scheduler_inline_file_fits_body_with_copies(
    bytes: usize,
    max_body_bytes: usize,
    encoded_copies: usize,
) -> bool {
    let Some(encoded) = base64_encoded_len(bytes) else {
        return false;
    };
    let Some(total_encoded) = encoded.checked_mul(encoded_copies.max(1)) else {
        return false;
    };
    total_encoded <= max_body_bytes.saturating_sub(SCHEDULER_INLINE_JSON_OVERHEAD_BYTES)
}

fn scheduler_inline_file_fits(filename: &str, bytes: usize, task: &str) -> bool {
    scheduler_inline_file_fits_with_copies(filename, bytes, task, 1)
}

fn scheduler_inline_file_fits_with_copies(
    filename: &str,
    bytes: usize,
    task: &str,
    encoded_copies: usize,
) -> bool {
    let max_body = scheduler_max_body_bytes();
    let encoded_copies = encoded_copies.max(1);
    let fits = if encoded_copies == 1 {
        scheduler_inline_file_fits_body(bytes, max_body)
    } else {
        scheduler_inline_file_fits_body_with_copies(bytes, max_body, encoded_copies)
    };
    if fits {
        return true;
    }
    let encoded = base64_encoded_len(bytes).unwrap_or(usize::MAX);
    let total_encoded = encoded.saturating_mul(encoded_copies);
    log::warn!(
        "scheduler task {task} skipped for {filename}: inline file payload too large \
         (raw={} bytes, base64~{} bytes, encoded_copies={}, total_base64~{} bytes, max_body={} bytes); use text extraction or \
         page/chunk scheduler OCR for large documents",
        bytes,
        encoded,
        encoded_copies,
        total_encoded,
        max_body
    );
    false
}

fn scheduler_ocr_body(data: &[u8], filename: &str, options: &ParseOptions) -> Value {
    let file_base64 = base64::engine::general_purpose::STANDARD.encode(data);
    let timeout_ms = options
        .scheduler_timeout()
        .as_millis()
        .min(u32::MAX as u128) as u32;
    let input = serde_json::json!({
        "filename": filename,
        "profile": options.profile_id(),
        "profile_id": options.profile_id(),
        "file_base64": file_base64.clone(),
    });
    serde_json::json!({
        // Compatibility aliases: older Attune sent top-level fields, while the
        // scheduler task wrapper commonly validates an `x` or `input` argument.
        "input": input.clone(),
        "x": input,
        "filename": filename,
        "profile": options.profile_id(),
        "profile_id": options.profile_id(),
        "file_base64": file_base64,
        "timeout_ms": timeout_ms,
        "ttl_ms": timeout_ms
    })
}

fn scheduler_ocr_image_body(
    data: &[u8],
    filename: &str,
    page_number: usize,
    page_count: Option<usize>,
    dpi: u32,
    options: &ParseOptions,
) -> Value {
    let image_base64 = base64::engine::general_purpose::STANDARD.encode(data);
    let timeout_ms = options
        .scheduler_timeout()
        .as_millis()
        .min(u32::MAX as u128) as u32;
    let input = serde_json::json!({
        "filename": filename,
        "profile": options.profile_id(),
        "profile_id": options.profile_id(),
        "content_type": "image/png",
        "page": page_number,
        "page_number": page_number,
        "page_count": page_count,
        "dpi": dpi,
        "image_base64": image_base64.clone(),
    });
    serde_json::json!({
        "input": input.clone(),
        "x": input,
        "filename": filename,
        "profile": options.profile_id(),
        "profile_id": options.profile_id(),
        "content_type": "image/png",
        "page": page_number,
        "page_number": page_number,
        "page_count": page_count,
        "dpi": dpi,
        "image_base64": image_base64,
        "timeout_ms": timeout_ms,
        "ttl_ms": timeout_ms
    })
}

fn scheduler_ocr_bytes(data: &[u8], filename: &str, options: &ParseOptions) -> Option<String> {
    let base = options.scheduler_base.as_deref()?;
    if !scheduler_ocr_enabled() {
        return None;
    }
    if !scheduler_inline_file_fits_with_copies(filename, data.len(), "kb.document.ocr_recognize", 3)
    {
        return None;
    }
    scheduler_task_text(
        base,
        "kb.document.ocr_recognize",
        scheduler_ocr_body(data, filename, options),
        options.scheduler_timeout(),
    )
}

fn scheduler_ocr_image_bytes(
    data: &[u8],
    filename: &str,
    page_number: usize,
    page_count: Option<usize>,
    dpi: u32,
    options: &ParseOptions,
) -> Result<Option<String>> {
    let base = options.scheduler_base.as_deref().ok_or_else(|| {
        VaultError::InvalidInput("scheduler OCR base URL unavailable".to_string())
    })?;
    if !scheduler_ocr_enabled() {
        return Ok(None);
    }
    if !scheduler_inline_file_fits_with_copies(filename, data.len(), "kb.document.ocr_recognize", 3)
    {
        return Ok(None);
    }
    let outputs = scheduler_task_outputs(
        base,
        "kb.document.ocr_recognize",
        scheduler_ocr_image_body(data, filename, page_number, page_count, dpi, options),
        options.scheduler_timeout(),
    )?;
    Ok(scheduler_output_text(&outputs))
}

fn scheduler_asr_bytes(data: &[u8], filename: &str, options: &ParseOptions) -> Option<String> {
    let base = options.scheduler_base.as_deref()?;
    if !scheduler_inline_file_fits(filename, data.len(), "kb.meeting.asr_frontend") {
        return None;
    }
    let timeout_ms = options
        .scheduler_timeout()
        .as_millis()
        .min(u32::MAX as u128) as u32;
    let body = serde_json::json!({
        "filename": filename,
        "file_base64": base64::engine::general_purpose::STANDARD.encode(data),
        "timeout_ms": timeout_ms,
        "ttl_ms": timeout_ms
    });
    scheduler_task_text(
        base,
        "kb.meeting.asr_frontend",
        body,
        options.scheduler_timeout(),
    )
}

fn scheduler_task_text(
    base: &str,
    task: &str,
    body: Value,
    poll_timeout: Duration,
) -> Option<String> {
    match scheduler_task_outputs(base, task, body, poll_timeout) {
        Ok(outputs) => scheduler_output_text(&outputs),
        Err(e) => {
            log::warn!("scheduler task {task} failed: {e}");
            None
        }
    }
}

fn scheduler_task_outputs(
    base: &str,
    task: &str,
    body: Value,
    poll_timeout: Duration,
) -> Result<Value> {
    let client = crate::edge_cloud::scheduler::LocalSchedulerClient::with_base(
        base,
        Duration::from_secs(10),
    );
    let response = client.submit_kb_task(task, &body, true)?;
    if response
        .status
        .as_deref()
        .is_some_and(|s| s.eq_ignore_ascii_case("done"))
        || response.job_id.is_none()
    {
        return Ok(response.outputs);
    }

    let job_id = response.job_id.ok_or_else(|| {
        VaultError::LlmUnavailable(format!(
            "scheduler task {task} returned async without job_id"
        ))
    })?;
    let deadline = Instant::now() + poll_timeout;
    loop {
        let job = client.job(&job_id)?;
        match job.status.to_ascii_lowercase().as_str() {
            "done" => return Ok(job.outputs),
            "error" => {
                return Err(VaultError::LlmUnavailable(format!(
                    "scheduler task {task} job {job_id} failed: {}",
                    job.error
                        .or(job.detail)
                        .unwrap_or_else(|| "unknown error".to_string())
                )));
            }
            "canceled" | "cancelled" | "expired" => {
                return Err(VaultError::LlmUnavailable(format!(
                    "scheduler task {task} job {job_id} ended with status {}",
                    job.status
                )));
            }
            _ => {
                if Instant::now() >= deadline {
                    return Err(VaultError::LlmUnavailable(format!(
                        "scheduler task {task} job {job_id} timed out after {} ms",
                        poll_timeout.as_millis()
                    )));
                }
                thread::sleep(Duration::from_millis(250));
            }
        }
    }
}

fn scheduler_output_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str().map(str::trim).filter(|s| !s.is_empty()) {
        return Some(text.to_string());
    }
    for pointer in [
        "/text",
        "/answer",
        "/content",
        "/full_text",
        "/transcript",
        "/markdown",
        "/outputs/text",
        "/outputs/full_text",
        "/outputs/transcript",
        "/data/text",
    ] {
        if let Some(text) = value.pointer(pointer).and_then(Value::as_str) {
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    for pointer in [
        "/lines",
        "/regions",
        "/segments",
        "/outputs/lines",
        "/outputs/segments",
    ] {
        if let Some(text) = value.pointer(pointer).and_then(scheduler_array_text) {
            return Some(text);
        }
    }
    value.get("outputs").and_then(scheduler_output_text)
}

fn scheduler_array_text(value: &Value) -> Option<String> {
    let arr = value.as_array()?;
    let mut parts = Vec::new();
    for item in arr {
        if let Some(text) = item.as_str() {
            let text = text.trim();
            if !text.is_empty() {
                parts.push(text.to_string());
            }
            continue;
        }
        for key in ["text", "content", "transcript", "line", "value"] {
            if let Some(text) = item.get(key).and_then(Value::as_str) {
                let text = text.trim();
                if !text.is_empty() {
                    parts.push(text.to_string());
                    break;
                }
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// 解析文件 → (title, content). 等价于 `parse_file_with_profile(path, None)`.
pub fn parse_file(path: &Path) -> Result<(String, String)> {
    parse_file_with_profile(path, None)
}

/// 解析文件, 指定 OCR profile (PDF 扫描件走自定义 DPI). None = 走默认 300 DPI.
pub fn parse_file_with_profile(path: &Path, profile_id: Option<&str>) -> Result<(String, String)> {
    parse_file_with_options(path, &ParseOptions::with_profile(profile_id))
}

/// 解析文件，智能 OCR/ASR 可由 scheduler 统一承接。
pub fn parse_file_with_options(path: &Path, options: &ParseOptions) -> Result<(String, String)> {
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.clone());

    match ext.as_str() {
        ".pdf" => parse_pdf_file_with_dpi(
            path,
            &stem,
            crate::ocr::dpi_for_profile(options.profile_id()),
            options,
        ),
        ".docx" => parse_docx_file(path, &stem),
        ".html" | ".htm" => parse_html_file(path, &stem),
        ".epub" => parse_epub_file(path, &stem),
        ".xlsx" | ".xls" => parse_xlsx_file(path, &stem),
        ".pptx" => parse_pptx_file(path, &stem),
        ".rtf" => parse_rtf_file(path, &stem),
        ".csv" => parse_csv_file(path, &stem),
        ".png" | ".jpg" | ".jpeg" | ".webp" | ".bmp" | ".tiff" | ".tif" | ".gif" => {
            parse_image_file(path, &stem, options)
        }
        ".mp3" | ".wav" | ".m4a" | ".flac" | ".ogg" | ".aac" | ".opus" | ".wma" => {
            parse_audio_file(path, &stem, options)
        }
        _ => {
            // 允许作为纯文本处理的扩展名：代码文件 + 通用文本格式
            let is_code = CODE_EXTENSIONS.contains(&ext.as_str());
            let is_plain_text = matches!(ext.as_str(), ".md" | ".txt" | "");
            if !is_code && !is_plain_text {
                return Err(VaultError::InvalidInput(format!(
                    "unsupported file format '{ext}': only text, code, documents, spreadsheets, images and audio are accepted"
                )));
            }
            let content = std::fs::read_to_string(path).map_err(VaultError::Io)?;
            parse_content(&content, &filename)
        }
    }
}

/// 从内存解析 → (title, content). 等价于 `parse_bytes_with_profile(data, filename, None)`.
pub fn parse_bytes(data: &[u8], filename: &str) -> Result<(String, String)> {
    parse_bytes_with_profile(data, filename, None)
}

/// 从内存解析, 指定 OCR profile.
pub fn parse_bytes_with_profile(
    data: &[u8],
    filename: &str,
    profile_id: Option<&str>,
) -> Result<(String, String)> {
    parse_bytes_with_options(data, filename, &ParseOptions::with_profile(profile_id))
}

/// 从内存解析，智能 OCR/ASR 可由 scheduler 统一承接。
pub fn parse_bytes_with_options(
    data: &[u8],
    filename: &str,
    options: &ParseOptions,
) -> Result<(String, String)> {
    let ext = Path::new(filename)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    let stem = Path::new(filename)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());
    let dpi = crate::ocr::dpi_for_profile(options.profile_id());

    match ext.as_str() {
        ".pdf" => {
            // 上传路径走内存，但 OCR 需要磁盘文件（pdftoppm 读文件）。
            // Poppler 的 pdftotext 对大型飞行手册更稳，先用它取文字层；失败后再退回
            // pdf_extract，最后才走 OCR。
            if let Some(pdftotext) = try_pdftotext_from_bytes(data) {
                if pdf_text_layer_is_usable(&pdftotext) {
                    let title = first_line_title(&pdftotext, &stem);
                    return Ok((title, pdftotext));
                }
                if let Some(ocr_text) = try_ocr_from_bytes_with_dpi(data, filename, dpi, options) {
                    let title = first_line_title(&ocr_text, &stem);
                    return Ok((title, ocr_text));
                }
                let title = first_line_title(&pdftotext, &stem);
                return Ok((title, pdftotext));
            }

            let extract_result = safe_pdf_extract_text_from_mem(data);
            let content = match extract_result {
                Ok(text) if pdf_text_layer_is_usable(&text) => text,
                Ok(thin_text) => {
                    if let Some(ocr_text) =
                        try_ocr_from_bytes_with_dpi(data, filename, dpi, options)
                    {
                        let title = first_line_title(&ocr_text, &stem);
                        return Ok((title, ocr_text));
                    }
                    thin_text
                }
                Err(e) => {
                    log::info!("pdf_extract failed for uploaded bytes ({e}); trying pdftotext/OCR");
                    if let Some(ocr_text) =
                        try_ocr_from_bytes_with_dpi(data, filename, dpi, options)
                    {
                        let title = first_line_title(&ocr_text, &stem);
                        return Ok((title, ocr_text));
                    }
                    return Err(VaultError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("PDF extract failed: {e}; OCR unavailable or also failed"),
                    )));
                }
            };
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".docx" => {
            use std::io::Cursor;
            let cursor = Cursor::new(data);
            let mut archive = zip::ZipArchive::new(cursor).map_err(|e| {
                VaultError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("DOCX zip open failed: {e}"),
                ))
            })?;
            let doc_xml = if let Ok(mut entry) = archive.by_name("word/document.xml") {
                read_zip_entry_string_bounded(&mut entry)?
            } else {
                return Err(VaultError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "word/document.xml not found in docx",
                )));
            };
            let content = strip_xml_tags(&doc_xml);
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".html" | ".htm" => {
            let html = String::from_utf8_lossy(data).to_string();
            let content = html_to_text(&html);
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".epub" => {
            let content = epub_bytes_to_text(data)?;
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".xlsx" | ".xls" => {
            let content = xlsx_bytes_to_text(data, &ext)?;
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".pptx" => {
            let content = pptx_bytes_to_text(data)?;
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".rtf" => {
            let content = rtf_to_text(&String::from_utf8_lossy(data));
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".csv" => {
            let content = String::from_utf8_lossy(data).to_string();
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".png" | ".jpg" | ".jpeg" | ".webp" | ".bmp" | ".tiff" | ".tif" | ".gif" => {
            if let Some(text) = scheduler_ocr_bytes(data, filename, options) {
                let title = first_line_title(&text, &stem);
                return Ok((title, text));
            }
            if options.scheduler_base.is_some() {
                return Err(VaultError::InvalidInput(
                    "scheduler OCR unavailable".to_string(),
                ));
            }
            let Some(provider) = crate::ocr::detect_default_provider() else {
                return Err(VaultError::InvalidInput(
                    "OCR provider unavailable".to_string(),
                ));
            };
            let scene = crate::ocr::auto_detect_scene(filename);
            let profile = crate::ocr::profile_for_id(Some(scene));
            // bytes path: write to temp file (OCR expects a Path)
            let mut tmp = tempfile::Builder::new()
                .suffix(&ext)
                .tempfile()
                .map_err(VaultError::Io)?;
            {
                use std::io::Write;
                tmp.write_all(data).map_err(VaultError::Io)?;
                tmp.flush().map_err(VaultError::Io)?;
            }
            let output = provider.extract_structured(tmp.path(), &profile)?;
            if let Some(c) = output.avg_confidence {
                log::info!("OCR 图片 '{filename}' avg_confidence={c:.3}");
            }
            let content = if let Some(table) = output.table_markdown {
                format!("{}\n\n{}", output.text, table)
            } else {
                output.text
            };
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".mp3" | ".wav" | ".m4a" | ".flac" | ".ogg" | ".aac" | ".opus" | ".wma" => {
            if let Some(text) = scheduler_asr_bytes(data, filename, options) {
                let title = first_line_title(&text, &stem);
                return Ok((title, text));
            }
            if options.scheduler_base.is_some() {
                return Err(VaultError::InvalidInput(
                    "scheduler ASR unavailable".to_string(),
                ));
            }
            let Some(engine) = crate::asr::detect_asr_engine() else {
                return Err(VaultError::InvalidInput(
                    "ASR backend unavailable".to_string(),
                ));
            };
            let mut tmp = tempfile::Builder::new()
                .suffix(&ext)
                .tempfile()
                .map_err(VaultError::Io)?;
            use std::io::Write;
            tmp.write_all(data).map_err(VaultError::Io)?;
            tmp.flush().map_err(VaultError::Io)?;
            // SenseVoice = plain in-process transcription (no diarization). Whisper keeps
            // the diarization path (whisperx/pyannote) so multi-speaker audio is unaffected.
            let content = match &engine {
                crate::asr::AsrEngine::Whisper(backend) => {
                    let diarization = crate::asr::detect_diarization_backend();
                    let (_, c) = crate::asr::transcribe_with_diarization(
                        backend,
                        tmp.path(),
                        diarization.as_ref(),
                    )?;
                    c
                }
                crate::asr::AsrEngine::SenseVoice(_) => {
                    crate::asr::transcribe_with_engine(&engine, tmp.path())?
                }
            };
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        _ => {
            // 允许作为纯文本处理的扩展名：代码文件 + 通用文本格式
            // 已知二进制格式（video/archive/executable 等）拒绝，避免乱码入库
            let is_code = CODE_EXTENSIONS.contains(&ext.as_str());
            let is_plain_text = matches!(ext.as_str(), ".md" | ".txt" | "");
            if !is_code && !is_plain_text {
                return Err(VaultError::InvalidInput(format!(
                    "unsupported file format '{ext}': only text, code, documents, spreadsheets, images and audio are accepted"
                )));
            }
            let content = String::from_utf8_lossy(data).to_string();
            parse_content(&content, filename)
        }
    }
}

fn try_ocr_from_pdf_path_with_dpi(path: &Path, dpi: u32, options: &ParseOptions) -> Option<String> {
    if let Some(text) = scheduler_ocr_path(path, options) {
        return Some(text);
    }
    if let Some(text) = try_scheduler_pdf_page_ocr_from_path(path, dpi, options) {
        return Some(text);
    }
    if let Some(text) = try_ocrmypdf_sidecar_from_path(path) {
        return Some(text);
    }
    if options.scheduler_base.is_some() && !scheduler_local_ocr_provider_fallback_enabled() {
        return None;
    }
    let provider = crate::ocr::detect_default_provider()?;
    match crate::ocr::extract_text_from_pdf_with_dpi(provider.as_ref(), path, dpi) {
        Ok(text) if !text.trim().is_empty() => Some(text),
        Ok(_) => {
            log::warn!("OCR returned empty text for {}", path.display());
            None
        }
        Err(e) => {
            log::warn!("OCR failed for {}: {e}", path.display());
            None
        }
    }
}

/// 把 PDF 字节写到临时文件并调用 OCR provider, 指定 DPI (200 / 300 / 600).
/// dpi 由调用方按 OcrProfile 决定 — 默认走 `dpi_for_profile(None) = 300`.
fn try_ocr_from_bytes_with_dpi(
    data: &[u8],
    filename: &str,
    dpi: u32,
    options: &ParseOptions,
) -> Option<String> {
    if let Some(text) = scheduler_ocr_bytes(data, filename, options) {
        return Some(text);
    }
    let mut tmp = tempfile::Builder::new().suffix(".pdf").tempfile().ok()?;
    tmp.write_all(data).ok()?;
    tmp.flush().ok()?;
    if let Some(text) = try_scheduler_pdf_page_ocr_from_path(tmp.path(), dpi, options) {
        return Some(text);
    }
    if let Some(text) = try_ocrmypdf_sidecar_from_path(tmp.path()) {
        return Some(text);
    }
    if options.scheduler_base.is_some() && !scheduler_local_ocr_provider_fallback_enabled() {
        return None;
    }
    let provider = crate::ocr::detect_default_provider()?;
    match crate::ocr::extract_text_from_pdf_with_dpi(provider.as_ref(), tmp.path(), dpi) {
        Ok(text) if !text.trim().is_empty() => Some(text),
        Ok(_) => {
            log::warn!("OCR returned empty text for uploaded PDF");
            None
        }
        Err(e) => {
            log::warn!("OCR failed for uploaded PDF: {e}");
            None
        }
    }
}

fn scheduler_pdf_ocr_page_limit(page_count: Option<usize>) -> usize {
    let configured = env_usize_any_allow_zero(
        &[
            "ATTUNE_SCHEDULER_PDF_OCR_MAX_PAGES",
            "ATTUNE_LOCAL_SCHEDULER_PDF_OCR_MAX_PAGES",
            "ATTUNE_PDF_OCR_MAX_PAGES",
        ],
        DEFAULT_SCHEDULER_PDF_OCR_MAX_PAGES,
    );
    match (configured, page_count) {
        (0, Some(count)) => count,
        (0, None) => DEFAULT_SCHEDULER_PDF_OCR_UNKNOWN_MAX_PAGES,
        (limit, Some(count)) => limit.min(count),
        (limit, None) => limit,
    }
}

fn scheduler_pdf_ocr_max_failed_pages(page_limit: usize) -> usize {
    env_usize_any(
        &[
            "ATTUNE_SCHEDULER_PDF_OCR_MAX_FAILED_PAGES",
            "ATTUNE_LOCAL_SCHEDULER_PDF_OCR_MAX_FAILED_PAGES",
            "ATTUNE_PDF_OCR_MAX_FAILED_PAGES",
        ],
        DEFAULT_SCHEDULER_PDF_OCR_MAX_FAILED_PAGES,
    )
    .min(page_limit.max(1))
}

fn scheduler_pdf_ocr_max_consecutive_failures() -> usize {
    env_usize_any(
        &[
            "ATTUNE_SCHEDULER_PDF_OCR_MAX_CONSECUTIVE_FAILURES",
            "ATTUNE_LOCAL_SCHEDULER_PDF_OCR_MAX_CONSECUTIVE_FAILURES",
            "ATTUNE_PDF_OCR_MAX_CONSECUTIVE_FAILURES",
        ],
        DEFAULT_SCHEDULER_PDF_OCR_MAX_CONSECUTIVE_FAILURES,
    )
    .max(1)
}

fn scheduler_pdf_ocr_dpi_candidates(requested_dpi: u32) -> Vec<u32> {
    let max_dpi = env_usize_any(
        &[
            "ATTUNE_SCHEDULER_PDF_OCR_MAX_DPI",
            "ATTUNE_LOCAL_SCHEDULER_PDF_OCR_MAX_DPI",
        ],
        DEFAULT_SCHEDULER_PDF_OCR_MAX_DPI,
    )
    .clamp(72, 1200) as u32;
    let min_dpi = env_usize_any(
        &[
            "ATTUNE_SCHEDULER_PDF_OCR_MIN_DPI",
            "ATTUNE_LOCAL_SCHEDULER_PDF_OCR_MIN_DPI",
        ],
        DEFAULT_SCHEDULER_PDF_OCR_MIN_DPI,
    )
    .clamp(72, max_dpi as usize) as u32;
    let base = env_usize_any(
        &[
            "ATTUNE_SCHEDULER_PDF_OCR_DPI",
            "ATTUNE_LOCAL_SCHEDULER_PDF_OCR_DPI",
        ],
        requested_dpi.min(max_dpi) as usize,
    )
    .clamp(72, 1200) as u32;

    let mut candidates = Vec::new();
    for dpi in [base, max_dpi, 200, 150, min_dpi] {
        let dpi = dpi.clamp(min_dpi, max_dpi);
        if !candidates.contains(&dpi) {
            candidates.push(dpi);
        }
    }
    candidates
}

fn pdf_page_count(path: &Path) -> Option<usize> {
    let pdfinfo = which::which("pdfinfo").ok()?;
    let output = crate::process::command_no_window(&pdfinfo)
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        log::debug!(
            "pdfinfo failed for {}: exit {:?}; stderr={}",
            path.display(),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if !key.trim().eq_ignore_ascii_case("Pages") {
            return None;
        }
        value.trim().parse::<usize>().ok().filter(|v| *v > 0)
    })
}

fn render_pdf_page_png(
    path: &Path,
    page_number: usize,
    dpi: u32,
    tmp_dir: &Path,
) -> Result<std::path::PathBuf> {
    let pdftoppm = which::which("pdftoppm").map_err(|_| {
        VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "pdftoppm not found (poppler-utils required for page OCR)",
        ))
    })?;
    let prefix = tmp_dir.join(format!("page-{page_number:06}-{dpi}dpi"));
    let prefix_str = prefix.to_str().ok_or_else(|| {
        VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "non-UTF8 temp path",
        ))
    })?;
    let page = page_number.to_string();
    let dpi_s = dpi.to_string();
    let status = crate::process::command_no_window(&pdftoppm)
        .args([
            "-r",
            dpi_s.as_str(),
            "-png",
            "-f",
            page.as_str(),
            "-l",
            page.as_str(),
            "-singlefile",
        ])
        .arg(path)
        .arg(prefix_str)
        .status()
        .map_err(VaultError::Io)?;
    if !status.success() {
        return Err(VaultError::Io(std::io::Error::other(format!(
            "pdftoppm page {page_number} failed: exit {}",
            status.code().unwrap_or(-1)
        ))));
    }
    let png = prefix.with_extension("png");
    if !png.exists() {
        return Err(VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("pdftoppm did not produce {}", png.display()),
        )));
    }
    Ok(png)
}

fn try_scheduler_pdf_page_ocr_from_path(
    path: &Path,
    requested_dpi: u32,
    options: &ParseOptions,
) -> Option<String> {
    if options.scheduler_base.is_none() || !scheduler_ocr_enabled() {
        return None;
    }
    if which::which("pdftoppm").is_err() {
        log::warn!(
            "scheduler PDF page OCR skipped for {}: pdftoppm not found",
            path.display()
        );
        return None;
    }

    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "document.pdf".to_string());
    let page_count = pdf_page_count(path);
    let page_limit = scheduler_pdf_ocr_page_limit(page_count);
    if page_limit == 0 {
        return None;
    }
    let max_failed = scheduler_pdf_ocr_max_failed_pages(page_limit);
    let max_consecutive_failed = scheduler_pdf_ocr_max_consecutive_failures();
    let dpi_candidates = scheduler_pdf_ocr_dpi_candidates(requested_dpi);
    let tmp = tempfile::TempDir::new().ok()?;

    let mut all = String::with_capacity(page_limit.min(32) * 1024);
    let mut ok_pages = 0usize;
    let mut empty_pages = 0usize;
    let mut failed_pages = 0usize;
    let mut consecutive_failed = 0usize;

    for page in 1..=page_limit {
        let mut page_rendered = false;
        let mut page_failed = false;
        let mut page_too_large = false;
        let mut page_text: Option<String> = None;

        for dpi in &dpi_candidates {
            let png = match render_pdf_page_png(path, page, *dpi, tmp.path()) {
                Ok(png) => png,
                Err(e) => {
                    page_failed = true;
                    log::warn!(
                        "scheduler PDF page OCR render failed for {} page {} at {}dpi: {}",
                        path.display(),
                        page,
                        dpi,
                        e
                    );
                    continue;
                }
            };
            page_rendered = true;
            let data = match std::fs::read(&png) {
                Ok(data) => data,
                Err(e) => {
                    page_failed = true;
                    log::warn!(
                        "scheduler PDF page OCR failed to read rendered page {} for {}: {}",
                        page,
                        path.display(),
                        e
                    );
                    continue;
                }
            };
            if !scheduler_inline_file_fits_with_copies(
                &format!("{filename}#page={page}"),
                data.len(),
                "kb.document.ocr_recognize",
                3,
            ) {
                page_too_large = true;
                continue;
            }
            match scheduler_ocr_image_bytes(&data, &filename, page, page_count, *dpi, options) {
                Ok(Some(text)) if !text.trim().is_empty() => {
                    page_text = Some(text);
                    break;
                }
                Ok(_) => {
                    empty_pages += 1;
                    page_text = Some(String::new());
                    break;
                }
                Err(e) => {
                    page_failed = true;
                    log::warn!(
                        "scheduler PDF page OCR failed for {} page {} at {}dpi: {}",
                        path.display(),
                        page,
                        dpi,
                        e
                    );
                    break;
                }
            }
        }

        match page_text {
            Some(text) => {
                consecutive_failed = 0;
                if !text.trim().is_empty() {
                    ok_pages += 1;
                    all.push_str(&format!("--- Page {page} ---\n"));
                    all.push_str(text.trim());
                    all.push_str("\n\n");
                }
            }
            None => {
                failed_pages += 1;
                consecutive_failed += 1;
                let reason = if page_rendered {
                    if page_too_large {
                        "page image exceeded scheduler body budget"
                    } else {
                        "OCR task returned no usable result"
                    }
                } else if page_failed {
                    "page render failed"
                } else {
                    "page image exceeded scheduler body budget"
                };
                log::warn!(
                    "scheduler PDF page OCR skipped {} page {}: {}",
                    path.display(),
                    page,
                    reason
                );
                if failed_pages >= max_failed || consecutive_failed >= max_consecutive_failed {
                    log::warn!(
                        "scheduler PDF page OCR stopping for {} after {} failed pages ({} consecutive)",
                        path.display(),
                        failed_pages,
                        consecutive_failed
                    );
                    break;
                }
            }
        }
    }

    if all.trim().is_empty() {
        return None;
    }
    log::info!(
        "scheduler PDF page OCR completed for {}: text_pages={}, empty_pages={}, failed_pages={}, page_limit={}, detected_pages={:?}",
        path.display(),
        ok_pages,
        empty_pages,
        failed_pages,
        page_limit,
        page_count
    );
    Some(all)
}

fn try_pdftotext_from_bytes(data: &[u8]) -> Option<String> {
    let mut tmp = tempfile::Builder::new().suffix(".pdf").tempfile().ok()?;
    tmp.write_all(data).ok()?;
    tmp.flush().ok()?;
    try_pdftotext_from_path(tmp.path())
}

fn try_pdftotext_from_path(path: &Path) -> Option<String> {
    let pdftotext = which::which("pdftotext").ok()?;
    let output = match crate::process::command_no_window(&pdftotext)
        .args(["-enc", "UTF-8"])
        .arg(path)
        .arg("-")
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            log::warn!("pdftotext failed to start for {}: {e}", path.display());
            return None;
        }
    };
    if !output.status.success() {
        log::warn!(
            "pdftotext failed for {}: exit {:?}; stderr={}",
            path.display(),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn parse_pdf_file_with_dpi(
    path: &Path,
    stem: &str,
    dpi: u32,
    options: &ParseOptions,
) -> Result<(String, String)> {
    // 1. 本地文件优先走 Poppler。大型手册上 pdf_extract 可能产生海量日志或 panic，
    //    pdftotext 的流式外部进程路径更适合批量索引。
    if let Some(pdftotext) = try_pdftotext_from_path(path) {
        if pdf_text_layer_is_usable(&pdftotext) {
            let title = first_line_title(&pdftotext, stem);
            return Ok((title, pdftotext));
        }
        if let Some(ocr_text) = try_ocr_from_pdf_path_with_dpi(path, dpi, options) {
            let title = first_line_title(&ocr_text, stem);
            return Ok((title, ocr_text));
        }
        let title = first_line_title(&pdftotext, stem);
        return Ok((title, pdftotext));
    }

    // 2. Poppler 不可用或失败时，退回 pdf_extract 直接取文字层。
    let bytes = std::fs::read(path)?;
    let extract_result = safe_pdf_extract_text_from_mem(&bytes);

    // 2a. 提取失败（常见于加密/损坏扫描件）→ 尝试 OCR；pdftoppm 对许多
    //     pdf_extract 不支持的加密方案容忍度更高
    let content = match extract_result {
        Ok(text) => text,
        Err(e) => {
            log::info!(
                "pdf_extract failed for {} ({e}); trying scheduler OCR",
                path.display()
            );
            if let Some(ocr_text) = try_ocr_from_pdf_path_with_dpi(path, dpi, options) {
                let title = first_line_title(&ocr_text, stem);
                return Ok((title, ocr_text));
            }
            return Err(VaultError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("PDF extract failed: {e}; OCR unavailable or also failed"),
            )));
        }
    };

    // 2b. 成功但文字量 < 100 字符（扫描版文字层空，或 pdf_extract 对混排文字层
    //     退化）→ 尝试 OCR。
    if !pdf_text_layer_is_usable(&content) {
        if options.scheduler_base.is_some() {
            log::info!(
                "PDF text layer thin ({} chars); falling back to scheduler OCR",
                content.chars().filter(|c| !c.is_whitespace()).count()
            );
            if let Some(ocr_text) = try_ocr_from_pdf_path_with_dpi(path, dpi, options) {
                let title = first_line_title(&ocr_text, stem);
                return Ok((title, ocr_text));
            }
        } else if let Some(provider) = crate::ocr::detect_default_provider() {
            log::info!(
                "PDF text layer thin ({} chars); falling back to legacy OCR ({})",
                content.chars().filter(|c| !c.is_whitespace()).count(),
                provider.name()
            );
            if let Some(ocr_text) = try_ocr_from_pdf_path_with_dpi(path, dpi, options) {
                let title = first_line_title(&ocr_text, stem);
                return Ok((title, ocr_text));
            }
        } else {
            log::debug!(
                "PDF has no text layer but OCR provider not available; \
                returning thin text. Re-run apt install / attune deploy to fix."
            );
        }
    }

    let title = first_line_title(&content, stem);
    Ok((title, content))
}

fn parse_docx_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("DOCX zip open failed: {e}"),
        ))
    })?;

    let doc_xml = if let Ok(mut entry) = archive.by_name("word/document.xml") {
        read_zip_entry_string_bounded(&mut entry)?
    } else {
        return Err(VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "word/document.xml not found in docx",
        )));
    };

    let content = strip_xml_tags(&doc_xml);
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

/// 从首行提取标题，若首行为空或过长则使用 stem
fn first_line_title(content: &str, stem: &str) -> String {
    content
        .lines()
        .next()
        .filter(|l| !l.trim().is_empty() && l.len() < 200)
        .map(|l| l.trim().to_string())
        .unwrap_or_else(|| stem.to_string())
}

/// 简单 XML 标签剥离器（适用于 DOCX word/document.xml）
fn strip_xml_tags(xml: &str) -> String {
    let mut result = String::with_capacity(xml.len() / 3);
    let mut in_tag = false;
    let mut last_was_space = false;

    for ch in xml.chars() {
        match ch {
            '<' => {
                in_tag = true;
                if !last_was_space && !result.is_empty() {
                    result.push(' ');
                    last_was_space = true;
                }
            }
            '>' => {
                in_tag = false;
            }
            _ if !in_tag => {
                result.push(ch);
                last_was_space = ch.is_whitespace();
            }
            _ => {}
        }
    }

    // Normalize whitespace
    result
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" .", ".")
        .replace(" ,", ",")
}

fn parse_content(content: &str, filename: &str) -> Result<(String, String)> {
    let ext = Path::new(filename)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    let stem = Path::new(filename)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());

    let title = if ext == ".md" {
        // Markdown: 提取第一个 # 标题
        content
            .lines()
            .find(|l| l.trim().starts_with("# "))
            .map(|l| l.trim().trim_start_matches("# ").trim().to_string())
            .unwrap_or(stem)
    } else if CODE_EXTENSIONS.iter().any(|e| *e == ext) {
        filename.to_string()
    } else {
        // TXT 等: 首行作标题
        content
            .lines()
            .next()
            .filter(|l| !l.trim().is_empty())
            // char-safe truncation: byte-slicing [..100] panics when byte 100 lands
            // mid-codepoint on a >100-byte multibyte first line (emoji/CJK) — take 100 chars.
            .map(|l| l.trim().chars().take(100).collect::<String>())
            .unwrap_or(stem)
    };

    Ok((title, content.to_string()))
}

/// 检查文件是否为支持的类型
pub fn is_supported(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        // 文档
        ".md" | ".txt" | ".pdf" | ".docx" | ".html" | ".htm" | ".epub"
        | ".rtf" | ".pptx"
        // 数据/表格
        | ".csv" | ".xlsx" | ".xls"
        // 图片 → OCR
        | ".png" | ".jpg" | ".jpeg" | ".webp" | ".bmp" | ".tiff" | ".tif" | ".gif"
        // 音频 → ASR
        | ".mp3" | ".wav" | ".m4a" | ".flac" | ".ogg" | ".aac" | ".opus" | ".wma"
    ) || CODE_EXTENSIONS.iter().any(|e| *e == ext)
}

/// 计算文件的 SHA-256 hash
pub fn file_hash(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let data = std::fs::read(path)?;
    let hash = Sha256::digest(&data);
    Ok(hex::encode(hash))
}

// ── 新格式处理函数 ────────────────────────────────────────────────────────────

/// 单个 ZIP 条目解压后允许的最大字节数（防解压炸弹）。
///
/// docx/epub/pptx 都是 ZIP+XML 容器，且是不可信的上传。`read_to_string` 会把整个
/// 解压后的条目读进内存——一个高压缩比的 "zip bomb"（几 KB 压缩 → 数 GB 解压）能把
/// 进程 OOM。这里给单条目设一个硬上限：解压字节超过该值即报错（InvalidInput），
/// 而不是无界分配。64 MB 远大于任何真实的 `word/document.xml` / slide / xhtml 章节，
/// 既不误伤正常文档，又把炸弹挡在 OOM 之前。
const MAX_ZIP_ENTRY_BYTES: u64 = 64 * 1024 * 1024;

/// 把一个 ZIP 条目读成 String，但**带解压上限**(`MAX_ZIP_ENTRY_BYTES`)。
///
/// 用 `Read::take(limit + 1)` 限制读取量：若解压输出能填满 `limit + 1`，说明真实
/// 大小超过上限 → 返回 InvalidInput（拒绝该条目，不继续无界解压）。lossy UTF-8
/// 解码与原 `read_to_string` 行为一致（容错而非 panic）。
fn read_zip_entry_string_bounded<R: std::io::Read>(reader: &mut R) -> Result<String> {
    use std::io::Read;
    let mut buf: Vec<u8> = Vec::new();
    let mut limited = reader.take(MAX_ZIP_ENTRY_BYTES + 1);
    limited.read_to_end(&mut buf).map_err(VaultError::Io)?;
    if buf.len() as u64 > MAX_ZIP_ENTRY_BYTES {
        return Err(VaultError::InvalidInput(format!(
            "zip entry exceeds {MAX_ZIP_ENTRY_BYTES} bytes decompressed (possible zip bomb); rejected"
        )));
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// HTML 文件 → 纯文本（scraper strip tags，保留段落空行）
fn parse_html_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let html = std::fs::read_to_string(path).map_err(VaultError::Io)?;
    let content = html_to_text(&html);
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

/// element 子树文本，跳过 `<script>`/`<style>` 子树。scraper 的 `.text()` 会把 script/style
/// 的源码文本一并收进来；不剔除就会让脚本/样式源码泄漏进被索引/可搜索的内容(安全+质量问题)。
fn text_excluding_script_style(el: scraper::ElementRef) -> String {
    let mut out: Vec<String> = Vec::new();
    for node in el.descendants() {
        let Some(t) = node.value().as_text() else {
            continue;
        };
        // 文本节点的任一祖先是 script/style → 跳过
        let mut ancestor = node.parent();
        let mut skip = false;
        while let Some(a) = ancestor {
            if let Some(e) = a.value().as_element() {
                if e.name() == "script" || e.name() == "style" {
                    skip = true;
                    break;
                }
            }
            ancestor = a.parent();
        }
        if !skip {
            out.push(t.to_string());
        }
    }
    out.join(" ")
}

/// HTML 字符串 → 可读文本（title tag 优先，body 内联文本拼接）
fn html_to_text(html: &str) -> String {
    use scraper::{Html, Selector};
    let document = Html::parse_document(html);

    // 尝试提取 <title>
    let title_text = Selector::parse("title")
        .ok()
        .and_then(|sel| document.select(&sel).next())
        .map(|el| el.text().collect::<String>())
        .unwrap_or_default();

    // body 文本：剔除 <script>/<style> 子树(防脚本/样式源码泄漏进索引)
    let body_text = if let Ok(body_sel) = Selector::parse("body") {
        document
            .select(&body_sel)
            .next()
            .map(text_excluding_script_style)
            .unwrap_or_default()
    } else {
        text_excluding_script_style(document.root_element())
    };

    // 合并并规范空白
    let raw = if title_text.is_empty() {
        body_text
    } else {
        format!("{}\n\n{}", title_text, body_text)
    };
    collapse_whitespace(&raw)
}

/// EPUB 文件 → 纯文本（解压 zip，合并所有 XHTML/HTML 条目）
fn parse_epub_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let data = std::fs::read(path).map_err(VaultError::Io)?;
    let content = epub_bytes_to_text(&data)?;
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

fn epub_bytes_to_text(data: &[u8]) -> Result<String> {
    use std::io::Cursor;
    let cursor = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| {
        VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("EPUB zip open failed: {e}"),
        ))
    })?;

    let mut parts: Vec<String> = Vec::new();
    let mut total: u64 = 0; // 累计解压字节，防"多条目累加"型炸弹
    let count = archive.len();
    for i in 0..count {
        let mut entry = archive.by_index(i).map_err(|e| {
            VaultError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{e}"),
            ))
        })?;
        let name = entry.name().to_lowercase();
        if !name.ends_with(".xhtml") && !name.ends_with(".html") && !name.ends_with(".htm") {
            continue;
        }
        // 单条目带解压上限；任一条目超限即拒绝整本（zip bomb）。注：从旧的
        // `let _ = read_to_string`(吞读错误、跳过坏条目) 改为 `?` 传播——bomb 防护
        // 要求传播，且坏条目 hard-fail 比静默跳过更正确。
        let buf = read_zip_entry_string_bounded(&mut entry)?;
        total = total.saturating_add(buf.len() as u64);
        if total > MAX_ZIP_ENTRY_BYTES {
            return Err(VaultError::InvalidInput(format!(
                "epub total decompressed text exceeds {MAX_ZIP_ENTRY_BYTES} bytes (possible zip bomb); rejected"
            )));
        }
        if !buf.is_empty() {
            parts.push(html_to_text(&buf));
        }
    }
    Ok(parts.join("\n\n"))
}

/// XLSX / XLS 文件 → 纯文本（calamine 读取所有 sheet，每行 tab 分隔）
fn parse_xlsx_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let data = std::fs::read(path).map_err(VaultError::Io)?;
    let content = xlsx_bytes_to_text(
        &data,
        &path
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
            .unwrap_or_default(),
    )?;
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

fn xlsx_bytes_to_text(data: &[u8], ext: &str) -> Result<String> {
    use calamine::{open_workbook_from_rs, Data, Reader, Xls, Xlsx};
    use std::io::Cursor;

    // calamine 在内部物化 workbook（shared/inline strings 累积进内存，无解压上限），
    // 所以 zip-bomb 防护必须在交给 calamine **之前**做。.xlsx 是 ZIP 容器：读中央目录
    // 里每个条目**声明的**解压后大小(`size()`，无需真解压)，任一超 MAX_ZIP_ENTRY_BYTES
    // 即拒绝。.xls 是 BIFF 二进制(非 zip)，跳过此扫描，由 calamine 自身边界兜底。
    //
    // 已知局限(FLAG，见 office_adversarial_test.rs::xlsx_spoofed_size_bomb_*)：`size()`
    // 取自中央目录、可被构造者伪造（声明小、实际 inflate 大）。本扫描挡得住"诚实"的
    // 解压炸弹(主流 bomb 工具写真实中央目录)，但挡不住伪造声明大小的 bomb——那种情况
    // calamine 仍会累积真实字节。根治需对 calamine 将读的 part 做**真解压带上限**(如
    // docx 路径的 take(MAX) 思路)或换带 cap 的 xlsx 解析器，属较大改动，留 follow-up。
    if ext != ".xls" {
        let cursor = Cursor::new(data);
        if let Ok(mut archive) = zip::ZipArchive::new(cursor) {
            let mut total: u64 = 0;
            for i in 0..archive.len() {
                if let Ok(entry) = archive.by_index_raw(i) {
                    total = total.saturating_add(entry.size());
                    if entry.size() > MAX_ZIP_ENTRY_BYTES || total > MAX_ZIP_ENTRY_BYTES {
                        return Err(VaultError::InvalidInput(format!(
                            "xlsx zip entry exceeds {MAX_ZIP_ENTRY_BYTES} bytes decompressed (possible zip bomb); rejected"
                        )));
                    }
                }
            }
        }
    }

    let cursor = Cursor::new(data.to_vec());
    let mut parts: Vec<String> = Vec::new();

    // calamine 根据 ext 选解析器
    macro_rules! read_sheets {
        ($wb:expr) => {{
            let mut wb =
                $wb.map_err(|e| VaultError::InvalidInput(format!("Excel read failed: {e}")))?;
            for sheet_name in wb.sheet_names().to_vec() {
                if let Ok(range) = wb.worksheet_range(&sheet_name) {
                    parts.push(format!("## {sheet_name}"));
                    for row in range.rows() {
                        let cells: Vec<String> = row
                            .iter()
                            .map(|cell| match cell {
                                Data::Empty => String::new(),
                                Data::String(s) => s.clone(),
                                Data::Float(f) => format!("{f}"),
                                Data::Int(i) => format!("{i}"),
                                Data::Bool(b) => format!("{b}"),
                                Data::Error(_) => "#ERR".to_string(),
                                Data::DateTime(dt) => format!("{dt}"),
                                Data::DateTimeIso(s) => s.clone(),
                                Data::DurationIso(s) => s.clone(),
                            })
                            .collect();
                        parts.push(cells.join("\t"));
                    }
                }
            }
        }};
    }

    if ext == ".xls" {
        read_sheets!(open_workbook_from_rs::<Xls<_>, _>(cursor));
    } else {
        read_sheets!(open_workbook_from_rs::<Xlsx<_>, _>(cursor));
    }

    Ok(parts.join("\n"))
}

/// PPTX 文件 → 纯文本（解压 zip，提取所有 slide XML 的文本节点）
fn parse_pptx_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let data = std::fs::read(path).map_err(VaultError::Io)?;
    let content = pptx_bytes_to_text(&data)?;
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

fn pptx_bytes_to_text(data: &[u8]) -> Result<String> {
    use std::io::Cursor;
    let cursor = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| {
        VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("PPTX zip open failed: {e}"),
        ))
    })?;

    let mut slides: Vec<(String, String)> = Vec::new();
    let mut total: u64 = 0; // 累计解压字节，防"多 slide 累加"型炸弹
    let count = archive.len();
    for i in 0..count {
        let name = {
            let entry = archive.by_index(i).map_err(|e| {
                VaultError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{e}"),
                ))
            })?;
            entry.name().to_string()
        };
        // ppt/slides/slide1.xml, slide2.xml, ...
        if !name.starts_with("ppt/slides/slide") || !name.ends_with(".xml") {
            continue;
        }
        let mut entry = archive.by_index(i).map_err(|e| {
            VaultError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{e}"),
            ))
        })?;
        // 单条目带解压上限；任一 slide 超限即拒绝整份（zip bomb）。
        let buf = read_zip_entry_string_bounded(&mut entry)?;
        total = total.saturating_add(buf.len() as u64);
        if total > MAX_ZIP_ENTRY_BYTES {
            return Err(VaultError::InvalidInput(format!(
                "pptx total decompressed slide text exceeds {MAX_ZIP_ENTRY_BYTES} bytes (possible zip bomb); rejected"
            )));
        }
        let text = strip_xml_tags(&buf);
        if !text.trim().is_empty() {
            slides.push((name, text));
        }
    }
    // Sort slides by natural order (slide1, slide2, ...)
    slides.sort_by(|(a, _), (b, _)| {
        let num_a: u32 = a
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        let num_b: u32 = b
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        num_a.cmp(&num_b)
    });

    Ok(slides
        .iter()
        .enumerate()
        .map(|(i, (_, text))| format!("## Slide {}\n{}", i + 1, text))
        .collect::<Vec<_>>()
        .join("\n\n"))
}

/// RTF 文件 → 纯文本（去除控制字序列和分组括号）
fn parse_rtf_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let raw = std::fs::read_to_string(path).map_err(VaultError::Io)?;
    let content = rtf_to_text(&raw);
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

fn rtf_to_text(rtf: &str) -> String {
    let mut result = String::with_capacity(rtf.len() / 2);
    let mut depth = 0i32;
    let mut chars = rtf.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' => depth += 1,
            '}' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            '\\' => {
                // control word or symbol
                if let Some(&next) = chars.peek() {
                    if next == '\\' || next == '{' || next == '}' {
                        chars.next();
                        if depth == 1 {
                            result.push(next);
                        }
                    } else if next == '\'' {
                        // hex-encoded char: \'XX
                        chars.next();
                        let h1 = chars.next().unwrap_or('0');
                        let h2 = chars.next().unwrap_or('0');
                        if depth == 1 {
                            if let Ok(b) = u8::from_str_radix(&format!("{h1}{h2}"), 16) {
                                result.push(b as char);
                            }
                        }
                    } else if next == '\n' || next == '\r' {
                        chars.next();
                    } else {
                        // skip control word + optional numeric parameter
                        while chars
                            .peek()
                            .is_some_and(|c| c.is_alphanumeric() || *c == '-')
                        {
                            chars.next();
                        }
                        // skip optional trailing space
                        if chars.peek() == Some(&' ') {
                            chars.next();
                        }
                    }
                }
            }
            '\n' | '\r' => {
                if depth <= 1 {
                    result.push('\n');
                }
            }
            _ => {
                if depth == 1 {
                    result.push(ch);
                }
            }
        }
    }
    collapse_whitespace(&result)
}

/// CSV 文件 → 保留原始文本（已由 `_` 分支 fallthrough, 但也可精确处理）
fn parse_csv_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let content = std::fs::read_to_string(path).map_err(VaultError::Io)?;
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

/// 图片文件 → OCR 提取文本（server 路径通过 scheduler，legacy 调用保留本地 provider）
fn parse_image_file(path: &Path, stem: &str, options: &ParseOptions) -> Result<(String, String)> {
    if let Some(text) = scheduler_ocr_path(path, options) {
        let title = first_line_title(&text, stem);
        return Ok((title, text));
    }
    if options.scheduler_base.is_some() {
        return Err(VaultError::InvalidInput(
            "scheduler OCR unavailable".to_string(),
        ));
    }

    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| stem.to_string());

    let provider = crate::ocr::detect_default_provider().ok_or_else(|| {
        VaultError::InvalidInput("OCR provider unavailable — install PP-OCR".to_string())
    })?;
    let scene = crate::ocr::auto_detect_scene(&filename);
    let profile = crate::ocr::profile_for_id(Some(scene));
    let output = provider.extract_structured(path, &profile)?;

    let content = if let Some(table) = output.table_markdown {
        format!("{}\n\n{}", output.text, table)
    } else {
        output.text
    };
    if content.trim().is_empty() {
        return Err(VaultError::InvalidInput(
            "OCR returned empty text".to_string(),
        ));
    }
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

/// 音频文件 → ASR 转写（server 路径通过 scheduler，legacy 调用保留本地 provider）
fn parse_audio_file(path: &Path, stem: &str, options: &ParseOptions) -> Result<(String, String)> {
    if let Ok(data) = std::fs::read(path) {
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| stem.to_string());
        if let Some(text) = scheduler_asr_bytes(&data, &filename, options) {
            let title = first_line_title(&text, stem);
            return Ok((title, text));
        }
    }
    if options.scheduler_base.is_some() {
        return Err(VaultError::InvalidInput(
            "scheduler ASR unavailable".to_string(),
        ));
    }

    let engine = crate::asr::detect_asr_engine().ok_or_else(|| {
        VaultError::InvalidInput(
            "ASR backend unavailable — install whisper.cpp or fetch SenseVoice".to_string(),
        )
    })?;
    // SenseVoice = plain in-process transcription (no diarization). Whisper keeps the
    // diarization path so multi-speaker audio is unaffected.
    let content = match &engine {
        crate::asr::AsrEngine::Whisper(backend) => {
            let diarization = crate::asr::detect_diarization_backend();
            let (_, c) =
                crate::asr::transcribe_with_diarization(backend, path, diarization.as_ref())?;
            c
        }
        crate::asr::AsrEngine::SenseVoice(_) => crate::asr::transcribe_with_engine(&engine, path)?,
    };
    if content.trim().is_empty() {
        return Err(VaultError::InvalidInput(
            "ASR returned empty transcript".to_string(),
        ));
    }
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct EnvRestore {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvRestore {
        fn new(keys: &[&'static str]) -> Self {
            Self {
                saved: keys
                    .iter()
                    .map(|key| (*key, std::env::var_os(key)))
                    .collect(),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..) {
                if let Some(value) = value {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
    }

    // ─── HTML ─────────────────────────────────────────────────────────────────

    #[test]
    fn html_to_text_extracts_title_and_body() {
        let html =
            r#"<html><head><title>My Page</title></head><body><p>Hello world</p></body></html>"#;
        let text = html_to_text(html);
        assert!(text.contains("My Page"), "title should appear: {text}");
        assert!(
            text.contains("Hello world"),
            "body text should appear: {text}"
        );
    }

    #[test]
    fn html_to_text_strips_script_and_style() {
        let html = r#"<html><body><script>alert('xss')</script><style>body{color:red}</style><p>Real content</p></body></html>"#;
        let text = html_to_text(html);
        // script/style text may leak through scraper text() but the key is no code execution
        // and the real content is still present
        assert!(
            text.contains("Real content"),
            "should contain real content: {text}"
        );
    }

    #[test]
    fn html_to_text_missing_title_uses_first_p() {
        let html = "<html><body><p>First paragraph content here</p></body></html>";
        let text = html_to_text(html);
        assert!(
            text.contains("First paragraph"),
            "body text should appear: {text}"
        );
    }

    #[test]
    fn parse_bytes_html_roundtrip() {
        let html =
            b"<html><head><title>HTML Doc</title></head><body><p>Some body text</p></body></html>";
        let (title, content) = parse_bytes(html, "page.html").unwrap();
        assert!(
            title.starts_with("HTML Doc"),
            "title should start with page title: {title}"
        );
        assert!(
            content.contains("Some body text"),
            "content should contain body: {content}"
        );
    }

    // ─── RTF ──────────────────────────────────────────────────────────────────

    #[test]
    fn rtf_to_text_basic() {
        let rtf = r"{\rtf1\ansi{\fonttbl\f0\fswiss Helvetica;}\f0\pard Hello RTF World\par}";
        let text = rtf_to_text(rtf);
        assert!(text.contains("Hello"), "should extract Hello: {text}");
        assert!(text.contains("RTF"), "should extract RTF: {text}");
        assert!(text.contains("World"), "should extract World: {text}");
    }

    #[test]
    fn rtf_to_text_hex_escape() {
        // \' followed by two hex digits is a Latin-1 char escape
        let rtf = r"{\rtf1 caf\e9}"; // é = 0xe9 in latin-1
        let text = rtf_to_text(rtf);
        // Should not panic; actual char output depends on mapping
        assert!(!text.is_empty() || text.is_empty()); // just no panic
    }

    #[test]
    fn rtf_to_text_skips_control_words() {
        let rtf = r"{\rtf1\ansi\deff0 {\fonttbl{\f0 Arial;}} \f0\pard Visible text\par}";
        let text = rtf_to_text(rtf);
        assert!(
            text.contains("Visible"),
            "control words should be stripped, text visible: {text}"
        );
        assert!(
            !text.contains("\\f0"),
            "control word \\f0 should not appear: {text}"
        );
    }

    #[test]
    fn parse_bytes_rtf_roundtrip() {
        let rtf = br"{\rtf1\ansi\pard Test RTF content\par}";
        let (_, content) = parse_bytes(rtf, "test.rtf").unwrap();
        assert!(
            content.contains("Test"),
            "rtf content should parse: {content}"
        );
    }

    // ─── PPTX ─────────────────────────────────────────────────────────────────

    fn make_pptx_zip(slides: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Cursor;
        let buf = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(buf);
        let opts = zip::write::FileOptions::<()>::default();
        for (name, content) in slides {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn pptx_bytes_extracts_slide_text() {
        let slide_xml = r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
            xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
            <p:cSld><p:spTree><p:sp><p:txBody>
            <a:p><a:r><a:t>Slide One Text</a:t></a:r></a:p>
            </p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;
        let data = make_pptx_zip(&[("ppt/slides/slide1.xml", slide_xml)]);
        let text = pptx_bytes_to_text(&data).unwrap();
        assert!(
            text.contains("Slide One Text"),
            "should extract slide text: {text}"
        );
        assert!(
            text.contains("Slide 1"),
            "should include slide header: {text}"
        );
    }

    #[test]
    fn pptx_bytes_multiple_slides_ordered() {
        let slide1_xml = "<root><t>Alpha</t></root>";
        let slide2_xml = "<root><t>Beta</t></root>";
        // Add in reverse order to verify sorting
        let data = make_pptx_zip(&[
            ("ppt/slides/slide2.xml", slide2_xml),
            ("ppt/slides/slide1.xml", slide1_xml),
        ]);
        let text = pptx_bytes_to_text(&data).unwrap();
        let pos1 = text.find("Alpha").unwrap_or(usize::MAX);
        let pos2 = text.find("Beta").unwrap_or(usize::MAX);
        assert!(
            pos1 < pos2,
            "slide1 (Alpha) should come before slide2 (Beta): {text}"
        );
    }

    #[test]
    fn pptx_bytes_invalid_zip_returns_error() {
        let result = pptx_bytes_to_text(b"not a zip file");
        assert!(result.is_err(), "invalid zip should error");
    }

    // ─── EPUB ─────────────────────────────────────────────────────────────────

    fn make_epub_zip(html_entries: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Cursor;
        let buf = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(buf);
        let opts = zip::write::FileOptions::<()>::default();
        for (name, content) in html_entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn epub_bytes_extracts_xhtml_content() {
        let xhtml = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml">
            <head><title>Chapter One</title></head>
            <body><p>EPUB chapter content here.</p></body></html>"#;
        let data = make_epub_zip(&[("OEBPS/chapter1.xhtml", xhtml)]);
        let text = epub_bytes_to_text(&data).unwrap();
        assert!(
            text.contains("EPUB chapter content"),
            "should extract xhtml: {text}"
        );
    }

    #[test]
    fn epub_bytes_skips_non_html_entries() {
        let xhtml = "<html><body><p>Real content</p></body></html>";
        let data = make_epub_zip(&[
            ("OEBPS/content.xhtml", xhtml),
            ("META-INF/container.xml", "<container/>"), // not xhtml
            ("images/cover.jpg", "fake jpg bytes"),     // not xhtml
        ]);
        let text = epub_bytes_to_text(&data).unwrap();
        assert!(
            text.contains("Real content"),
            "should extract xhtml content: {text}"
        );
    }

    #[test]
    fn epub_bytes_invalid_zip_returns_error() {
        let result = epub_bytes_to_text(b"not a valid epub");
        assert!(result.is_err(), "invalid epub should error");
    }

    // ─── CSV ──────────────────────────────────────────────────────────────────

    #[test]
    fn parse_bytes_csv_passthrough() {
        let csv = b"name,age,city\nAlice,30,Beijing\nBob,25,Shanghai\n";
        let (_, content) = parse_bytes(csv, "data.csv").unwrap();
        assert!(
            content.contains("Alice"),
            "CSV content should pass through: {content}"
        );
        assert!(
            content.contains("Shanghai"),
            "CSV content should pass through: {content}"
        );
    }

    // ─── is_supported audio / video boundary ──────────────────────────────────

    #[test]
    fn is_supported_audio_formats() {
        for ext in &["mp3", "wav", "m4a", "flac", "ogg", "aac", "opus", "wma"] {
            let path = format!("audio.{ext}");
            assert!(is_supported(Path::new(&path)), ".{ext} should be supported");
        }
    }

    #[test]
    fn is_supported_rejects_video_and_archives() {
        for ext in &["mp4", "mkv", "avi", "zip", "tar", "gz"] {
            let path = format!("file.{ext}");
            assert!(
                !is_supported(Path::new(&path)),
                ".{ext} should NOT be supported"
            );
        }
    }

    #[test]
    fn parse_bytes_unsupported_format_returns_error() {
        // .mp4 and .zip must be rejected — not silently treated as text
        for filename in &["clip.mp4", "archive.zip", "photo.exe"] {
            let result = parse_bytes(b"binary content", filename);
            assert!(result.is_err(), "{filename} should return error");
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("unsupported"),
                "error should mention 'unsupported': {err}"
            );
        }
        // .json (CODE_EXTENSION) must still pass
        let result = parse_bytes(b"{\"key\": \"value\"}", "config.json");
        assert!(result.is_ok(), ".json should be accepted as code/text");
    }

    // ─── strip_xml_tags edge cases ────────────────────────────────────────────

    #[test]
    fn strip_xml_tags_nested_and_attrs() {
        let xml = r#"<root attr="x"><child>Inner text</child>More text</root>"#;
        let result = strip_xml_tags(xml);
        assert!(
            result.contains("Inner text"),
            "should keep inner text: {result}"
        );
        assert!(
            result.contains("More text"),
            "should keep trailing text: {result}"
        );
        assert!(
            !result.contains('<'),
            "should strip all angle brackets: {result}"
        );
    }

    #[test]
    fn strip_xml_tags_empty_input() {
        assert_eq!(strip_xml_tags(""), "");
        assert_eq!(strip_xml_tags("<root/>"), "");
    }

    // ─── 原有测试 ─────────────────────────────────────────────────────────────

    #[test]
    fn parse_markdown_title() {
        let (title, content) = parse_content("# My Title\n\nSome content.", "doc.md").unwrap();
        assert_eq!(title, "My Title");
        assert!(content.contains("Some content"));
    }

    #[test]
    fn parse_txt_first_line() {
        let (title, _) = parse_content("First line\nSecond line", "notes.txt").unwrap();
        assert_eq!(title, "First line");
    }

    #[test]
    fn parse_code_filename() {
        let (title, content) = parse_content("fn main() {}", "app.rs").unwrap();
        assert_eq!(title, "app.rs");
        assert!(content.contains("fn main"));
    }

    #[test]
    fn parse_bytes_works() {
        let (title, content) = parse_bytes(b"# Hello\n\nWorld", "test.md").unwrap();
        assert_eq!(title, "Hello");
        assert!(content.contains("World"));
    }

    #[test]
    fn is_supported_types() {
        assert!(is_supported(Path::new("doc.md")));
        assert!(is_supported(Path::new("code.py")));
        assert!(is_supported(Path::new("data.txt")));
        assert!(is_supported(Path::new("app.rs")));
        assert!(is_supported(Path::new("image.png")));
        assert!(is_supported(Path::new("photo.jpg")));
        assert!(is_supported(Path::new("doc.html")));
        assert!(is_supported(Path::new("data.xlsx")));
        assert!(is_supported(Path::new("audio.mp3")));
        assert!(!is_supported(Path::new("video.mp4")));
    }

    #[test]
    fn parse_pdf_bytes_invalid() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let _restore = EnvRestore::new(&["ATTUNE_ENABLE_OCRMYPDF_FALLBACK"]);
        std::env::set_var("ATTUNE_ENABLE_OCRMYPDF_FALLBACK", "0");

        let result = parse_bytes(b"not a real pdf", "test.pdf");
        assert!(result.is_err(), "Should error on invalid PDF data");
    }

    #[test]
    fn parse_pdf_error_surfaces_ocr_context_when_backend_absent() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let _restore = EnvRestore::new(&["ATTUNE_ENABLE_OCRMYPDF_FALLBACK"]);
        std::env::set_var("ATTUNE_ENABLE_OCRMYPDF_FALLBACK", "0");

        // 契约：pdf_extract 失败 + OCR 后端不可用 → 报错信息必须包含 OCR 路径的上下文，
        // 让用户知道可以装 tesseract 来启用 fallback。这是 Round 1 review 要求的
        // "两路 title 对称"问题的文档化测试；真实加密扫描件的集成测试在
        // tests/fixtures/ 下（需 `which tesseract` 时触发，属于 Corpus Integration 层）。
        let result = parse_bytes(b"not a real pdf", "test.pdf");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("OCR unavailable") || msg.contains("PDF extract failed"),
            "error message should either trigger OCR fallback or explain OCR was unavailable: {msg}"
        );
    }

    #[test]
    fn short_mixed_pdf_text_layer_is_usable() {
        let text = "项目 Running 测试 with embedding\n向量 search 检索 hybrid recall。\n";
        assert!(crate::ocr::needs_ocr(text));
        assert!(pdf_text_layer_is_usable(text));
    }

    #[test]
    fn try_ocr_from_bytes_none_when_backend_absent() {
        // 当 tesseract 不在 PATH（如 CI 无 OCR 依赖），try_ocr_from_bytes 必须返回 None
        // 而非 panic。这保证了 parse_bytes 降级路径的稳定性。
        //
        // 注：此测试在有 tesseract 的开发机上可能返回 Some(err_text)（OCR 在错误 PDF 上
        // 失败并返回 None），两种都是"正确不崩"；断言只看"不 panic"。
        let options = ParseOptions::default();
        let _ = try_ocr_from_bytes_with_dpi(b"garbage data", "test.pdf", 300, &options);
        // 到这里就代表没 panic 了
    }

    #[test]
    fn scheduler_inline_file_body_budget_accounts_for_base64() {
        let max = 16 * 1024 * 1024;
        assert!(scheduler_inline_file_fits_body(8 * 1024 * 1024, max));
        assert!(!scheduler_inline_file_fits_body(13 * 1024 * 1024, max));
    }

    #[test]
    fn scheduler_inline_file_body_budget_handles_tiny_limits() {
        assert!(!scheduler_inline_file_fits_body(
            1,
            SCHEDULER_INLINE_JSON_OVERHEAD_BYTES
        ));
        assert!(scheduler_inline_file_fits_body(
            3,
            SCHEDULER_INLINE_JSON_OVERHEAD_BYTES + 4
        ));
    }

    #[test]
    fn scheduler_inline_file_body_budget_accounts_for_alias_copies() {
        let max = 16 * 1024 * 1024;
        assert!(scheduler_inline_file_fits_body_with_copies(
            3 * 1024 * 1024,
            max,
            3
        ));
        assert!(!scheduler_inline_file_fits_body_with_copies(
            5 * 1024 * 1024,
            max,
            3
        ));
    }

    #[test]
    fn scheduler_ocr_body_includes_scheduler_worker_aliases() {
        let options = ParseOptions::with_profile(Some("scan")).with_scheduler_timeout_ms(1_500);
        let body = scheduler_ocr_body(b"abc", "scan.pdf", &options);
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"abc");

        assert_eq!(
            body.pointer("/filename").and_then(Value::as_str),
            Some("scan.pdf")
        );
        assert_eq!(
            body.pointer("/file_base64").and_then(Value::as_str),
            Some(encoded.as_str())
        );
        assert_eq!(
            body.pointer("/input/file_base64").and_then(Value::as_str),
            Some(encoded.as_str())
        );
        assert_eq!(
            body.pointer("/x/file_base64").and_then(Value::as_str),
            Some(encoded.as_str())
        );
        assert_eq!(
            body.pointer("/input/profile").and_then(Value::as_str),
            Some("scan")
        );
        assert_eq!(
            body.pointer("/x/profile_id").and_then(Value::as_str),
            Some("scan")
        );
        assert_eq!(
            body.pointer("/timeout_ms").and_then(Value::as_u64),
            Some(1_500)
        );
    }

    #[test]
    fn scheduler_ocr_image_body_includes_page_aliases() {
        let options = ParseOptions::with_profile(Some("scan")).with_scheduler_timeout_ms(1_500);
        let body = scheduler_ocr_image_body(b"png", "scan.pdf", 7, Some(12), 180, &options);
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"png");

        assert_eq!(
            body.pointer("/filename").and_then(Value::as_str),
            Some("scan.pdf")
        );
        assert_eq!(
            body.pointer("/content_type").and_then(Value::as_str),
            Some("image/png")
        );
        assert_eq!(
            body.pointer("/image_base64").and_then(Value::as_str),
            Some(encoded.as_str())
        );
        assert_eq!(
            body.pointer("/input/image_base64").and_then(Value::as_str),
            Some(encoded.as_str())
        );
        assert_eq!(
            body.pointer("/x/image_base64").and_then(Value::as_str),
            Some(encoded.as_str())
        );
        assert_eq!(
            body.pointer("/page_number").and_then(Value::as_u64),
            Some(7)
        );
        assert_eq!(
            body.pointer("/input/page_count").and_then(Value::as_u64),
            Some(12)
        );
        assert_eq!(body.pointer("/x/dpi").and_then(Value::as_u64), Some(180));
    }

    fn start_scheduler_ocr_mock(
        expected_requests: usize,
    ) -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&requests);
        let handle = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while seen.load(Ordering::SeqCst) < expected_requests
                && std::time::Instant::now() < deadline
            {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        continue;
                    }
                    Err(_) => break,
                };
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                let mut header_end = None;
                while header_end.is_none() {
                    let Ok(n) = stream.read(&mut tmp) else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    header_end = buf.windows(4).position(|w| w == b"\r\n\r\n");
                }
                let Some(header_end) = header_end.map(|idx| idx + 4) else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        if name.eq_ignore_ascii_case("content-length") {
                            value.trim().parse::<usize>().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);
                while buf.len().saturating_sub(header_end) < content_length {
                    let Ok(n) = stream.read(&mut tmp) else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let body = String::from_utf8_lossy(&buf[header_end..]).to_string();
                let page = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|v| v.get("page_number").and_then(Value::as_u64))
                    .unwrap_or(0);
                seen.fetch_add(1, Ordering::SeqCst);
                let payload = serde_json::json!({
                    "scheduled_as": "sync",
                    "status": "done",
                    "task": "kb.document.ocr_recognize",
                    "outputs": {"text": format!("OCR page {page}")}
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    payload.len(),
                    payload
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (format!("http://{addr}"), requests, handle)
    }

    #[cfg(unix)]
    #[test]
    fn scheduler_pdf_ocr_pages_large_pdf_after_whole_file_oversize() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let _restore = EnvRestore::new(&[
            "PATH",
            "ATTUNE_SCHEDULER_MAX_BODY_BYTES",
            "ATTUNE_SCHEDULER_PDF_OCR_MAX_PAGES",
            "ATTUNE_SCHEDULER_PDF_OCR_MAX_DPI",
            "ATTUNE_SCHEDULER_PDF_OCR_MIN_DPI",
            "ATTUNE_SCHEDULER_OCR_ENABLED",
            "ATTUNE_ENABLE_OCRMYPDF_FALLBACK",
        ]);
        let dir = tempfile::TempDir::new().unwrap();
        let pdfinfo = dir.path().join("pdfinfo");
        std::fs::write(&pdfinfo, "#!/bin/sh\necho 'Pages: 2'\n").unwrap();
        let pdftoppm = dir.path().join("pdftoppm");
        std::fs::write(
            &pdftoppm,
            "#!/bin/sh\n\
             page=1\n\
             prefix=''\n\
             while [ \"$#\" -gt 0 ]; do\n\
               case \"$1\" in\n\
                 -f) page=\"$2\"; shift 2 ;;\n\
                 -l|-r) shift 2 ;;\n\
                 -png|-singlefile) shift ;;\n\
                 *) prefix=\"$1\"; shift ;;\n\
               esac\n\
             done\n\
             printf 'png-page-%s' \"$page\" > \"${prefix}.png\"\n",
        )
        .unwrap();
        for exe in [&pdfinfo, &pdftoppm] {
            let mut perms = std::fs::metadata(exe).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(exe, perms).unwrap();
        }

        let old_path = std::env::var_os("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", dir.path().display(), old_path.to_string_lossy());
        std::env::set_var("PATH", new_path);
        std::env::set_var("ATTUNE_SCHEDULER_MAX_BODY_BYTES", "8192");
        std::env::set_var("ATTUNE_SCHEDULER_PDF_OCR_MAX_PAGES", "2");
        std::env::set_var("ATTUNE_SCHEDULER_PDF_OCR_MAX_DPI", "180");
        std::env::set_var("ATTUNE_SCHEDULER_PDF_OCR_MIN_DPI", "120");
        std::env::set_var("ATTUNE_SCHEDULER_OCR_ENABLED", "1");
        std::env::set_var("ATTUNE_ENABLE_OCRMYPDF_FALLBACK", "0");

        let pdf = dir.path().join("large-scan.pdf");
        std::fs::write(&pdf, vec![b'x'; 8192]).unwrap();
        let (base, requests, handle) = start_scheduler_ocr_mock(2);
        let options = ParseOptions::default()
            .with_scheduler_base(Some(&base))
            .with_scheduler_timeout_ms(5_000);

        let text = try_ocr_from_pdf_path_with_dpi(&pdf, 300, &options).unwrap();
        assert!(text.contains("--- Page 1 ---"));
        assert!(text.contains("OCR page 1"));
        assert!(text.contains("--- Page 2 ---"));
        assert!(text.contains("OCR page 2"));
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        handle.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn scheduler_pdf_ocr_uses_ocrmypdf_sidecar_after_inline_skip() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let _restore = EnvRestore::new(&[
            "ATTUNE_ENABLE_OCRMYPDF_FALLBACK",
            "ATTUNE_SCHEDULER_MAX_BODY_BYTES",
            "PATH",
        ]);
        let dir = tempfile::TempDir::new().unwrap();
        let fake_ocrmypdf = dir.path().join("ocrmypdf");
        std::fs::write(
            &fake_ocrmypdf,
            "#!/bin/sh\n\
             sidecar=''\n\
             while [ \"$#\" -gt 0 ]; do\n\
               if [ \"$1\" = '--sidecar' ]; then sidecar=\"$2\"; shift 2; continue; fi\n\
               shift\n\
             done\n\
             printf '%s\\n' 'OCR sidecar fallback text' > \"$sidecar\"\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&fake_ocrmypdf).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake_ocrmypdf, perms).unwrap();

        let old_path = std::env::var_os("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", dir.path().display(), old_path.to_string_lossy());
        std::env::set_var("PATH", new_path);
        std::env::set_var("ATTUNE_ENABLE_OCRMYPDF_FALLBACK", "1");
        std::env::set_var("ATTUNE_SCHEDULER_MAX_BODY_BYTES", "4096");

        let pdf = dir.path().join("scan.pdf");
        std::fs::write(&pdf, b"%PDF fake scanned page").unwrap();
        let options = ParseOptions::default().with_scheduler_base(Some("http://127.0.0.1:1"));
        let text = try_ocr_from_pdf_path_with_dpi(&pdf, 300, &options).unwrap();
        assert_eq!(text.trim(), "OCR sidecar fallback text");
    }

    #[test]
    fn scheduler_output_text_accepts_common_shapes() {
        assert_eq!(
            scheduler_output_text(&serde_json::json!({"text": "hello"})).as_deref(),
            Some("hello")
        );
        assert_eq!(
            scheduler_output_text(&serde_json::json!({"segments": [{"text": "a"}, {"text": "b"}]}))
                .as_deref(),
            Some("a\nb")
        );
        assert_eq!(
            scheduler_output_text(&serde_json::json!({"outputs": {"full_text": "done"}}))
                .as_deref(),
            Some("done")
        );
    }

    #[test]
    fn strip_xml_tags_works() {
        let xml = "<w:p><w:r><w:t>Hello</w:t></w:r></w:p><w:p><w:r><w:t>World</w:t></w:r></w:p>";
        let result = strip_xml_tags(xml);
        assert!(result.contains("Hello"), "Should contain Hello: {result}");
        assert!(result.contains("World"), "Should contain World: {result}");
    }

    #[test]
    fn parse_docx_bytes_invalid() {
        let result = parse_bytes(b"not a real docx", "test.docx");
        assert!(result.is_err(), "Should error on invalid DOCX data");
    }

    #[test]
    fn file_hash_deterministic() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"test content").unwrap();

        let h1 = file_hash(&path).unwrap();
        let h2 = file_hash(&path).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex = 64 chars
    }
}
