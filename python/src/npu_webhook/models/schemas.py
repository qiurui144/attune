"""Pydantic 数据模型"""

from datetime import datetime

from pydantic import BaseModel, Field


class IngestRequest(BaseModel):
    """知识注入请求"""

    title: str
    content: str
    url: str | None = None
    source_type: str = "webpage"  # webpage/ai_chat/selection/file/note
    domain: str | None = None
    tags: list[str] = []
    metadata: dict[str, str] = {}


class IngestResponse(BaseModel):
    id: str
    status: str = "ok"
    duplicate: bool = False  # True 表示内容已存在，返回已有条目 ID


class SearchQuery(BaseModel):
    """搜索请求参数"""

    q: str
    top_k: int = 10
    source_types: str | None = None  # 逗号分隔


class RelevantRequest(BaseModel):
    """注入用相关知识请求"""

    query: str
    top_k: int = 3
    source_types: list[str] | None = None
    context: list[str] | None = None  # 最近对话上下文（用于上下文感知搜索）
    min_score: float = 0.0  # 最低分数阈值，低于此分数的结果不返回


class SearchResult(BaseModel):
    """搜索结果条目"""

    id: str
    title: str = ""
    content: str
    score: float
    source_type: str = ""
    url: str | None = None
    created_at: str | None = None


class SearchResponse(BaseModel):
    results: list[SearchResult]
    total: int
    feedback_ids: list[int] = []  # 注入反馈 ID（用于后续提交有用/无用反馈）


class KnowledgeItem(BaseModel):
    """知识条目"""

    id: str
    title: str
    content: str
    url: str | None = None
    source_type: str
    domain: str | None = None
    tags: list[str] = []
    metadata: dict[str, str] = {}
    created_at: str
    updated_at: str


class ItemListResponse(BaseModel):
    items: list[KnowledgeItem]
    total: int
    offset: int
    limit: int


class ItemUpdateRequest(BaseModel):
    title: str | None = None
    tags: list[str] | None = None
    metadata: dict[str, str] | None = None


class BindDirectoryRequest(BaseModel):
    path: str
    recursive: bool = True
    file_types: list[str] = Field(default=["md", "txt", "pdf", "docx", "py", "js"])


class SystemStatus(BaseModel):
    version: str
    device: str
    model_name: str
    embedding_available: bool
    total_items: int
    total_vectors: int
    pending_embeddings: int
    bound_directories: int


class SettingsResponse(BaseModel):
    server_host: str
    server_port: int
    embedding_model: str
    embedding_device: str
    embedding_batch_size: int
    ingest_min_length: int
    excluded_domains: list[str]


class SettingsUpdateRequest(BaseModel):
    embedding_model: str | None = None
    embedding_device: str | None = None
    embedding_batch_size: int | None = None
    ingest_min_length: int | None = None
    excluded_domains: list[str] | None = None
