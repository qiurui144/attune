//! v0.6 Phase A.5.3 — 出网审计日志 HTTP 路由
//!
//! 端点：
//! - `GET /api/v1/audit/outbound?from_ms=&to_ms=&limit=` — JSON 列表（合规员前端 / 用户面板）
//! - `GET /api/v1/audit/outbound/export.csv?from_ms=&to_ms=` — CSV 流式导出（合规员典型流程）

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Json;
use serde::Deserialize;

use crate::routes::errors::{internal, vault_locked};
use crate::error::AppResult;
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub from_ms: Option<i64>,
    #[serde(default)]
    pub to_ms: Option<i64>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    100
}

#[derive(Deserialize)]
pub struct ExportQuery {
    #[serde(default)]
    pub from_ms: Option<i64>,
    #[serde(default)]
    pub to_ms: Option<i64>,
}

/// GET /api/v1/audit/outbound — 出网审计列表
///
/// 不需要 vault DEK：审计字段全部明文（合规员要直接读 timestamp/model/redaction_count）。
/// 但仍要求 vault 已 unlock — 防止任何外部进程绕开身份验证拉日志。
pub async fn list(
    State(state): State<SharedState>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let limit = q.limit.min(10_000);
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|_| vault_locked())?;

    let events = vault
        .store()
        .list_outbound_audit(q.from_ms, q.to_ms, limit)
        .map_err(|e| internal("list_outbound_audit", e))?;

    Ok(Json(serde_json::json!({
        "total": events.len(),
        "items": events,
    })))
}

/// GET /api/v1/audit/outbound/export.csv — CSV 流式导出
pub async fn export_csv(
    State(state): State<SharedState>,
    Query(q): Query<ExportQuery>,
) -> AppResult<Response> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|_| vault_locked())?;

    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    let count = vault
        .store()
        .export_outbound_csv(q.from_ms, q.to_ms, &mut buf)
        .map_err(|e| internal("export_outbound_csv", e))?;

    // attune-server 不直接依赖 chrono；用 SystemTime 生成时间戳后缀
    let ts_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let filename = format!("attune-audit-{ts_secs}.csv");

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header("X-Audit-Row-Count", count.to_string())
        .body(Body::from(buf))
        .map_err(|e| internal("build csv response", e))
}

// ---------------------------------------------------------------------------
// v0.7: 简版 audit_log (route + CSV export) — F-17 PII redact 调用入口
//
// 路由（待 lib.rs 挂载）：
//   GET /api/v1/audit/log?since=<unix>&limit=&offset=  → JSON {total, items}
//   GET /api/v1/audit/log.csv?since=<unix>             → text/csv; charset=utf-8
// ---------------------------------------------------------------------------

use attune_core::store::audit::{self as audit_log};

#[derive(Deserialize)]
pub struct LogListQuery {
    #[serde(default)]
    pub since: Option<i64>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

#[derive(Deserialize)]
pub struct LogExportQuery {
    #[serde(default)]
    pub since: Option<i64>,
}

/// GET /api/v1/audit/log — 简版 audit_log 列表
pub async fn list_log(
    State(state): State<SharedState>,
    Query(q): Query<LogListQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|_| vault_locked())?;

    let store = vault.store();
    let items = match q.since {
        Some(s) => store.audit_log_list_since(s).map_err(|e| internal("audit log list_since", e))?,
        None => store
            .audit_log_list(q.limit.min(10_000), q.offset)
            .map_err(|e| internal("audit log list", e))?,
    };
    let total = store.audit_log_count().map_err(|e| internal("audit log count", e))?;
    Ok(Json(serde_json::json!({
        "total": total,
        "items": items,
    })))
}

/// GET /api/v1/audit/log.csv — 简版 audit_log CSV 导出
pub async fn export_log_csv(
    State(state): State<SharedState>,
    Query(q): Query<LogExportQuery>,
) -> AppResult<Response> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|_| vault_locked())?;

    let store = vault.store();
    let items = match q.since {
        Some(s) => store.audit_log_list_since(s).map_err(|e| internal("audit log list_since", e))?,
        None => store
            .audit_log_list(1_000_000, 0)
            .map_err(|e| internal("audit log list", e))?,
    };
    let csv = audit_log::entries_to_csv(&items);
    let row_count = items.len();

    let ts_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let filename = format!("attune-audit-log-{ts_secs}.csv");

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header("X-Audit-Row-Count", row_count.to_string())
        .body(Body::from(csv))
        .map_err(|e| internal("build csv response", e))
}
