//! LLM settings 合并 — 把 cloud 下发的 gateway token + endpoint 写进
//! `app_settings.llm`，供桌面 chat 走云端代理且用户零手填 key.
//!
//! `app_settings` 的形态见 attune-server `routes/settings.rs::default_settings`：
//!   { "llm": { "provider": "openai_compat", "endpoint": <url>, "model": <m>, "api_key": <k> }, ... }
//!
//! 付费会员登录后 provider 固定 openai_compat、endpoint = gateway、api_key = new-api token.

use serde_json::{json, Value};

/// vault meta 中 app_settings 的 key — 被 attune-server routes/settings.rs 和
/// routes/member.rs 共享，集中在此避免两处硬编码漂移。
pub const SETTINGS_META_KEY: &str = "app_settings";

/// 判断 gateway 是否应该自动应用。
///
/// 行为：**configure-if-unconfigured** — 只有用户当前没有可用的 LLM 配置时才自动写入
/// gateway。若用户已设置了自己的 `api_key` 或 `endpoint`（BYOK / 本地 Ollama），保持不动。
///
/// "未配置"判定：`llm` 字段不存在，或其 `api_key` 与 `endpoint` 均为 null / 空字符串。
pub fn gateway_should_apply(settings: &Value) -> bool {
    let llm = match settings.get("llm") {
        Some(v) if v.is_object() => v,
        // llm 字段缺失或不是对象 → 视为未配置
        _ => return true,
    };
    let api_key_empty = llm
        .get("api_key")
        .and_then(|v| v.as_str())
        .map(|s| s.is_empty())
        .unwrap_or(true);
    let endpoint_empty = llm
        .get("endpoint")
        .and_then(|v| v.as_str())
        .map(|s| s.is_empty())
        .unwrap_or(true);
    // 只有两者都为空才视为未配置 → 应用 gateway
    api_key_empty && endpoint_empty
}

/// 把 gateway endpoint + token 合并进一份 `app_settings` JSON.
///
/// 仅在 [`gateway_should_apply`] 返回 `true` 时由调用方调用。
/// 函数本身无条件覆写 `provider`/`endpoint`/`api_key`；保留 `model` 等其它字段.
/// 返回新的 JSON（纯函数，不做 IO）.
///
/// **默认模型语义**(per spec 2026-05-24-deepseek-via-new-api-gateway-e2e.md Bug-1
/// 修复 Option C):若 `default_model` 为 `Some(...)` 且现有 `llm.model` 缺失 / null /
/// 空字符串,则把 `default_model` 写入 `llm.model`。已有用户配置 model 时不覆盖
/// (用户偏好优先)。`None` (老版 cloud 不返回此字段) → 不动 model 字段,保持向后兼容。
pub fn merge_gateway_into_settings(
    mut settings: Value,
    endpoint: &str,
    token: &str,
    default_model: Option<&str>,
) -> Value {
    if !settings.is_object() {
        settings = json!({});
    }
    // Safety: 上面已确保 settings 是 object，所以 as_object_mut 一定 Some。
    if let Some(obj) = settings.as_object_mut() {
        let llm = obj.entry("llm").or_insert_with(|| json!({}));
        // 若现有 llm 值不是 object，整体替换。
        if !llm.is_object() {
            *llm = json!({});
        }
        if let Some(llm_obj) = llm.as_object_mut() {
            llm_obj.insert("provider".into(), json!("openai_compat"));
            llm_obj.insert("endpoint".into(), json!(endpoint));
            llm_obj.insert("api_key".into(), json!(token));
            // Bug-1 fix: 只在用户未配置 model 时写入 cloud 默认 model。
            // "未配置" 判定与 gateway_should_apply 内的字段判定一致 — None / null / 空字符串。
            if let Some(dm) = default_model.filter(|s| !s.is_empty()) {
                let model_empty = llm_obj
                    .get("model")
                    .map(|v| match v {
                        Value::Null => true,
                        Value::String(s) => s.is_empty(),
                        _ => false,
                    })
                    .unwrap_or(true);
                if model_empty {
                    llm_obj.insert("model".into(), json!(dm));
                }
            }
        }
    }
    settings
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── merge_gateway_into_settings ──────────────────────────────────────────

    #[test]
    fn merges_into_empty_settings() {
        let out = merge_gateway_into_settings(json!({}), "https://gw/v1", "sk-abc", None);
        assert_eq!(out["llm"]["provider"], "openai_compat");
        assert_eq!(out["llm"]["endpoint"], "https://gw/v1");
        assert_eq!(out["llm"]["api_key"], "sk-abc");
    }

    #[test]
    fn preserves_existing_model_field() {
        let existing = json!({"llm": {"model": "gpt-4o", "provider": "ollama"}, "search": {}});
        let out = merge_gateway_into_settings(existing, "https://gw/v1", "sk-xyz", None);
        assert_eq!(out["llm"]["model"], "gpt-4o");           // kept
        assert_eq!(out["llm"]["provider"], "openai_compat"); // overwritten
        assert_eq!(out["llm"]["api_key"], "sk-xyz");
        assert!(out["search"].is_object());                  // unrelated key kept
    }

    #[test]
    fn replaces_non_object_llm() {
        let weird = json!({"llm": "garbage"});
        let out = merge_gateway_into_settings(weird, "https://gw/v1", "sk-1", None);
        assert_eq!(out["llm"]["endpoint"], "https://gw/v1");
    }

    // ── Bug-1 fix: default_model handling (spec 2026-05-24) ─────────────────

    #[test]
    fn applies_default_model_when_llm_absent() {
        // fresh vault, no llm section → default_model 写入 llm.model
        let out = merge_gateway_into_settings(
            json!({}),
            "https://gw/v1",
            "sk-abc",
            Some("deepseek-v4-flash"),
        );
        assert_eq!(out["llm"]["model"], "deepseek-v4-flash");
        assert_eq!(out["llm"]["endpoint"], "https://gw/v1");
        assert_eq!(out["llm"]["api_key"], "sk-abc");
    }

    #[test]
    fn applies_default_model_when_existing_model_null() {
        // 老 vault meta: llm.model=null → 视为未配置,写入 default
        let existing = json!({"llm": {"model": null, "api_key": "", "endpoint": ""}});
        let out = merge_gateway_into_settings(
            existing,
            "https://gw/v1",
            "sk-1",
            Some("deepseek-v4-flash"),
        );
        assert_eq!(out["llm"]["model"], "deepseek-v4-flash");
    }

    #[test]
    fn applies_default_model_when_existing_model_empty_string() {
        let existing = json!({"llm": {"model": "", "api_key": "", "endpoint": ""}});
        let out = merge_gateway_into_settings(
            existing,
            "https://gw/v1",
            "sk-1",
            Some("deepseek-v4-flash"),
        );
        assert_eq!(out["llm"]["model"], "deepseek-v4-flash");
    }

    #[test]
    fn does_not_override_user_configured_model() {
        // 用户手挑了 model — gateway 默认 model 不应覆盖。
        // (注意: 这条 path 实际不会触发,因 gateway_should_apply 会因 endpoint/key 为空才走进来;
        // 但函数本身要做到"用户已选 model 就保留",防回归。)
        let existing = json!({"llm": {"model": "qwen2.5:3b"}});
        let out = merge_gateway_into_settings(
            existing,
            "https://gw/v1",
            "sk-1",
            Some("deepseek-v4-flash"),
        );
        assert_eq!(out["llm"]["model"], "qwen2.5:3b");
    }

    #[test]
    fn skips_default_model_when_cloud_returns_none() {
        // 老版 accounts server 不返回 gateway_default_model → None,
        // attune-server 不写 model 字段,保持向后兼容(行为同旧版)。
        let out = merge_gateway_into_settings(json!({}), "https://gw/v1", "sk-abc", None);
        assert!(out["llm"].get("model").is_none(),
            "None default_model 时不应写入 model 字段");
    }

    #[test]
    fn skips_default_model_when_cloud_returns_empty_string() {
        // 防御: cloud 返回空串 "" 视同 None,不写入。
        let out = merge_gateway_into_settings(json!({}), "https://gw/v1", "sk-abc", Some(""));
        assert!(out["llm"].get("model").is_none(),
            "空串 default_model 不应被写入(防 model='' 触发 new-api 400)");
    }

    // ── gateway_should_apply ─────────────────────────────────────────────────

    #[test]
    fn should_apply_when_llm_absent() {
        // 默认出厂设置没有 llm 字段 → 应应用 gateway
        assert!(gateway_should_apply(&json!({})));
    }

    #[test]
    fn should_apply_when_llm_empty_fields() {
        // api_key 和 endpoint 均为空 → 视为未配置
        assert!(gateway_should_apply(
            &json!({"llm": {"model": "qwen2.5:3b", "api_key": "", "endpoint": ""}})
        ));
    }

    #[test]
    fn should_apply_when_llm_fields_null() {
        // null 值等价于未配置
        assert!(gateway_should_apply(
            &json!({"llm": {"api_key": null, "endpoint": null}})
        ));
    }

    #[test]
    fn should_not_apply_when_api_key_set() {
        // 用户已配置 BYOK api_key → 不覆盖
        assert!(!gateway_should_apply(
            &json!({"llm": {"api_key": "sk-user-byok", "endpoint": ""}})
        ));
    }

    #[test]
    fn should_not_apply_when_endpoint_set() {
        // 用户配置了本地 Ollama endpoint → 不覆盖
        assert!(!gateway_should_apply(
            &json!({"llm": {"api_key": "", "endpoint": "http://localhost:11434/v1"}})
        ));
    }

    #[test]
    fn should_not_apply_when_both_set() {
        // 完整的 BYOK 配置 → 不覆盖
        assert!(!gateway_should_apply(
            &json!({"llm": {"api_key": "sk-abc", "endpoint": "https://api.openai.com/v1"}})
        ));
    }

    #[test]
    fn should_apply_when_llm_is_not_object() {
        // llm 字段为非 object 类型 → 视为未配置，允许覆盖
        assert!(gateway_should_apply(&json!({"llm": "garbage"})));
    }
}
