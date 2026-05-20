# Office 办公助理入口 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** v0.7.1 minor release — 把 attune-core 已实现的 OCR (PP-OCRv5 mobile) + ASR (whisper.cpp + diarization) 首次暴露成产品化"办公助理"入口，含 5 个结构化 OCR scene + ASR 异步转写 + L1/L2 稳定性保障。

**Architecture:** REST 同步 OCR + 异步 ASR job + WebSocket 进度推送；零 LLM 字段抽取（正则锚点 + bbox 邻近 + 校验）；tagged union schema（`structured.schema = *_v1`）；个人助手语义（不限并发不 reject，信号量门控 + FIFO 排队）；结果不入 vault。

**Tech Stack:** Rust (axum + tokio + serde tagged enum) / TypeScript Preact UI / clap CLI / proptest + golden YAML 回归测试。

**Spec reference:** `docs/superpowers/specs/2026-05-20-office-helper-design.md` (commit 81a7dae).

**Deadline:** 2026-05-25 (6 天 wall-clock，硬约束，scope 可降不延期)。

---

## Phase D1 (5/20 Tue) — REST 骨架 + A 档输出

目标：5 个 OCR scene 全返 A 档（lines + bbox，structured=null）；ASR async job queue + WS 框架；CLI 桩；happy path 集成测试绿。

### Task D1.1: 扩展 `OcrOutput` 加 `lines: Option<Vec<RawLine>>`

**Files:**
- Modify: `rust/crates/attune-core/src/ocr/mod.rs`
- Test: `rust/crates/attune-core/src/ocr/mod.rs` (内嵌 `#[cfg(test)]`)

- [ ] **Step 1: 加 RawLine + BBox 公共类型**

修改 `rust/crates/attune-core/src/ocr/mod.rs` 在 `OcrOutput` 上方加：

```rust
/// 单行 OCR 输出（含 bbox 坐标，办公助理结构化抽取需要）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RawLine {
    pub text: String,
    pub bbox: BBox,
    pub confidence: f32,
}

/// 像素坐标 bbox（左上角 + 宽高）。
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct BBox {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}
```

修改 `OcrOutput` 加 `lines` 字段（向后兼容，Option）：

```rust
#[derive(Debug, Clone)]
pub struct OcrOutput {
    pub text: String,
    pub table_markdown: Option<String>,
    pub avg_confidence: Option<f32>,
    /// 行级 OCR 输出（含 bbox），用于 office helper 结构化抽取。
    /// `None` = provider 不支持（默认实现 / mock）；`Some` = PP-OCR 等真实 provider 填充。
    pub lines: Option<Vec<RawLine>>,
}
```

更新 `extract_structured` 默认实现（同文件第 59-62 行）：

```rust
fn extract_structured(&self, image_path: &Path, _profile: &OcrProfile) -> Result<OcrOutput> {
    let text = self.extract_text_from_image(image_path)?;
    Ok(OcrOutput { text, table_markdown: None, avg_confidence: None, lines: None })
}
```

- [ ] **Step 2: 修复 PpOcrProvider 填充 lines**

修改 `rust/crates/attune-core/src/ocr/ppocr.rs` 在 `extract_structured` 返回 `OcrOutput` 处（约 340 行附近）填充 `lines`。在已有 `text_blocks` 迭代后加：

```rust
let lines = result.text_blocks.iter()
    .filter(|b| !b.text.is_empty())
    .map(|b| {
        // box_points: [tl, tr, br, bl], Point { x: u32, y: u32 }
        let xs: Vec<u32> = b.box_points.iter().map(|p| p.x).collect();
        let ys: Vec<u32> = b.box_points.iter().map(|p| p.y).collect();
        let x = *xs.iter().min().unwrap_or(&0);
        let y = *ys.iter().min().unwrap_or(&0);
        let w = xs.iter().max().unwrap_or(&0).saturating_sub(x);
        let h = ys.iter().max().unwrap_or(&0).saturating_sub(y);
        super::RawLine {
            text: b.text.clone(),
            bbox: super::BBox { x, y, w, h },
            confidence: b.score,
        }
    })
    .collect::<Vec<_>>();
```

将其放进返回的 `OcrOutput { ..., lines: Some(lines) }`。

- [ ] **Step 3: 修复所有 OcrOutput 构造点**

`grep -rn "OcrOutput {" rust/crates/` 找到所有构造点，每处加 `lines: None`（或对 PP-OCR 加 `lines: Some(...)`）。

- [ ] **Step 4: 单测 — RawLine bbox 计算正确**

在 `rust/crates/attune-core/src/ocr/mod.rs` 末尾加：

```rust
#[cfg(test)]
mod office_types_tests {
    use super::*;

    #[test]
    fn raw_line_serde_roundtrip() {
        let l = RawLine { text: "hi".into(), bbox: BBox { x: 1, y: 2, w: 3, h: 4 }, confidence: 0.9 };
        let s = serde_json::to_string(&l).unwrap();
        let d: RawLine = serde_json::from_str(&s).unwrap();
        assert_eq!(d.text, "hi");
        assert_eq!(d.bbox.x, 1);
        assert!((d.confidence - 0.9).abs() < 1e-6);
    }
}
```

- [ ] **Step 5: cargo test 验证**

```bash
cd /data/company/project/attune
cargo test -p attune-core ocr::office_types_tests 2>&1 | tail -10
```
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add rust/crates/attune-core/src/ocr/
git commit -m "feat(ocr): expose RawLine + bbox in OcrOutput for office helper

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task D1.2: `office_job_queue.rs` — 内存 job state machine

**Files:**
- Create: `rust/crates/attune-core/src/office_job_queue.rs`
- Modify: `rust/crates/attune-core/src/lib.rs` (注册 module)
- Test: 内嵌 `#[cfg(test)]`

- [ ] **Step 1: 注册 module**

修改 `rust/crates/attune-core/src/lib.rs` 添加：

```rust
pub mod office_job_queue;
```

放在与 `pub mod asr;` 同一区块。

- [ ] **Step 2: 创建 module 文件**

创建 `rust/crates/attune-core/src/office_job_queue.rs`:

```rust
//! Office helper async job queue — in-memory state machine.
//!
//! 个人助手语义：不限并发，FIFO 排队，信号量门控防止资源踩踏。
//! 服务重启后所有 in-flight job 标 cancelled（不持久化）。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStage {
    Queued,
    LoadingModel,
    Transcribing,
    Diarizing,
    Postprocess,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobError {
    pub message: String,
    pub code: String,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub state: JobState,
    pub stage: JobStage,
    pub progress: f32,
    pub created_at: Instant,
    pub started_at: Option<Instant>,
    pub elapsed_ms: u64,
    pub eta_ms: Option<u64>,
    pub result_json: Option<String>,
    pub error: Option<JobError>,
    pub warnings: Vec<String>,
}

impl Job {
    pub fn new(id: String) -> Self {
        Self {
            id, state: JobState::Queued, stage: JobStage::Queued,
            progress: 0.0, created_at: Instant::now(), started_at: None,
            elapsed_ms: 0, eta_ms: None, result_json: None, error: None,
            warnings: vec![],
        }
    }
}

pub struct JobRegistry {
    jobs: Mutex<HashMap<String, Job>>,
}

impl JobRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { jobs: Mutex::new(HashMap::new()) })
    }

    pub fn insert(&self, job: Job) {
        let mut g = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        g.insert(job.id.clone(), job);
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        let g = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        g.get(id).cloned()
    }

    pub fn update<F: FnOnce(&mut Job)>(&self, id: &str, f: F) -> bool {
        let mut g = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(j) = g.get_mut(id) { f(j); true } else { false }
    }

    pub fn queue_position(&self, id: &str) -> usize {
        let g = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        g.values()
            .filter(|j| j.state == JobState::Queued && j.created_at < g.get(id).map(|x| x.created_at).unwrap_or_else(Instant::now))
            .count()
    }

    pub fn cancel_all_running(&self) {
        let mut g = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        for j in g.values_mut() {
            if matches!(j.state, JobState::Running | JobState::Queued) {
                j.state = JobState::Cancelled;
                j.warnings.push("server restarted, please resubmit".into());
            }
        }
    }
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self { jobs: Mutex::new(HashMap::new()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_roundtrip() {
        let r = JobRegistry::new();
        r.insert(Job::new("j1".into()));
        let j = r.get("j1").unwrap();
        assert_eq!(j.state, JobState::Queued);
    }

    #[test]
    fn update_changes_state() {
        let r = JobRegistry::new();
        r.insert(Job::new("j1".into()));
        assert!(r.update("j1", |j| j.state = JobState::Running));
        assert_eq!(r.get("j1").unwrap().state, JobState::Running);
    }

    #[test]
    fn cancel_all_running_marks_them() {
        let r = JobRegistry::new();
        let mut j = Job::new("j1".into());
        j.state = JobState::Running;
        r.insert(j);
        r.cancel_all_running();
        assert_eq!(r.get("j1").unwrap().state, JobState::Cancelled);
    }
}
```

- [ ] **Step 3: cargo test 验证**

```bash
cargo test -p attune-core office_job_queue::tests 2>&1 | tail -10
```
Expected: 3 tests PASS

- [ ] **Step 4: Commit**

```bash
git add rust/crates/attune-core/src/office_job_queue.rs rust/crates/attune-core/src/lib.rs
git commit -m "feat(core): add office_job_queue in-memory job state machine

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task D1.3: `routes/office.rs` — REST 端点骨架 + A 档输出

**Files:**
- Create: `rust/crates/attune-server/src/routes/office.rs`
- Modify: `rust/crates/attune-server/src/routes/mod.rs` (export module)
- Modify: `rust/crates/attune-server/src/state.rs` (加 office_jobs 字段)
- Modify: `rust/crates/attune-server/src/lib.rs` (注册路由)

- [ ] **Step 1: AppState 加 office_jobs**

修改 `rust/crates/attune-server/src/state.rs`，在 `use` 段加：
```rust
use attune_core::office_job_queue::JobRegistry;
```

在 `pub struct AppState { ... }` 内（vault 字段附近）加：
```rust
pub office_jobs: Arc<JobRegistry>,
```

在 `impl AppState { fn new() }` 内构造 `Self { ... }` 处加：
```rust
office_jobs: JobRegistry::new(),
```

- [ ] **Step 2: 创建 office.rs 骨架**

创建 `rust/crates/attune-server/src/routes/office.rs`:

```rust
//! /api/v1/office — 办公助理入口 (OCR 同步 + ASR 异步)。

use crate::state::SharedState;
use attune_core::ocr::{self, RawLine};
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize)]
pub struct OcrResponse {
    pub envelope_version: &'static str,
    pub profile: String,
    pub elapsed_ms: u64,
    pub engine: String,
    pub lines: Vec<RawLine>,
    pub structured: Option<serde_json::Value>,
}

fn err(code: &str, msg: &str, status: StatusCode) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(serde_json::json!({ "error": msg, "code": code })))
}

/// POST /api/v1/office/ocr
pub async fn post_ocr(
    State(_state): State<SharedState>,
    mut multipart: Multipart,
) -> Result<Json<OcrResponse>, (StatusCode, Json<serde_json::Value>)> {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    let mut profile: Option<String> = None;
    let mut id_card_subtype: Option<String> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e|
        err("invalid-input", &format!("multipart parse: {e}"), StatusCode::BAD_REQUEST))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                filename = field.file_name().map(|s| s.to_string());
                file_bytes = Some(field.bytes().await.map_err(|e|
                    err("invalid-input", &format!("file read: {e}"), StatusCode::BAD_REQUEST))?.to_vec());
            }
            "profile" => profile = Some(field.text().await.unwrap_or_default()),
            "id_card_subtype" => id_card_subtype = Some(field.text().await.unwrap_or_default()),
            _ => {}
        }
    }

    let bytes = file_bytes.ok_or_else(|| err("invalid-input", "file required", StatusCode::BAD_REQUEST))?;
    if bytes.is_empty() {
        return Err(err("empty-file", "file is empty", StatusCode::BAD_REQUEST));
    }
    let profile = profile.ok_or_else(|| err("invalid-input", "profile required", StatusCode::BAD_REQUEST))?;
    if profile == "id_card" && id_card_subtype.is_none() {
        return Err(err("id-card-subtype-required",
            "id_card_subtype required when profile=id_card", StatusCode::BAD_REQUEST));
    }

    // 检查 profile 合法性
    let allowed = ["document", "receipt", "table", "card", "id_card",
                   "screenshot", "ancient", "form", "contract"];
    if !allowed.contains(&profile.as_str()) {
        return Err(err("profile-not-found", &format!("unknown profile: {profile}"),
            StatusCode::NOT_FOUND));
    }

    let start = std::time::Instant::now();

    // 写到 tmp 文件 (PP-OCR provider 接受 path)
    let tmp = tempfile::NamedTempFile::new().map_err(|e|
        err("internal-error", &format!("tempfile: {e}"), StatusCode::INTERNAL_SERVER_ERROR))?;
    std::fs::write(tmp.path(), &bytes).map_err(|e|
        err("internal-error", &format!("write tmp: {e}"), StatusCode::INTERNAL_SERVER_ERROR))?;

    // 路由到 OCR provider — A 档先返 lines + bbox
    let provider = ocr::detect_default_provider()
        .ok_or_else(|| err("ocr-engine-failed", "no OCR provider available",
            StatusCode::INTERNAL_SERVER_ERROR))?;

    // 判断是 PDF 还是图片（按 filename 后缀）
    let is_pdf = filename.as_deref().map(|n| n.to_lowercase().ends_with(".pdf")).unwrap_or(false);

    let ocr_profile = ocr::profile_for_id(Some(&profile));
    let lines: Vec<RawLine> = if is_pdf {
        // PDF：现有 API 只返 text，A 档 PDF 路径 lines 为空（D1 接受）
        // PDF 完整 lines 输出在 D2 / D3 补
        vec![]
    } else {
        // 图片
        let out = provider.extract_structured(tmp.path(), &ocr_profile).map_err(|e|
            err("ocr-engine-failed", &format!("OCR: {e}"), StatusCode::INTERNAL_SERVER_ERROR))?;
        out.lines.unwrap_or_default()
    };

    Ok(Json(OcrResponse {
        envelope_version: "1",
        profile: profile.clone(),
        elapsed_ms: start.elapsed().as_millis() as u64,
        engine: provider.name().to_string(),
        lines,
        structured: None, // A 档先 null；D2 补
    }))
}

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
    pub max_speakers: Option<u32>,
}
fn default_language() -> String { "auto".into() }
fn default_model() -> String { "small".into() }

#[derive(Serialize)]
pub struct TranscribeResponse {
    pub job_id: String,
    pub ws_url: String,
}

/// POST /api/v1/office/transcribe — 提交 ASR job，立即返 job_id
pub async fn post_transcribe(
    State(state): State<SharedState>,
    Json(req): Json<TranscribeRequest>,
) -> Result<(StatusCode, Json<TranscribeResponse>), (StatusCode, Json<serde_json::Value>)> {
    let path = std::path::Path::new(&req.file_path);
    if !path.exists() {
        return Err(err("invalid-input", &format!("file not found: {}", req.file_path),
            StatusCode::BAD_REQUEST));
    }
    let job_id = format!("asr-{}", Uuid::new_v4().simple());
    let ws_url = format!("/api/v1/office/jobs/ws?job_id={job_id}");
    state.office_jobs.insert(attune_core::office_job_queue::Job::new(job_id.clone()));

    // D1 仅占位：实际 spawn worker 在 D1.4
    // TODO(D1.4): spawn tokio::task 跑 asr::transcribe_with_diarization 并更新 job

    Ok((StatusCode::ACCEPTED, Json(TranscribeResponse { job_id, ws_url })))
}

/// GET /api/v1/office/jobs/{job_id}
pub async fn get_job(
    State(state): State<SharedState>,
    Path(job_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let job = state.office_jobs.get(&job_id)
        .ok_or_else(|| err("not-found", "job not found", StatusCode::NOT_FOUND))?;
    let queue_pos = state.office_jobs.queue_position(&job_id);
    Ok(Json(serde_json::json!({
        "job_id": job.id,
        "state": job.state,
        "stage": job.stage,
        "queue_position": queue_pos,
        "progress": job.progress,
        "elapsed_ms": job.elapsed_ms,
        "eta_ms": job.eta_ms,
        "result": job.result_json.as_ref().and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()),
        "error": job.error,
        "warnings": job.warnings,
    })))
}

/// DELETE /api/v1/office/jobs/{job_id}
pub async fn delete_job(
    State(state): State<SharedState>,
    Path(job_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let job = state.office_jobs.get(&job_id)
        .ok_or_else(|| err("not-found", "job not found", StatusCode::NOT_FOUND))?;
    match job.state {
        attune_core::office_job_queue::JobState::Done =>
            Err(err("job-already-completed", "job already done", StatusCode::CONFLICT)),
        attune_core::office_job_queue::JobState::Cancelled =>
            Err(err("job-already-cancelled", "job already cancelled", StatusCode::CONFLICT)),
        _ => {
            state.office_jobs.update(&job_id, |j|
                j.state = attune_core::office_job_queue::JobState::Cancelled);
            Ok(StatusCode::NO_CONTENT)
        }
    }
}
```

- [ ] **Step 3: 注册 module + 路由**

修改 `rust/crates/attune-server/src/routes/mod.rs` 添加：
```rust
pub mod office;
```

修改 `rust/crates/attune-server/src/lib.rs`，在 `.route("/api/v1/ocr/profiles", ...)` 附近加：

```rust
.route("/api/v1/office/ocr",
    axum::routing::post(routes::office::post_ocr))
.route("/api/v1/office/transcribe",
    axum::routing::post(routes::office::post_transcribe))
.route("/api/v1/office/jobs/{job_id}",
    axum::routing::get(routes::office::get_job)
        .delete(routes::office::delete_job))
```

- [ ] **Step 4: 验证编译**

```bash
cargo build -p attune-server 2>&1 | tail -20
```
Expected: 编译通过（warnings 允许，errors 不允许）。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/routes/office.rs \
        rust/crates/attune-server/src/routes/mod.rs \
        rust/crates/attune-server/src/state.rs \
        rust/crates/attune-server/src/lib.rs
git commit -m "feat(server): scaffold /api/v1/office REST endpoints (A-layer stub)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task D1.4: ASR worker spawn + happy path 集成测试

**Files:**
- Modify: `rust/crates/attune-server/src/routes/office.rs` (实际 spawn worker)
- Create: `rust/crates/attune-server/tests/office_happy_path.rs`

- [ ] **Step 1: post_transcribe 内 spawn 异步 worker**

修改 `post_transcribe`，在 `state.office_jobs.insert(...)` 之后加：

```rust
let registry = state.office_jobs.clone();
let job_id_for_task = job_id.clone();
let file_path = req.file_path.clone();
let language = req.language.clone();
let diarization = req.diarization;

tokio::task::spawn_blocking(move || {
    use attune_core::office_job_queue::{JobState, JobStage};

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
                j.error = Some(attune_core::office_job_queue::JobError {
                    message: "ASR backend not available".into(),
                    code: "asr-engine-failed".into(),
                });
            });
            return;
        }
    };

    registry.update(&job_id_for_task, |j| j.stage = JobStage::Transcribing);

    let diar_backend = if diarization { attune_core::asr::detect_diarization_backend() } else { None };
    let (_full, segments) = match attune_core::asr::transcribe_with_diarization(
        &backend, std::path::Path::new(&file_path), &language, diar_backend.as_ref(),
    ) {
        Ok(r) => r,
        Err(e) => {
            registry.update(&job_id_for_task, |j| {
                j.state = JobState::Failed;
                j.error = Some(attune_core::office_job_queue::JobError {
                    message: e.to_string(),
                    code: "asr-engine-failed".into(),
                });
            });
            return;
        }
    };

    let result = serde_json::json!({
        "model": backend.model_name,
        "language_detected": backend.language,
        "segments": segments.iter().map(|s| serde_json::json!({
            "start_sec": s.start_sec,
            "end_sec": s.end_sec,
            "text": s.text,
            "speaker": s.speaker,
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
```

注意：`TranscriptSegment` 可能没有 `text` 字段名而是 `to_display()` — 先看 `attune-core/src/asr.rs:254` `TranscriptSegment` 实际定义：

```bash
grep -A 15 "pub struct TranscriptSegment" rust/crates/attune-core/src/asr.rs
```

按实际字段名（应是 `text` / `start_sec` / `end_sec` / `speaker`）调整。

- [ ] **Step 2: 创建 happy path 测试**

创建 `rust/crates/attune-server/tests/office_happy_path.rs`:

```rust
//! Office helper REST 端点 happy-path 烟测。
//! 仅测端点存在 + 错误码契约，不测实际 OCR/ASR 输出（那些走 golden gate）。

use attune_server::state::AppState;
use attune_server::build_router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;

fn make_app() -> axum::Router {
    let tmp = tempfile::tempdir().unwrap();
    let vault = attune_core::vault::Vault::open_or_create(tmp.path(), b"test-password-1234567890").unwrap();
    let state = AppState::new(vault, false);
    build_router(Arc::new(state))
}

#[tokio::test]
async fn ocr_missing_file_returns_400_invalid_input() {
    let app = make_app();
    let resp = app.oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/v1/office/ocr")
            .header("Content-Type", "multipart/form-data; boundary=X")
            .body(Body::from("--X--"))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], "invalid-input");
}

#[tokio::test]
async fn transcribe_missing_file_returns_400() {
    let app = make_app();
    let resp = app.oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/v1/office/transcribe")
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"file_path": "/nonexistent/file.mp3"}"#))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_unknown_job_returns_404() {
    let app = make_app();
    let resp = app.oneshot(
        Request::builder()
            .method("GET")
            .uri("/api/v1/office/jobs/no-such-id")
            .body(Body::empty())
            .unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 3: 跑测试**

```bash
cargo test -p attune-server --test office_happy_path 2>&1 | tail -10
```
Expected: 3 tests PASS（如失败检查 `AppState::new()` 签名 + `Vault::open_or_create` 签名是否对得上当前代码——可能要按现有 server 测试模板调整 helper）。

- [ ] **Step 4: Commit**

```bash
git add rust/crates/attune-server/
git commit -m "feat(office): ASR async worker spawn + happy-path smoke tests

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task D1.5: WS endpoint + CLI 桩

D1 收尾。

- [ ] **Step 1: WS endpoint** — `routes/office.rs` 加 `pub async fn ws_jobs(ws: WebSocketUpgrade, State, Query)` 端点，订阅 job 进度，每 500ms tick 一次发 `{type: "progress", ...}` 帧；job 终态发对应 `done/failed/cancelled` 帧；client 发 `{type: "cancel"}` 调 `state.office_jobs.update(...)` 设 `Cancelled`。

完整代码：

```rust
use axum::extract::{Query, WebSocketUpgrade};
use axum::extract::ws::{Message, WebSocket};
use axum::response::Response;

#[derive(Deserialize)]
pub struct WsQuery { pub job_id: String }

pub async fn ws_jobs(
    ws: WebSocketUpgrade,
    State(state): State<SharedState>,
    Query(q): Query<WsQuery>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state, q.job_id))
}

async fn handle_socket(mut socket: WebSocket, state: SharedState, job_id: String) {
    use attune_core::office_job_queue::JobState;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let Some(job) = state.office_jobs.get(&job_id) else {
                    let _ = socket.send(Message::Text(serde_json::json!({
                        "type": "failed", "job_id": job_id,
                        "error": {"message": "job not found", "code": "not-found"}
                    }).to_string().into())).await;
                    return;
                };
                let queue_pos = state.office_jobs.queue_position(&job_id);
                let frame = match job.state {
                    JobState::Done => serde_json::json!({
                        "type": "done", "job_id": job.id,
                        "result": job.result_json.as_ref().and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()),
                    }),
                    JobState::Failed => serde_json::json!({
                        "type": "failed", "job_id": job.id, "error": job.error,
                    }),
                    JobState::Cancelled => serde_json::json!({
                        "type": "cancelled", "job_id": job.id,
                    }),
                    _ => serde_json::json!({
                        "type": "progress", "job_id": job.id,
                        "state": job.state, "stage": job.stage,
                        "queue_position": queue_pos,
                        "progress": job.progress,
                        "elapsed_ms": job.elapsed_ms,
                    }),
                };
                if socket.send(Message::Text(frame.to_string().into())).await.is_err() { return; }
                if matches!(job.state, JobState::Done | JobState::Failed | JobState::Cancelled) { return; }
            }
            msg = socket.recv() => {
                let Some(Ok(Message::Text(t))) = msg else { return; };
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                    if v["type"] == "cancel" {
                        state.office_jobs.update(&job_id, |j|
                            j.state = JobState::Cancelled);
                    }
                }
            }
        }
    }
}
```

在 `lib.rs` 路由表加：
```rust
.route("/api/v1/office/jobs/ws", axum::routing::get(routes::office::ws_jobs))
```

- [ ] **Step 2: CLI 桩** — 修改 `rust/crates/attune-cli/src/main.rs`（按现有 clap 结构添加 `Ocr` + `Transcribe` 子命令，body 先打印 "not yet wired"，D5 完整实现）。

- [ ] **Step 3: 编译验证**

```bash
cargo build --workspace 2>&1 | tail -10
```

- [ ] **Step 4: Commit**

```bash
git add .
git commit -m "feat(office): WS progress endpoint + CLI subcommand stubs (D1 done)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Phase D2 (5/21 Wed) — B 档 5 scene 抽取规则

### Task D2.1: `ocr/structured/mod.rs` + `normalize.rs` + 公共框架

**Files:**
- Create: `rust/crates/attune-core/src/ocr/structured/mod.rs`
- Create: `rust/crates/attune-core/src/ocr/structured/normalize.rs`

- [ ] **Step 1: 注册 module** — `rust/crates/attune-core/src/ocr/mod.rs` 末尾加 `pub mod structured;`

- [ ] **Step 2: 创建 mod.rs**

```rust
//! Office helper 结构化字段抽取 — 零 LLM, 全规则 (正则锚点 + bbox 邻近 + 校验)。

use super::{BBox, RawLine};
use serde::{Deserialize, Serialize};

pub mod normalize;
pub mod scene_document;
pub mod scene_receipt;
pub mod scene_table;
pub mod scene_card;
pub mod scene_id_card;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldValue {
    pub value: Option<String>,
    pub confidence: f32,
    pub bbox: Option<BBox>,
    pub source_line_idx: Option<usize>,
}

impl FieldValue {
    pub fn none() -> Self {
        Self { value: None, confidence: 0.0, bbox: None, source_line_idx: None }
    }
    pub fn some(value: String, confidence: f32, line: &RawLine, idx: usize) -> Self {
        Self {
            value: Some(value), confidence, bbox: Some(line.bbox), source_line_idx: Some(idx),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "schema", rename_all = "snake_case")]
pub enum StructuredFields {
    DocumentV1 {
        fields: scene_document::DocumentFields,
        #[serde(default)] unrecognized_fields: Vec<String>,
        #[serde(default)] validation_warnings: Vec<String>,
    },
    ReceiptV1 {
        fields: scene_receipt::ReceiptFields,
        #[serde(default)] unrecognized_fields: Vec<String>,
        #[serde(default)] validation_warnings: Vec<String>,
    },
    TableV1 {
        fields: scene_table::TableFields,
        #[serde(default)] unrecognized_fields: Vec<String>,
        #[serde(default)] validation_warnings: Vec<String>,
    },
    CardV1 {
        fields: scene_card::CardFields,
        #[serde(default)] unrecognized_fields: Vec<String>,
        #[serde(default)] validation_warnings: Vec<String>,
    },
    IdCardCnV1 {
        fields: scene_id_card::IdCardCnFields,
        #[serde(default)] unrecognized_fields: Vec<String>,
        #[serde(default)] validation_warnings: Vec<String>,
    },
    BankCardV1 {
        fields: scene_id_card::BankCardFields,
        #[serde(default)] unrecognized_fields: Vec<String>,
        #[serde(default)] validation_warnings: Vec<String>,
    },
    BusinessLicenseV1 {
        fields: scene_id_card::BusinessLicenseFields,
        #[serde(default)] unrecognized_fields: Vec<String>,
        #[serde(default)] validation_warnings: Vec<String>,
    },
}

/// 在锚点行之后找 value（同一行右侧 / 下一行左侧）。
pub fn find_value_after_anchor(
    lines: &[RawLine], anchor_re: &regex::Regex, max_offset: usize,
) -> Option<(usize, String, f32)> {
    for (i, l) in lines.iter().enumerate() {
        if let Some(m) = anchor_re.find(&l.text) {
            // 同行右侧
            let after = &l.text[m.end()..].trim_start_matches(|c: char| c == ':' || c == '：' || c.is_whitespace());
            if !after.is_empty() {
                return Some((i, after.to_string(), l.confidence));
            }
            // 下 1-N 行
            for off in 1..=max_offset {
                if let Some(next) = lines.get(i + off) {
                    if !next.text.trim().is_empty() {
                        return Some((i + off, next.text.trim().to_string(), next.confidence));
                    }
                }
            }
        }
    }
    None
}

/// 入口：按 profile id 路由到对应 scene extractor。
pub fn extract(profile: &str, lines: &[RawLine], id_card_subtype: Option<&str>) -> Option<StructuredFields> {
    match profile {
        "document" => Some(scene_document::extract(lines)),
        "receipt"  => Some(scene_receipt::extract(lines)),
        "table"    => Some(scene_table::extract(lines)),
        "card"     => Some(scene_card::extract(lines)),
        "id_card"  => scene_id_card::extract(lines, id_card_subtype?),
        _ => None, // A 档场景 (screenshot/contract/ancient/form) 返 None
    }
}
```

- [ ] **Step 3: normalize.rs**

```rust
//! 字段值规范化辅助。

/// `2026/05/18` / `2026年5月18日` / `26-5-18` → ISO `2026-05-18`
pub fn normalize_date(s: &str) -> Option<String> {
    let re = regex::Regex::new(r"(\d{2,4})[年/\-\.](\d{1,2})[月/\-\.](\d{1,2})").ok()?;
    let cap = re.captures(s)?;
    let (y, m, d) = (
        cap.get(1)?.as_str().parse::<u32>().ok()?,
        cap.get(2)?.as_str().parse::<u32>().ok()?,
        cap.get(3)?.as_str().parse::<u32>().ok()?,
    );
    let y = if y < 100 { 2000 + y } else { y };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) { return None; }
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// 去千分位 / 全角 / 货币符号 → "1234.56"
pub fn normalize_amount(s: &str) -> Option<String> {
    let cleaned: String = s.chars().filter_map(|c| match c {
        '0'..='9' | '.' | '-' => Some(c),
        '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32),
        ',' | '，' | '￥' | '$' | ' ' => None,
        _ => None,
    }).collect();
    if cleaned.is_empty() { return None; }
    cleaned.parse::<f64>().ok().map(|f| format!("{f:.2}"))
}

/// Luhn 算法（银行卡校验）
pub fn luhn_check(card: &str) -> bool {
    let digits: Vec<u32> = card.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() < 13 || digits.len() > 19 { return false; }
    let sum: u32 = digits.iter().rev().enumerate().map(|(i, &d)|
        if i % 2 == 1 { let dd = d * 2; if dd > 9 { dd - 9 } else { dd } } else { d }
    ).sum();
    sum % 10 == 0
}

/// GB 11643-1999 居民身份证号码校验
pub fn id_card_cn_check(id: &str) -> bool {
    if id.len() != 18 { return false; }
    let weights = [7,9,10,5,8,4,2,1,6,3,7,9,10,5,8,4,2];
    let check_chars = ['1','0','X','9','8','7','6','5','4','3','2'];
    let mut sum = 0u32;
    let bytes: Vec<char> = id.chars().collect();
    for (i, w) in weights.iter().enumerate() {
        match bytes[i].to_digit(10) { Some(d) => sum += d * w, None => return false }
    }
    bytes[17].to_ascii_uppercase() == check_chars[(sum % 11) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn date_iso() { assert_eq!(normalize_date("2026年5月18日").as_deref(), Some("2026-05-18")); }
    #[test] fn date_slash() { assert_eq!(normalize_date("2026/05/18").as_deref(), Some("2026-05-18")); }
    #[test] fn date_short_year() { assert_eq!(normalize_date("26-5-18").as_deref(), Some("2026-05-18")); }
    #[test] fn amount_comma() { assert_eq!(normalize_amount("1,234.56").as_deref(), Some("1234.56")); }
    #[test] fn amount_yuan() { assert_eq!(normalize_amount("￥1,234.56").as_deref(), Some("1234.56")); }
    #[test] fn luhn_valid() { assert!(luhn_check("6225781234567890")); /* 实际用真卡号测 */ }
    #[test] fn luhn_invalid_short() { assert!(!luhn_check("123")); }
    #[test] fn id_card_invalid_length() { assert!(!id_card_cn_check("1234")); }
}
```

- [ ] **Step 4: 检查 regex crate 已在 attune-core Cargo.toml**

```bash
grep "^regex" rust/crates/attune-core/Cargo.toml
```
若无则加 `regex = "1"`. Test pass：

```bash
cargo test -p attune-core ocr::structured::normalize::tests 2>&1 | tail -10
```

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ocr/
git commit -m "feat(ocr): scaffold structured field extraction framework + normalize helpers"
```

---

### Task D2.2-D2.6: 5 个 scene 抽取 module

**每个 scene 一个 task**，结构相同：
1. 创建 `scene_*.rs`（定义 `*Fields` struct + `extract(lines)` 函数）
2. 内嵌 ≥ 5 个 boundary test (`#[cfg(test)]`)
3. `cargo test` 验证
4. Commit

按 spec §4.2 抽取规则实现：
- **D2.2 scene_receipt.rs** — 7 字段（invoice_no/issue_date/seller/buyer/amount_total/tax_amount/amount_chinese）+ 锚点正则 + amount 中文数字交叉校验
- **D2.3 scene_table.rs** — k-means 列对齐 + 行聚类（headers/rows/row_count/column_count）
- **D2.4 scene_card.rs** — 启发式（phone/email/job_title/name/company/address）
- **D2.5 scene_id_card.rs** — 3 子类型 + 校验位（GB 11643 / Luhn / GB 32100-2015）
- **D2.6 scene_document.rs** — 段落聚类 + 双栏检测 + block 类型

每个 module 完成后 `cargo test -p attune-core ocr::structured::scene_<name>::tests`。

每 task 完成后立刻 commit：
```bash
git commit -m "feat(ocr): scene_<name> field extraction with N boundary tests"
```

D2 收尾后，修改 `routes/office.rs` 的 `post_ocr` 末段，把 `structured: None` 改为：

```rust
let structured = if !lines.is_empty() {
    attune_core::ocr::structured::extract(&profile, &lines, id_card_subtype.as_deref())
        .map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null))
} else { None };
```

D2 收口 commit。

---

## Phase D3 (5/22 Thu) — Golden 数据集 + L1 准入门

### Task D3.1: 数据集采集 + expected.yaml 编写

**Files:**
- Create: `rust/crates/attune-server/tests/golden/office/ocr/<scene>/*.png + .expected.yaml` × 50
- Create: `rust/crates/attune-server/tests/golden/office/asr/<lang>/*.wav + .expected.yaml` × 55
- Create: `rust/crates/attune-server/tests/golden/office/BASELINE_ENV.md`

- [ ] **Step 1: 公开数据集自动下载脚本**

创建 `scripts/fetch-office-golden.sh`:

```bash
#!/usr/bin/env bash
# 下载 AISHELL-3 / LibriSpeech / SROIE / FUNSD 抽样到 tests/golden/office/
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/rust/crates/attune-server/tests/golden/office"
mkdir -p "$DEST"/{ocr/{document,receipt,table,card,id_card_cn,bank_card,business_license},asr/{zh_aishell,en_libri,zh_en_mixed,meeting}}

# AISHELL-3 抽 20 段（已有公开 mirror）
# LibriSpeech test-clean 抽 20 段
# SROIE 公开发票数据集（5 张）+ 内部脱敏 5 张
# FUNSD 公开 forms 抽 10 张作 table 替代
# 详细 URL 在文档中固化
echo "TODO: 实际下载 URL 由 D3 实施时填入（公开镜像选 ICDAR / aliendao / huggingface dataset hub）"
```

- [ ] **Step 2: 内部脱敏样本采集**（用户提供 OR 现有 attune 测试集复用）

`grep -rln "test.*invoice\|test.*receipt" rust/crates/` 看是否有已有 sample 可复用。

- [ ] **Step 3: expected.yaml 模板**

每个 OCR sample 配套 `<name>.expected.yaml`（spec §6.2 schema），人工标注 ground truth 字段。

- [ ] **Step 4: BASELINE_ENV.md** — 固化测试环境（CPU 型号 / RAM / OS / whisper.cpp 版本 / PP-OCRv5 模型 hash）

- [ ] **Step 5: Commit**

```bash
git add scripts/fetch-office-golden.sh rust/crates/attune-server/tests/golden/office/
git commit -m "test(office): golden dataset + expected.yaml + BASELINE_ENV"
```

---

### Task D3.2-D3.4: L1 准入门测试代码

**Files:**
- Create: `rust/crates/attune-server/tests/office_ocr_golden_gate.rs`
- Create: `rust/crates/attune-server/tests/office_asr_golden_gate.rs`
- Create: `rust/crates/attune-server/tests/office_error_contract.rs`
- Create: `rust/crates/attune-server/tests/office_schema_compat.rs`

每个测试文件按 spec §6.3 矩阵实现。框架（D3.2 为例）：

```rust
//! L1 准入门 — OCR 准确度 + 速度。
//! per CLAUDE.md "law-pro golden gate" 模式，类似 deterministic_agent_golden_gate。

use std::path::PathBuf;
use std::time::Instant;

#[derive(serde::Deserialize)]
struct ExpectedYaml {
    id: String,
    profile: String,
    schema_version: String,
    expected_fields: std::collections::BTreeMap<String, String>,
    expected_lines_count_min: Option<usize>,
    max_elapsed_ms: u64,
}

fn run_scene(scene: &str, min_accuracy: f64) {
    let dir: PathBuf = format!("tests/golden/office/ocr/{scene}").into();
    let mut total = 0usize;
    let mut hits = 0usize;
    let mut elapsed_ms: Vec<u64> = vec![];

    for entry in std::fs::read_dir(&dir).expect("dir") {
        let p = entry.unwrap().path();
        if !p.extension().and_then(|s| s.to_str()).map(|s| s == "yaml").unwrap_or(false) { continue; }
        let exp: ExpectedYaml = serde_yaml::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();

        let img_path = p.with_extension("png");
        if !img_path.exists() { continue; }

        let provider = attune_core::ocr::detect_default_provider().expect("PP-OCR not available");
        let ocr_profile = attune_core::ocr::profile_for_id(Some(&exp.profile));

        let start = Instant::now();
        let out = provider.extract_structured(&img_path, &ocr_profile).unwrap();
        let ms = start.elapsed().as_millis() as u64;
        elapsed_ms.push(ms);

        assert!(ms <= exp.max_elapsed_ms,
            "{} elapsed {}ms > red line {}ms", exp.id, ms, exp.max_elapsed_ms);

        let lines = out.lines.unwrap_or_default();
        let structured = attune_core::ocr::structured::extract(&exp.profile, &lines, None);

        if let Some(s) = structured {
            let json = serde_json::to_value(&s).unwrap();
            for (k, expected) in &exp.expected_fields {
                total += 1;
                let actual = json.pointer(&format!("/fields/{k}/value")).and_then(|v| v.as_str());
                if actual == Some(expected.as_str()) { hits += 1; }
            }
        }
    }
    let acc = hits as f64 / total.max(1) as f64;
    assert!(acc >= min_accuracy,
        "scene={scene} accuracy={acc:.4} < red line {min_accuracy}; hits={hits}/{total}");
}

#[test] fn ocr_receipt_field_accuracy() { run_scene("receipt", 0.92); }
#[test] fn ocr_document_field_accuracy() { run_scene("document", 0.92); }
#[test] fn ocr_table_cell_accuracy() { run_scene("table", 0.92); }
#[test] fn ocr_card_field_accuracy() { run_scene("card", 0.92); }
#[test] fn ocr_id_card_cn_accuracy() { run_scene("id_card_cn", 0.95); }
#[test] fn ocr_bank_card_accuracy() { run_scene("bank_card", 0.95); }
#[test] fn ocr_business_license_accuracy() { run_scene("business_license", 0.95); }
```

**office_asr_golden_gate.rs**: WER (字符级编辑距离 / ground truth 长度) + DER + RTF 红线。

**office_error_contract.rs**: spec §6.3 矩阵每个测试一个 `#[tokio::test]`。

**office_schema_compat.rs**: typed deser 测 ReceiptV1 / 未知 schema fallback / envelope_version present / 所有 scene 有 ≥ 1 approved YAML。

每文件完成跑 `cargo test --test <name>`，commit:

```bash
git commit -m "test(office): L1 golden gate (OCR/ASR accuracy + speed + error contract + schema compat)"
```

---

### Task D3.5: 第一次完整跑 L1，记录基线 + 识别 fail 项

- [ ] **Step 1: 跑全套 L1**

```bash
cargo test -p attune-server --test office_ocr_golden_gate --test office_asr_golden_gate --test office_error_contract --test office_schema_compat 2>&1 | tee /tmp/d3-l1-baseline.txt
```

- [ ] **Step 2: 把 baseline 写入 `tests/golden/office/benchmarks/2026-05-22-d3-baseline.json`**

- [ ] **Step 3: 列出 fail 场景到 RELEASE.md draft** (D4 用)

- [ ] **Step 4: D3 收口 commit**

```bash
git add rust/crates/attune-server/tests/golden/office/benchmarks/
git commit -m "test(office): D3 baseline — first full L1 run, document accuracy gaps"
```

---

## Phase D4 (5/23 Fri) — 准确度迭代 + UI 完整化

### Task D4.1-D4.N: 准确度迭代

**对每个 D3 fail 的 scene，循环：**
1. 看 fail YAML 的 expected vs actual diff
2. 加锚点关键词 / 调正则 / 加启发式权重 / 调 confidence 公式
3. 单测覆盖该 case 的修复
4. 重跑 golden gate 验证不退化其他 case
5. Commit

按 spec §9.1 兜底链：D5 早晨仍 fail → 触发兜底②（RELEASE.md 显式声明实测值 + target 在 v0.7.2）。

### Task D4.x: OfficeView UI

**Files:**
- Create: `rust/crates/attune-server/ui/src/views/OfficeView.tsx`
- Create: `rust/crates/attune-server/ui/src/hooks/useOfficeJob.ts`
- Modify: `rust/crates/attune-server/ui/src/views/index.ts`
- Modify: `rust/crates/attune-server/ui/src/i18n/zh.ts` + `en.ts`（必须**两边同时加 key**, per CLAUDE.md i18n 铁律）
- Modify: 主 shell / 导航（注册 Office tab）

按 spec §1 架构 + §3 数据契约实现。提交前跑 grep 守卫确认 0 硬编码中文（per CLAUDE.md i18n 自检）：

```bash
cd rust/crates/attune-server/ui/src
grep -rnP "(toast\([^)]*'[^']*[\x{4e00}-\x{9fff}]|(title|placeholder|label|description|aria-label)=\"[^\"]*[\x{4e00}-\x{9fff}]|>[^<{]*[\x{4e00}-\x{9fff}])" --include="*.tsx" . | grep -v "/i18n/"
diff <(grep -oP "^\s+'[\w.]+'\s*:" i18n/zh.ts | tr -d " ':" | sort) \
     <(grep -oP "^\s+'[\w.]+'\s*:" i18n/en.ts | tr -d " ':" | sort)
```

两条都无输出才能 commit。

D4 收口 commit：
```bash
git commit -m "feat(office): D4 — accuracy iterations + OfficeView UI"
```

---

## Phase D5 (5/24 Sat) — L2 + 文档

### Task D5.1-D5.4: L2 测试

按 spec §6.3 矩阵实现：
- **D5.1** `office_concurrent_test.rs` — `tokio::join!` 5 OCR + 2 ASR 同跑无 panic、queue_position 正确
- **D5.2** `office_cancel_test.rs` — SIGTERM whisper-cli 后 `pgrep whisper` = 0，cancel done/cancelled 返 409，WS 断开 ≠ 取消
- **D5.3** `office_failure_recovery_test.rs` — corrupt PDF / 0 字节 / SIGKILL 注入；重提 fail 文件成功
- **D5.4** `office_prop_tests.rs` — 5 个 proptest invariants：
  - `prop_corrupt_pdf_no_panic`
  - `prop_zero_byte_file_returns_empty_file_code`
  - `prop_truncated_audio_no_panic`
  - `prop_field_confidence_in_0_1_range`
  - `prop_schema_serde_roundtrip`

每个文件完成 → `cargo test` 验证 → commit。

### Task D5.5: ENFORCE mode gate

- [ ] **Step 1: 创建 `tests/office_six_category_floor.rs`**

按 law-pro `six_category_floor_check` 模式实现 office 版本：扫 `golden/office/ocr/<scene>/` 数 approved YAML, 扫 prop_tests / boundary tests, 扫 binary integration tests, 输出 violations 列表。

- [ ] **Step 2: 跑 ENFORCE**

```bash
ATTUNE_ENFORCE_OFFICE_FLOOR=1 cargo test -p attune-server --test office_six_category_floor 2>&1 | tail -10
```
Expected: 0 violations

### Task D5.6: 文档更新

- [ ] **README**: 加 Office helper 段（端点 / 5 scene 列表 / 速度准确度红线）
- [ ] **DEVELOP.md**: 加 OCR/ASR API 用法 + golden 测试运行命令 + ENFORCE mode 说明
- [ ] **RELEASE.md**: v0.7.1 changelog (含基线数据)

### Task D5.7: CLI 完整化

完成 D1.5 桩的 CLI:
- `attune ocr <file> [--profile receipt] [--json]` — 内部 `POST /api/v1/office/ocr` (or 直接调 attune-core 函数)
- `attune transcribe <audio> [--model small] [--wait]`

D5 收口 commit:
```bash
git commit -m "test(office): D5 — L2 concurrent/cancel/recovery/proptest + ENFORCE gate + docs + CLI"
```

---

## Phase D6 (5/25 Sun) — RC → GA tag

### Task D6.1: 全链路联调

- [ ] **Step 1: 跑全套 L1 + L2 + ENFORCE**

```bash
cargo test -p attune-server 2>&1 | tee /tmp/d6-final.txt
ATTUNE_ENFORCE_OFFICE_FLOOR=1 cargo test -p attune-server --test office_six_category_floor 2>&1 | tail
```
所有 pass + 0 violations 才能往下。

- [ ] **Step 2: 端到端 curl 验证（非单元 mock）**

启动 server，提交真实发票 PDF / 真实会议音频，跑 OCR + ASR 端到端，把 stdout/stderr 存到 `/tmp/d6-e2e.txt`。

- [ ] **Step 3: Playwright 真 Chrome UI 验证**

按 CLAUDE.md MCP 限制走 Chrome，截图归档到 `docs/screenshots/v071-office-verification/`，文件名 `attune-v071-office-01-ocr-receipt.png` 等。

### Task D6.2: develop → main merge

- [ ] **Step 1: 复核**

```bash
git checkout develop
git log --oneline origin/develop..HEAD
git status
```

- [ ] **Step 2: push develop**

```bash
git push origin develop
```

- [ ] **Step 3: develop → main (per CLAUDE.md GitFlow Lite §)**

```bash
git checkout main
git pull origin main
git merge --no-ff develop -m "merge: develop → main (v0.7.1 GA — office helper release)"
```

- [ ] **Step 4: 打 tags**

```bash
git tag -a v0.7.1 -m "v0.7.1 — Office helper (OCR 5 scenes + ASR async transcribe)"
git tag -a desktop-v0.7.1 -m "desktop-v0.7.1 — Office helper desktop release"
```

- [ ] **Step 5: push**

```bash
git push origin main v0.7.1 desktop-v0.7.1
```

- [ ] **Step 6: 回 develop**

```bash
git checkout develop
git merge main  # 把 main 的 merge commit 拉回 develop, 保持同步
git push origin develop
```

### Task D6.3: 验收 checklist

按 spec §9.4 GA checklist 13 项逐个勾完。

任一项 fail → 触发兜底（spec §9.1 ⑤）：发 `v0.7.1-rc.1` 让用户验，正式 GA 滑下周；**不接受为赶 release 关准确度门**。

---

## Self-Review

**Spec coverage:**
- §1 背景 → 无独立 task（背景章节）
- §2 架构 → D1.3-D1.5 覆盖
- §3 数据契约 → D1.3-D1.5 + D5.6 schema_compat
- §4 字段抽取规则 → D2.1-D2.6
- §5 红线 → D3.2-D3.4 (L1) 实施
- §6 测试矩阵 → D3 (L1) + D5.1-D5.5 (L2 + ENFORCE)
- §7 边界 case → D5.3 failure_recovery 覆盖
- §8 兜底原则 → 每个 scene 抽取代码中体现（D2.2-D2.6 内的 None 兜底逻辑）
- §9 计划 → 本 plan 全文映射
- §10 v0.8+ → 不在本次 scope（明确）
- §11 引用 → 文档章节，无 task

**Placeholder scan:** D2.2-D2.6 每个 scene 抽取代码细节展开较少（按 spec §4.2 实施），D3.1 数据集下载 URL 标 TODO 是合理的（公开 URL 取决于实施时镜像可用性）— 这两处由实施 agent 按 spec 展开，可接受。

**Type consistency:** `RawLine` / `BBox` / `FieldValue` / `StructuredFields` / `JobState` / `JobStage` / `JobError` / `Job` 全程一致。`extract_structured()` 返回 `OcrOutput { ..., lines: Option<Vec<RawLine>> }` 一致。

---

**Plan complete and saved to `docs/superpowers/plans/2026-05-20-office-helper.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — 每 task 派 fresh subagent，review 之间，快速迭代
**2. Inline Execution** — 本会话内 executing-plans 批量执行 + checkpoint

**Which approach?**
