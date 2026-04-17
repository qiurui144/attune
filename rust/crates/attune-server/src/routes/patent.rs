// npu-vault/crates/vault-server/src/routes/patent.rs
//
// POST /api/v1/patent/search  — 网络检索专利 + 可选自动入库
// GET  /api/v1/patent/databases — 列出支持的数据库

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::state::SharedState;
use attune_core::scanner_patent::{ingest_patent_records, search_patents, PatentDatabase, PatentQuery};

/// 单次查询关键词最大字节数
const MAX_QUERY_BYTES: usize = 500;
/// 最大返回 / 入库条数
const MAX_LIMIT: usize = 20;

#[derive(Deserialize)]
pub struct PatentSearchRequest {
    /// 检索关键词（中英文均可）
    pub q: String,
    /// 最大返回条数（1–20，超出自动截断）
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// 目标数据库：目前支持 "uspto"
    #[serde(default = "default_database")]
    pub database: String,
    /// 可选 IPC 大类过滤（如 "G06F"）
    pub ipc_filter: Option<String>,
    /// 是否自动将结果入库（默认 false）
    #[serde(default)]
    pub auto_ingest: bool,
}

fn default_limit() -> usize { 10 }
fn default_database() -> String { "uspto".into() }

/// POST /api/v1/patent/search
pub async fn search(
    State(state): State<SharedState>,
    Json(body): Json<PatentSearchRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // 输入校验
    if body.q.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "query must not be empty"})),
        ));
    }
    if body.q.len() > MAX_QUERY_BYTES {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "query too long (max 500 bytes)"})),
        ));
    }
    let limit = body.limit.min(MAX_LIMIT).max(1);

    let database = match body.database.to_lowercase().as_str() {
        "uspto" => PatentDatabase::Uspto,
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("unsupported database: {other}. Supported: uspto")
                })),
            ));
        }
    };

    let query = PatentQuery {
        keywords: body.q.trim().to_string(),
        limit,
        database,
        ipc_filter: body.ipc_filter.clone(),
    };
    let auto_ingest = body.auto_ingest;

    // 若需要 auto_ingest，提前获取 DEK（在 async 上下文中）
    let dek_opt = if auto_ingest {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        Some(vault.dek_db().map_err(|e| (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        ))?)
    } else {
        None
    };

    // 在 blocking 线程执行网络查询（使用同步 reqwest）
    let state_clone = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut search_result = search_patents(&query)?;

        if let Some(dek) = dek_opt {
            let vault = state_clone.vault.lock().unwrap_or_else(|e| e.into_inner());
            let ingested = ingest_patent_records(vault.store(), &dek, &search_result.records)?;
            search_result.ingested = ingested;
        }

        Ok::<_, attune_core::error::VaultError>(search_result)
    })
    .await
    .map_err(|e| (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": format!("task join error: {e}")})),
    ))?
    .map_err(|e| (
        StatusCode::BAD_GATEWAY,
        Json(serde_json::json!({"error": e.to_string()})),
    ))?;

    Ok(Json(serde_json::json!({
        "database": result.database,
        "keywords": result.keywords,
        "total_found": result.total_found,
        "count": result.records.len(),
        "ingested": result.ingested,
        "results": result.records.iter().map(|r| serde_json::json!({
            "patent_number": r.patent_number,
            "title": r.title,
            "abstract": r.abstract_text,
            "grant_date": r.grant_date,
            "assignees": r.assignees,
            "inventors": r.inventors,
            "ipc_classes": r.ipc_classes,
            "source_url": r.source_url,
        })).collect::<Vec<_>>(),
    })))
}

/// GET /api/v1/patent/databases — 返回支持的专利数据库列表及说明
pub async fn databases() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "databases": [
            {
                "id": "uspto",
                "name": "USPTO PatentsView",
                "description": "美国专利及商标局公开数据库，覆盖1976年至今的美国专利，无需认证",
                "coverage": "US patents 1976–present",
                "auth_required": false,
                "rate_limit": "45 req/min"
            }
        ]
    }))
}
