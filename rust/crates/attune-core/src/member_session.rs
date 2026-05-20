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

/// 应用窗口 (Tauri 桌面 GUI) 暴露给用户的可配置项.
///
/// 产品定义:
/// - 应用面向非专业用户, 不暴露技术配置 (LLM 底座 / embedding / reranker / OCR 引擎 / 数据目录)
/// - 底座模型随二进制打包, 默认高精度, 不降级
/// - 用户唯一可改:
///   1. vault 主密码 (改密码)
///   2. 本地知识库目录关联 (隐私自管)
///   3. plugin 装载 (开源标准 MCP / skill / agents)
///   4. 云端大模型配置 — 仅普通免费用户 (自己 API key); 付费 hidden (gateway 下发)
///   5. OCR 多场景预设 (场景化, 不暴露引擎参数)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsLocks {
    /// vault 主密码 (始终可改, 用户自管)
    pub vault_password: SettingLock,
    /// 本地知识库目录关联 (始终可改, 隐私自管)
    pub local_folder_links: SettingLock,
    /// 装 plugin (社区免费 / 开发者本地). 付费用户锁 — 云端按 license 自动 sync.
    pub plugin_install: SettingLock,
    /// 卸载 plugin. 付费用户锁 — 防误删自动装的 pro plugin.
    pub plugin_uninstall: SettingLock,
    /// 云端大模型配置 (普通免费用户配自己 API key + endpoint).
    /// 付费用户锁 — gateway 自动下发, 用户不持 raw key.
    pub cloud_llm: SettingLock,
    /// OCR 场景预设 (用户选不同预设给不同场景: 合同 / 票据 / 截图 / ...)
    /// 不暴露引擎 / 模型 / DPI 等技术参数.
    pub ocr_profiles: SettingLock,
}

impl SettingsLocks {
    /// 按 MemberState 推导锁定规则.
    ///
    /// 离线 / 免费: 全可改 (含云端 LLM — 普通用户配自己 API key).
    /// 付费:        cloud_llm 锁 (gateway 下发, 用户不持 raw key) + plugin_uninstall 锁
    ///              (防误删 cloud-sync 自动装的 pro plugin).
    ///              **plugin_install / pluginhub URL 解锁** — 付费会员是 hub 的目标受众,
    ///              不应被锁在 Mock provider 上 (per E2E P0 bug fix, 2026-05-20):
    ///              桌面要能 PATCH `pluginhub.{url,license_key}` 切到 HttpPluginHubProvider
    ///              才装得到真 pro plugin.
    ///              其他不变: 主密码 + 本地目录 + OCR profile 仍可改.
    pub fn for_state(state: &MemberState) -> Self {
        let all_editable = Self {
            vault_password: SettingLock::Editable,
            local_folder_links: SettingLock::Editable,
            plugin_install: SettingLock::Editable,
            plugin_uninstall: SettingLock::Editable,
            cloud_llm: SettingLock::Editable,
            ocr_profiles: SettingLock::Editable,
        };
        match state {
            MemberState::LoggedOut | MemberState::Free { .. } => all_editable,
            MemberState::Paid { .. } => Self {
                // plugin_install 保持 Editable — 付费会员需要能 PATCH pluginhub.{url,license_key}
                // 切到 HttpPluginHubProvider 才能装真 pro plugin。
                plugin_uninstall: SettingLock::Locked, // 防误删 cloud-sync 自动装的 pro plugin
                cloud_llm: SettingLock::Locked,        // gateway 自动下发, 用户不持 raw key
                ..all_editable
            },
        }
    }

    /// 检查具体字段是否可改 — 给 PATCH /settings handler 用.
    /// 未知字段默认放行 (向后兼容: 不在此 6 字段内的设置 server 不 enforce lock).
    pub fn can_edit(&self, field: &str) -> bool {
        let lock = match field {
            "vault_password" => self.vault_password,
            "local_folder_links" => self.local_folder_links,
            "plugin_install" => self.plugin_install,
            "plugin_uninstall" => self.plugin_uninstall,
            "cloud_llm" => self.cloud_llm,
            "ocr_profiles" => self.ocr_profiles,
            _ => return true,
        };
        matches!(lock, SettingLock::Editable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logged_out_can_edit_all_6_fields() {
        let locks = SettingsLocks::for_state(&MemberState::LoggedOut);
        for f in ["vault_password", "local_folder_links", "plugin_install",
                  "plugin_uninstall", "cloud_llm", "ocr_profiles"] {
            assert!(locks.can_edit(f), "logged_out should edit {f}");
        }
    }

    #[test]
    fn free_user_can_edit_all_6_fields() {
        let locks = SettingsLocks::for_state(&MemberState::Free {
            account_id: "u1".into(),
        });
        // 普通免费用户: 仍配自己的云端 LLM API key
        for f in ["vault_password", "local_folder_links", "plugin_install",
                  "plugin_uninstall", "cloud_llm", "ocr_profiles"] {
            assert!(locks.can_edit(f), "free user should edit {f}");
        }
    }

    #[test]
    fn paid_locks_cloud_llm_and_plugin_uninstall_only() {
        let locks = SettingsLocks::for_state(&MemberState::Paid {
            account_id: "u1".into(),
            license_id: "l1".into(),
            llm_quota_remaining: 1_000_000,
        });
        // 付费锁: cloud_llm (gateway 下发) + plugin_uninstall (防误删 cloud-sync pro plugin).
        assert!(!locks.can_edit("cloud_llm"));
        assert!(!locks.can_edit("plugin_uninstall"));
        // 付费会员必须能改 plugin_install / pluginhub URL — 切到 HttpPluginHubProvider
        // 是 entitled 用户的核心权益, 不能锁在 Mock provider 上.
        assert!(
            locks.can_edit("plugin_install"),
            "paid members must be able to configure pluginhub URL to reach real hub"
        );
        // 仍可改: 主密码 + 本地目录 + OCR profile
        assert!(locks.can_edit("vault_password"));
        assert!(locks.can_edit("local_folder_links"));
        assert!(locks.can_edit("ocr_profiles"));
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
        let s = SettingsLocks::for_state(&MemberState::Paid {
            account_id: "a".into(),
            license_id: "l".into(),
            llm_quota_remaining: 0,
        });
        let json = serde_json::to_string(&s).unwrap();
        let back: SettingsLocks = serde_json::from_str(&json).unwrap();
        // paid 锁: cloud_llm (gateway 下发) + plugin_uninstall
        assert_eq!(back.cloud_llm, SettingLock::Locked);
        assert_eq!(back.plugin_uninstall, SettingLock::Locked);
        // 仍可改 — plugin_install / pluginhub 解锁让付费会员能配真 hub
        assert_eq!(back.plugin_install, SettingLock::Editable);
        assert_eq!(back.vault_password, SettingLock::Editable);
        assert_eq!(back.local_folder_links, SettingLock::Editable);
        assert_eq!(back.ocr_profiles, SettingLock::Editable);
    }
}
