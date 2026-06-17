use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use crate::error::{AppError, AppResult};
use crate::state::SharedState;
use attune_core::llm_settings::SETTINGS_META_KEY as SETTINGS_KEY;

pub async fn get_settings(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    let recommended_summary = state.hardware.recommended_summary_model();
    let form_factor = state.hardware.form_factor;
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        AppError::Forbidden(e.to_string())
    })?;

    let settings = vault.store().get_meta(SETTINGS_KEY)
        .map_err(|e| AppError::Internal(e.to_string()))?;

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

/// 全字段设置校验:所有设置保存前必须有效,拒绝静默接受无效值(URL scheme / 枚举 / 数值范围)。
/// 返回 Err(用户可读信息);调用方映射为 400。与上方 4 个 ad-hoc 检查(endpoint/browser_path/
/// skills.disabled/ocr.active_profile)互补,这里补齐其余字段。
fn validate_settings_fields(body: &serde_json::Value) -> Result<(), String> {
    let Some(obj) = body.as_object() else { return Ok(()) };
    let nested = |sect: &str, key: &str| -> Option<String> {
        obj.get(sect)?.as_object()?.get(key)?.as_str().map(str::to_string)
    };

    // 1. 所有 URL 字段统一 http/https scheme 校验(对齐 llm.endpoint,防 javascript:/data: + 明显无效)
    for (sect, key, label) in [
        ("embedding", "ollama_url", "embedding.ollama_url"),
        ("pluginhub", "url", "pluginhub.url"),
        ("cloud", "accounts_url", "cloud.accounts_url"),
        ("cloud", "gateway_url", "cloud.gateway_url"),
    ] {
        if let Some(v) = nested(sect, key) {
            if !v.is_empty() && !is_safe_http_url(&v) {
                return Err(format!("{label} 必须是 http:// 或 https:// 开头的有效 URL"));
            }
        }
    }

    // 2. 顶层枚举字段
    for (key, allowed) in [
        ("theme", &["system", "dark", "light"][..]),
        ("language", &["zh-CN", "en", "en-US"][..]),
        ("context_strategy", &["economical", "accurate", "raw"][..]),
        ("injection_mode", &["auto", "manual", "off"][..]),
    ] {
        if let Some(v) = obj.get(key).and_then(|v| v.as_str()) {
            if !allowed.contains(&v) {
                return Err(format!("{key} 取值无效:'{v}'(允许:{})", allowed.join(" / ")));
            }
        }
    }
    if let Some(p) = nested("llm", "provider") {
        const PROVIDERS: &[&str] = &["openai_compat", "anthropic", "deepseek", "qwen", "ollama", "claude", "gemini"];
        if !PROVIDERS.contains(&p.as_str()) {
            return Err(format!("llm.provider 无效:'{p}'(允许:{})", PROVIDERS.join(" / ")));
        }
    }
    if let Some(e) = nested("web_search", "engine") {
        const ENGINES: &[&str] = &["duckduckgo", "bing", "google", "searxng"];
        if !ENGINES.contains(&e.as_str()) {
            return Err(format!("web_search.engine 无效:'{e}'(允许:{})", ENGINES.join(" / ")));
        }
    }

    // 3. 数值范围
    if let Some(b) = obj.get("injection_budget").and_then(serde_json::Value::as_i64) {
        if !(100..=32_768).contains(&b) {
            return Err(format!("injection_budget 须在 100-32768(当前 {b})"));
        }
    }
    if let Some(s) = obj.get("search").and_then(|v| v.as_object()) {
        if let Some(k) = s.get("default_top_k").and_then(serde_json::Value::as_i64) {
            if !(1..=200).contains(&k) {
                return Err(format!("search.default_top_k 须在 1-200(当前 {k})"));
            }
        }
        for w in ["vector_weight", "fulltext_weight"] {
            if let Some(x) = s.get(w).and_then(serde_json::Value::as_f64) {
                if !(0.0..=1.0).contains(&x) {
                    return Err(format!("search.{w} 须在 0.0-1.0(当前 {x})"));
                }
            }
        }
    }
    if let Some(ms) = obj.get("web_search").and_then(|v| v.as_object())
        .and_then(|o| o.get("min_interval_ms")).and_then(serde_json::Value::as_i64)
    {
        if !(0..=600_000).contains(&ms) {
            return Err(format!("web_search.min_interval_ms 须在 0-600000(当前 {ms})"));
        }
    }

    // 4. Trust-chain T11: plugin_trust_mode 三态 + plugin_trusted_pubkeys 64-hex 数组。
    if let Some(m) = obj.get("plugin_trust_mode").and_then(|v| v.as_str()) {
        const MODES: &[&str] = &["off", "warn", "strict"];
        if !MODES.contains(&m) {
            return Err(format!("plugin_trust_mode 无效:'{m}'(允许:{})", MODES.join(" / ")));
        }
    }
    if let Some(keys) = obj.get("plugin_trusted_pubkeys") {
        let arr = keys
            .as_array()
            .ok_or_else(|| "plugin_trusted_pubkeys 必须是字符串数组".to_string())?;
        for (i, k) in arr.iter().enumerate() {
            let s = k
                .as_str()
                .ok_or_else(|| format!("plugin_trusted_pubkeys[{i}] 必须是字符串"))?;
            // Ed25519 公钥 = 32 字节 → 64 hex 字符。拒绝非 64-hex(防注入垃圾)。
            if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!("plugin_trusted_pubkeys[{i}] 必须是 64 位 hex 公钥"));
            }
        }
    }

    Ok(())
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
) -> AppResult<Json<serde_json::Value>> {
    let recommended_summary = state.hardware.recommended_summary_model();
    let form_factor = state.hardware.form_factor;
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        AppError::Forbidden(e.to_string())
    })?;

    // Merge with existing settings
    let existing = vault.store().get_meta(SETTINGS_KEY)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let mut current: serde_json::Value = match existing {
        Some(data) => serde_json::from_slice(&data)
            .unwrap_or_else(|_| default_settings(recommended_summary, form_factor)),
        None => default_settings(recommended_summary, form_factor),
    };

    // 白名单校验：只允许写入已知配置键，防止任意键污染 vault_meta
    const ALLOWED_KEYS: &[&str] = &[
        "injection_mode", "injection_budget", "excluded_domains",
        "search", "embedding", "web_search", "llm",
        "summary_model", "summary", "context_strategy", "theme", "language",
        "skills",  // Sprint 2 Skills Router: { disabled: string[] }
        "wizard",  // wizard completion state: { complete: bool, current_step: int }
        "pluginhub", // G2 (2026-05-01): { url, license_key }
        "cloud", // FEAT-1 (2026-05-14): { accounts_url } — 自部署 / 私有 cloud 环境覆盖默认 engi-stack.com
        "privacy", // v1.0.6 Privacy Logic Strategy: { llm, cloud_saas, webdav, web_search, telemetry, privacy_tour_seen }
        "plugin_trust_mode", // Trust-chain T11: "off" | "warn" | "strict" (default warn)
        "plugin_trusted_pubkeys", // Trust-chain T11: user-whitelisted third-party signer pubkeys (64-hex[])
    ];

    // v1.0.6 Privacy Logic: telemetry 必须通过 isolation patch 切换,
    // 不允许搭车其他 settings update (防 buggy UI / 第三方 plugin piggyback)
    if !is_telemetry_path_allowed(&body) {
        return Err(AppError::BadRequest("telemetry-must-be-isolated".into()));
    }
    // URL 字段白名单 scheme 校验（防 javascript: / data: 注入成 XSS 种子）
    if let Some(body_obj) = body.as_object() {
        if let Some(llm_obj) = body_obj.get("llm").and_then(|v| v.as_object()) {
            if let Some(ep) = llm_obj.get("endpoint").and_then(|v| v.as_str()) {
                if !ep.is_empty() && !is_safe_http_url(ep) {
                    return Err(AppError::BadRequest(
                        "llm.endpoint must be http:// or https:// URL".into(),
                    ));
                }
            }
        }
        if let Some(ws_obj) = body_obj.get("web_search").and_then(|v| v.as_object()) {
            if let Some(bp) = ws_obj.get("browser_path").and_then(|v| v.as_str()) {
                // 浏览器路径是文件路径，不是 URL；但不允许以 - 开头（防 argv 注入）
                if bp.starts_with('-') {
                    return Err(AppError::BadRequest(
                        "web_search.browser_path cannot start with '-' (argv injection risk)".into(),
                    ));
                }
            }
        }
        // 本地模型一键化 (2026-06-01): summary 模式枚举校验。
        // off  = 纯检索，不跑文档/上下文摘要 (弱机 / 离线默认)
        // local= 用本地 summary_model (Ollama)
        // cloud= 复用 chat LLM (远端 token)
        if let Some(summary) = body_obj.get("summary").and_then(|v| v.as_str()) {
            const SUMMARY_MODES: &[&str] = &["off", "local", "cloud"];
            if !SUMMARY_MODES.contains(&summary) {
                return Err(AppError::BadRequest(
                    "summary must be one of: off / local / cloud".into(),
                ));
            }
        }
        // Sprint 2 Skills Router: 校验 skills.disabled 必须是 string[]
        if let Some(skills_obj) = body_obj.get("skills").and_then(|v| v.as_object()) {
            if let Some(d) = skills_obj.get("disabled") {
                let arr_ok = d.as_array().map(|arr| arr.iter().all(|x| x.is_string())).unwrap_or(false);
                if !arr_ok {
                    return Err(AppError::BadRequest(
                        "skills.disabled must be an array of strings".into(),
                    ));
                }
            }
        }
        // 校验 ocr.active_profile 必须是已存在的 profile id (避免用户输错导致 OCR 回退)
        if let Some(ocr_obj) = body_obj.get("ocr").and_then(|v| v.as_object()) {
            if let Some(prof) = ocr_obj.get("active_profile").and_then(|v| v.as_str()) {
                let reg = attune_core::ocr::profile_registry::ProfileRegistry::load_default()
                    .map_err(|e| AppError::Internal(e.to_string()))?;
                if reg.get(prof).is_none() {
                    return Err(AppError::BadRequest(format!(
                        "ocr.active_profile '{prof}' 不存在 (用 GET /api/v1/ocr/profiles 查看可用 id)"
                    )));
                }
            }
        }
    }

    // 全字段校验:所有设置保存前必须有效(URL scheme / 枚举 / 数值范围),拒绝静默接受无效值。
    if let Err(msg) = validate_settings_fields(&body) {
        return Err(AppError::detailed(StatusCode::BAD_REQUEST, serde_json::json!({
            "error": msg, "code": "invalid-setting"
        })));
    }

    // SettingsLocks enforce — 会员锁定字段拒绝更新.
    let member_state = state.member_state.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let locks = attune_core::member_session::SettingsLocks::for_state(&member_state);
    if let Some(violation) = check_settings_locks(&body, &locks) {
        return Err(AppError::detailed(
            StatusCode::FORBIDDEN,
            serde_json::json!({
                "error": "setting_locked_by_member_tier",
                "field": violation.settings_key,
                "lock_reason": format!("'{}' is locked under current membership tier", violation.lock_field),
                "hint": "请升级会员或在「设置 → 会员」查看锁定矩阵",
            }),
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
        .map_err(|e| AppError::Internal(e.to_string()))?;
    vault.store().set_meta(SETTINGS_KEY, &data)
        .map_err(|e| AppError::Internal(e.to_string()))?;

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
        // 本地模型一键化 (2026-06-01): 摘要模式 off / local / cloud。
        // K3 一体机预装本地模型 → local；其他形态 LLM 默认走远端 token → 摘要也复用云端
        // (cloud) 避免要求笔电先装 Ollama 才能用摘要。弱机用户可在 Settings 改 off 纯检索。
        "summary": if form_factor == FormFactor::K3Appliance { "local" } else { "cloud" },
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

        // Trust-chain T11 (spec §10 决策 2): 插件签名信任门三态 + 用户白名单公钥。
        // 默认 warn(升级 grandfather:未签名加载+警示,篡改拒载)。官方公钥是编译期
        // const(plugin_anchor::OFFICIAL_PLUGIN_ANCHORS),settings 不可覆盖(防降级攻击)。
        "plugin_trust_mode": "warn",
        "plugin_trusted_pubkeys": [],

        // G2 (2026-05-01) — PluginHub 远端市场对接
        // null = 走内嵌 Mock provider（默认离线，看到 4 个 attune-pro 试用卡）
        // 配 url + license_key 后切到 HttpPluginHubProvider，调真 hub.engi-stack.com
        "pluginhub": {
            "url": null,                  // 例: "https://hub.engi-stack.com"
            "license_key": null           // 同 attune Pro 会员 license key（与 LLM Gateway 共享）
        },

        // FEAT-1 (2026-05-14) — 自部署 cloud cluster 入口
        // null = 默认 engi-stack.com 公共 cloud (accounts.engi-stack.com / hub.engi-stack.com / gateway.engi-stack.com)
        // 自部署: 填入私有 cluster URL, 三个 endpoint 分别对应不同微服务.
        // 用户场景: 企业内网部署 attune-cloud-* 容器后, 在 Settings UI 填入这三个地址
        "cloud": {
            "accounts_url": null,         // 例: "https://accounts.your-company.com" (member login / license)
            "gateway_url": null,          // 例: "https://gateway.your-company.com" (LLM token gateway)
                                          // pluginhub URL 仍走上方 pluginhub.url (历史命名保留)
        },

        // ── v1.0.6 Privacy Logic Strategy (per docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md) ──
        // 5 个出网点 + privacy tour seen flag,**所有出网点默认 false**:
        //   - llm: wizard step 引导用户主动开
        //   - cloud_saas: 登录 Attune Pro 后开
        //   - webdav: 用户自行配置后开
        //   - web_search: 用户在 Settings 开关后开
        //   - telemetry: **永远默认关 + 必须 opt-in**,绝不自动启用(per spec §4.2 #⑤)
        // PATCH /privacy/settings 是切换唯一入口;telemetry 必须走 isolated patch (见 is_telemetry_path_allowed)
        "privacy": {
            "llm": false,
            "cloud_saas": false,
            "webdav": false,
            "web_search": false,
            "telemetry": false,
            "privacy_tour_seen": false
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

/// Telemetry MUST only be toggled through a patch whose ONLY top-level keys are
/// "privacy" (or "privacy_tour_seen" piggyback inside privacy). Mixed patches
/// are rejected so a buggy UI or third-party plugin cannot piggyback
/// `privacy.telemetry=true` on an unrelated settings update.
///
/// per spec `docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md` §4.2 #⑤.
pub fn is_telemetry_path_allowed(body: &serde_json::Value) -> bool {
    let Some(obj) = body.as_object() else { return true };
    let touches_telemetry = obj
        .get("privacy")
        .and_then(|p| p.as_object())
        .map(|p| p.contains_key("telemetry"))
        .unwrap_or(false);
    if !touches_telemetry {
        return true;
    }
    // 仅当本次 patch 唯一顶级 key 是 "privacy" 时允许触碰 telemetry。
    obj.keys().all(|k| k == "privacy")
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_core::platform::FormFactor;

    // ── Trust-chain T11: trust_mode + pubkey whitelist ──────────────────────

    /// Fresh vault default `plugin_trust_mode` = "warn" (决策 2 + spec §10 grandfather).
    #[test]
    fn settings_trust_mode_default_warn() {
        let d = default_settings("qwen2.5:3b", FormFactor::Laptop);
        assert_eq!(
            d.get("plugin_trust_mode").and_then(|v| v.as_str()),
            Some("warn"),
            "fresh vault must default plugin_trust_mode to warn"
        );
        assert_eq!(
            d.get("plugin_trusted_pubkeys").and_then(|v| v.as_array()).map(|a| a.len()),
            Some(0),
            "default whitelist is empty"
        );
        // The default value is itself an accepted settings shape.
        assert!(validate_settings_fields(&d).is_ok());
    }

    /// Settings can NOT inject an "official" trust root: the official anchor is the
    /// compile-time `plugin_anchor::OFFICIAL_PLUGIN_ANCHORS` const, and a
    /// `plugin_trusted_pubkeys` entry only ever yields `Trust::ThirdParty` via
    /// `verify_with_whitelist` — never `Official` (§9 adversarial 2, defends downgrade).
    #[test]
    fn settings_pubkey_cannot_override_official() {
        use attune_core::plugin_sig::{verify_with_whitelist, SigOutcome};
        // A user-whitelisted key signs a plugin; the SigOutcome must be ThirdParty,
        // not Official — settings pubkeys are a separate, lower trust domain.
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("p");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.yaml"), "id: p\nname: P\ntype: industry\nversion: \"1.0.0\"\n").unwrap();
        // generate a fresh signer + sign the plugin dir contents.
        let sk = attune_core::plugin_sig::generate_signing_key();
        attune_core::plugin_sig::sign_plugin(&dir, &sk).expect("sign");
        let pubkey_hex = attune_core::plugin_sig::derive_verifying_key_hex(&sk);
        // official_keys = the production anchor const (NOT the user key).
        let official: Vec<&str> = attune_core::plugin_anchor::OFFICIAL_PLUGIN_ANCHORS.to_vec();
        let user_keys = vec![pubkey_hex];
        let outcome = verify_with_whitelist(&dir, &official, &user_keys).unwrap();
        assert_eq!(
            outcome,
            SigOutcome::ThirdParty,
            "a settings-whitelisted pubkey yields ThirdParty, never Official"
        );
        // And: there is no settings key that mutates the official anchor const — the
        // settings schema only carries plugin_trusted_pubkeys (third-party domain).
        let d = default_settings("qwen2.5:3b", FormFactor::Laptop);
        assert!(d.get("official_pubkeys").is_none(), "no settings path to official anchor");
    }

    /// plugin_trusted_pubkeys validates as a 64-hex array and round-trips through the
    /// settings JSON (PATCH merge → GET). Non-hex / wrong-length entries are rejected.
    #[test]
    fn trusted_pubkeys_roundtrip() {
        let valid = "ab".repeat(32); // 64 hex chars
        let body = serde_json::json!({
            "plugin_trust_mode": "strict",
            "plugin_trusted_pubkeys": [valid.clone()],
        });
        assert!(validate_settings_fields(&body).is_ok(), "valid 64-hex pubkey accepted");
        // round-trip: the array survives a clone through the settings value shape.
        assert_eq!(body["plugin_trusted_pubkeys"][0], serde_json::Value::String(valid));
        assert_eq!(body["plugin_trust_mode"], "strict");

        // reject malformed: not 64-hex, non-string, bad mode.
        for bad in [
            serde_json::json!({"plugin_trusted_pubkeys": ["xyz"]}),
            serde_json::json!({"plugin_trusted_pubkeys": ["ab".repeat(33)]}),
            serde_json::json!({"plugin_trusted_pubkeys": [123]}),
            serde_json::json!({"plugin_trusted_pubkeys": "not-an-array"}),
            serde_json::json!({"plugin_trust_mode": "paranoid"}),
        ] {
            assert!(validate_settings_fields(&bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn validate_settings_fields_rejects_invalid_and_accepts_valid() {
        // 有效:全通过
        assert!(validate_settings_fields(&serde_json::json!({
            "theme": "dark", "language": "en", "context_strategy": "accurate",
            "injection_mode": "auto", "injection_budget": 2000,
            "embedding": {"ollama_url": "http://localhost:11434"},
            "cloud": {"accounts_url": "https://a.example.com", "gateway_url": "https://g.example.com"},
            "pluginhub": {"url": "https://hub.example.com"},
            "llm": {"provider": "deepseek"},
            "web_search": {"engine": "duckduckgo", "min_interval_ms": 2000},
            "search": {"default_top_k": 10, "vector_weight": 0.6, "fulltext_weight": 0.4}
        })).is_ok());

        // 无效 URL(各 url 字段)
        for bad in [
            serde_json::json!({"embedding": {"ollama_url": "javascript:alert(1)"}}),
            serde_json::json!({"pluginhub": {"url": "ftp://x"}}),
            serde_json::json!({"cloud": {"accounts_url": "not-a-url"}}),
            serde_json::json!({"cloud": {"gateway_url": "data:text/html,x"}}),
        ] {
            assert!(validate_settings_fields(&bad).is_err(), "should reject {bad:?}");
        }
        // 无效枚举 + 越界
        for bad in [
            serde_json::json!({"theme": "neon"}),
            serde_json::json!({"language": "ja-JP"}),
            serde_json::json!({"context_strategy": "turbo"}),
            serde_json::json!({"injection_mode": "always"}),
            serde_json::json!({"llm": {"provider": "skynet"}}),
            serde_json::json!({"web_search": {"engine": "altavista"}}),
            serde_json::json!({"injection_budget": 50}),
            serde_json::json!({"injection_budget": 99999}),
            serde_json::json!({"search": {"default_top_k": 0}}),
            serde_json::json!({"search": {"vector_weight": 1.5}}),
            serde_json::json!({"web_search": {"min_interval_ms": -1}}),
        ] {
            assert!(validate_settings_fields(&bad).is_err(), "should reject {bad:?}");
        }
        // 空 URL 视为"清空",允许
        assert!(validate_settings_fields(&serde_json::json!({"cloud": {"accounts_url": ""}})).is_ok());
    }

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

    /// 本地模型一键化 (2026-06-01): summary 默认值随形态分裂。
    /// K3 一体机预装本地模型 → "local"；其他形态 LLM 走远端 → "cloud"
    /// (不强制笔电先装 Ollama 才能用摘要)。
    #[test]
    fn summary_default_splits_by_form_factor() {
        assert_eq!(
            default_settings("qwen2.5:3b", FormFactor::K3Appliance)
                .get("summary").and_then(|v| v.as_str()),
            Some("local"),
            "K3 预装本地模型 → summary=local"
        );
        for ff in [FormFactor::Laptop, FormFactor::Server, FormFactor::Unknown] {
            assert_eq!(
                default_settings("qwen2.5:3b", ff)
                    .get("summary").and_then(|v| v.as_str()),
                Some("cloud"),
                "FormFactor::{ff:?} LLM 默认远端 → summary=cloud"
            );
        }
    }

    /// summary 默认值必须落在合法枚举内 (off/local/cloud)。
    #[test]
    fn summary_default_is_valid_enum() {
        for ff in [
            FormFactor::Laptop, FormFactor::Server,
            FormFactor::Unknown, FormFactor::K3Appliance,
        ] {
            let v = default_settings("qwen2.5:3b", ff);
            let summary = v.get("summary").and_then(|x| x.as_str()).unwrap_or("");
            assert!(
                ["off", "local", "cloud"].contains(&summary),
                "summary '{summary}' must be a valid enum value"
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
        let body = serde_json::json!({"pluginhub": {"url": "https://hub.engi-stack.com"}});
        assert!(check_settings_locks(&body, &paid_locks()).is_none(),
            "付费用户应能改 pluginhub URL(plugin_install 解锁)");
    }

    // ── v1.0.6 Privacy Logic Strategy — default-false block + telemetry isolation ──

    /// 默认 settings 必须有 privacy block,且 5 个出网点全 false + privacy_tour_seen=false.
    /// per spec §4.2 5 个出网点默认全关。
    #[test]
    fn default_settings_has_privacy_block_all_outbound_disabled_except_llm_off_by_default() {
        let settings = default_settings("", FormFactor::Laptop);
        let privacy = settings.get("privacy").expect("settings should contain privacy block");
        assert_eq!(privacy.get("telemetry"), Some(&serde_json::json!(false)),
            "telemetry MUST default to false");
        assert_eq!(privacy.get("web_search"), Some(&serde_json::json!(false)),
            "web_search MUST default to false");
        assert_eq!(privacy.get("cloud_saas"), Some(&serde_json::json!(false)),
            "cloud_saas MUST default to false (login required to enable)");
        assert_eq!(privacy.get("webdav"), Some(&serde_json::json!(false)),
            "webdav MUST default to false (user configures explicitly)");
        assert_eq!(privacy.get("llm"), Some(&serde_json::json!(false)),
            "llm MUST default to false (wizard step enables it)");
        assert_eq!(privacy.get("privacy_tour_seen"), Some(&serde_json::json!(false)));
    }

    /// privacy block 在 K3 / Server / Unknown 形态下也必须全 false(form_factor 不影响 privacy).
    #[test]
    fn default_settings_privacy_block_invariant_across_form_factors() {
        for ff in [FormFactor::Laptop, FormFactor::Server, FormFactor::Unknown, FormFactor::K3Appliance] {
            let s = default_settings("qwen2.5:3b", ff);
            let privacy = s.get("privacy").unwrap_or_else(|| panic!("privacy missing for {ff:?}"));
            for key in &["llm", "cloud_saas", "webdav", "web_search", "telemetry"] {
                assert_eq!(privacy.get(*key), Some(&serde_json::json!(false)),
                    "{ff:?}: privacy.{key} must default false");
            }
        }
    }

    /// telemetry 只能通过纯 privacy patch 切换 — 混合 patch (privacy.telemetry + llm) 拒绝.
    /// per spec §4.2 #⑤ telemetry 永远 opt-in,不可 piggyback.
    #[test]
    fn telemetry_only_togglable_through_explicit_privacy_patch() {
        // 1. 非 privacy patch → allowed (无 telemetry 触碰)
        let llm_only = serde_json::json!({ "llm": { "model": "deepseek-v4-pro" } });
        assert!(is_telemetry_path_allowed(&llm_only),
            "non-privacy patches must be allowed when they don't touch telemetry");

        // 2. 纯 privacy patch 切 telemetry → allowed
        let privacy_only = serde_json::json!({ "privacy": { "telemetry": true } });
        assert!(is_telemetry_path_allowed(&privacy_only),
            "isolated privacy patch toggling telemetry must be allowed");

        // 3. privacy.telemetry + llm 混合 → rejected
        let mixed = serde_json::json!({ "privacy": { "telemetry": true }, "llm": { "model": "x" } });
        assert!(!is_telemetry_path_allowed(&mixed),
            "mixed patch with telemetry must be rejected to prevent accidental enabling");

        // 4. privacy.telemetry + theme → rejected
        let mixed_theme = serde_json::json!({ "privacy": { "telemetry": false }, "theme": "dark" });
        assert!(!is_telemetry_path_allowed(&mixed_theme),
            "even disabling telemetry must be isolated to keep audit-log meaningful");

        // 5. privacy 块改其他 key (不含 telemetry) + llm → allowed
        let privacy_non_telemetry_mixed = serde_json::json!({
            "privacy": { "web_search": true },
            "llm": { "model": "x" }
        });
        assert!(is_telemetry_path_allowed(&privacy_non_telemetry_mixed),
            "non-telemetry privacy keys may piggyback (only telemetry is super-protected)");

        // 6. 非 object body → allowed(其他错误会拦截)
        let not_obj = serde_json::json!([1, 2, 3]);
        assert!(is_telemetry_path_allowed(&not_obj));
    }
}
