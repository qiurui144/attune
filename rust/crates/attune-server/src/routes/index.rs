use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::state::SharedState;
use attune_core::scanner;

#[derive(Deserialize)]
pub struct BindRequest {
    pub path: String,
    #[serde(default = "default_true")]
    pub recursive: bool,
    #[serde(default = "default_file_types")]
    pub file_types: Vec<String>,
    /// v0.6 Phase B F-Pro：bind 时声明 corpus 领域用于跨域 retrieval 防污染。
    /// 'legal' / 'tech' / 'medical' / 'patent' / 'academic' / 'general'(默认)。
    #[serde(default = "default_corpus_domain")]
    pub corpus_domain: String,
}

fn default_corpus_domain() -> String {
    "general".to_string()
}

fn default_true() -> bool {
    true
}

fn default_file_types() -> Vec<String> {
    vec![
        "md".into(),
        "txt".into(),
        "py".into(),
        "js".into(),
        "rs".into(),
    ]
}

#[derive(Deserialize)]
pub struct UnbindQuery {
    pub dir_id: String,
}

/// Validates that a raw path string is:
/// 1. An absolute path
/// 2. Exists and is a directory (via canonicalization)
/// 3. Within the user's home directory
///
/// Returns the canonicalized PathBuf on success.
pub fn validate_bind_path(
    raw: &str,
    home: &std::path::Path,
) -> Result<std::path::PathBuf, (StatusCode, Json<serde_json::Value>)> {
    let path = std::path::Path::new(raw);

    if !path.is_absolute() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "path must be absolute"})),
        ));
    }

    let canonical = path.canonicalize().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "directory not found or inaccessible"})),
        )
    })?;

    if !canonical.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "path is not a directory"})),
        ));
    }

    if !canonical.starts_with(home) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "path must be within the user home directory",
                "home": home.display().to_string(),
            })),
        ));
    }

    Ok(canonical)
}

pub async fn bind_directory(
    State(state): State<SharedState>,
    Json(body): Json<BindRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    let home = dirs::home_dir().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "internal error"})),
        )
    })?;
    let canonical = validate_bind_path(&body.path, &home)?;

    // 使用规范化后的路径字符串
    let canonical_str = canonical.display().to_string();

    let file_type_strs: Vec<&str> = body.file_types.iter().map(|s| s.as_str()).collect();
    let dir_id = vault
        .store()
        .bind_directory_with_domain(&canonical_str, body.recursive, &file_type_strs, &body.corpus_domain)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;

    // Scan directory synchronously
    let scan_result =
        scanner::scan_directory(vault.store(), &dek, &dir_id, &canonical, body.recursive, &body.file_types)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": e.to_string()})),
                )
            })?;

    {
        let ft_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ft) = ft_guard.as_ref() {
            if let Ok(ids) = vault.store().list_all_item_ids() {
                for id in &ids {
                    if let Ok(Some(item)) = vault.store().get_item(&dek, id) {
                        let _ = ft.add_document(&item.id, &item.title, &item.content, &item.source_type);
                    }
                }
            }
        }
    }

    Ok(Json(serde_json::json!({
        "status": "ok",
        "dir_id": dir_id,
        "scan": {
            "total": scan_result.total_files,
            "new": scan_result.new_files,
            "updated": scan_result.updated_files,
            "skipped": scan_result.skipped_files,
        }
    })))
}

pub async fn unbind_directory(
    State(state): State<SharedState>,
    Query(params): Query<UnbindQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    vault.store().unbind_directory(&params.dir_id).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    Ok(Json(serde_json::json!({"status": "ok"})))
}

pub async fn index_status(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    let dirs = vault.store().list_bound_directories().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;
    let pending = vault.store().pending_embedding_count().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    Ok(Json(serde_json::json!({
        "directories": dirs,
        "pending_embeddings": pending,
    })))
}
