//! 零成本主动建议引擎 — REST routes(能力 A 切片 A2)。
//!
//! `GET  /api/v1/suggestions`              → 当前活跃建议卡(未 dismiss / 未 mute)
//! `POST /api/v1/suggestions/{id}/dismiss` → 忽略单卡(id = signature)
//! `POST /api/v1/suggestions/mute`         → 永久关闭某 kind
//! `DELETE /api/v1/suggestions/mute`       → 恢复某 kind
//!
//! **成本契约**:本 route **零 LLM**。它只从 Store 读已有确定性信号,填充
//! [`SignalContext`](attune_core::suggestions::SignalContext)(纯数据,无 LLM 句柄),
//! 调 [`evaluate`](attune_core::suggestions::evaluate)(编译期无法偷跑 LLM)。建议卡
//! 实时重算不落库生成内容。点击卡的 action 由前端路由到既有显式 handler
//! (OrganizeWizard / Search / SkillEvolution / Profile),那里才带成本 UI。
//!
//! per spec docs/superpowers/specs/2026-06-17-suggestions-and-thirdparty-accounts.md
//! 能力 A §5 API 契约。

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;

use attune_core::organizer::types::OrganizationProposal;
use attune_core::suggestions::{
    self, ClusterCandidate, SignalContext, ENRICH_MISS_THRESHOLD, MAX_REF_IDS, ORGANIZE_MIN_GROUP,
};

use crate::error::AppResult;
use crate::routes::errors::{bad_request, internal, vault_locked};
use crate::state::SharedState;

/// 草稿提案最多投影这么多个 → 限制单次 evaluate 成本(纯防御)。
const MAX_DRAFT_PROPOSALS: usize = 20;
/// missed-query 聚合上限。
const MAX_MISSED_QUERIES: usize = 20;

/// GET /api/v1/suggestions — 返回当前活跃建议卡(JSON)。
pub async fn list(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    // 需要 dek 解密草稿提案(organize 信号源);locked 时直接退化为"无建议卡"。
    let dek = vault.dek_db().map_err(|_| vault_locked())?;
    let store = vault.store();

    // === 收集确定性信号(全部零成本读取,无 LLM)===
    let missed_queries = store
        .aggregate_missed_queries(ENRICH_MISS_THRESHOLD, MAX_MISSED_QUERIES)
        .unwrap_or_default();
    let unprocessed_search_miss =
        store.count_unprocessed_signals().unwrap_or(0).min(u32::MAX as usize) as u32;
    let browse_signal_count =
        store.browse_signals_count().unwrap_or(0).min(u32::MAX as usize) as u32;
    let annotation_marker_count = store
        .count_unprocessed_signals_by_kind("annotation_marker")
        .unwrap_or(0)
        .min(u32::MAX as usize) as u32;
    let cluster_candidates = gather_cluster_candidates(store, &dek);
    let muted_kinds = store.list_muted_suggestion_kinds().unwrap_or_default();
    let dismissed_signatures = store.list_dismissed_signatures().unwrap_or_default();
    // 已连接第三方源数量(ConnectSource 卡驱动)。Some(0) → 出连接卡(若用户活跃);
    // 读失败退化为 None(未评估,不出连接卡)—— 不因 store 抖动误弹。
    let connected_source_count = store
        .connected_provider_kinds()
        .ok()
        .map(|kinds| kinds.len().min(u32::MAX as usize) as u32);

    let ctx = SignalContext {
        missed_queries,
        unprocessed_search_miss,
        cluster_candidates,
        browse_signal_count,
        annotation_marker_count,
        muted_kinds,
        dismissed_signatures,
        connected_source_count,
    };

    let cards = suggestions::evaluate(&ctx);
    let json: Vec<serde_json::Value> = cards
        .iter()
        .map(|c| {
            serde_json::json!({
                "id": c.signature,
                "signature": c.signature,
                "kind": c.kind.as_str(),
                "title": c.title,
                "detail": c.detail,
                "ref_ids": c.ref_ids,
                "action_kind": c.action_kind.as_str(),
                "cost_hint": {
                    "tier": cost_tier_str(c.cost_tier),
                    "note": c.cost_note,
                },
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "cards": json })))
}

/// POST /api/v1/suggestions/{id}/dismiss — 忽略单卡(id = signature),幂等。
pub async fn dismiss(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|_| vault_locked())?;
    vault
        .store()
        .dismiss_suggestion(&id)
        .map_err(|e| bad_request(format!("dismiss failed: {e}")))?;
    Ok(Json(serde_json::json!({ "dismissed": id })))
}

#[derive(Deserialize)]
pub struct MuteBody {
    pub kind: String,
}

/// POST /api/v1/suggestions/mute — 永久关闭某 kind。
pub async fn mute(
    State(state): State<SharedState>,
    Json(body): Json<MuteBody>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|_| vault_locked())?;
    vault
        .store()
        .mute_suggestion_kind(&body.kind)
        .map_err(|e| bad_request(format!("mute failed: {e}")))?;
    Ok(Json(serde_json::json!({ "muted": body.kind })))
}

/// DELETE /api/v1/suggestions/mute — 恢复某 kind。
pub async fn unmute(
    State(state): State<SharedState>,
    Json(body): Json<MuteBody>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|_| vault_locked())?;
    vault
        .store()
        .unmute_suggestion_kind(&body.kind)
        .map_err(|e| internal("unmute_suggestion_kind", e))?;
    Ok(Json(serde_json::json!({ "unmuted": body.kind })))
}

/// 把草稿 organize 提案投影成 [`ClusterCandidate`](确定性,已算好的聚类,零成本)。
/// 仅取 draft 状态(已 apply/discard 的不再提示)。解密失败的单条静默跳过。
fn gather_cluster_candidates(
    store: &attune_core::store::Store,
    dek: &attune_core::crypto::Key32,
) -> Vec<ClusterCandidate> {
    let drafts = match store.list_proposals(Some("draft"), MAX_DRAFT_PROPOSALS as i64, 0) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for (id, _status, _created_at) in drafts {
        let json = match store.get_proposal(dek, &id) {
            Ok(Some((_status, json))) => json,
            _ => continue,
        };
        let proposal: OrganizationProposal = match serde_json::from_str(&json) {
            Ok(p) => p,
            Err(_) => continue,
        };
        for g in proposal.groups {
            if g.items.len() < ORGANIZE_MIN_GROUP {
                continue;
            }
            let item_ids: Vec<String> = g
                .items
                .iter()
                .map(|i| i.item_id.clone())
                .take(MAX_REF_IDS)
                .collect();
            // top_entities = group label(extractive,确定性;不调 LLM 命名)。
            let top_entities = if g.label.trim().is_empty() {
                vec![]
            } else {
                vec![g.label.clone()]
            };
            out.push(ClusterCandidate {
                item_ids,
                top_entities,
            });
        }
    }
    out
}

fn cost_tier_str(t: suggestions::CostTier) -> &'static str {
    match t {
        suggestions::CostTier::Free => "free",
        suggestions::CostTier::Local => "local",
        suggestions::CostTier::Cloud => "cloud",
    }
}
