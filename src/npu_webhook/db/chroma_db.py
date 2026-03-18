"""ChromaDB 客户端"""

COLLECTION_NAME = "knowledge_embeddings"


class ChromaDB:
    """ChromaDB 客户端封装"""

    def __init__(self, persist_dir: str) -> None:
        self.persist_dir = persist_dir
        # TODO Phase 1: 初始化 chromadb.PersistentClient
