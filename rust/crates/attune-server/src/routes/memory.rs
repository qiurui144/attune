//! /api/v1/memory/* — 记忆延续路由（2026-06-15）。
//!
//! Task 4 范围：迁移状态查询 + reindex 暂停/恢复。导出/导入在 Task 7。
//! reindex 是 tier-2（只 re-embed summary，不调 LLM）→ 不走 member-gate。
//!
//! 见 `docs/superpowers/plans/2026-06-15-memory-continuity-and-portability.md` Task 4。

use attune_core::vault::VaultState;
use axum::extract::State;
use axum::Json;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

fn int(e: impl std::fmt::Display) -> AppError {
    AppError::Internal(e.to_string())
}

/// GET /api/v1/memory/migration/status
///
/// 报告当前 embedding 签名 + 仍需 reindex 的旧向量数（stale）+ 暂停态。stale=0
/// 表示所有记忆向量已对齐当前模型；>0 表示后台 loop 会逐批迁移（除非已暂停）。
pub async fn migration_status(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    // 当前签名取自 active embedding provider（与后台 reindex 用同一 SSOT）。
    let sig = state
        .embedding()
        .map(|e| attune_core::embed::current_embedding_signature(e.as_ref()))
        .ok_or_else(|| AppError::Internal("no embedding provider configured".into()))?;

    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(vault.state(), VaultState::Unlocked) {
        return Err(AppError::Forbidden("vault locked".into()));
    }
    let stale = vault.store().list_stale_memory_ids(&sig.model).map_err(int)?;

    Ok(Json(serde_json::json!({
        "current_model": sig.model,
        "current_dim": sig.dim,
        "stale": stale.len(),
        "paused": state.reindex_paused(),
    })))
}

/// POST /api/v1/memory/reindex   body: {"pause": bool}（缺省 pause=false）
///
/// 不直接驱动迁移——只翻转后台 loop 读取的暂停 flag。实际 reindex 由消费者
/// 后台 loop（run_memory_reindex_batch）逐批执行（vault unlocked + 未暂停时）。
/// 这样 reindex 与现有 memory layering 共用一个后台循环与锁序，不另起线程。
pub async fn reindex(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> AppResult<Json<serde_json::Value>> {
    let pause = body.get("pause").and_then(|b| b.as_bool()).unwrap_or(false);
    state.set_reindex_paused(pause);
    Ok(Json(serde_json::json!({ "running": !pause, "paused": pause })))
}
