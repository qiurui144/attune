"""Pydantic Settings 配置管理"""

from pathlib import Path

from pydantic import BaseModel
from pydantic_settings import BaseSettings


class ServerConfig(BaseModel):
    host: str = "127.0.0.1"
    port: int = 18900


class EmbeddingConfig(BaseModel):
    model: str = "bge-small-zh-v1.5"
    device: str = "auto"  # auto/cpu/npu/gpu
    batch_size: int = 16


class AuthConfig(BaseModel):
    mode: str = "localhost"  # localhost/token


class IngestConfig(BaseModel):
    min_content_length: int = 100
    excluded_domains: list[str] = ["mail.google.com", "web.whatsapp.com"]


class LoggingConfig(BaseModel):
    level: str = "INFO"
    max_size_mb: int = 50


class Settings(BaseSettings):
    server: ServerConfig = ServerConfig()
    embedding: EmbeddingConfig = EmbeddingConfig()
    auth: AuthConfig = AuthConfig()
    ingest: IngestConfig = IngestConfig()
    logging: LoggingConfig = LoggingConfig()

    # 运行时计算的路径
    data_dir: Path = Path("")
    config_dir: Path = Path("")


settings = Settings()
