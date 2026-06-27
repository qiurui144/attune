//! Project 卷宗 REST API（spec §2.3）— 通用项目/卷宗管理（不绑定具体行业）
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
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use attune_core::store::{ConversationSummary, Project, ProjectFile, ProjectTimelineEntry};
use attune_core::vault::VaultState;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
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

fn vault_locked_error() -> AppError {
    AppError::Forbidden("vault locked".into())
}

fn internal_error(e: impl std::fmt::Display) -> AppError {
    AppError::Internal(e.to_string())
}

/// POST /api/v1/projects
pub async fn create_project(
    State(state): State<SharedState>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<Project>), AppError> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(vault.state(), VaultState::Unlocked) {
        return Err(vault_locked_error());
    }
    let kind = req.kind.as_deref().unwrap_or("generic");
    let title = req.title.trim();
    if title.is_empty() {
        return Err(AppError::BadRequest("title required".into()));
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
) -> Result<Json<ProjectListResponse>, AppError> {
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
) -> Result<Json<Project>, AppError> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(vault.state(), VaultState::Unlocked) {
        return Err(vault_locked_error());
    }
    let p = vault.store().get_project(&id).map_err(internal_error)?;
    match p {
        Some(p) => Ok(Json(p)),
        None => Err(AppError::NotFound("project not found".into())),
    }
}

/// POST /api/v1/projects/:id/files
pub async fn add_file_to_project(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(req): Json<AddFileRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
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
        return Err(AppError::NotFound("project not found".into()));
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
) -> Result<Json<FilesListResponse>, AppError> {
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

#[derive(Debug, Deserialize)]
pub struct ConversationsQuery {
    #[serde(default = "default_conv_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_conv_limit() -> usize {
    50
}

#[derive(Debug, Serialize)]
pub struct ProjectConversationsResponse {
    pub conversations: Vec<ConversationSummary>,
    pub total: usize,
}

/// GET /api/v1/projects/:id/conversations
///
/// chat-centric IA (2026-06-26): list a project's branch conversations (the chats
/// created under this project). 404 if the project does not exist; loose
/// conversations are never included.
pub async fn list_project_conversations(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Query(q): Query<ConversationsQuery>,
) -> Result<Json<ProjectConversationsResponse>, AppError> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(vault.state(), VaultState::Unlocked) {
        return Err(vault_locked_error());
    }
    let dek = vault
        .dek_db()
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    // Project must exist → 404 otherwise (do not return an empty list for a
    // nonexistent project, which would mask a client bug).
    if vault.store().get_project(&id).map_err(internal_error)?.is_none() {
        return Err(AppError::NotFound("project not found".into()));
    }
    let limit = q.limit.min(200);
    let conversations = vault
        .store()
        .list_conversations_scoped(&dek, Some(Some(&id)), limit, q.offset)
        .map_err(internal_error)?;
    let total = conversations.len();
    Ok(Json(ProjectConversationsResponse { conversations, total }))
}

/// GET /api/v1/projects/:id/timeline
pub async fn list_project_timeline(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<TimelineResponse>, AppError> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(vault.state(), VaultState::Unlocked) {
        return Err(vault_locked_error());
    }
    let entries = vault.store().list_timeline(&id).map_err(internal_error)?;
    Ok(Json(TimelineResponse { entries }))
}
