use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use crate::state::SharedState;

const SETTINGS_KEY: &str = "app_settings";

pub async fn get_settings(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let recommended_summary = state.hardware.recommended_summary_model();
    let form_factor = state.hardware.form_factor;
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    let settings = vault.store().get_meta(SETTINGS_KEY)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let mut json: serde_json::Value = match settings {
        Some(data) => serde_json::from_slice(&data)
            .unwrap_or_else(|_| default_settings(recommended_summary, form_factor)),
        None => default_settings(recommended_summary, form_factor),
    };
    // 🔐 安全：redact api_key —— 即便 vault 已解锁，GET 响应也不该回传明文密钥。
    // 前端检测 `api_key_set: true` 表示已配置，显示占位 "●●●●●" 而非实际值。
    // 用户改 key 时必须重新填（否则保留旧值不变，见 update_settings::body 合并）
    redact_api_key(&mut json);
    Ok(Json(json))
}

/// 只接受 http:// 或 https:// 前缀，拒绝 javascript: / data: / file: 等危险 scheme
fn is_safe_http_url(s: &str) -> bool {
    let lower = s.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// 把 settings JSON 中的 `llm.api_key` 明文替换为 `null`，同时加 `llm.api_key_set` bool。
/// 用于 GET 响应 —— 前端永远拿不到明文 key。
fn redact_api_key(json: &mut serde_json::Value) {
    let Some(llm) = json.get_mut("llm").and_then(|v| v.as_object_mut()) else { return; };
    let has_key = llm.get("api_key")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    llm.insert("api_key".into(), serde_json::Value::Null);
    llm.insert("api_key_set".into(), serde_json::Value::Bool(has_key));
}

pub async fn update_settings(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let recommended_summary = state.hardware.recommended_summary_model();
    let form_factor = state.hardware.form_factor;
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    // Merge with existing settings
    let existing = vault.store().get_meta(SETTINGS_KEY)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let mut current: serde_json::Value = match existing {
        Some(data) => serde_json::from_slice(&data)
            .unwrap_or_else(|_| default_settings(recommended_summary, form_factor)),
        None => default_settings(recommended_summary, form_factor),
    };

    // 白名单校验：只允许写入已知配置键，防止任意键污染 vault_meta
    const ALLOWED_KEYS: &[&str] = &[
        "injection_mode", "injection_budget", "excluded_domains",
        "search", "embedding", "web_search", "llm",
        "summary_model", "context_strategy", "theme", "language",
        "skills",  // Sprint 2 Skills Router: { disabled: string[] }
        "wizard",  // wizard completion state: { complete: bool, current_step: int }
        "pluginhub", // G2 (2026-05-01): { url, license_key }
    ];
    // URL 字段白名单 scheme 校验（防 javascript: / data: 注入成 XSS 种子）
    if let Some(body_obj) = body.as_object() {
        if let Some(llm_obj) = body_obj.get("llm").and_then(|v| v.as_object()) {
            if let Some(ep) = llm_obj.get("endpoint").and_then(|v| v.as_str()) {
                if !ep.is_empty() && !is_safe_http_url(ep) {
                    return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({
                        "error": "llm.endpoint must be http:// or https:// URL"
                    }))));
                }
            }
        }
        if let Some(ws_obj) = body_obj.get("web_search").and_then(|v| v.as_object()) {
            if let Some(bp) = ws_obj.get("browser_path").and_then(|v| v.as_str()) {
                // 浏览器路径是文件路径，不是 URL；但不允许以 - 开头（防 argv 注入）
                if bp.starts_with('-') {
                    return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({
                        "error": "web_search.browser_path cannot start with '-' (argv injection risk)"
                    }))));
                }
            }
        }
        // Sprint 2 Skills Router: 校验 skills.disabled 必须是 string[]
        if let Some(skills_obj) = body_obj.get("skills").and_then(|v| v.as_object()) {
            if let Some(d) = skills_obj.get("disabled") {
                let arr_ok = d.as_array().map(|arr| arr.iter().all(|x| x.is_string())).unwrap_or(false);
                if !arr_ok {
                    return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({
                        "error": "skills.disabled must be an array of strings"
                    }))));
                }
            }
        }
        // 校验 ocr.active_profile 必须是已存在的 profile id (避免用户输错导致 OCR 回退)
        if let Some(ocr_obj) = body_obj.get("ocr").and_then(|v| v.as_object()) {
            if let Some(prof) = ocr_obj.get("active_profile").and_then(|v| v.as_str()) {
                let reg = attune_core::ocr::profile_registry::ProfileRegistry::load_default()
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;
                if reg.get(prof).is_none() {
                    return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({
                        "error": format!("ocr.active_profile '{prof}' 不存在 (用 GET /api/v1/ocr/profiles 查看可用 id)")
                    }))));
                }
            }
        }
    }

    // SettingsLocks enforce — 会员锁定字段拒绝更新.
    // 字段映射: settings JSON key → SettingsLocks field name
    let member_state = state.member_state.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let locks = attune_core::member_session::SettingsLocks::for_state(&member_state);
    if let Some(body_obj) = body.as_object() {
        // 仅 enforce 用户在应用窗口能改的字段; 底座配置 (embedding/ocr/data_dir 等) 由
        // 二进制打包默认装配, 不接受 PATCH (server 不在此 lock_map enforce).
        let lock_map: &[(&str, &str)] = &[
            ("llm", "cloud_llm"),               // 普通用户改云端 LLM, 付费锁
            ("pluginhub", "plugin_install"),    // pluginhub 配置变 → plugin_install lock
            ("ocr", "ocr_profiles"),            // ocr.active_profile 改受 ocr_profiles lock
        ];
        for (settings_key, lock_field) in lock_map {
            if body_obj.contains_key(*settings_key) && !locks.can_edit(lock_field) {
                return Err((
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({
                        "error": "setting_locked_by_member_tier",
                        "field": settings_key,
                        "lock_reason": format!("'{lock_field}' is locked under current membership tier"),
                        "hint": "GET /api/v1/member/locks 看完整锁定矩阵",
                    })),
                ));
            }
        }
    }

    // 嵌套对象键：这些字段的子字段支持 deep merge（客户端省略某子字段时保留原值）。
    // 主要为了 `llm.api_key` —— GET 响应已 redact，客户端若只改 model/provider 而不重填 key，
    // 我们不应把 key 抹成 null。
    const DEEP_MERGE_KEYS: &[&str] = &["llm", "ocr"];
    if let (Some(current_obj), Some(body_obj)) = (current.as_object_mut(), body.as_object()) {
        for (k, v) in body_obj {
            if !ALLOWED_KEYS.contains(&k.as_str()) { continue; }
            if DEEP_MERGE_KEYS.contains(&k.as_str()) {
                // Deep merge：取 current_obj[k] 和 body_obj[k] 两个对象，子字段逐个覆盖
                if let (Some(cur_sub), Some(new_sub)) = (
                    current_obj.get_mut(k).and_then(|x| x.as_object_mut()),
                    v.as_object(),
                ) {
                    for (sub_k, sub_v) in new_sub {
                        cur_sub.insert(sub_k.clone(), sub_v.clone());
                    }
                    continue;
                }
            }
            current_obj.insert(k.clone(), v.clone());
        }
    }

    let data = serde_json::to_vec(&current)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;
    vault.store().set_meta(SETTINGS_KEY, &data)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    // G2 (2026-05-01) — pluginhub 字段变化时热切 provider
    if body.get("pluginhub").is_some() {
        let url = current.get("pluginhub").and_then(|p| p.get("url")).and_then(|v| v.as_str());
        let key = current.get("pluginhub").and_then(|p| p.get("license_key")).and_then(|v| v.as_str());
        // 释放 vault lock 让 reload 能拿 plugin_hub mutex
        drop(vault);
        state.reload_plugin_hub(url, key);
    }

    // 返回前先 redact（防 API key 回流）
    redact_api_key(&mut current);
    Ok(Json(current))
}

/// 默认设置。`recommended_summary` 仅作为"用户主动选本地"时的硬件推荐 fallback；
/// `form_factor` 决定 LLM 默认 provider 路径：
/// - `Laptop` / `Server` / `Unknown` → `openai_compat`（远端 token，wizard 引导填 endpoint + key）
/// - `K3Appliance` → `ollama`（K3 镜像预装 qwen2.5:3b，开箱即用本地）
///
/// **v0.6.0-rc.3 起 LLM 默认走远端 token**（per CLAUDE.md M2 决策 + 用户反馈），
/// 避免本地 3B 模型在大多数硬件上 OOM 或效果差；K3 一体机形态例外（硬件预选过、镜像预装模型）。
fn default_settings(_recommended_summary: &str, form_factor: attune_core::platform::FormFactor) -> serde_json::Value {
    use attune_core::platform::FormFactor;

    // 形态分裂的 LLM 默认配置
    let llm_default = if form_factor == FormFactor::K3Appliance {
        // K3 一体机：本地 Ollama 优先，预装 qwen2.5:3b
        serde_json::json!({
            "provider": "ollama",
            "endpoint": "http://localhost:11434/v1",
            "model": "qwen2.5:3b",
            "api_key": null
        })
    } else {
        // Laptop / Server / Unknown：远端 token 默认，wizard 引导填
        serde_json::json!({
            "provider": "openai_compat",   // openai_compat / anthropic / deepseek / qwen / ollama / claude
            "endpoint": null,              // null → UI 引导填 (e.g. https://api.openai.com/v1)
            "model": null,                 // null → UI 引导填 (e.g. gpt-4o-mini / claude-3-5-haiku / deepseek-chat)
            "api_key": null
        })
    };

    serde_json::json!({
        // ── 普通用户可见 ──
        "theme": "system",         // system / dark / light
        "language": "zh-CN",
        // 摘要模型 null = 用户主动选 (Settings UI 引导填 LLM endpoint 后启用)；
        // 想用本地的可填 "qwen2.5:1.5b" 等 (recommended_summary 给硬件推荐建议)
        "summary_model": null,
        "context_strategy": "economical",      // economical(150字) / accurate(300字+片段) / raw(不压缩，仅本地)
        "web_search": {
            "enabled": true,
            "engine": "duckduckgo",
            "browser_path": null,
            "min_interval_ms": 2000
        },
        "llm": llm_default,

        // ── 本地 AI 底座（per CLAUDE.md "本地仅捆绑必要底座"决策）──
        // Embedding / Rerank / OCR / ASR 都是本地零费用，自动加载，用户无需配置。
        // 状态查询: GET /api/v1/ai_stack
        "embedding": {
            "model": "bge-m3",
            "ollama_url": "http://localhost:11434"
        },
        "rerank": {
            "enabled": true,                  // bge-reranker-v2-m3 自动从 HuggingFace 拉取
            "model_repo": "Xenova/bge-reranker-base"  // 想换可填 jina-v2-multilingual / bge-base-official
        },
        "ocr": {
            "enabled": true,                  // PP-OCRv5 + pdftoppm 自动检测
            "languages": "chi_sim+eng",
            "active_profile": attune_core::ocr::profile::OcrProfile::DEFAULT_ID
        },
        "asr": {
            "enabled": false,                 // v0.6: whisper.cpp 集成中；v0.6.x 启用
            "model": "whisper-small-q8"       // 中文 WER < 20% 实测满足
        },

        "skills": {
            "disabled": []
        },
        "plugins": {
            "disabled": []  // W4 E1: marketplace 禁用列表，list 用于 enabled 字段
        },

        // G2 (2026-05-01) — PluginHub 远端市场对接
        // null = 走内嵌 Mock provider（默认离线，看到 4 个 attune-pro 试用卡）
        // 配 url + license_key 后切到 HttpPluginHubProvider，调真 hub.attune.ai
        "pluginhub": {
            "url": null,                  // 例: "https://hub.attune.ai"
            "license_key": null           // 同 attune Pro 会员 license key（与 LLM Gateway 共享）
        },

        // ── 不在 UI 暴露（保留后端行为）──
        "injection_mode": "auto",
        "injection_budget": 2000,
        "excluded_domains": ["mail.google.com", "web.whatsapp.com"],
        "search": {
            "default_top_k": 10,
            "vector_weight": 0.6,
            "fulltext_weight": 0.4
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_core::platform::FormFactor;

    /// Laptop 形态：LLM 默认走远端 token (openai_compat + null endpoint/model)
    /// — 这是 v0.6.0 GA 既有行为，v0.6.1 必须保持兼容。
    #[test]
    fn laptop_form_factor_uses_remote_token() {
        let s = default_settings("qwen2.5:3b", FormFactor::Laptop);
        let llm = s.get("llm").expect("llm key");
        assert_eq!(llm.get("provider").and_then(|v| v.as_str()), Some("openai_compat"));
        assert!(llm.get("endpoint").map_or(true, |v| v.is_null()),
            "Laptop endpoint must be null (UI 引导填), got: {:?}", llm.get("endpoint"));
        assert!(llm.get("model").map_or(true, |v| v.is_null()),
            "Laptop model must be null (UI 引导填), got: {:?}", llm.get("model"));
        assert!(llm.get("api_key").map_or(true, |v| v.is_null()));
    }

    /// K3 一体机形态：LLM 默认走本地 Ollama (qwen2.5:3b 预装)
    /// — v0.6.1 新增的形态分裂路径。
    #[test]
    fn k3_form_factor_uses_local_ollama() {
        let s = default_settings("qwen2.5:3b", FormFactor::K3Appliance);
        let llm = s.get("llm").expect("llm key");
        assert_eq!(llm.get("provider").and_then(|v| v.as_str()), Some("ollama"));
        assert_eq!(llm.get("endpoint").and_then(|v| v.as_str()), Some("http://localhost:11434/v1"));
        assert_eq!(llm.get("model").and_then(|v| v.as_str()), Some("qwen2.5:3b"));
    }

    /// Server / Unknown 形态：与 Laptop 同行为（远端 token 默认）
    #[test]
    fn server_and_unknown_fallback_to_remote_token() {
        for ff in [FormFactor::Server, FormFactor::Unknown] {
            let s = default_settings("qwen2.5:3b", ff);
            let llm = s.get("llm").expect("llm key");
            assert_eq!(
                llm.get("provider").and_then(|v| v.as_str()), Some("openai_compat"),
                "FormFactor::{:?} should fall back to openai_compat", ff
            );
        }
    }

    /// 关键不变量：除 llm 之外的字段在所有形态下保持一致
    /// （form_factor 只影响 LLM 默认路径，不影响 web_search / embedding / reranker 等本地底座）
    #[test]
    fn non_llm_settings_invariant_across_form_factors() {
        let laptop = default_settings("qwen2.5:3b", FormFactor::Laptop);
        let k3 = default_settings("qwen2.5:3b", FormFactor::K3Appliance);

        // Embedding / web_search / rerank / OCR 这些"本地底座"应该完全相同
        for key in &["web_search", "embedding", "rerank", "ocr", "asr"] {
            assert_eq!(laptop.get(key), k3.get(key),
                "{} should be identical across form factors (only LLM differs)", key);
        }
    }
}
