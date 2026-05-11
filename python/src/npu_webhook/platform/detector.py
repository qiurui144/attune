"""NPU/iGPU 硬件检测 + 内核/驱动/软件栈检查 + 芯片-驱动精确匹配"""

import logging
import platform
import re
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

from npu_webhook.platform.base import NPUDevice

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# 芯片-驱动精确匹配表
# ---------------------------------------------------------------------------

# Intel NPU 各代芯片信息
INTEL_NPU_CHIPS = {
    "meteor_lake": {
        "name": "Intel Core Ultra 1xx (Meteor Lake)",
        "pci_ids": ["7d1d"],
        "min_kernel": "6.3",
        "firmware": "intel/vpu/vpu_37xx_v0.0.bin",
        "npu_driver_min": "1.0.0",
        "openvino_min": "2024.0",
        "notes": "首代 Intel NPU, 11 TOPS",
    },
    "lunar_lake": {
        "name": "Intel Core Ultra 2xx V (Lunar Lake)",
        "pci_ids": ["643e"],
        "min_kernel": "6.8",
        "firmware": "intel/vpu/vpu_40xx_v0.0.bin",
        "npu_driver_min": "1.5.0",
        "openvino_min": "2024.4",
        "notes": "第二代 NPU, 48 TOPS",
    },
    "arrow_lake": {
        "name": "Intel Core Ultra 2xx (Arrow Lake)",
        "pci_ids": ["ad1d"],
        "min_kernel": "6.8",
        "firmware": "intel/vpu/vpu_40xx_v0.0.bin",
        "npu_driver_min": "1.5.0",
        "openvino_min": "2024.4",
        "notes": "桌面版 NPU, 13 TOPS",
    },
}

# Intel iGPU 各代信息
INTEL_IGPU_CHIPS = {
    "alder_lake": {
        "name": "Intel UHD/Iris Xe (Alder Lake, Gen12)",
        "gen": 12, "driver": "i915", "min_kernel": "5.15",
        "openvino_gpu": True,
    },
    "raptor_lake": {
        "name": "Intel UHD/Iris Xe (Raptor Lake, Gen12)",
        "gen": 12, "driver": "i915", "min_kernel": "6.0",
        "openvino_gpu": True,
    },
    "meteor_lake": {
        "name": "Intel Arc iGPU (Meteor Lake, Xe-LPG)",
        "gen": 12.7, "driver": "i915", "min_kernel": "6.5",
        "openvino_gpu": True,
    },
    "lunar_lake": {
        "name": "Intel Arc iGPU (Lunar Lake, Xe2-LPG)",
        "gen": 20, "driver": "xe", "min_kernel": "6.8",
        "openvino_gpu": True,
    },
    "arrow_lake": {
        "name": "Intel Arc iGPU (Arrow Lake, Xe-LPG+)",
        "gen": 12.7, "driver": "i915", "min_kernel": "6.8",
        "openvino_gpu": True,
    },
}

# AMD NPU 各代信息
AMD_NPU_CHIPS = {
    "phoenix": {
        "name": "AMD Ryzen 7x40 (Phoenix, XDNA1)",
        "min_kernel": "6.10",
        "xdna_version": 1,
        "tops": 10,
        "firmware": "amdnpu/1502_00/npu.sbin",
        "notes": "第一代 XDNA, 需要 IOMMU SVA",
    },
    "hawk_point": {
        "name": "AMD Ryzen 8x40 (Hawk Point, XDNA1)",
        "min_kernel": "6.10",
        "xdna_version": 1,
        "tops": 16,
        "firmware": "amdnpu/17f0_10/npu.sbin",
        "notes": "XDNA1 刷新版, 16 TOPS",
    },
    "strix_point": {
        "name": "AMD Ryzen AI 3xx (Strix Point, XDNA2)",
        "min_kernel": "6.14",
        "xdna_version": 2,
        "tops": 50,
        "firmware": "amdnpu/1502_00/npu.sbin",
        "notes": "第二代 XDNA, 50 TOPS",
        "known_issues": "内核 6.18-6.18.7 有 IOMMU SVA 回归",
    },
    "krackan_point": {
        "name": "AMD Ryzen AI 2xx (Krackan Point, XDNA2)",
        "min_kernel": "6.14",
        "xdna_version": 2,
        "tops": 50,
        "firmware": "amdnpu/1502_00/npu.sbin",
        "notes": "轻薄本 XDNA2",
    },
}


@dataclass
class DriverCheck:
    """驱动/软件栈检查结果"""
    name: str
    installed: bool
    version: str = ""
    message: str = ""
    required_by: str = ""


@dataclass
class ChipMatch:
    """芯片匹配结果"""
    chip_id: str
    chip_name: str
    vendor: str
    kernel_ok: bool
    min_kernel: str
    current_kernel: str
    firmware_ok: bool
    driver_installed: bool
    missing: list[str] = field(default_factory=list)
    install_commands: list[str] = field(default_factory=list)


@dataclass
class PlatformReport:
    """完整平台检测报告"""
    os: str = ""
    kernel: str = ""
    arch: str = ""
    devices: list[NPUDevice] = field(default_factory=list)
    drivers: list[DriverCheck] = field(default_factory=list)
    chip_matches: list[ChipMatch] = field(default_factory=list)
    install_commands: list[str] = field(default_factory=list)


# ---------------------------------------------------------------------------
# 工具函数
# ---------------------------------------------------------------------------


def _run_cmd(cmd: list[str], timeout: int = 5) -> str:
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        return r.stdout.strip() if r.returncode == 0 else ""
    except Exception:
        return ""


def _check_kernel_module(module: str) -> bool:
    """检查内核模块是否已加载（匹配行首，避免短名称误匹配子串）"""
    output = _run_cmd(["lsmod"])
    # lsmod 每行格式: "<module_name> <size> <refcount> ..."，匹配行首
    return bool(re.search(rf"^{re.escape(module)}\s", output, re.MULTILINE))


def _kernel_version_tuple(ver: str) -> tuple:
    """解析内核版本为可比较元组: '6.14.0-27-generic' -> (6, 14, 0)"""
    match = re.match(r"(\d+)\.(\d+)(?:\.(\d+))?", ver)
    if match:
        return (int(match.group(1)), int(match.group(2)), int(match.group(3) or 0))
    return (0, 0, 0)


def _kernel_ge(current: str, minimum: str) -> bool:
    """当前内核版本 >= 最低要求"""
    return _kernel_version_tuple(current) >= _kernel_version_tuple(minimum)


def _get_lspci_output() -> str:
    return _run_cmd(["lspci", "-nn"])


# ---------------------------------------------------------------------------
# 硬件检测
# ---------------------------------------------------------------------------


def _check_intel_npu() -> NPUDevice | None:
    accel_devs = list(Path("/dev").glob("accel*"))
    if accel_devs:
        try:
            import openvino  # noqa: F401
            return NPUDevice(name="Intel NPU", device_type="npu", vendor="intel", driver="openvino")
        except ImportError:
            return NPUDevice(name="Intel NPU (no driver)", device_type="npu", vendor="intel", driver="")

    try:
        from openvino import Core
        if "NPU" in Core().available_devices:
            return NPUDevice(name="Intel NPU", device_type="npu", vendor="intel", driver="openvino")
    except Exception:
        pass
    return None


def _check_amd_npu() -> NPUDevice | None:
    xdna_devs = list(Path("/dev").glob("amdxdna*"))
    if xdna_devs:
        return NPUDevice(name="AMD XDNA NPU", device_type="npu", vendor="amd", driver="xdna")

    # accel 子系统也可能暴露 amdxdna
    if _check_kernel_module("amdxdna"):
        return NPUDevice(name="AMD XDNA NPU", device_type="npu", vendor="amd", driver="xdna")

    try:
        import onnxruntime as ort
        if "DmlExecutionProvider" in ort.get_available_providers():
            return NPUDevice(name="AMD NPU (DirectML)", device_type="npu", vendor="amd", driver="directml")
    except Exception:
        pass
    return None


def _check_intel_igpu() -> NPUDevice | None:
    try:
        from openvino import Core
        if "GPU" in Core().available_devices:
            return NPUDevice(name="Intel iGPU", device_type="igpu", vendor="intel", driver="openvino")
    except Exception:
        pass

    render_devs = list(Path("/dev/dri").glob("renderD*")) if Path("/dev/dri").exists() else []
    if render_devs:
        lspci = _get_lspci_output()
        if "Intel" in lspci and ("VGA" in lspci or "Display" in lspci):
            driver = "xe" if _check_kernel_module("xe") else "i915"
            return NPUDevice(name="Intel iGPU", device_type="igpu", vendor="intel", driver=driver)
    return None


def _check_amd_igpu() -> NPUDevice | None:
    try:
        import onnxruntime as ort
        if "ROCMExecutionProvider" in ort.get_available_providers():
            return NPUDevice(name="AMD Radeon iGPU", device_type="igpu", vendor="amd", driver="rocm")
    except Exception:
        pass

    lspci = _get_lspci_output()
    for line in lspci.splitlines():
        if "AMD" in line and ("VGA" in line or "Display" in line):
            return NPUDevice(name="AMD Radeon iGPU", device_type="igpu", vendor="amd", driver="amdgpu")
    return None


def _check_ollama() -> NPUDevice | None:
    import urllib.request
    try:
        with urllib.request.urlopen("http://localhost:11434/api/tags", timeout=3) as resp:
            if resp.status == 200:
                return NPUDevice(name="Ollama", device_type="ollama", vendor="ollama", driver="http")
    except Exception:
        pass
    return None


def detect_all_devices() -> list[NPUDevice]:
    devices = []
    for checker in [_check_intel_npu, _check_amd_npu, _check_intel_igpu, _check_amd_igpu, _check_ollama]:
        try:
            dev = checker()
            if dev:
                devices.append(dev)
        except Exception as e:
            logger.debug("Device check failed: %s", e)
    devices.append(NPUDevice(name="CPU", device_type="cpu", vendor="generic"))
    return devices


def detect_best_device() -> NPUDevice:
    devices = detect_all_devices()
    priority = {"ollama": 0, "npu": 1, "igpu": 2, "cpu": 9}
    devices.sort(key=lambda d: priority.get(d.device_type, 5))
    best = devices[0]
    logger.info("Best device: %s (%s/%s)", best.name, best.device_type, best.vendor)
    return best


def device_to_engine_params(device: NPUDevice) -> dict:
    if device.device_type == "ollama":
        return {"device": "ollama"}
    if device.vendor == "intel" and device.driver == "openvino":
        return {"device": "openvino"}
    if device.vendor == "amd" and device.driver in ("directml", "xdna"):
        return {"device": "directml"}
    if device.vendor == "amd" and device.driver == "rocm":
        return {"device": "rocm"}
    return {"device": "cpu"}


# ---------------------------------------------------------------------------
# 芯片-驱动精确匹配
# ---------------------------------------------------------------------------


def _identify_intel_npu_chip(lspci: str, kernel: str) -> ChipMatch | None:
    """通过 PCI ID 精确识别 Intel NPU 芯片代"""
    for chip_id, info in INTEL_NPU_CHIPS.items():
        for pci_id in info["pci_ids"]:
            if pci_id in lspci.lower():
                kernel_ok = _kernel_ge(kernel, info["min_kernel"])
                fw_path = Path(f"/lib/firmware/{info['firmware']}")
                fw_ok = fw_path.exists()

                missing = []
                cmds = []
                if not kernel_ok:
                    missing.append(f"内核 >= {info['min_kernel']}（当前 {kernel}）")
                if not fw_ok:
                    missing.append(f"固件 {info['firmware']}")
                    cmds.append("sudo apt-get install -y intel-npu-firmware")

                # 检查 OpenVINO
                try:
                    import openvino  # noqa: F401
                except ImportError:
                    missing.append(f"openvino >= {info['openvino_min']}")
                    cmds.append(f"pip install 'openvino>={info['openvino_min']}'")

                # Level Zero
                if not _run_cmd(["dpkg", "-l", "level-zero"]):
                    missing.append("Level Zero 运行时")
                    cmds.append("sudo apt-get install -y level-zero intel-level-zero-gpu")

                return ChipMatch(
                    chip_id=chip_id,
                    chip_name=info["name"],
                    vendor="intel",
                    kernel_ok=kernel_ok,
                    min_kernel=info["min_kernel"],
                    current_kernel=kernel,
                    firmware_ok=fw_ok,
                    driver_installed=not bool(missing),
                    missing=missing,
                    install_commands=cmds,
                )
    return None


def _identify_intel_igpu_chip(lspci: str, kernel: str) -> ChipMatch | None:
    """通过 lspci 关键词识别 Intel iGPU 代"""
    keywords_map = {
        "lunar_lake": ["Lunar Lake", "LNL"],
        "arrow_lake": ["Arrow Lake", "ARL"],
        "meteor_lake": ["Meteor Lake", "MTL"],
        "raptor_lake": ["Raptor Lake", "RPL"],
        "alder_lake": ["Alder Lake", "ADL"],
    }

    for chip_id, keywords in keywords_map.items():
        for kw in keywords:
            if kw.lower() in lspci.lower():
                info = INTEL_IGPU_CHIPS[chip_id]
                kernel_ok = _kernel_ge(kernel, info["min_kernel"])
                missing = []
                cmds = []

                if not kernel_ok:
                    missing.append(f"内核 >= {info['min_kernel']}（当前 {kernel}）")

                # 内核驱动
                expected_driver = info["driver"]
                if not _check_kernel_module(expected_driver):
                    missing.append(f"{expected_driver} 内核模块")

                # OpenVINO GPU
                if info["openvino_gpu"]:
                    try:
                        import openvino  # noqa: F401
                    except ImportError:
                        missing.append("openvino（GPU 推理）")
                        cmds.append("pip install openvino")

                    if not _run_cmd(["dpkg", "-l", "intel-opencl-icd"]):
                        missing.append("Intel OpenCL ICD")
                        cmds.append("sudo apt-get install -y intel-opencl-icd intel-level-zero-gpu")

                return ChipMatch(
                    chip_id=chip_id,
                    chip_name=info["name"],
                    vendor="intel",
                    kernel_ok=kernel_ok,
                    min_kernel=info["min_kernel"],
                    current_kernel=kernel,
                    firmware_ok=True,
                    driver_installed=not bool(missing),
                    missing=missing,
                    install_commands=cmds,
                )
    return None


def _identify_amd_npu_chip(lspci: str, kernel: str) -> ChipMatch | None:
    """识别 AMD NPU 芯片代"""
    keywords_map = {
        "strix_point": ["Strix", "Ryzen AI 3"],
        "krackan_point": ["Krackan", "Ryzen AI 2"],
        "hawk_point": ["Hawk Point", "Ryzen 8"],
        "phoenix": ["Phoenix", "Ryzen 7040", "Ryzen 7x40"],
    }

    # 也通过 amdxdna 设备存在来判断
    has_xdna = _check_kernel_module("amdxdna") or bool(list(Path("/dev").glob("amdxdna*")))

    matched_chip = None
    for chip_id, keywords in keywords_map.items():
        for kw in keywords:
            if kw.lower() in lspci.lower():
                matched_chip = chip_id
                break
        if matched_chip:
            break

    # 如果有 XDNA 设备但无法识别具体代，默认 phoenix
    if not matched_chip and has_xdna:
        matched_chip = "phoenix"

    if not matched_chip:
        return None

    info = AMD_NPU_CHIPS[matched_chip]
    kernel_ok = _kernel_ge(kernel, info["min_kernel"])
    fw_path = Path(f"/lib/firmware/{info['firmware']}")
    fw_ok = fw_path.exists()

    missing = []
    cmds = []

    if not kernel_ok:
        missing.append(f"内核 >= {info['min_kernel']}（当前 {kernel}）")
        if info["min_kernel"] == "6.14":
            cmds.append("# 升级到 Ubuntu 25.04+ 或安装 linux-generic-hwe 内核")

    if not _check_kernel_module("amdxdna"):
        missing.append("amdxdna 内核模块")
        if _kernel_ge(kernel, "6.14"):
            cmds.append("sudo modprobe amdxdna")
        else:
            cmds.append("# 内核 < 6.14: sudo apt install amdxdna-dkms (需 AMD 官方源)")

    if not fw_ok:
        missing.append(f"NPU 固件 {info['firmware']}")
        cmds.append("sudo apt-get install -y linux-firmware")

    # 检查 IOMMU SVA（通过 sysfs 而非 dmesg，避免读取大量内核日志）
    if platform.system() == "Linux":
        iommu_path = Path("/sys/class/iommu")
        iommu_ok = iommu_path.exists() and any(True for _ in iommu_path.iterdir())
        if not iommu_ok:
            missing.append("IOMMU SVA（BIOS 中启用）")

    # 已知问题警告
    known_issues = info.get("known_issues")
    if known_issues and _kernel_ge(kernel, "6.18") and not _kernel_ge(kernel, "6.18.8"):
        missing.append(f"⚠ {known_issues}")

    return ChipMatch(
        chip_id=matched_chip,
        chip_name=info["name"],
        vendor="amd",
        kernel_ok=kernel_ok,
        min_kernel=info["min_kernel"],
        current_kernel=kernel,
        firmware_ok=fw_ok,
        driver_installed=not bool(missing),
        missing=missing,
        install_commands=cmds,
    )


# ---------------------------------------------------------------------------
# 通用驱动检查（保持向后兼容）
# ---------------------------------------------------------------------------


def check_kernel_info() -> dict:
    return {
        "system": platform.system(),
        "kernel": platform.release(),
        "arch": platform.machine(),
        "version": platform.version(),
    }


def check_intel_drivers() -> list[DriverCheck]:
    checks = []
    system = platform.system()

    if system == "Linux":
        checks.append(DriverCheck(
            name="intel_vpu 内核模块", installed=_check_kernel_module("intel_vpu"),
            message="Intel NPU 驱动" if _check_kernel_module("intel_vpu") else "未加载 — 需要 Linux 6.3+ 内核",
            required_by="Intel NPU",
        ))
        i915 = _check_kernel_module("i915")
        checks.append(DriverCheck(
            name="i915 内核模块", installed=i915,
            message="Intel iGPU 驱动" if i915 else "未加载",
            required_by="Intel iGPU (Gen9-Gen12)",
        ))
        xe = _check_kernel_module("xe")
        checks.append(DriverCheck(
            name="xe 内核模块", installed=xe,
            message="Intel Xe2 驱动" if xe else "未加载（Lunar Lake+ 需要）",
            required_by="Intel iGPU (Xe2/Lunar Lake+)",
        ))
        npu_fw = Path("/lib/firmware/intel/vpu").exists() or Path("/lib/firmware/intel/npu").exists()
        checks.append(DriverCheck(
            name="Intel NPU 固件", installed=npu_fw,
            message="已安装" if npu_fw else "未找到 — apt install intel-npu-firmware",
            required_by="Intel NPU",
        ))
        accel = bool(list(Path("/dev").glob("accel*")))
        checks.append(DriverCheck(
            name="/dev/accel 设备节点", installed=accel,
            message="可用" if accel else "不可用 — 内核需支持 DRM accel 子系统",
            required_by="Intel NPU",
        ))
        l0 = bool(_run_cmd(["dpkg", "-l", "level-zero"]))
        checks.append(DriverCheck(
            name="Level Zero 运行时", installed=l0,
            message="已安装" if l0 else "未安装 — apt install level-zero intel-level-zero-gpu",
            required_by="Intel NPU/iGPU (OpenVINO)",
        ))

    try:
        import openvino
        checks.append(DriverCheck(name="openvino", installed=True, version=openvino.__version__,
                                  required_by="Intel NPU/iGPU"))
    except ImportError:
        checks.append(DriverCheck(name="openvino", installed=False, message="pip install openvino",
                                  required_by="Intel NPU/iGPU"))

    return checks


def check_amd_drivers() -> list[DriverCheck]:
    checks = []
    system = platform.system()

    if system == "Linux":
        amdxdna = _check_kernel_module("amdxdna")
        checks.append(DriverCheck(
            name="amdxdna 内核模块", installed=amdxdna,
            message="AMD XDNA NPU 驱动" if amdxdna else "未加载 — 需要内核 6.14+ 或 amdxdna-dkms",
            required_by="AMD Ryzen AI NPU",
        ))
        amdgpu = _check_kernel_module("amdgpu")
        checks.append(DriverCheck(
            name="amdgpu 内核模块", installed=amdgpu,
            message="AMD GPU 驱动" if amdgpu else "未加载",
            required_by="AMD Radeon iGPU",
        ))
        xdna_dev = bool(list(Path("/dev").glob("amdxdna*")))
        checks.append(DriverCheck(
            name="/dev/amdxdna 设备节点", installed=xdna_dev,
            message="可用" if xdna_dev else "不可用",
            required_by="AMD Ryzen AI NPU",
        ))
        rocm = Path("/opt/rocm").exists()
        rocm_ver = Path("/opt/rocm/.info/version").read_text().strip() if rocm else ""
        checks.append(DriverCheck(
            name="ROCm", installed=rocm, version=rocm_ver,
            message="已安装" if rocm else "未安装",
            required_by="AMD Radeon iGPU (ROCm 推理)",
        ))

    try:
        import onnxruntime as ort
        providers = ort.get_available_providers()
        has_dml = "DmlExecutionProvider" in providers
        has_rocm = "ROCMExecutionProvider" in providers
        if has_dml:
            checks.append(DriverCheck(name="onnxruntime-directml", installed=True, version=ort.__version__,
                                      required_by="AMD NPU (Windows)"))
        elif has_rocm:
            checks.append(DriverCheck(name="onnxruntime-rocm", installed=True, version=ort.__version__,
                                      required_by="AMD Radeon iGPU"))
        else:
            checks.append(DriverCheck(
                name="onnxruntime (AMD 加速)", installed=False,
                message="pip install onnxruntime-directml (Win) 或 onnxruntime-rocm (Linux)",
                required_by="AMD 加速推理",
            ))
    except ImportError:
        checks.append(DriverCheck(name="onnxruntime", installed=False, message="pip install onnxruntime",
                                  required_by="ONNX 推理"))

    return checks


def check_ollama() -> list[DriverCheck]:
    checks = []
    import urllib.request
    import json
    try:
        with urllib.request.urlopen("http://localhost:11434/api/tags", timeout=3) as resp:
            data = json.loads(resp.read())
        models = [m["name"] for m in data.get("models", [])]
        checks.append(DriverCheck(name="Ollama 服务", installed=True, message=f"运行中，{len(models)} 个模型"))
        has_bge = any("bge-m3" in m for m in models)
        checks.append(DriverCheck(
            name="bge-m3 模型", installed=has_bge,
            message="已拉取" if has_bge else "未拉取 — ollama pull bge-m3",
        ))
    except Exception:
        checks.append(DriverCheck(
            name="Ollama 服务", installed=False,
            message="不可达 — curl -fsSL https://ollama.com/install.sh | sh",
        ))
    return checks


# ---------------------------------------------------------------------------
# 一键安装命令生成
# ---------------------------------------------------------------------------


def generate_install_commands(
    devices: list[NPUDevice],
    drivers: list[DriverCheck],
    chip_matches: list[ChipMatch],
) -> list[str]:
    """根据芯片匹配和检测结果生成精确安装命令"""
    commands = []
    system = platform.system()
    if system != "Linux":
        return commands

    seen = set()
    # 优先使用芯片匹配的精确命令
    for cm in chip_matches:
        for cmd in cm.install_commands:
            if cmd not in seen:
                commands.append(cmd)
                seen.add(cmd)

    # 补充通用缺失
    missing_names = {d.name for d in drivers if not d.installed}
    if "Ollama 服务" in missing_names:
        cmd = "curl -fsSL https://ollama.com/install.sh | sh"
        if cmd not in seen:
            commands.append(cmd)
            seen.add(cmd)
        cmd2 = "ollama pull bge-m3"
        if cmd2 not in seen:
            commands.append(cmd2)
    elif "bge-m3 模型" in missing_names:
        cmd = "ollama pull bge-m3"
        if cmd not in seen:
            commands.append(cmd)

    return commands


# ---------------------------------------------------------------------------
# 完整平台报告
# ---------------------------------------------------------------------------


def full_platform_check() -> PlatformReport:
    """完整平台检测报告：硬件 + 内核 + 芯片匹配 + 驱动 + 安装建议"""
    kernel_info = check_kernel_info()
    kernel = kernel_info["kernel"]
    devices = detect_all_devices()

    drivers: list[DriverCheck] = []
    chip_matches: list[ChipMatch] = []
    vendors = {d.vendor for d in devices}

    lspci = _get_lspci_output()

    if "intel" in vendors:
        drivers.extend(check_intel_drivers())
        # 芯片级匹配
        npu_match = _identify_intel_npu_chip(lspci, kernel)
        if npu_match:
            chip_matches.append(npu_match)
        igpu_match = _identify_intel_igpu_chip(lspci, kernel)
        if igpu_match:
            chip_matches.append(igpu_match)

    if "amd" in vendors:
        drivers.extend(check_amd_drivers())
        npu_match = _identify_amd_npu_chip(lspci, kernel)
        if npu_match:
            chip_matches.append(npu_match)

    drivers.extend(check_ollama())

    install_cmds = generate_install_commands(devices, drivers, chip_matches)

    return PlatformReport(
        os=kernel_info["system"],
        kernel=kernel,
        arch=kernel_info["arch"],
        devices=devices,
        drivers=drivers,
        chip_matches=chip_matches,
        install_commands=install_cmds,
    )
