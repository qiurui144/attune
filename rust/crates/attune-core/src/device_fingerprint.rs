//! Cloud device fingerprint — 授权码激活时绑定本机到 license 的稳定指纹.
//!
//! 与 cloud `accounts/api/devices.py` 的 `POST /api/v1/devices/activate` 契约对齐:
//! 请求体的 `device_fingerprint` = `{device_id, hostname, os, cpu_brand,
//! hardware_uuid, form_factor, fingerprint_sig}`,其中
//!
//! ```text
//! fingerprint_sig = SHA-256(device_id|hostname|os|cpu_brand|hardware_uuid) hex
//! ```
//!
//! **字段顺序与 cloud `device_service._compute_fingerprint_sig` 逐字节一致**
//! (`device_id|hostname|os|cpu_brand|hardware_uuid`),且 `hardware_uuid` 缺失时
//! 写入字面量 `"null"`(cloud 注释明确:`str(None)` → `"null"`)。两端任何一处
//! 字段顺序 / 空值表示分歧都会导致 server recompute 不匹配 → 403。
//!
//! 这与 `attune-accounts::device_binding`(已 quarantine 的 OSS self-host reference,
//! 签名顺序 `device_id|hostname|hardware_uuid|os`、32-byte raw)是**不同契约**,不复用。
//! 生产路径走本模块 + `cloud_client::device_activate`(HTTPS Bearer/session)。
//!
//! `device_id` 稳定性:优先平台级 machine-id(Linux `/etc/machine-id`、Windows
//! `MachineGuid`);取不到 → 生成一个 UUID v4 持久化到 `config_dir/device.id`,跨
//! 重启稳定(同机两次调用返回同一 `device_id`)。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 客户端采集的设备指纹,序列化进 `device_activate` 请求体的 `device_fingerprint`。
///
/// 字段名 / 形态镜像 cloud `DeviceFingerprint` pydantic 模型。`fingerprint_sig`
/// 由 [`Self::fingerprint_sig`] 计算填入(server 缺失时会 recompute,但客户端主动
/// 带上以便 server 校验一致性)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceFingerprint {
    pub device_id: String,
    pub hostname: String,
    pub os: String,
    pub cpu_brand: String,
    /// 平台硬件 UUID(machine-id / MachineGuid);取不到 → `None`(签名计入 `"null"`)。
    #[serde(default)]
    pub hardware_uuid: Option<String>,
    /// laptop | local_scheduler_appliance | server | unknown
    pub form_factor: String,
    /// SHA-256(device_id|hostname|os|cpu_brand|hardware_uuid) hex.
    pub fingerprint_sig: String,
}

impl DeviceFingerprint {
    /// 计算 `fingerprint_sig`:`SHA-256(device_id|hostname|os|cpu_brand|hardware_uuid)`
    /// 的 hex。**字段顺序 + 空值表示与 cloud `_compute_fingerprint_sig` 逐字节一致**
    /// (`hardware_uuid` 为 `None` 时计入字面 `"null"`)。pure 函数,便于跨端对拍。
    pub fn compute_sig(
        device_id: &str,
        hostname: &str,
        os: &str,
        cpu_brand: &str,
        hardware_uuid: Option<&str>,
    ) -> String {
        let hw = hardware_uuid.unwrap_or("null");
        let raw = format!("{device_id}|{hostname}|{os}|{cpu_brand}|{hw}");
        let digest = Sha256::digest(raw.as_bytes());
        hex_lower(&digest)
    }

    /// 重新计算本指纹的 `fingerprint_sig`(用其当前字段)。
    pub fn fingerprint_sig(&self) -> String {
        Self::compute_sig(
            &self.device_id,
            &self.hostname,
            &self.os,
            &self.cpu_brand,
            self.hardware_uuid.as_deref(),
        )
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// 采集本机的设备指纹(稳定 + 跨平台)。
///
/// - `device_id`:稳定机器标识(machine-id / 持久化 UUID,见 [`stable_device_id`])。
/// - `hostname` / `os` / `cpu_brand`:复用 `platform` 硬件画像;缺失退回保守默认。
/// - `hardware_uuid`:平台硬件 UUID(同 machine-id 源);取不到 → `None`。
/// - `form_factor`:`platform::FormFactor` 映射(laptop/local_scheduler_appliance/server/unknown)。
///
/// 同机两次调用返回 **相同** 的 `device_id` 与 `fingerprint_sig`(指纹稳定性,
/// 是设备数配额 + 幂等激活的前提)。
pub fn device_fingerprint() -> DeviceFingerprint {
    let profile = crate::platform::HardwareProfile::detect();
    let hardware_uuid = machine_uuid();
    let device_id = stable_device_id(hardware_uuid.as_deref());
    let hostname = hostname_or_default();
    let os = profile.os.to_string();
    let cpu_brand = if profile.cpu_model.is_empty() {
        std::env::consts::ARCH.to_string()
    } else {
        profile.cpu_model.clone()
    };
    let form_factor = form_factor_str(profile.form_factor);
    let fingerprint_sig = DeviceFingerprint::compute_sig(
        &device_id,
        &hostname,
        &os,
        &cpu_brand,
        hardware_uuid.as_deref(),
    );
    DeviceFingerprint {
        device_id,
        hostname,
        os,
        cpu_brand,
        hardware_uuid,
        form_factor,
        fingerprint_sig,
    }
}

/// `platform::FormFactor` → cloud 形态字符串。
fn form_factor_str(ff: crate::platform::FormFactor) -> String {
    use crate::platform::FormFactor as F;
    match ff {
        F::Laptop => "laptop",
        F::LocalSchedulerAppliance => "local_scheduler_appliance",
        F::Server => "server",
        F::Unknown => "unknown",
    }
    .to_string()
}

/// 稳定的 `device_id`:
/// 1. 若已存在持久化的 `config_dir/device.id` → 读它(跨重启稳定的真相源)。
/// 2. 否则:有 machine-id → 用 machine-id;无 → 生成 UUID v4。
///    把结果写入 `config_dir/device.id` 持久化,后续调用复用(同机稳定)。
///
/// 写盘失败(只读 / 无 config dir)不致命:同进程内仍稳定,跨重启可能重算
/// (machine-id 通常仍稳定;无 machine-id 的极端环境才会漂移)。
pub fn stable_device_id(machine_uuid: Option<&str>) -> String {
    let path = crate::platform::config_dir().join("device.id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let id = match machine_uuid {
        Some(m) if !m.trim().is_empty() => m.trim().to_string(),
        _ => uuid::Uuid::new_v4().to_string(),
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, &id);
    id
}

/// 平台硬件 UUID:Linux `/etc/machine-id`(或 `/var/lib/dbus/machine-id`)、
/// Windows registry `MachineGuid`、macOS `IOPlatformUUID`。取不到 → `None`。
fn machine_uuid() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        for p in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
            if let Ok(s) = std::fs::read_to_string(p) {
                let t = s.trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
        None
    }
    #[cfg(target_os = "windows")]
    {
        // reg query 读 MachineGuid;失败保持 None(签名计入 "null")。
        let out = crate::process::command_no_window("reg")
            .args([
                "query",
                r"HKLM\SOFTWARE\Microsoft\Cryptography",
                "/v",
                "MachineGuid",
            ])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if let Some(idx) = line.find("REG_SZ") {
                let val = line[idx + "REG_SZ".len()..].trim();
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        let out = crate::process::command_no_window("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if line.contains("IOPlatformUUID") {
                if let Some(start) = line.find('=') {
                    let v = line[start + 1..].trim().trim_matches('"').to_string();
                    if !v.is_empty() {
                        return Some(v);
                    }
                }
            }
        }
        None
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

/// 主机名:env `HOSTNAME`(Unix)/ `COMPUTERNAME`(Windows),都缺 → Linux 读
/// `/etc/hostname`,仍无 → `"unknown"`(签名稳定,不 panic)。
fn hostname_or_default() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let t = h.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Ok(h) = std::env::var("COMPUTERNAME") {
        let t = h.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
            let t = h.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 字段顺序对齐 cloud `_compute_fingerprint_sig`:
    /// `SHA-256("did|host|linux|CPU|hw")` 的 hex,且 `hardware_uuid=None` → 字面 `"null"`。
    /// 这是 cross-repo 唯一可能 silent 字节分歧的点 —— 用已知向量钉死。
    #[test]
    fn sig_matches_cloud_field_order_and_null() {
        // 与 cloud `"|".join([device_id, hostname, os, cpu_brand, "null"])` 对拍。
        let device_id = "dev-abc";
        let hostname = "host1";
        let os = "linux";
        let cpu = "AMD Ryzen 7";
        let raw_none = format!("{device_id}|{hostname}|{os}|{cpu}|null");
        let expected_none = hex_lower(&Sha256::digest(raw_none.as_bytes()));
        assert_eq!(
            DeviceFingerprint::compute_sig(device_id, hostname, os, cpu, None),
            expected_none,
            "hardware_uuid=None must hash the literal 'null' (cloud parity)"
        );

        // 有 hardware_uuid 时计入其值(不再是 "null")。
        let raw_some = format!("{device_id}|{hostname}|{os}|{cpu}|hw-uuid-9");
        let expected_some = hex_lower(&Sha256::digest(raw_some.as_bytes()));
        assert_eq!(
            DeviceFingerprint::compute_sig(device_id, hostname, os, cpu, Some("hw-uuid-9")),
            expected_some
        );
        // None 与 Some 必不同。
        assert_ne!(expected_none, expected_some);
    }

    /// sig 是 64-hex 小写 SHA-256。
    #[test]
    fn sig_is_64_hex_lowercase() {
        let sig = DeviceFingerprint::compute_sig("a", "b", "linux", "c", None);
        assert_eq!(sig.len(), 64, "SHA-256 hex is 64 chars");
        assert!(sig
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    /// 字段顺序敏感:换 hostname / os 位置必产生不同 sig(防止两端顺序漂移未被发现)。
    #[test]
    fn sig_is_field_order_sensitive() {
        let a = DeviceFingerprint::compute_sig("id", "host", "os", "cpu", None);
        // 把 hostname 与 os 互换 → 不同输入 → 不同 sig。
        let b = DeviceFingerprint::compute_sig("id", "os", "host", "cpu", None);
        assert_ne!(a, b, "swapping field positions must change the signature");
    }

    /// `fingerprint_sig()` 复算与 `compute_sig` 一致(self-consistency)。
    #[test]
    fn instance_sig_matches_compute() {
        let fp = DeviceFingerprint {
            device_id: "d".into(),
            hostname: "h".into(),
            os: "linux".into(),
            cpu_brand: "cpu".into(),
            hardware_uuid: Some("hw".into()),
            form_factor: "laptop".into(),
            fingerprint_sig: String::new(),
        };
        assert_eq!(
            fp.fingerprint_sig(),
            DeviceFingerprint::compute_sig("d", "h", "linux", "cpu", Some("hw"))
        );
    }

    /// 指纹稳定性:同机两次 `device_fingerprint()` 返回**完全相同**的 device_id
    /// 与 fingerprint_sig(设备数配额 + 幂等激活的前提)。用 thread-local 临时 config
    /// dir 隔离,避免污染真实 `~/.config/attune`。
    #[test]
    fn fingerprint_is_stable_across_calls() {
        let td = tempfile::tempdir().expect("tempdir");
        let prev = crate::platform::set_dir_override_for_test(Some(td.path().to_path_buf()));
        let fp1 = device_fingerprint();
        let fp2 = device_fingerprint();
        crate::platform::set_dir_override_for_test(prev);

        assert_eq!(
            fp1.device_id, fp2.device_id,
            "device_id must be stable across calls"
        );
        assert_eq!(
            fp1.fingerprint_sig, fp2.fingerprint_sig,
            "sig must be stable"
        );
        assert_eq!(fp1, fp2, "whole fingerprint stable across calls");
        // sig 必须与字段自洽。
        assert_eq!(
            fp1.fingerprint_sig,
            fp1.fingerprint_sig(),
            "self-consistent sig"
        );
        assert!(!fp1.device_id.is_empty(), "device_id never empty");
    }

    /// `stable_device_id` 优先复用已持久化的 `device.id`(machine_uuid 即便变也忽略)。
    #[test]
    fn stable_device_id_reuses_persisted_file() {
        let td = tempfile::tempdir().expect("tempdir");
        let prev = crate::platform::set_dir_override_for_test(Some(td.path().to_path_buf()));

        // 首次:无文件,有 machine-id → 用 machine-id 并落盘。
        let id1 = stable_device_id(Some("machine-xyz"));
        assert_eq!(id1, "machine-xyz");
        // 第二次即使传不同 machine_uuid,也必须复用已落盘的值。
        let id2 = stable_device_id(Some("DIFFERENT-machine"));
        crate::platform::set_dir_override_for_test(prev);
        assert_eq!(
            id2, "machine-xyz",
            "persisted device.id must win over a changed machine-id"
        );
    }

    /// 无 machine-id 时生成 UUID v4 并持久化复用(跨调用稳定)。
    #[test]
    fn stable_device_id_generates_and_persists_uuid_when_no_machine_id() {
        let td = tempfile::tempdir().expect("tempdir");
        let prev = crate::platform::set_dir_override_for_test(Some(td.path().to_path_buf()));
        let id1 = stable_device_id(None);
        let id2 = stable_device_id(None);
        crate::platform::set_dir_override_for_test(prev);
        assert!(!id1.is_empty());
        assert_eq!(id1, id2, "generated UUID must persist + be reused");
        // UUID v4 形态(含连字符)。
        assert_eq!(id1.len(), 36, "uuid v4 string is 36 chars");
    }

    /// form_factor 字符串映射与 cloud 形态字符串一致。
    #[test]
    fn form_factor_strings() {
        use crate::platform::FormFactor as F;
        assert_eq!(form_factor_str(F::Laptop), "laptop");
        assert_eq!(
            form_factor_str(F::LocalSchedulerAppliance),
            "local_scheduler_appliance"
        );
        assert_eq!(form_factor_str(F::Server), "server");
        assert_eq!(form_factor_str(F::Unknown), "unknown");
    }
}
