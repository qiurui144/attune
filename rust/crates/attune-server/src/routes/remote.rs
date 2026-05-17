use crate::state::SharedState;
use attune_core::scanner_webdav::WebDavConfig;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct BindRemoteRequest {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    #[serde(default = "default_depth")]
    pub depth: u32,
    /// 语料领域（legal / tech / medical / patent / general），驱动 F-Pro
    /// 跨域防污染。缺省 general。
    pub corpus_domain: Option<String>,
}
fn default_depth() -> u32 {
    1
}

/// POST /api/v1/index/bind-remote — 绑定远程 WebDAV 目录并扫描入库。
///
/// route 层直接驱动 WebDavConnector，ETag 增量判断在此内联（对应 Task 11 的
/// sync_webdav_dir 公共函数抽取点）。响应 scan 字段形态与重构前完全一致。
pub async fn bind_remote(
    State(state): State<SharedState>,
    Json(body): Json<BindRemoteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if body.depth > 2 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "depth must be <= 2 to prevent runaway directory traversal"})),
        ));
    }
    let config = WebDavConfig {
        url: body.url.clone(),
        username: body.username.clone(),
        password: body.password.clone(),
        depth: body.depth,
    };

    // 创建/复用 bound_dirs 记录（webdav: 前缀标记远程目录）。
    let dir_id = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let _ = vault.dek_db().map_err(|e| {
            (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;

        vault
            .store()
            .bind_directory(&format!("webdav:{}", body.url), false, &["md", "txt"])
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": e.to_string()})),
                )
            })?
    };

    // 决策 4：落库加密 remote 配置，让周期 worker 能读回凭据自动重扫。
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|e| {
            (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
        })?;
        let input = attune_core::store::webdav_remotes::WebDavRemoteInput {
            dir_id: dir_id.clone(),
            url: body.url.clone(),
            username: body.username.clone(),
            password: body.password.clone(),
            depth: body.depth,
            corpus_domain: body.corpus_domain.clone().unwrap_or_else(|| "general".into()),
        };
        if let Err(e) = vault.store().upsert_webdav_remote(&dek, &input) {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("persist webdav remote: {e}")})),
            ));
        }
    }

    // WebDAV I/O 是阻塞的 —— 在 spawn_blocking 里跑，不阻塞 axum worker。
    let corpus_domain = body.corpus_domain.clone().unwrap_or_else(|| "general".into());
    let state_clone = state.clone();
    let dir_id_clone = dir_id.clone();
    let scan = tokio::task::spawn_blocking(move || {
        crate::ingest_webdav::sync_webdav_dir(&state_clone, &dir_id_clone, config, &corpus_domain)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e})),
        )
    })?;

    Ok(Json(serde_json::json!({
        "status": "ok",
        "dir_id": dir_id,
        "scan": scan,
    })))
}
