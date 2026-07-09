//! LLM 运维端点 —— 为 Wizard / Settings 提供的 utility 路由
//!
//! - `POST /api/v1/llm/test`：测试云端 LLM 连接（ping 一次，验证 endpoint + api_key + model）
//! - `POST /api/v1/models/pull`：legacy compatibility endpoint; local model lifecycle is scheduler-owned.
//!
//! 见 spec `2026-04-19-frontend-redesign-design.md §6`。

use std::collections::HashSet;
use std::net::IpAddr;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::state::SharedState;
use attune_core::edge_cloud::capacity::DEFAULT_SCHEDULER_BASE;
use attune_core::llm::{ChatMessage, LlmProvider, OpenAiLlmProvider};
use attune_core::llm_settings::SETTINGS_META_KEY;
use attune_core::outbound_gate::{OutboundGate, OutboundKind, OutboundPolicy};
use attune_core::vault::VaultState;

type ApiError = (StatusCode, Json<serde_json::Value>);
const LOCAL_SCHEDULER_PORT: u16 = 8090;

// ── POST /api/v1/llm/test ────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct LlmTestRequest {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
}

#[derive(Serialize)]
pub struct LlmTestResponse {
    pub ok: bool,
    pub latency_ms: Option<u64>,
    pub reply: Option<String>,
    pub error: Option<String>,
}

pub async fn test_llm(
    State(state): State<SharedState>,
    Json(body): Json<LlmTestRequest>,
) -> Result<Json<LlmTestResponse>, ApiError> {
    // 输入校验（防 javascript: 注入到"endpoint"）
    let ep = body.endpoint.trim();
    if !(ep.starts_with("http://") || ep.starts_with("https://")) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "endpoint must start with http(s)://"})),
        ));
    }
    if body.model.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "model required"})),
        ));
    }
    if is_local_probe_target(ep) && !is_scheduler_endpoint(ep) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "local LLM endpoints must be reached through the local scheduler (:8090/v1)",
            })),
        ));
    }

    let api_key = if body.api_key.trim().is_empty() {
        stored_llm_api_key(&state).unwrap_or_default()
    } else {
        body.api_key.trim().to_string()
    };
    let provider = OpenAiLlmProvider::new(ep, &api_key, body.model.trim());
    let messages = vec![ChatMessage::user("ping")];

    let start = std::time::Instant::now();

    // 阻塞 LLM 调用通过 spawn_blocking 跑
    let result = tokio::task::spawn_blocking(move || provider.chat_with_history(&messages))
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("task join: {e}")})),
            )
        })?;

    let latency_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok((reply, _usage)) => Ok(Json(LlmTestResponse {
            ok: true,
            latency_ms: Some(latency_ms),
            reply: Some(reply.chars().take(100).collect()),
            error: None,
        })),
        Err(e) => Ok(Json(LlmTestResponse {
            ok: false,
            latency_ms: Some(latency_ms),
            reply: None,
            error: Some(e.to_string()),
        })),
    }
}

fn stored_llm_api_key(state: &SharedState) -> Option<String> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let raw = vault.store().get_meta(SETTINGS_META_KEY).ok().flatten()?;
    let json: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    json.get("llm")
        .and_then(|llm| llm.get("api_key"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

// ── POST /api/v1/llm/probe-local-scheduler ─────────────────────────────────

#[derive(Deserialize)]
pub struct ProbeLocalSchedulerRequest {
    pub endpoints: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct ProbeLocalSchedulerResponse {
    pub found: bool,
    pub endpoint: Option<String>,
    pub checked: Vec<String>,
}

pub async fn probe_local_scheduler(
    State(state): State<SharedState>,
    Json(body): Json<ProbeLocalSchedulerRequest>,
) -> Result<Json<ProbeLocalSchedulerResponse>, ApiError> {
    let mut candidates = Vec::new();
    let mut dedup = HashSet::new();

    // 1) 用户显式传入的地址优先探测
    for raw in body.endpoints.unwrap_or_default() {
        if let Some(ep) = normalize_probe_endpoint(&raw) {
            if dedup.insert(ep.clone()) {
                candidates.push(ep);
            }
        }
    }

    // 2) 本机回环兜底。:8090 = local-scheduler 统一收口口(OpenAI-compatible)。
    for ep in [
        "http://127.0.0.1:8090/v1",
        "http://localhost:8090/v1",
    ] {
        let ep = ep.to_string();
        if dedup.insert(ep.clone()) {
            candidates.push(ep);
        }
    }

    // 3) 动态读取本机私有网段并扫描
    for ep in discover_local_subnet_candidates() {
        if dedup.insert(ep.clone()) {
            candidates.push(ep);
        }
    }

    // R1.1b: the loopback + discovered-subnet candidates are local destinations
    // (loopback / RFC1918) — no egress, no gate needed. But user-supplied
    // candidates (1) accept ANY http(s) URL, i.e. a non-local probe path. Those
    // go through the OutboundGate (kind=Llm — it's an LLM-endpoint probe) and
    // are silently dropped (graceful: local probing continues) when the gate
    // refuses. Probe payload is empty (bare GET /models), so no redactor needed.
    let (mut candidates, nonlocal): (Vec<String>, Vec<String>) = candidates
        .into_iter()
        .partition(|ep| is_local_probe_target(ep));
    if !nonlocal.is_empty() {
        let enabled =
            super::chat::read_privacy_outbound_enabled(&state, OutboundKind::Llm.as_str());
        let vault_unlocked = matches!(
            state
                .vault
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .state(),
            VaultState::Unlocked
        );
        let policy = OutboundPolicy::cloud(OutboundKind::Llm, enabled, vault_unlocked, None);
        match OutboundGate::enforce(&policy, "") {
            Ok(_) => candidates.extend(nonlocal),
            Err(e) => tracing::info!(
                target: "outbound_audit",
                "R1.1b: probe-local-scheduler dropped {} non-local candidate(s) — outbound gate refused: {e}",
                nonlocal.len()
            ),
        }
    }

    let checked = candidates.clone();
    if candidates.is_empty() {
        return Ok(Json(ProbeLocalSchedulerResponse {
            found: false,
            endpoint: None,
            checked,
        }));
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(350))
        .build()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("probe client init failed: {e}")})),
            )
        })?;

    let mut set = tokio::task::JoinSet::new();
    for endpoint in &candidates {
        let ep = endpoint.clone();
        let client = client.clone();
        set.spawn(async move {
            let ok = probe_openai_compat_models(&client, &ep).await;
            (ep, ok)
        });
    }

    while let Some(joined) = set.join_next().await {
        if let Ok((ep, ok)) = joined {
            if ok {
                set.abort_all();
                return Ok(Json(ProbeLocalSchedulerResponse {
                    found: true,
                    endpoint: Some(ep),
                    checked,
                }));
            }
        }
    }

    Ok(Json(ProbeLocalSchedulerResponse {
        found: false,
        endpoint: None,
        checked,
    }))
}

fn normalize_probe_endpoint(input: &str) -> Option<String> {
    let mut ep = input.trim().trim_end_matches('/').to_string();
    if !(ep.starts_with("http://") || ep.starts_with("https://")) {
        return None;
    }
    if !ep.ends_with("/v1") {
        ep.push_str("/v1");
    }
    Some(ep)
}

/// R1.1b — classify a probe candidate URL as a **local destination** (no egress):
/// host `localhost`, or an IP literal that is loopback / RFC1918 private /
/// link-local (IPv4), or IPv6 loopback. Everything else — public IPs and ALL
/// named hosts (a name can resolve anywhere, fail closed) — is non-local and
/// must pass the OutboundGate before being probed.
fn is_local_probe_target(ep: &str) -> bool {
    let rest = ep
        .strip_prefix("http://")
        .or_else(|| ep.strip_prefix("https://"))
        .unwrap_or(ep);
    let authority = rest.split('/').next().unwrap_or("");
    // strip port; tolerate bracketed IPv6 (`[::1]:8090`)
    let host = if let Some(h) = authority.strip_prefix('[') {
        h.split(']').next().unwrap_or("")
    } else {
        authority
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(authority)
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        Ok(IpAddr::V6(v6)) => v6.is_loopback(),
        Err(_) => false,
    }
}

fn discover_local_subnet_candidates() -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    let ifaces = match local_ip_address::list_afinet_netifas() {
        Ok(m) => m,
        Err(_) => return out,
    };

    for (_name, ip) in ifaces {
        let IpAddr::V4(v4) = ip else {
            continue;
        };
        if !v4.is_private() || v4.is_loopback() || v4.is_link_local() {
            continue;
        }

        let oct = v4.octets();
        let my_host = oct[3];
        // 本地调度器默认在 LAN 上对外收口 :8090。
        for host in 1u8..=254u8 {
            if host == my_host {
                continue;
            }
            for port in [8090u16] {
                let ep = format!(
                    "http://{}.{}.{}.{}:{}/v1",
                    oct[0], oct[1], oct[2], host, port
                );
                if seen.insert(ep.clone()) {
                    out.push(ep);
                }
            }
        }
    }

    out
}

fn is_scheduler_endpoint(ep: &str) -> bool {
    url::Url::parse(ep)
        .ok()
        .and_then(|u| u.port_or_known_default())
        .is_some_and(|port| port == LOCAL_SCHEDULER_PORT)
}

async fn probe_openai_compat_models(client: &reqwest::Client, endpoint: &str) -> bool {
    let url = format!("{endpoint}/models");
    let res = match client.get(url).send().await {
        Ok(r) => r,
        Err(_) => return false,
    };

    if !res.status().is_success() {
        return false;
    }

    let value = match res.json::<serde_json::Value>().await {
        Ok(v) => v,
        Err(_) => return false,
    };

    value
        .get("data")
        .and_then(|v| v.as_array())
        .map(|arr| !arr.is_empty())
        .unwrap_or(false)
}

// ── POST /api/v1/models/pull ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ModelPullRequest {
    pub model: String,
}

#[derive(Serialize)]
pub struct ModelPullResponse {
    pub task_id: String,
    pub status: String,
}

pub async fn pull_model(
    State(_state): State<SharedState>,
    Json(body): Json<ModelPullRequest>,
) -> Result<Json<ModelPullResponse>, ApiError> {
    let model = body.model.trim().to_string();
    if model.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "model required"})),
        ));
    }
    // 基本校验防止 shell 注入（只允许常见模型名字符）
    if !model
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || ":-.".contains(c))
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid model name"})),
        ));
    }

    tracing::info!(
        "legacy model pull request accepted as scheduler-managed: model={model}"
    );
    Ok(Json(ModelPullResponse {
        task_id: format!("scheduler-managed-{}", uuid::Uuid::new_v4()),
        status: "scheduler-managed".to_string(),
    }))
}

// ── GET /api/v1/local-scheduler/readiness?model=<chat_model> ────────────────
//
// Local model lifecycle is scheduler-owned; this route does not probe concrete
// workers directly.

#[derive(Deserialize)]
pub struct ReadinessQuery {
    /// 要核对的 chat 模型；缺省时只判断 scheduler 是否在。
    pub model: Option<String>,
}

pub async fn local_scheduler_readiness(
    axum::extract::Query(q): axum::extract::Query<ReadinessQuery>,
) -> Json<serde_json::Value> {
    let configured = q.model.unwrap_or_default().trim().to_string();
    let resolved = if configured.is_empty() {
        "local-scheduler".to_string()
    } else {
        configured.clone()
    };
    let models = if configured.is_empty() {
        Vec::new()
    } else {
        vec![configured]
    };
    Json(serde_json::json!({
        "readiness": {
            "state": "ready",
            "resolved": resolved,
        },
        "models": models,
        "install_plan": scheduler_managed_install_plan(),
        "scheduler": {
            "managed": true,
            "endpoint": format!("{DEFAULT_SCHEDULER_BASE}/v1"),
        },
    }))
}

// ── POST /api/v1/local-scheduler/ensure ─────────────────────────────────────
//
// Attune no longer installs local model runtimes directly; scheduler owns
// install/update/startup on each supported platform.

#[derive(Serialize)]
pub struct InstallResponse {
    /// scheduler-managed / manual / busy.
    pub status: String,
    pub task_id: Option<String>,
    /// scheduler-managed responses leave it empty.
    pub download_url: Option<String>,
    /// 用户友好提示 (§4.5 可操作错误信息)。
    pub message: String,
}

pub async fn ensure_local_scheduler() -> Result<Json<InstallResponse>, ApiError> {
    Ok(Json(InstallResponse {
        status: "scheduler-managed".into(),
        task_id: None,
        download_url: None,
        message: format!(
            "本地模型生命周期由 local scheduler 管理，请通过 {} 检查 scheduler 状态",
            DEFAULT_SCHEDULER_BASE
        ),
    }))
}

fn scheduler_managed_install_plan() -> serde_json::Value {
    serde_json::json!({
        "platform": "scheduler",
        "method": {
            "kind": "manual_download",
            "download_url": DEFAULT_SCHEDULER_BASE,
        },
        "homepage": DEFAULT_SCHEDULER_BASE,
    })
}

// ─── 单元测试 (覆盖纯函数: normalize_probe_endpoint, model name validation) ────
#[cfg(test)]
mod tests {
    use super::*;

    // normalize_probe_endpoint: 已有 http:// → 加 /v1
    #[test]
    fn normalize_adds_v1_suffix() {
        assert_eq!(
            normalize_probe_endpoint("http://192.168.1.10:8090"),
            Some("http://192.168.1.10:8090/v1".into())
        );
    }

    // 已有 /v1 → 不重复加
    #[test]
    fn normalize_keeps_existing_v1() {
        assert_eq!(
            normalize_probe_endpoint("http://192.168.1.10:8090/v1"),
            Some("http://192.168.1.10:8090/v1".into())
        );
    }

    // https:// → 同样加
    #[test]
    fn normalize_https_with_v1() {
        assert_eq!(
            normalize_probe_endpoint("https://api.example.com"),
            Some("https://api.example.com/v1".into())
        );
    }

    // trailing / 应被 strip
    #[test]
    fn normalize_strips_trailing_slash() {
        assert_eq!(
            normalize_probe_endpoint("http://host:8090/"),
            Some("http://host:8090/v1".into())
        );
    }

    // trim whitespace
    #[test]
    fn normalize_trims_whitespace() {
        assert_eq!(
            normalize_probe_endpoint("  http://host  "),
            Some("http://host/v1".into())
        );
    }

    // Adversarial: 非 http(s) 协议 (javascript:, file:, ftp:) → None
    #[test]
    fn normalize_rejects_javascript_protocol() {
        assert_eq!(normalize_probe_endpoint("javascript:alert(1)"), None);
    }

    #[test]
    fn normalize_rejects_file_protocol() {
        assert_eq!(normalize_probe_endpoint("file:///etc/passwd"), None);
    }

    #[test]
    fn normalize_rejects_ftp_protocol() {
        assert_eq!(normalize_probe_endpoint("ftp://host/file"), None);
    }

    // Edge: empty string
    #[test]
    fn normalize_empty_returns_none() {
        assert_eq!(normalize_probe_endpoint(""), None);
    }

    // Edge: 仅空白 → None
    #[test]
    fn normalize_whitespace_only_returns_none() {
        assert_eq!(normalize_probe_endpoint("   "), None);
    }

    // discover_local_subnet_candidates: 返回的应都是 /v1 后缀
    #[test]
    fn discover_subnet_endpoints_have_v1_suffix() {
        let out = discover_local_subnet_candidates();
        for ep in &out {
            assert!(ep.ends_with("/v1"), "{ep} should end with /v1");
            assert!(ep.starts_with("http://"), "{ep} should be http://");
        }
    }

    // Adversarial: 模型名校验 (与 pull_model 内同一规则)
    // 这里测试该规则的边界 — invalid chars 应被拒
    #[test]
    fn model_name_validation_rejects_shell_injection() {
        let invalid_names = [
            "model;rm -rf /",
            "../etc/passwd",
            "model && cat",
            "model$(whoami)",
            "model`id`",
            "model|cat",
            "model>file",
        ];
        for name in invalid_names {
            let safe = name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ":-.".contains(c));
            assert!(!safe, "{name} should be rejected");
        }
    }

    #[test]
    fn model_name_validation_accepts_common_models() {
        let valid_names = [
            "qwen2.5:3b",
            "bge-m3",
            "llama3.2:1b",
            "deepseek-coder-v2:16b",
            "model-7b-q4_0.gguf",
        ];
        for name in valid_names {
            let safe = name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ":-.".contains(c));
            // _ 是 invalid (per current rule), gguf 后缀 ok 但 _ 不行
            if !name.contains('_') {
                assert!(safe, "{name} should be accepted");
            }
        }
    }

    // Edge: LlmTestRequest validation rules (must start with http(s))
    // model 必填 (non-empty after trim)
    #[test]
    fn llm_test_request_validation_rules() {
        // model trim 后空 → 应拒绝
        let model_with_only_whitespace = "   ";
        assert!(model_with_only_whitespace.trim().is_empty());
        let model_ok = "  gpt-4  ";
        assert_eq!(model_ok.trim(), "gpt-4");
        // endpoint 协议校验
        for bad in ["", "ws://", "javascript:", "ftp://host", "   "] {
            let ep = bad.trim();
            assert!(
                !(ep.starts_with("http://") || ep.starts_with("https://")),
                "{bad} should fail validation"
            );
        }
        for good in ["http://h:8090", "https://api.x.com/v1"] {
            assert!(good.starts_with("http://") || good.starts_with("https://"));
        }
    }

    // R1.1b — probe candidate locality classification (gate boundary)
    #[test]
    fn local_probe_targets_classified_local() {
        for ep in [
            "http://localhost:8090/v1",
            "http://127.0.0.1:8090/v1",
            "http://127.0.0.1/v1",
            "http://[::1]:8090/v1",
            "http://192.168.1.50:8090/v1",
            "http://10.0.0.2:8090/v1",
            "http://172.16.3.4:8090/v1",
            "http://169.254.1.1:8090/v1",
        ] {
            assert!(super::is_local_probe_target(ep), "{ep} should be local");
        }
    }

    #[test]
    fn scheduler_endpoint_requires_scheduler_port() {
        assert!(super::is_scheduler_endpoint("http://127.0.0.1:8090/v1"));
        assert!(super::is_scheduler_endpoint("http://192.168.1.50:8090/v1"));
        assert!(!super::is_scheduler_endpoint("http://127.0.0.1:18080/v1"));
        assert!(!super::is_scheduler_endpoint("https://api.openai.com/v1"));
    }

    #[test]
    fn nonlocal_probe_targets_classified_nonlocal() {
        // Public IPs and named hosts (can resolve anywhere → fail closed) must be
        // gated before probing.
        for ep in [
            "http://8.8.8.8:8090/v1",
            "https://1.2.3.4/v1",
            "http://scheduler.example.com:8090/v1",
            "https://attacker.tld/v1",
            "http://[2001:db8::1]:8090/v1",
        ] {
            assert!(
                !super::is_local_probe_target(ep),
                "{ep} should be non-local"
            );
        }
    }
}
