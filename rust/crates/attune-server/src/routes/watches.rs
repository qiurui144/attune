//! 信息监控闭环 route —— watch CRUD + hits/triage + digest + watch-scoped 问答 + 深度研究。
//!
//! 挂载前缀 `/api/v1/monitoring/*`（kebab-case）。vault auth middleware 守门（Locked 不可达）。
//!
//! 成本契约（spec §8）：
//! - GET watches / hits / 默认 digest / triage = 🆓 零成本（确定性，无 LLM）。
//! - 建 watch 的 anchor embed = ⚡ 本地一次性。
//! - digest LLM 摘要 = ⚡/💰 仅 per-watch `llm_summary=1` 时显式调用。
//! - watch-scoped ask + research = 💰 用户显式触发，UI 显示成本。

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use attune_core::entities::extract_entities;
use attune_core::llm::LlmProvider;
use attune_core::monitoring::deep_research::{DeepResearch, ResearchDoc, ResearchOpts, SourceKind};
use attune_core::monitoring::digest::{DigestBuilder, MapContentSource};
use attune_core::store::watches::{WatchInput, WatchPatch};
use std::sync::Arc;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

// ── tier-3 gates（会员门 + 隐私门 + PII 脱敏），与 routes/writing.rs 对齐 ──────
//
// `ask_watch` / `research` 都是 tier-3 💰（解密 vault 内容 + 用户问题 → cloud LLM）。
// 三道门缺一不可（GA P0 修复）：
//   1. 会员门  — is_paid() 否则 403 membership-required（与 writing.rs / documents.rs 一致）。
//   2. 隐私门  — privacy.llm 关则拒（fail-closed），杜绝默认偷偷出网。
//   3. PII 脱敏 — RedactingLlmProvider 包裹后再调，出网内容经脱敏（chat.rs / writing.rs 同纪律）。

/// MemberState::is_paid()? — tier-3 操作的会员门（parity with writing.rs）。
fn is_paid(state: &SharedState) -> bool {
    state
        .member_state
        .lock()
        .map(|g| g.is_paid())
        .unwrap_or(false)
}

/// 会员门：tier-3 操作必须付费会员，否则 403。
fn enforce_member_gate(state: &SharedState) -> AppResult<()> {
    if is_paid(state) {
        Ok(())
    } else {
        Err(AppError::detailed(
            axum::http::StatusCode::FORBIDDEN,
            serde_json::json!({
                "error": "this operation requires a paid membership",
                "code": "membership-required"
            }),
        ))
    }
}

// ── Watch 管理 ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateWatchRequest {
    pub label: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default)]
    pub anchor_text: String,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub match_threshold: Option<f32>,
    #[serde(default)]
    pub source_weights: HashMap<String, f32>,
    #[serde(default)]
    pub digest_period: Option<String>,
    #[serde(default)]
    pub llm_summary: bool,
    #[serde(default)]
    pub notify: bool,
}

#[derive(Serialize)]
pub struct WatchView {
    pub id: String,
    pub label: String,
    pub keywords: Vec<String>,
    pub entities: Vec<String>,
    pub source_ids: Vec<String>,
    pub digest_period: String,
    pub llm_summary: bool,
    pub notify: bool,
    pub match_threshold: Option<f32>,
    pub enabled: bool,
    pub last_digested_at: Option<String>,
    pub hit_count_pending: usize,
}

/// POST /api/v1/monitoring/watches — 新增关注项（anchor embed 走统一本地/云边界）。
pub async fn create_watch(
    State(state): State<SharedState>,
    Json(req): Json<CreateWatchRequest>,
) -> AppResult<Json<WatchView>> {
    if req.label.trim().is_empty() {
        return Err(AppError::BadRequest("watch-label-empty".into()));
    }
    let has_criteria =
        !req.keywords.is_empty() || !req.entities.is_empty() || !req.anchor_text.trim().is_empty();
    if !has_criteria {
        return Err(AppError::BadRequest("watch-no-criteria".into()));
    }

    // anchor embed（一次性 ⚡）。失败 → 退化为纯关键词/实体匹配 + warning（不阻塞建 watch）。
    let anchor_vec = if req.anchor_text.trim().is_empty() {
        None
    } else {
        match crate::routes::privacy::governed_embedding(&state, false) {
            Some(emb) => match emb.embed(&[req.anchor_text.trim()]) {
                Ok((v, _)) if !v.is_empty() => Some(v[0].clone()),
                _ => {
                    tracing::warn!("watch-anchor-embed-failed: degrading to keyword/entity match");
                    None
                }
            },
            None => {
                tracing::warn!("no embedding provider; watch anchor degrades to keyword/entity");
                None
            }
        }
    };

    // entities 字符串 → Entity（复用 extract_entities，从用户给的实体文本抽取结构）。
    let entities = req
        .entities
        .iter()
        .flat_map(|e| extract_entities(e))
        .collect();

    let input = WatchInput {
        label: req.label.trim().to_string(),
        keywords: req.keywords,
        entities,
        anchor_text: req.anchor_text.trim().to_string(),
        anchor_vec,
        source_ids: req.source_ids,
        match_threshold: req.match_threshold,
        source_weights: req.source_weights,
        digest_period: req.digest_period.unwrap_or_else(|| "weekly".into()),
        llm_summary: req.llm_summary,
        notify: req.notify,
    };

    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db()?;
    let id = vault.store().add_watch(&dek, &input)?;
    let row = vault
        .store()
        .get_watch(&dek, &id)?
        .ok_or_else(|| AppError::Internal("watch not found after create".into()))?;
    let pending = vault.store().count_pending_hits(&id)?;
    Ok(Json(to_view(&row, pending)))
}

fn to_view(row: &attune_core::store::watches::WatchRow, pending: usize) -> WatchView {
    WatchView {
        id: row.watch.id.clone(),
        label: row.watch.label.clone(),
        keywords: row.watch.keywords.clone(),
        entities: row.watch.entities.iter().map(|e| e.value.clone()).collect(),
        source_ids: row.watch.source_ids.clone(),
        digest_period: row.digest_period.clone(),
        llm_summary: row.llm_summary,
        notify: row.notify,
        match_threshold: Some(row.watch.match_threshold),
        enabled: row.watch.enabled,
        last_digested_at: row.last_digested_at.clone(),
        hit_count_pending: pending,
    }
}

/// GET /api/v1/monitoring/watches
pub async fn list_watches(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db()?;
    let rows = vault.store().list_watches(&dek)?;
    let mut watches = Vec::with_capacity(rows.len());
    for row in &rows {
        let pending = vault.store().count_pending_hits(&row.watch.id)?;
        watches.push(to_view(row, pending));
    }
    Ok(Json(serde_json::json!({ "watches": watches })))
}

#[derive(Deserialize)]
pub struct PatchWatchRequest {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub digest_period: Option<String>,
    #[serde(default)]
    pub llm_summary: Option<bool>,
    #[serde(default)]
    pub notify: Option<bool>,
    #[serde(default)]
    pub match_threshold: Option<f32>,
}

/// PATCH /api/v1/monitoring/watches/:id
pub async fn patch_watch(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(req): Json<PatchWatchRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db()?; // ensure unlocked
    let patch = WatchPatch {
        enabled: req.enabled,
        digest_period: req.digest_period,
        llm_summary: req.llm_summary,
        notify: req.notify,
        match_threshold: req.match_threshold,
    };
    if !vault.store().patch_watch(&id, &patch)? {
        return Err(AppError::NotFound("watch-not-found".into()));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /api/v1/monitoring/watches/:id（级联删 hits；已入库 item 保留）。
pub async fn delete_watch(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db()?;
    if !vault.store().delete_watch(&id)? {
        return Err(AppError::NotFound("watch-not-found".into()));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── 命中 / triage ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct HitsQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
}
fn default_limit() -> usize {
    50
}

#[derive(Serialize)]
pub struct HitView {
    pub item_id: String,
    pub title: String,
    pub score: f32,
    pub reasons: Vec<String>,
    pub dedup_group: Option<String>,
    pub created_at: String,
}

/// GET /api/v1/monitoring/watches/:id/hits — 按 triage 分降序（零成本）。
pub async fn list_hits(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Query(q): Query<HitsQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db()?;
    if vault.store().get_watch(&dek, &id)?.is_none() {
        return Err(AppError::NotFound("watch-not-found".into()));
    }
    let hits = vault.store().list_pending_hits(&id, q.limit.min(500))?;
    let views: Vec<HitView> = hits
        .into_iter()
        .map(|h| HitView {
            item_id: h.item_id,
            title: h.title,
            score: h.score,
            reasons: h.reasons,
            dedup_group: h.dedup_group,
            created_at: h.created_at,
        })
        .collect();
    Ok(Json(serde_json::json!({ "hits": views })))
}

/// POST /api/v1/monitoring/scan — 手动触发一遍监控匹配（零成本；与后台 worker 同函数）。
/// UI "立即检查" 用；落 watch_hits 后 GET hits / digest 即可见。
pub async fn scan_now(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    // dek 检查（locked → 401）。
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let _ = vault.dek_db()?;
    }
    let n = crate::state::AppState::run_monitoring_pass(&state, 500);
    Ok(Json(serde_json::json!({ "new_hits": n })))
}

// ── digest ────────────────────────────────────────────────────────────────

/// POST /api/v1/monitoring/watches/:id/digest — 手动触发一次 digest（与 worker 同函数）。
///
/// 默认零成本 extractive；watch.llm_summary=1 时同步加 LLM 摘要（显式路径）。生成后标记
/// hits digested（跨时间去重）。
pub async fn trigger_digest(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    // Materialize all vault-derived inputs, then release the vault before any
    // possible LLM call. This also gives the cloud gate a complete L0 snapshot.
    let (row, hits, hit_sources, content, contains_l0) = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db()?;
        let store = vault.store();
        let row = store
            .get_watch(&dek, &id)?
            .ok_or_else(|| AppError::NotFound("watch-not-found".into()))?;
        let hits = store.list_pending_hits(&id, 200)?;
        let mut hit_sources: HashMap<String, Vec<String>> = HashMap::new();
        let mut content = HashMap::new();
        let mut contains_l0 = false;
        for hit in &hits {
            if let Ok(Some(item)) = store.get_item(&dek, &hit.item_id) {
                hit_sources.insert(hit.item_id.clone(), vec![item.source_type]);
                content.insert(hit.item_id.clone(), item.content);
            }
            contains_l0 |= store
                .get_item_privacy_tier(&hit.item_id)
                .map(|tier| matches!(tier, attune_core::store::audit::PrivacyTier::L0))
                .unwrap_or(true);
        }
        (
            row,
            hits,
            hit_sources,
            MapContentSource(content),
            contains_l0,
        )
    };
    if hits.is_empty() {
        // digest-no-hits（非错误，spec §7）。
        return Ok(Json(serde_json::json!({
            "card_id": null, "entries": 0, "llm_summary_queued": false,
            "cost_hint": { "tier": "free", "note": "no new hits" }
        })));
    }

    let builder = DigestBuilder::default();
    let now = chrono::Utc::now().to_rfc3339();

    let (card, llm_used) = if row.llm_summary {
        enforce_member_gate(&state)?;
        let llm = crate::routes::privacy::governed_llm(&state, contains_l0)?;
        let card = builder.build_llm_summary(
            &id,
            &row.watch.label,
            &hits,
            &content,
            &hit_sources,
            &now,
            llm.as_ref(),
        );
        let used = card.llm_summary.is_some();
        (card, used)
    } else {
        (
            builder.build_default(&id, &row.watch.label, &hits, &content, &hit_sources, &now),
            false,
        )
    };

    let entries = card.entries.len();
    // 跨时间去重：标记本批 hits 已 digest（marker = 最新 hit created_at）。
    let marker = hits
        .iter()
        .map(|h| h.created_at.as_str())
        .max()
        .map(str::to_string);
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let _ = vault.dek_db()?;
        vault.store().mark_hits_digested(&id, marker.as_deref())?;
    }

    let tier = if llm_used { "cloud" } else { "free" };
    Ok(Json(serde_json::json!({
        "card_id": format!("digest:{id}:{now}"),
        "entries": entries,
        "llm_summary": card.llm_summary,
        "llm_summary_queued": llm_used,
        "card": card,
        "cost_hint": { "tier": tier, "note": if llm_used { "LLM summary generated" } else { "extractive (zero-cost)" } }
    })))
}

// ── 源-grounded 问答（watch-scoped RAG）──────────────────────────────────

#[derive(Deserialize)]
pub struct AskRequest {
    pub question: String,
}

/// POST /api/v1/monitoring/watches/:id/ask — watch 范围内的源-grounded 问答（💰 显式）。
///
/// scoped RAG：先取该 watch 的命中 item 集，正常 search 后过滤到该集合（route-level scope，
/// 不改 search 热点签名），LLM 基于这些命中作答并带源引用。
pub async fn ask_watch(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(req): Json<AskRequest>,
) -> AppResult<Json<serde_json::Value>> {
    if req.question.trim().is_empty() {
        return Err(AppError::BadRequest("question must not be empty".into()));
    }
    // tier-3 会员门（free-tier 不得花 token；direct request 同样拒绝）。
    enforce_member_gate(&state)?;
    let emb = crate::routes::privacy::governed_embedding(&state, false);

    // dek 在独立 vault 锁段取出（Key32 是 Clone），并在该段内做 watch 存在校验 + scope 收集，
    // 然后 drop vault guard —— 绝不与 fulltext/vectors 同时持有（防 ABBA 死锁，对齐 search.rs）。
    let (dek, scope) = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db()?;
        let store = vault.store();
        if store.get_watch(&dek, &id)?.is_none() {
            return Err(AppError::NotFound("watch-not-found".into()));
        }
        // watch-scoped item 集（命中 + 已 digest，取较宽范围）。
        let scope: std::collections::HashSet<String> = store
            .list_pending_hits(&id, 500)?
            .into_iter()
            .map(|h| h.item_id)
            .collect();
        (dek, scope)
    };

    // 检索（复用 search）→ 过滤到 scope。锁序严格 fulltext → vectors → vault（热点路径序，
    // 见 routes/search.rs:178-180），三锁在同一作用域内取齐、用完即 drop。
    let results = {
        let ft_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
        let vec_guard = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
        let vault_guard = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = attune_core::search::SearchContext {
            fulltext: ft_guard.as_ref(),
            vectors: vec_guard.as_ref(),
            embedding: emb,
            reranker: None,
            store: vault_guard.store(),
            dek: &dek,
        };
        let params = attune_core::search::SearchParams::with_defaults(20);
        attune_core::search::search_with_context(&ctx, req.question.trim(), &params)
            .map_err(|e| AppError::Internal(e.to_string()))?
    };
    let scoped: Vec<_> = results
        .into_iter()
        .filter(|r| scope.contains(&r.item_id))
        .take(8)
        .collect();

    // An unknown tier is treated as L0 at this egress boundary. In particular,
    // an empty watch scope must stay empty rather than falling back to the whole
    // vault (the old `scope.is_empty()` branch crossed watch boundaries).
    let contains_l0 = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        scoped.iter().any(|result| {
            vault
                .store()
                .get_item_privacy_tier(&result.item_id)
                .map(|tier| matches!(tier, attune_core::store::audit::PrivacyTier::L0))
                .unwrap_or(true)
        })
    };

    // tier-3 隐私门 + PII 脱敏：privacy.llm 关 → 403；开 → provider 经 RedactingLlmProvider 包裹，
    // 出网内容（解密 vault 片段 + 用户问题）先脱敏（对齐 writing.rs / chat.rs）。
    let llm = crate::routes::privacy::governed_llm(&state, contains_l0)?;

    // 构造带编号源的 context（grounding 引用核验）。
    let mut ctx_text = String::new();
    let mut citations = Vec::new();
    for (i, r) in scoped.iter().enumerate() {
        let snippet: String = r.content.chars().take(400).collect();
        ctx_text.push_str(&format!("[{}] {} — {}\n", i + 1, r.title, snippet));
        citations.push(serde_json::json!({
            "item_id": r.item_id, "title": r.title,
            "snippet": snippet, "ref": i + 1
        }));
    }
    let system = "你是知识助手。仅基于给定的编号材料回答用户问题，每个论断后用 [n] 标注来源。\
        若材料不足以回答，明确说明，不要编造。用中文。";
    let user = format!("材料：\n{ctx_text}\n\n问题：{}", req.question.trim());
    let (answer, _usage) = llm
        .chat(system, &user)
        .map_err(|e| AppError::Internal(format!("research-llm-unavailable: {e}")))?;
    // grounding heuristic：答案是否引用了至少一个 [n]。
    let grounded = !scoped.is_empty() && answer.contains('[');

    Ok(Json(serde_json::json!({
        "answer": answer,
        "citations": citations,
        "grounded": grounded,
        "cost_hint": { "tier": "cloud", "note": "watch-scoped grounded QA" }
    })))
}

// ── 深度研究（显式触发）───────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ResearchRequest {
    pub topic: String,
    #[serde(default = "default_true")]
    pub use_web: bool,
    #[serde(default)]
    pub watch_id: Option<String>,
}
fn default_true() -> bool {
    true
}

/// POST /api/v1/monitoring/research — 用户显式发起深度研究（💰 显式，UI 显示成本）。
pub async fn research(
    State(state): State<SharedState>,
    Json(req): Json<ResearchRequest>,
) -> AppResult<Json<serde_json::Value>> {
    if req.topic.trim().is_empty() {
        return Err(AppError::BadRequest("topic must not be empty".into()));
    }
    // tier-3 会员门（free-tier 不得花 token；direct request 同样拒绝）。
    enforce_member_gate(&state)?;

    let emb = crate::routes::privacy::governed_embedding(&state, false);
    let web = state.web_search();
    let web_enabled = req.use_web && web.is_some();

    // 1. vault RAG（多源搜之 vault 半）。
    let mut docs: Vec<ResearchDoc> = Vec::new();
    let contains_l0;
    {
        // dek + watch scope 在独立 vault 锁段取出（Key32 是 Clone），然后 drop guard。
        let (dek, scope) = {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            let dek = vault.dek_db()?;
            let store = vault.store();
            // watch scope（可选）。
            let scope: Option<std::collections::HashSet<String>> = match &req.watch_id {
                Some(wid) => Some(
                    store
                        .list_pending_hits(wid, 500)?
                        .into_iter()
                        .map(|h| h.item_id)
                        .collect(),
                ),
                None => None,
            };
            (dek, scope)
        };

        // 检索锁序严格 fulltext → vectors → vault（热点路径序，见 routes/search.rs:178-180），
        // 三锁同作用域取齐、用完即 drop —— 绝不持 vault 时再取 fulltext/vectors（防 ABBA 死锁）。
        let results = {
            let ft_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
            let vec_guard = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
            let vault_guard = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            let ctx = attune_core::search::SearchContext {
                fulltext: ft_guard.as_ref(),
                vectors: vec_guard.as_ref(),
                embedding: emb,
                reranker: None,
                store: vault_guard.store(),
                dek: &dek,
            };
            let params = attune_core::search::SearchParams::with_defaults(12);
            attune_core::search::search_with_context(&ctx, req.topic.trim(), &params)
                .map_err(|e| AppError::Internal(e.to_string()))?
        };

        let results: Vec<_> = results
            .into_iter()
            .filter(|result| {
                scope
                    .as_ref()
                    .map_or(true, |watch_scope| watch_scope.contains(&result.item_id))
            })
            .collect();
        contains_l0 = {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            results.iter().any(|result| {
                vault
                    .store()
                    .get_item_privacy_tier(&result.item_id)
                    .map(|tier| matches!(tier, attune_core::store::audit::PrivacyTier::L0))
                    .unwrap_or(true)
            })
        };

        for r in results {
            // extractive 预裁（零 LLM 省 token）。
            let snippet = attune_core::document_intelligence::extractive::extract_candidates(
                &r.content,
                0.25,
                &[],
            );
            let snippet: String = if snippet.is_empty() {
                r.content.chars().take(400).collect()
            } else {
                snippet.chars().take(400).collect()
            };
            docs.push(ResearchDoc {
                kind: SourceKind::Vault,
                reference: r.item_id,
                title: r.title,
                snippet,
            });
        }
    }

    // Local inference may consume L0 data. Cloud synthesis requires explicit
    // consent, redaction, and a non-L0 source set; otherwise research remains
    // available in its established extractive/degraded mode.
    let provider = state.llm();
    let l0_cloud_blocked = contains_l0
        && provider
            .as_ref()
            .map(|provider| !provider.is_local())
            .unwrap_or(false);
    let llm: Option<Arc<dyn LlmProvider>> =
        crate::routes::privacy::governed_llm(&state, contains_l0).ok();
    let llm_disabled = llm.is_none();

    // 2. web 搜（走 OutboundGate WebSearch；禁用 → 退化纯 vault，不报错）。
    let mut web_disabled_note = false;
    if req.use_web {
        match web {
            Some(ws) => {
                if let Ok(results) = ws.search(req.topic.trim(), 6) {
                    for r in results {
                        docs.push(ResearchDoc {
                            kind: SourceKind::Web,
                            reference: r.url,
                            title: r.title,
                            snippet: r.snippet.chars().take(400).collect(),
                        });
                    }
                }
            }
            None => web_disabled_note = true,
        }
    }

    // 3. 综合 + 跨源核实。
    let opts = ResearchOpts {
        use_web: web_enabled,
        ..Default::default()
    };
    let report = DeepResearch.run(req.topic.trim(), &docs, &opts, llm.as_deref());

    Ok(Json(serde_json::json!({
        "report_markdown": report.report_markdown,
        "claims": report.claims,
        "degraded": report.degraded,
        "web_disabled": web_disabled_note,
        // privacy.llm 关 → LLM 综合被门控掉（纯 vault extractive 降级），UI 可据此提示用户开启。
        "llm_disabled": llm_disabled,
        "l0_cloud_blocked": l0_cloud_blocked,
        "item_id": null,
        "cost_hint": { "tier": if report.degraded { "free" } else { "cloud" },
                       "note": "explicit deep research" }
    })))
}
