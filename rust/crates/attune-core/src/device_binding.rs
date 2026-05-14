//! 设备绑定 — 1 账号最多 2 设备激活.
//!
//! 客户端职责:
//! - 启动时收集 DeviceFingerprint
//! - 调 accounts 服务 register / verify
//! - 离线缓存 license_token (30 天有效)
//!
//! 不在此模块: 云端 accounts 服务 (HTTP API + DB schema 在 attune-cloud 仓).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FormFactor {
    Laptop,
    Desktop,
    K3Appliance,
    Other,
}

impl FormFactor {
    pub fn as_str(&self) -> &'static str {
        match self {
            FormFactor::Laptop => "laptop",
            FormFactor::Desktop => "desktop",
            FormFactor::K3Appliance => "k3_appliance",
            FormFactor::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceFingerprint {
    /// 本地 UUID v4 (持久化到 vault, 跨重启稳定)
    pub device_id: String,
    pub hostname: String,
    pub os: String,
    pub cpu_brand: String,
    pub hardware_uuid: Option<String>,
    pub form_factor: FormFactor,
}

impl DeviceFingerprint {
    /// 从环境采集 (调用方需先生成/读取持久化的 device_id)
    pub fn collect(device_id: String) -> Self {
        Self {
            device_id,
            hostname: hostname_or_default(),
            os: std::env::consts::OS.to_string(),
            cpu_brand: cpu_brand_or_default(),
            hardware_uuid: None, // 由调用方按平台填入 (Win: WMIC / Linux: dmidecode)
            form_factor: FormFactor::Laptop, // 默认笔电, 调用方按需 override
        }
    }

    /// 32 字节 sha256 签名 (用于云端 accounts 校验)
    pub fn signature(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(self.device_id.as_bytes());
        hasher.update(b"|");
        hasher.update(self.hostname.as_bytes());
        hasher.update(b"|");
        if let Some(uuid) = &self.hardware_uuid {
            hasher.update(uuid.as_bytes());
        }
        hasher.update(b"|");
        hasher.update(self.os.as_bytes());
        let result = hasher.finalize();
        let mut out = [0u8; 32];
        out.copy_from_slice(&result);
        out
    }
}

fn hostname_or_default() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

fn cpu_brand_or_default() -> String {
    // 简化: 通过 std::env::consts::ARCH 标 + cpuid 在桌面端可用 raw_cpuid crate.
    // 此处不强依赖, 默认返 ARCH.
    std::env::consts::ARCH.to_string()
}

/// 客户端持久化的 license token + 30 天离线有效期
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceLicense {
    pub device_id: String,
    pub account_id: String,
    pub token: String,
    /// ISO 8601 issued_at
    pub issued_at: String,
    /// ISO 8601 expires_at (issued_at + 30 天)
    pub expires_at: String,
}

impl DeviceLicense {
    pub fn is_within_30_days(&self) -> bool {
        let Ok(exp) = chrono::DateTime::parse_from_rfc3339(&self.expires_at) else {
            return false;
        };
        chrono::Utc::now() < exp.with_timezone(&chrono::Utc)
    }
}

/// 注册返回的状态 (云端响应)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum RegisterResponse {
    #[serde(rename = "ok")]
    Ok { license: DeviceLicense },
    #[serde(rename = "max_devices_reached")]
    MaxDevicesReached { existing: Vec<DeviceSummary> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceSummary {
    pub device_id: String,
    pub hostname: String,
    pub last_seen_at: String,
    pub form_factor: String,
}

/// 客户端启动决策机器
pub enum BootDecision {
    /// 在线注册成功, 用此 license
    Online(DeviceLicense),
    /// 设备超限, 需要用户选择踢下线某台
    NeedDeactivation(Vec<DeviceSummary>),
    /// 离线但 cached license 仍有效
    OfflineCached(DeviceLicense),
    /// 离线且 cached 过期, 必须连一次网
    OfflineExpired,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_signature_stable_for_same_inputs() {
        let fp1 = DeviceFingerprint {
            device_id: "uuid-1".into(),
            hostname: "host".into(),
            os: "linux".into(),
            cpu_brand: "x86_64".into(),
            hardware_uuid: Some("hw-1".into()),
            form_factor: FormFactor::Laptop,
        };
        let fp2 = fp1.clone();
        assert_eq!(fp1.signature(), fp2.signature());
    }

    #[test]
    fn fingerprint_signature_differs_for_diff_uuid() {
        let mut fp1 = DeviceFingerprint::collect("uuid-1".into());
        fp1.hardware_uuid = Some("hw-1".into());
        let mut fp2 = DeviceFingerprint::collect("uuid-2".into());
        fp2.hardware_uuid = Some("hw-1".into());
        assert_ne!(fp1.signature(), fp2.signature());
    }

    #[test]
    fn license_validity_check() {
        let now = chrono::Utc::now();
        let lic = DeviceLicense {
            device_id: "d1".into(),
            account_id: "a1".into(),
            token: "tok".into(),
            issued_at: now.to_rfc3339(),
            expires_at: (now + chrono::Duration::days(30)).to_rfc3339(),
        };
        assert!(lic.is_within_30_days());

        let expired = DeviceLicense {
            expires_at: (now - chrono::Duration::days(1)).to_rfc3339(),
            ..lic
        };
        assert!(!expired.is_within_30_days());
    }

    #[test]
    fn register_response_serde_ok() {
        let resp = RegisterResponse::Ok {
            license: DeviceLicense {
                device_id: "d".into(),
                account_id: "a".into(),
                token: "t".into(),
                issued_at: chrono::Utc::now().to_rfc3339(),
                expires_at: (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339(),
            },
        };
        let json = serde_json::to_string(&resp).expect("ser");
        assert!(json.contains("\"status\":\"ok\""));
        let back: RegisterResponse = serde_json::from_str(&json).expect("de");
        assert!(matches!(back, RegisterResponse::Ok { .. }));
    }

    #[test]
    fn register_response_serde_max_devices() {
        let resp = RegisterResponse::MaxDevicesReached {
            existing: vec![DeviceSummary {
                device_id: "d1".into(),
                hostname: "host1".into(),
                last_seen_at: "2025-01-01T00:00:00Z".into(),
                form_factor: "laptop".into(),
            }],
        };
        let json = serde_json::to_string(&resp).expect("ser");
        assert!(json.contains("max_devices_reached"));
    }

    #[test]
    fn form_factor_str() {
        assert_eq!(FormFactor::Laptop.as_str(), "laptop");
        assert_eq!(FormFactor::K3Appliance.as_str(), "k3_appliance");
    }
}
