"""应用全局状态：在 lifespan 中初始化，API 中使用"""

from dataclasses import dataclass, field

from attune_python.core.chunker import Chunker
from attune_python.core.embedding import EmbeddingEngine
from attune_python.core.search import HybridSearchEngine
from attune_python.core.vectorstore import VectorStore
from attune_python.db.chroma_db import ChromaDB
from attune_python.db.sqlite_db import SQLiteDB
from attune_python.indexer.pipeline import IndexPipeline
from attune_python.indexer.watcher import DirectoryWatcher
from attune_python.scheduler.cleaner import KnowledgeCleaner
from attune_python.scheduler.queue import EmbeddingQueueWorker


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
