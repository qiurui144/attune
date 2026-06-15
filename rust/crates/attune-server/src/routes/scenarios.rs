//! Scenarios route — 行业工作台「场景卡片」聚合端点。
//!
//! `GET /api/v1/scenarios` 把所有已装插件的可 dispatch agent → 派生扁平场景清单,
//! 供前端 WorkbenchView 一次性渲染分组卡片(免前端各自拼 plugins+forms)。
//! 数据全部从内存中已加载的 PluginRegistry 只读派生(plugin_registry::all_scenarios),
//! 无 DB 表、无网络。chat 关键词触发不变(本端点是**新增主入口**,非替换,spec §10)。
//!
//! 派生规则见 `attune_core::plugin_registry::Scenario`:
//!   - cost_tier: llm_tokens==0/未声明 → "free";>0 → "cloud"
//!   - has_form / form_ref: 来自 ui_component `target: agent:<id>`
//!   - library runtime 的 agent(内部工具)不出卡
//!
//! 每张卡附 `enabled`(settings.plugins.disabled 过滤,与 /plugins 一致)+
//! `entitlement_status`(EntitlementCache 运行态) → 前端对未授权插件显示锁标/购买引导。

use crate::error::AppResult;
use crate::state::SharedState;
use axum::extract::State;
use axum::Json;

const SETTINGS_KEY: &str = "app_settings";

/// 从 settings.json 读 plugins.disabled 数组。vault locked 时返回空(默认全启用)。
/// 与 routes/plugins.rs 同逻辑(工作台与插件市场对 enabled 判定一致)。
fn load_disabled_plugin_ids(state: &SharedState) -> Vec<String> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if vault.dek_db().is_err() {
        return Vec::new();
    }
    let raw = match vault.store().get_meta(SETTINGS_KEY) {
        Ok(Some(b)) => b,
        _ => return Vec::new(),
    };
    let json: serde_json::Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    json.get("plugins")
        .and_then(|p| p.get("disabled"))
        .and_then(|d| d.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

/// GET /api/v1/scenarios — 工作台场景卡片清单(已装插件 → agent 派生)。
pub async fn list(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    let disabled = load_disabled_plugin_ids(&state);
    let now = chrono::Utc::now();

    let scenarios: Vec<serde_json::Value> = state
        .plugin_registry
        .all_scenarios()
        .into_iter()
        .map(|s| {
            let enabled = !disabled.iter().any(|d| d == &s.plugin_id);
            let entitlement_status = state.entitlement_cache.status(&s.plugin_id, &now).as_api_str();
            serde_json::json!({
                "plugin_id": s.plugin_id,
                "plugin_label": s.plugin_label,
                "plugin_type": s.plugin_type,
                "agent_id": s.agent_id,
                "label": s.label,
                "intent": s.intent,
                "scenario": s.scenario,
                "cost_tier": s.cost_tier,
                "llm_required": s.llm_required,
                "case_kind": s.case_kind,
                "has_form": s.has_form,
                "form_ref": s.form_ref,
                "output_modes": s.output_modes,
                "enabled": enabled,
                "entitlement_status": entitlement_status,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "scenarios": scenarios })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // These tests pin attune's data dir to a temp dir via the thread-local override
    // seam (test_support::override_data_dir) — works on Windows too, where `dirs`
    // ignores XDG_DATA_HOME. The guard restores the prior override on drop.

    /// 装一个 law-pro 插件 → /scenarios 派生出该 agent 的卡片,带 cost_tier/has_form/enabled。
    #[tokio::test]
    async fn list_returns_scenario_cards() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let _dir = crate::test_support::override_data_dir(tmp.path().join("attune"));
        let plugin_dir = tmp.path().join("attune").join("plugins").join("law-pro");
        std::fs::create_dir_all(&plugin_dir).expect("mkdir plugin");
        std::fs::write(
            plugin_dir.join("plugin.yaml"),
            r#"
id: law-pro
name: 律师助手
type: industry
version: "1.0.0"
agents:
  - id: civil_loan_agent
    scenario: "借贷本息计算"
    intent: 核算 compute
    case_kinds: [civil-loan]
    runtime: rust_binary
    binary: bin/x
    cost: { llm_tokens: 0 }
ui_components:
  - id: civil_loan
    target: agent:civil_loan_agent
    html: forms/civil_loan.yaml
    description: 借贷表单
"#,
        )
        .expect("write plugin.yaml");

        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-scenarios-not-real").expect("setup");
        let state = Arc::new(crate::state::AppState::new(vault, false));

        let resp = list(axum::extract::State(state)).await.expect("list ok");
        let body = resp.0;
        let cards = body["scenarios"].as_array().expect("scenarios array");
        let card = cards.iter().find(|c| c["agent_id"] == "civil_loan_agent").expect("card present");
        assert_eq!(card["label"], "借贷本息计算");
        assert_eq!(card["plugin_label"], "律师助手");
        assert_eq!(card["cost_tier"], "free");
        assert_eq!(card["has_form"], true);
        assert_eq!(card["form_ref"]["form_id"], "civil_loan");
        assert_eq!(card["enabled"], true, "未 disabled → enabled");
        // free / unregistered plugin → entitlement_status "free".
        assert_eq!(card["entitlement_status"], "free");
    }

    /// 无任何插件 → scenarios 空数组(工作台空态),不报错。
    #[tokio::test]
    async fn list_empty_when_no_plugins() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let _dir = crate::test_support::override_data_dir(tmp.path().join("attune"));
        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-scenarios-empty-not-real").expect("setup");
        let state = Arc::new(crate::state::AppState::new(vault, false));

        let resp = list(axum::extract::State(state)).await.expect("list ok");
        let cards = resp.0["scenarios"].as_array().expect("array");
        assert!(cards.is_empty(), "no plugins → empty scenarios");
    }

    /// disabled 插件的卡片 enabled=false(工作台据此置灰/隐藏)。
    #[tokio::test]
    async fn list_marks_disabled_plugin() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let _dir = crate::test_support::override_data_dir(tmp.path().join("attune"));
        let plugin_dir = tmp.path().join("attune").join("plugins").join("law-pro");
        std::fs::create_dir_all(&plugin_dir).expect("mkdir");
        std::fs::write(
            plugin_dir.join("plugin.yaml"),
            "id: law-pro\nname: 律师\ntype: industry\nversion: \"1.0.0\"\nagents:\n  - id: a1\n    scenario: 场景1\n    runtime: rust_binary\n    binary: bin/x\n",
        )
        .expect("write");

        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-scenarios-disabled-not-real").expect("setup");
        // 写 settings.plugins.disabled = [law-pro]
        let settings = serde_json::json!({ "plugins": { "disabled": ["law-pro"] } });
        vault
            .store()
            .set_meta(SETTINGS_KEY, &serde_json::to_vec(&settings).unwrap())
            .expect("set settings");
        let state = Arc::new(crate::state::AppState::new(vault, false));

        let resp = list(axum::extract::State(state)).await.expect("list ok");
        let cards = resp.0["scenarios"].as_array().expect("array").clone();
        let card = cards.iter().find(|c| c["agent_id"] == "a1").expect("card");
        assert_eq!(card["enabled"], false, "disabled plugin → card enabled=false");
    }

    /// 派生层永不 silent-fail: cost_tier/has_form 字段恒存在(契约稳定)。
    #[tokio::test]
    async fn list_card_shape_is_stable() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let _dir = crate::test_support::override_data_dir(tmp.path().join("attune"));
        let plugin_dir = tmp.path().join("attune").join("plugins").join("tech-pro");
        std::fs::create_dir_all(&plugin_dir).expect("mkdir");
        std::fs::write(
            plugin_dir.join("plugin.yaml"),
            "id: tech-pro\nname: 工程\ntype: industry\nversion: \"1.0.0\"\nagents:\n  - id: code-reviewer\n    description: 代码审查\n    runtime: in_process\n",
        )
        .expect("write");

        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-scenarios-shape-not-real").expect("setup");
        let state = Arc::new(crate::state::AppState::new(vault, false));

        let resp = list(axum::extract::State(state)).await.expect("ok");
        let cards = resp.0["scenarios"].as_array().expect("array").clone();
        let card = &cards[0];
        // 无 case_kind 的 agent 也出卡,case_kind = null。
        assert_eq!(card["case_kind"], serde_json::Value::Null);
        assert_eq!(card["cost_tier"], "free");
        assert_eq!(card["has_form"], false);
        assert!(card.get("label").is_some());
        assert!(card.get("llm_required").is_some());
    }
}
