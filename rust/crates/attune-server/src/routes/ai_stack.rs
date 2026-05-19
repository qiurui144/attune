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

    let asr_backend = attune_core::asr::detect_asr_backend();
    let asr_available = asr_backend.is_some();
    let asr_model: Option<String> = asr_backend.as_ref().map(|b| b.model_name.clone());
    // F-16 hardware utilization: expose whisper.cpp GPU build status so Settings
    // UI can warn user when CPU-only build limits ASR throughput (10x slower).
    let asr_gpu_capable: Option<bool> = asr_backend.as_ref().map(|b| b.gpu_capable);

    // v0.6.0-rc.4: 硬件 tier + 模型推荐 + region
    let hw = &state.hardware;
    let tier = attune_core::platform::classify_hardware(hw);
    let recommendation = attune_core::platform::ModelRecommendation::for_tier(tier);
    let region = attune_core::platform::detect_region();
    let passmark = attune_core::platform::cpu_db::lookup(&hw.cpu_model)
        .map(|e| e.passmark);
    let npu_tops = attune_core::platform::cpu_db::lookup(&hw.cpu_model)
        .and_then(|e| e.npu_tops);

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
            "engine": "whisper.cpp",
            "model": asr_model,
            // F-16 GPU build flag — false 时 60s 音频转写 ~60s, true 时 GPU build ~5s (10x)
            "gpu_capable": asr_gpu_capable,
            "note": note(asr_available, "装 whisper.cpp + 下载 ggml-small.bin 到 ~/.local/share/attune/models/whisper/"),
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
        }
    }))
}
