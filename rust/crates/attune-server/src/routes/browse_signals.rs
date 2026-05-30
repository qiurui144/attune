//! G1 浏览状态信号 routes（W3 batch B，2026-04-27）。
//!
//! per spec `docs/superpowers/specs/2026-04-27-w3-batch-b-design.md` §3。
//! Chrome 扩展 background worker 周期 flush 队列调用 POST。
//! 路径设计：批量收以减少 HTTP 调用次数（每 30s 一次最多 50 条）。

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use attune_core::store::browse_signals::BrowseSignalInput;

use crate::state::SharedState;

const MAX_BATCH_SIZE: usize = 50;

#[derive(Deserialize)]
pub struct BrowseSignalsBatch {
    pub signals: Vec<BrowseSignalInput>,
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_list_limit")]
    pub limit: usize,
}
fn default_list_limit() -> usize {
    20
}

#[derive(Deserialize)]
pub struct DeleteQuery {
    pub domain: Option<String>,
}

/// POST /api/v1/browse_signals — 批量接收 Chrome 扩展上报
pub async fn record_batch(
    State(state): State<SharedState>,
    Json(body): Json<BrowseSignalsBatch>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if body.signals.is_empty() {
        return Ok(Json(serde_json::json!({"recorded": 0, "high_engagement": 0})));
    }
    if body.signals.len() > MAX_BATCH_SIZE {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({
                "error": format!("batch too large (max {MAX_BATCH_SIZE})")
            })),
        ));
    }

    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault
        .dek_db()
        .map_err(|_| crate::routes::errors::vault_locked())?;

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut recorded = 0usize;
    let mut failed_indices: Vec<usize> = Vec::new();
    let mut high_engagement = 0usize;
    let mut auto_bookmarked = 0usize;
    for (idx, signal) in body.signals.iter().enumerate() {
        // URL 协议白名单。仅允许 http/https；
        // javascript: / data: / file: 等协议是 XSS / 任意文件读取风险。
        // chrome 扩展虽在 manifest exclude chrome://，但页面 history.pushState 可
        // 注入伪协议 URL，必须后端兜底。
        if !signal.url.starts_with("https://") && !signal.url.starts_with("http://") {
            tracing::warn!("G1 reject non-http(s) URL at idx={idx}");
            failed_indices.push(idx);
            continue;
        }

        // 截断超长字段（防恶意页面 1MB title）
        let mut owned = signal.clone();
        owned.truncate_to_limits();

        if owned.is_high_engagement() {
            high_engagement += 1;
            // W4 G2 (2026-04-27): 高 engagement → auto_bookmark 候选表
            // per spec §3.G2 修订: 不入主 items 表；G3 (W5-6) worker SELECT WHERE promoted=0
            // 抓取真实正文后 insert_item + mark_auto_bookmark_promoted。
            // 失败 silent skip — 不阻断 browse_signals 主路径。
            // domain_hash 与 record_browse_signal 不同源 — 此处直接传 owned.domain() 字符串
            // 让 store fn 自己 hash（重新实现 hash_domain 在 store::browse_signals private，
            // 暴露 domain 给 store 层后由 record_auto_bookmark 重算 — 此处简化：
            // 用 list_recent_browse_signals 的 domain_hash 计算逻辑，读 store 已 record 的
            // 那条 row 的 domain_hash 作为 source of truth）。
            // 务实做法: 调用方传 domain 明文 + domain_hash（占用同一组 owned），
            // 让 store 端用 hash_domain 私有 fn 算 — 两表一致性靠同源代码路径保证。
            // 现版本: domain_hash 字段直接传空 + store 自己 hash（小不一致由 W5 收尾）
            let domain_hash_for_bookmark = format!(
                "pending:{}",
                owned.domain()
            );
            if let Err(e) = vault.store().record_auto_bookmark(
                &dek,
                &owned.url,
                &owned.title,
                &domain_hash_for_bookmark,
                owned.dwell_ms,
                owned.scroll_pct,
                owned.copy_count,
                owned.visit_count,
                now_secs,
            ) {
                tracing::warn!("G2 record_auto_bookmark failed at idx={idx}: {e}");
                // 不阻断主路径；G2 失败不让 G1 也失败
            } else {
                auto_bookmarked += 1;
            }
        }
        match vault.store().record_browse_signal(&dek, &owned, now_secs) {
            Ok(_) => recorded += 1,
            Err(e) => {
                tracing::warn!("G1 record_browse_signal failed at idx={idx}: {e}");
                failed_indices.push(idx);
            }
        }
    }

    Ok(Json(serde_json::json!({
        "recorded": recorded,
        "high_engagement": high_engagement,
        "auto_bookmarked": auto_bookmarked,  // 实际入候选表的条数
        // 返回失败 indices，让客户端能精准重试某几条
        "failed_indices": failed_indices,
    })))
}

/// GET /api/v1/browse_signals?limit=20 — 诊断查询
pub async fn list(
    State(state): State<SharedState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let limit = q.limit.min(200);
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault
        .dek_db()
        .map_err(|_| crate::routes::errors::vault_locked())?;
    let count = vault.store().browse_signals_count().unwrap_or(0);
    let signals = vault
        .store()
        .list_recent_browse_signals(&dek, limit)
        .map_err(|e| crate::routes::errors::internal("list_recent_browse_signals", e))?;
    Ok(Json(serde_json::json!({
        "count": count,
        "signals": signals,
    })))
}

/// DELETE /api/v1/browse_signals[?domain=example.com] — 全清或 per-domain
pub async fn delete(
    State(state): State<SharedState>,
    Query(q): Query<DeleteQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault
        .dek_db()
        .map_err(|_| crate::routes::errors::vault_locked())?;
    let n = match q.domain.as_deref() {
        Some(d) if !d.is_empty() => vault
            .store()
            .clear_browse_signals_for_domain(d)
            .unwrap_or(0),
        _ => vault.store().clear_all_browse_signals().unwrap_or(0),
    };
    Ok(Json(serde_json::json!({"deleted": n})))
}
