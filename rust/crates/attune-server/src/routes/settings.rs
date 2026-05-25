use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use crate::state::SharedState;
use attune_core::llm_settings::SETTINGS_META_KEY as SETTINGS_KEY;

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

/// SettingsLocks 校验违规 (体内字段级粒度)。
pub(crate) struct LockViolation {
    pub settings_key: &'static str,
    pub lock_field: &'static str,
}

/// 按 SettingsLocks + body 内容判断是否触犯会员锁定字段。
///
/// 字段映射: settings JSON key → SettingsLocks field name + (可选)子字段白名单。
/// - 子字段 = `None`: 整对象触发 lock(老行为)
/// - 子字段 = `Some(&[...])`: **仅当 body 改了任一列出的子字段时触发 lock**(粒度细化)
///
/// Bug-2 fix (spec 2026-05-24-deepseek-via-new-api-gateway-e2e.md):
/// `cloud_llm` 锁不再 cover 整个 `llm` 对象。付费用户应能换 `model`(channel 内有多个
/// 模型可选);仅锁 `endpoint` / `api_key` / `provider`(gateway URL + token 由 cloud
/// 下发,provider 若被切走会绕过 gateway 计量)。
pub(crate) fn check_settings_locks(
    body: &serde_json::Value,
    locks: &attune_core::member_session::SettingsLocks,
) -> Option<LockViolation> {
    let body_obj = body.as_object()?;

    type SubFields = Option<&'static [&'static str]>;
    const LLM_LOCKED_SUBFIELDS: SubFields = Some(&["endpoint", "api_key", "provider"]);
    let lock_map: &[(&str, SubFields, &str)] = &[
        // `llm`: cloud_llm 锁仅 cover endpoint/api_key/provider;model 用户可改
        ("llm", LLM_LOCKED_SUBFIELDS, "cloud_llm"),
        // `pluginhub`: 整对象锁(老行为)
        ("pluginhub", None, "plugin_install"),
        // `ocr`: 整对象锁(老行为)
        ("ocr", None, "ocr_profiles"),
    ];

    for (settings_key, sub_fields, lock_field) in lock_map {
        if !body_obj.contains_key(*settings_key) {
            continue;
        }
        if locks.can_edit(lock_field) {
            continue;
        }
        // lock_field 是 Locked — 看是否真的触碰了 locked sub-fields。
        let touches_locked = match sub_fields {
            None => true, // 整对象锁
            Some(allowed_locked) => body_obj
                .get(*settings_key)
                .and_then(|v| v.as_object())
                .map(|sub_obj| sub_obj.keys().any(|k| allowed_locked.contains(&k.as_str())))
                // body 给的不是 object(异常输入) → 保守拒绝
                .unwrap_or(true),
        };
        if touches_locked {
            return Some(LockViolation {
                settings_key,
                lock_field,
            });
        }
    }
    None
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
        "cloud", // FEAT-1 (2026-05-14): { accounts_url } — 自部署 / 私有 cloud 环境覆盖默认 attune.ai
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
    let member_state = state.member_state.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let locks = attune_core::member_session::SettingsLocks::for_state(&member_state);
    if let Some(violation) = check_settings_locks(&body, &locks) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "setting_locked_by_member_tier",
                "field": violation.settings_key,
                "lock_reason": format!("'{}' is locked under current membership tier", violation.lock_field),
                "hint": "请升级会员或在「设置 → 会员」查看锁定矩阵",
            })),
        ));
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

    // 准备热切参数（仍持 vault lock），值复制完释放再触发 reload，避免死锁
    let pluginhub_url = body.get("pluginhub").and_then(|_| {
        current.get("pluginhub").and_then(|p| p.get("url")).and_then(|v| v.as_str()).map(|s| s.to_string())
    });
    let pluginhub_key = body.get("pluginhub").and_then(|_| {
        current.get("pluginhub").and_then(|p| p.get("license_key")).and_then(|v| v.as_str()).map(|s| s.to_string())
    });
    let need_llm_reload = body.get("llm").is_some();
    drop(vault);

    // G2 (2026-05-01) — pluginhub 字段变化时热切 provider
    if body.get("pluginhub").is_some() {
        state.reload_plugin_hub(pluginhub_url.as_deref(), pluginhub_key.as_deref());
    }
    // 2026-05-14 — llm 字段变化时热切 LLM provider, 避免要求重启 server。
    // 修复 wizard 5 步保存云端 LLM 后, state.llm 仍是 None 导致 chat 503 的 bug。
    if need_llm_reload {
        state.reload_llm();
    }

    // 返回前先 redact（防 API key 回流）
    redact_api_key(&mut current);
    Ok(Json(current))
}

/// 默认设置。`recommended_summary` 仅作为"用户主动选本地"时的硬件推荐 fallback；
/// `form_factor` 决定 LLM 默认 provider 路径：
/// - `Laptop` / `Server` / `Unknown` → `openai_compat`（远端 token，wizard 引导填 endpoint + key）
/// - `K3Appliance` → `ollama`（K3 镜像默认本地 Ollama，但不预设具体 chat 模型）
///
/// **v0.6.0-rc.3 起 LLM 默认走远端 token**（per CLAUDE.md M2 决策 + 用户反馈），
/// 避免本地 3B 模型在大多数硬件上 OOM 或效果差；K3 一体机形态例外（硬件预选过、镜像预装模型）。
fn default_settings(_recommended_summary: &str, form_factor: attune_core::platform::FormFactor) -> serde_json::Value {
    use attune_core::platform::FormFactor;

    // 形态分裂的 LLM 默认配置
    let llm_default = if form_factor == FormFactor::K3Appliance {
        // K3 一体机：本地 Ollama 优先，但不预设具体 chat 模型
        serde_json::json!({
            "provider": "ollama",
            "endpoint": "http://localhost:11434/v1",
                "model": null,
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
        // 摘要模型默认固定为本地可运行且效果较稳的 qwen2.5:3b；可在 Settings 中覆盖。
        "summary_model": "qwen2.5:3b",
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

        // FEAT-1 (2026-05-14) — 自部署 cloud cluster 入口
        // null = 默认 attune.ai 公共 cloud (accounts.attune.ai / hub.attune.ai / gateway.attune.ai)
        // 自部署: 填入私有 cluster URL, 三个 endpoint 分别对应不同微服务.
        // 用户场景: 企业内网部署 attune-cloud-* 容器后, 在 Settings UI 填入这三个地址
        "cloud": {
            "accounts_url": null,         // 例: "https://accounts.your-company.com" (member login / license)
            "gateway_url": null,          // 例: "https://gateway.your-company.com" (LLM token gateway)
                                          // pluginhub URL 仍走上方 pluginhub.url (历史命名保留)
        },

        // ── 不在 UI 暴露（保留后端行为）──
        "injection_mode": "auto",
        "injection_budget": 2000,
        "excluded_domains": ["mail.google.com", "web.whatsapp.com"],
        "search": {
            "default_top_k": 10,
            "vector_weight": 0.6,
            "fulltext_weight": 0.4,
            // 检索 query 改写：LLM 把口语化 query 转为关键词序列，提升 RAG hit rate。
            // 默认关闭——需要 LLM 配置且用户明确开启；LLM 不可用时自动跳过，不报错。
            "query_rewrite": {
                "enabled": false
            }
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

    /// K3 一体机形态：LLM 默认走本地 Ollama，但不预设具体 chat 模型。
    /// — v0.6.1 新增的形态分裂路径。
    #[test]
    fn k3_form_factor_uses_local_ollama() {
        let s = default_settings("qwen2.5:3b", FormFactor::K3Appliance);
        let llm = s.get("llm").expect("llm key");
        assert_eq!(llm.get("provider").and_then(|v| v.as_str()), Some("ollama"));
        assert_eq!(llm.get("endpoint").and_then(|v| v.as_str()), Some("http://localhost:11434/v1"));
            assert!(llm.get("model").map_or(true, |v| v.is_null()),
                "K3 model must stay unset so runtime can auto-detect a lighter local model, got: {:?}", llm.get("model"));
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

    // ── Bug-2 fix: SettingsLocks 粒度 (spec 2026-05-24) ─────────────────────

    use attune_core::member_session::{MemberState, SettingsLocks};

    fn paid_locks() -> SettingsLocks {
        SettingsLocks::for_state(&MemberState::Paid {
            account_id: "u1".into(),
            license_id: "lic-1".into(),
            llm_quota_remaining: 0,
        })
    }

    fn free_locks() -> SettingsLocks {
        SettingsLocks::for_state(&MemberState::Free { account_id: "u1".into() })
    }

    #[test]
    fn paid_user_can_change_llm_model() {
        // Bug-2 核心:付费会员只改 model(channel 内 alias),应放行,不触发 cloud_llm 锁
        let body = serde_json::json!({"llm": {"model": "deepseek-v4-pro"}});
        assert!(check_settings_locks(&body, &paid_locks()).is_none(),
            "paid user should be able to swap model under same channel");
    }

    #[test]
    fn paid_user_cannot_change_llm_endpoint() {
        // gateway URL 由 cloud 下发,用户不能改 (绕开 gateway 计量 / 路由)
        let body = serde_json::json!({"llm": {"endpoint": "https://api.openai.com/v1"}});
        let v = check_settings_locks(&body, &paid_locks()).expect("must violate");
        assert_eq!(v.settings_key, "llm");
        assert_eq!(v.lock_field, "cloud_llm");
    }

    #[test]
    fn paid_user_cannot_change_llm_api_key() {
        // gateway token 由 cloud 下发
        let body = serde_json::json!({"llm": {"api_key": "sk-user-tries-to-swap"}});
        assert!(check_settings_locks(&body, &paid_locks()).is_some());
    }

    #[test]
    fn paid_user_cannot_change_llm_provider() {
        // 用户切换 provider(如 ollama) 会绕过 gateway → 锁
        let body = serde_json::json!({"llm": {"provider": "ollama"}});
        assert!(check_settings_locks(&body, &paid_locks()).is_some());
    }

    #[test]
    fn paid_user_partial_patch_with_only_model_and_query_rewrite() {
        // 混合 patch: 改 model + 改 search 子配置 → 只看 lock 字段,放行
        let body = serde_json::json!({
            "llm": {"model": "deepseek-v4-flash"},
            "theme": "dark",
        });
        assert!(check_settings_locks(&body, &paid_locks()).is_none());
    }

    #[test]
    fn paid_user_patch_with_model_plus_endpoint_still_locks() {
        // 即便混入了允许的 model,只要触碰任一 locked sub-field,就拒绝
        let body = serde_json::json!({"llm": {"model": "x", "endpoint": "https://leaky"}});
        assert!(check_settings_locks(&body, &paid_locks()).is_some());
    }

    #[test]
    fn paid_user_llm_non_object_body_rejected() {
        // 防御: body.llm 不是 object → 保守拒绝(否则可绕过 sub-field 检查)
        let body = serde_json::json!({"llm": "garbage"});
        assert!(check_settings_locks(&body, &paid_locks()).is_some());
    }

    #[test]
    fn free_user_can_change_anything_in_llm() {
        // 免费用户没有 cloud_llm 锁,endpoint/api_key/model 都可改
        for k in ["model", "endpoint", "api_key", "provider"] {
            let body = serde_json::json!({"llm": {k: "anything"}});
            assert!(check_settings_locks(&body, &free_locks()).is_none(),
                "free user should change llm.{}", k);
        }
    }

    #[test]
    fn paid_user_pluginhub_still_whole_object_lock_when_locked() {
        // pluginhub 是整对象锁;但 P0 fix 后付费会员 plugin_install=Editable
        // → 这里应该放行 (per member_session.rs::paid_locks_cloud_llm_and_plugin_uninstall_only)
        let body = serde_json::json!({"pluginhub": {"url": "https://hub.attune.ai"}});
        assert!(check_settings_locks(&body, &paid_locks()).is_none(),
            "付费用户应能改 pluginhub URL(plugin_install 解锁)");
    }
}
