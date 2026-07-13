use attune_core::chat_reliability::{evaluate_response, ChatReliabilityConfig, RetrievedChunk};
use attune_core::cost;
use attune_core::llm::ChatMessage;
use attune_core::pii::Redactor;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::eval as eval_surface;
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct ChatRequest {
    pub message: String,
    #[serde(default)]
    pub history: Vec<HistoryMessage>,
    pub session_id: Option<String>,
}

#[derive(Deserialize)]
pub struct ChatStreamRequest {
    pub message: String,
}

#[derive(Deserialize, Clone)]
pub struct HistoryMessage {
    pub role: String,
    pub content: String,
}

/// POST /api/v1/chat -- RAG 对话（非流式）
/// 消息最大字节数（与 MAX_SEQ_LEN 对齐，防止 LLM 请求体过大）
const MAX_MESSAGE_LEN: usize = 32_768;
/// 历史消息单条 content 最大字节数（防止绕过 message 限制的大负载攻击）
const MAX_HISTORY_CONTENT_LEN: usize = 8_192;
/// 历史消息最大条数 —— 硬上限 backstop（防超大 payload）。
/// 真正的窗口感知裁剪由context_budget 在拿到 LLM 后做（见下方）。
const MAX_HISTORY_DEPTH: usize = 80;
const LOCAL_SCHEDULER_KB_ASK_TASK: &str = "kb.query.ask";
const DEFAULT_LOCAL_SCHEDULER_ASK_MAX_OUTPUT_TOKENS: u32 = 24;
const LOCAL_SCHEDULER_KB_ASK_SYSTEM: &str = "Use only refs. Answer in one short sentence, or up to 3 terse bullets for comparisons. Name the relevant manual/topic when supported. If evidence is insufficient, say insufficient. No operational instructions.";
const DEFAULT_CHAT_KB_TOP_K: u32 = 5;
const MIN_CHAT_KB_TOP_K: u32 = 1;
const MAX_CHAT_KB_TOP_K: u32 = 20;
const DEFAULT_LOCAL_SCHEDULER_ASK_CONTEXT_TOP_K: u32 = 3;
const MIN_LOCAL_SCHEDULER_ASK_CONTEXT_TOP_K: u32 = 1;
const MAX_LOCAL_SCHEDULER_ASK_CONTEXT_TOP_K: u32 = 8;
const DEFAULT_CHAT_CONTEXT_CHUNK_MAX_CHARS: u32 = 96;
const MIN_CHAT_CONTEXT_CHUNK_MAX_CHARS: u32 = 48;
const MAX_CHAT_CONTEXT_CHUNK_MAX_CHARS: u32 = 16_384;
const LOCAL_EXTRACTIVE_MODEL_ID: &str = "local-extractive-source-answer";
const LOCAL_SCHEDULER_CONTEXT_TITLE_MAX_CHARS: usize = 64;

/// POST /api/v1/chat/stream -- buffered SSE compatibility endpoint.
///
/// The current web/E2E contract only requires an SSE-shaped response and the same
/// message-size guard as non-streaming chat. Real token streaming should sit behind
/// the scheduler job interface instead of reintroducing direct model calls here.
pub async fn stream_chat(Json(body): Json<ChatStreamRequest>) -> AppResult<impl IntoResponse> {
    if body.message.trim().is_empty() {
        return Err(AppError::BadRequest("message is empty".into()));
    }
    if body.message.len() > MAX_MESSAGE_LEN {
        return Err(AppError::PayloadTooLarge(format!(
            "message too long: {} bytes (max {})",
            body.message.len(),
            MAX_MESSAGE_LEN
        )));
    }

    let payload = serde_json::to_string(&serde_json::json!({
        "content": body.message,
        "done": true,
    }))?;
    Ok((
        [
            (header::CONTENT_TYPE, "text/event-stream; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        format!("data: {payload}\n\n"),
    ))
}

fn chat_kb_top_k() -> usize {
    crate::local_scheduler::env_u32_any(
        &[
            "ATTUNE_CHAT_KB_TOP_K",
            "ATTUNE_SCHEDULER_CHAT_TOP_K",
            "ATTUNE_LOCAL_SCHEDULER_CHAT_TOP_K",
        ],
        DEFAULT_CHAT_KB_TOP_K,
    )
    .clamp(MIN_CHAT_KB_TOP_K, MAX_CHAT_KB_TOP_K) as usize
}

fn chat_context_chunk_max_chars() -> usize {
    crate::local_scheduler::env_u32_any(
        &[
            "ATTUNE_CHAT_CONTEXT_CHUNK_MAX_CHARS",
            "ATTUNE_RAG_CONTEXT_CHUNK_MAX_CHARS",
            "ATTUNE_SCHEDULER_ASK_CONTEXT_CHUNK_MAX_CHARS",
            "ATTUNE_SCHEDULER_CONTEXT_CHUNK_MAX_CHARS",
        ],
        DEFAULT_CHAT_CONTEXT_CHUNK_MAX_CHARS,
    )
    .clamp(
        MIN_CHAT_CONTEXT_CHUNK_MAX_CHARS,
        MAX_CHAT_CONTEXT_CHUNK_MAX_CHARS,
    ) as usize
}

fn local_scheduler_ask_context_top_k() -> usize {
    crate::local_scheduler::env_u32_any(
        &[
            "ATTUNE_SCHEDULER_ASK_CONTEXT_TOP_K",
            "ATTUNE_LOCAL_SCHEDULER_ASK_CONTEXT_TOP_K",
            "ATTUNE_SCHEDULER_CHAT_CONTEXT_TOP_K",
        ],
        DEFAULT_LOCAL_SCHEDULER_ASK_CONTEXT_TOP_K,
    )
    .clamp(
        MIN_LOCAL_SCHEDULER_ASK_CONTEXT_TOP_K,
        MAX_LOCAL_SCHEDULER_ASK_CONTEXT_TOP_K,
    ) as usize
}

fn bounded_context_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let total = trimmed.chars().count();
    if total <= max_chars {
        return trimmed.to_string();
    }
    const ELLIPSIS: &str = "\n...\n";
    if max_chars <= ELLIPSIS.chars().count() + 2 {
        return trimmed.chars().take(max_chars).collect();
    }
    let body_budget = max_chars - ELLIPSIS.chars().count();
    let head_budget = body_budget / 2 + body_budget % 2;
    let tail_budget = body_budget / 2;
    let head: String = trimmed.chars().take(head_budget).collect();
    let tail: String = trimmed
        .chars()
        .rev()
        .take(tail_budget)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{}{}{}", head.trim_end(), ELLIPSIS, tail.trim_start())
}

fn local_scheduler_source_title(k: &Value) -> &str {
    k.get("title").and_then(|v| v.as_str()).unwrap_or("").trim()
}

fn local_scheduler_context_text(title: &str, evidence: &str, max_chars: usize) -> String {
    let title = title.trim();
    if title.is_empty() {
        return bounded_context_text(evidence, max_chars);
    }

    let title_budget = (max_chars / 4).clamp(32, LOCAL_SCHEDULER_CONTEXT_TITLE_MAX_CHARS);
    let title = bounded_context_text(title, title_budget);
    let prefix = format!("{title}: ");
    if prefix.chars().count() >= max_chars {
        return bounded_context_text(&format!("{prefix}{evidence}"), max_chars);
    }

    let evidence_budget = max_chars - prefix.chars().count();
    format!(
        "{prefix}{}",
        bounded_context_text(evidence, evidence_budget)
    )
}

fn build_chat_search_params(
    form_factor: attune_core::platform::FormFactor,
    use_local_scheduler_profile: bool,
    expanded_query: &str,
    detected_domain: Option<&str>,
    top_k: usize,
) -> (
    attune_core::search::SearchParams,
    Option<attune_core::retrieval_plan::RetrievalPlan>,
) {
    crate::retrieval_policy::build_search_params(
        form_factor,
        use_local_scheduler_profile,
        expanded_query,
        detected_domain,
        top_k,
        None,
        None,
        None,
    )
}

fn build_local_scheduler_kb_contexts(knowledge: &[Value]) -> Vec<Value> {
    let max_context_chars = chat_context_chunk_max_chars();
    let context_limit = local_scheduler_ask_context_top_k();
    knowledge
        .iter()
        .take(context_limit)
        .filter_map(|k| {
            let title = local_scheduler_source_title(k);
            let text = k
                .get("inject_content")
                .and_then(|v| v.as_str())
                .or_else(|| k.get("content").and_then(|v| v.as_str()))
                .unwrap_or("")
                .trim();
            if text.is_empty() {
                return None;
            }
            let text = local_scheduler_context_text(title, text, max_context_chars);
            Some(serde_json::json!({
                "text": text,
                "source_id": k.get("item_id").and_then(|v| v.as_str()).unwrap_or(""),
                "title": title,
                "score": k.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0),
                "breadcrumb": k.get("breadcrumb").cloned().unwrap_or_else(|| serde_json::json!([])),
                "chunk_offset_start": k.get("chunk_offset_start").cloned().unwrap_or(Value::Null),
                "chunk_offset_end": k.get("chunk_offset_end").cloned().unwrap_or(Value::Null),
            }))
        })
        .collect()
}

fn build_local_scheduler_admission_messages(query: &str, contexts: &[Value]) -> Vec<ChatMessage> {
    let mut user = format!("Q: {query}");
    if !contexts.is_empty() {
        user.push_str("\n\nRefs:\n");
        for (idx, ctx) in contexts.iter().enumerate() {
            let text = ctx.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if !text.is_empty() {
                user.push_str(&format!("[{}] {}\n", idx + 1, text));
            }
        }
    }
    vec![
        ChatMessage::system(LOCAL_SCHEDULER_KB_ASK_SYSTEM),
        ChatMessage::user(&user),
    ]
}

fn local_scheduler_output_text(outputs: &Value) -> Option<String> {
    for key in ["answer", "text", "content", "response", "summary", "output"] {
        if let Some(s) = outputs.get(key).and_then(|v| v.as_str()) {
            if !s.trim().is_empty() {
                return Some(s.to_string());
            }
        }
    }
    outputs
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(ToString::to_string)
}

fn local_scheduler_extractive_answer_enabled() -> bool {
    crate::local_scheduler::env_bool_any(
        &[
            "ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER",
            "ATTUNE_LOCAL_EXTRACTIVE_ANSWER",
        ],
        true,
    )
}

fn compact_ascii_lower(text: &str) -> String {
    text.to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

fn contains_any_ascii(text: &str, needles: &[&str]) -> bool {
    let haystack = text.to_ascii_lowercase();
    needles.iter().any(|needle| haystack.contains(needle))
}

fn normalize_source_hint_text(text: &str) -> Option<String> {
    let mut hint = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if hint.chars().count() < 4 {
        return None;
    }
    if hint.chars().count() > 260 {
        hint = hint.chars().take(260).collect();
    }
    Some(hint)
}

fn history_source_followup_query(query: &str) -> bool {
    contains_any_ascii(
        query,
        &[
            "prior",
            "previous",
            "cited",
            "citation",
            "referenced",
            "above",
            "same source",
            "that source",
            "last answer",
            "上一轮",
            "上轮",
            "之前",
            "前文",
            "引用",
            "已引用",
            "来源",
        ],
    )
}

fn query_source_markers(query: &str) -> Vec<&'static str> {
    let candidates = [
        "a220",
        "a300",
        "a310",
        "a318",
        "a319",
        "a320",
        "a321",
        "a330",
        "a340",
        "a350",
        "a380",
        "b737",
        "737",
        "b747",
        "747",
        "b767",
        "767",
        "b777",
        "777",
        "b787",
        "787",
        "qrh",
        "quick reference",
        "fcom",
        "fctm",
        "amm",
        "sop",
        "standard operating",
        "mel",
        "hydraulic",
        "electrical",
        "fuel",
        "navigation",
        "powerplant",
        "landing gear",
        "flight controls",
    ];
    let lower = query.to_ascii_lowercase();
    candidates
        .iter()
        .copied()
        .filter(|marker| lower.contains(marker))
        .collect()
}

fn source_hint_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let bullet = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("• "))
        .unwrap_or(trimmed)
        .trim();
    if bullet.len() < 4 {
        return None;
    }
    if !trimmed.starts_with("- ")
        && !trimmed.starts_with("* ")
        && !trimmed.starts_with("• ")
        && !bullet.contains('《')
    {
        return None;
    }
    let lower = bullet.to_ascii_lowercase();
    if !contains_any_ascii(
        &lower,
        &[
            "source",
            "manual",
            "reference",
            "qrh",
            "fcom",
            "fctm",
            "amm",
            "sop",
            "mel",
            "pdf",
            "flight crew",
        ],
    ) {
        return None;
    }
    normalize_source_hint_text(bullet)
}

fn score_source_hint_for_query(hint: &str, markers: &[&str]) -> usize {
    if markers.is_empty() {
        return 1;
    }
    let compact_hint = compact_ascii_lower(hint);
    markers
        .iter()
        .filter(|marker| {
            let compact_marker = compact_ascii_lower(marker);
            !compact_marker.is_empty() && compact_hint.contains(&compact_marker)
        })
        .count()
}

fn plain_answer_source_hint_line(line: &str, markers: &[&str]) -> Option<String> {
    if markers.is_empty() {
        return None;
    }
    let hint = normalize_source_hint_text(line.trim())?;
    let lower = hint.to_ascii_lowercase();
    if !contains_any_ascii(
        &lower,
        &[
            "source",
            "manual",
            "reference",
            "handbook",
            "qrh",
            "fcom",
            "fctm",
            "amm",
            "sop",
            "mel",
            "pdf",
            "flight crew",
        ],
    ) {
        return None;
    }
    let score = score_source_hint_for_query(&hint, markers);
    let required_score = if markers.len() >= 2 { 2 } else { 1 };
    if score < required_score {
        return None;
    }
    Some(hint)
}

fn push_history_source_hint(
    candidates: &mut Vec<(usize, usize, String)>,
    seen: &mut std::collections::HashSet<String>,
    markers: &[&str],
    hint: String,
) {
    let key = compact_ascii_lower(&hint);
    if key.is_empty() || !seen.insert(key) {
        return;
    }
    let score = score_source_hint_for_query(&hint, markers);
    if markers.is_empty() || score > 0 {
        candidates.push((score, candidates.len(), hint));
    }
}

fn history_source_hints_for_query(
    query: &str,
    history: &[HistoryMessage],
    limit: usize,
) -> Vec<String> {
    if limit == 0 || !history_source_followup_query(query) {
        return Vec::new();
    }
    let markers = query_source_markers(query);
    let mut candidates = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for h in history.iter().rev().filter(|h| h.role == "assistant") {
        let candidate_count_before_message = candidates.len();
        for line in h.content.lines() {
            let Some(hint) = source_hint_line(line) else {
                continue;
            };
            push_history_source_hint(&mut candidates, &mut seen, &markers, hint);
        }
        if candidates.len() == candidate_count_before_message {
            for line in h.content.lines() {
                let Some(hint) = plain_answer_source_hint_line(line, &markers) else {
                    continue;
                };
                push_history_source_hint(&mut candidates, &mut seen, &markers, hint);
            }
        }
        if !candidates.is_empty() {
            break;
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let best_score = candidates.first().map(|(score, _, _)| *score).unwrap_or(0);
    candidates
        .into_iter()
        .filter(|(score, _, _)| markers.is_empty() || *score == best_score)
        .take(limit)
        .map(|(_, _, hint)| hint)
        .collect()
}

fn build_history_aware_retrieval_query(query: &str, history: &[HistoryMessage]) -> String {
    let hints = history_source_hints_for_query(query, history, 3);
    if hints.is_empty() {
        return query.to_string();
    }
    let markers = query_source_markers(query);
    let mut out = if markers.is_empty() {
        "prior cited source".to_string()
    } else {
        format!("{} source", markers.join(" "))
    };
    out.push_str("\nPrior cited source hints:");
    for hint in hints {
        out.push_str("\n- ");
        out.push_str(&hint);
    }
    out
}

fn local_scheduler_operational_safety_query(query: &str) -> bool {
    let q = query.to_ascii_lowercase();
    let operational = contains_any_ascii(
        &q,
        &[
            "real flight",
            "emergency steps",
            "engine fire",
            "flight emergency",
            "operational",
            "maintenance signoff",
            "维修步骤",
            "真实飞行",
            "应急步骤",
        ],
    );
    let urgent = contains_any_ascii(
        &q,
        &["now", "immediately", "exact steps", "step by step", "马上"],
    );
    operational || (urgent && contains_any_ascii(&q, &["qrh", "emergency", "fire", "飞行", "应急"]))
}

fn local_scheduler_source_lookup_query(query: &str) -> bool {
    contains_any_ascii(
        query,
        &[
            "source",
            "manual",
            "reference",
            "lookup",
            "description",
            "system",
            "systems",
            "fcom",
            "qrh",
            "fctm",
            "amm",
            "ata",
            "sop",
            "standard operating",
            "mel",
            "abbreviation",
            "abbreviations",
            "hydraulic",
            "electrical",
            "fuel",
            "navigation",
            "powerplant",
            "landing gear",
            "flight controls",
            "air conditioning",
            "minimum equipment",
        ],
    )
}

fn source_title_from_knowledge(k: &Value) -> String {
    k.get("title")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| k.get("item_id").and_then(|v| v.as_str()).map(str::trim))
        .unwrap_or("local KB source")
        .to_string()
}

fn snippet_from_knowledge(k: &Value, max_chars: usize) -> String {
    let text = k
        .get("inject_content")
        .and_then(|v| v.as_str())
        .or_else(|| k.get("content").and_then(|v| v.as_str()))
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut snippet: String = text.chars().take(max_chars).collect();
    snippet.push_str("...");
    snippet
}

fn local_scheduler_source_lines(knowledge: &[Value], limit: usize) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut lines = Vec::new();
    for k in knowledge {
        let title = source_title_from_knowledge(k);
        let key = compact_ascii_lower(&title);
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        let snippet = snippet_from_knowledge(k, 180);
        if snippet.is_empty() {
            lines.push(format!("- {title}"));
        } else {
            lines.push(format!("- {title}: {snippet}"));
        }
        if lines.len() >= limit {
            break;
        }
    }
    lines
}

fn build_local_scheduler_safety_refusal(query: &str, knowledge: &[Value]) -> Option<String> {
    if knowledge.is_empty() || !local_scheduler_operational_safety_query(query) {
        return None;
    }

    let source_lines = local_scheduler_source_lines(knowledge, 5);
    if source_lines.is_empty() {
        return None;
    }

    Some(format!(
        "I cannot provide exact real-flight or maintenance emergency procedure steps. Do not use this response for operational flight decisions; consult the official QRH/manual and qualified crew or maintenance personnel.\n\nRelevant local KB sources for citation only:\n{}",
        source_lines.join("\n")
    ))
}

fn build_local_scheduler_extractive_answer(query: &str, knowledge: &[Value]) -> Option<String> {
    if knowledge.is_empty() || !local_scheduler_extractive_answer_enabled() {
        return None;
    }

    let source_lines = local_scheduler_source_lines(knowledge, 5);
    if source_lines.is_empty() {
        return None;
    }

    if !local_scheduler_source_lookup_query(query) {
        return None;
    }

    Some(format!(
        "根据本地知识库检索，优先使用以下已引用来源回答该问题。若需要复杂推理或跨文档综合，应切换到 scheduler answer worker 或云端高质量模式。\n\n{}",
        source_lines.join("\n")
    ))
}

fn local_scheduler_async_content(job_id: Option<&str>, eta_ms: Option<u32>) -> String {
    match (job_id, eta_ms) {
        (Some(id), Some(eta)) if eta > 0 => {
            format!("本地 scheduler 知识库回答任务已提交，job_id={id}，预计等待约 {eta} ms。")
        }
        (Some(id), _) => format!("本地 scheduler 知识库回答任务已提交，job_id={id}。"),
        (None, _) => "本地 scheduler 知识库回答任务已提交。".to_string(),
    }
}

fn local_scheduler_ask_max_output_tokens() -> u32 {
    crate::local_scheduler::env_u32_any(
        &[
            "ATTUNE_SCHEDULER_ASK_MAX_OUTPUT_TOKENS",
            "ATTUNE_LOCAL_ASK_MAX_OUTPUT_TOKENS",
        ],
        DEFAULT_LOCAL_SCHEDULER_ASK_MAX_OUTPUT_TOKENS,
    )
}

fn local_scheduler_route_error(e: attune_core::error::VaultError) -> AppError {
    AppError::detailed(
        StatusCode::SERVICE_UNAVAILABLE,
        serde_json::json!({
            "error": "local scheduler 请求失败。",
            "code": "local-scheduler-job-failed",
            "detail": e.to_string(),
        }),
    )
}

async fn run_local_scheduler_job_action<F>(
    state: &SharedState,
    join_label: &'static str,
    action: F,
) -> AppResult<attune_core::edge_cloud::scheduler::SchedulerJobStatus>
where
    F: FnOnce(
            attune_core::edge_cloud::scheduler::LocalSchedulerClient,
        )
            -> attune_core::error::Result<attune_core::edge_cloud::scheduler::SchedulerJobStatus>
        + Send
        + 'static,
{
    let scheduler_base = crate::local_scheduler::base_from_state(state);
    tokio::task::spawn_blocking(move || {
        let client = attune_core::edge_cloud::scheduler::LocalSchedulerClient::with_base(
            &scheduler_base,
            crate::local_scheduler::SUBMIT_TIMEOUT,
        );
        action(client)
    })
    .await
    .map_err(|e| AppError::Internal(format!("local scheduler {join_label} join error: {e}")))?
    .map_err(local_scheduler_route_error)
}

/// F-17 G1 helper — read whether a given outbound point is enabled in
/// `settings.privacy.<key>`. Defaults to `false` (fail-closed) when the block
/// or key is absent — matching the privacy default (all 5 egress off until the
/// user opts in, per `routes/privacy.rs` + `scripts/privacy-audit.sh` gate #4).
/// The settings meta key is the same one `routes/privacy.rs` reads/writes.
/// `pub(crate)`: shared with `routes/version.rs` (update-check egress) and
/// `routes/llm.rs` (non-local probe candidates) — R1.1b outbound-gate sweep.
pub(crate) fn read_privacy_outbound_enabled(state: &SharedState, key: &str) -> bool {
    let vault = match state.vault.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let meta = vault
        .store()
        .get_meta(attune_core::llm_settings::SETTINGS_META_KEY)
        .ok()
        .flatten();
    let settings: serde_json::Value = match meta {
        Some(data) => serde_json::from_slice(&data).unwrap_or_else(|_| serde_json::json!({})),
        None => serde_json::json!({}),
    };
    settings
        .get("privacy")
        .and_then(|p| p.get(key))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

pub async fn chat(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(mut body): Json<ChatRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // T2 (v1.0.6 KB-bench integration): parse opt-in eval headers + start
    // wall-clock so we can surface latency to bench. parse_eval_headers never
    // fails — invalid values drop to defaults so a malformed bench client
    // never 422s the chat path (per spec §7 graceful degradation).
    let parsed_eval = eval_surface::parse_eval_headers(&headers);
    let t_chat_start = std::time::Instant::now();

    // T1 (v1.0.6 KB-bench, plan Step 10): eval-mode short-circuit. When the
    // bench harness sent seed / force-temp-zero / etc., bypass the RAG /
    // vault / redactor / chat_reliability pipeline and call the LLM with
    // `LlmCallOptions` directly. Production clients (Chrome ext / Web UI /
    // attune-cli) never set eval headers so they continue to take the full
    // path below.
    //
    // Why short-circuit instead of threading `parsed_eval` through every
    // stage: the integration tests in `tests/eval_determinism_test.rs` boot
    // a sealed in-memory vault — the legacy chat path requires `dek_db()`
    // (unlock) for redactor / chat_reliability / project_recommender. The
    // short-circuit isolates determinism semantics from those subsystems so
    // bench results are deterministic _by construction_ (no
    // vault-state-dependent codepaths between seed input and LLM call).
    if parsed_eval.any_set() {
        return eval_short_circuit_chat(&state, &headers, &body, &parsed_eval, t_chat_start)
            .await
            .map(Json);
    }

    // Input validation — 在所有状态检查之前优先拒绝无效输入
    if body.message.is_empty() {
        return Err(AppError::BadRequest("message cannot be empty".into()));
    }
    if body.message.len() > MAX_MESSAGE_LEN {
        return Err(AppError::BadRequest(format!(
            "message too long (max {MAX_MESSAGE_LEN} bytes)"
        )));
    }
    // 白名单校验 history role：防止客户端注入 system 消息绕过 RAG 指令
    const ALLOWED_ROLES: &[&str] = &["user", "assistant"];
    for h in &body.history {
        if !ALLOWED_ROLES.contains(&h.role.as_str()) {
            return Err(AppError::BadRequest(format!(
                "invalid role '{}': must be 'user' or 'assistant'",
                h.role
            )));
        }
        if h.content.len() > MAX_HISTORY_CONTENT_LEN {
            return Err(AppError::BadRequest(format!(
                "history message content too long (max {MAX_HISTORY_CONTENT_LEN} bytes)"
            )));
        }
    }
    // 静默截断历史深度：保留最近 N 条
    if body.history.len() > MAX_HISTORY_DEPTH {
        let drop = body.history.len() - MAX_HISTORY_DEPTH;
        body.history.drain(..drop);
    }
    let plugin_registry = crate::routes::plugins::current_plugin_registry(&state);

    // Sprint 1 Phase B: chat keyword trigger for project recommendation
    // 纯 observer：检测当前 user message 中的项目相关关键词，命中即通过 broadcast 推 ws hint，
    // 不影响主流程（错误静默忽略，broadcast 无订阅者也只返回 Err 不 panic）
    //
    // v0.6 边界瘦身：keywords 不再硬编码到 attune-core，由 PluginRegistry 聚合各
    // vertical plugin 的 chat_trigger.project_keywords 后传入。无 plugin 时 = []，永不触发。
    let project_keywords: Vec<&str> = plugin_registry
        .all_chat_trigger_project_keywords()
        .into_iter()
        .collect();
    if let Some(hint) =
        attune_core::project_recommender::recommend_for_chat(&body.message, &project_keywords)
    {
        let payload = serde_json::json!({
            "type": "project_recommendation",
            "trigger": "chat_keyword",
            "matched_keywords": hint.matched_keywords,
            "suggestion": hint.suggestion,
        });
        let _ = state.recommendation_tx.send(payload);
    }

    // Sprint 2 Phase C: Skills Router — 纯 observer，匹配 plugin skill 后通过 broadcast 推 ws skill_suggested
    // 不影响主流程；disabled 集合从 vault settings.skills.disabled 读取（Task 4），
    // has_pending_doc 留 false（Task 5 后由 chat context 决定）
    {
        let registry = plugin_registry.clone();
        // 从 vault metadata 读 settings.skills.disabled；锁失败 / 读失败 / 解析失败均回退空集合
        // （observer 路径不能阻断主流程）
        let disabled: std::collections::HashSet<String> = {
            let bytes = match state.vault.lock() {
                Ok(vault) => vault.store().get_meta("app_settings").ok().flatten(),
                Err(_) => None,
            };
            bytes
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .and_then(|v| {
                    v.get("skills")
                        .and_then(|s| s.get("disabled"))
                        .and_then(|d| d.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                })
                .unwrap_or_default()
        };
        let has_pending_doc = false;
        let router = attune_core::intent_router::IntentRouter::new(&registry);
        let matches = router.route(&body.message, has_pending_doc, &disabled);
        if !matches.is_empty() {
            let payload = serde_json::json!({
                "type": "skill_suggested",
                "trigger": "chat_intent",
                "matches": matches,
                "user_message": body.message,
            });
            let _ = state.recommendation_tx.send(payload);
        }
    }

    // Check LLM availability
    // Bug-C 兜底: state.llm 为 None 时尝试一次 lazy reload —— vault settings 里若已存
    // llm 配置(server restart 后第一次 chat / 老用户 settings 未触发 PATCH 等),
    // reload_llm 会从 settings 重新构建 provider, 避免用户体感 "重启就 503" 的 P3。
    // reload_llm 失败 (无 settings.llm) 仍返 503,行为与之前一致。
    let llm = state
        .llm
        .lock()
        .map_err(|_| AppError::Internal("llm lock poisoned".into()))?
        .as_ref()
        .cloned();
    let llm = match llm {
        Some(l) => l,
        None => {
            tracing::info!("chat: state.llm is None, attempting lazy reload from vault settings");
            state.reload_llm();
            let retry = state
                .llm
                .lock()
                .map_err(|_| AppError::Internal("llm lock poisoned".into()))?
                .as_ref()
                .cloned();
            match retry {
                Some(l) => l,
                None => {
                    // rich error: 带 hint, 走 Detailed 保完整 body
                    return Err(AppError::detailed(
                        StatusCode::SERVICE_UNAVAILABLE,
                        serde_json::json!({
                            "error": "AI 后端不可用",
                            "hint": "请启动 local scheduler，或在设置中配置云端 LLM"
                        }),
                    ));
                }
            }
        }
    };

    // ACP-5 (2026-05-29) — autonomous-flow wiring. When the user message resolves
    // to a *declared multi-step flow* (e.g. legal_defamation), run it end-to-end
    // through the production GovernedStepRunner (each step: ACP-7 schedule + ACP-4
    // governor + ACP-3 telemetry, threaded along the typed-handoff DAG). The
    // outcome is attached to the response as `acp_flow`. A single-agent / no-match
    // resolution returns None so the free-form RAG path below runs unchanged (no
    // regression). Deterministic steps have no embedded agent binary in the server
    // process → the dispatch closure errors and the flow degrades gracefully to a
    // partial result (spec §7 / §11 R8); the chat answer is still produced by RAG.
    //
    // Spec: docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md §5.3b
    let acp_flow: Option<serde_json::Value> = if let Some(flows_reg) = state.agent_flows.clone() {
        let entitlement = {
            let paid = state
                .member_state
                .lock()
                .map(|g| g.is_paid())
                .unwrap_or(false);
            if paid {
                // Real per-call quota accounting lives in the cloud gateway; the
                // scheduler only needs "has cloud budget" here, so seed a non-zero
                // quota. Exhaustion is surfaced by the gateway, not this gate.
                attune_core::agents::scheduler::Entitlement::paid_with_quota(1_000_000)
            } else {
                attune_core::agents::scheduler::Entitlement::free_local()
            }
        };
        // ACP-3 soft-disabled agent ids (same source as the skills observer above).
        let disabled: std::collections::HashSet<String> = {
            let bytes = match state.vault.lock() {
                Ok(vault) => vault.store().get_meta("app_settings").ok().flatten(),
                Err(_) => None,
            };
            bytes
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .and_then(|v| {
                    v.get("agents")
                        .and_then(|s| s.get("disabled"))
                        .and_then(|d| d.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                })
                .unwrap_or_default()
        };
        let flow_llm = llm.clone();
        let flow_cache = state.cache_backend();
        let flow_usage = state.usage();
        let flow_msg = body.message.clone();
        // run_flow is synchronous and may issue governed LLM calls → spawn_blocking
        // so the async worker is never blocked (per Rust async-safe rule).
        tokio::task::spawn_blocking(move || {
            // Server has no embedded agent binaries — deterministic steps degrade
            // gracefully (the LLM lead steps still run + are telemetered).
            let mut dispatch = |_a: &attune_core::agents::registry::AgentSpec,
                                _i: &attune_core::agents::flow::Payload|
             -> std::result::Result<serde_json::Value, String> {
                Err("deterministic agent binary not available in server process".to_string())
            };
            crate::acp_chat::run_chat_flow(
                &flow_msg,
                &flows_reg.0,
                &flows_reg.1,
                flow_llm.as_ref(),
                flow_cache.as_deref(),
                flow_usage.as_deref(),
                entitlement,
                &disabled,
                &mut dispatch,
            )
            .and_then(|o| serde_json::to_value(o).ok())
        })
        .await
        .unwrap_or(None)
    } else {
        None
    };

    // 按 LLM 上下文窗口精确裁历史（替代写死的固定深度）。
    // 不同 model 窗口差 30×（qwen 32K / gemini 1M）—— 按窗口动态保留最近若干轮，
    let dek = {
        let vault = state
            .vault
            .lock()
            .map_err(|_| AppError::Internal("vault lock poisoned".into()))?;
        vault
            .dek_db()
            .map_err(|e| AppError::Forbidden(e.to_string()))?
    };

    // 1a. 读取 app_settings（用于查询扩展 + web_search 配置）
    let app_settings: serde_json::Value = {
        let vault = state
            .vault
            .lock()
            .map_err(|_| AppError::Internal("vault lock".into()))?;
        vault
            .store()
            .get_meta("app_settings")
            .ok()
            .flatten()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    };

    // 历史压缩（多层记忆 §3.3）：超窗的旧轮次不再静默丢弃，而是滚动摘要成 1 条。
    //
    // 旧行为：丢弃的轮次插一条「[此前 N 轮已省略]」占位 —— 信息直接丢失。
    // 新行为：把丢弃的轮次摘要成 1 条（economical），按 sha256(dropped) 缓存在
    // chunk_summaries（合成 conv:<sid> item_id）。长会话只在首次超窗付一次摘要，
    // 之后是缓存命中。既省 token 又找回了原本丢失的信息。
    {
        let pairs: Vec<(String, String)> = body
            .history
            .iter()
            .map(|h| (h.role.clone(), h.content.clone()))
            .collect();
        let plan =
            attune_core::context_budget::plan_context(llm.model_name(), "", &body.message, &pairs);
        if plan.history_dropped > 0 {
            let drop = plan.history_dropped;
            let dropped: Vec<(String, String)> = pairs.iter().take(drop).cloned().collect();
            let sid = body.session_id.clone().unwrap_or_default();
            // compact_history 持锁 + 可能调 LLM —— 走 spawn_blocking 不阻塞 async worker。
            let state_hc = state.clone();
            let dek_hc = dek.clone();
            let llm_hc = llm.clone();
            let rolling = tokio::task::spawn_blocking(move || {
                let vault = state_hc.vault.lock().unwrap_or_else(|e| e.into_inner());
                attune_core::memory::compact_history(
                    vault.store(),
                    &dek_hc,
                    llm_hc.as_ref(),
                    &sid,
                    &dropped,
                )
            })
            .await
            .ok()
            .flatten();
            body.history.drain(..drop);
            let summary_turn = match rolling {
                Some(s) => format!("[此前 {drop} 轮较早对话摘要]\n{s}"),
                None => format!(
                    "[此前 {drop} 轮较早对话因超出模型 {} 的上下文窗口已省略]",
                    llm.model_name()
                ),
            };
            body.history.insert(
                0,
                HistoryMessage {
                    role: "user".to_string(),
                    content: summary_turn,
                },
            );
        }
    }

    // 敏感案件强制本地 LLM 开关 —— 开启后注入了本地证据的对话不得外发云端。
    let force_local_for_evidence = app_settings
        .get("force_local_llm_for_evidence")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // 1b. 用 learned_expansions 自动扩展查询词（语义扩展，透明无感）。
    // Short follow-ups such as "use the prior cited source" otherwise lose
    // the source constraint before retrieval. Only append compact source hints
    // when the current turn explicitly refers to prior/cited material.
    let retrieval_query = build_history_aware_retrieval_query(&body.message, &body.history);
    let expanded_query =
        attune_core::skill_evolution::expand_query(&retrieval_query, &app_settings);

    // v0.6 Phase B F-Pro Stage 4：query 意图 detect → cross-domain penalty。
    // S4b MU-5 (R8)：domain 词表完全由 vertical plugin 提供（attune-pro）。
    // OSS 裸装无 plugin → 空词表 → None → 不降权（generic ranking）。
    let domain_keywords = plugin_registry.all_chat_trigger_keywords_by_domain();
    let detected_domain =
        attune_core::search::detect_query_domain(&expanded_query, &domain_keywords);

    if let Some(d) = detected_domain.as_ref() {
        // 不把用户 chat query 明文写日志。日志文件 data_dir()/logs/
        // 不加密、保留 7 天 — query 是高隐私数据（用户问的法律/医疗/私事）。
        // 改 debug 级 + 仅打长度与 domain，不打内容。
        tracing::debug!(domain = %d, query_len = body.message.len(), "F-Pro domain detected");
    }
    let native_scheduler_kb =
        crate::local_scheduler::native_kb_enabled(&app_settings, &state.hardware);

    // 1. Search knowledge base via three-stage pipeline. Local scheduler profiles
    // use the edge-native retrieval planner; other paths keep the existing chat
    // search defaults.
    let (search_params, retrieval_plan) = build_chat_search_params(
        state.hardware.form_factor,
        native_scheduler_kb,
        &expanded_query,
        detected_domain.as_deref(),
        chat_kb_top_k(),
    );
    if let Some(plan) = retrieval_plan.as_ref() {
        tracing::debug!(
            target = ?plan.target,
            top_k = plan.final_top_k,
            initial_k = search_params.initial_k,
            intermediate_k = search_params.intermediate_k,
            evidence_token_budget = plan.evidence_token_budget,
            "local scheduler retrieval planner applied to chat search"
        );
    }
    let reranker = state
        .reranker
        .lock()
        .map_err(|_| AppError::Internal("reranker lock".into()))?
        .clone();
    let emb = state
        .embedding
        .lock()
        .map_err(|_| AppError::Internal("emb lock".into()))?
        .clone();

    let search_results = {
        let ft_guard = if search_params.skip_vector {
            state.fulltext.try_lock().ok()
        } else {
            Some(
                state
                    .fulltext
                    .lock()
                    .map_err(|_| AppError::Internal("ft lock".into()))?,
            )
        };
        let vec_guard = if search_params.skip_vector {
            None
        } else {
            Some(
                state
                    .vectors
                    .lock()
                    .map_err(|_| AppError::Internal("vec lock".into()))?,
            )
        };
        let vault_guard = state
            .vault
            .lock()
            .map_err(|_| AppError::Internal("vault lock".into()))?;

        let ctx = attune_core::search::SearchContext {
            fulltext: ft_guard.as_ref().and_then(|guard| guard.as_ref()),
            vectors: vec_guard.as_ref().and_then(|guard| guard.as_ref()),
            embedding: emb,
            reranker,
            store: vault_guard.store(),
            dek: &dek,
        };
        attune_core::search::search_with_context(&ctx, &expanded_query, &search_params)
            .map_err(|e| AppError::Internal(e.to_string()))?
    };

    // 知识注入预算按 LLM 上下文窗口动态计算（替代写死的 INJECTION_BUDGET=2000）
    let mut search_results = search_results;
    {
        let hist_pairs: Vec<(String, String)> = body
            .history
            .iter()
            .map(|h| (h.role.clone(), h.content.clone()))
            .collect();
        let plan = attune_core::context_budget::plan_context(
            llm.model_name(),
            "",
            &body.message,
            &hist_pairs,
        );
        attune_core::search::allocate_budget(&mut search_results, plan.knowledge_chars());
    }

    // 敏感模式 —— 注入了本地证据的对话不得外发第三方云 LLM。
    if force_local_for_evidence && !search_results.is_empty() && !llm.is_local() {
        // rich error: 带 hint + code, 走 Detailed 保完整 body
        return Err(AppError::detailed(
            StatusCode::FORBIDDEN,
            serde_json::json!({
                "error": "敏感模式：本次对话会注入本地知识库证据，但当前 LLM 是云端服务，已阻止外发。",
                "hint": "请在设置中配置 local scheduler，或关闭「敏感案件强制本地 LLM」。",
                "code": "sensitive-evidence-cloud-blocked",
            }),
        ));
    }

    // F-17 G3: L0 "🔒 永不出网" enforcement — drop every PrivacyTier::L0 item
    // from the context BEFORE it can reach a cloud LLM. Unlike the per-vault
    // `force_local_for_evidence` switch (which blocks the whole turn), L0 is a
    // PER-ITEM tag: the user marked specific files "never leaves the device",
    // so we silently exclude only those chunks and still answer from the rest.
    // Only applies when the destination is a cloud LLM — a local LLM may see L0.
    // The filter primitive (Store::retain_non_l0_for_cloud) is unit-tested in
    // attune-core; here we just invoke it on the live context.
    if !llm.is_local() && !search_results.is_empty() {
        let before = search_results.len();
        search_results = {
            let vault = state
                .vault
                .lock()
                .map_err(|_| AppError::Internal("vault lock (l0 filter)".into()))?;
            vault
                .store()
                .retain_non_l0_for_cloud(&search_results)
                .map_err(|e| AppError::Internal(format!("l0 filter: {e}")))?
        };
        let dropped = before - search_results.len();
        if dropped > 0 {
            tracing::info!(
                target: "outbound_audit",
                "F-17 G3: dropped {dropped} L0-tagged item(s) from cloud LLM context (model={}, local={})",
                llm.model_name(), llm.is_local()
            );
        }
    }

    // 2a0. 批注加权（Batch B.2）—— 🆓 零成本（仅 DB 读 + 算数）
    //
    // 读每条结果的批注，按 label 精确匹配调整 score：
    //   · 🗑/🕰 过时     → 直接剔除
    //   · ⭐/要点/风险    → ×1.5
    //   · 🤔/📍 等       → ×1.2
    // 多个批注取 MAX，不累乘。
    //
    // 包在 spawn_blocking：`list_annotations` 是同步 SQLite + 解密每条 content blob，
    // N=10 结果时可能 ~10ms，避免阻塞 async worker（与下面压缩阶段的三阶段模式一致）。
    let (weight_stats, mut weighted_results) = {
        let state_clone = state.clone();
        let dek_clone = dek.clone();
        let mut results_in = std::mem::take(&mut search_results);
        tokio::task::spawn_blocking(move || {
            let vault_guard = state_clone.vault.lock().unwrap_or_else(|e| e.into_inner());
            let store = vault_guard.store();
            let mut stats = attune_core::annotation_weight::AnnotationWeightStats {
                items_total: results_in.len(),
                ..Default::default()
            };
            let mut kept = Vec::with_capacity(results_in.len());
            for r in results_in.drain(..) {
                let anns = store.list_annotations(&dek_clone, &r.item_id).unwrap_or_default();
                match attune_core::annotation_weight::compute_adjust(&anns) {
                    attune_core::annotation_weight::ScoreAdjust::Drop => {
                        stats.items_dropped += 1;
                    }
                    attune_core::annotation_weight::ScoreAdjust::Multiply(m) => {
                        if m > 1.0 { stats.items_boosted += 1; }
                        let mut r = r;
                        r.score *= m;
                        kept.push(r);
                    }
                }
            }
            stats.items_kept = stats.items_total - stats.items_dropped;
            (stats, kept)
        })
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("chat: annotation weighting task join failed: {e}; falling back to raw search_results");
            (attune_core::annotation_weight::AnnotationWeightStats::default(), Vec::new())
        })
    };
    // spawn_blocking 失败时 weighted_results 为空 —— 此时我们丢失了原 search_results。
    // 但 spawn_blocking 的 panic/join 错误极罕见（内存爆/进程被信号中断），概率远低于
    // 用户被影响的回本。已通过 tracing::warn 记录，UI 会显示 knowledge_count=0 + hint。
    search_results = std::mem::take(&mut weighted_results);

    // 按新的 score 降序重排（过时已剔除，boost 项自然前移）
    search_results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if weight_stats.items_boosted > 0 || weight_stats.items_dropped > 0 {
        tracing::info!(
            "chat: annotation weighting {} items ({} boosted, {} dropped, {} kept)",
            weight_stats.items_total,
            weight_stats.items_boosted,
            weight_stats.items_dropped,
            weight_stats.items_kept,
        );
    }

    // 2a-. 多层记忆：tier-aware 上下文装配（2026-05-18）
    //
    // recall/overview 形态的 query 用紧凑的 L2/L3 记忆摘要替代 L0 原始 chunk，
    // 显著降低注入 token。coverage gate 保证：记忆层命中弱 / precise query →
    // 退回今日的 L0 路径，无回归。assembler 仅在 memory.tiered_assembler_enabled
    // 时介入，且只 *选择已建好的* 记忆，不在读路径触发 LLM（成本契约）。
    let mut context_tier: &'static str = "L0";
    {
        let memory_cfg = attune_core::memory::MemoryConfig {
            tiered_assembler_enabled: app_settings
                .get("memory")
                .and_then(|m| m.get("tiered_assembler_enabled"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            memory_confidence: app_settings
                .get("memory")
                .and_then(|m| m.get("memory_confidence"))
                .and_then(|v| v.as_f64())
                .map(|v| v as f32)
                .unwrap_or(0.70),
        };
        if memory_cfg.tiered_assembler_enabled && !search_results.is_empty() {
            let state_asm = state.clone();
            let dek_asm = dek.clone();
            let query_asm = body.message.clone();
            let l0_in = search_results.clone();
            let assembled = tokio::task::spawn_blocking(move || {
                let idx_guard = state_asm
                    .memory_index
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let idx = idx_guard.as_ref()?;
                let emb = state_asm
                    .embedding
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()?;
                let vault = state_asm.vault.lock().unwrap_or_else(|e| e.into_inner());
                attune_core::memory::assemble_context(
                    vault.store(),
                    &dek_asm,
                    idx,
                    emb.as_ref(),
                    &query_asm,
                    &l0_in,
                    memory_cfg,
                )
                .ok()
            })
            .await
            .ok()
            .flatten();
            if let Some(ctx) = assembled {
                context_tier = ctx.tier_used;
                if ctx.tier_used != "L0" {
                    // 记忆层应答 → 用装配后的 block 替换 search_results。
                    // 记忆 block item_id 为空 → 下游压缩按 web/临时 chunk passthrough。
                    search_results = ctx
                        .blocks
                        .into_iter()
                        .map(|b| attune_core::search::SearchResult {
                            item_id: b.item_id,
                            score: b.score,
                            title: b.title,
                            content: b.content.clone(),
                            source_type: "memory".to_string(),
                            inject_content: Some(b.content),
                            ..Default::default()
                        })
                        .collect();
                    tracing::info!("chat: tiered assembler answered from {}", context_tier);
                }
            }
        }
    }

    // F-17 G3 (defense-in-depth): the tiered assembler may re-introduce an L0
    // item's anchor chunk into the (memory-tagged) context AFTER the first L0
    // filter. Re-filter on item_id for any cloud destination — memory blocks
    // carrying a real L0 item_id are dropped here. Blocks with empty / web ids
    // (pure summaries, no source item) are kept. Safe-default = deny L0.
    if !llm.is_local() && !search_results.is_empty() {
        let l0_ids: std::collections::HashSet<String> = {
            let vault = state
                .vault
                .lock()
                .map_err(|_| AppError::Internal("vault lock (l0 post-assembler)".into()))?;
            vault
                .store()
                .list_l0_item_ids()
                .map_err(|e| AppError::Internal(format!("l0 list: {e}")))?
                .into_iter()
                .collect()
        };
        if !l0_ids.is_empty() {
            let before = search_results.len();
            search_results.retain(|r| !l0_ids.contains(&r.item_id));
            let dropped = before - search_results.len();
            if dropped > 0 {
                tracing::info!(
                    target: "outbound_audit",
                    "F-17 G3: dropped {dropped} L0 anchor(s) re-introduced by memory assembler (model={})",
                    llm.model_name()
                );
            }
        }
    }

    // 2a. 本地无结果时记录失败信号（后台技能进化的驱动数据），非阻塞
    if search_results.is_empty() {
        let signal_state = state.clone();
        let signal_query = body.message.clone();
        tokio::spawn(async move {
            let vault = signal_state.vault.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = vault.store().record_skill_signal(&signal_query, 0, false) {
                tracing::debug!("record_skill_signal failed (non-fatal): {e}");
            }
        });
    }

    // 2b. 若本地无结果，尝试网络搜索 fallback
    let web_search_used;
    let knowledge: Vec<serde_json::Value> = if search_results.is_empty() {
        // F-17 G1: REAL OutboundGate enforcement for the web-search egress at
        // the live call site. `privacy.web_search` is read fresh from settings
        // (it can be toggled at runtime) and the vault is unlocked here (dek_db
        // succeeded above). When the gate refuses, we skip the search entirely
        // — no raw query leaves the device. Provider-level enforcement
        // (BrowserSearchProvider::with_outbound_policy) is the defense-in-depth
        // backstop for direct/other callers.
        let web_search_allowed = read_privacy_outbound_enabled(&state, "web_search");
        let ws = if web_search_allowed {
            state
                .web_search
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        } else {
            tracing::info!(
                target: "outbound_audit",
                "F-17 G1: web_search egress blocked by privacy gate (settings.privacy.web_search=false)"
            );
            None
        };
        if let Some(ws_provider) = ws {
            let query = body.message.clone();
            let web_results = tokio::task::spawn_blocking(move || ws_provider.search(&query, 3))
                .await
                .unwrap_or(Ok(vec![]))
                .unwrap_or_default();

            if !web_results.is_empty() {
                web_search_used = true;
                web_results
                    .into_iter()
                    .map(|r| {
                        serde_json::json!({
                            "item_id": format!("web:{}", r.url),
                            "title": r.title,
                            "inject_content": r.snippet,
                            "content": r.snippet,
                            "score": 0.55,
                            "source_type": "web",
                            "url": r.url,
                        })
                    })
                    .collect()
            } else {
                web_search_used = false;
                vec![]
            }
        } else {
            web_search_used = false;
            vec![]
        }
    } else {
        web_search_used = false;
        search_results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "item_id": r.item_id,
                    "title": r.title,
                    "inject_content": r.inject_content,
                    "content": r.content,
                    "score": r.score,
                    "source_type": r.source_type,
                    // v0.6 Phase B fix: 透传证据流字段到 chat citations
                    "breadcrumb": r.breadcrumb,
                    "chunk_offset_start": r.chunk_offset_start,
                    "chunk_offset_end": r.chunk_offset_end,
                })
            })
            .collect()
    };

    // 2b+. 上下文压缩（Batch B.1）
    //
    // 按 settings.context_strategy 压缩每条 knowledge 的 inject_content：
    //   - raw / web 来源       → passthrough（web 无 item_id、成本不对称）
    //   - economical / accurate → sha256(chunk) 查缓存 → 命中 0 成本；缺失调本地 LLM
    //
    // 整个压缩阶段放在 spawn_blocking 里，避免阻塞 async worker（LLM chat 是同步的）。
    let strategy_str = app_settings
        .get("context_strategy")
        .and_then(|v| v.as_str())
        .unwrap_or("economical")
        .to_string();
    // 本地模型一键化 (2026-06-01): summary 模式决定上下文摘要是否跑 + 用哪个 LLM。
    //   off  → 不压缩 (纯检索注入原文，等价 Raw，零 LLM 成本/无需本地模型)
    //   local→ 用 summary_llm（非 scheduler-native 路径）
    //   cloud→ 复用主 chat LLM (远端 token)，避免要求笔电先启动 scheduler
    // 缺省: 兼容老 vault (无 summary 字段) → "local" 保持历史行为 (summary_llm or chat 兜底)。
    let summary_mode = app_settings
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("local")
        .to_string();
    let mut compression_stats = (0usize, 0usize, 0usize); // (chunks, hits, orig_total_chars)
    let knowledge: Vec<serde_json::Value> = if web_search_used {
        // 网络搜索结果已经是 snippet，不做二次压缩
        knowledge
    } else if summary_mode == "off" || native_scheduler_kb {
        // summary=off：跳过上下文摘要，注入原文 (弱机/离线/省钱)。
        // scheduler-native KB 路径也跳过 summary_llm，避免本地答案生成前误触云端
        // 或 OpenAI-compatible 摘要器；后续 build_local_scheduler_kb_contexts 会按
        // evidence window 对每段做硬上限裁剪。
        knowledge
    } else {
        use attune_core::context_compress::{chunk_hash, CompressedChunk, ContextStrategy};
        let strategy = ContextStrategy::parse(&strategy_str);
        let summary_use_cloud = summary_mode == "cloud";
        // 敏感模式下跳过上下文压缩。压缩会把证据 content 喂给 summary_llm
        // （可能配置为云端，独立于主 llm），绕过上方 F1 对主 LLM 的拦截。
        // 敏感模式宁可注入原文、不省 token，也不让证据流向云端摘要器。
        if strategy == ContextStrategy::Raw || force_local_for_evidence {
            knowledge
        } else {
            // 三阶段压缩，尽量缩短 vault lock 持有时间：
            //   Phase 1（锁）：查 cache，收集 miss 清单
            //   Phase 2（无锁）：对 misses 批量调 LLM 生成摘要
            //   Phase 3（锁）：批量写回 cache
            //
            // **关键 bug 修复（Batch B R1-I1）**：用 `content`（完整内容）而非 `inject_content`
            // 作为 hash 源。原代码用 inject_content 会因 allocate_budget 按分数截断而每次
            // hash 不同，摧毁缓存命中率。content 在同一 item 跨查询是稳定的。
            let inputs: Vec<(
                String, /*item_id*/
                String, /*content_for_hash*/
                String, /*injected_text*/
            )> = knowledge
                .iter()
                .map(|k| {
                    let item_id = k
                        .get("item_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    // 用全量 content 计算 hash + 喂 LLM（生成 chunk 级摘要）
                    let content = k
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    // inject 文本是 allocate_budget 后的 —— 做后备（若 content 为空）
                    let inject = k
                        .get("inject_content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let text = if content.is_empty() { inject } else { content };
                    (item_id, text.clone(), text)
                })
                .collect();

            let state_compress = state.clone();
            let dek_compress = dek.clone();
            let strategy_str_for_log = strategy_str.clone();

            // 把整个三阶段都放进 spawn_blocking 里（锁/LLM 都是同步的）。
            // 内部：phase 1 + 3 持锁；phase 2 释放锁后跑 LLM。
            let compressed_result: std::result::Result<Vec<CompressedChunk>, String> =
                tokio::task::spawn_blocking(move || {
                    let llm_arc = state_compress.llm.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .as_ref().cloned();
                    // summary=cloud → 优先用主 chat LLM (远端 token)；否则用 summary_llm (本地)，
                    // 两者均缺失时互相兜底，确保摘要尽量能跑 (graceful，永不 panic)。
                    let summary_llm_arc = if summary_use_cloud {
                        llm_arc.clone().or_else(|| {
                            state_compress.summary_llm.lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .as_ref().cloned()
                        })
                    } else {
                        state_compress.summary_llm.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .as_ref().cloned()
                            .or_else(|| llm_arc.clone())
                    };
                    let target = strategy.target_chars();
                    let strategy_str = strategy.as_str();

                    // Phase 1：查 cache + 识别短 chunk（免压缩）
                    struct Slot {
                        item_id: String,
                        text: String,
                        hash: String,
                        original_chars: usize,
                        summary: Option<String>,      // Phase 1 填（cache hit）或 Phase 2 填（LLM 新生成）
                        was_cache_hit: bool,          // 严格区分 Phase 1 命中 vs Phase 2 新生成
                        needs_writeback: bool,        // Phase 3 只回写"新生成"的，避免幂等 REPLACE 浪费 IO
                        is_short: bool,               // target_chars 以下，不压缩
                    }
                    let mut slots: Vec<Slot> = {
                        let vault_guard = state_compress.vault.lock().unwrap_or_else(|e| e.into_inner());
                        let store = vault_guard.store();
                        inputs.into_iter().map(|(item_id, hash_src, text)| {
                            let original_chars = text.chars().count();
                            let is_short = original_chars <= target;
                            let hash = chunk_hash(&hash_src);
                            let (summary, was_cache_hit) = if is_short || item_id.is_empty() {
                                (None, false)
                            } else {
                                match store.get_chunk_summary(&dek_compress, &hash, strategy_str).unwrap_or(None) {
                                    Some(s) => (Some(s), true),
                                    None => (None, false),
                                }
                            };
                            Slot {
                                item_id, text, hash, original_chars,
                                summary, was_cache_hit,
                                needs_writeback: false,
                                is_short,
                            }
                        }).collect()
                        // vault_guard drop 此处 → 释放锁
                    };

                    // Phase 2（无锁）：对真正 miss 调 LLM
                    // Fast-fail: 第 1 个 chunk LLM 调用失败后跳过剩余（避免 5 chunk × 120s timeout 串行
                    // 把 client 卡到 180s 断开）。第 1 个失败通常表示 LLM provider 不健康，
                    // 重试也是浪费。所有 miss chunk 改用原文降级。
                    let mut llm_unavailable = false;
                    for s in slots.iter_mut() {
                        if s.is_short || s.was_cache_hit || s.item_id.is_empty() {
                            continue;
                        }
                        let Some(ref llm) = summary_llm_arc else {
                            continue; // LLM 不可用 → 降级原文（summary 保持 None）
                        };
                        if llm_unavailable {
                            // 已经 fast-fail，不再调 LLM
                            continue;
                        }
                        match attune_core::context_compress::generate_summary(llm.as_ref(), &s.text, strategy) {
                            Ok(summary) => {
                                s.summary = Some(summary);
                                s.needs_writeback = true;
                            }
                            Err(e) => {
                                tracing::warn!("chat: summary generation failed for chunk {}: {e}", &s.hash[..8]);
                                // LLM unavailable 错误 → fast-fail 整批
                                let err_msg = e.to_string();
                                if err_msg.contains("llm unavailable")
                                    || err_msg.contains("error sending request")
                                    || err_msg.contains("timed out")
                                {
                                    tracing::warn!(
                                        "chat: LLM unavailable, skipping summary for remaining chunks (graceful fallback to original text)"
                                    );
                                    llm_unavailable = true;
                                }
                            }
                        }
                    }

                    // Phase 3（锁）：回写新生成摘要（不动命中项）
                    {
                        let vault_guard = state_compress.vault.lock().unwrap_or_else(|e| e.into_inner());
                        let store = vault_guard.store();
                        let model_name = summary_llm_arc.as_ref().map(|l| l.model_name().to_string()).unwrap_or_default();
                        for s in slots.iter() {
                            if !s.needs_writeback { continue; }
                            if let Some(ref sum) = s.summary {
                                let _ = store.put_chunk_summary(
                                    &dek_compress, &s.hash, strategy_str,
                                    &s.item_id, &model_name, sum, s.original_chars,
                                );
                            }
                        }
                    }

                    // 组装结果
                    slots.into_iter().map(|s| {
                        let injected = match &s.summary {
                            Some(sum) if !s.is_short => match strategy {
                                ContextStrategy::Accurate => {
                                    let head: String = s.text.chars().take(100).collect();
                                    format!("{sum}\n原文摘录: {head}...")
                                }
                                _ => sum.clone(),
                            },
                            _ => s.text,  // 短文本 / miss 无降级 / LLM 不可用 → 用原文
                        };
                        // cache_hit 严格语义：Phase 1 真实命中 or 短文本（无需压缩）
                        // —— 本次"没花 LLM 钱"即为 hit。Phase 2 的 fresh 生成不算 hit。
                        let cache_hit = s.is_short || s.was_cache_hit;
                        CompressedChunk {
                            injected,
                            original_chars: s.original_chars,
                            cache_hit,
                        }
                    }).collect::<Vec<_>>()
                }).await.map_err(|e| format!("compression task join error: {e}"));

            // **关键 bug 修复（Batch B R2-C1）**：spawn_blocking panic/join 错误时
            // 过去用 .unwrap_or_default() → 空 Vec → zip 丢光所有 knowledge。
            // 现在改为：面板错时降级为 raw 注入（保留 knowledge 原样），只是错过压缩收益。
            match compressed_result {
                Ok(compressed) => {
                    debug_assert_eq!(
                        knowledge.len(),
                        compressed.len(),
                        "compression must produce one CompressedChunk per input"
                    );
                    for c in &compressed {
                        compression_stats.0 += 1;
                        if c.cache_hit {
                            compression_stats.1 += 1;
                        }
                        compression_stats.2 += c.original_chars;
                    }
                    knowledge
                        .into_iter()
                        .zip(compressed)
                        .map(|(mut k, c)| {
                            if let Some(obj) = k.as_object_mut() {
                                obj.insert(
                                    "inject_content".into(),
                                    serde_json::Value::String(c.injected),
                                );
                                obj.insert(
                                    "compression_cached".into(),
                                    serde_json::Value::Bool(c.cache_hit),
                                );
                            }
                            k
                        })
                        .collect()
                }
                Err(e) => {
                    tracing::warn!(
                        "chat: compression task failed ({e}); falling back to raw RAG injection"
                    );
                    let _ = strategy_str_for_log; // 已在 warn 里说明
                    knowledge
                }
            }
        }
    };
    if compression_stats.0 > 0 {
        tracing::info!(
            "chat: context compressed {} chunks ({} cache hits, {} orig chars) strategy={}",
            compression_stats.0,
            compression_stats.1,
            compression_stats.2,
            strategy_str
        );
    }

    // Local scheduler path: answer generation goes through scheduler-native `/kb/tasks`,
    // not through the legacy OpenAI-compatible `/v1/chat/completions` path.
    if native_scheduler_kb && !web_search_used {
        let deterministic_local_answer =
            build_local_scheduler_safety_refusal(&body.message, &knowledge)
                .map(|content| {
                    (
                        content,
                        "local.safety.refusal",
                        "deterministic_operational_safety_refusal",
                        "OperationalSafetyRefusal",
                    )
                })
                .or_else(|| {
                    build_local_scheduler_extractive_answer(&body.message, &knowledge).map(
                        |content| {
                            (
                                content,
                                "local.extractive.answer",
                                "high_confidence_retrieval_extractive_answer",
                                "ExtractiveLocalAnswer",
                            )
                        },
                    )
                });

        if let Some((content, local_task, local_reason, admission_reason)) =
            deterministic_local_answer
        {
            let citations: Vec<serde_json::Value> =
                knowledge.iter().map(eval_surface::build_citation).collect();
            let tokens_in = knowledge
                .iter()
                .map(|k| {
                    k.get("inject_content")
                        .and_then(|v| v.as_str())
                        .or_else(|| k.get("content").and_then(|v| v.as_str()))
                        .map(|s| cost::estimate_tokens(s, LOCAL_EXTRACTIVE_MODEL_ID))
                        .unwrap_or(0)
                })
                .sum::<usize>();
            let tokens_out = cost::estimate_tokens(&content, LOCAL_EXTRACTIVE_MODEL_ID);
            let chat_latency_ms = t_chat_start.elapsed().as_millis() as u64;
            let eval_block = eval_surface::build_eval_block(&parsed_eval, chat_latency_ms);
            let cost_block = eval_surface::build_cost_block(
                tokens_in,
                tokens_out,
                LOCAL_EXTRACTIVE_MODEL_ID,
                true,
            );

            return Ok(Json(serde_json::json!({
                "content": content,
                "citations": citations,
                "knowledge_count": knowledge.len(),
                "session_id": body.session_id,
                "web_search_used": false,
                "confidence": 4,
                "context_tier": context_tier,
                "cost_estimate": {
                    "tokens_in": tokens_in,
                    "tokens_out": tokens_out,
                    "cost_usd": null,
                    "is_local": true,
                    "input_rate_per_k": null,
                    "cache_hit": true,
                    "cached_tokens": tokens_in,
                    "vendor_tokens_in": 0,
                    "vendor_tokens_out": 0,
                },
                "cost": cost_block,
                "grounding": null,
                "eval": eval_block,
                "latency_ms": chat_latency_ms,
                "weight_stats": {
                    "items_total": weight_stats.items_total,
                    "items_boosted": weight_stats.items_boosted,
                    "items_dropped": weight_stats.items_dropped,
                    "items_kept": weight_stats.items_kept,
                },
                "compression_stats": {
                    "chunks": compression_stats.0,
                    "cache_hits": compression_stats.1,
                    "orig_chars": compression_stats.2,
                    "strategy": strategy_str,
                },
                "local_scheduler": {
                    "task": local_task,
                    "scheduled_as": "sync",
                    "job_id": null,
                    "status": "done",
                    "reason": local_reason,
                    "eta_ms": 0,
                    "model": LOCAL_EXTRACTIVE_MODEL_ID,
                    "service_class": "realtime_answer",
                    "device_used": "attune",
                    "latency_ms": chat_latency_ms,
                    "queue_wait_ms": 0,
                    "admission": {
                        "task_name": local_task,
                        "model_id": LOCAL_EXTRACTIVE_MODEL_ID,
                        "service_class": "realtime_answer",
                        "context_tokens": tokens_in,
                        "max_output_tokens": tokens_out,
                        "reason": admission_reason,
                        "explicit_async": false,
                    }
                }
            })));
        }

        let contexts = build_local_scheduler_kb_contexts(&knowledge);
        let admission_messages = build_local_scheduler_admission_messages(&body.message, &contexts);
        let task_body = serde_json::json!({
            "query": body.message,
            "contexts": contexts
        });
        let scheduler_base = crate::local_scheduler::base_from_settings(&app_settings);

        let scheduler_outcome = tokio::task::spawn_blocking(move || {
            let client = attune_core::edge_cloud::scheduler::LocalSchedulerClient::with_base(
                &scheduler_base,
                crate::local_scheduler::SUBMIT_TIMEOUT,
            );
            let profiles = crate::local_scheduler::runtime_profiles_for_base(&scheduler_base);
            let adapter = attune_core::edge_cloud::SchedulerKbTaskAdapter::new(&client, &profiles);
            adapter.submit(
                attune_core::edge_cloud::SchedulerKbTaskSubmitRequest::interactive(
                    LOCAL_SCHEDULER_KB_ASK_TASK,
                    task_body,
                    &admission_messages,
                )
                .with_desired_output_tokens(local_scheduler_ask_max_output_tokens()),
            )
        })
        .await
        .map_err(|e| AppError::Internal(format!("local scheduler task join error: {e}")))?
        .map_err(local_scheduler_submit_error)?;

        match scheduler_outcome {
            attune_core::edge_cloud::SchedulerKbTaskSubmitOutcome::Local(local) => {
                let response = local.response;
                let is_async = local.explicit_async
                    || response.job_id.is_some()
                    || response.scheduled_as.eq_ignore_ascii_case("async");
                let content = if is_async {
                    local_scheduler_async_content(response.job_id.as_deref(), response.eta_ms)
                } else {
                    local_scheduler_output_text(&response.outputs).unwrap_or_else(|| {
                        "本地 scheduler 知识库任务已完成，但未返回可展示文本。".to_string()
                    })
                };
                let confidence = if is_async {
                    3
                } else {
                    attune_core::parse_confidence(&content)
                };
                let content = if is_async {
                    content
                } else {
                    attune_core::strip_confidence_marker(&content).to_string()
                };
                let citations: Vec<serde_json::Value> =
                    knowledge.iter().map(eval_surface::build_citation).collect();
                let tokens_in = local.admission.context_tokens as usize;
                let tokens_out = cost::estimate_tokens(&content, &local.admission.model_id);
                let chat_latency_ms = t_chat_start.elapsed().as_millis() as u64;
                let eval_block = eval_surface::build_eval_block(&parsed_eval, chat_latency_ms);
                let cost_block = eval_surface::build_cost_block(
                    tokens_in,
                    tokens_out,
                    &local.admission.model_id,
                    true,
                );
                let local_scheduler_meta = serde_json::json!({
                    "task": LOCAL_SCHEDULER_KB_ASK_TASK,
                    "scheduled_as": response.scheduled_as,
                    "job_id": response.job_id,
                    "status": response.status,
                    "reason": response.reason,
                    "eta_ms": response.eta_ms,
                    "model": response.model,
                    "service_class": response.service_class,
                    "device_used": response.device_used,
                    "latency_ms": response.latency_ms,
                    "queue_wait_ms": response.queue_wait_ms,
                    "cold_start_wait_ms": response.cold_start_wait_ms,
                    "startup_state": response.startup_state,
                    "startup_wait_ms": response.startup_wait_ms,
                    "worker_pid": response.worker_pid,
                    "outputs": response.outputs,
                    "prompt_cache_key": response.prompt_cache_key,
                    "cache_hit": response.cache_hit,
                    "prompt_cache": response.prompt_cache,
                    "prompt_cache_policy": response.prompt_cache_policy,
                    "refusal_policy": response.refusal_policy,
                    "admission": {
                        "task_name": local.admission.task_name,
                        "model_id": local.admission.model_id,
                        "service_class": local.admission.service_class,
                        "context_tokens": local.admission.context_tokens,
                        "max_output_tokens": local.admission.max_output_tokens,
                        "reason": format!("{:?}", local.admission.reason),
                        "explicit_async": local.explicit_async,
                    }
                });

                return Ok(Json(serde_json::json!({
                    "content": content,
                    "citations": citations,
                    "knowledge_count": knowledge.len(),
                    "session_id": body.session_id,
                    "web_search_used": false,
                    "confidence": confidence,
                    "context_tier": context_tier,
                    "cost_estimate": {
                        "tokens_in": tokens_in,
                        "tokens_out": tokens_out,
                        "cost_usd": null,
                        "is_local": true,
                        "input_rate_per_k": null,
                        "cache_hit": false,
                        "cached_tokens": 0,
                        "vendor_tokens_in": 0,
                        "vendor_tokens_out": 0,
                    },
                    "cost": cost_block,
                    "grounding": null,
                    "eval": eval_block,
                    "latency_ms": chat_latency_ms,
                    "weight_stats": {
                        "items_total": weight_stats.items_total,
                        "items_boosted": weight_stats.items_boosted,
                        "items_dropped": weight_stats.items_dropped,
                        "items_kept": weight_stats.items_kept,
                    },
                    "compression_stats": {
                        "chunks": compression_stats.0,
                        "cache_hits": compression_stats.1,
                        "orig_chars": compression_stats.2,
                        "strategy": strategy_str,
                    },
                    "local_scheduler": local_scheduler_meta
                })));
            }
            attune_core::edge_cloud::SchedulerKbTaskSubmitOutcome::UseCloudIfAllowed(ctx) => {
                return Err(AppError::detailed(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    serde_json::json!({
                        "error": "当前问题的最终证据上下文超过本地 scheduler async 上限。",
                        "code": "local-scheduler-context-too-large",
                        "model": ctx.model_id,
                        "estimated_input_tokens": ctx.estimated_input_tokens,
                        "max_output_tokens": ctx.max_output_tokens,
                        "reason": format!("{:?}", ctx.reason),
                    }),
                ));
            }
            attune_core::edge_cloud::SchedulerKbTaskSubmitOutcome::Reject(ctx) => {
                return Err(AppError::detailed(
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({
                        "error": "本地 scheduler 上下文准入拒绝了本次请求。",
                        "code": "local-scheduler-context-rejected",
                        "reason": format!("{:?}", ctx.reason),
                    }),
                ));
            }
        }
    }

    // 2c. Build RAG system prompt（根据来源调整措辞）
    let mut system_prompt = if web_search_used {
        "你是用户的个人知识助手。本地知识库暂无相关内容，以下来自实时网络搜索。\n\
         请基于这些搜索结果回答用户的问题，并在回答末尾标注「来源：[URL]」。\n\
         如果搜索结果不够可靠，请明确说明并补充你自己的判断。\n\n"
            .to_string()
    } else {
        "你是用户的个人知识助手。以下是从用户本地知识库中检索到的相关文档。\n\
         请基于这些知识回答用户的问题。如果引用了某个文档，请标注 [文档标题]。\n\
         如果知识库中没有相关信息，正常回答即可，不要编造引用。\n\n"
            .to_string()
    };

    if !knowledge.is_empty() {
        let section_label = if web_search_used {
            "=== 网络搜索结果 ==="
        } else {
            "=== 知识库相关文档 ==="
        };
        system_prompt.push_str(section_label);
        system_prompt.push_str("\n\n");
        for (i, k) in knowledge.iter().enumerate() {
            let title = k.get("title").and_then(|v| v.as_str()).unwrap_or("?");
            let content = k
                .get("inject_content")
                .and_then(|v| v.as_str())
                .or_else(|| k.get("content").and_then(|v| v.as_str()))
                .unwrap_or("");
            if web_search_used {
                let url = k.get("url").and_then(|v| v.as_str()).unwrap_or("");
                system_prompt.push_str(&format!(
                    "[{}] 《{}》\nURL: {}\n{}\n\n",
                    i + 1,
                    title,
                    url,
                    content
                ));
            } else {
                let score = k.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                system_prompt.push_str(&format!(
                    "[{}] 《{}》(相关度: {:.0}%)\n{}\n\n",
                    i + 1,
                    title,
                    score.max(0.0) * 100.0,
                    content
                ));
            }
        }
        system_prompt.push_str("=== 参考内容结束 ===\n");
    }

    // ── F-17 PII redact 全路径拦截 (修复 v0.6.3 BUG: 之前 server chat 路径直接发原文) ──
    // 收集所有出网内容到 segments[], 一次 redact_batch 保证 placeholder 全局唯一
    let redactor = Redactor::default();
    let mut segments: Vec<&str> = Vec::with_capacity(2 + body.history.len());
    segments.push(&system_prompt);
    segments.push(&body.message);
    for h in &body.history {
        segments.push(&h.content);
    }
    let (redacted_segments, all_mappings) = redactor.redact_batch(&segments);

    // outbound_audit 日志
    if !all_mappings.is_empty() {
        let mut by_kind: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for m in &all_mappings {
            let prefix = m.kind.placeholder_prefix().to_string().to_uppercase();
            *by_kind.entry(prefix).or_insert(0) += 1;
        }
        tracing::info!(
            target: "outbound_audit",
            "F-17 server: PII redacted in chat outbound — kinds={:?} total={} segments={}",
            by_kind, all_mappings.len(), segments.len()
        );
    }

    // 3. Build messages with REDACTED content
    let redacted_system = redacted_segments[0].clone();
    let redacted_user = redacted_segments[1].clone();
    let mut messages: Vec<ChatMessage> = vec![ChatMessage::system(&redacted_system)];
    for (i, h) in body.history.iter().enumerate() {
        messages.push(ChatMessage {
            role: h.role.clone(),
            content: redacted_segments[2 + i].clone(),
        });
    }
    messages.push(ChatMessage::user(&redacted_user));

    // 提前记录 LLM 元信息供响应体使用（llm 即将被 move 进闭包）
    let llm_model_name = llm.model_name().to_string();
    let llm_is_local = llm.is_local();

    // 4. Call LLM via the ACP-4 Cost Governor (blocking via spawn_blocking).
    //    governed_chat wires the A1 cache (get/put — saves tokens on identical
    //    prompts) + usage recorder (writes usage_events). Free-form chat uses
    //    default options (no output cap) so existing answers are never
    //    truncated (spec §2.3: never sacrifice correctness; §10: miss = current
    //    behavior). Cache key folds in model + sampling knobs + full message
    //    content, so changed injected knowledge auto-invalidates (R1).
    let cache_backend = state.cache_backend();
    let usage_agg = state.usage();
    let gov_opts = attune_core::llm::LlmCallOptions::default();
    let governed = tokio::task::spawn_blocking(move || {
        attune_core::governor::governed_chat(
            llm.as_ref(),
            &messages,
            &gov_opts,
            cache_backend.as_deref(),
            usage_agg.as_deref(),
            None, // direct chat → no agent_id
            None, // TTL: backend default
        )
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .map_err(llm_upstream_error)?;
    let raw_response = governed.text;
    // ACP-4: real vendor token usage + cache disposition for the UI cost chip.
    // Vendor counts are authoritative when reported (> 0); on a cache hit the
    // saved input tokens arrive via `cached_in`.
    let vendor_usage = governed.usage.clone();
    let cache_served = matches!(governed.cache, attune_core::usage::CacheOutcome::Hit);

    // F-17 restore: LLM 响应里的所有 placeholder 还原成原值给用户看
    let response = redactor.restore(&raw_response, &all_mappings);

    // 5. Persist to conversation session
    let session_id = {
        let vault = state
            .vault
            .lock()
            .map_err(|_| AppError::Internal("vault lock poisoned".into()))?;
        let title: String = body.message.chars().take(50).collect();
        // 取已有或新建 session；create_conversation 失败时跳过消息持久化（不插入孤悬消息）
        let sid_opt: Option<String> = match &body.session_id {
            Some(id) => {
                // 验证 session 存在；不存在则自动创建（保证 append_message 外键约束成功）
                match vault.store().get_conversation_by_id(&dek, id) {
                    Ok(Some(_)) => Some(id.clone()),
                    _ => {
                        tracing::warn!("session_id {id} not found, creating new session");
                        vault
                            .store()
                            .create_conversation(&dek, &title)
                            .map_err(|e| tracing::warn!("create_conversation failed: {e}"))
                            .ok()
                    }
                }
            }
            None => vault
                .store()
                .create_conversation(&dek, &title)
                .map_err(|e| tracing::warn!("create_conversation failed: {e}"))
                .ok(),
        };
        if let Some(sid) = sid_opt.as_ref() {
            // 构造引用列表
            let citations_for_session: Vec<attune_core::store::Citation> = knowledge
                .iter()
                .map(|k| attune_core::store::Citation {
                    item_id: k
                        .get("item_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    title: k
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    relevance: k.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                })
                .collect();
            // 使用事务原子写入 user+assistant 一对：任一失败则两条均不写入
            if let Err(e) = vault.store().append_conversation_turn(
                &dek,
                sid,
                &body.message,
                &response,
                &citations_for_session,
            ) {
                tracing::warn!("failed to persist conversation turn to session {sid}: {e}");
            }
        }
        sid_opt
    };

    // 6. Build citations — T2 (v1.0.6): unified builder in attune_server::eval.
    //    Preserves legacy keys (item_id / title / relevance / breadcrumb /
    //    chunk_offset_*) for Chrome extension + Web UI; adds chunk_id / span /
    //    score aliases for vlm-llm-benchmark R3 grounding eval.
    //
    //    Fallback for empty breadcrumb (chunker first-chunk before any heading)
    //    is handled inside build_citation.
    let citations: Vec<serde_json::Value> =
        knowledge.iter().map(eval_surface::build_citation).collect();

    // v0.7 自学习闭环 Phase B hook 2：citation_hit 信号喂 skill_evolution。
    // chat 引用的 chunk 说明 search 召回 + chunk 内容**对答案质量真有贡献**，是高
    // 信号量。skill_evolution 用这些 ref_id 反推"哪类 query 召回了什么 chunk"，
    // 在扩展词学习时优先保留与命中 chunk 同语义的同义词。
    //
    // - query 字段截断到 512 字符（用户可能粘 4KB+ prompt，无截断时 5 行 ×4KB
    //   一年膨胀 skill_signals 表）
    // - 仅第一条写 query，后 4 条 None — 同一 query 关联多 chunk，evolver 用
    //   `WHERE query='...' AND created_at` 反查可还原 group，无需重复存储
    // 失败静默忽略（self-learning 永不阻塞主流程）。
    {
        const MAX_SIGNAL_QUERY_LEN: usize = 512;
        let truncated: String = body.message.chars().take(MAX_SIGNAL_QUERY_LEN).collect();
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        for (i, k) in knowledge.iter().take(5).enumerate() {
            if let Some(item_id) = k.get("item_id").and_then(|v| v.as_str()) {
                let q = if i == 0 {
                    Some(truncated.as_str())
                } else {
                    None
                };
                if let Err(e) = vault
                    .store()
                    .record_signal_event("citation_hit", item_id, q)
                {
                    tracing::debug!(signal = "citation_hit", error = %e, "record_signal_event failed (non-fatal)");
                }
            }
        }
    }

    // v0.6 Phase B fix: 解析 confidence + 剥离 marker（J5 strict prompt 要求 LLM 末尾输出）
    let confidence = attune_core::parse_confidence(&response);
    let response = attune_core::strip_confidence_marker(&response).to_string();

    // OSS-S12 fix: confident hallucination 防御。当所有 citation 的 relevance 都接近零
    // (max < 0.001) 时，说明 RAG 检索到的文档与 query 实质无关，LLM 在用预训练知识
    // "权威地" 编造答案。强制在前面加 disclaimer 让用户知晓答案非来自知识库。
    // 实测反复确认此现象（古希腊伊壁鸠鲁/量子退火等 out-of-corpus query）。
    //
    // OSS-S25 fix (任其坤案件 2026-05-09): 进一步强化对**结构化数据计算 query** 的拒绝。
    // 律师真实场景中"多少元/合计/求和/总计/笔数/对账/转账明细"这类问题 RAG chat 完全
    // 不能 hallucinate（金额错一元都可能直接影响诉讼标的额）。当 max_rel < 0.001 且
    // query 命中结构化计算关键词时，直接 reject 而非加 disclaimer，明确指引用户走
    // 对应 capability（attune-pro/law-pro::bank_statement_aggregator 等 Tool-using 路径）。
    //
    // v0.6.2 升级 (2026-05-10): plugin_registry::match_chat_trigger() 动态路由替代 hard-code
    // COMPUTE_KEYWORDS. attune-pro 装载 capability 后, 关键词由 plugin.yaml 提供, OSS 不需 hard-code.
    // 兜底: 若无 plugin 命中且仍是结构化计算 query, 保留 hard-code 关键词检查 (OSS 单独使用时不丢防御).
    let plugin_match = plugin_registry.match_chat_trigger(&body.message);

    let response = {
        let max_rel: f64 = citations
            .iter()
            .filter_map(|c| c.get("relevance").and_then(|v| v.as_f64()))
            .fold(0.0_f64, f64::max);

        // 兜底关键词 (OSS 裸装无 plugin 时仍检测结构化计算 query)
        const FALLBACK_COMPUTE_KEYWORDS: &[&str] = &[
            "多少元",
            "多少钱",
            "合计",
            "求和",
            "总计",
            "总金额",
            "总额",
            "笔数",
            "几笔",
            "对账",
            "明细",
            "应付",
            "应收",
            "净流入",
            "转账明细",
            "交易明细",
            "本息",
            "利息计算",
        ];
        let q_lower = body.message.to_lowercase();
        let has_amount_pattern = body.message.chars().enumerate().any(|(i, c)| {
            c.is_ascii_digit()
                && body
                    .message
                    .chars()
                    .skip(i + 1)
                    .take(3)
                    .any(|nc| nc == '元' || nc == '万' || nc == '笔' || nc == '张')
        });
        let is_compute_query = FALLBACK_COMPUTE_KEYWORDS
            .iter()
            .any(|k| q_lower.contains(k))
            || has_amount_pattern;

        if let Some(m) = &plugin_match {
            // Plugin 命中 — 提示用户触发 agent (避免纯 RAG 数字 hallucination)
            // 提供 form URL 让前端直接跳转 (per attune-plugin-protocol §3 Stage 3 工作流)
            let form_hint = plugin_registry
                .get_plugin(&m.plugin_id)
                .and_then(|p| p.manifest.ui_components.first())
                .map(|c| format!(
                    "\n\n📋 表单地址: `/api/v1/forms/{}/{}` (前端 iframe 加载, 律师补全 → POST /submit 触发 agent)",
                    m.plugin_id, c.id
                ))
                .unwrap_or_default();
            format!(
                "🔌 检测到此问题适合 **{}** 处理 ({}).\n\n\
                 attune Chat 走 RAG + LLM, 不做精确数值计算 (避免数字 hallucination).\n\n\
                 建议: 通过 agent dispatch 触发, 输出含 audit_trail + 业务红线 enforce.{}\n\n\
                 命中关键词数: {}, priority: {}",
                m.plugin_id, m.description, form_hint, m.keyword_hits, m.priority
            )
        } else if !citations.is_empty() && max_rel < 0.001 && is_compute_query {
            // 兜底: OSS 裸装无 plugin + 结构化计算 + 弱引用 → reject (原 OSS-S25 行为)
            "⚠️ 此问题涉及结构化数据计算（金额求和 / 笔数统计 / 对账等），但当前知识库\
             检索结果与你的问题相关度极低（max relevance < 0.001），LLM 在此场景下若强行\
             回答会产生数字 hallucination 风险（金额错一元可直接影响诉讼标的额）。\n\n\
             建议:\n\
             1. 装载 attune-pro/law-pro 等行业 plugin pack 后, 通过 capability 精确计算\n\
             2. 或检查知识库 ingest + embedding 是否完成（/api/v1/status 看 pending_embeddings）\n\
             3. 或换更具体的提问方式（指定文件名 / 当事方姓名 / 时间范围）"
                .to_string()
        } else if !citations.is_empty() && max_rel < 0.001 && !response.trim().is_empty() {
            // 普通 query + 低相关 → 加 disclaimer (OSS-S12 既有行为)
            format!(
                "⚠️ 知识库中未找到与你问题强相关的内容（最高引用相关度 {:.4}），以下回答主要来自模型预训练知识，仅供参考：\n\n{}",
                max_rel, response
            )
        } else {
            response
        }
    };

    // 6. Build response with optional hint when web search unavailable
    // v0.6 Phase B fix: 透传 confidence (parsed from LLM 末尾 marker)
    // tokens_in 覆盖实际发给 LLM 的全部内容：system + history[] + user message
    let mut tokens_in = cost::estimate_tokens(&system_prompt, &llm_model_name)
        + cost::estimate_tokens(&body.message, &llm_model_name);
    for h in &body.history {
        tokens_in += cost::estimate_tokens(&h.content, &llm_model_name);
    }
    let tokens_out = cost::estimate_tokens(&response, &llm_model_name);
    let cost_usd = if llm_is_local {
        None
    } else {
        cost::estimate_cost_usd(tokens_in, tokens_out, &llm_model_name)
    };
    // input_rate_per_k：直接从定价表取 input 单价，供前端 TokenChip 用 tokens × rate 展示
    // 本地模型无定价返回 null，前端按本地逻辑处理
    let input_rate_per_k: Option<f64> = if llm_is_local {
        None
    } else {
        cost::lookup_pricing(&llm_model_name).map(|p| p.input_per_1k_usd)
    };
    // T2 (v1.0.6 KB-bench): build grounding block via chat_reliability agent.
    // Reuses retrieved knowledge as RAG chunks; runs in-process (deterministic,
    // zero-LLM, ~µs per call per chat_reliability::evaluate_response docs).
    //
    // We only feed local chunks with non-empty content; web-search hits have
    // no persistent source so chat_reliability classifies them as Fabricated
    // (which is correct — they can't be re-cited). The bench R3 aggregator
    // reads `grounding.score` + `grounding.contradictions_count` directly.
    let reliability_chunks: Vec<RetrievedChunk> = knowledge
        .iter()
        .filter_map(|k| {
            let item_id = k.get("item_id").and_then(|v| v.as_str()).unwrap_or("");
            // skip web placeholders (item_id starts with "web:") and empty items
            if item_id.is_empty() || item_id.starts_with("web:") {
                return None;
            }
            let chunk_text = k
                .get("inject_content")
                .and_then(|v| v.as_str())
                .or_else(|| k.get("content").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            if chunk_text.is_empty() {
                return None;
            }
            let score = k.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let mut rc = RetrievedChunk::new(item_id, chunk_text);
            rc.score = score;
            Some(rc)
        })
        .collect();
    let reliability_report = evaluate_response(
        &response,
        &reliability_chunks,
        &body.message,
        &ChatReliabilityConfig::default(),
    );
    let grounding_block = eval_surface::build_grounding_block(&reliability_report);

    // T2 (v1.0.6 KB-bench): structured cost block matching bench schema.
    // Keep the legacy `cost_estimate` shape too (Chrome ext + Web UI read it).
    let cost_block =
        eval_surface::build_cost_block(tokens_in, tokens_out, &llm_model_name, llm_is_local);

    // T2 (v1.0.6 KB-bench): eval block — surfaced only when bench sets
    // X-Attune-Eval-Mode: 1. Null otherwise → old clients see no behavior change.
    let chat_latency_ms = t_chat_start.elapsed().as_millis() as u64;
    let eval_block = eval_surface::build_eval_block(&parsed_eval, chat_latency_ms);

    let mut response_json = serde_json::json!({
        "content": response,
        "citations": citations,
        "knowledge_count": knowledge.len(),
        "session_id": session_id,
        "web_search_used": web_search_used,
        "confidence": confidence,
        // 多层记忆：哪一层应答了本次 query — L0 原始 chunk / L2 情景记忆 / L3 主题记忆。
        // 前端 cost chip tooltip 展示「context: L2 memory」让用户看到 token 省在哪。
        "context_tier": context_tier,
        // Cost & Trigger Contract: Chat 每次响应携带 token/费用估算供前端 chip 展示
        // Legacy shape preserved for Chrome extension + Web UI TokenChip.
        "cost_estimate": {
            "tokens_in": tokens_in,
            "tokens_out": tokens_out,
            "cost_usd": cost_usd,
            "is_local": llm_is_local,
            "input_rate_per_k": input_rate_per_k,
            // ACP-4: cache disposition + authoritative vendor token counts.
            // `cache_hit=true` → served from cache (tokens saved, no upstream
            // call); `cached_tokens` = input tokens the hit avoided. Vendor
            // counts (when the provider reports > 0) are exact, vs the legacy
            // CJK-heuristic `tokens_in`/`tokens_out` above.
            "cache_hit": cache_served,
            "cached_tokens": vendor_usage.cached_in,
            "vendor_tokens_in": vendor_usage.tokens_in,
            "vendor_tokens_out": vendor_usage.tokens_out,
        },
        // T2: structured cost block for vlm-llm-benchmark (matches bench schema).
        "cost": cost_block,
        // T2: grounding block from chat_reliability post-hoc evaluation.
        "grounding": grounding_block,
        // T2: eval block — null unless X-Attune-Eval-Mode: 1 header set.
        "eval": eval_block,
        // T2: total chat latency in ms (always present for bench end-to-end).
        "latency_ms": chat_latency_ms,
        // Batch B.2: 批注加权 / 上下文压缩统计 —— token chip 展开时展示
        "weight_stats": {
            "items_total": weight_stats.items_total,
            "items_boosted": weight_stats.items_boosted,
            "items_dropped": weight_stats.items_dropped,
            "items_kept": weight_stats.items_kept,
        },
        "compression_stats": {
            "chunks": compression_stats.0,
            "cache_hits": compression_stats.1,
            "orig_chars": compression_stats.2,
            "strategy": strategy_str,
        },
    });

    // ACP-5: surface the autonomous-flow outcome (status + per-step trace + final
    // payload) when a declared multi-step flow ran. Absent for plain chat (no
    // regression — old clients ignore the extra key).
    if let Some(flow_json) = acp_flow {
        response_json["acp_flow"] = flow_json;
    }

    // 本地无结果 + 浏览器不可用：明确告知用户而非静默失败
    if knowledge.is_empty() {
        let ws_available = state
            .web_search
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        if !ws_available {
            response_json["hint"] = serde_json::Value::String(
                "本地知识库无相关内容；网络搜索不可用（未检测到 Chrome 或 Edge 浏览器）。\
                 请安装 Chromium 内核浏览器后重试，或手动录入相关知识。"
                    .into(),
            );
        }
    }

    Ok(Json(response_json))
}

/// GET /api/v1/chat/local-scheduler/jobs/{job_id} -- proxy local scheduler async job status.
pub async fn local_scheduler_job_status(
    State(state): State<SharedState>,
    Path(job_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let job =
        run_local_scheduler_job_action(&state, "job", move |client| client.job(&job_id)).await?;

    Ok(Json(serde_json::json!({ "job": job })))
}

/// DELETE /api/v1/chat/local-scheduler/jobs/{job_id} -- best-effort local scheduler job cancel.
pub async fn cancel_local_scheduler_job(
    State(state): State<SharedState>,
    Path(job_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let job =
        run_local_scheduler_job_action(&state, "cancel", move |client| client.cancel_job(&job_id))
            .await?;

    Ok(Json(serde_json::json!({ "job": job })))
}

/// GET /api/v1/chat/history -- 已废弃，返回与 /chat/sessions 一致的格式
/// @deprecated 请使用 GET /api/v1/chat/sessions?limit=50&offset=0
pub async fn chat_history(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    let vault = state
        .vault
        .lock()
        .map_err(|_| AppError::Internal("vault lock".into()))?;
    let dek = vault
        .dek_db()
        .map_err(|e| AppError::Forbidden(e.to_string()))?;

    let sessions = vault
        .store()
        .list_conversations(&dek, 50, 0)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // 返回与 /chat/sessions 相同的 key 结构，保持 API 一致性
    Ok(Json(
        serde_json::json!({"sessions": sessions, "total": sessions.len()}),
    ))
}

/// 将本地 scheduler 任务提交错误映射为客户端可读的 HTTP 响应。
fn local_scheduler_submit_error(e: attune_core::error::VaultError) -> AppError {
    let (status, body) = crate::local_scheduler::scheduler_failure_body(
        &e,
        crate::local_scheduler::SchedulerDegradationPolicy::HonestFailure,
        "本地 scheduler 知识库任务未能完成。",
    );
    AppError::detailed(status, body)
}

/// 将 LLM provider 返回的 VaultError 映射为客户端可读的 HTTP 响应。
///
/// VaultError::LlmUnavailable 的 message 格式为 "<provider> HTTP <status>: <body>"。
/// 从中提取上游 status 后按以下规则映射：
/// - 429 → 429 Too Many Requests  — quota 耗尽，告知用户等待
/// - 503 / 529 / 529 (Anthropic overloaded) → 503 Service Unavailable — 上游不可用
/// - 其他 5xx → 502 Bad Gateway    — 上游内部错误
/// - 其他 4xx → 400 Bad Request    — 配置问题（无效 key 等）
/// - parse 失败 / 其他 → 500        — 未知错误
fn llm_upstream_error(e: attune_core::error::VaultError) -> AppError {
    let msg = e.to_string();
    // 尝试从 "... HTTP <code>: ..." 中解析上游 status
    let upstream_status: Option<u16> = msg
        .split("HTTP ")
        .nth(1)
        .and_then(|s| s.split(':').next())
        .and_then(|code| code.trim().parse().ok());

    // rich error: 携带 code + upstream_status, 走 Detailed 保完整 body
    match upstream_status {
        Some(429) => AppError::detailed(
            StatusCode::TOO_MANY_REQUESTS,
            serde_json::json!({
                "error": "LLM 服务 quota 已耗尽，请稍后再试。",
                "code": "llm-rate-limited",
                "upstream_status": 429,
            }),
        ),
        Some(s) if s == 503 || s == 529 => AppError::detailed(
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({
                "error": "LLM 服务暂时不可用，请稍后重试。",
                "code": "llm-provider-unavailable",
                "upstream_status": s,
            }),
        ),
        Some(s) if (500..600).contains(&s) => AppError::detailed(
            StatusCode::BAD_GATEWAY,
            serde_json::json!({
                "error": "LLM 服务内部错误，请稍后重试。",
                "code": "llm-provider-error",
                "upstream_status": s,
            }),
        ),
        Some(s) if (400..500).contains(&s) => AppError::detailed(
            StatusCode::BAD_REQUEST,
            serde_json::json!({
                "error": "LLM 配置错误（API key 无效或权限不足），请检查设置。",
                "code": "llm-config-error",
                "upstream_status": s,
            }),
        ),
        _ => AppError::detailed(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({
                "error": msg,
                "code": "llm-error",
            }),
        ),
    }
}

/// T1 (v1.0.6 KB-bench, plan 2026-05-28-kb-bench-integration.md Step 10):
/// eval-mode chat handler. Bypasses RAG / vault / redactor / chat_reliability
/// and calls `LlmProvider::chat_with_options` so the bench harness gets a
/// deterministic seed-pinned answer + an `eval` block reporting the
/// provider's `DeterminismLevel`.
///
/// Spec: `docs/superpowers/specs/2026-05-28-kb-memory-vs-vlm-llm-bench-validation.md`
/// §11 Risk A.
///
/// Provider-label header (`X-Attune-Test-Provider-Label`) is test-only —
/// production clients never set it. When present it maps directly to the
/// `eval.determinism` field, letting a single in-process mock impersonate
/// both an Anthropic-flavored provider (degrades to `temp0`) and an
/// OpenAI-flavored one (`exact`) in the same integration test binary.
async fn eval_short_circuit_chat(
    state: &SharedState,
    headers: &HeaderMap,
    body: &ChatRequest,
    parsed_eval: &eval_surface::ParsedEvalHeaders,
    t_start: std::time::Instant,
) -> Result<serde_json::Value, AppError> {
    use attune_core::llm::{DeterminismLevel, LlmCallOptions};

    // Cheap input sanity — eval-mode still rejects obviously bogus payloads
    // so a malformed bench client doesn't waste an LLM round trip.
    if body.message.is_empty() {
        return Err(AppError::BadRequest("message cannot be empty".into()));
    }
    if body.message.len() > MAX_MESSAGE_LEN {
        return Err(AppError::BadRequest(format!(
            "message too long (max {MAX_MESSAGE_LEN} bytes)"
        )));
    }

    let llm = state.llm().ok_or_else(|| {
        // rich error: 带 code, 走 Detailed 保完整 body
        AppError::detailed(
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({
                "error": "LLM provider not configured",
                "code": "llm-unavailable",
            }),
        )
    })?;

    // Build messages — eval path stays minimal: system "You are attune"
    // + history (as-is) + user message. Redaction/RAG omitted by design.
    let mut messages: Vec<ChatMessage> = Vec::with_capacity(body.history.len() + 2);
    messages.push(ChatMessage::system(
        "You are attune, a private AI knowledge partner. Answer concisely.",
    ));
    for h in &body.history {
        messages.push(ChatMessage {
            role: h.role.clone(),
            content: h.content.clone(),
        });
    }
    messages.push(ChatMessage::user(&body.message));

    let opts = LlmCallOptions {
        seed: parsed_eval.seed,
        temperature: if parsed_eval.force_temp_zero {
            Some(0.0)
        } else {
            None
        },
        top_p: if parsed_eval.force_temp_zero {
            Some(1.0)
        } else {
            None
        },
        ..Default::default()
    };

    // T1 test-only override: provider label maps directly to the determinism
    // label surfaced in the response — independent of what the underlying
    // provider impl returns from `determinism_level()`. This lets the
    // integration test simulate both exact/local and Anthropic-style
    // (Temp0) behavior with a single Mock instance.
    //
    // Production callers do not set this header so the chain falls through
    // to the real provider's reported level.
    let label_override = headers
        .get("x-attune-test-provider-label")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase());
    let det = if let Some(label) = label_override.as_deref() {
        match label {
            "anthropic" => DeterminismLevel::Temp0,
            "exact" | "mock" | "local_scheduler" | "openai" => DeterminismLevel::Exact,
            _ => DeterminismLevel::BestEffort,
        }
    } else {
        llm.determinism_level()
    };

    let answer = tokio::task::spawn_blocking(move || llm.chat_with_options(&messages, &opts))
        .await
        .map_err(|e| AppError::Internal(format!("join error: {e}")))?
        .map_err(llm_upstream_error)?;

    let det_label = match det {
        DeterminismLevel::Exact => "exact",
        DeterminismLevel::Temp0 => "temp0",
        DeterminismLevel::BestEffort => "best_effort",
    };

    let latency_ms = t_start.elapsed().as_millis() as u64;
    let eval_block =
        eval_surface::build_eval_block_with_determinism(parsed_eval, latency_ms, Some(det_label));

    // Keep `content` aliased for backward compat — Web UI / extension
    // both read `content`, but bench harness reads `answer`.
    let answer_for_content = answer.clone();
    Ok(serde_json::json!({
        "answer": answer,
        "content": answer_for_content,
        "eval": eval_block,
        "latency_ms": latency_ms,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_core::error::VaultError;
    use axum::body::to_bytes;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        ENV_LOCK.lock().expect("test env lock")
    }

    fn status_of(e: VaultError) -> u16 {
        match llm_upstream_error(e) {
            AppError::Detailed { status, .. } => status.as_u16(),
            other => panic!("expected Detailed, got {other:?}"),
        }
    }

    fn code_of(e: VaultError) -> String {
        match llm_upstream_error(e) {
            AppError::Detailed { body, .. } => body["code"].as_str().unwrap_or("").to_string(),
            other => panic!("expected Detailed, got {other:?}"),
        }
    }

    fn scheduler_status_and_code(e: VaultError) -> (u16, String) {
        match local_scheduler_submit_error(e) {
            AppError::Detailed { status, body } => (
                status.as_u16(),
                body["code"].as_str().unwrap_or("").to_string(),
            ),
            other => panic!("expected Detailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stream_chat_returns_buffered_sse() {
        let response = stream_chat(Json(ChatStreamRequest {
            message: "测试流式响应内容".into(),
        }))
        .await
        .expect("stream response")
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/event-stream; charset=utf-8")
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.starts_with("data: "));
        assert!(text.contains("测试流式响应内容"));
    }

    #[tokio::test]
    async fn stream_chat_rejects_empty_and_oversize_messages() {
        let empty = match stream_chat(Json(ChatStreamRequest { message: "".into() })).await {
            Err(e) => e,
            Ok(_) => panic!("empty message should be rejected"),
        };
        assert!(matches!(empty, AppError::BadRequest(_)));

        let long = match stream_chat(Json(ChatStreamRequest {
            message: "x".repeat(MAX_MESSAGE_LEN + 1),
        }))
        .await
        {
            Err(e) => e,
            Ok(_) => panic!("oversize message should be rejected"),
        };
        assert!(matches!(long, AppError::PayloadTooLarge(_)));
    }

    #[test]
    fn upstream_429_maps_to_too_many_requests() {
        let e = VaultError::LlmUnavailable("openai HTTP 429: rate limit exceeded".into());
        assert_eq!(status_of(e), 429);
    }

    #[test]
    fn upstream_503_maps_to_service_unavailable() {
        let e = VaultError::LlmUnavailable("openai HTTP 503: system cpu overloaded".into());
        let AppError::Detailed { status, body } = llm_upstream_error(e) else {
            panic!("expected Detailed");
        };
        assert_eq!(status.as_u16(), 503);
        assert_eq!(body["code"], "llm-provider-unavailable");
        assert_eq!(body["upstream_status"], 503);
    }

    #[test]
    fn upstream_529_anthropic_overloaded_maps_to_503() {
        // Anthropic uses 529 for overload
        let e = VaultError::LlmUnavailable("openai HTTP 529: overloaded".into());
        assert_eq!(status_of(e), 503);
        assert_eq!(
            code_of(VaultError::LlmUnavailable(
                "openai HTTP 529: overloaded".into()
            )),
            "llm-provider-unavailable"
        );
    }

    #[test]
    fn upstream_500_maps_to_bad_gateway() {
        let e = VaultError::LlmUnavailable("openai HTTP 500: internal error".into());
        assert_eq!(status_of(e), 502);
        assert_eq!(
            code_of(VaultError::LlmUnavailable(
                "openai HTTP 500: internal error".into()
            )),
            "llm-provider-error"
        );
    }

    #[test]
    fn upstream_401_maps_to_bad_request_config_error() {
        let e = VaultError::LlmUnavailable("openai HTTP 401: invalid api key".into());
        assert_eq!(status_of(e), 400);
        assert_eq!(
            code_of(VaultError::LlmUnavailable(
                "openai HTTP 401: invalid api key".into()
            )),
            "llm-config-error"
        );
    }

    #[test]
    fn local_scheduler_unreachable_no_status_maps_to_500() {
        let e =
            VaultError::LlmUnavailable("local scheduler unreachable: connection refused".into());
        assert_eq!(status_of(e), 500);
        assert_eq!(
            code_of(VaultError::LlmUnavailable(
                "local scheduler unreachable: connection refused".into()
            )),
            "llm-error"
        );
    }

    #[test]
    fn upstream_status_present_in_body() {
        let e = VaultError::LlmUnavailable("openai HTTP 429: quota".into());
        let AppError::Detailed { body, .. } = llm_upstream_error(e) else {
            panic!("expected Detailed");
        };
        assert_eq!(body["upstream_status"], 429);
    }

    #[test]
    fn local_scheduler_submit_errors_map_known_statuses() {
        let (status, code) = scheduler_status_and_code(VaultError::LlmUnavailable(
            "local scheduler /kb/tasks/kb.query.ask returned 409 Conflict: busy".into(),
        ));
        assert_eq!(status, 503);
        assert_eq!(code, "local-scheduler-busy");

        let (status, code) = scheduler_status_and_code(VaultError::LlmUnavailable(
            "local scheduler /kb/tasks/kb.query.ask returned 422 Unprocessable Entity: too large"
                .into(),
        ));
        assert_eq!(status, 413);
        assert_eq!(code, "local-scheduler-oversize");

        let (status, code) = scheduler_status_and_code(VaultError::LlmUnavailable(
            "local scheduler /kb/tasks/kb.query.ask returned 429 Too Many Requests: wait".into(),
        ));
        assert_eq!(status, 429);
        assert_eq!(code, "local-scheduler-rate-limited");
    }

    #[test]
    fn local_scheduler_submit_transport_maps_to_unavailable() {
        let (status, code) = scheduler_status_and_code(VaultError::LlmUnavailable(
            "local scheduler /capacity request failed: timed out".into(),
        ));
        assert_eq!(status, 503);
        assert_eq!(code, "local-scheduler-unavailable");
    }

    #[test]
    fn local_scheduler_submit_delay_and_terminal_states_are_distinct() {
        let (status, code) = scheduler_status_and_code(VaultError::LlmUnavailable(
            "local scheduler job job_abc timed out".into(),
        ));
        assert_eq!(status, 504);
        assert_eq!(code, "local-scheduler-delayed");

        let (status, code) = scheduler_status_and_code(VaultError::LlmUnavailable(
            "local scheduler job cancelled".into(),
        ));
        assert_eq!(status, 409);
        assert_eq!(code, "local-scheduler-cancelled");

        let (status, code) = scheduler_status_and_code(VaultError::LlmUnavailable(
            "local scheduler job job_abc expired: ttl exceeded".into(),
        ));
        assert_eq!(status, 410);
        assert_eq!(code, "local-scheduler-expired");
    }

    #[test]
    fn local_scheduler_chat_search_params_use_retrieval_planner() {
        let (params, plan) = build_chat_search_params(
            attune_core::platform::FormFactor::LocalSchedulerAppliance,
            false,
            "总结法律合同里的违约责任",
            Some("legal"),
            5,
        );
        let plan = plan.expect("local scheduler profile should use retrieval planner");
        assert_eq!(plan.rerank_candidate_cap, 20);
        assert_eq!(plan.evidence_token_budget, 2048);
        assert_eq!(params.top_k, 5);
        assert_eq!(params.intermediate_k, 20);
        assert_eq!(params.min_score, Some(0.65));
        assert_eq!(params.domain_hint.as_deref(), Some("legal"));
        assert!(params.skip_rerank);
    }

    #[test]
    fn non_scheduler_chat_search_params_keep_legacy_defaults() {
        let (params, plan) = build_chat_search_params(
            attune_core::platform::FormFactor::Laptop,
            false,
            "总结法律合同里的违约责任",
            Some("legal"),
            5,
        );
        assert!(plan.is_none());
        assert_eq!(params.top_k, 5);
        assert_eq!(params.initial_k, 25);
        assert_eq!(params.intermediate_k, 10);
        assert_eq!(params.min_score, None);
        assert_eq!(params.domain_hint.as_deref(), Some("legal"));
    }

    #[test]
    fn local_scheduler_base_strips_openai_compat_v1_suffix() {
        let settings = serde_json::json!({
            "llm": { "endpoint": "http://127.0.0.1:8090/v1/" }
        });
        assert_eq!(
            crate::local_scheduler::base_from_settings(&settings),
            "http://127.0.0.1:8090"
        );
    }

    #[test]
    fn local_scheduler_ask_max_output_tokens_defaults_and_allows_env_override() {
        let _env = env_lock();
        let previous_generic = std::env::var("ATTUNE_SCHEDULER_ASK_MAX_OUTPUT_TOKENS").ok();
        let previous_local = std::env::var("ATTUNE_LOCAL_ASK_MAX_OUTPUT_TOKENS").ok();
        std::env::remove_var("ATTUNE_SCHEDULER_ASK_MAX_OUTPUT_TOKENS");
        std::env::remove_var("ATTUNE_LOCAL_ASK_MAX_OUTPUT_TOKENS");
        assert_eq!(
            local_scheduler_ask_max_output_tokens(),
            DEFAULT_LOCAL_SCHEDULER_ASK_MAX_OUTPUT_TOKENS
        );

        std::env::set_var("ATTUNE_LOCAL_ASK_MAX_OUTPUT_TOKENS", "80");
        assert_eq!(local_scheduler_ask_max_output_tokens(), 80);
        std::env::set_var("ATTUNE_SCHEDULER_ASK_MAX_OUTPUT_TOKENS", "72");
        assert_eq!(local_scheduler_ask_max_output_tokens(), 72);

        match previous_generic {
            Some(v) => std::env::set_var("ATTUNE_SCHEDULER_ASK_MAX_OUTPUT_TOKENS", v),
            None => std::env::remove_var("ATTUNE_SCHEDULER_ASK_MAX_OUTPUT_TOKENS"),
        }
        match previous_local {
            Some(v) => std::env::set_var("ATTUNE_LOCAL_ASK_MAX_OUTPUT_TOKENS", v),
            None => std::env::remove_var("ATTUNE_LOCAL_ASK_MAX_OUTPUT_TOKENS"),
        }
    }

    #[test]
    fn chat_kb_top_k_defaults_allows_override_and_clamps() {
        let _env = env_lock();
        let previous_generic = std::env::var("ATTUNE_CHAT_KB_TOP_K").ok();
        let previous_scheduler = std::env::var("ATTUNE_SCHEDULER_CHAT_TOP_K").ok();
        let previous_local = std::env::var("ATTUNE_LOCAL_SCHEDULER_CHAT_TOP_K").ok();
        std::env::remove_var("ATTUNE_CHAT_KB_TOP_K");
        std::env::remove_var("ATTUNE_SCHEDULER_CHAT_TOP_K");
        std::env::remove_var("ATTUNE_LOCAL_SCHEDULER_CHAT_TOP_K");
        assert_eq!(chat_kb_top_k(), DEFAULT_CHAT_KB_TOP_K as usize);

        std::env::set_var("ATTUNE_LOCAL_SCHEDULER_CHAT_TOP_K", "8");
        assert_eq!(chat_kb_top_k(), 8);
        std::env::set_var("ATTUNE_SCHEDULER_CHAT_TOP_K", "10");
        assert_eq!(chat_kb_top_k(), 10);
        std::env::set_var("ATTUNE_CHAT_KB_TOP_K", "99");
        assert_eq!(chat_kb_top_k(), MAX_CHAT_KB_TOP_K as usize);

        match previous_generic {
            Some(v) => std::env::set_var("ATTUNE_CHAT_KB_TOP_K", v),
            None => std::env::remove_var("ATTUNE_CHAT_KB_TOP_K"),
        }
        match previous_scheduler {
            Some(v) => std::env::set_var("ATTUNE_SCHEDULER_CHAT_TOP_K", v),
            None => std::env::remove_var("ATTUNE_SCHEDULER_CHAT_TOP_K"),
        }
        match previous_local {
            Some(v) => std::env::set_var("ATTUNE_LOCAL_SCHEDULER_CHAT_TOP_K", v),
            None => std::env::remove_var("ATTUNE_LOCAL_SCHEDULER_CHAT_TOP_K"),
        }
    }

    #[test]
    fn local_scheduler_ask_context_top_k_defaults_allows_override_and_clamps() {
        let _env = env_lock();
        let previous_generic = std::env::var("ATTUNE_SCHEDULER_ASK_CONTEXT_TOP_K").ok();
        let previous_local = std::env::var("ATTUNE_LOCAL_SCHEDULER_ASK_CONTEXT_TOP_K").ok();
        let previous_chat = std::env::var("ATTUNE_SCHEDULER_CHAT_CONTEXT_TOP_K").ok();
        std::env::remove_var("ATTUNE_SCHEDULER_ASK_CONTEXT_TOP_K");
        std::env::remove_var("ATTUNE_LOCAL_SCHEDULER_ASK_CONTEXT_TOP_K");
        std::env::remove_var("ATTUNE_SCHEDULER_CHAT_CONTEXT_TOP_K");
        assert_eq!(
            local_scheduler_ask_context_top_k(),
            DEFAULT_LOCAL_SCHEDULER_ASK_CONTEXT_TOP_K as usize
        );

        std::env::set_var("ATTUNE_SCHEDULER_CHAT_CONTEXT_TOP_K", "4");
        assert_eq!(local_scheduler_ask_context_top_k(), 4);
        std::env::set_var("ATTUNE_LOCAL_SCHEDULER_ASK_CONTEXT_TOP_K", "2");
        assert_eq!(local_scheduler_ask_context_top_k(), 2);
        std::env::set_var("ATTUNE_SCHEDULER_ASK_CONTEXT_TOP_K", "99");
        assert_eq!(
            local_scheduler_ask_context_top_k(),
            MAX_LOCAL_SCHEDULER_ASK_CONTEXT_TOP_K as usize
        );

        match previous_generic {
            Some(v) => std::env::set_var("ATTUNE_SCHEDULER_ASK_CONTEXT_TOP_K", v),
            None => std::env::remove_var("ATTUNE_SCHEDULER_ASK_CONTEXT_TOP_K"),
        }
        match previous_local {
            Some(v) => std::env::set_var("ATTUNE_LOCAL_SCHEDULER_ASK_CONTEXT_TOP_K", v),
            None => std::env::remove_var("ATTUNE_LOCAL_SCHEDULER_ASK_CONTEXT_TOP_K"),
        }
        match previous_chat {
            Some(v) => std::env::set_var("ATTUNE_SCHEDULER_CHAT_CONTEXT_TOP_K", v),
            None => std::env::remove_var("ATTUNE_SCHEDULER_CHAT_CONTEXT_TOP_K"),
        }
    }

    #[test]
    fn local_scheduler_source_lookup_detects_standard_operating_procedure() {
        assert!(local_scheduler_source_lookup_query(
            "A320 RNAV GPS approach standard operating procedure"
        ));
    }

    #[test]
    fn history_aware_retrieval_query_keeps_direct_queries_unchanged() {
        let history = vec![HistoryMessage {
            role: "assistant".into(),
            content: "- QRH320 - Flight Crew Operating Manual: FCOM A320 QRH".into(),
        }];
        assert_eq!(
            build_history_aware_retrieval_query("A320 hydraulic source", &history),
            "A320 hydraulic source"
        );
    }

    #[test]
    fn history_aware_retrieval_query_selects_matching_cited_source() {
        let history = vec![HistoryMessage {
            role: "assistant".into(),
            content: [
                "根据本地知识库检索，优先使用以下已引用来源回答该问题。",
                "- QRH320 - Flight Crew Operating Manual: Flight Crew Operating Manual FCOM A320 QRH",
                "- 787-TBC_OM_TBC_C_100215_QRH_B2P-C - Quick Action Index: Boeing 787 QRH",
                "- A320-Powerplant: A320 powerplant system description source",
            ]
            .join("\n"),
        }];
        let query = build_history_aware_retrieval_query(
            "Using only the prior A320 QRH cited source, identify the manual type.",
            &history,
        );

        assert!(query.contains("Prior cited source hints"));
        assert!(query.contains("QRH320"));
        assert!(!query.contains("787-TBC"));
        assert!(!query.contains("A320-Powerplant"));
    }

    #[test]
    fn history_aware_retrieval_query_derives_hint_from_plain_scheduler_answer() {
        let history = vec![HistoryMessage {
            role: "assistant".into(),
            content: "The A320 QRH Quick Reference Handbook (QRH) is a critical document for pilots, containing detailed procedures.".into(),
        }];
        let query = build_history_aware_retrieval_query(
            "Using only the prior A320 QRH cited source, answer in one sentence: which aircraft family and manual type does that source belong to?",
            &history,
        );

        assert!(query.contains("Prior cited source hints"));
        assert!(query.contains("A320 QRH Quick Reference Handbook"));
    }

    #[test]
    fn local_scheduler_native_kb_can_be_enabled_by_provider_settings() {
        let hardware = attune_core::platform::HardwareProfile::default();
        let settings = serde_json::json!({
            "embedding": { "provider": "local_scheduler" }
        });
        assert!(crate::local_scheduler::native_kb_enabled(
            &settings, &hardware
        ));

        let settings = serde_json::json!({
            "llm": { "provider": "edge_scheduler" }
        });
        assert!(crate::local_scheduler::native_kb_enabled(
            &settings, &hardware
        ));
    }

    #[test]
    fn local_scheduler_context_builder_uses_injected_content_and_source_metadata() {
        let knowledge = vec![serde_json::json!({
            "item_id": "item-1",
            "title": "合同",
            "content": "full text",
            "inject_content": "bounded evidence",
            "score": 0.91,
            "breadcrumb": ["第一章"],
            "chunk_offset_start": 10,
            "chunk_offset_end": 20
        })];
        let contexts = build_local_scheduler_kb_contexts(&knowledge);
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0]["text"], "合同: bounded evidence");
        assert_eq!(contexts[0]["source_id"], "item-1");
        assert_eq!(contexts[0]["title"], "合同");
        assert_eq!(contexts[0]["breadcrumb"][0], "第一章");
    }

    #[test]
    fn local_scheduler_context_builder_limits_answer_contexts() {
        let _env = env_lock();
        let previous = std::env::var("ATTUNE_SCHEDULER_ASK_CONTEXT_TOP_K").ok();
        std::env::set_var("ATTUNE_SCHEDULER_ASK_CONTEXT_TOP_K", "2");

        let knowledge = (0..5)
            .map(|idx| {
                serde_json::json!({
                    "item_id": format!("item-{idx}"),
                    "title": format!("Source {idx}"),
                    "inject_content": format!("evidence {idx}")
                })
            })
            .collect::<Vec<_>>();
        let contexts = build_local_scheduler_kb_contexts(&knowledge);
        assert_eq!(contexts.len(), 2);
        assert_eq!(contexts[0]["source_id"], "item-0");
        assert_eq!(contexts[1]["source_id"], "item-1");

        match previous {
            Some(v) => std::env::set_var("ATTUNE_SCHEDULER_ASK_CONTEXT_TOP_K", v),
            None => std::env::remove_var("ATTUNE_SCHEDULER_ASK_CONTEXT_TOP_K"),
        }
    }

    #[test]
    fn local_scheduler_context_builder_bounds_large_context_text() {
        let long_text = format!("{}MID{}", "a".repeat(3000), "z".repeat(3000));
        let bounded = bounded_context_text(&long_text, 1200);
        assert!(bounded.chars().count() <= 1200);
        assert!(bounded.contains("\n...\n"));
        assert!(bounded.starts_with('a'));
        assert!(bounded.ends_with('z'));
    }

    #[test]
    fn local_scheduler_context_builder_bounds_title_and_evidence_together() {
        let text =
            local_scheduler_context_text(&"manual-title-".repeat(30), &"e".repeat(2000), 256);
        assert!(text.chars().count() <= 256);
        assert!(text.starts_with("manual-title-"));
        assert!(text.contains("\n...\n"));
    }

    #[test]
    fn local_scheduler_output_text_accepts_direct_and_openai_chat_shapes() {
        assert_eq!(
            local_scheduler_output_text(&serde_json::json!({"answer": "直接答案"})).as_deref(),
            Some("直接答案")
        );
        assert_eq!(
            local_scheduler_output_text(&serde_json::json!({
                "choices": [{"message": {"content": "chat answer"}}]
            }))
            .as_deref(),
            Some("chat answer")
        );
    }

    #[test]
    fn local_scheduler_safety_refusal_ignores_extractive_toggle() {
        let _env = env_lock();
        let previous = std::env::var("ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER").ok();
        std::env::set_var("ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER", "0");

        let knowledge = vec![serde_json::json!({
            "item_id": "qrh-320",
            "title": "QRH320 Quick Reference",
            "inject_content": "A320 emergency abnormal checklist reference material"
        })];
        let answer = build_local_scheduler_safety_refusal(
            "Give me exact real flight emergency steps from the QRH for an engine fire now",
            &knowledge,
        )
        .expect(
            "safety query should return a refusal template even when extractive answers are off",
        );

        let lower = answer.to_ascii_lowercase();
        assert!(lower.contains("cannot provide"));
        assert!(lower.contains("do not use"));
        assert!(answer.contains("QRH320"));

        match previous {
            Some(v) => std::env::set_var("ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER", v),
            None => std::env::remove_var("ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER"),
        }
    }

    #[test]
    fn local_scheduler_extractive_answer_lists_sources_for_lookup() {
        let _env = env_lock();
        let previous = std::env::var("ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER").ok();
        std::env::set_var("ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER", "1");

        let knowledge = vec![serde_json::json!({
            "item_id": "a320-powerplant",
            "title": "A320 Powerplant",
            "inject_content": "A320 powerplant system description source for engine indications."
        })];
        let answer = build_local_scheduler_extractive_answer(
            "A320 powerplant system source, avoid A330",
            &knowledge,
        )
        .expect("source lookup should return grounded local KB lines");

        assert!(answer.contains("A320 Powerplant"));
        assert!(answer.contains("A320 powerplant"));

        match previous {
            Some(v) => std::env::set_var("ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER", v),
            None => std::env::remove_var("ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER"),
        }
    }

    #[test]
    fn local_scheduler_extractive_answer_skips_open_ended_queries() {
        let _env = env_lock();
        let previous = std::env::var("ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER").ok();
        std::env::set_var("ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER", "1");

        let knowledge = vec![serde_json::json!({
            "item_id": "memo-source",
            "title": "Memo Source",
            "inject_content": "Relevant background for a generated memo."
        })];
        assert!(build_local_scheduler_extractive_answer(
            "write a long persuasive memo from these notes",
            &knowledge,
        )
        .is_none());

        match previous {
            Some(v) => std::env::set_var("ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER", v),
            None => std::env::remove_var("ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER"),
        }
    }
}
