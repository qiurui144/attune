//! LLM settings 合并 — 把 cloud 下发的 gateway token + endpoint 写进
//! `app_settings.llm`，供桌面 chat 走云端代理且用户零手填 key.
//!
//! `app_settings` 的形态见 attune-server `routes/settings.rs::default_settings`：
//!   { "llm": { "provider": "openai_compat", "endpoint": <url>, "model": <m>, "api_key": <k> }, ... }
//!
//! 付费会员登录后 provider 固定 openai_compat、endpoint = gateway、api_key = new-api token.

use serde_json::{json, Value};

/// 把 gateway endpoint + token 合并进一份 `app_settings` JSON.
///
/// - 不存在 `llm` 对象时创建之.
/// - 只覆写 `provider`/`endpoint`/`api_key`；保留 `model` 等其它字段.
/// - 返回新的 JSON（纯函数，不做 IO）.
pub fn merge_gateway_into_settings(mut settings: Value, endpoint: &str, token: &str) -> Value {
    if !settings.is_object() {
        settings = json!({});
    }
    let obj = settings.as_object_mut().expect("settings is object");
    let llm = obj.entry("llm").or_insert_with(|| json!({}));
    // If the existing `llm` value is not an object, replace it entirely.
    if !llm.is_object() {
        *llm = json!({});
    }
    let llm_obj = llm.as_object_mut().expect("llm is object");
    llm_obj.insert("provider".into(), json!("openai_compat"));
    llm_obj.insert("endpoint".into(), json!(endpoint));
    llm_obj.insert("api_key".into(), json!(token));
    settings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_into_empty_settings() {
        let out = merge_gateway_into_settings(json!({}), "https://gw/v1", "sk-abc");
        assert_eq!(out["llm"]["provider"], "openai_compat");
        assert_eq!(out["llm"]["endpoint"], "https://gw/v1");
        assert_eq!(out["llm"]["api_key"], "sk-abc");
    }

    #[test]
    fn preserves_existing_model_field() {
        let existing = json!({"llm": {"model": "gpt-4o", "provider": "ollama"}, "search": {}});
        let out = merge_gateway_into_settings(existing, "https://gw/v1", "sk-xyz");
        assert_eq!(out["llm"]["model"], "gpt-4o");           // kept
        assert_eq!(out["llm"]["provider"], "openai_compat"); // overwritten
        assert_eq!(out["llm"]["api_key"], "sk-xyz");
        assert!(out["search"].is_object());                  // unrelated key kept
    }

    #[test]
    fn replaces_non_object_llm() {
        let weird = json!({"llm": "garbage"});
        let out = merge_gateway_into_settings(weird, "https://gw/v1", "sk-1");
        assert_eq!(out["llm"]["endpoint"], "https://gw/v1");
    }
}
