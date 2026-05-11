//! /api/v1/ocr/profiles — OCR 场景预设 CRUD.
//!
//! 设计:
//! - GET 任何 tier 可读
//! - POST/PUT/DELETE 受 `SettingsLocks::ocr_profiles` lock 控制 (per for_state 规则: Paid
//!   也 Editable, 因为场景预设不属于"会员锁定项" — 见 member_session.rs §locks)
//! - builtin profile 拒删拒改 (registry 层强制)
//! - 持久化磁盘文件每个请求现 load — profile 列表 ~KB 数量级, 不是热点

use crate::state::SharedState;
use attune_core::member_session::SettingsLocks;
use attune_core::ocr::profile::OcrProfile;
use attune_core::ocr::profile_registry::ProfileRegistry;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

fn lock_forbidden() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": "ocr_profiles 设置项已被 lock — 当前会员等级不允许修改"
        })),
    )
}

fn registry_err(e: attune_core::error::VaultError) -> (StatusCode, Json<serde_json::Value>) {
    let (code, msg) = match &e {
        attune_core::error::VaultError::NotFound(_) => (StatusCode::NOT_FOUND, e.to_string()),
        attune_core::error::VaultError::InvalidInput(_) => (StatusCode::BAD_REQUEST, e.to_string()),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    (code, Json(serde_json::json!({ "error": msg })))
}

fn can_write(state: &SharedState) -> bool {
    let m = state.member_state.lock().unwrap_or_else(|e| e.into_inner()).clone();
    SettingsLocks::for_state(&m).can_edit("ocr_profiles")
}

fn load_registry() -> Result<ProfileRegistry, (StatusCode, Json<serde_json::Value>)> {
    ProfileRegistry::load_default().map_err(registry_err)
}

/// GET /api/v1/ocr/profiles
pub async fn list_profiles() -> Result<Json<Vec<OcrProfile>>, (StatusCode, Json<serde_json::Value>)>
{
    let reg = load_registry()?;
    Ok(Json(reg.list().to_vec()))
}

/// POST /api/v1/ocr/profiles
pub async fn create_profile(
    State(state): State<SharedState>,
    Json(p): Json<OcrProfile>,
) -> Result<Json<OcrProfile>, (StatusCode, Json<serde_json::Value>)> {
    if !can_write(&state) {
        return Err(lock_forbidden());
    }
    let mut reg = load_registry()?;
    if reg.get(&p.id).is_some() {
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": format!("profile {} 已存在, 用 PUT 修改", p.id)
            })),
        ));
    }
    reg.upsert(p.clone()).map_err(registry_err)?;
    Ok(Json(reg.get(&p.id).cloned().unwrap_or(p)))
}

/// PUT /api/v1/ocr/profiles/:id
pub async fn update_profile(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(mut p): Json<OcrProfile>,
) -> Result<Json<OcrProfile>, (StatusCode, Json<serde_json::Value>)> {
    if !can_write(&state) {
        return Err(lock_forbidden());
    }
    p.id = id.clone();
    let mut reg = load_registry()?;
    if reg.get(&id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("profile {} 不存在", id) })),
        ));
    }
    reg.upsert(p.clone()).map_err(registry_err)?;
    Ok(Json(reg.get(&id).cloned().unwrap_or(p)))
}

/// DELETE /api/v1/ocr/profiles/:id
pub async fn delete_profile(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !can_write(&state) {
        return Err(lock_forbidden());
    }
    let mut reg = load_registry()?;
    reg.delete(&id).map_err(registry_err)?;
    Ok(Json(serde_json::json!({ "status": "deleted", "id": id })))
}
