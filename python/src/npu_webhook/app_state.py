"""应用全局状态：在 lifespan 中初始化，API 中使用"""

from dataclasses import dataclass, field

from npu_webhook.core.chunker import Chunker
from npu_webhook.core.embedding import EmbeddingEngine
from npu_webhook.core.search import HybridSearchEngine
from npu_webhook.core.vectorstore import VectorStore
from npu_webhook.db.chroma_db import ChromaDB
from npu_webhook.db.sqlite_db import SQLiteDB
from npu_webhook.indexer.pipeline import IndexPipeline
from npu_webhook.indexer.watcher import DirectoryWatcher
from npu_webhook.scheduler.cleaner import KnowledgeCleaner
from npu_webhook.scheduler.queue import EmbeddingQueueWorker


@dataclass
class AppState:
    """应用全局状态容器"""

    db: SQLiteDB | None = None
    chroma: ChromaDB | None = None
    embedding_engine: EmbeddingEngine | None = None
    vector_store: VectorStore | None = None
    search_engine: HybridSearchEngine | None = None
    chunker: Chunker | None = None
    pipeline: IndexPipeline | None = None
    watcher: DirectoryWatcher | None = None
    queue_worker: EmbeddingQueueWorker | None = None
    cleaner: KnowledgeCleaner | None = None
    session_upload_ids: dict = field(default_factory=dict)


# 全局单例
state = AppState()
