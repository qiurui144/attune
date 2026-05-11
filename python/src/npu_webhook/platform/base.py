"""跨平台抽象基类"""

from abc import ABC, abstractmethod
from dataclasses import dataclass
from pathlib import Path


@dataclass
class NPUDevice:
    """NPU/GPU 设备信息"""

    name: str
    device_type: str  # npu/igpu/cpu
    vendor: str  # intel/amd/generic
    driver: str = ""


class PlatformProvider(ABC):
    """跨平台功能抽象基类"""

    @abstractmethod
    def get_data_dir(self) -> Path:
        ...

    @abstractmethod
    def get_config_dir(self) -> Path:
        ...

    @abstractmethod
    def get_idle_seconds(self) -> float:
        ...

    @abstractmethod
    def detect_npu(self) -> list[NPUDevice]:
        ...

    @abstractmethod
    def register_autostart(self) -> bool:
        ...
