"""/models - 模型管理 + 预置检查"""

import asyncio
import logging
from pathlib import Path

from fastapi import APIRouter

from npu_webhook.app_state import state
from npu_webhook.config import settings
from npu_webhook.platform.detector import (
    detect_all_devices,
    device_to_engine_params,
    full_platform_check,
)

logger = logging.getLogger(__name__)

router = APIRouter(prefix="/api/v1", tags=["models"])

# 支持的模型清单
SUPPORTED_MODELS = {
    "bge-m3": {
        "name": "BAAI/bge-m3",
        "dimension": 1024,
        "description": "多语言 embedding 模型 (1024 dim)",
        "ollama_name": "bge-m3",
        "onnx_repo": "BAAI/bge-m3",
    },
    "bge-small-zh-v1.5": {
        "name": "BAAI/bge-small-zh-v1.5",
        "dimension": 512,
        "description": "中文小型 embedding 模型 (512 dim)",
        "ollama_name": "bge-small-zh-v1.5",
        "onnx_repo": "BAAI/bge-small-zh-v1.5",
    },
    "bge-large-zh-v1.5": {
        "name": "BAAI/bge-large-zh-v1.5",
        "dimension": 1024,
        "description": "中文大型 embedding 模型 (1024 dim)",
        "ollama_name": "bge-large-zh-v1.5",
        "onnx_repo": "BAAI/bge-large-zh-v1.5",
    },
}


def _check_onnx_model(model_name: str) -> dict:
    """检查 ONNX 模型是否已下载"""
    data_dir = settings.data_dir
    candidates = [
        data_dir / "models" / model_name,
        Path.home() / ".cache" / "npu-webhook" / "models" / model_name,
    ]
    for p in candidates:
        if (p / "model.onnx").exists() and (p / "tokenizer.json").exists():
            return {"available": True, "path": str(p), "backend": "onnx"}
    return {"available": False, "path": str(candidates[0]), "backend": "onnx"}


def _check_ollama_model_sync(model_name: str) -> dict:
    """检查 Ollama 模型是否已拉取（同步，供 asyncio.to_thread 调用）"""
    import json
    import urllib.request

    ollama_name = SUPPORTED_MODELS.get(model_name, {}).get("ollama_name", model_name)
    try:
        with urllib.request.urlopen("http://localhost:11434/api/tags", timeout=3) as resp:
            data = json.loads(resp.read())
        models = [m["name"].split(":")[0] for m in data.get("models", [])]
        available = ollama_name in models
        return {"available": available, "backend": "ollama", "ollama_name": ollama_name}
    except Exception:
        return {"available": False, "backend": "ollama", "ollama_name": ollama_name, "error": "Ollama not reachable"}


async def _check_ollama_model(model_name: str) -> dict:
    """异步包装，避免阻塞 event loop"""
    return await asyncio.to_thread(_check_ollama_model_sync, model_name)


@router.get("/models")
async def list_models() -> dict:
    """列出支持的模型及其状态"""
    devices = detect_all_devices()
    current_model = settings.embedding.model
    engine_available = state.embedding_engine is not None

    models = []
    for name, info in SUPPORTED_MODELS.items():
        onnx_status = _check_onnx_model(name)
        ollama_status = await _check_ollama_model(name)
        models.append({
            "name": name,
            "description": info["description"],
            "dimension": info["dimension"],
            "active": name == current_model and engine_available,
            "onnx": onnx_status,
            "ollama": ollama_status,
        })

    return {
        "current_model": current_model,
        "embedding_available": engine_available,
        "devices": [{"name": d.name, "type": d.device_type, "vendor": d.vendor, "driver": d.driver} for d in devices],
        "models": models,
    }


@router.post("/models/check")
async def check_deployment() -> dict:
    """部署前置检查：内核 + 硬件 + 驱动 + 模型 + 依赖 + 一键安装命令"""
    report = full_platform_check()
    checks = []

    # 1. 内核/系统信息
    checks.append({
        "name": "system",
        "status": "ok",
        "message": f"{report.os} {report.kernel} ({report.arch})",
    })

    # 2. 硬件检测
    has_accelerator = any(d.device_type in ("npu", "igpu", "ollama") for d in report.devices)
    checks.append({
        "name": "hardware",
        "status": "ok" if has_accelerator else "warn",
        "message": f"检测到 {len(report.devices)} 个设备" + ("（含加速器）" if has_accelerator else "（仅 CPU）"),
        "devices": [{"name": d.name, "type": d.device_type, "vendor": d.vendor, "driver": d.driver} for d in report.devices],
    })

    # 3. 芯片级精确匹配
    if report.chip_matches:
        for cm in report.chip_matches:
            checks.append({
                "name": f"chip:{cm.chip_id}",
                "status": "ok" if cm.driver_installed else "warn",
                "message": cm.chip_name,
                "kernel_ok": cm.kernel_ok,
                "min_kernel": cm.min_kernel,
                "current_kernel": cm.current_kernel,
                "firmware_ok": cm.firmware_ok,
                "missing": cm.missing,
                "install_commands": cm.install_commands,
            })

    # 4. 驱动/软件栈检查
    driver_ok = sum(d.installed for d in report.drivers)
    driver_total = len(report.drivers)
    checks.append({
        "name": "drivers",
        "status": "ok" if driver_ok == driver_total else "warn",
        "message": f"{driver_ok}/{driver_total} 驱动/组件已就绪",
        "details": [
            {"name": d.name, "installed": d.installed, "version": d.version,
             "message": d.message, "required_by": d.required_by}
            for d in report.drivers
        ],
    })

    # 4. 当前模型状态
    model = settings.embedding.model
    engine_ok = state.embedding_engine is not None
    checks.append({
        "name": "embedding_engine",
        "status": "ok" if engine_ok else "fail",
        "message": f"模型 {model} {'已就绪' if engine_ok else '未就绪'}",
        "model": model,
        "dimension": state.embedding_engine.get_dimension() if engine_ok else None,
    })

    # 5. Ollama 可达性
    ollama_status = await _check_ollama_model(model)
    ollama_ok = ollama_status.get("available", False)
    checks.append({
        "name": "ollama",
        "status": "ok" if ollama_ok else "warn",
        "message": f"Ollama {'可用' if ollama_ok else '不可用或模型未拉取'}",
        "detail": ollama_status,
    })

    # 6. ONNX 模型文件
    onnx_status = _check_onnx_model(model)
    checks.append({
        "name": "onnx_model",
        "status": "ok" if onnx_status["available"] else "info",
        "message": f"ONNX 模型{'已下载' if onnx_status['available'] else '未下载（使用 Ollama 时不需要）'}",
        "path": onnx_status["path"],
    })

    # 7. Python 依赖检查
    dep_checks = []
    for pkg, purpose in [
        ("onnxruntime", "ONNX CPU/DirectML"),
        ("openvino", "Intel NPU/iGPU"),
        ("tokenizers", "ONNX tokenizer"),
    ]:
        try:
            __import__(pkg)
            dep_checks.append({"package": pkg, "installed": True, "purpose": purpose})
        except ImportError:
            dep_checks.append({"package": pkg, "installed": False, "purpose": purpose})

    all_critical_deps = all(d["installed"] for d in dep_checks if d["package"] == "tokenizers")
    checks.append({
        "name": "dependencies",
        "status": "ok" if all_critical_deps else "warn",
        "message": f"{sum(d['installed'] for d in dep_checks)}/{len(dep_checks)} 依赖已安装",
        "packages": dep_checks,
    })

    # 8. 数据库状态
    db_ok = state.db is not None
    checks.append({
        "name": "database",
        "status": "ok" if db_ok else "fail",
        "message": f"SQLite {'正常' if db_ok else '未初始化'}",
        "items": state.db.count_items() if db_ok else 0,
        "pending": state.db.pending_embedding_count() if db_ok else 0,
    })

    overall = "ok" if all(c["status"] in ("ok", "info") for c in checks) else "warn"
    if any(c["status"] == "fail" for c in checks):
        overall = "fail"

    return {
        "overall": overall,
        "checks": checks,
        "install_commands": report.install_commands,
    }


@router.post("/models/download")
async def download_model(model_name: str = "bge-m3", backend: str = "ollama") -> dict:
    """触发模型下载

    backend: ollama / onnx
    """
    if model_name not in SUPPORTED_MODELS:
        return {"status": "error", "message": f"不支持的模型: {model_name}"}

    if backend == "ollama":
        import json
        import urllib.request

        ollama_name = SUPPORTED_MODELS[model_name].get("ollama_name", model_name)

        def _pull() -> dict:
            data = json.dumps({"name": ollama_name}).encode()
            req = urllib.request.Request(
                "http://localhost:11434/api/pull",
                data=data,
                headers={"Content-Type": "application/json"},
            )
            try:
                with urllib.request.urlopen(req, timeout=600) as resp:
                    # Ollama pull 是流式响应，读取最后一行
                    last_line = ""
                    for line in resp:
                        last_line = line.decode().strip()
                return {"status": "ok", "backend": "ollama", "model": ollama_name, "detail": last_line}
            except Exception as e:
                return {"status": "error", "backend": "ollama", "message": str(e)}

        return await asyncio.to_thread(_pull)

    return {"status": "error", "message": f"ONNX 下载需要 optimum，建议使用 ollama 后端"}
