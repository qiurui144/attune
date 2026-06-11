use attune_core::reindex;
use attune_core::store::audit::PrivacyTier;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use crate::error::{AppError, AppResult};
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize { 20 }

pub async fn list_items(
    State(state): State<SharedState>,
    Query(params): Query<ListQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock()
        .map_err(|_| AppError::Internal("vault lock poisoned".into()))?;
    let _ = vault.dek_db().map_err(|e| {
        AppError::Forbidden(e.to_string())
    })?;
    let limit = params.limit.min(200);
    let items = vault.store().list_items(limit, params.offset).map_err(|e| {
        AppError::Internal(e.to_string())
    })?;
    Ok(Json(serde_json::json!({"items": items, "count": items.len()})))
}

pub async fn get_item(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock()
        .map_err(|_| AppError::Internal("vault lock poisoned".into()))?;
    let dek = vault.dek_db().map_err(|e| {
        AppError::Forbidden(e.to_string())
    })?;
    match vault.store().get_item(&dek, &id) {
        Ok(Some(item)) => Ok(Json(serde_json::json!(item))),
        Ok(None) => Err(AppError::NotFound("not found".into())),
        Err(e) => Err(AppError::Internal(e.to_string())),
    }
}

/// GET /api/v1/items/{id}/original — 取回 A1 留存的原始证据文件（解密后内联返回）。
///
/// 变体 A「查看证据原文」入口：律师点击即在浏览器内联预览原始扫描件 / 图片 / PDF，
/// 核对 OCR 转录是否准确。404 = 该 item 无留存原件（纯文本笔记 / A1 之前入库的老 item）。
pub async fn get_item_original(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> AppResult<axum::response::Response> {
    let vault = state.vault.lock()
        .map_err(|_| AppError::Internal("vault lock poisoned".into()))?;
    let dek = vault.dek_db().map_err(|e| {
        AppError::Forbidden(e.to_string())
    })?;
    match vault.store().get_item_blob(&dek, &id) {
        Ok(Some(blob)) => {
            use axum::http::header;
            // 证据敏感 → no-store 防浏览器缓存落盘；inline → 内联预览不下载
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, blob.mime)
                .header(header::CONTENT_DISPOSITION, "inline")
                .header(header::CACHE_CONTROL, "no-store")
                .body(axum::body::Body::from(blob.bytes))
                .map_err(|e| {
                    AppError::Internal(e.to_string())
                })
        }
        Ok(None) => Err(AppError::NotFound(
            "no original file retained for this item".into(),
        )),
        Err(e) => Err(AppError::Internal(e.to_string())),
    }
}

#[derive(Deserialize)]
pub struct UpdateRequest {
    pub title: Option<String>,
    pub content: Option<String>,
}

/// v0.7 记忆护城河：UI 编辑文档触发完整 reindex pipeline。
///
/// 之前 (≤ v0.6.3) 仅刷 SQL items.content → 搜索永远返回旧内容（release blocker）。
/// 现在：
/// 1. `Store::update_item` 算 content_hash 与旧值对比，返回 [`UpdateOutcome`]
/// 2. `content_changed == true` 时调 `reindex::reindex_item` 清旧向量/FTS/queue + 重切 chunk + 入队
/// 3. 同步喂 doc_update 信号到 skill_signals（自学习闭环 Phase B hook 1）
pub async fn update_item(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // 输入长度上限。否则恶意 PATCH 500MB content → crypto::encrypt
    // 在 async handler 同步执行阻塞 tokio worker + 写 500MB BLOB 到 SQLite。
    const MAX_ID_LEN: usize = 64;
    const MAX_TITLE_LEN: usize = 1024;
    const MAX_CONTENT_LEN: usize = 100 * 1024 * 1024; // 100 MB 与 upload 一致
    if id.len() > MAX_ID_LEN {
        return Err(AppError::BadRequest("id too long".into()));
    }
    if body.title.as_ref().is_some_and(|t| t.len() > MAX_TITLE_LEN) {
        return Err(AppError::PayloadTooLarge("title too long".into()));
    }
    if body.content.as_ref().is_some_and(|c| c.len() > MAX_CONTENT_LEN) {
        return Err(AppError::PayloadTooLarge("content too large".into()));
    }

    // --- Phase 1: vault ops only (no vectors/fulltext held) ---
    // Lock ordering: vault must be released before acquiring vectors/fulltext to prevent
    // ABBA deadlock with search/chat paths that acquire fulltext→vectors→vault in that order.
    // We release vault here and re-acquire it narrowly in later phases.
    let (outcome, item_for_reindex) = {
        let vault = state.vault.lock()
            .map_err(|_| AppError::Internal("vault lock poisoned".into()))?;
        let dek = vault.dek_db().map_err(|e| {
            AppError::Forbidden(e.to_string())
        })?;

        let outcome = vault.store()
            .update_item(&dek, &id, body.title.as_deref(), body.content.as_deref())
            .map_err(|e| AppError::Internal(e.to_string()))?;

        if !outcome.existed {
            return Err(AppError::NotFound("not found".into()));
        }

        // Read item data as owned Strings before dropping vault guard.
        let item = if outcome.content_changed {
            let item = vault.store().get_item(&dek, &id)
                .map_err(|e| AppError::Internal(e.to_string()))?
                .ok_or_else(|| AppError::NotFound("race: item gone".into()))?;
            Some(item)
        } else {
            None
        };
        // vault guard drops here — no overlap with vectors/fulltext locks below
        (outcome, item)
    };

    let mut reindex_stats = None;
    if outcome.content_changed {
        let item = item_for_reindex.expect("item_for_reindex is Some when content_changed");

        // --- Phase 2: pre-reindex store ops (vault only, no vectors/fulltext) ---
        // Clear stale embed queue and chunk summaries before touching index structures.
        let queue_cleared = {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            let queue_cleared = vault.store()
                .purge_embed_queue_for_item(&id)
                .unwrap_or(0);
            // QW-5: chunk_summary entries keyed by old chunk_hash become stale on content change.
            let _ = vault.store().delete_chunk_summaries_for_item(&id);
            // vault guard drops here
            queue_cleared
        };

        // --- Phase 3: index ops only (vectors + fulltext, no vault held) ---
        // Acquire vectors/fulltext only after vault is released to maintain consistent
        // lock order with search/chat (fulltext→vectors without vault).
        let mut vectors_deleted = 0usize;
        {
            let mut vectors_guard = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
            let fulltext_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
            if let (Some(vectors), Some(fulltext)) = (vectors_guard.as_mut(), fulltext_guard.as_ref()) {
                match vectors.delete_by_item_id(&id) {
                    Ok(n) => vectors_deleted = n,
                    Err(e) => tracing::warn!("vectors.delete_by_item_id failed for {id}: {e}"),
                }
                if let Err(e) = fulltext.delete_document(&id) {
                    tracing::warn!("fulltext.delete_document failed for {id}: {e}");
                }
                if let Err(e) = fulltext.add_document(&id, &item.title, &item.content, &item.source_type) {
                    tracing::warn!("fulltext.add_document failed for {id}: {e} — search 可能短暂 stale，下次 update 重试");
                }
            } else {
                tracing::warn!("vectors / fulltext 未初始化，update_item skip reindex (item={id})");
            }
            // vectors_guard and fulltext_guard drop here
        }

        // --- Phase 4: post-reindex store ops (vault only, no vectors/fulltext held) ---
        // Enqueue embedding chunks and emit doc_update signal.
        let chunks_enqueued = {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            let mut chunk_counter: usize = 0;
            let sections = attune_core::chunker::extract_sections(&item.content);
            let mut enqueue_ok = true;
            for (section_idx, section_text) in &sections {
                if section_text.trim().is_empty() { continue; }
                if let Err(e) = vault.store().enqueue_embedding(&id, chunk_counter, section_text, 1, 1, *section_idx) {
                    tracing::warn!("enqueue_embedding L1 failed for {id}: {e}");
                    enqueue_ok = false;
                    break;
                }
                chunk_counter += 1;
            }
            if enqueue_ok {
                for (section_idx, section_text) in &sections {
                    for chunk_text in attune_core::chunker::chunk(section_text, attune_core::chunker::DEFAULT_CHUNK_SIZE, attune_core::chunker::DEFAULT_OVERLAP) {
                        if let Err(e) = vault.store().enqueue_embedding(&id, chunk_counter, &chunk_text, 2, 2, *section_idx) {
                            tracing::warn!("enqueue_embedding L2 failed for {id}: {e}");
                            break;
                        }
                        chunk_counter += 1;
                    }
                }
            }
            // Phase B hook 1: doc_update 信号喂 skill_evolution
            // 失败不阻塞主流程但留 debug 痕（schema drift / WAL 故障可诊断）
            if let Err(e) = vault.store().record_signal_event("doc_update", &id, None) {
                tracing::debug!(signal = "doc_update", error = %e, "record_signal_event failed (non-fatal)");
            }
            // vault guard drops here
            chunk_counter
        };

        reindex_stats = Some(reindex::ReindexStats {
            vectors_deleted,
            queue_cleared,
            chunks_enqueued,
        });

        // 内容变了 → 失效 search 缓存，否则搜旧关键词命中陈旧结果
        state.invalidate_search_cache();
    }

    Ok(Json(serde_json::json!({
        "status": "ok",
        "content_changed": outcome.content_changed,
        "backfilled_hash": outcome.backfilled_hash,
        "reindex": reindex_stats.map(|s| serde_json::json!({
            "vectors_deleted": s.vectors_deleted,
            "queue_cleared": s.queue_cleared,
            "chunks_enqueued": s.chunks_enqueued,
        })),
    })))
}

/// v0.7 记忆护城河：删除路径同步清向量 + FTS + 队列。
///
/// 之前 (≤ v0.6.3) 仅 SQL 软删 item — `vectors::delete_by_item_id` 与
/// `fulltext::delete_document` 已实现但 0 处调用（死代码）→ orphan 向量
/// 永久残留，搜索时假命中已删文档。
pub async fn delete_item(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    // id 长度上限（防 Path<String> GB 级输入浪费 query plan）
    if id.len() > 64 {
        return Err(AppError::BadRequest("id too long".into()));
    }
    let vault = state.vault.lock()
        .map_err(|_| AppError::Internal("vault lock poisoned".into()))?;
    // vault Locked/Sealed 时拒绝删除（与 update_item / list_items 等
    // mutating handler 一致 — "锁着的 vault 不可被改"语义）。
    let _ = vault.dek_db().map_err(|e| {
        AppError::Forbidden(e.to_string())
    })?;

    // 先清索引（vector + FTS + queue），再 SQL 软删 — 让 search 在删除窗口内
    // 不会读到"DB 已删但向量还在"的 partial 状态
    let mut purge_stats = None;
    {
        let mut vectors_guard = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
        let fulltext_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
        if let (Some(vectors), Some(fulltext)) = (vectors_guard.as_mut(), fulltext_guard.as_ref()) {
            match reindex::purge_item_indexes(vault.store(), vectors, fulltext, &id) {
                Ok(stats) => purge_stats = Some(stats),
                Err(e) => tracing::warn!("purge_item_indexes failed for {id}: {e}"),
            }
        }
    }

    match vault.store().delete_item(&id) {
        Ok(true) => {
            if let Err(e) = vault.store().record_signal_event("doc_delete", &id, None) {
                tracing::debug!(signal = "doc_delete", error = %e, "record_signal_event failed (non-fatal)");
            }
            // 删除后失效 search 缓存，否则已删文档仍被缓存命中
            state.invalidate_search_cache();
            Ok(Json(serde_json::json!({
                "status": "ok",
                "purge": purge_stats.map(|s| serde_json::json!({
                    "vectors_deleted": s.vectors_deleted,
                    "queue_cleared": s.queue_cleared,
                })),
            })))
        },
        Ok(false) => Err(AppError::NotFound("not found".into())),
        Err(e) => Err(AppError::Internal(e.to_string())),
    }
}

#[derive(serde::Deserialize)]
pub struct StaleQuery {
    #[serde(default = "default_stale_days")]
    pub days: i64,
    #[serde(default = "default_stale_limit")]
    pub limit: i64,
}

fn default_stale_days() -> i64 { 30 }
fn default_stale_limit() -> i64 { 50 }

pub async fn list_stale_items(
    State(state): State<SharedState>,
    Query(params): Query<StaleQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock()
        .map_err(|_| AppError::Internal("vault lock poisoned".into()))?;
    let _ = vault.dek_db().map_err(|e| {
        AppError::Forbidden(e.to_string())
    })?;
    let limit = params.limit.min(200);
    let items = vault.store().list_stale_items(params.days, limit).map_err(|e| {
        AppError::Internal(e.to_string())
    })?;
    let count = items.len();
    Ok(Json(serde_json::json!({"items": items, "count": count, "days": params.days})))
}

pub async fn get_item_stats(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock()
        .map_err(|_| AppError::Internal("vault lock poisoned".into()))?;
    let _ = vault.dek_db().map_err(|e| {
        AppError::Forbidden(e.to_string())
    })?;
    match vault.store().get_item_stats(&id) {
        Ok(Some(stats)) => Ok(Json(serde_json::json!(stats))),
        Ok(None) => Err(AppError::NotFound("not found".into())),
        Err(e) => Err(AppError::Internal(e.to_string())),
    }
}

// ============================================================
// v0.6 Phase A.5.4 — per-file 隐私分级
// ============================================================

#[derive(Deserialize)]
pub struct PrivacyTierBody {
    /// "L0" (🔒 永不出网) | "L1" (脱敏→云，默认) | "L3" (LLM 脱敏→云)
    pub tier: String,
}

fn parse_tier(s: &str) -> Result<PrivacyTier, AppError> {
    match s.to_uppercase().as_str() {
        "L0" => Ok(PrivacyTier::L0),
        "L1" => Ok(PrivacyTier::L1),
        "L3" => Ok(PrivacyTier::L3),
        other => Err(AppError::BadRequest(format!(
            "invalid tier '{other}'; expected L0|L1|L3"
        ))),
    }
}

fn tier_str(t: PrivacyTier) -> &'static str {
    match t {
        PrivacyTier::L0 => "L0",
        PrivacyTier::L1 => "L1",
        PrivacyTier::L3 => "L3",
    }
}

/// PATCH /api/v1/items/{id}/privacy_tier  body: { "tier": "L0"|"L1"|"L3" }
pub async fn set_item_privacy(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<PrivacyTierBody>,
) -> AppResult<Json<serde_json::Value>> {
    let tier = parse_tier(&body.tier)?;
    let vault = state.vault.lock().map_err(|_| {
        AppError::Internal("vault lock poisoned".into())
    })?;
    let _ = vault.dek_db().map_err(|e| {
        AppError::Forbidden(e.to_string())
    })?;
    vault.store().set_item_privacy_tier(&id, tier).map_err(|e| {
        let m = e.to_string();
        if m.contains("not found") {
            AppError::NotFound(m)
        } else {
            AppError::Internal(m)
        }
    })?;
    Ok(Json(serde_json::json!({"id": id, "privacy_tier": tier_str(tier)})))
}

/// GET /api/v1/items/{id}/privacy_tier
pub async fn get_item_privacy(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().map_err(|_| {
        AppError::Internal("vault lock poisoned".into())
    })?;
    let _ = vault.dek_db().map_err(|e| {
        AppError::Forbidden(e.to_string())
    })?;
    let tier = vault.store().get_item_privacy_tier(&id).map_err(|e| {
        let m = e.to_string();
        if m.contains("not found") {
            AppError::NotFound(m)
        } else {
            AppError::Internal(m)
        }
    })?;
    Ok(Json(serde_json::json!({"id": id, "privacy_tier": tier_str(tier)})))
}

/// GET /api/v1/items/protected — 列出所有标记为 L0 的 item id（Settings UI "受保护文件"）
pub async fn list_protected_items(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().map_err(|_| {
        AppError::Internal("vault lock poisoned".into())
    })?;
    let _ = vault.dek_db().map_err(|e| {
        AppError::Forbidden(e.to_string())
    })?;
    let ids = vault.store().list_l0_item_ids().map_err(|e| {
        AppError::Internal(e.to_string())
    })?;
    let count = ids.len();
    Ok(Json(serde_json::json!({"items": ids, "count": count})))
}
