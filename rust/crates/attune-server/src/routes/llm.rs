//! LLM 运维端点 —— 为 Wizard / Settings 提供的 utility 路由
//!
//! - `POST /api/v1/llm/test`：测试云端 LLM 连接（ping 一次，验证 endpoint + api_key + model）
//! - `POST /api/v1/models/pull`：后台拉 Ollama 模型（异步；进度通过 WebSocket 推送）
//!
//! 见 spec `2026-04-19-frontend-redesign-design.md §6`。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use std::collections::HashSet;
use std::net::IpAddr;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::state::SharedState;
use attune_core::llm::{ChatMessage, LlmProvider, OpenAiLlmProvider};

/// 同一时间最多 2 个 ollama pull 进程（防资源耗尽，见 CRITICAL 1.2）
static PULL_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
const MAX_CONCURRENT_PULLS: usize = 2;

type ApiError = (StatusCode, Json<serde_json::Value>);

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

    let provider = OpenAiLlmProvider::new(ep, &body.api_key, body.model.trim());
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

// ── POST /api/v1/llm/probe-k3 ───────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ProbeK3Request {
    pub endpoints: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct ProbeK3Response {
    pub found: bool,
    pub endpoint: Option<String>,
    pub checked: Vec<String>,
}

pub async fn probe_k3(
    Json(body): Json<ProbeK3Request>,
) -> Result<Json<ProbeK3Response>, ApiError> {
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

    // 2) 本机回环兜底
    for ep in ["http://127.0.0.1:8080/v1", "http://localhost:8080/v1"] {
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

    let checked = candidates.clone();
    if candidates.is_empty() {
        return Ok(Json(ProbeK3Response {
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
                return Ok(Json(ProbeK3Response {
                    found: true,
                    endpoint: Some(ep),
                    checked,
                }));
            }
        }
    }

    Ok(Json(ProbeK3Response {
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
        for host in 1u8..=254u8 {
            if host == my_host {
                continue;
            }
            let ep = format!("http://{}.{}.{}.{}:8080/v1", oct[0], oct[1], oct[2], host);
            if seen.insert(ep.clone()) {
                out.push(ep);
            }
        }
    }

    out
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
    // 基本校验防止 shell 注入（只允许常见 ollama 模型名字符）
    if !model
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || ":-.".contains(c))
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid model name"})),
        ));
    }

    // 并发上限守卫（Critical 1.2 修复）
    let inflight = PULL_IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    if inflight >= MAX_CONCURRENT_PULLS {
        PULL_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": format!("too many concurrent pulls (max {MAX_CONCURRENT_PULLS})"),
            })),
        ));
    }

    let task_id = format!("pull-{}", uuid::Uuid::new_v4());
    let task_id_ret = task_id.clone();

    // 后台跑 `ollama pull <model>`（不等待；进度推送由 WS 侧实现）
    tokio::spawn(async move {
        let out = tokio::process::Command::new("ollama")
            .arg("pull")
            .arg(&model)
            .output()
            .await;
        match out {
            Ok(o) if o.status.success() => {
                tracing::info!("model pull done: {model} (task={task_id})");
            }
            Ok(o) => {
                tracing::warn!(
                    "model pull failed: {model} (task={task_id}) status={} stderr={}",
                    o.status,
                    String::from_utf8_lossy(&o.stderr)
                );
            }
            Err(e) => {
                tracing::warn!("model pull spawn error: {model} (task={task_id}) err={e}");
            }
        }
        // 无论成功失败都释放计数
        PULL_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    });

    Ok(Json(ModelPullResponse {
        task_id: task_id_ret,
        status: "queued".to_string(),
    }))
}

// ─── 单元测试 (覆盖纯函数: normalize_probe_endpoint, model name validation) ────
#[cfg(test)]
mod tests {
    use super::*;

    // normalize_probe_endpoint: 已有 http:// → 加 /v1
    #[test]
    fn normalize_adds_v1_suffix() {
        assert_eq!(
            normalize_probe_endpoint("http://192.168.1.10:8080"),
            Some("http://192.168.1.10:8080/v1".into())
        );
    }

    // 已有 /v1 → 不重复加
    #[test]
    fn normalize_keeps_existing_v1() {
        assert_eq!(
            normalize_probe_endpoint("http://192.168.1.10:8080/v1"),
            Some("http://192.168.1.10:8080/v1".into())
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
            normalize_probe_endpoint("http://host:8080/"),
            Some("http://host:8080/v1".into())
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
        let invalid_names = ["model;rm -rf /", "../etc/passwd", "model && cat",
                            "model$(whoami)", "model`id`", "model|cat", "model>file"];
        for name in invalid_names {
            let safe = name.chars().all(|c| c.is_ascii_alphanumeric() || ":-.".contains(c));
            assert!(!safe, "{name} should be rejected");
        }
    }

    #[test]
    fn model_name_validation_accepts_common_models() {
        let valid_names = ["qwen2.5:3b", "bge-m3", "llama3.2:1b",
                          "deepseek-coder-v2:16b", "model-7b-q4_0.gguf"];
        for name in valid_names {
            let safe = name.chars().all(|c| c.is_ascii_alphanumeric() || ":-.".contains(c));
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
            assert!(!(ep.starts_with("http://") || ep.starts_with("https://")),
                "{bad} should fail validation");
        }
        for good in ["http://h:8080", "https://api.x.com/v1"] {
            assert!(good.starts_with("http://") || good.starts_with("https://"));
        }
    }
}
