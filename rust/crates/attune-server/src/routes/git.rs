//! Git 仓导入 route —— bind-git（首次绑定 + 全量）/ sync-git（增量）。
//!
//! 与 routes/remote.rs（WebDAV）同模式：vault 必须 unlocked（middleware 守门），
//! token 在 body 接收后**立即加密**落 git_sources.token_ref_enc（不明文持久化 /
//! 不回显 / 不进日志，per 全局 §1.4）；clone + ingest 走 spawn_blocking 不阻塞
//! axum worker。
//!
//! 错误码（per spec §7，kebab）由 ingest_git / connector 错误 message 的前缀携带，
//! 本层 `git_error_response` 解析前缀 → HTTP status + 透传 code，不被 AppError 的
//! 通用 code 覆盖。

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use attune_core::store::git_sources::GitSourceInput;

use crate::state::SharedState;

#[derive(Deserialize)]
pub struct BindGitRequest {
    pub url: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub subdir: Option<String>,
    #[serde(default)]
    pub include_glob: Vec<String>,
    #[serde(default)]
    pub exclude_glob: Vec<String>,
    #[serde(default)]
    pub corpus_domain: Option<String>,
    /// 私有仓 PAT —— 立即加密落库, 不明文持久化 / 不回显。
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub max_files: Option<u64>,
    /// 额外允许的自建 git host（SSRF allowlist 扩展）。
    #[serde(default)]
    pub allow_hosts: Vec<String>,
}

#[derive(Deserialize)]
pub struct SyncGitRequest {
    pub dir_id: String,
}

type RouteResult = Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)>;

/// 把 git 错误（message 带 kebab 前缀）映射到 HTTP status + 透传 code。
fn git_error_response(msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    // message 形如 "git-auth-failed: ...";取 ':' 前的 code。
    let code = msg.split(':').next().unwrap_or("git-error").trim();
    let status = match code {
        "invalid-git-url" | "git-url-not-allowed" => StatusCode::BAD_REQUEST,
        "git-auth-failed" => StatusCode::BAD_GATEWAY,
        "git-repo-not-found" | "git-ref-not-found" => StatusCode::NOT_FOUND,
        "git-repo-too-large" => StatusCode::PAYLOAD_TOO_LARGE,
        "git-cli-missing" => StatusCode::SERVICE_UNAVAILABLE,
        "git-network-error" => StatusCode::BAD_GATEWAY,
        _ => StatusCode::BAD_GATEWAY,
    };
    // error message 已脱敏（connector map_git_err 不含 token / 内网细节）。
    (status, Json(json!({ "error": msg, "code": code })))
}

/// POST /api/v1/index/bind-git — 绑定 git 仓并全量导入（对齐 bind-remote）。
pub async fn bind_git(State(state): State<SharedState>, Json(body): Json<BindGitRequest>) -> RouteResult {
    // 归一 URL + SSRF 预校验（fail fast，clone 前就拒非法 / 内网）。
    let config = {
        let mut c = attune_core::ingest::git::GitSourceConfig::new(body.url.trim());
        c.branch = body.branch.clone();
        c.subdir = body.subdir.clone();
        c.include_glob = body.include_glob.clone();
        c.exclude_glob = body.exclude_glob.clone();
        c.corpus_domain = body.corpus_domain.clone();
        c.token = body.token.clone();
        c.allow_hosts = body.allow_hosts.clone();
        if let Some(m) = body.max_files {
            c.max_files = m.min(attune_core::ingest::git::GitSourceConfig::DEFAULT_MAX_FILES);
        }
        c
    };
    let connector = match attune_core::ingest::git::GitConnector::with_cloner(
        config.clone(),
        Box::new(attune_core::ingest::git::Git2Cloner),
    ) {
        Ok(c) => c,
        Err(e) => return Err(git_error_response(&e.to_string())),
    };
    if let Err(e) = connector.check_ssrf(&|h| attune_core::net::url_guard::system_resolve(h)) {
        return Err(git_error_response(&e.to_string()));
    }
    let normalized = match attune_core::ingest::git::normalize_url(body.url.trim()) {
        Ok(n) => n,
        Err(e) => return Err(git_error_response(&e.to_string())),
    };

    // bound_dir id = "git:<normalized-url>#<ref>"。
    let bind_key = format!(
        "git:{}#{}",
        normalized.clone_url,
        config.branch.clone().unwrap_or_else(|| "HEAD".into())
    );

    let dir_id = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let _ = vault.dek_db().map_err(|e| {
            (StatusCode::FORBIDDEN, Json(json!({"error": e.to_string(), "code": "unauthorized"})))
        })?;
        vault
            .store()
            .bind_directory(&bind_key, false, &["md", "txt", "rst", "rs", "py"])
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string(), "code": "internal"}))))?
    };

    // 落库加密配置（token 立即加密；不回 body 明文 / 不入日志）。
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|e| {
            (StatusCode::FORBIDDEN, Json(json!({"error": e.to_string(), "code": "unauthorized"})))
        })?;
        let input = GitSourceInput {
            dir_id: dir_id.clone(),
            url: normalized.clone_url.clone(),
            host: normalized.host.clone(),
            branch: config.branch.clone(),
            subdir: config.subdir.clone(),
            include_glob: serde_json::to_string(&config.include_glob).unwrap_or_default(),
            exclude_glob: serde_json::to_string(&config.exclude_glob).unwrap_or_default(),
            corpus_domain: config.corpus_domain.clone().unwrap_or_else(|| "general".into()),
            // token 明文进 GitSourceInput.token_ref, 由 upsert_git_source 用 dek
            // 加密落 token_ref_enc (不回显 / 不日志)。
            token_ref: body.token.clone(),
            max_files: config.max_files,
            max_file_bytes: config.max_file_bytes,
            max_total_bytes: config.max_total_bytes,
        };
        if let Err(e) = vault.store().upsert_git_source(&dek, &input) {
            return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("persist git source: {e}"), "code": "internal"}))));
        }
    }

    // clone + ingest 走 spawn_blocking（token 从加密配置解密后注入内存）。
    let token = body.token.clone();
    let state_clone = state.clone();
    let dir_id_clone = dir_id.clone();
    let scan = tokio::task::spawn_blocking(move || {
        crate::ingest_git::sync_git_source(&state_clone, &dir_id_clone, token)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string(), "code": "internal"}))))?;

    match scan {
        Ok(scan) => Ok(Json(json!({ "status": "ok", "dir_id": dir_id, "scan": scan }))),
        Err(msg) => Err(git_error_response(&msg)),
    }
}

/// POST /api/v1/index/sync-git — 手动增量同步（token 从加密配置取）。
pub async fn sync_git(State(state): State<SharedState>, Json(body): Json<SyncGitRequest>) -> RouteResult {
    let dir_id = body.dir_id.clone();
    // 取 token（解密）。
    let token = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|e| {
            (StatusCode::FORBIDDEN, Json(json!({"error": e.to_string(), "code": "unauthorized"})))
        })?;
        match vault.store().get_git_source(&dek, &dir_id) {
            Ok(Some(row)) => row.token_ref,
            Ok(None) => {
                return Err((StatusCode::NOT_FOUND, Json(json!({"error": format!("git source {dir_id}"), "code": "not-found"}))))
            }
            Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string(), "code": "internal"})))),
        }
    };

    let state_clone = state.clone();
    let dir_id_clone = dir_id.clone();
    let scan = tokio::task::spawn_blocking(move || {
        crate::ingest_git::sync_git_source(&state_clone, &dir_id_clone, token)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string(), "code": "internal"}))))?;

    match scan {
        Ok(scan) => Ok(Json(json!({ "status": "ok", "dir_id": dir_id, "scan": scan }))),
        Err(msg) => Err(git_error_response(&msg)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_error_response_maps_codes() {
        assert_eq!(git_error_response("invalid-git-url: x").0, StatusCode::BAD_REQUEST);
        assert_eq!(git_error_response("git-url-not-allowed: x").0, StatusCode::BAD_REQUEST);
        assert_eq!(git_error_response("git-auth-failed: x").0, StatusCode::BAD_GATEWAY);
        assert_eq!(git_error_response("git-repo-not-found: x").0, StatusCode::NOT_FOUND);
        assert_eq!(git_error_response("git-ref-not-found: x").0, StatusCode::NOT_FOUND);
        assert_eq!(git_error_response("git-repo-too-large: x").0, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(git_error_response("git-network-error: x").0, StatusCode::BAD_GATEWAY);
        assert_eq!(git_error_response("git-cli-missing: x").0, StatusCode::SERVICE_UNAVAILABLE);
    }
}
