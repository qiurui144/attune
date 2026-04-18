use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use attune_core::vault::VaultState;

use crate::state::SharedState;

pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok"}))
}

pub async fn status(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "vault lock poisoned"})),
        )
    })?;
    let vault_state = vault.state();

    let (items, pending) = if matches!(vault_state, VaultState::Unlocked) {
        let items = vault.store().item_count().unwrap_or(0);
        let pending = vault.store().pending_embedding_count().unwrap_or(0);
        (items, pending)
    } else {
        (0, 0)
    };
    // Drop vault lock before accessing other mutexes
    drop(vault);

    let has_embedding = state.embedding.lock().ok().map(|g| g.is_some()).unwrap_or(false);
    let has_vectors = state.vectors.lock().ok().map(|g| g.is_some()).unwrap_or(false);
    let has_fulltext = state.fulltext.lock().ok().map(|g| g.is_some()).unwrap_or(false);

    Ok(Json(serde_json::json!({
        "state": vault_state,
        "items": items,
        "pending_embeddings": pending,
        "embedding_available": has_embedding,
        "vector_index": has_vectors,
        "fulltext_index": has_fulltext,
        "version": attune_core::version(),
    })))
}

/// GET /api/v1/status/diagnostics — AI 后端健康检查
pub async fn diagnostics(
    State(state): State<SharedState>,
) -> Json<serde_json::Value> {
    let vault_state = state.vault.lock().unwrap_or_else(|e| e.into_inner()).state();

    let embedding_available = state.embedding.lock().unwrap_or_else(|e| e.into_inner()).is_some();
    let classifier_ready = state.classifier.lock().unwrap_or_else(|e| e.into_inner()).is_some();

    let chat_model = state.llm.lock().unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|l| l.model_name().to_string())
        .unwrap_or_default();

    let pending_tasks = if matches!(vault_state, VaultState::Unlocked) {
        state.vault.lock().unwrap_or_else(|e| e.into_inner()).store().pending_embedding_count().unwrap_or(0)
    } else { 0 };

    let fulltext_ready = state.fulltext.lock().unwrap_or_else(|e| e.into_inner()).is_some();
    let vector_ready = state.vectors.lock().unwrap_or_else(|e| e.into_inner()).is_some();
    let tag_index_count = state.tag_index.lock().unwrap_or_else(|e| e.into_inner())
        .as_ref().map(|i| i.item_count()).unwrap_or(0);

    // Determine overall AI status
    let ai_status = if classifier_ready && embedding_available {
        "ready"
    } else if embedding_available {
        "partial"  // embedding works but no chat model for classification
    } else {
        "unavailable"
    };

    // 硬件画像：启动时已在 AppState 里检测过，这里零成本复用。
    // 前端用 hardware 字段显示"根据你的硬件推荐 xxx"并决定默认摘要模型。
    let hw = &state.hardware;
    const GB: u64 = 1024 * 1024 * 1024;

    Json(serde_json::json!({
        "vault_state": vault_state,
        "ai_status": ai_status,
        "embedding_available": embedding_available,
        "classifier_ready": classifier_ready,
        "chat_model": chat_model,
        "fulltext_ready": fulltext_ready,
        "vector_ready": vector_ready,
        "tag_index_items": tag_index_count,
        "pending_tasks": pending_tasks,
        "hardware": {
            "os": hw.os,
            "cpu_model": hw.cpu_model,
            "cpu_vendor": hw.cpu_vendor,
            "total_ram_gb": hw.total_ram_bytes / GB,
            "has_nvidia_gpu": hw.has_nvidia_gpu,
            "has_amd_gpu": hw.has_amd_gpu,
            "amd_gfx_target": hw.amd_gfx_target,
            "has_amd_xdna_npu": hw.has_amd_xdna_npu,
            "has_intel_npu": hw.has_intel_npu,
            "has_accelerator": hw.has_accelerator(),
            "recommended_summary_model": hw.recommended_summary_model(),
        },
        "hint": if ai_status == "unavailable" {
            "安装 Ollama 获取 AI 分类能力: curl -fsSL https://ollama.com/install.sh | sh && ollama pull qwen2.5:3b"
        } else { "" }
    }))
}
