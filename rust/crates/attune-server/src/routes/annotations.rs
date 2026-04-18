use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::state::SharedState;
use attune_core::ai_annotator::{self, AiAngle};
use attune_core::store::AnnotationInput;

// ── 批注路由 ──────────────────────────────────────────────────────────────────
//
// 成本/触发契约：所有批注操作都是用户显式触发（UI 选中文字 → 弹窗 → 保存；
// 或 AI 分析按钮 → 显式生成）。建库流水线绝不调这些 route。
// 上限：每条笔记最多 500 批注（防止恶意或失控脚本灌爆表）；text_snippet 最长 2000 字符。

const MAX_ANNOTATIONS_PER_ITEM: usize = 500;
const MAX_SNIPPET_LEN: usize = 2000;
const MAX_CONTENT_LEN: usize = 10_000;
const MAX_LABEL_LEN: usize = 64;

const ALLOWED_COLORS: &[&str] = &["yellow", "red", "green", "blue"];

#[derive(Deserialize)]
pub struct CreateAnnotationRequest {
    pub item_id: String,
    pub offset_start: i64,
    pub offset_end: i64,
    pub text_snippet: String,
    pub label: Option<String>,
    pub color: Option<String>,
    pub content: Option<String>,
    /// 默认 "user"；AI 路径传 "ai"
    pub source: Option<String>,
}

#[derive(Deserialize)]
pub struct ListAnnotationsQuery {
    pub item_id: String,
}

#[derive(Deserialize)]
pub struct UpdateAnnotationRequest {
    pub label: Option<String>,
    pub color: Option<String>,
    pub content: Option<String>,
    pub source: Option<String>,
}

fn bad_request(msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": msg})))
}

fn validate_source(source: Option<&str>) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if let Some(s) = source {
        if !matches!(s, "user" | "ai") {
            return Err(bad_request("source must be 'user' or 'ai'"));
        }
    }
    Ok(())
}

fn validate_common(
    offset_start: i64,
    offset_end: i64,
    text_snippet: &str,
    label: Option<&str>,
    color: &str,
    content: &str,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if offset_start < 0 || offset_end < offset_start {
        return Err(bad_request("invalid offsets"));
    }
    if text_snippet.len() > MAX_SNIPPET_LEN {
        return Err(bad_request("text_snippet too long (max 2000 bytes)"));
    }
    if content.len() > MAX_CONTENT_LEN {
        return Err(bad_request("content too long (max 10000 bytes)"));
    }
    if let Some(l) = label {
        if l.len() > MAX_LABEL_LEN {
            return Err(bad_request("label too long (max 64 bytes)"));
        }
    }
    if !ALLOWED_COLORS.contains(&color) {
        return Err(bad_request("color must be one of yellow/red/green/blue"));
    }
    Ok(())
}

/// POST /api/v1/annotations — 创建批注
pub async fn create_annotation(
    State(state): State<SharedState>,
    Json(body): Json<CreateAnnotationRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let color = body.color.as_deref().unwrap_or("yellow");
    let content = body.content.as_deref().unwrap_or("");
    validate_common(
        body.offset_start, body.offset_end,
        &body.text_snippet, body.label.as_deref(), color, content,
    )?;
    validate_source(body.source.as_deref())?;

    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    // 拉取条目：既校验存在性（404 清晰于 SQL 外键错），又拿 content 长度做 offset 越界校验
    let item = vault.store().get_item(&dek, &body.item_id).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
    })?.ok_or_else(|| (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "item not found"}))))?;

    // offset 上界：按 UTF-16 code unit（与前端 JS String index 对齐）
    let content_utf16_len = item.content.encode_utf16().count() as i64;
    if body.offset_end > content_utf16_len {
        return Err(bad_request(&format!(
            "offset_end {} exceeds item content length {}",
            body.offset_end, content_utf16_len
        )));
    }

    // 单条目批注数上限
    let existing = vault.store().count_annotations(&body.item_id).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    if existing >= MAX_ANNOTATIONS_PER_ITEM {
        return Err((StatusCode::TOO_MANY_REQUESTS, Json(serde_json::json!({
            "error": format!("annotation limit {MAX_ANNOTATIONS_PER_ITEM} reached for this item")
        }))));
    }

    let input = AnnotationInput {
        offset_start: body.offset_start,
        offset_end: body.offset_end,
        text_snippet: body.text_snippet,
        label: body.label,
        color: color.to_string(),
        content: content.to_string(),
        source: body.source,
    };
    let id = vault.store().create_annotation(&dek, &body.item_id, &input).map_err(|e| {
        (StatusCode::UNPROCESSABLE_ENTITY, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    Ok(Json(serde_json::json!({"id": id, "status": "ok"})))
}

/// GET /api/v1/annotations?item_id=xxx — 列出某条目的所有批注
pub async fn list_annotations(
    State(state): State<SharedState>,
    Query(q): Query<ListAnnotationsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if q.item_id.len() > 64 {
        return Err(bad_request("item_id too long"));
    }
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    let anns = vault.store().list_annotations(&dek, &q.item_id).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    Ok(Json(serde_json::json!({"annotations": anns})))
}

/// PATCH /api/v1/annotations/{id} — 编辑批注（未指定 source 时回到 user）
pub async fn update_annotation(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateAnnotationRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if id.len() > 64 {
        return Err(bad_request("id too long"));
    }
    let color = body.color.as_deref().unwrap_or("yellow");
    let content = body.content.as_deref().unwrap_or("");
    // 更新场景下 offset/snippet 不变（UI 不允许拖动重新定位；如要改定位就删了重建）。
    // 这里仍然跑共通校验覆盖 label/color/content 长度 + source 白名单。
    validate_common(0, 0, "", body.label.as_deref(), color, content)?;
    validate_source(body.source.as_deref())?;

    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    // 读现有记录以保留 offset/snippet（update 路径不接受改定位）
    let input = AnnotationInput {
        offset_start: 0, // 未使用 — update SQL 仅改 label/color/content/source
        offset_end: 0,
        text_snippet: String::new(),
        label: body.label,
        color: color.to_string(),
        content: content.to_string(),
        source: body.source,
    };

    vault.store().update_annotation(&dek, &id, &input).map_err(|e| {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    Ok(Json(serde_json::json!({"status": "ok"})))
}

#[derive(Deserialize)]
pub struct AiAnalyzeRequest {
    pub item_id: String,
    pub angle: String,
    /// scope = "whole_item" (全文) | "selection" (段落范围)
    #[serde(default = "default_scope")]
    pub scope: String,
    /// scope = "selection" 时必填
    pub selection_start: Option<i64>,
    pub selection_end: Option<i64>,
}

fn default_scope() -> String { "whole_item".to_string() }

/// POST /api/v1/annotations/ai — AI 分析指定条目，生成 source='ai' 批注。
///
/// 成本契约：**必须用户显式触发**（UI 里的"🤖 AI 分析"下拉，标注了耗时/本地/云端）。
/// 建库管道、classify worker、skill evolver 都不调此路由。
pub async fn ai_analyze(
    State(state): State<SharedState>,
    Json(body): Json<AiAnalyzeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // 1. 参数校验
    let angle = AiAngle::parse(&body.angle)
        .ok_or_else(|| bad_request("unknown angle (must be risk/outdated/highlights/questions)"))?;
    if body.item_id.len() > 64 {
        return Err(bad_request("item_id too long"));
    }
    if !matches!(body.scope.as_str(), "whole_item" | "selection") {
        return Err(bad_request("scope must be 'whole_item' or 'selection'"));
    }

    // 2. 锁 vault 拉内容 + LLM 句柄（放在一个锁临时作用域里避免长持有）
    let (item_content, llm_arc) = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|e| {
            (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
        })?;
        let item = vault.store().get_item(&dek, &body.item_id).map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
        })?.ok_or_else(|| {
            (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "item not found"})))
        })?;
        let llm = state.llm.lock().unwrap_or_else(|e| e.into_inner())
            .as_ref().cloned();
        (item.content, llm)
    };

    let llm = llm_arc.ok_or_else(|| {
        (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({
            "error": "LLM not configured. Install Ollama or configure cloud provider in Settings."
        })))
    })?;

    // 3. 确定分析范围
    let (scope_text, scope_base) = if body.scope == "selection" {
        let s = body.selection_start.unwrap_or(-1);
        let e = body.selection_end.unwrap_or(-1);
        if s < 0 || e <= s {
            return Err(bad_request("selection requires positive selection_start < selection_end"));
        }
        // 按 UTF-16 index 切 substring —— 前端传来的 offset 是 JS String index。
        // Rust 端用 char_indices 手动切（不能直接 &str[s..e]）。
        let utf16_len: usize = item_content.encode_utf16().count();
        if (e as usize) > utf16_len {
            return Err(bad_request("selection out of item bounds"));
        }
        let sub = substring_by_utf16(&item_content, s as usize, e as usize);
        (sub, s)
    } else {
        (item_content.clone(), 0)
    };

    // 4. 调 LLM（同步调用，在 tokio spawn_blocking 里跑以避免阻塞）
    let content_full_clone = item_content.clone();
    let findings = tokio::task::spawn_blocking(move || {
        ai_annotator::generate_annotations(
            llm.as_ref(),
            &scope_text,
            &content_full_clone,
            scope_base,
            angle,
        )
    }).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": format!("tokio join: {e}")}))))?
    .map_err(|e| (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": format!("LLM error: {e}")}))))?;

    // 5. 为每个 finding 创建 source='ai' 批注
    let label = angle.label_prefix().to_string();
    let color = angle.default_color().to_string();
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    // TOCTOU 防御：步骤 2 释放锁后到此，item 可能被删。提前探测给 404，避免所有 INSERT
    // 因外键失败 + 返回 200 created_count=0 的歧义。
    if !vault.store().item_exists(&body.item_id).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
    })? {
        return Err((StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": "item was deleted during AI analysis"
        }))));
    }

    let mut created_ids = Vec::with_capacity(findings.len());
    for f in &findings {
        let input = AnnotationInput {
            offset_start: f.offset_start,
            offset_end: f.offset_end,
            text_snippet: f.snippet.clone(),
            label: Some(label.clone()),
            color: color.clone(),
            content: f.reason.clone(),
            source: Some("ai".into()),
        };
        match vault.store().create_annotation(&dek, &body.item_id, &input) {
            Ok(id) => created_ids.push(id),
            Err(e) => {
                tracing::warn!("ai_analyze: failed to persist finding {:?}: {e}", f.snippet);
                // 单条失败不整体回滚 —— 尽可能保留已分析出的结果
            }
        }
    }

    Ok(Json(serde_json::json!({
        "status": "ok",
        "angle": body.angle,
        "created_count": created_ids.len(),
        "created_ids": created_ids,
    })))
}

/// 按 UTF-16 code unit 索引切取子串 —— 与前端 JS String index 语义对齐。
fn substring_by_utf16(s: &str, start: usize, end: usize) -> String {
    let mut out = String::new();
    let mut i = 0usize;
    for ch in s.chars() {
        let step = ch.len_utf16();
        if i >= end { break; }
        if i >= start {
            out.push(ch);
        }
        i += step;
    }
    out
}

/// DELETE /api/v1/annotations/{id} — 删除批注
pub async fn delete_annotation(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if id.len() > 64 {
        return Err(bad_request("id too long"));
    }
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    vault.store().delete_annotation(&id).map_err(|e| {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    Ok(Json(serde_json::json!({"status": "ok"})))
}
