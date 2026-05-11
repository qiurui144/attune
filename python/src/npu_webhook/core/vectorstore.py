"""向量存储封装：连接 ChromaDB + Embedding 引擎"""

import logging

from npu_webhook.core.embedding import EmbeddingEngine
from npu_webhook.db.chroma_db import ChromaDB

logger = logging.getLogger(__name__)


class VectorStore:
    """向量存储：负责文本的向量化存储和检索"""

    def __init__(self, chroma: ChromaDB, engine: EmbeddingEngine | None) -> None:
        self.chroma = chroma
        self.engine = engine

    @property
    def available(self) -> bool:
        return self.engine is not None

    def add(
        self,
        doc_id: str,
        text: str,
        metadata: dict | None = None,
    ) -> bool:
        """向量化并存储单条文本"""
        if not self.engine:
            return False
        embeddings = self.engine.embed([text])
        self.chroma.add(doc_id, embeddings[0], metadata=metadata, document=text)
        return True

    def add_batch(
        self,
        doc_ids: list[str],
        texts: list[str],
        metadatas: list[dict] | None = None,
    ) -> bool:
        """批量向量化并存储"""
        if not self.engine or not texts:
            return False
        embeddings = self.engine.embed(texts)
        self.chroma.add_batch(doc_ids, embeddings, metadatas=metadatas, documents=texts)
        return True

    def search(
        self,
        query: str,
        top_k: int = 10,
        where: dict | None = None,
    ) -> list[dict]:
        """向量搜索：返回 [{id, document, score, metadata}, ...]"""
        if not self.engine:
            return []
        query_embedding = self.engine.embed([query])[0]
        results = self.chroma.query(query_embedding, top_k=top_k, where=where)

        items = []
        if results and results.get("ids"):
            ids = results["ids"][0]
            documents = results.get("documents", [[]])[0]
            distances = results.get("distances", [[]])[0]
            metadatas = results.get("metadatas", [[]])[0]

            for i, doc_id in enumerate(ids):
                # ChromaDB cosine distance → similarity score
                score = 1.0 - distances[i] if i < len(distances) else 0.0
                items.append({
                    "id": doc_id,
                    "document": documents[i] if i < len(documents) else "",
                    "score": score,
                    "metadata": metadatas[i] if i < len(metadatas) else {},
                })
        return items

    def delete(self, doc_ids: list[str]) -> None:
        self.chroma.delete(doc_ids)

    def delete_by_item_ids(self, item_ids: list[str]) -> None:
        """按 item_id 删除所有关联 chunk（修复多分块孤立向量问题）"""
        self.chroma.delete_by_item_ids(item_ids)
