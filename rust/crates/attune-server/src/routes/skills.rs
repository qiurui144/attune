//! GET /api/v1/skills — 列出 type=skill 的所有 plugin（含 chat_trigger 摘要 + 用户禁用状态）
//!
//! 配置简单：UI 通过这个端点列出 + PATCH /settings 写 skills.disabled，
//! 用户从不需要手编 plugin.yaml。

use axum::{extract::State, Json};
use serde::Serialize;
use std::collections::HashSet;

use crate::state::SharedState;

#[derive(Serialize)]
pub struct SkillSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub keywords: Vec<String>,
    pub patterns: Vec<String>,
    /// chat_trigger.enabled — plugin 自身声明的状态（false 表示插件作者关闭了 chat 触发）
    pub enabled_in_plugin: bool,
    /// settings.skills.disabled.contains(id) — 用户在 UI 里禁用
    pub disabled_by_user: bool,
}

#[derive(Serialize)]
pub struct SkillsListResponse {
    pub skills: Vec<SkillSummary>,
}

pub async fn list_skills(State(state): State<SharedState>) -> Json<SkillsListResponse> {
    let disabled: HashSet<String> = {
        let bytes = match state.vault.lock() {
            Ok(vault) => vault.store().get_meta("app_settings").ok().flatten(),
            Err(_) => None,
        };
        bytes
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| {
                v.get("skills")
                    .and_then(|s| s.get("disabled"))
                    .and_then(|d| d.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
            })
            .unwrap_or_default()
    };

    let plugin_registry = crate::routes::plugins::current_plugin_registry(&state);
    let mut skills = Vec::new();
    for plugin in plugin_registry.plugins_by_type("skill") {
        let trigger = plugin.manifest.chat_trigger.as_ref();
        skills.push(SkillSummary {
            id: plugin.manifest.id.clone(),
            name: plugin.manifest.name.clone(),
            description: plugin.manifest.description.clone(),
            version: plugin.manifest.version.clone(),
            keywords: trigger.map(|t| t.keywords.clone()).unwrap_or_default(),
            patterns: trigger.map(|t| t.patterns.clone()).unwrap_or_default(),
            enabled_in_plugin: trigger.map(|t| t.enabled).unwrap_or(false),
            disabled_by_user: disabled.contains(&plugin.manifest.id),
        });
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Json(SkillsListResponse { skills })
}
