"""Pydantic Settings 配置管理，支持 YAML 配置文件"""

import platform
from pathlib import Path

import yaml
from pydantic import BaseModel
from pydantic_settings import BaseSettings


class ServerConfig(BaseModel):
    host: str = "127.0.0.1"
    port: int = 18900


class EmbeddingConfig(BaseModel):
    model: str = "bge-m3"
    device: str = "auto"  # auto/ollama/cpu/npu/gpu
    batch_size: int = 16
    max_length: int = 512


class AuthConfig(BaseModel):
    mode: str = "localhost"  # localhost/token
    token: str = ""


class IngestConfig(BaseModel):
    min_content_length: int = 100
    excluded_domains: list[str] = ["mail.google.com", "web.whatsapp.com"]


class LoggingConfig(BaseModel):
    level: str = "INFO"
    max_size_mb: int = 50


class SearchConfig(BaseModel):
    default_top_k: int = 10
    rrf_k: int = 60  # RRF 常数
    vector_weight: float = 0.6
    fulltext_weight: float = 0.4


class ChunkConfig(BaseModel):
    chunk_size: int = 512
    overlap: int = 128


class Settings(BaseSettings):
    server: ServerConfig = ServerConfig()
    embedding: EmbeddingConfig = EmbeddingConfig()
    auth: AuthConfig = AuthConfig()
    ingest: IngestConfig = IngestConfig()
    logging: LoggingConfig = LoggingConfig()
    search: SearchConfig = SearchConfig()
    chunk: ChunkConfig = ChunkConfig()

    data_dir: Path = Path("")
    config_dir: Path = Path("")


def _default_data_dir() -> Path:
    system = platform.system()
    if system == "Windows":
        return Path.home() / "AppData" / "Local" / "npu-webhook"
    return Path.home() / ".local" / "share" / "npu-webhook"


def _default_config_dir() -> Path:
    system = platform.system()
    if system == "Windows":
        return Path.home() / "AppData" / "Roaming" / "npu-webhook"
    return Path.home() / ".config" / "npu-webhook"


def load_settings() -> Settings:
    """从 YAML 配置文件加载设置，不存在则使用默认值"""
    config_dir = _default_config_dir()
    data_dir = _default_data_dir()
    config_file = config_dir / "config.yaml"

    s = Settings(data_dir=data_dir, config_dir=config_dir)

    if config_file.exists():
        with open(config_file) as f:
            data = yaml.safe_load(f) or {}
        # 合并 YAML 配置
        for section_name, section_data in data.items():
            if hasattr(s, section_name) and isinstance(section_data, dict):
                section = getattr(s, section_name)
                if isinstance(section, BaseModel):
                    for k, v in section_data.items():
                        if hasattr(section, k):
                            setattr(section, k, v)
            elif hasattr(s, section_name):
                setattr(s, section_name, section_data)

    # 确保目录存在
    s.data_dir.mkdir(parents=True, exist_ok=True)
    s.config_dir.mkdir(parents=True, exist_ok=True)

    return s


def save_settings(s: Settings) -> None:
    """将当前设置持久化回 YAML 配置文件"""
    config_file = s.config_dir / "config.yaml"

    # 读取已有 YAML，保留未知字段
    existing: dict = {}
    if config_file.exists():
        with open(config_file) as f:
            existing = yaml.safe_load(f) or {}

    # 覆盖可写字段（只写会变的 section）
    for section_name in ("server", "embedding", "auth", "ingest", "logging", "search", "chunk"):
        section = getattr(s, section_name, None)
        if isinstance(section, BaseModel):
            existing[section_name] = section.model_dump()

    with open(config_file, "w") as f:
        yaml.safe_dump(existing, f, allow_unicode=True, default_flow_style=False)


settings = load_settings()
