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

/// Probe Ollama at localhost:11434, return (status, model_names).
async fn probe_ollama() -> (&'static str, Vec<String>) {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap_or_default();
    match client.get("http://localhost:11434/api/tags").send().await {
        Ok(resp) if resp.status().is_success() => {
            let models: Vec<String> = resp
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v.get("models").cloned())
                .and_then(|m| serde_json::from_value(m).ok())
                .map(|arr: Vec<serde_json::Value>| {
                    arr.iter()
                        .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            ("ready", models)
        }
        _ => ("missing", vec![]),
    }
}

/// F-16 hardware utilization: probe Ollama `/api/ps` to detect actual GPU usage.
///
/// `/api/ps` returns models currently loaded in memory, with `size_vram` field
/// indicating VRAM allocation. If any model has `size_vram > 0`, the Ollama
/// runtime is using GPU acceleration. If all loaded models have `size_vram = 0`,
/// Ollama is falling back to CPU (the user may have expected GPU).
///
/// Returns:
/// - `None` — Ollama unreachable, or no models currently loaded (can't infer)
/// - `Some(true)` — at least one loaded model uses GPU VRAM
/// - `Some(false)` — Ollama is loaded but no model uses GPU (CPU fallback)
///
/// This complements `probe_ollama()` (which only checks HTTP reachability +
/// installed model list) — actual GPU vs CPU is only knowable when a model
/// is in memory, which happens after the first chat request.
async fn probe_ollama_gpu_active() -> Option<bool> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap_or_default();
    let resp = client.get("http://localhost:11434/api/ps").send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    let models = body.get("models")?.as_array()?;
    if models.is_empty() {
        // No model currently loaded — can't infer GPU usage. Return None.
        return None;
    }
    // any model with size_vram > 0 → GPU active
    let any_gpu = models
        .iter()
        .any(|m| m.get("size_vram").and_then(|v| v.as_u64()).unwrap_or(0) > 0);
    Some(any_gpu)
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

    let (ollama_status, ollama_models) = probe_ollama().await;
    // F-16: probe actual GPU usage (only meaningful when a model is currently loaded)
    let ollama_gpu_active = probe_ollama_gpu_active().await;

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
        "ollama_status": ollama_status,
        "ollama_models": ollama_models,
        // F-16 hardware utilization: actual GPU usage probe
        // - null: Ollama unreachable OR no model currently loaded (probe inconclusive)
        // - true: at least one loaded model has size_vram > 0 (GPU acceleration confirmed)
        // - false: model(s) loaded but all size_vram = 0 (CPU fallback — user may want to investigate)
        "ollama_gpu_active": ollama_gpu_active,
        "hardware": {
            "os": hw.os,
            "cpu_model": hw.cpu_model,
            "cpu_vendor": hw.cpu_vendor,
            "total_ram_gb": hw.total_ram_bytes / GB,
            "has_nvidia_gpu": hw.has_nvidia_gpu,
            "has_amd_gpu": hw.has_amd_gpu,
            "has_intel_igpu": hw.has_intel_igpu,
            "gpu_label": hw.gpu_label,
            "amd_gfx_target": hw.amd_gfx_target,
            "has_amd_xdna_npu": hw.has_amd_xdna_npu,
            "has_intel_npu": hw.has_intel_npu,
            "has_accelerator": hw.has_accelerator(),
            "recommended_summary_model": hw.recommended_summary_model(),
            // form_factor 决定 LLM 默认路径：Laptop/Server/Unknown → 远端 token；K3Appliance → 本地 Ollama
            "form_factor": match hw.form_factor {
                attune_core::platform::FormFactor::Laptop => "laptop",
                attune_core::platform::FormFactor::K3Appliance => "k3",
                attune_core::platform::FormFactor::Server => "server",
                attune_core::platform::FormFactor::Unknown => "unknown",
            },
            "prefers_local_llm": hw.form_factor.prefers_local_llm(),
        },
        "hint": if ai_status == "unavailable" {
            "安装 Ollama 获取 AI 分类能力: curl -fsSL https://ollama.com/install.sh | sh && ollama pull <轻量本地模型>"
        } else { "" }
    }))
}
