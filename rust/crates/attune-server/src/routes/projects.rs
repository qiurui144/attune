//! Project / Case 卷宗 REST API（spec §2.3）
//!
//! 6 endpoints：
//! - POST   /api/v1/projects                     创建项目
//! - GET    /api/v1/projects                     列出项目
//! - GET    /api/v1/projects/:id                 获取单个项目
//! - POST   /api/v1/projects/:id/files           关联文件到项目
//! - GET    /api/v1/projects/:id/files           列出项目的文件
//! - GET    /api/v1/projects/:id/timeline        列出项目时间线
//!
//! 所有端点都需要 vault unlocked（vault_guard middleware 已在 build_router 层
//! 拦截 locked 情形并返 403；handler 内仍保留 defensive check 以防中间件配置变更）。

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use attune_core::store::{Project, ProjectFile, ProjectTimelineEntry};
use attune_core::vault::VaultState;
use serde::{Deserialize, Serialize};

use crate::state::SharedState;

#[derive(Debug, Deserialize)]
pub struct CreateProjectRequest {
    pub title: String,
    /// 'generic' / 'case' / 'deal' / 'topic' / 任意 plugin 自定义类型 —
    /// attune-core 不约束。未指定时默认 'generic'。
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddFileRequest {
    pub file_id: String,
    /// 文件在该 project 中的角色，由 plugin / 调用方自由约定。
    /// 空字符串/None 表示未分类，attune-core 不约束取值集合。
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProjectListResponse {
    pub projects: Vec<Project>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct FilesListResponse {
    pub files: Vec<ProjectFile>,
}

#[derive(Debug, Serialize)]
pub struct TimelineResponse {
    pub entries: Vec<ProjectTimelineEntry>,
}

fn vault_locked_error() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({"error": "vault locked"})),
    )
}

fn internal_error(e: impl std::fmt::Display) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": e.to_string()})),
    )
}

/// POST /api/v1/projects
pub async fn create_project(
    State(state): State<SharedState>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<Project>), (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(vault.state(), VaultState::Unlocked) {
        return Err(vault_locked_error());
    }
    let kind = req.kind.as_deref().unwrap_or("generic");
    let title = req.title.trim();
    if title.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "title required"})),
        ));
    }
    let p = vault
        .store()
        .create_project(title, kind)
        .map_err(internal_error)?;
    Ok((StatusCode::CREATED, Json(p)))
}

/// GET /api/v1/projects?include_archived=false
pub async fn list_projects(
    State(state): State<SharedState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<ProjectListResponse>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(vault.state(), VaultState::Unlocked) {
        return Err(vault_locked_error());
    }
    let include_archived = q
        .get("include_archived")
        .map(|s| s == "true" || s == "1")
        .unwrap_or(false);
    let projects = vault
        .store()
        .list_projects(include_archived)
        .map_err(internal_error)?;
    let total = projects.len();
    Ok(Json(ProjectListResponse { projects, total }))
}

/// GET /api/v1/projects/:id
pub async fn get_project(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<Project>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(vault.state(), VaultState::Unlocked) {
        return Err(vault_locked_error());
    }
    let p = vault.store().get_project(&id).map_err(internal_error)?;
    match p {
        Some(p) => Ok(Json(p)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "project not found"})),
        )),
    }
}

/// POST /api/v1/projects/:id/files
pub async fn add_file_to_project(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(req): Json<AddFileRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(vault.state(), VaultState::Unlocked) {
        return Err(vault_locked_error());
    }
    let exists = vault
        .store()
        .get_project(&id)
        .map_err(internal_error)?
        .is_some();
    if !exists {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "project not found"})),
        ));
    }
    let role = req.role.as_deref().unwrap_or("");
    vault
        .store()
        .add_file_to_project(&id, &req.file_id, role)
        .map_err(internal_error)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({"status": "ok"}))))
}

/// GET /api/v1/projects/:id/files
pub async fn list_project_files(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<FilesListResponse>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(vault.state(), VaultState::Unlocked) {
        return Err(vault_locked_error());
    }
    let files = vault
        .store()
        .list_files_for_project(&id)
        .map_err(internal_error)?;
    Ok(Json(FilesListResponse { files }))
}

/// GET /api/v1/projects/:id/timeline
pub async fn list_project_timeline(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<TimelineResponse>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(vault.state(), VaultState::Unlocked) {
        return Err(vault_locked_error());
    }
    let entries = vault.store().list_timeline(&id).map_err(internal_error)?;
    Ok(Json(TimelineResponse { entries }))
}
