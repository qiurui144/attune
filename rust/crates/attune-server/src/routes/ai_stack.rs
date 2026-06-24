//! AI 底座状态 API（v0.6.0-rc.3，2026-04-27）。
//!
//! per CLAUDE.md "本地 AI 底座边界" 决策：本地仅捆绑必要底座（Embedding / Rerank /
//! OCR / ASR），LLM 走远端 token 默认。
//!
//! 本 route 暴露各底座的可用性 + 模型名 / 后端路径 — 让 Settings UI 简洁地显示
//! 是否加载，无需让用户配置（默认全部自动检测 / 加载）。

use axum::extract::State;
use axum::Json;
use serde_json::json;

use crate::state::SharedState;

fn note(available: bool, msg: &str) -> Option<String> {
    if available { None } else { Some(msg.to_string()) }
}

/// GET /api/v1/ai_stack — 返各底座状态 + 硬件 tier + 模型推荐 + region
pub async fn status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let embedding_loaded = state.embedding.lock().ok().map(|g| g.is_some()).unwrap_or(false);
    let rerank_loaded = state.reranker.lock().ok().map(|g| g.is_some()).unwrap_or(false);
    let llm_configured = state.llm.lock().ok().map(|g| g.is_some()).unwrap_or(false);
    // web_search readiness mirrors the actual decision in routes/chat.rs:
    // state.web_search is Some iff a usable browser was auto-detected (or an
    // explicit browser_path was set and verified). Checking the same Arc means
    // the status here stays in sync with whether chat web-search would succeed.
    let web_search_available = state.web_search.lock().ok().map(|g| g.is_some()).unwrap_or(false);

    let ocr_provider = attune_core::ocr::detect_default_provider();
    let ocr_available = ocr_provider.is_some();
    let ocr_engine: String = ocr_provider
        .as_ref()
        .map(|p| p.name().to_string())
        .unwrap_or_else(|| "none".into());

    // ASR engine is catalog-driven (sensevoice on AMD/Intel-Win + model-present; whisper
    // CPU fallback). `engine` is no longer hardcoded "whisper.cpp" — it reflects the
    // actually-selected backend so Settings UI shows the real engine.
    let asr_engine_sel = attune_core::asr::detect_asr_engine();
    let asr_available = asr_engine_sel.is_some();
    let asr_engine: String = asr_engine_sel
        .as_ref()
        .map(|e| e.label().to_string())
        .unwrap_or_else(|| "none".to_string());
    let asr_model: Option<String> = asr_engine_sel.as_ref().and_then(|e| e.model_label());
    // F-16 hardware utilization: GPU-build flag only meaningful for whisper.cpp;
    // SenseVoice is in-process ONNX (CPU int8 ~7x realtime) → no GPU-build concept.
    let asr_gpu_capable: Option<bool> = match asr_engine_sel.as_ref() {
        Some(attune_core::asr::AsrEngine::Whisper(b)) => Some(b.gpu_capable),
        _ => None,
    };

    // v0.6.0-rc.4: 硬件 tier + 模型推荐 + region
    let hw = &state.hardware;
    let tier = attune_core::platform::classify_hardware(hw);
    let recommendation = attune_core::platform::ModelRecommendation::for_tier(tier);
    let region = attune_core::platform::detect_region();
    let passmark = attune_core::platform::cpu_db::lookup(&hw.cpu_model)
        .map(|e| e.passmark);
    let npu_tops = attune_core::platform::cpu_db::lookup(&hw.cpu_model)
        .and_then(|e| e.npu_tops);

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
            "has_gpu": hw.has_nvidia_gpu || hw.has_amd_gpu,
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
        "region": {
            "detected": region.label(),
            "hf_endpoint": region.hf_endpoint(),
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
            "model": "bge-m3",
            "note": note(embedding_loaded, "vault locked / Ollama 未启动")
        },
        "rerank": {
            "available": rerank_loaded,
            "model": "bge-reranker-base (Xenova quantized)",
            "note": note(rerank_loaded, "ONNX 模型加载失败 / HuggingFace 拉取中")
        },
        "ocr": {
            "available": ocr_available,
            "engine": ocr_engine,
            "note": note(ocr_available, "PP-OCR 模型缺失 — 重新跑 attune deploy 或 apt install --reinstall attune")
        },
        "asr": {
            "available": asr_available,
            "engine": asr_engine,
            "model": asr_model,
            // F-16 GPU build flag — false 时 60s 音频转写 ~60s, true 时 GPU build ~5s (10x)
            "gpu_capable": asr_gpu_capable,
            "note": note(asr_available, "装 whisper.cpp 或一键拉取 SenseVoice (ai-stack/ensure) 到 ~/.local/share/attune/models/asr/sensevoice/"),
            "gpu_note": match asr_gpu_capable {
                Some(false) => Some("⚠ whisper.cpp 是 CPU-only build, 60s 音频可能耗时 60s+. 装 GPU build (CUDA/Metal/Vulkan) 可获 10x 加速.".to_string()),
                Some(true) => None,
                None => None,
            }
        },
        "llm": {
            "configured": llm_configured,
            "default": "remote token (per CLAUDE.md M2: 不在本地预装 LLM)",
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

/// POST /api/v1/ai-stack/ensure — 一键拉取缺失的本地底座模型（OCR + ASR）。
///
/// 面向非技术用户：底座模型缺失时不再要求用户去终端 / 重装包，应用内一键拉取。
/// OCR (PP-OCRv5 ~16MB) 与 ASR (whisper ggml) 走 HuggingFace（支持 HF_ENDPOINT 镜像）。
/// 后台执行（不阻塞请求），UI 轮询 GET /ai_stack 检测 available 翻绿。
/// Embedding / Rerank 在 vault 解锁 + 首次检索时自动加载，不在此处单独拉取。
pub async fn ensure(State(state): State<SharedState>) -> Json<serde_json::Value> {
    // 按硬件 tier 选 ASR ggml（弱机自动落到更小模型）。
    let tier = attune_core::platform::classify_hardware(&state.hardware);
    let asr_ggml = attune_core::platform::ModelRecommendation::for_tier(tier)
        .map(|r| r.asr_ggml.to_string());
    // Catalog-driven ASR engine: when sensevoice is selected for this hardware, fetch the
    // SenseVoice ONNX model + tokens instead of the whisper ggml. CPU-tier stays whisper.
    let asr_is_sensevoice = attune_core::asr::catalog_asr_engine() == "sensevoice";

    let state_for_persist = state.clone();
    tokio::spawn(async move {
        // OCR：~16MB，缺失才拉。失败不 panic，仅 log（§4.5 graceful）。
        let ocr = tokio::task::spawn_blocking(
            attune_core::ocr::ppocr::PpOcrProvider::ensure_models_downloaded,
        )
        .await;
        match ocr {
            Ok(Ok(())) => tracing::info!("ai-stack ensure: OCR models ready"),
            Ok(Err(e)) => tracing::warn!("ai-stack ensure: OCR download failed: {e}"),
            Err(e) => tracing::warn!("ai-stack ensure: OCR task join error: {e}"),
        }
        // ASR 模型：catalog 选 sensevoice → 拉 SenseVoice ONNX + tokens；否则按 tier 拉
        // whisper ggml。缺失才拉，失败不 panic（§4.5 graceful）。
        if asr_is_sensevoice {
            let r = tokio::task::spawn_blocking(
                attune_core::asr_sensevoice::ensure_sensevoice_model,
            )
            .await;
            match r {
                Ok(Ok(b)) => tracing::info!(
                    "ai-stack ensure: SenseVoice model ready at {}",
                    b.model_path.display()
                ),
                Ok(Err(e)) => tracing::warn!("ai-stack ensure: SenseVoice download failed: {e}"),
                Err(e) => tracing::warn!("ai-stack ensure: SenseVoice task join error: {e}"),
            }
        } else if let Some(ggml) = asr_ggml {
            let r = tokio::task::spawn_blocking(move || {
                attune_core::asr::ensure_whisper_model(&ggml)
            })
            .await;
            match r {
                Ok(Ok(path)) => tracing::info!("ai-stack ensure: ASR model ready at {}", path.display()),
                Ok(Err(e)) => tracing::warn!("ai-stack ensure: ASR download failed: {e}"),
                Err(e) => tracing::warn!("ai-stack ensure: ASR task join error: {e}"),
            }
        }
        // S8 cache: persist the source the resolver just selected so the next cold start
        // seeds it (makes write_selected_source/persist_used_source live, not dead code).
        // vault guard taken alone — respects lock ordering.
        if let Some(src_id) = attune_core::infer::model_source::current_top_source_id() {
            let vault_guard = state_for_persist.vault.lock().unwrap_or_else(|e| e.into_inner());
            let cur = vault_guard.store().get_meta("app_settings").ok().flatten()
                .and_then(|d| serde_json::from_slice::<serde_json::Value>(&d).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            let updated = attune_core::infer::model_source::persist_used_source(cur, &src_id);
            if let Ok(bytes) = serde_json::to_vec(&updated) {
                if let Err(e) = vault_guard.store().set_meta("app_settings", &bytes) {
                    tracing::warn!("ai-stack ensure: persist selected source failed: {e}");
                }
            }
        }
    });

    Json(json!({
        "status": "queued",
        "message": "正在后台下载缺失的本地底座模型，完成后将自动可用",
    }))
}
