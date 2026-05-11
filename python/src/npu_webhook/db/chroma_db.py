"""ChromaDB 向量数据库封装"""

from pathlib import Path

import chromadb

COLLECTION_NAME = "knowledge_embeddings"


class ChromaDB:
    """ChromaDB 客户端封装"""

    def __init__(self, persist_dir: str | Path) -> None:
        self.client = chromadb.PersistentClient(path=str(persist_dir))
        self.collection = self.client.get_or_create_collection(
            name=COLLECTION_NAME,
            metadata={"hnsw:space": "cosine"},
        )

    def add(
        self,
        doc_id: str,
        embedding: list[float],
        metadata: dict | None = None,
        document: str = "",
    ) -> None:
        self.collection.upsert(
            ids=[doc_id],
            embeddings=[embedding],
            metadatas=[metadata or {}],
            documents=[document],
        )

    def add_batch(
        self,
        ids: list[str],
        embeddings: list[list[float]],
        metadatas: list[dict] | None = None,
        documents: list[str] | None = None,
    ) -> None:
        self.collection.upsert(
            ids=ids,
            embeddings=embeddings,
            metadatas=metadatas,
            documents=documents,
        )

    def query(
        self,
        embedding: list[float],
        top_k: int = 10,
        where: dict | None = None,
    ) -> dict:
        kwargs: dict = {
            "query_embeddings": [embedding],
            "n_results": top_k,
        }
        if where:
            kwargs["where"] = where
        return self.collection.query(**kwargs)

    def delete(self, doc_ids: list[str]) -> None:
        self.collection.delete(ids=doc_ids)

    def delete_by_item_ids(self, item_ids: list[str]) -> None:
        """按 item_id 元数据删除所有 chunk（支持多分块条目）"""
        if not item_ids:
            return
        self.collection.delete(where={"item_id": {"$in": item_ids}})

    def count(self) -> int:
        return self.collection.count()
