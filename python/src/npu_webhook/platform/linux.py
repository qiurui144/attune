"""Linux 平台实现"""

from pathlib import Path

from npu_webhook.platform.base import NPUDevice, PlatformProvider


class LinuxPlatformProvider(PlatformProvider):
    def get_data_dir(self) -> Path:
        return Path.home() / ".local" / "share" / "npu-webhook"

    def get_config_dir(self) -> Path:
        return Path.home() / ".config" / "npu-webhook"

    def get_idle_seconds(self) -> float:
        # TODO Phase 4: xprintidle / DBus
        return 0.0

    def detect_npu(self) -> list[NPUDevice]:
        # TODO Phase 4: /sys/class/accel/ + OpenVINO
        return []

    def register_autostart(self) -> bool:
        # TODO Phase 5: systemd user service
        return False
