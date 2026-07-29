//! AI 底座状态 API（v0.6.0-rc.3，2026-04-27）。
//!
//! per CLAUDE.md "本地 AI 底座边界" 决策：本地仅捆绑必要底座（Embedding / Rerank /
//! OCR / ASR / TTS），LLM 走远端 token 默认。
//!
//! 本 route 暴露各底座的可用性 + 模型名 / 后端路径 — 让 Settings UI 简洁地显示
//! 是否加载，无需让用户配置（默认全部自动检测 / 加载）。

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::json;
use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

fn note(available: bool, msg: &str) -> Option<String> {
    if available {
        None
    } else {
        Some(msg.to_string())
    }
}

fn model_is_ready(model: &attune_core::edge_cloud::SchedulerModelStatus) -> bool {
    crate::routes::voice::scheduler_model_ready(model)
}

fn task_capabilities(task_name: &str) -> &'static [&'static str] {
    match task_name {
        "kb.query.ask" | "kb.query.ask_hq" | "kb.query.answer" => &["chat"],
        "kb.document.summary" | "kb.document.long_summary" | "doc.summarize" => &["summary"],
        "kb.query.embed" | "kb.ingest.embed_batch" => &["embedding"],
        "kb.query.rerank" | "kb.ingest.rerank_batch" => &["rerank"],
        "kb.document.ocr_detect" | "kb.document.ocr_recognize" => &["ocr"],
        "kb.meeting.asr_frontend" => &["asr"],
        "kb.speech.synthesize" => &["tts"],
        "kb.query.vlm_extract"
        | "kb.document.extract"
        | "kb.document.intel"
        | "kb.document.vlm_extract"
        | "doc.extract" => &["vlm"],
        _ => &[],
    }
}

#[cfg(test)]
fn task_has_capability(task_name: &str, capability: &str) -> bool {
    task_capabilities(task_name)
        .iter()
        .any(|cap| *cap == capability)
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn dynamic_model_capabilities(
    scheduler: &crate::local_scheduler::SchedulerRuntimeProbe,
    settings: &Value,
    llm_configured: bool,
) -> Vec<Value> {
    let mut rows = scheduler
        .models
        .iter()
        .filter(|model| !model.name.trim().is_empty())
        .map(|model| {
            let mut capabilities = Vec::<String>::new();
            let mut task_names = Vec::<String>::new();
            for task in scheduler
                .runtime_tasks
                .iter()
                .filter(|task| task.model == model.name)
            {
                push_unique(&mut task_names, &task.name);
                for cap in task_capabilities(&task.name) {
                    push_unique(&mut capabilities, cap);
                }
            }
            json!({
                "name": model.name,
                "source": "scheduler",
                "capability_source": "scheduler-runtime-task",
                "capabilities": capabilities,
                "ready": model_is_ready(model),
                "state": model.state,
                "lifecycle": model.lifecycle,
                "dispatchable": model.dispatchable,
                "queue_depth": model.queue_depth,
                "queue_capacity": model.queue_capacity,
                "task_names": task_names,
            })
        })
        .collect::<Vec<_>>();

    if rows.is_empty() {
        let configured_model = settings
            .pointer("/llm/model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty());
        if let Some(model) = configured_model {
            rows.push(json!({
                "name": model,
                "source": "attune-settings",
                "capability_source": "attune-llm-settings",
                "capabilities": ["chat", "summary"],
                "ready": llm_configured,
                "state": if llm_configured { "configured" } else { "missing" },
                "lifecycle": if llm_configured { "ready" } else { "unknown" },
                "dispatchable": if llm_configured { "FREE" } else { "UNAVAILABLE" },
                "task_names": [],
            }));
        }
    }

    rows
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModelCapabilityGate {
    Allowed,
    ModelNotFound,
    CapabilityUnsupported { available: Vec<String> },
    ModelNotReady,
}

pub(crate) fn model_capability_gate_result(
    rows: &[Value],
    model: &str,
    capability: &str,
) -> ModelCapabilityGate {
    let requested_model = model.trim();
    let Some(row) = rows
        .iter()
        .find(|row| row.get("name").and_then(Value::as_str) == Some(requested_model))
    else {
        return ModelCapabilityGate::ModelNotFound;
    };
    let capabilities = row
        .get("capabilities")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !capabilities.iter().any(|cap| cap == capability) {
        return ModelCapabilityGate::CapabilityUnsupported {
            available: capabilities,
        };
    }
    if row.get("ready").and_then(Value::as_bool) != Some(true) {
        return ModelCapabilityGate::ModelNotReady;
    }
    ModelCapabilityGate::Allowed
}

fn model_capability_gate_error(
    rows: &[Value],
    model: &str,
    capability: &str,
    outcome: ModelCapabilityGate,
) -> AppError {
    let available_models = rows
        .iter()
        .filter_map(|row| row.get("name").and_then(Value::as_str).map(str::to_string))
        .collect::<Vec<_>>();
    let row = rows
        .iter()
        .find(|row| row.get("name").and_then(Value::as_str) == Some(model));
    let ready = row
        .and_then(|row| row.get("ready"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let lifecycle = row
        .and_then(|row| row.get("lifecycle"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let dispatchable = row
        .and_then(|row| row.get("dispatchable"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let (status, error, code, available_capabilities) = match outcome {
        ModelCapabilityGate::Allowed => unreachable!("allowed gate is not an error"),
        ModelCapabilityGate::ModelNotFound => (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("model '{model}' is not known to Attune"),
            "model-not-found",
            Vec::new(),
        ),
        ModelCapabilityGate::CapabilityUnsupported { available } => (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("model '{model}' does not support capability '{capability}'"),
            "model-capability-unsupported",
            available,
        ),
        ModelCapabilityGate::ModelNotReady => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("model '{model}' is not ready for capability '{capability}'"),
            "model-not-ready",
            row.and_then(|row| row.get("capabilities"))
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        ),
    };
    AppError::detailed(
        status,
        json!({
            "error": error,
            "code": code,
            "model": model,
            "capability": capability,
            "ready": ready,
            "lifecycle": lifecycle,
            "dispatchable": dispatchable,
            "available_capabilities": available_capabilities,
            "available_models": available_models,
        }),
    )
}

pub(crate) async fn require_model_capability_ready(
    state: &SharedState,
    model: &str,
    capability: &str,
) -> AppResult<()> {
    let model = model.trim();
    if model.is_empty() {
        return Err(AppError::Unprocessable("model cannot be empty".into()));
    }
    let scheduler_base = crate::local_scheduler::base_from_state(state);
    let scheduler = crate::local_scheduler::probe_scheduler_runtime(scheduler_base).await;
    let settings = state
        .vault
        .lock()
        .ok()
        .and_then(|vault| vault.store().get_meta("app_settings").ok().flatten())
        .and_then(|data| serde_json::from_slice::<Value>(&data).ok())
        .unwrap_or_else(|| json!({}));
    let llm_configured = state.llm.lock().ok().map(|g| g.is_some()).unwrap_or(false);
    let rows = dynamic_model_capabilities(&scheduler, &settings, llm_configured);
    match model_capability_gate_result(&rows, model, capability) {
        ModelCapabilityGate::Allowed => Ok(()),
        outcome => Err(model_capability_gate_error(
            &rows, model, capability, outcome,
        )),
    }
}

/// GET /api/v1/ai_stack — 返各底座状态 + 硬件 tier + 模型推荐 + region
pub async fn status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let scheduler_base = crate::local_scheduler::base_from_state(&state);
    let scheduler = crate::local_scheduler::probe_scheduler_runtime(scheduler_base.clone()).await;
    let settings = state
        .vault
        .lock()
        .ok()
        .and_then(|vault| vault.store().get_meta("app_settings").ok().flatten())
        .and_then(|data| serde_json::from_slice::<Value>(&data).ok())
        .unwrap_or_else(|| json!({}));
    let embedding_loaded = state
        .embedding
        .lock()
        .ok()
        .map(|g| g.is_some())
        .unwrap_or(false);
    let rerank_loaded = state
        .reranker
        .lock()
        .ok()
        .map(|g| g.is_some())
        .unwrap_or(false);
    let llm_configured = state.llm.lock().ok().map(|g| g.is_some()).unwrap_or(false);
    // web_search readiness mirrors the actual decision in routes/chat.rs:
    // state.web_search is Some iff a usable browser was auto-detected (or an
    // explicit browser_path was set and verified). Checking the same Arc means
    // the status here stays in sync with whether chat web-search would succeed.
    let web_search_available = state
        .web_search
        .lock()
        .ok()
        .map(|g| g.is_some())
        .unwrap_or(false);

    let has_scheduler_task = |task: &str| scheduler.tasks.iter().any(|t| t == task);
    let ocr_available = has_scheduler_task("kb.document.ocr_detect")
        && has_scheduler_task("kb.document.ocr_recognize");
    let ocr_engine = if ocr_available {
        "scheduler:kb.document.ocr_*"
    } else {
        "scheduler"
    };
    let voice_readiness = crate::routes::voice::scheduler_voice_readiness(&scheduler);
    let asr_engine = if voice_readiness.asr.available {
        "scheduler:kb.meeting.asr_frontend"
    } else {
        "scheduler"
    };
    state.set_tts_capability_ready(voice_readiness.tts.available);
    let scheduler_models = scheduler
        .models
        .iter()
        .map(|model| model.name.clone())
        .collect::<Vec<_>>();
    let model_capabilities = dynamic_model_capabilities(&scheduler, &settings, llm_configured);

    // v0.6.0-rc.4: 硬件 tier + 模型推荐 + region
    let hw = &state.hardware;
    let tier = attune_core::platform::classify_hardware(hw);
    let recommendation = attune_core::platform::ModelRecommendation::for_tier(tier);
    let region = attune_core::platform::detect_region();
    let passmark = attune_core::platform::cpu_db::lookup(&hw.cpu_model).map(|e| e.passmark);
    let npu_tops = attune_core::platform::cpu_db::lookup(&hw.cpu_model).and_then(|e| e.npu_tops);

    // 统一加速器视图：枚举本机所有推理加速器 (CPU/NVIDIA/AMD GPU+NPU/Intel iGPU+NPU)
    // + 每个的就绪度，并给底座 ONNX EP 选择提示 (recommended_ep_hint, 仅硬件视角建议)。
    let accel = attune_core::platform::AccelCapabilities::from_profile(hw);

    // 实际 EP 选型链(硬件 × 当前 artifact 编入的 EP × ATTUNE_ORT_EP env)→ 有序链,
    // 末位永远 CPU。telemetry 给 UI 显示「embedding 预计跑在 cuda / cpu」+ fallback 原因。
    let ep_sel = attune_core::infer::accel::cached_selection();
    let ep_chain = ep_sel.recommend_ep_chain();
    let ep_telemetry = attune_core::infer::accel::EpSelectionTelemetry::from_chain(&ep_chain);

    Json(json!({
        "hardware": {
            "tier": tier.label(),
            "supported": tier.is_supported(),
            "cpu_model": &hw.cpu_model,
            "cpu_passmark": passmark,
            "npu_tops": npu_tops,
            "ram_gb": hw.total_ram_bytes / (1024 * 1024 * 1024),
            "has_gpu": hw.has_nvidia_gpu || hw.has_amd_gpu || hw.has_intel_igpu || hw.has_intel_npu,
        },
        "accel": {
            "recommended_ep_hint": accel.recommended_ep_hint(),
            "has_hw_accelerator": accel.has_hw_accelerator(),
            "accelerators": accel.accelerators.iter().map(|a| json!({
                "kind": a.kind.id(),
                "vendor": a.vendor,
                "present": a.present,
                "driver_ready": a.driver_ready,
                "notes": a.notes,
            })).collect::<Vec<_>>(),
            // 实际 ORT EP 选型链 + 当前 artifact 编入的 EP + best-effort active EP。
            "ep_chain": ep_telemetry.requested,
            "active_ep": ep_telemetry.active,
            "active_ep_approx": ep_telemetry.approx,
            "fallback_reason": ep_telemetry.fallback_reason,
            "compiled_eps": ep_sel.compiled.iter().map(|e| e.id()).collect::<Vec<_>>(),
        },
        // EP 运行时软件栈按需安装状态(cuda/openvino/rocm/directml/vitisai userspace)。
        // 平行 model_bootstrap:栈像底座模型一样首次运行按需拉取(内核驱动除外)。UI 轮询
        // 此字段显示「安装中 / 已就绪 / 失败」。栈装不上 → 对应 EP 降级 CPU。
        "ep_runtime_stacks": state.stack_install.snapshot(),
        // AMD NPU(VitisAI)监测 + 下载推荐 + benchmark 数据。零再分发:仅检测用户是否
        // 已自行装 Ryzen AI 运行时;缺则指向 AMD 官方下载页(非托管)。无 AMD NPU → null。
        // 「有 vitis 就用,没有就 AMD GPU(DirectML)+ CPU 兜底」由 ep_chain 落地。
        "vitisai_advice": attune_core::platform::npu::vitisai_advice(
            hw.has_amd_xdna_npu,
            attune_core::platform::npu::vitisai_runtime_present(),
            cfg!(feature = "vitis"),
        ),
        "region": {
            "detected": region.label(),
            "hf_endpoint": region.hf_endpoint(),
        },
        "scheduler": {
            "managed": true,
            "endpoint": scheduler_base,
            "status": scheduler.status,
            "tasks": scheduler.tasks,
            "models": scheduler_models,
            "model_capabilities": model_capabilities,
            "error": scheduler.error,
        },
        "model_capabilities": model_capabilities,
        "recommendation": recommendation.as_ref().map(|r| json!({
            "embedding_repo": r.embedding_repo,
            "embedding_size_mb": r.embedding_size_mb,
            "reranker_repo": r.reranker_repo,
            "reranker_size_mb": r.reranker_size_mb,
            "asr_ggml": r.asr_ggml,
            "asr_size_mb": r.asr_size_mb,
            "total_download_mb": r.total_download_mb(),
        })),
        "embedding": {
            "available": embedding_loaded,
            "model": "scheduler:kb.query.embed",
            "note": note(embedding_loaded, "vault locked / local scheduler unavailable")
        },
        "rerank": {
            "available": rerank_loaded,
            "model": "scheduler:kb.query.rerank",
            "note": note(rerank_loaded, "local scheduler unavailable")
        },
        "ocr": {
            "available": ocr_available,
            "engine": ocr_engine,
            "note": note(ocr_available, "local scheduler 未暴露 kb.document.ocr_detect / kb.document.ocr_recognize")
        },
        "asr": {
            "available": voice_readiness.asr.available,
            "registered": voice_readiness.asr.registered,
            "engine": asr_engine,
            "task": "kb.meeting.asr_frontend",
            "model": voice_readiness.asr.model,
            "model_state": voice_readiness.asr.model_state,
            "dispatchable": voice_readiness.asr.dispatchable,
            "gpu_capable": serde_json::Value::Null,
            "note": voice_readiness.asr.note,
            "gpu_note": serde_json::Value::Null
        },
        "tts": {
            "available": voice_readiness.tts.available,
            "registered": voice_readiness.tts.registered,
            "task": crate::routes::tts::TTS_TASK,
            "model": "tts-default",
            "model_state": voice_readiness.tts.model_state,
            "dispatchable": voice_readiness.tts.dispatchable,
            "engine": if voice_readiness.tts.available { "scheduler:tts-default" } else { "scheduler" },
            "note": voice_readiness.tts.note
        },
        "llm": {
            "configured": llm_configured,
            "default": "cloud or local scheduler OpenAI-compatible endpoint",
            "note": note(llm_configured, "Settings → AI 模型 配 endpoint + api_key")
        },
        "web_search": {
            "available": web_search_available,
            "engine": "browser (DuckDuckGo)",
            "note": note(web_search_available, "未检测到 Chrome/Edge — 安装 Chrome 或在 Settings 中指定 browser_path")
        },
        // #2 #5: 底座模型后台下载进度（embedding/reranker/ocr/asr）。解锁立即返回，
        // 模型在后台拉取；UI 轮询此字段显示"下载中 / 已就绪 / 失败"，不再静默卡住。
        "model_bootstrap": state.model_bootstrap.snapshot()
    }))
}

/// POST /api/v1/ai-stack/ensure — legacy compatibility.
///
/// Local model lifecycle is scheduler-owned. Attune does not download or start
/// OCR/ASR/embedding/rerank runtimes directly on any platform.
pub async fn ensure(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let scheduler_base = crate::local_scheduler::base_from_state(&state);
    Json(json!({
        "status": "scheduler-managed",
        "endpoint": scheduler_base,
        "message": "本地底座模型由 local scheduler 管理；请通过 scheduler ready/models/benchmark contract 检查状态",
        "tasks": [
            "kb.query.embed",
            "kb.query.rerank",
            "kb.document.ocr_detect",
            "kb.document.ocr_recognize",
            "kb.meeting.asr_frontend",
            "kb.speech.synthesize"
        ],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(
        name: &str,
        lifecycle: &str,
        dispatchable: &str,
    ) -> attune_core::edge_cloud::SchedulerModelStatus {
        attune_core::edge_cloud::SchedulerModelStatus {
            name: name.to_string(),
            lifecycle: lifecycle.to_string(),
            dispatchable: dispatchable.to_string(),
            ..Default::default()
        }
    }

    fn task(name: &str, model: &str) -> attune_core::edge_cloud::SchedulerRuntimeTaskSpec {
        attune_core::edge_cloud::SchedulerRuntimeTaskSpec {
            name: name.to_string(),
            model: model.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn scheduler_model_capabilities_are_explicit_task_mappings_not_name_heuristics() {
        let scheduler = crate::local_scheduler::SchedulerRuntimeProbe {
            status: "ready".to_string(),
            tasks: vec![],
            runtime_tasks: vec![
                task("kb.query.embed", "embedding-int8"),
                task("kb.speech.synthesize", "tts-default"),
                task("kb.document.summary", "llm-summary"),
                task("kb.query.ask", "llm-chat"),
            ],
            models: vec![
                model("embedding-int8", "ready", "FREE"),
                model("tts-default", "ready", "FREE"),
                model("llm-summary", "loading", "UNAVAILABLE"),
                model("llm-chat", "ready", "FREE"),
                model("qwen-instruct-unknown", "ready", "FREE"),
            ],
            error: None,
        };

        let rows = dynamic_model_capabilities(&scheduler, &json!({}), false);
        let caps_for = |name: &str| {
            rows.iter()
                .find(|row| row["name"] == name)
                .and_then(|row| row["capabilities"].as_array())
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
        };
        assert_eq!(caps_for("embedding-int8"), vec!["embedding"]);
        assert_eq!(caps_for("tts-default"), vec!["tts"]);
        assert_eq!(caps_for("llm-summary"), vec!["summary"]);
        assert_eq!(caps_for("llm-chat"), vec!["chat"]);
        assert!(caps_for("qwen-instruct-unknown").is_empty());
    }

    #[test]
    fn model_capability_gate_rejects_unready_or_wrong_capability_models() {
        let rows = vec![
            json!({
                "name": "llm-chat",
                "capabilities": ["chat"],
                "ready": true,
            }),
            json!({
                "name": "llm-summary",
                "capabilities": ["summary"],
                "ready": false,
            }),
        ];

        assert_eq!(
            model_capability_gate_result(&rows, "llm-chat", "chat"),
            ModelCapabilityGate::Allowed
        );
        assert_eq!(
            model_capability_gate_result(&rows, "llm-chat", "summary"),
            ModelCapabilityGate::CapabilityUnsupported {
                available: vec!["chat".to_string()]
            }
        );
        assert_eq!(
            model_capability_gate_result(&rows, "llm-summary", "summary"),
            ModelCapabilityGate::ModelNotReady
        );
        assert_eq!(
            model_capability_gate_result(&rows, "missing", "chat"),
            ModelCapabilityGate::ModelNotFound
        );
    }

    #[test]
    fn task_capability_registry_covers_k3_scheduler_contract_tasks() {
        assert!(task_has_capability("kb.query.ask", "chat"));
        assert!(task_has_capability("kb.query.ask_hq", "chat"));
        assert!(task_has_capability("kb.query.answer", "chat"));
        assert!(task_has_capability("kb.document.summary", "summary"));
        assert!(task_has_capability("doc.summarize", "summary"));
        assert!(task_has_capability("kb.document.long_summary", "summary"));
        assert!(!task_has_capability("kb.query.embed", "chat"));
        assert!(!task_has_capability("kb.speech.synthesize", "summary"));
    }
}
