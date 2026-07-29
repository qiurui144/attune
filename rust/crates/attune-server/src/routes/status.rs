use attune_core::vault::VaultState;
use axum::extract::State;
use axum::Json;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;
use attune_core::edge_cloud::capacity::DEFAULT_PROBE_TIMEOUT;
use attune_core::edge_cloud::scheduler::LocalSchedulerClient;

pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok"}))
}

fn status_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// D-R13 ARCH-A reference migration: 用 AppError + AppResult 代替 (StatusCode, Json)
/// tuple style. 客户端拿到统一 {"error": msg, "code": kebab} shape.
pub async fn status(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    let vault = state
        .vault
        .lock()
        .map_err(|_| AppError::Internal("vault lock poisoned".into()))?;
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

    let has_embedding = state
        .embedding
        .lock()
        .ok()
        .map(|g| g.is_some())
        .unwrap_or(false);
    let has_vectors = state
        .vectors
        .lock()
        .ok()
        .map(|g| g.is_some())
        .unwrap_or(false);
    let has_fulltext = state
        .fulltext
        .lock()
        .ok()
        .map(|g| g.is_some())
        .unwrap_or(false);

    Ok(Json(serde_json::json!({
        "state": vault_state,
        "items": items,
        "pending_embeddings": pending,
        "embedding_available": has_embedding,
        "vector_index": has_vectors,
        "fulltext_index": has_fulltext,
        "version": status_version(),
    })))
}

#[cfg(test)]
mod tests {
    #[test]
    fn status_version_uses_server_package_version() {
        assert_eq!(super::status_version(), env!("CARGO_PKG_VERSION"));
    }
}

#[derive(Debug, Default)]
struct SchedulerProbe {
    status: String,
    models: Vec<String>,
    error: Option<String>,
}

/// Probe scheduler observability only. Attune must not inspect concrete local
/// inference runtimes directly.
async fn probe_scheduler_models(base_url: String) -> SchedulerProbe {
    tokio::task::spawn_blocking(move || {
        let client = LocalSchedulerClient::with_base(&base_url, DEFAULT_PROBE_TIMEOUT);
        match client.models() {
            Ok(snapshot) => SchedulerProbe {
                status: "ready".to_string(),
                models: snapshot.models.into_iter().map(|m| m.name).collect(),
                error: None,
            },
            Err(e) => SchedulerProbe {
                status: "missing".to_string(),
                models: Vec::new(),
                error: Some(e.to_string()),
            },
        }
    })
    .await
    .unwrap_or_else(|e| SchedulerProbe {
        status: "missing".to_string(),
        models: Vec::new(),
        error: Some(format!("scheduler probe task join failed: {e}")),
    })
}

/// GET /api/v1/status/diagnostics — AI 后端健康检查
pub async fn diagnostics(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let vault_state = state
        .vault
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .state();

    let embedding_available = state
        .embedding
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some();
    let classifier_ready = state
        .classifier
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some();

    let chat_model = state
        .llm
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|l| l.model_name().to_string())
        .unwrap_or_default();

    let pending_tasks = if matches!(vault_state, VaultState::Unlocked) {
        state
            .vault
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .store()
            .pending_embedding_count()
            .unwrap_or(0)
    } else {
        0
    };

    let fulltext_ready = state
        .fulltext
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some();
    let vector_ready = state
        .vectors
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some();
    let tag_index_count = state
        .tag_index
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|i| i.item_count())
        .unwrap_or(0);

    // Determine overall AI status
    let ai_status = if classifier_ready && embedding_available {
        "ready"
    } else if embedding_available {
        "partial" // embedding works but no chat model for classification
    } else {
        "unavailable"
    };

    // 硬件画像：启动时已在 AppState 里检测过，这里零成本复用。
    // 前端用 hardware 字段显示"根据你的硬件推荐 xxx"并决定默认摘要模型。
    let hw = &state.hardware;
    const GB: u64 = 1024 * 1024 * 1024;

    let scheduler_base = crate::local_scheduler::base_from_state(&state);
    let scheduler_probe = probe_scheduler_models(scheduler_base.clone()).await;

    // AMD Ryzen AI NPU 细粒度状态 + consent-gated 安装计划(#6)。只读探测,零成本。
    // 非 AMD/无 NPU 主机 → null。
    let amd_npu = amd_npu_json();

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
        "scheduler": {
            "managed": true,
            "endpoint": scheduler_base,
            "status": scheduler_probe.status,
            "models": scheduler_probe.models,
            "error": scheduler_probe.error,
        },
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
            // form_factor 决定 LLM 默认路径：Laptop/Server/Unknown → 远端 token；LocalSchedulerAppliance → local-scheduler :8090 收口。
            "form_factor": match hw.form_factor {
                attune_core::platform::FormFactor::Laptop => "laptop",
                attune_core::platform::FormFactor::LocalSchedulerAppliance => "local_scheduler",
                attune_core::platform::FormFactor::Server => "server",
                attune_core::platform::FormFactor::Unknown => "unknown",
            },
            "prefers_local_llm": hw.form_factor.prefers_local_llm(),
        },
        // #6: AMD Ryzen AI NPU 细粒度状态 + consent-gated 安装计划(非 AMD/无 NPU → null)
        "amd_npu": amd_npu,
        "hint": if ai_status == "unavailable" {
            "请启动 local scheduler，并确认其 /models 与 /kb/tasks/* 可用"
        } else { "" }
    }))
}

/// 把 `NpuStatus` + 安装计划序列化成 diagnostics 用的 JSON;非 AMD Ryzen AI 主机 → null。
///
/// 暴露安装计划但**不执行**:每条 step 带 danger 等级 + consent_required,前端据此引导用户
/// (能安全自动的 safe-auto / 需同意的 needs-consent / 纯手工的 manual),由用户同意后执行。
fn amd_npu_json() -> serde_json::Value {
    use attune_core::platform::NpuStatus;
    let Some(npu) = NpuStatus::detect_amd() else {
        return serde_json::Value::Null;
    };
    let plan = npu.install_plan();
    let steps: Vec<_> = plan
        .steps
        .iter()
        .map(|s| {
            serde_json::json!({
                "description": s.description,
                "command": s.command,
                "danger": s.danger.as_str(),
                "consent_required": s.consent_required,
            })
        })
        .collect();
    serde_json::json!({
        "chip_id": npu.chip_id,
        "chip_name": npu.chip_name,
        "xdna_version": npu.xdna_version,
        "tops": npu.tops,
        "ready": npu.is_ready(),
        "driver_loaded": npu.driver_loaded,
        "firmware_present": npu.firmware_present,
        "device_node_present": npu.device_node_present,
        "kernel_ok": npu.kernel_ok,
        "current_kernel": npu.current_kernel,
        "min_kernel": npu.min_kernel,
        "iommu_ok": npu.iommu_ok,
        "summary": npu.summary(),
        "install_plan": {
            "missing": plan.missing,
            "steps": steps,
        }
    })
}
