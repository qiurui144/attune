//! RSS / Atom 订阅 route —— 订阅 CRUD + 手动 poll 触发。
//!
//! 与 routes/email.rs 同模式：vault 必须 unlocked（middleware 已守门），写入路径
//! 解密 URL 落 store，sync 走 spawn_blocking 避免阻塞 axum worker。
//!
//! ⚠ 这些 route 仅注册 HTTP handler；vault auth middleware（require_auth 路径在
//! lib.rs::build_router 加 vault_guard）保证 Locked vault 不能调到这里。

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use attune_core::store::rss_feeds::RssFeedInput;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct AddFeedRequest {
    pub url: String,
    #[serde(default)]
    pub name: String,
    /// `None` → 用 store 层 DEFAULT_POLL_INTERVAL_MINUTES (60)。
    #[serde(default)]
    pub poll_interval_minutes: Option<u32>,
}

#[derive(Deserialize)]
pub struct UpdateFeedRequest {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub poll_interval_minutes: Option<u32>,
}

#[derive(Serialize)]
pub struct FeedView {
    pub id: String,
    pub name: String,
    pub url: String,
    pub last_entry_guid: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub last_polled_at: Option<String>,
    pub poll_interval_minutes: u32,
    pub enabled: bool,
}

/// 校验 URL：非空 + http/https scheme。
/// 不校验 DNS / 可达性 —— add-feed 不应因为短暂网络抖动失败；首轮 poll 自然会暴露问题。
fn validate(req: &AddFeedRequest) -> Result<(), AppError> {
    let trimmed = req.url.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest("url must not be empty".into()));
    }
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return Err(AppError::BadRequest(
            "url must start with http:// or https://".into(),
        ));
    }
    if let Some(iv) = req.poll_interval_minutes {
        if iv == 0 {
            return Err(AppError::BadRequest(
                "poll_interval_minutes must be > 0".into(),
            ));
        }
    }
    Ok(())
}

/// GET /api/v1/sources/rss/feeds —— 列出已订阅 feed（URL 已解密展示给本地 UI）。
pub async fn list_feeds(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db()?;
    let rows = vault.store().list_rss_feeds(&dek)?;
    let feeds: Vec<FeedView> = rows
        .into_iter()
        .map(|r| FeedView {
            id: r.id,
            name: r.name,
            url: r.url,
            last_entry_guid: r.last_entry_guid,
            etag: r.etag,
            last_modified: r.last_modified,
            last_polled_at: r.last_polled_at,
            poll_interval_minutes: r.poll_interval_minutes,
            enabled: r.enabled,
        })
        .collect();
    Ok(Json(serde_json::json!({ "feeds": feeds })))
}

/// POST /api/v1/sources/rss/feeds —— 新增订阅并立即触发首轮 poll。
pub async fn add_feed(
    State(state): State<SharedState>,
    Json(req): Json<AddFeedRequest>,
) -> AppResult<Json<serde_json::Value>> {
    validate(&req)?;
    let url_trimmed = req.url.trim().to_string();

    let feed_id = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db()?;
        let input = RssFeedInput {
            name: req.name.trim().to_string(),
            url: url_trimmed,
            poll_interval_minutes: req.poll_interval_minutes,
        };
        vault
            .store()
            .add_rss_feed(&dek, &input)
            .map_err(|e| AppError::Internal(format!("persist rss feed: {e}")))?
    };

    // 首轮 poll 在 spawn_blocking 跑（HTTP I/O + DB 写）—— 不阻塞 axum worker。
    let state_cloned = state.clone();
    let feed_cloned = feed_id.clone();
    let stats = tokio::task::spawn_blocking(move || {
        crate::ingest_rss::sync_rss_feed(&state_cloned, &feed_cloned)
    })
    .await
    .map_err(|e| AppError::Internal(format!("rss sync task join: {e}")))?
    .map_err(AppError::BadGateway)?;

    Ok(Json(serde_json::json!({
        "id": feed_id,
        "sync": stats,
    })))
}

/// DELETE /api/v1/sources/rss/feeds/:id —— 删除订阅。已 ingest 的 item 保留。
pub async fn delete_feed(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db()?;
    vault
        .store()
        .delete_rss_feed(&id)
        .map_err(|e| AppError::Internal(format!("delete rss feed: {e}")))?;
    Ok(Json(serde_json::json!({ "deleted": id })))
}

/// PATCH /api/v1/sources/rss/feeds/:id —— 切换 enabled / 修改 poll 周期。
pub async fn update_feed(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateFeedRequest>,
) -> AppResult<Json<serde_json::Value>> {
    if let Some(iv) = req.poll_interval_minutes {
        if iv == 0 {
            return Err(AppError::BadRequest(
                "poll_interval_minutes must be > 0".into(),
            ));
        }
    }
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db()?;
    vault
        .store()
        .update_rss_feed_settings(&id, req.enabled, req.poll_interval_minutes)
        .map_err(|e| AppError::Internal(format!("update rss feed: {e}")))?;
    Ok(Json(serde_json::json!({ "updated": id })))
}

/// POST /api/v1/sources/rss/feeds/:id/poll —— 手动触发一次条件 GET 增量。
/// 与周期 worker 共用 sync_rss_feed 实现。
pub async fn poll_feed_now(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    // 存在性校验 + dek 解锁验证（vault 未 unlock 在此就抛 401）。
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db()?;
        let exists = vault
            .store()
            .get_rss_feed(&dek, &id)
            .map_err(|e| AppError::Internal(format!("get rss feed: {e}")))?;
        if exists.is_none() {
            return Err(AppError::NotFound(format!("rss feed {id}")));
        }
    }
    let state_cloned = state.clone();
    let id_cloned = id.clone();
    let stats = tokio::task::spawn_blocking(move || {
        crate::ingest_rss::sync_rss_feed(&state_cloned, &id_cloned)
    })
    .await
    .map_err(|e| AppError::Internal(format!("rss sync task join: {e}")))?
    .map_err(AppError::BadGateway)?;

    Ok(Json(serde_json::json!({
        "id": id,
        "sync": stats,
    })))
}
