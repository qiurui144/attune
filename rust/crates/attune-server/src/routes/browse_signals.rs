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
    let dek = vault.dek_db().map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut recorded = 0usize;
    let mut failed_indices: Vec<usize> = Vec::new();
    let mut high_engagement = 0usize;
    for (idx, signal) in body.signals.iter().enumerate() {
        // per reviewer I3：截断超长字段（防恶意页面 1MB title）
        let mut owned = signal.clone();
        owned.truncate_to_limits();

        if owned.is_high_engagement() {
            high_engagement += 1;
            // per reviewer N4 + spec §3.G2：本批次仅计数，不创建 item — 留 G3 (W5-6)
            // 真正的 page content extraction 后再 insert_item with extracted body。
            // spec 段落"creates a placeholder item via Store::insert_item" 已在 spec
            // 内更新为"defer to G3 + extract content together"。
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
        // per reviewer I2：返回失败 indices，让客户端能精准重试某几条
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
    let dek = vault.dek_db().map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;
    let count = vault.store().browse_signals_count().unwrap_or(0);
    let signals = vault
        .store()
        .list_recent_browse_signals(&dek, limit)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;
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
    let _ = vault.dek_db().map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;
    let n = match q.domain.as_deref() {
        Some(d) if !d.is_empty() => vault
            .store()
            .clear_browse_signals_for_domain(d)
            .unwrap_or(0),
        _ => vault.store().clear_all_browse_signals().unwrap_or(0),
    };
    Ok(Json(serde_json::json!({"deleted": n})))
}
