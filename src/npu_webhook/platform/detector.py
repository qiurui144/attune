"""NPU/iGPU 硬件检测"""

from npu_webhook.platform.base import NPUDevice


def detect_best_device() -> NPUDevice:
    """检测并返回最优计算设备

    优先级:
    1. Intel NPU (OpenVINO NPU plugin)
    2. AMD XDNA NPU (onnxruntime-directml)
    3. Intel iGPU (OpenVINO GPU plugin)
    4. AMD Radeon iGPU (onnxruntime-rocm)
    5. CPU fallback
    """
    # TODO Phase 4: 硬件检测逻辑
    return NPUDevice(name="CPU", device_type="cpu", vendor="generic")
