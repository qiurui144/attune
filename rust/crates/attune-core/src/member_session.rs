//! Member session — 客户端会员登录状态 + 设置锁定规则.
//!
//! 设计要点 (per 用户拍板):
//! - 会员登录后**大多数配置自动锁定** (云端下发, UI 灰显)
//! - 免费用户可手动配置更多
//! - 服务器端 cloud.sh 一键部署 cloud accounts; 客户端通过 cloud_client 对接

use serde::{Deserialize, Serialize};

/// 个人应用会员状态 — 只有 LoggedOut / Free / Paid 三档.
/// 不引入 admin / 团队管理概念 (个人应用 N 设备共享同一账号).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemberState {
    /// 未登录 — 全部配置可改 (本地 self-host)
    LoggedOut,
    /// 免费用户 — 自己配 API, 大部分可改
    Free { account_id: String },
    /// 付费会员 — pluginhub 配什么就是什么 (云端下发锁定)
    Paid {
        account_id: String,
        license_id: String,
        /// 月度 LLM token 配额 (云端分配)
        llm_quota_remaining: u64,
    },
}

impl MemberState {
    pub fn is_logged_in(&self) -> bool {
        !matches!(self, MemberState::LoggedOut)
    }
    pub fn is_paid(&self) -> bool {
        matches!(self, MemberState::Paid { .. })
    }
    pub fn account_id(&self) -> Option<&str> {
        match self {
            MemberState::LoggedOut => None,
            MemberState::Free { account_id } | MemberState::Paid { account_id, .. } => {
                Some(account_id)
            }
        }
    }
}

/// 配置项锁定状态 — 给 UI 决定是否灰显
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SettingLock {
    /// 用户可改
    Editable,
    /// 锁定, UI 灰显 (付费会员云端下发)
    Locked,
}

/// 客户端所有可锁的 setting 字段 + 当前 lock 状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsLocks {
    pub llm_endpoint: SettingLock,
    pub llm_model: SettingLock,
    pub llm_api_key: SettingLock,
    pub embedding_model: SettingLock,
    pub ocr_engine: SettingLock,
    pub data_dir: SettingLock,
    pub local_folder_links: SettingLock,
    pub plugin_install: SettingLock,
    pub plugin_uninstall: SettingLock,
    pub vault_password: SettingLock,
    pub device_binding: SettingLock,
    pub backup_destination: SettingLock,
}

impl SettingsLocks {
    /// 按 MemberState 推导锁定规则.
    /// 个人应用: 只有 LoggedOut/Free 全可改, Paid 锁 LLM+插件 (云端下发).
    pub fn for_state(state: &MemberState) -> Self {
        let all_editable = Self {
            llm_endpoint: SettingLock::Editable,
            llm_model: SettingLock::Editable,
            llm_api_key: SettingLock::Editable,
            embedding_model: SettingLock::Editable,
            ocr_engine: SettingLock::Editable,
            data_dir: SettingLock::Editable,
            local_folder_links: SettingLock::Editable,
            plugin_install: SettingLock::Editable,
            plugin_uninstall: SettingLock::Editable,
            vault_password: SettingLock::Editable,
            device_binding: SettingLock::Editable,
            backup_destination: SettingLock::Editable,
        };
        match state {
            MemberState::LoggedOut => all_editable,
            MemberState::Free { .. } => Self {
                device_binding: SettingLock::Locked, // 云端绑定, 防共享
                ..all_editable
            },
            // 付费会员: pluginhub 配什么就是什么 — LLM/插件锁, 用户隐私保留
            MemberState::Paid { .. } => Self {
                llm_endpoint: SettingLock::Locked,
                llm_model: SettingLock::Locked,
                llm_api_key: SettingLock::Locked,
                embedding_model: SettingLock::Locked,
                ocr_engine: SettingLock::Editable, // 本地装载, 可换
                data_dir: SettingLock::Locked,
                local_folder_links: SettingLock::Editable, // 隐私自管
                plugin_install: SettingLock::Locked,
                plugin_uninstall: SettingLock::Locked,
                vault_password: SettingLock::Editable,
                device_binding: SettingLock::Locked,
                backup_destination: SettingLock::Editable,
            },
        }
    }

    /// 检查具体字段是否可改 — 给 PATCH /settings handler 用
    pub fn can_edit(&self, field: &str) -> bool {
        let lock = match field {
            "llm_endpoint" => self.llm_endpoint,
            "llm_model" => self.llm_model,
            "llm_api_key" => self.llm_api_key,
            "embedding_model" => self.embedding_model,
            "ocr_engine" => self.ocr_engine,
            "data_dir" => self.data_dir,
            "local_folder_links" => self.local_folder_links,
            "plugin_install" => self.plugin_install,
            "plugin_uninstall" => self.plugin_uninstall,
            "vault_password" => self.vault_password,
            "device_binding" => self.device_binding,
            "backup_destination" => self.backup_destination,
            _ => return true, // 未知字段默认放行 (向后兼容)
        };
        match lock {
            SettingLock::Editable => true,
            SettingLock::Locked => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logged_out_can_edit_everything() {
        let locks = SettingsLocks::for_state(&MemberState::LoggedOut);
        assert!(locks.can_edit("llm_endpoint"));
        assert!(locks.can_edit("plugin_install"));
        assert!(locks.can_edit("data_dir"));
    }

    #[test]
    fn free_user_blocked_from_device_binding() {
        let locks = SettingsLocks::for_state(&MemberState::Free {
            account_id: "u1".into(),
        });
        assert!(!locks.can_edit("device_binding"));
        // 但 LLM / 模型 / 插件 仍可改 (自己 API 自己选)
        assert!(locks.can_edit("llm_endpoint"));
        assert!(locks.can_edit("plugin_install"));
    }

    #[test]
    fn paid_locked_on_llm_and_plugin() {
        let locks = SettingsLocks::for_state(&MemberState::Paid {
            account_id: "u1".into(),
            license_id: "l1".into(),
            llm_quota_remaining: 1_000_000,
        });
        // LLM 全锁 (pluginhub 配什么就是什么)
        assert!(!locks.can_edit("llm_endpoint"));
        assert!(!locks.can_edit("llm_model"));
        assert!(!locks.can_edit("llm_api_key"));
        // 插件锁 (云端按 license 自动装)
        assert!(!locks.can_edit("plugin_install"));
        assert!(!locks.can_edit("plugin_uninstall"));
        // 用户隐私部分仍可改
        assert!(locks.can_edit("local_folder_links"));
        assert!(locks.can_edit("vault_password"));
        assert!(locks.can_edit("backup_destination"));
    }

    #[test]
    fn member_state_helpers() {
        assert!(!MemberState::LoggedOut.is_logged_in());
        assert!(!MemberState::LoggedOut.is_paid());

        let free = MemberState::Free { account_id: "a".into() };
        assert!(free.is_logged_in());
        assert!(!free.is_paid());
        assert_eq!(free.account_id(), Some("a"));

        let paid = MemberState::Paid {
            account_id: "a".into(),
            license_id: "l".into(),
            llm_quota_remaining: 0,
        };
        assert!(paid.is_paid());
        assert_eq!(paid.account_id(), Some("a"));
    }

    #[test]
    fn unknown_setting_field_defaults_editable() {
        let locks = SettingsLocks::for_state(&MemberState::Paid {
            account_id: "u".into(),
            license_id: "l".into(),
            llm_quota_remaining: 100,
        });
        // 未在 SettingsLocks 列出的新字段默认可改 (向后兼容)
        assert!(locks.can_edit("totally_new_field_xyz"));
    }

    #[test]
    fn settings_locks_serde_roundtrip() {
        let s = SettingsLocks::for_state(&MemberState::Free { account_id: "a".into() });
        let json = serde_json::to_string(&s).unwrap();
        let back: SettingsLocks = serde_json::from_str(&json).unwrap();
        assert_eq!(back.device_binding, SettingLock::Locked);
        assert_eq!(back.llm_endpoint, SettingLock::Editable);
    }
}
