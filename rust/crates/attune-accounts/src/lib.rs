//! attune-accounts — reference SaaS impl for device 1:2 binding.
//!
//! 给生产 SaaS 部署方一份可工作的 reference. 实际生产应替换为真正的 PostgreSQL +
//! 鉴权 / 计费 / 多区域 / 监控 等. 此 crate 是 OSS attune 验证客户端流程用.
//!
//! Endpoints (per attune docs/specs/attune-plugin-protocol.md §10):
//! - POST /api/v1/devices/register
//! - POST /api/v1/devices/{id}/deactivate
//! - GET  /api/v1/devices?account_id=...
//! - POST /api/v1/devices/verify

use attune_core::device_binding::{DeviceFingerprint, DeviceLicense, DeviceSummary};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default)]
pub struct AccountsState {
    /// account_id → 该账号 active devices (内存版, 实际 SaaS 走 PostgreSQL).
    devices: Arc<Mutex<HashMap<String, Vec<StoredDevice>>>>,
}

#[derive(Debug, Clone)]
struct StoredDevice {
    device_id: String,
    fingerprint: DeviceFingerprint,
    issued_at: chrono::DateTime<chrono::Utc>,
    expires_at: chrono::DateTime<chrono::Utc>,
    token: String,
}

const MAX_DEVICES_PER_ACCOUNT: usize = 2;
const LICENSE_TTL_DAYS: i64 = 30;

pub fn router(state: AccountsState) -> Router {
    Router::new()
        .route("/api/v1/devices/register", post(register_device))
        .route("/api/v1/devices/{id}/deactivate", post(deactivate_device))
        .route("/api/v1/devices", get(list_devices))
        .route("/api/v1/devices/verify", post(verify_license))
        .with_state(state)
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegisterReq {
    pub account_id: String,
    pub fingerprint: DeviceFingerprint,
}

async fn register_device(
    State(state): State<AccountsState>,
    Json(req): Json<RegisterReq>,
) -> impl IntoResponse {
    let mut devices = state.devices.lock().unwrap_or_else(|e| e.into_inner());
    let entry = devices.entry(req.account_id.clone()).or_insert_with(Vec::new);

    // 已绑定 (按 device_id) → 续期返回新 license
    if let Some(existing) = entry
        .iter_mut()
        .find(|d| d.device_id == req.fingerprint.device_id)
    {
        let now = chrono::Utc::now();
        existing.issued_at = now;
        existing.expires_at = now + chrono::Duration::days(LICENSE_TTL_DAYS);
        existing.token = uuid::Uuid::new_v4().to_string();
        let lic = DeviceLicense {
            device_id: existing.device_id.clone(),
            account_id: req.account_id.clone(),
            token: existing.token.clone(),
            issued_at: existing.issued_at.to_rfc3339(),
            expires_at: existing.expires_at.to_rfc3339(),
        };
        return (StatusCode::OK, Json(serde_json::to_value(lic).unwrap()));
    }

    // 新设备 + 已满 → 409 + 候选清单
    if entry.len() >= MAX_DEVICES_PER_ACCOUNT {
        let summaries: Vec<DeviceSummary> = entry
            .iter()
            .map(|d| DeviceSummary {
                device_id: d.device_id.clone(),
                hostname: d.fingerprint.hostname.clone(),
                last_seen_at: d.issued_at.to_rfc3339(),
                form_factor: d.fingerprint.form_factor.as_str().to_string(),
            })
            .collect();
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "max_devices_reached",
                "existing": summaries,
            })),
        );
    }

    // 新设备 + 未满 → 接受
    let now = chrono::Utc::now();
    let lic = DeviceLicense {
        device_id: req.fingerprint.device_id.clone(),
        account_id: req.account_id.clone(),
        token: uuid::Uuid::new_v4().to_string(),
        issued_at: now.to_rfc3339(),
        expires_at: (now + chrono::Duration::days(LICENSE_TTL_DAYS)).to_rfc3339(),
    };
    entry.push(StoredDevice {
        device_id: lic.device_id.clone(),
        fingerprint: req.fingerprint,
        issued_at: now,
        expires_at: now + chrono::Duration::days(LICENSE_TTL_DAYS),
        token: lic.token.clone(),
    });
    (StatusCode::OK, Json(serde_json::to_value(lic).unwrap()))
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeactivateReq {
    pub account_id: String,
    #[serde(default)]
    pub confirm: bool,
}

async fn deactivate_device(
    State(state): State<AccountsState>,
    Path(device_id): Path<String>,
    Json(req): Json<DeactivateReq>,
) -> impl IntoResponse {
    if !req.confirm {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "confirm=true required"})),
        );
    }
    let mut devices = state.devices.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = devices.get_mut(&req.account_id) {
        let before = entry.len();
        entry.retain(|d| d.device_id != device_id);
        let removed = before - entry.len();
        return (
            StatusCode::OK,
            Json(serde_json::json!({"removed": removed})),
        );
    }
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "account not found"})),
    )
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub account_id: String,
}

async fn list_devices(
    State(state): State<AccountsState>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let devices = state.devices.lock().unwrap_or_else(|e| e.into_inner());
    let summaries: Vec<DeviceSummary> = devices
        .get(&q.account_id)
        .map(|v| {
            v.iter()
                .map(|d| DeviceSummary {
                    device_id: d.device_id.clone(),
                    hostname: d.fingerprint.hostname.clone(),
                    last_seen_at: d.issued_at.to_rfc3339(),
                    form_factor: d.fingerprint.form_factor.as_str().to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    (
        StatusCode::OK,
        Json(serde_json::json!({"devices": summaries})),
    )
}

async fn verify_license(
    State(state): State<AccountsState>,
    Json(lic): Json<DeviceLicense>,
) -> impl IntoResponse {
    let devices = state.devices.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = devices.get(&lic.account_id) {
        if entry
            .iter()
            .any(|d| d.device_id == lic.device_id && d.token == lic.token)
        {
            return (StatusCode::OK, Json(serde_json::json!({"valid": true})));
        }
    }
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"valid": false})),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_core::device_binding::FormFactor;

    fn fp(device_id: &str) -> DeviceFingerprint {
        DeviceFingerprint {
            device_id: device_id.into(),
            hostname: format!("host-{device_id}"),
            os: "linux".into(),
            cpu_brand: "x86_64".into(),
            hardware_uuid: None,
            form_factor: FormFactor::Laptop,
        }
    }

    #[tokio::test]
    async fn first_device_registers_ok() {
        let state = AccountsState::default();
        let resp = register_device(
            State(state.clone()),
            Json(RegisterReq {
                account_id: "acc-1".into(),
                fingerprint: fp("d-1"),
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn third_device_returns_409_with_existing() {
        let state = AccountsState::default();
        for i in 1..=2 {
            let _ = register_device(
                State(state.clone()),
                Json(RegisterReq {
                    account_id: "acc-1".into(),
                    fingerprint: fp(&format!("d-{i}")),
                }),
            )
            .await
            .into_response();
        }
        let resp = register_device(
            State(state),
            Json(RegisterReq {
                account_id: "acc-1".into(),
                fingerprint: fp("d-3"),
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn re_register_same_device_renews() {
        let state = AccountsState::default();
        let _ = register_device(
            State(state.clone()),
            Json(RegisterReq {
                account_id: "acc-1".into(),
                fingerprint: fp("d-1"),
            }),
        )
        .await
        .into_response();
        // 同 device_id 再次注册应 200 (续期), 不算占第二个 slot
        let resp = register_device(
            State(state.clone()),
            Json(RegisterReq {
                account_id: "acc-1".into(),
                fingerprint: fp("d-1"),
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let devs = state.devices.lock().unwrap();
        assert_eq!(devs.get("acc-1").unwrap().len(), 1);
    }

    #[tokio::test]
    async fn deactivate_then_register_new_device_works() {
        let state = AccountsState::default();
        for i in 1..=2 {
            let _ = register_device(
                State(state.clone()),
                Json(RegisterReq {
                    account_id: "acc-1".into(),
                    fingerprint: fp(&format!("d-{i}")),
                }),
            )
            .await
            .into_response();
        }
        // 踢下线 d-1
        let _ = deactivate_device(
            State(state.clone()),
            Path("d-1".into()),
            Json(DeactivateReq {
                account_id: "acc-1".into(),
                confirm: true,
            }),
        )
        .await
        .into_response();
        // 现在 d-3 可以加进
        let resp = register_device(
            State(state),
            Json(RegisterReq {
                account_id: "acc-1".into(),
                fingerprint: fp("d-3"),
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn deactivate_without_confirm_400() {
        let state = AccountsState::default();
        let resp = deactivate_device(
            State(state),
            Path("d-x".into()),
            Json(DeactivateReq {
                account_id: "acc-1".into(),
                confirm: false,
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
