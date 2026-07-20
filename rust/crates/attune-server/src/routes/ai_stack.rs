//! AI 底座状态 API（v0.6.0-rc.3，2026-04-27）。
//!
//! per CLAUDE.md "本地 AI 底座边界" 决策：本地仅捆绑必要底座（Embedding / Rerank /
//! OCR / ASR / TTS），LLM 走远端 token 默认。
//!
//! 本 route 暴露各底座的可用性 + 模型名 / 后端路径 — 让 Settings UI 简洁地显示
//! 是否加载，无需让用户配置（默认全部自动检测 / 加载）。

use axum::extract::State;
use axum::Json;
use serde_json::json;

use crate::state::SharedState;

fn note(available: bool, msg: &str) -> Option<String> {
    if available {
        None
    } else {
        Some(msg.to_string())
    }
}

/// GET /api/v1/ai_stack — 返各底座状态 + 硬件 tier + 模型推荐 + region
pub async fn status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let scheduler_base = crate::local_scheduler::base_from_state(&state);
    let scheduler = crate::local_scheduler::probe_scheduler_runtime(scheduler_base.clone()).await;
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
    let asr_available = has_scheduler_task("kb.meeting.asr_frontend");
    let asr_engine = if asr_available {
        "scheduler:kb.meeting.asr_frontend"
    } else {
        "scheduler"
    };
    let tts_registered = has_scheduler_task(crate::routes::tts::TTS_TASK);
    let tts_task_dispatchable = scheduler.runtime_tasks.iter().any(|task| {
        task.name == crate::routes::tts::TTS_TASK
            && task.model == crate::routes::tts::TTS_ENGINE
            && task.async_only
    });
    let tts_model = scheduler
        .models
        .iter()
        .find(|model| model.name == "tts-default");
    let tts_model_ready = tts_model.is_some_and(|model| {
        model.lifecycle.eq_ignore_ascii_case("ready")
            && matches!(
                model.dispatchable.trim().to_ascii_uppercase().as_str(),
                "FREE" | "BUSY" | "QUEUED"
            )
    });
    let tts_available = tts_registered && tts_task_dispatchable && tts_model_ready;
    state.set_tts_capability_ready(tts_available);
    let scheduler_models = scheduler
        .models
        .iter()
        .map(|model| model.name.clone())
        .collect::<Vec<_>>();

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
            "error": scheduler.error,
        },
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
            "available": asr_available,
            "engine": asr_engine,
            "model": serde_json::Value::Null,
            "gpu_capable": serde_json::Value::Null,
            "note": note(asr_available, "local scheduler 未暴露 kb.meeting.asr_frontend"),
            "gpu_note": serde_json::Value::Null
        },
        "tts": {
            "available": tts_available,
            "registered": tts_registered,
            "task": crate::routes::tts::TTS_TASK,
            "model": "tts-default",
            "model_state": tts_model.map(|model| model.state.clone()),
            "dispatchable": tts_model.map(|model| model.dispatchable.clone()),
            "engine": if tts_available { "scheduler:tts-default" } else { "scheduler" },
            "note": if !tts_registered {
                Some("local scheduler 未暴露 kb.speech.synthesize".to_string())
            } else if !tts_model_ready {
                Some("local scheduler 已注册 TTS task，但 tts-default 模型尚未配置或不可调度".to_string())
            } else {
                None
            }
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
