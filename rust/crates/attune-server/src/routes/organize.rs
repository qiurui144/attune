//! 文件夹一键智能整理 → 案卷 — analyze 路由(Task 8)。
//!
//! `POST /api/v1/organize/analyze`:gather(item ids → 标题/摘要 + 向量)→ member-gate
//! (有付费会员 → tier-3 LLM 命名;否则 tier-2 extractive)→ `analyze_items` 引擎 →
//! 缓存提案(加密)→ 返回 proposal JSON。用户随后在审核面板确认,再走 apply(Task 9)。
//!
//! 锁序铁律:vectors 必须先于 vault 取(规范序 fulltext → vectors → vault,与
//! search/chat 热点路径一致;反序持锁 = ABBA 死锁)。本 handler 镜像 clusters::rebuild。
//!
//! Task 9 追加 apply/get_one/list/delete_one(用户审核后归档 + 提案生命周期)。

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use attune_core::llm::LlmProvider;
use attune_core::organizer::analyze_items;
use attune_core::organizer::types::ItemView;
use attune_core::vault::VaultState;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

/// gather 圈选范围:按 item_id 列表,或按文件系统路径前缀(文件夹一键整理)。
#[derive(Deserialize)]
#[serde(untagged)]
pub enum Scope {
    ByItems { item_ids: Vec<String> },
    ByPath { bind_path: String },
}

#[derive(Deserialize, Default)]
pub struct AnalyzeOpts {
    pub min_cluster_size: Option<usize>,
}

#[derive(Deserialize)]
pub struct AnalyzeReq {
    pub scope: Scope,
    pub corpus_domain: Option<String>,
    #[serde(default)]
    pub options: Option<AnalyzeOpts>,
}

/// HDBSCAN 命名/聚类默认下限(与 clusters::rebuild 一致语义;< 此走 fallback 子目录)。
const DEFAULT_MIN_CLUSTER_SIZE: usize = 20;
/// 单次整理 item 上限(防大 vault 持锁超时 / proposal 巨大)。
const ORGANIZE_ITEM_CAP: usize = 10_000;

pub async fn analyze(
    State(state): State<SharedState>,
    Json(req): Json<AnalyzeReq>,
) -> AppResult<Json<serde_json::Value>> {
    // 1. gather item ids(ByItems 直接;ByPath → indexed_files 前缀子树)。
    //    取 dek + ids 时短取 vault 锁,语句结束即释放,后续 gather 再按规范序取锁。
    let (item_ids, dek) = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(vault.state(), VaultState::Unlocked) {
            return Err(AppError::Forbidden("vault locked".into()));
        }
        let dek = vault.dek_db().map_err(|e| AppError::Forbidden(e.to_string()))?;
        let ids = match &req.scope {
            Scope::ByItems { item_ids } => item_ids.clone(),
            Scope::ByPath { bind_path } => vault
                .store()
                .list_item_ids_under_path(bind_path)
                .map_err(int)?,
        };
        (ids, dek)
    };
    if item_ids.is_empty() {
        return Err(AppError::BadRequest("empty-scope".into()));
    }
    if item_ids.len() > ORGANIZE_ITEM_CAP {
        return Err(AppError::BadRequest(format!(
            "scope too large: {} items (max {ORGANIZE_ITEM_CAP})",
            item_ids.len()
        )));
    }

    // 2. 组 ItemView:title/snippet 从 store(解密);embedding 从 vector index。
    //    规范锁序 fulltext → vectors → vault:vectors 必须在 vault 之前取。
    //    无向量的 item 不丢 —— ItemView.embedding=None,引擎会把它归入 noise。
    let mut items: Vec<ItemView> = Vec::with_capacity(item_ids.len());
    {
        let vecs = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let vecs_ref = vecs.as_ref();
        for id in &item_ids {
            // item 可能已被删除 / 不存在 → 跳过(不让一个坏 id 整体失败)。
            let Ok(Some(item)) = vault.store().get_item(&dek, id) else {
                continue;
            };
            let snippet: String = item.content.chars().take(200).collect();
            let embedding = vecs_ref.and_then(|v| v.get_vector(id));
            items.push(ItemView {
                item_id: item.id,
                title: item.title,
                content_snippet: snippet,
                // dir 仅用于"向量不足时按子目录 fallback";per-item 目录无廉价反查,
                // 留空 → fallback 退化为单组,主路径(HDBSCAN on vectors)不受影响。
                dir: String::new(),
                embedding,
            });
        }
    }
    if items.is_empty() {
        // 所有 id 都查不到(已删除 / 无效)→ 与空 scope 同等对待。
        return Err(AppError::BadRequest("empty-scope".into()));
    }

    // 3. member-gate:仅付费会员 + 已配置 LLM 才走 tier-3 strategy 命名;
    //    否则 llm=None → 引擎走 tier-2 extractive(标题词频)命名。
    let is_paid = state
        .member_state
        .lock()
        .map(|g| g.is_paid())
        .unwrap_or(false);
    let llm: Option<std::sync::Arc<dyn LlmProvider>> = if is_paid {
        state
            .llm
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .cloned()
    } else {
        None
    };

    let registry = state.strategy_registry();
    let strategy = registry.resolve(req.corpus_domain.as_deref());
    let min_cluster_size = req
        .options
        .as_ref()
        .and_then(|o| o.min_cluster_size)
        .unwrap_or(DEFAULT_MIN_CLUSTER_SIZE);

    let proposal_id = uuid::Uuid::new_v4().to_string();
    let proposal = analyze_items(
        proposal_id.clone(),
        req.corpus_domain.clone(),
        items,
        strategy.as_ref(),
        llm.as_deref(),
        min_cluster_size,
    )
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // 4. 缓存提案(加密)+ 顺手清理过期 draft。scope 摘要不含敏感全文,仅圈选元数据。
    let proposal_json = serde_json::to_string(&proposal).map_err(int)?;
    let scope_summary = scope_summary_json(&req.scope, proposal.all_item_ids().len());
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let store = vault.store();
        let _ = store.cleanup_stale_proposals();
        store
            .save_proposal(
                &dek,
                &proposal_id,
                &scope_summary,
                req.corpus_domain.as_deref(),
                &proposal_json,
            )
            .map_err(int)?;
    }

    // 5. 响应:非付费会员附 tier 提示 code(前端据此引导升级以解锁 LLM 命名)。
    let mut val = serde_json::to_value(&proposal).map_err(int)?;
    if !is_paid {
        val["code"] = serde_json::json!("member-required-for-llm-label");
    }
    Ok(Json(val))
}

/// scope 摘要:只记圈选方式 + item 计数,**不含**敏感文件标题/路径全文。
fn scope_summary_json(scope: &Scope, item_count: usize) -> String {
    let v = match scope {
        Scope::ByItems { .. } => {
            serde_json::json!({ "kind": "by-items", "item_count": item_count })
        }
        Scope::ByPath { .. } => {
            serde_json::json!({ "kind": "by-path", "item_count": item_count })
        }
    };
    v.to_string()
}

/// 用户审核后确认的整理动作。action: "create" | "add-to" | "skip"。
#[derive(Deserialize)]
pub struct ApplyReq {
    pub proposal_id: String,
    #[serde(default)]
    pub confirmed_groups: Vec<CgReq>,
}

#[derive(Deserialize)]
pub struct CgReq {
    #[allow(dead_code)] // group_id 仅前端追踪;归档以 items 为准。
    pub group_id: Option<i32>,
    pub action: String,
    pub project_id: Option<String>,
    pub title: Option<String>,
    pub kind: Option<String>,
    #[serde(default)]
    pub items: Vec<CgItem>,
}

#[derive(Deserialize)]
pub struct CgItem {
    pub item_id: String,
    pub role: Option<String>,
}

/// `POST /api/v1/organize/apply`:把确认后的分组批量归入案卷。单事务幂等(Task 7),
/// 重复 apply 同 proposal_id 返回 already_applied。归档动作不涉及加密字段 → 无需 dek。
pub async fn apply(
    State(state): State<SharedState>,
    Json(req): Json<ApplyReq>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(vault.state(), VaultState::Unlocked) {
        return Err(AppError::Forbidden("vault locked".into()));
    }
    let groups: Vec<attune_core::store::ConfirmedGroup> = req
        .confirmed_groups
        .into_iter()
        .map(|g| attune_core::store::ConfirmedGroup {
            action: g.action,
            project_id: g.project_id,
            title: g.title,
            kind: g.kind,
            items: g
                .items
                .into_iter()
                .map(|i| (i.item_id, i.role.unwrap_or_default()))
                .collect(),
        })
        .collect();
    let r = vault
        .store()
        .apply_proposal(&req.proposal_id, &groups)
        .map_err(int)?;
    Ok(Json(serde_json::json!({
        "created": r.created,
        "updated": r.updated,
        "filed_count": r.filed_count,
        "skipped_count": r.skipped_count,
        "already_applied": r.already_applied,
    })))
}

/// `GET /api/v1/organize/proposals/{id}`:返回解密后的提案明文 JSON。
/// proposal_json 加密存储 → 需 dek 解密(照 analyze handler 的 dek_db() 取法)。
pub async fn get_one(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(vault.state(), VaultState::Unlocked) {
        return Err(AppError::Forbidden("vault locked".into()));
    }
    let dek = vault
        .dek_db()
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    match vault.store().get_proposal(&dek, &id).map_err(int)? {
        Some((_status, json)) => Ok(Json(serde_json::from_str(&json).map_err(int)?)),
        None => Err(AppError::NotFound("proposal not found".into())),
    }
}

/// `GET /api/v1/organize/proposals`:分页列出提案元数据(id/status/created_at)。
/// 列表不解密内容(降明文暴露面);查询参 status/limit/offset。无需 dek。
pub async fn list(
    State(state): State<SharedState>,
    Query(q): Query<HashMap<String, String>>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(vault.state(), VaultState::Unlocked) {
        return Err(AppError::Forbidden("vault locked".into()));
    }
    let limit: i64 = q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(20);
    let offset: i64 = q.get("offset").and_then(|s| s.parse().ok()).unwrap_or(0);
    let rows = vault
        .store()
        .list_proposals(q.get("status").map(|s| s.as_str()), limit, offset)
        .map_err(int)?;
    Ok(Json(serde_json::json!(rows
        .into_iter()
        .map(|(id, status, created_at)| serde_json::json!({
            "id": id,
            "status": status,
            "created_at": created_at,
        }))
        .collect::<Vec<_>>())))
}

/// `DELETE /api/v1/organize/proposals/{id}`:废弃提案(status='discarded',保留审计)。无需 dek。
pub async fn delete_one(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(vault.state(), VaultState::Unlocked) {
        return Err(AppError::Forbidden("vault locked".into()));
    }
    vault.store().discard_proposal(&id).map_err(int)?;
    Ok(StatusCode::NO_CONTENT)
}

fn int(e: impl std::fmt::Display) -> AppError {
    AppError::Internal(e.to_string())
}
