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

// Device + license schemas — moved from attune-core (2026-05-20).
// They lived there as dead code on the live cloud-Bearer-token path; the only
// real consumer is this OSS reference SaaS, so we keep them next to it.
pub mod device_binding;
pub mod license_protocol;

use crate::device_binding::{DeviceFingerprint, DeviceLicense, DeviceSummary};
use crate::license_protocol::{sign_license, LicenseClaims, LlmEndpointInfo, SignedLicense};
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
    /// license_id → 已激活记录 (防重放)
    activated_licenses: Arc<Mutex<HashMap<String, ActivatedLicense>>>,
    /// 服务器 license 签名私钥 (32 字节). 仅 admin endpoint 使用.
    /// 生产环境从 env / KMS 注入; 默认 None 拒绝 generate.
    license_signing_key: Arc<Mutex<Option<[u8; 32]>>>,
    /// 云端 LLM 配置 (admin 写, user 读)
    llm_config: Arc<Mutex<Option<LlmGatewayConfig>>>,
}

impl AccountsState {
    /// 注入 license signing key (生产从 env ATTUNE_LICENSE_SIGN_KEY 读取)
    pub fn set_signing_key(&self, sk: [u8; 32]) {
        *self.license_signing_key.lock().unwrap_or_else(|e| e.into_inner()) = Some(sk);
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // license/activated_at 待 GET /licenses/{id}/status endpoint 实装时使用
struct ActivatedLicense {
    license: SignedLicense,
    activated_at: chrono::DateTime<chrono::Utc>,
    used_tokens_this_month: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmGatewayConfig {
    /// 后端真实 OpenAI 兼容 endpoint (用户不可见)
    pub upstream_endpoint: String,
    /// 后端 API key (用户不可见)
    pub upstream_api_key: String,
    /// 客户端可见的 gateway endpoint URL (代理回 upstream)
    pub gateway_endpoint: String,
    /// 默认模型推荐 (按 tier 不同, 这里简化单值)
    pub default_model: String,
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
        // License 管理 (admin) — 离线激活码 + 集体授权
        .route("/api/v1/admin/licenses/generate", post(admin_generate_license))
        .route("/api/v1/licenses/activate", post(activate_license))
        // 云端 LLM gateway
        .route("/api/v1/admin/llm/configure", post(admin_configure_llm))
        .route("/api/v1/llm/endpoint", post(get_llm_endpoint))
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

// ── License 管理 ──────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct GenerateLicenseReq {
    pub account_id: String,
    pub tier: String,
    #[serde(default = "default_max_devices")]
    pub max_devices: usize,
    #[serde(default)]
    pub llm_monthly_quota: u64,
    /// 有效期天数 (0 = 永久授权)
    #[serde(default = "default_validity_days")]
    pub validity_days: i64,
    #[serde(default)]
    pub note: String,
}

fn default_max_devices() -> usize { 2 }
fn default_validity_days() -> i64 { 365 }

async fn admin_generate_license(
    State(state): State<AccountsState>,
    Json(req): Json<GenerateLicenseReq>,
) -> impl IntoResponse {
    let key_guard = state.license_signing_key.lock().unwrap_or_else(|e| e.into_inner());
    let sk = match *key_guard {
        Some(k) => k,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "license_signing_key_not_configured",
                    "hint": "set via AccountsState::set_signing_key()"
                })),
            );
        }
    };
    drop(key_guard);

    let now = chrono::Utc::now().timestamp();
    let expires_at = if req.validity_days <= 0 {
        0
    } else {
        now + req.validity_days * 86_400
    };
    let claims = LicenseClaims {
        license_id: uuid::Uuid::new_v4().to_string(),
        account_id: req.account_id,
        tier: req.tier,
        max_devices: req.max_devices,
        llm_monthly_quota: req.llm_monthly_quota,
        issued_at: now,
        expires_at,
        note: req.note,
    };
    let signed = sign_license(claims, &sk);
    let code = match signed.to_code() {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("encode: {e}")})),
            );
        }
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "license_id": signed.claims.license_id,
            "license_code": code,
        })),
    )
}

#[derive(Debug, Deserialize)]
pub struct ActivateReq {
    pub license_code: String,
}

async fn activate_license(
    State(state): State<AccountsState>,
    Json(req): Json<ActivateReq>,
) -> impl IntoResponse {
    let signed = match SignedLicense::from_code(&req.license_code) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("invalid license code: {e}")})),
            );
        }
    };
    let now = chrono::Utc::now().timestamp();
    if signed.claims.is_expired(now) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "license expired"})),
        );
    }
    let mut activated = state.activated_licenses.lock().unwrap_or_else(|e| e.into_inner());
    if activated.contains_key(&signed.claims.license_id) {
        // 已激活, 幂等返回原信息
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "already_activated",
                "license_id": signed.claims.license_id,
            })),
        );
    }
    activated.insert(
        signed.claims.license_id.clone(),
        ActivatedLicense {
            license: signed.clone(),
            activated_at: chrono::Utc::now(),
            used_tokens_this_month: 0,
        },
    );
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "activated",
            "license_id": signed.claims.license_id,
            "account_id": signed.claims.account_id,
            "tier": signed.claims.tier,
            "max_devices": signed.claims.max_devices,
            "llm_monthly_quota": signed.claims.llm_monthly_quota,
        })),
    )
}

// ── 云端 LLM gateway ─────────────────────────────

async fn admin_configure_llm(
    State(state): State<AccountsState>,
    Json(cfg): Json<LlmGatewayConfig>,
) -> impl IntoResponse {
    *state.llm_config.lock().unwrap_or_else(|e| e.into_inner()) = Some(cfg);
    (StatusCode::OK, Json(serde_json::json!({"status": "configured"})))
}

#[derive(Debug, Deserialize)]
pub struct LlmEndpointReq {
    pub license_code: String,
}

async fn get_llm_endpoint(
    State(state): State<AccountsState>,
    Json(req): Json<LlmEndpointReq>,
) -> impl IntoResponse {
    // 1. 解 license code
    let signed = match SignedLicense::from_code(&req.license_code) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("invalid license: {e}")})),
            );
        }
    };
    // 2. 检查激活状态 + 过期
    let activated = state.activated_licenses.lock().unwrap_or_else(|e| e.into_inner());
    let act = match activated.get(&signed.claims.license_id) {
        Some(a) => a.clone(),
        None => {
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": "license not activated; call /licenses/activate first"})),
            );
        }
    };
    drop(activated);
    let now = chrono::Utc::now().timestamp();
    if signed.claims.is_expired(now) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "license expired"})));
    }
    if signed.claims.llm_monthly_quota == 0 {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "this license has no LLM quota"})),
        );
    }
    // 3. 检查 quota
    let remaining = signed
        .claims
        .llm_monthly_quota
        .saturating_sub(act.used_tokens_this_month);
    if remaining == 0 {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "monthly_quota_exhausted",
                "quota": signed.claims.llm_monthly_quota,
            })),
        );
    }
    // 4. 拿 LLM config
    let cfg_guard = state.llm_config.lock().unwrap_or_else(|e| e.into_inner());
    let cfg = match cfg_guard.as_ref() {
        Some(c) => c.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "llm gateway not configured by admin"})),
            );
        }
    };
    drop(cfg_guard);

    // 5. 返客户端 endpoint info — 注意 upstream_api_key 不暴露
    let info = LlmEndpointInfo {
        endpoint: cfg.gateway_endpoint,
        gateway_token: format!("attune-gw-{}", signed.claims.license_id),
        default_model: cfg.default_model,
        remaining_quota: remaining,
        // 下个月 1 号重置 (简化, 实际按 tz / billing cycle)
        quota_reset_at: next_month_start(now),
    };
    (StatusCode::OK, Json(serde_json::to_value(info).unwrap()))
}

fn next_month_start(now_unix: i64) -> i64 {
    use chrono::{Datelike, NaiveDate, TimeZone, Utc};
    let now = Utc.timestamp_opt(now_unix, 0).single().unwrap_or_else(Utc::now);
    let (y, m) = if now.month() == 12 {
        (now.year() + 1, 1)
    } else {
        (now.year(), now.month() + 1)
    };
    let next = NaiveDate::from_ymd_opt(y, m, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|d| Utc.from_utc_datetime(&d))
        .unwrap_or_else(Utc::now);
    next.timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_binding::FormFactor;

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

    // ── License + LLM gateway 测试 ──────────────────

    fn signing_key() -> [u8; 32] {
        let mut k = [0u8; 32];
        for i in 0..32 { k[i] = i as u8 + 1; }
        k
    }

    fn pubkey_hex() -> String {
        attune_core::plugin_sig::derive_verifying_key_hex(&signing_key())
    }

    #[tokio::test]
    async fn license_generate_then_activate_roundtrip() {
        let state = AccountsState::default();
        state.set_signing_key(signing_key());

        // 1. admin 生成 license
        let resp = admin_generate_license(
            State(state.clone()),
            Json(GenerateLicenseReq {
                account_id: "acc-1".into(),
                tier: "paid".into(),
                max_devices: 2,
                llm_monthly_quota: 1_000_000,
                validity_days: 365,
                note: "test".into(),
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
        let val: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let code = val.get("license_code").and_then(|v| v.as_str()).unwrap().to_string();

        // 客户端独立校验 (无云端联系即可)
        let signed = SignedLicense::from_code(&code).unwrap();
        signed.verify(&pubkey_hex(), chrono::Utc::now().timestamp()).unwrap();

        // 2. 客户端激活
        let resp = activate_license(
            State(state.clone()),
            Json(ActivateReq { license_code: code.clone() }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);

        // 3. 再次激活幂等
        let resp = activate_license(
            State(state),
            Json(ActivateReq { license_code: code }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn license_generate_without_key_503() {
        let state = AccountsState::default();
        // 未注入 signing key
        let resp = admin_generate_license(
            State(state),
            Json(GenerateLicenseReq {
                account_id: "x".into(),
                tier: "paid".into(),
                max_devices: 2,
                llm_monthly_quota: 0,
                validity_days: 0,
                note: "".into(),
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn llm_endpoint_requires_activation() {
        let state = AccountsState::default();
        state.set_signing_key(signing_key());
        // 配 LLM
        *state.llm_config.lock().unwrap() = Some(LlmGatewayConfig {
            upstream_endpoint: "https://api.openai.com/v1".into(),
            upstream_api_key: "sk-secret".into(),
            gateway_endpoint: "https://gateway.attune.ai/v1".into(),
            default_model: "gpt-4o-mini".into(),
        });

        // 生成 license 但不激活
        let signed = sign_license(
            LicenseClaims {
                license_id: "l1".into(),
                account_id: "a1".into(),
                tier: "paid".into(),
                max_devices: 2,
                llm_monthly_quota: 100_000,
                issued_at: chrono::Utc::now().timestamp(),
                expires_at: chrono::Utc::now().timestamp() + 86_400,
                note: "".into(),
            },
            &signing_key(),
        );
        let code = signed.to_code().unwrap();

        let resp = get_llm_endpoint(
            State(state),
            Json(LlmEndpointReq { license_code: code }),
        )
        .await
        .into_response();
        // 未激活 → 403
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn llm_endpoint_after_activation_returns_info() {
        let state = AccountsState::default();
        state.set_signing_key(signing_key());
        *state.llm_config.lock().unwrap() = Some(LlmGatewayConfig {
            upstream_endpoint: "https://api.openai.com/v1".into(),
            upstream_api_key: "sk-secret".into(),
            gateway_endpoint: "https://gateway.attune.ai/v1".into(),
            default_model: "gpt-4o-mini".into(),
        });

        let signed = sign_license(
            LicenseClaims {
                license_id: "l2".into(),
                account_id: "a2".into(),
                tier: "paid".into(),
                max_devices: 2,
                llm_monthly_quota: 100_000,
                issued_at: chrono::Utc::now().timestamp(),
                expires_at: chrono::Utc::now().timestamp() + 86_400,
                note: "".into(),
            },
            &signing_key(),
        );
        let code = signed.to_code().unwrap();

        // 激活
        let _ = activate_license(
            State(state.clone()),
            Json(ActivateReq { license_code: code.clone() }),
        )
        .await
        .into_response();

        // 拿 endpoint
        let resp = get_llm_endpoint(
            State(state),
            Json(LlmEndpointReq { license_code: code }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
        let val: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let endpoint = val.get("endpoint").and_then(|v| v.as_str()).unwrap();
        assert_eq!(endpoint, "https://gateway.attune.ai/v1");
        // upstream 密钥不暴露
        let s = String::from_utf8(body.to_vec()).unwrap();
        assert!(!s.contains("sk-secret"));
        // gateway_token 含 license_id 但不是 raw OpenAI key
        assert!(s.contains("attune-gw-l2"));
    }

    #[tokio::test]
    async fn license_with_zero_quota_no_llm_endpoint() {
        let state = AccountsState::default();
        state.set_signing_key(signing_key());
        *state.llm_config.lock().unwrap() = Some(LlmGatewayConfig {
            upstream_endpoint: "x".into(),
            upstream_api_key: "y".into(),
            gateway_endpoint: "z".into(),
            default_model: "m".into(),
        });

        let signed = sign_license(
            LicenseClaims {
                license_id: "l3".into(),
                account_id: "a3".into(),
                tier: "free".into(),
                max_devices: 1,
                llm_monthly_quota: 0,  // 没 LLM 权益
                issued_at: chrono::Utc::now().timestamp(),
                expires_at: 0,
                note: "".into(),
            },
            &signing_key(),
        );
        let code = signed.to_code().unwrap();
        let _ = activate_license(
            State(state.clone()),
            Json(ActivateReq { license_code: code.clone() }),
        )
        .await
        .into_response();
        let resp = get_llm_endpoint(
            State(state),
            Json(LlmEndpointReq { license_code: code }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
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
