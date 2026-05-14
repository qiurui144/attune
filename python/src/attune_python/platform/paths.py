"""跨平台路径管理（XDG / KnownFolders）"""

import platform

from attune_python.platform.base import PlatformProvider


def get_platform_provider() -> PlatformProvider:
    """根据当前系统返回对应的平台实现"""
    system = platform.system()
    if system == "Linux":
        from attune_python.platform.linux import LinuxPlatformProvider

        return LinuxPlatformProvider()
    elif system == "Windows":
        from attune_python.platform.windows import WindowsPlatformProvider

        return WindowsPlatformProvider()
    else:
        raise NotImplementedError(f"Unsupported platform: {system}")
