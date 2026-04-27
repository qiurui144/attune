use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use crate::state::SharedState;

const SETTINGS_KEY: &str = "app_settings";

pub async fn get_settings(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let recommended_summary = state.hardware.recommended_summary_model();
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    let settings = vault.store().get_meta(SETTINGS_KEY)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let mut json: serde_json::Value = match settings {
        Some(data) => serde_json::from_slice(&data)
            .unwrap_or_else(|_| default_settings(recommended_summary)),
        None => default_settings(recommended_summary),
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
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    // Merge with existing settings
    let existing = vault.store().get_meta(SETTINGS_KEY)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let mut current: serde_json::Value = match existing {
        Some(data) => serde_json::from_slice(&data)
            .unwrap_or_else(|_| default_settings(recommended_summary)),
        None => default_settings(recommended_summary),
    };

    // 白名单校验：只允许写入已知配置键，防止任意键污染 vault_meta
    const ALLOWED_KEYS: &[&str] = &[
        "injection_mode", "injection_budget", "excluded_domains",
        "search", "embedding", "web_search", "llm",
        "summary_model", "context_strategy", "theme", "language",
        "skills",  // Sprint 2 Skills Router: { disabled: string[] }
        "wizard",  // wizard completion state: { complete: bool, current_step: int }
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
    }

    // 嵌套对象键：这些字段的子字段支持 deep merge（客户端省略某子字段时保留原值）。
    // 主要为了 `llm.api_key` —— GET 响应已 redact，客户端若只改 model/provider 而不重填 key，
    // 我们不应把 key 抹成 null。
    const DEEP_MERGE_KEYS: &[&str] = &["llm"];
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

    // 返回前先 redact（防 API key 回流）
    redact_api_key(&mut current);
    Ok(Json(current))
}

/// 默认设置。`recommended_summary` 仅作为"用户主动选本地"时的硬件推荐 fallback；
/// **v0.6.0-rc.3 起 LLM 默认走远端 token**（per CLAUDE.md M2 决策 + 用户反馈），
/// 避免本地 3B 模型在大多数硬件上 OOM 或效果差。
fn default_settings(_recommended_summary: &str) -> serde_json::Value {
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
        "llm": {
            // 默认远端 token (per CLAUDE.md "M2 决策"：本地不预装 LLM，避免 OOM / 3B 效果差)
            // 用户首次启动 Settings UI 引导填 endpoint + api_key
            "provider": "openai_compat",   // openai_compat / anthropic / deepseek / qwen / ollama / claude
            "endpoint": null,              // null → UI 引导填 (e.g. https://api.openai.com/v1)
            "model": null,                 // null → UI 引导填 (e.g. gpt-4o-mini / claude-3-5-haiku / deepseek-chat)
            "api_key": null
        },

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
            "enabled": true,                  // tesseract + pdftoppm 系统 PATH 自动检测
            "languages": "chi_sim+eng"
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
