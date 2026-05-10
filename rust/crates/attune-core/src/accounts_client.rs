//! 设备绑定 — 与 attune-cloud accounts 服务通信的 HTTP 客户端.
//!
//! Endpoints (per attune-plugin-protocol §8):
//! - POST /api/v1/devices/register
//! - POST /api/v1/devices/{id}/deactivate
//! - GET  /api/v1/devices?account_id=...
//! - POST /api/v1/devices/verify (校验 cached license)

use crate::device_binding::{DeviceFingerprint, DeviceLicense, DeviceSummary, RegisterResponse};
use crate::error::{Result, VaultError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct AccountsClient {
    base_url: String,
    http: reqwest::blocking::Client,
}

impl AccountsClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("build http client"),
        }
    }

    /// 注册设备 (account_id + fingerprint), 成功返回 license token, 满 2 设备返候选清单
    pub fn register_device(
        &self,
        account_id: &str,
        fp: &DeviceFingerprint,
    ) -> Result<RegisterResponse> {
        let url = format!("{}/api/v1/devices/register", self.base_url);
        let body = RegisterRequest {
            account_id: account_id.to_string(),
            fingerprint: fp.clone(),
        };
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .map_err(http_err)?;
        let status = resp.status();
        let text = resp.text().map_err(http_err)?;
        match status.as_u16() {
            200 => {
                let lic: DeviceLicense = serde_json::from_str(&text).map_err(json_err)?;
                Ok(RegisterResponse::Ok { license: lic })
            }
            409 => {
                let conflict: ConflictBody = serde_json::from_str(&text).map_err(json_err)?;
                Ok(RegisterResponse::MaxDevicesReached {
                    existing: conflict.existing,
                })
            }
            _ => Err(VaultError::Io(std::io::Error::other(format!(
                "register failed: {status} body={text}"
            )))),
        }
    }

    /// 踢下线某台设备
    pub fn deactivate_device(&self, account_id: &str, device_id: &str) -> Result<()> {
        let url = format!("{}/api/v1/devices/{device_id}/deactivate", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(&serde_json::json!({"account_id": account_id, "confirm": true}))
            .send()
            .map_err(http_err)?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(VaultError::Io(std::io::Error::other(format!(
                "deactivate failed: {status} body={body}"
            ))));
        }
        Ok(())
    }

    /// 列出该账号所有 active 设备
    pub fn list_devices(&self, account_id: &str) -> Result<Vec<DeviceSummary>> {
        let url = format!("{}/api/v1/devices", self.base_url);
        let resp = self
            .http
            .get(&url)
            .query(&[("account_id", account_id)])
            .send()
            .map_err(http_err)?;
        let body: ListDevicesResp = resp.json().map_err(http_err)?;
        Ok(body.devices)
    }

    /// 校验 cached license token 是否仍有效 (在线刷新 last_seen_at)
    pub fn verify_license(&self, license: &DeviceLicense) -> Result<bool> {
        let url = format!("{}/api/v1/devices/verify", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(license)
            .send()
            .map_err(http_err)?;
        Ok(resp.status().is_success())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegisterRequest {
    account_id: String,
    fingerprint: DeviceFingerprint,
}

#[derive(Debug, Clone, Deserialize)]
struct ConflictBody {
    #[serde(default)]
    existing: Vec<DeviceSummary>,
}

#[derive(Debug, Clone, Deserialize)]
struct ListDevicesResp {
    #[serde(default)]
    devices: Vec<DeviceSummary>,
}

fn http_err(e: reqwest::Error) -> VaultError {
    VaultError::Io(std::io::Error::other(format!("http: {e}")))
}

fn json_err(e: serde_json::Error) -> VaultError {
    VaultError::Io(std::io::Error::other(format!("json: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_binding::FormFactor;

    #[test]
    fn register_request_serializes() {
        let fp = DeviceFingerprint {
            device_id: "uuid-1".into(),
            hostname: "host".into(),
            os: "linux".into(),
            cpu_brand: "x86_64".into(),
            hardware_uuid: Some("hw-uuid".into()),
            form_factor: FormFactor::Laptop,
        };
        let req = RegisterRequest {
            account_id: "acc-1".into(),
            fingerprint: fp,
        };
        let json = serde_json::to_string(&req).expect("ser");
        assert!(json.contains("\"account_id\":\"acc-1\""));
        assert!(json.contains("\"device_id\":\"uuid-1\""));
        assert!(json.contains("\"form_factor\":\"Laptop\""));
    }

    #[test]
    fn client_builds_with_url() {
        let c = AccountsClient::new("https://accounts.attune.ai");
        assert_eq!(c.base_url, "https://accounts.attune.ai");
    }

    #[test]
    fn register_against_unreachable_server_returns_io() {
        let c = AccountsClient::new("http://127.0.0.1:1");
        let fp = DeviceFingerprint::collect("uuid-x".into());
        let err = c.register_device("acc", &fp).unwrap_err();
        assert!(matches!(err, VaultError::Io(_)));
    }
}
