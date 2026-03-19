"""跨平台抽象测试"""

import platform


def test_get_platform_provider():
    from npu_webhook.platform.paths import get_platform_provider

    provider = get_platform_provider()
    system = platform.system()
    if system == "Linux":
        from npu_webhook.platform.linux import LinuxPlatformProvider
        assert isinstance(provider, LinuxPlatformProvider)
    elif system == "Windows":
        from npu_webhook.platform.windows import WindowsPlatformProvider
        assert isinstance(provider, WindowsPlatformProvider)


def test_detect_best_device():
    from npu_webhook.platform.detector import detect_best_device

    device = detect_best_device()
    # 根据环境不同，可能是 ollama/igpu/cpu 等
    assert device.device_type in ("cpu", "npu", "igpu", "ollama")
    assert device.vendor in ("generic", "intel", "amd", "ollama")


def test_data_dir_exists():
    from npu_webhook.platform.paths import get_platform_provider

    provider = get_platform_provider()
    data_dir = provider.get_data_dir()
    assert data_dir is not None
    assert str(data_dir).endswith("npu-webhook")


def test_check_kernel_module_no_false_positive():
    """回归测试：_check_kernel_module 不应将子串当作模块名（如 'xe' 不应匹配 'xenfs'）"""
    from unittest.mock import patch
    from npu_webhook.platform.detector import _check_kernel_module

    fake_lsmod = (
        "Module                  Size  Used by\n"
        "xenfs                  16384  1\n"
        "amdxdna               131072  0\n"
        "i915                 2916352  5\n"
    )
    with patch("npu_webhook.platform.detector._run_cmd", return_value=fake_lsmod):
        assert _check_kernel_module("i915") is True       # 完整匹配
        assert _check_kernel_module("amdxdna") is True    # 完整匹配
        assert _check_kernel_module("xe") is False        # 只有 xenfs，不含独立 xe 模块
        assert _check_kernel_module("xen") is False       # 只有 xenfs，不含独立 xen 模块


def test_kernel_version_comparison():
    """内核版本比较逻辑验证"""
    from npu_webhook.platform.detector import _kernel_ge

    assert _kernel_ge("6.14.0-27-generic", "6.14") is True
    assert _kernel_ge("6.3.0", "6.14") is False
    assert _kernel_ge("6.14.1", "6.14.0") is True
    assert _kernel_ge("5.15.0", "6.0") is False


def test_full_platform_check_runs():
    """full_platform_check 应正常执行不抛异常"""
    from npu_webhook.platform.detector import full_platform_check

    report = full_platform_check()
    assert report.os != ""
    assert report.kernel != ""
    assert len(report.devices) >= 1  # 至少有 CPU
