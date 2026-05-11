"""Windows 平台实现"""

from pathlib import Path

from npu_webhook.platform.base import NPUDevice, PlatformProvider


class WindowsPlatformProvider(PlatformProvider):
    def get_data_dir(self) -> Path:
        local_app_data = Path.home() / "AppData" / "Local"
        return local_app_data / "npu-webhook"

    def get_config_dir(self) -> Path:
        app_data = Path.home() / "AppData" / "Roaming"
        return app_data / "npu-webhook"

    def get_idle_seconds(self) -> float:
        # TODO Phase 4: GetLastInputInfo
        return 0.0

    def detect_npu(self) -> list[NPUDevice]:
        # TODO Phase 4: WMI + OpenVINO
        return []

    def register_autostart(self) -> bool:
        # TODO Phase 5: 计划任务 / Windows Service
        return False
