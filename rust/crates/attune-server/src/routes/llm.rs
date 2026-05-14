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
        Ok(reply) => Ok(Json(LlmTestResponse {
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
