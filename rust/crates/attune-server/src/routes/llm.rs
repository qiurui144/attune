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
use attune_core::outbound_gate::{OutboundGate, OutboundKind, OutboundPolicy};
use attune_core::vault::VaultState;

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
    State(state): State<SharedState>,
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

    // R1.1b: the loopback + discovered-subnet candidates are local destinations
    // (loopback / RFC1918) — no egress, no gate needed. But user-supplied
    // candidates (1) accept ANY http(s) URL, i.e. a non-local probe path. Those
    // go through the OutboundGate (kind=Llm — it's an LLM-endpoint probe) and
    // are silently dropped (graceful: local probing continues) when the gate
    // refuses. Probe payload is empty (bare GET /models), so no redactor needed.
    let (mut candidates, nonlocal): (Vec<String>, Vec<String>) =
        candidates.into_iter().partition(|ep| is_local_probe_target(ep));
    if !nonlocal.is_empty() {
        let enabled = super::chat::read_privacy_outbound_enabled(&state, OutboundKind::Llm.as_str());
        let vault_unlocked = matches!(
            state.vault.lock().unwrap_or_else(|e| e.into_inner()).state(),
            VaultState::Unlocked
        );
        let policy = OutboundPolicy::cloud(OutboundKind::Llm, enabled, vault_unlocked, None);
        match OutboundGate::enforce(&policy, "") {
            Ok(_) => candidates.extend(nonlocal),
            Err(e) => tracing::info!(
                target: "outbound_audit",
                "R1.1b: probe-k3 dropped {} non-local candidate(s) — outbound gate refused: {e}",
                nonlocal.len()
            ),
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
    // strip port; tolerate bracketed IPv6 (`[::1]:8080`)
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

// ── GET /api/v1/ollama/readiness?model=<chat_model> ──────────────────────────
//
// 把 "daemon 是否在 + 配置模型是否已下载" 归一成三态，供 wizard / Settings 渲染
// 🔴 DaemonDown / 🟡 ModelMissing / 🟢 Ready + 对应一键按钮。纯查询，无副作用。

#[derive(Deserialize)]
pub struct ReadinessQuery {
    /// 要核对的 chat 模型；缺省时只判断 daemon 是否在 (Ready.resolved 为空)。
    pub model: Option<String>,
}

/// 探 Ollama `/api/tags`：返回 (daemon_reachable, model_names)。
async fn probe_ollama_tags() -> (bool, Vec<String>) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return (false, vec![]),
    };
    match client.get("http://localhost:11434/api/tags").send().await {
        Ok(resp) if resp.status().is_success() => {
            let models = resp
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v.get("models").cloned())
                .and_then(|m| serde_json::from_value::<Vec<serde_json::Value>>(m).ok())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            (true, models)
        }
        _ => (false, vec![]),
    }
}

pub async fn ollama_readiness(
    axum::extract::Query(q): axum::extract::Query<ReadinessQuery>,
) -> Json<serde_json::Value> {
    let (reachable, models) = probe_ollama_tags().await;
    // 缺省 model 时，daemon 在即视为 Ready(resolved="")；否则核对具体模型。
    let configured = q.model.unwrap_or_default();
    let readiness = if configured.trim().is_empty() {
        if reachable {
            attune_core::ollama_setup::OllamaReadiness::Ready { resolved: String::new() }
        } else {
            attune_core::ollama_setup::OllamaReadiness::DaemonDown
        }
    } else {
        attune_core::ollama_setup::check_readiness(reachable, &models, configured.trim())
    };
    // install_plan 一并返回，省一次往返：DaemonDown 时 UI 直接拿到一键安装方式。
    let plan = attune_core::ollama_setup::install_plan(std::env::consts::OS);
    Json(serde_json::json!({
        "readiness": readiness,
        "models": models,
        "install_plan": plan,
    }))
}

// ── POST /api/v1/ollama/install ──────────────────────────────────────────────
//
// 一键安装 Ollama runtime。Linux 后台跑 install.sh；Windows 下载 OllamaSetup.exe
// 静默安装；macOS / 未知平台无法应用内安装 → 返回 manual_download 给 UI 弹下载链接。
// 安装本身在后台跑（不阻塞请求），UI 轮询 /ollama/readiness 检测 daemon 起来。

/// 同一时间最多 1 个安装进程（安装是重操作，不并发）。
static INSTALL_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

#[derive(Serialize)]
pub struct InstallResponse {
    /// queued (后台执行中) / manual (需用户手动下载) / busy (已有安装在跑)。
    pub status: String,
    pub task_id: Option<String>,
    /// 当 status=manual 时给出下载链接。
    pub download_url: Option<String>,
    /// 用户友好提示 (§4.5 可操作错误信息)。
    pub message: String,
}

pub async fn install_ollama() -> Result<Json<InstallResponse>, ApiError> {
    let plan = attune_core::ollama_setup::install_plan(std::env::consts::OS);
    use attune_core::ollama_setup::OllamaInstallMethod as M;

    match &plan.method {
        M::ManualDownload { download_url } => Ok(Json(InstallResponse {
            status: "manual".into(),
            task_id: None,
            download_url: Some(download_url.clone()),
            message: format!("当前平台 ({}) 需手动安装 Ollama，请前往下载页", plan.platform),
        })),
        M::Script { command } => {
            // 并发守卫：安装是重操作，同时只跑一个。
            if INSTALL_IN_FLIGHT
                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                return Ok(Json(InstallResponse {
                    status: "busy".into(),
                    task_id: None,
                    download_url: Some(plan.homepage.clone()),
                    message: "已有一个 Ollama 安装任务在进行中".into(),
                }));
            }
            let task_id = format!("install-{}", uuid::Uuid::new_v4());
            let task_id_ret = task_id.clone();
            let cmd = command.clone();
            // 后台执行 install.sh；完成后尝试 `ollama serve`（install.sh 在多数 Linux
            // 上会装 systemd unit 并自启，serve 作为 fallback 不阻塞、失败静默）。
            tokio::spawn(async move {
                let out = tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(&cmd)
                    .output()
                    .await;
                match out {
                    Ok(o) if o.status.success() => {
                        tracing::info!("ollama install done (task={task_id})");
                        // best-effort 拉起 daemon（install.sh 通常已自启）
                        let _ = tokio::process::Command::new("ollama")
                            .arg("serve")
                            .spawn();
                    }
                    Ok(o) => {
                        tracing::warn!(
                            "ollama install failed (task={task_id}) status={} stderr={}",
                            o.status,
                            String::from_utf8_lossy(&o.stderr)
                        );
                    }
                    Err(e) => {
                        tracing::warn!("ollama install spawn error (task={task_id}) err={e}");
                    }
                }
                INSTALL_IN_FLIGHT.store(0, Ordering::SeqCst);
            });
            Ok(Json(InstallResponse {
                status: "queued".into(),
                task_id: Some(task_id_ret),
                download_url: None,
                message: "正在后台安装 Ollama，安装完成后将自动可用".into(),
            }))
        }
        M::Installer { download_url } => {
            // Windows: 应用内静默安装较脆弱（需下载 exe + 提权 + /S）。当前阶段
            // 安全做法是把下载链接交给 UI（用户双击安装器）；后续 desktop 端可接
            // Tauri sidecar 静默安装。返回 manual 形态但保留 installer URL。
            Ok(Json(InstallResponse {
                status: "manual".into(),
                task_id: None,
                download_url: Some(download_url.clone()),
                message: "请下载并运行 Ollama 安装器（OllamaSetup.exe）".into(),
            }))
        }
    }
}

// ── GET /api/v1/lmstudio/probe ───────────────────────────────────────────────
//
// 探测本机 LM Studio（默认 OpenAI 兼容 server :1234/v1）。探到则返回可一键填入
// 的 openai_compat endpoint + 模型列表；探不到给官网下载链接。

#[derive(Serialize)]
pub struct LmStudioProbeResponse {
    pub found: bool,
    /// 可一键填入 settings.llm.endpoint 的 OpenAI 兼容地址。
    pub endpoint: Option<String>,
    pub models: Vec<String>,
    /// 探不到时的官网下载链接。
    pub download_url: String,
}

const LMSTUDIO_ENDPOINT: &str = "http://localhost:1234/v1";
const LMSTUDIO_HOMEPAGE: &str = "https://lmstudio.ai/";

pub async fn lmstudio_probe() -> Json<LmStudioProbeResponse> {
    // R1.1b audit: the probe target is the compile-time constant
    // `LMSTUDIO_ENDPOINT` (http://localhost:1234/v1) — a local destination with
    // no user-controllable host, so this is NOT a network egress point and needs
    // no OutboundGate. If a configurable endpoint is ever added here, it must go
    // through `is_local_probe_target` + OutboundGate like `probe_k3`.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(800))
        .build()
        .unwrap_or_default();
    // OpenAI 兼容: GET /v1/models
    let url = format!("{LMSTUDIO_ENDPOINT}/models");
    let models: Option<Vec<String>> = match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => resp
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v.get("data").cloned())
            .and_then(|d| serde_json::from_value::<Vec<serde_json::Value>>(d).ok())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(String::from))
                    .collect()
            }),
        _ => None,
    };
    match models {
        Some(models) => Json(LmStudioProbeResponse {
            found: true,
            endpoint: Some(LMSTUDIO_ENDPOINT.to_string()),
            models,
            download_url: LMSTUDIO_HOMEPAGE.to_string(),
        }),
        None => Json(LmStudioProbeResponse {
            found: false,
            endpoint: None,
            models: vec![],
            download_url: LMSTUDIO_HOMEPAGE.to_string(),
        }),
    }
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

    // R1.1b — probe candidate locality classification (gate boundary)
    #[test]
    fn local_probe_targets_classified_local() {
        for ep in [
            "http://localhost:8080/v1",
            "http://127.0.0.1:8080/v1",
            "http://127.0.0.1/v1",
            "http://[::1]:8080/v1",
            "http://192.168.1.50:8080/v1",
            "http://10.0.0.2:8080/v1",
            "http://172.16.3.4:8080/v1",
            "http://169.254.1.1:8080/v1",
        ] {
            assert!(super::is_local_probe_target(ep), "{ep} should be local");
        }
    }

    #[test]
    fn nonlocal_probe_targets_classified_nonlocal() {
        // Public IPs and named hosts (can resolve anywhere → fail closed) must be
        // gated before probing.
        for ep in [
            "http://8.8.8.8:8080/v1",
            "https://1.2.3.4/v1",
            "http://k3.example.com:8080/v1",
            "https://attacker.tld/v1",
            "http://[2001:db8::1]:8080/v1",
        ] {
            assert!(!super::is_local_probe_target(ep), "{ep} should be non-local");
        }
    }
}
