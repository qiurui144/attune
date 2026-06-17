//! /api/v1/memory/* — 记忆延续路由（2026-06-15）。
//!
//! Task 4 范围：迁移状态查询 + reindex 暂停/恢复。导出/导入在 Task 7。
//! reindex 是 tier-2（只 re-embed summary，不调 LLM）→ 不走 member-gate。
//!
//! 见 `docs/superpowers/plans/2026-06-15-memory-continuity-and-portability.md` Task 4。

use attune_core::error::VaultError;
use attune_core::vault::VaultState;
use axum::extract::{Multipart, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

/// 口令长度下限。bundle 是离线可暴破的全量记忆密文,过短口令等于无加密。
const MIN_PASSPHRASE_LEN: usize = 8;

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

/// POST /api/v1/memory/export   body: {"passphrase": "..."}
///
/// 返回口令加密的全量记忆 bundle(二进制)。WHY 二进制而非 JSON:bundle 是
/// `[manifest]\n[salt][AES-256-GCM ciphertext]` 的字节流,前端直接当文件下载。
/// 口令下限在路由层先拦(< 8 → 400),避免把弱口令送进 KDF 产出"看似加密"的弱包。
pub async fn export(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> AppResult<Response> {
    let pw = body
        .get("passphrase")
        .and_then(|p| p.as_str())
        .ok_or_else(|| AppError::BadRequest("passphrase required".into()))?;
    if pw.len() < MIN_PASSPHRASE_LEN {
        return Err(AppError::BadRequest("passphrase too short".into()));
    }

    // dek 必须在 vault Unlocked 时取;锁内只取 dek + 调 export(不与 vectors/fulltext 交叠)。
    let bundle = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(vault.state(), VaultState::Unlocked) {
            return Err(AppError::Forbidden("vault locked".into()));
        }
        let dek = vault.dek_db().map_err(int)?;
        attune_core::memory::portability::export_memory_bundle(vault.store(), &dek, pw)
            .map_err(int)?
    };

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"attune-memory.bundle\"",
            ),
        ],
        bundle,
    )
        .into_response())
}

/// POST /api/v1/memory/import   multipart: file (bundle bytes) + passphrase
///
/// 合并式导入(幂等去重)。WHY vault 必须 Unlocked 才拒收(而非 export 那样 403):
/// import 要把记忆用本机 DEK 重加密入库,Sealed(未建库)/Locked 都拿不到 DEK,
/// 故统一 400 `vault-not-ready` 引导用户"先建库并解锁",这是可操作的前置条件错误。
///
/// 错误映射(handler 显式,不走 From<VaultError> —— 后者把 InvalidPassword 映 401):
///   InvalidPassword → 400 bad-passphrase;含 "unsupported"/"corrupt" 的结构错 → 400 对应 code。
pub async fn import(
    State(state): State<SharedState>,
    mut multipart: Multipart,
) -> AppResult<Json<serde_json::Value>> {
    // 先解析 multipart(不持锁),照 routes/upload.rs 的 next_field 模式。
    let mut bundle: Option<Vec<u8>> = None;
    let mut passphrase: Option<String> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        match field.name() {
            Some("file") => {
                bundle = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| AppError::BadRequest(e.to_string()))?
                        .to_vec(),
                );
            }
            Some("passphrase") => {
                passphrase = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError::BadRequest(e.to_string()))?,
                );
            }
            _ => {} // 忽略未知字段
        }
    }
    let bundle = bundle.ok_or_else(|| AppError::BadRequest("file field required".into()))?;
    let passphrase =
        passphrase.ok_or_else(|| AppError::BadRequest("passphrase field required".into()))?;

    // vault 必须 Unlocked,否则 400 vault-not-ready(Sealed/Locked 都拒,引导先建库解锁)。
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(vault.state(), VaultState::Unlocked) {
        return Err(AppError::BadRequest("vault-not-ready".into()));
    }
    let dek = vault.dek_db().map_err(int)?;

    let r =
        attune_core::memory::portability::import_memory_bundle(vault.store(), &dek, &bundle, &passphrase)
            .map_err(|e| match &e {
                // 口令错:portability 走 crypto::decrypt → InvalidPassword。显式映 400
                // bad-passphrase(From<VaultError> 默认会映 401,客户端无法区分"权限"vs"口令")。
                VaultError::InvalidPassword => AppError::BadRequest("bad-passphrase".into()),
                VaultError::InvalidInput(s) if s.contains("unsupported") => {
                    AppError::BadRequest("unsupported-bundle-version".into())
                }
                VaultError::InvalidInput(s) if s.contains("corrupt") => {
                    AppError::BadRequest("corrupt-bundle".into())
                }
                _ => AppError::Internal(e.to_string()),
            })?;

    Ok(Json(serde_json::json!({
        "imported": r.imported,
        "merged": r.merged,
        "skipped": r.skipped,
    })))
}
