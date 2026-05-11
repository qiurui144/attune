//! Member session — 客户端会员登录状态 + 设置锁定规则.
//!
//! 设计要点 (per 用户拍板):
//! - 会员登录后**大多数配置自动锁定** (云端下发, UI 灰显)
//! - 免费用户可手动配置更多
//! - 服务器端 cloud.sh 一键部署 cloud accounts; 客户端通过 cloud_client 对接

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemberState {
    /// 未登录 — 全部配置可改 (本地 self-host)
    LoggedOut,
    /// 免费用户 — 大部分配置可改, 一些 paid 功能锁
    Free { account_id: String },
    /// 付费会员 — 大部分配置由云端下发, UI 锁定
    Member {
        account_id: String,
        tier: String,
        license_id: String,
        /// 月度 LLM token 配额 (云端分配)
        llm_quota_remaining: u64,
    },
    /// 企业用户 (集体授权) — 几乎所有配置锁定, 管理员云端统一管
    Enterprise {
        account_id: String,
        team_id: String,
        license_id: String,
    },
}

impl MemberState {
    pub fn is_logged_in(&self) -> bool {
        !matches!(self, MemberState::LoggedOut)
    }
    pub fn is_paid(&self) -> bool {
        matches!(self, MemberState::Member { .. } | MemberState::Enterprise { .. })
    }
    pub fn account_id(&self) -> Option<&str> {
        match self {
            MemberState::LoggedOut => None,
            MemberState::Free { account_id }
            | MemberState::Member { account_id, .. }
            | MemberState::Enterprise { account_id, .. } => Some(account_id),
        }
    }
}

/// 配置项锁定状态 — 给 UI 决定是否灰显
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SettingLock {
    /// 用户可改
    Editable,
    /// 锁定, UI 灰显 (会员/企业云端下发)
    Locked,
    /// 仅企业管理员可改 (普通会员看到但不能改)
    EnterpriseAdminOnly,
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
    /// 按 MemberState 推导锁定规则
    pub fn for_state(state: &MemberState) -> Self {
        match state {
            // 未登录 = 全部可改 (self-host / 离线)
            MemberState::LoggedOut => Self {
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
            },
            // 免费用户: 大部分可改, paid 通道锁
            MemberState::Free { .. } => Self {
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
                device_binding: SettingLock::Locked,         // 云端绑定, 防共享
                backup_destination: SettingLock::Editable,
            },
            // 会员: LLM / 模型 / 插件 由云端管, UI 灰显
            MemberState::Member { .. } => Self {
                llm_endpoint: SettingLock::Locked,            // 云端 gateway 下发
                llm_model: SettingLock::Locked,               // 按 tier 推荐
                llm_api_key: SettingLock::Locked,             // 用户不持 raw key
                embedding_model: SettingLock::Locked,         // 按硬件 + tier
                ocr_engine: SettingLock::Editable,            // 本地装载, 可换
                data_dir: SettingLock::Locked,                // 防误操作
                local_folder_links: SettingLock::Editable,    // 用户隐私, 自管
                plugin_install: SettingLock::Locked,          // 云端按 license 自动装
                plugin_uninstall: SettingLock::Locked,        // 防误删
                vault_password: SettingLock::Editable,        // 用户主密码自己改
                device_binding: SettingLock::Locked,
                backup_destination: SettingLock::Editable,    // 用户隐私
            },
            // 企业: 几乎全锁, 仅 admin 可改
            MemberState::Enterprise { .. } => Self {
                llm_endpoint: SettingLock::EnterpriseAdminOnly,
                llm_model: SettingLock::EnterpriseAdminOnly,
                llm_api_key: SettingLock::Locked,
                embedding_model: SettingLock::EnterpriseAdminOnly,
                ocr_engine: SettingLock::EnterpriseAdminOnly,
                data_dir: SettingLock::Locked,
                local_folder_links: SettingLock::Editable,
                plugin_install: SettingLock::EnterpriseAdminOnly,
                plugin_uninstall: SettingLock::EnterpriseAdminOnly,
                vault_password: SettingLock::Editable,
                device_binding: SettingLock::Locked,
                backup_destination: SettingLock::EnterpriseAdminOnly,
            },
        }
    }

    /// 检查具体字段是否可改 — 给 PATCH /settings handler 用
    pub fn can_edit(&self, field: &str, is_enterprise_admin: bool) -> bool {
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
            SettingLock::EnterpriseAdminOnly => is_enterprise_admin,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logged_out_can_edit_everything() {
        let locks = SettingsLocks::for_state(&MemberState::LoggedOut);
        assert!(locks.can_edit("llm_endpoint", false));
        assert!(locks.can_edit("plugin_install", false));
        assert!(locks.can_edit("data_dir", false));
    }

    #[test]
    fn free_user_blocked_from_device_binding() {
        let locks = SettingsLocks::for_state(&MemberState::Free {
            account_id: "u1".into(),
        });
        assert!(!locks.can_edit("device_binding", false));
        // 但 LLM / 模型 / 插件 仍可改
        assert!(locks.can_edit("llm_endpoint", false));
        assert!(locks.can_edit("plugin_install", false));
    }

    #[test]
    fn member_locked_on_llm_and_plugin() {
        let locks = SettingsLocks::for_state(&MemberState::Member {
            account_id: "u1".into(),
            tier: "paid".into(),
            license_id: "l1".into(),
            llm_quota_remaining: 1_000_000,
        });
        // LLM 全锁 (云端下发)
        assert!(!locks.can_edit("llm_endpoint", false));
        assert!(!locks.can_edit("llm_model", false));
        assert!(!locks.can_edit("llm_api_key", false));
        // 插件锁 (按 license 自动装)
        assert!(!locks.can_edit("plugin_install", false));
        assert!(!locks.can_edit("plugin_uninstall", false));
        // 用户隐私部分仍可改
        assert!(locks.can_edit("local_folder_links", false));
        assert!(locks.can_edit("vault_password", false));
        assert!(locks.can_edit("backup_destination", false));
    }

    #[test]
    fn enterprise_admin_can_change_admin_only() {
        let locks = SettingsLocks::for_state(&MemberState::Enterprise {
            account_id: "u1".into(),
            team_id: "t1".into(),
            license_id: "l1".into(),
        });
        // 普通成员不能改
        assert!(!locks.can_edit("llm_model", false));
        assert!(!locks.can_edit("plugin_install", false));
        // 但企业 admin 可改
        assert!(locks.can_edit("llm_model", true));
        assert!(locks.can_edit("plugin_install", true));
        // 仍然 vault_password 自己改
        assert!(locks.can_edit("vault_password", false));
    }

    #[test]
    fn member_state_helpers() {
        assert!(!MemberState::LoggedOut.is_logged_in());
        assert!(!MemberState::LoggedOut.is_paid());

        let free = MemberState::Free { account_id: "a".into() };
        assert!(free.is_logged_in());
        assert!(!free.is_paid());
        assert_eq!(free.account_id(), Some("a"));

        let m = MemberState::Member {
            account_id: "a".into(),
            tier: "paid".into(),
            license_id: "l".into(),
            llm_quota_remaining: 0,
        };
        assert!(m.is_paid());

        let e = MemberState::Enterprise {
            account_id: "a".into(),
            team_id: "t".into(),
            license_id: "l".into(),
        };
        assert!(e.is_paid());
    }

    #[test]
    fn unknown_setting_field_defaults_editable() {
        let locks = SettingsLocks::for_state(&MemberState::Member {
            account_id: "u".into(),
            tier: "paid".into(),
            license_id: "l".into(),
            llm_quota_remaining: 100,
        });
        // 新字段 (未在 SettingsLocks 列出) 默认可改 (向后兼容)
        assert!(locks.can_edit("totally_new_field_xyz", false));
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
