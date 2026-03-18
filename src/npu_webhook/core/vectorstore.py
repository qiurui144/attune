"""ChromaDB 向量存储封装"""


class VectorStore:
    """ChromaDB 封装，管理知识向量的存储和检索"""

    def __init__(self, persist_dir: str) -> None:
        self.persist_dir = persist_dir
        # TODO Phase 1: 初始化 ChromaDB client + collection
