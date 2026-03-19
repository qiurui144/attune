"""RRF 混合搜索引擎（向量 + 全文融合 + Reranker + 质量加权）"""

import logging
from collections import OrderedDict

from npu_webhook.core.fulltext import build_fts_query
from npu_webhook.core.vectorstore import VectorStore
from npu_webhook.db.sqlite_db import SQLiteDB

logger = logging.getLogger(__name__)


class _LRUCache:
    """简单 LRU 缓存（线程不安全，但在单请求链路内使用足够）"""

    def __init__(self, maxsize: int = 128) -> None:
        self._cache: OrderedDict[str, list[list[float]]] = OrderedDict()
        self._maxsize = maxsize

    def get(self, key: str) -> list[list[float]] | None:
        if key in self._cache:
            self._cache.move_to_end(key)
            return self._cache[key]
        return None

    def put(self, key: str, value: list[list[float]]) -> None:
        if key in self._cache:
            self._cache.move_to_end(key)
        else:
            if len(self._cache) >= self._maxsize:
                self._cache.popitem(last=False)
        self._cache[key] = value


class Reranker:
    """Ollama Reranker — embedding 余弦相似度精排，带 LRU 缓存"""

    def __init__(self, model: str = "bge-m3", base_url: str = "http://localhost:11434") -> None:
        import time
        self.model = model
        self.base_url = base_url.rstrip("/")
        self._available: bool | None = None
        self._probe_ts: float = 0.0
        self._probe_ttl: float = 300.0  # 5 分钟重新探测
        self._cache = _LRUCache(maxsize=256)
        self._time = time

    @property
    def available(self) -> bool:
        now = self._time.time()
        if self._available is None or (now - self._probe_ts > self._probe_ttl):
            self._available = self._probe()
            self._probe_ts = now
        return self._available

    def _probe(self) -> bool:
        import urllib.request
        try:
            with urllib.request.urlopen(f"{self.base_url}/api/tags", timeout=2):
                return True
        except Exception:
            return False

    def _embed(self, texts: list[str]) -> list[list[float]]:
        """批量 embedding，对已缓存的文本跳过请求"""
        import json
        import urllib.request

        uncached_idx = []
        uncached_texts = []
        results: list[list[float] | None] = [None] * len(texts)

        for i, t in enumerate(texts):
            key = t[:512]  # 缓存 key 截取前 512 字符
            cached = self._cache.get(key)
            if cached:
                results[i] = cached[0]
            else:
                uncached_idx.append(i)
                uncached_texts.append(key)

        if uncached_texts:
            data = json.dumps({"model": self.model, "input": uncached_texts}).encode()
            req = urllib.request.Request(
                f"{self.base_url}/api/embed",
                data=data,
                headers={"Content-Type": "application/json"},
            )
            with urllib.request.urlopen(req, timeout=30) as resp:
                api_result = json.loads(resp.read())

            for j, idx in enumerate(uncached_idx):
                emb = api_result["embeddings"][j]
                results[idx] = emb
                self._cache.put(uncached_texts[j], [emb])

        return results  # type: ignore[return-value]

    def rerank(self, query: str, documents: list[dict], top_k: int = 3) -> list[dict]:
        """余弦相似度精排"""
        if not documents or not self.available:
            return documents[:top_k]

        try:
            import numpy as np

            texts = [query[:512]] + [d.get("content", "")[:512] for d in documents]
            embeddings = self._embed(texts)

            query_emb = np.array(embeddings[0])
            for i, doc in enumerate(documents):
                doc_emb = np.array(embeddings[i + 1])
                sim = float(np.dot(query_emb, doc_emb) / max(np.linalg.norm(query_emb) * np.linalg.norm(doc_emb), 1e-12))
                doc["rerank_score"] = sim

            documents.sort(key=lambda d: d.get("rerank_score", 0), reverse=True)
            return documents[:top_k]

        except Exception as e:
            logger.warning("Rerank failed, returning original order: %s", e)
            return documents[:top_k]


class HybridSearchEngine:
    """RRF 混合搜索 + Rerank + 质量加权"""

    def __init__(
        self,
        db: SQLiteDB,
        vector_store: VectorStore,
        rrf_k: int = 60,
        vector_weight: float = 0.6,
        fulltext_weight: float = 0.4,
    ) -> None:
        self.db = db
        self.vector_store = vector_store
        self.rrf_k = rrf_k
        self.vector_weight = vector_weight
        self.fulltext_weight = fulltext_weight
        # 从配置动态获取模型名
        model_name = "bge-m3"
        try:
            from npu_webhook.config import settings
            model_name = settings.embedding.model
        except Exception:
            pass
        self.reranker = Reranker(model=model_name)

    def search(
        self,
        query: str,
        top_k: int = 10,
        source_types: list[str] | None = None,
        context: list[str] | None = None,
        min_score: float = 0.0,
        rerank: bool = False,
    ) -> list[dict]:
        """混合搜索：向量 + 全文，RRF 融合，可选 rerank"""
        # 上下文感知：截取上下文长度防止 query 膨胀，用明确分隔符
        search_query = query
        if context:
            ctx_text = " | ".join(c[:150] for c in context[-3:])
            search_query = f"{ctx_text} || {query}"

        # 1. 向量搜索
        vector_results: list[dict] = []
        if self.vector_store.available:
            where = {"source_type": {"$in": source_types}} if source_types else None
            vector_results = self.vector_store.search(search_query, top_k=top_k * 2, where=where)

        # 2. 全文搜索（用原始 query 分词更精确）
        fts_query = build_fts_query(query)
        fts_results = self.db.fts_search(fts_query, limit=top_k * 2)
        if source_types:
            fts_results = [r for r in fts_results if r.get("source_type") in source_types]

        # 3. RRF 融合
        coarse_k = top_k * 3 if rerank else top_k
        merged = self._rrf_merge(vector_results, fts_results, coarse_k)

        # 4. 阈值过滤
        if min_score > 0:
            merged = [r for r in merged if r.get("score", 0) >= min_score]

        # 5. Reranker 精排
        if rerank and len(merged) > top_k:
            merged = self.reranker.rerank(query, merged, top_k=top_k)

        return merged[:top_k]

    def _rrf_merge(
        self,
        vector_results: list[dict],
        fts_results: list[dict],
        top_k: int,
    ) -> list[dict]:
        """RRF 融合排序 + 质量加权"""
        scores: dict[str, float] = {}
        item_data: dict[str, dict] = {}

        for rank, r in enumerate(vector_results):
            item_id = r.get("metadata", {}).get("item_id", r["id"].split(":")[0])
            scores[item_id] = scores.get(item_id, 0) + self.vector_weight / (self.rrf_k + rank + 1)
            if item_id not in item_data:
                item_data[item_id] = {
                    "id": item_id,
                    "content": r.get("document", ""),
                    "source_type": r.get("metadata", {}).get("source_type", ""),
                    "vector_score": r.get("score", 0),
                }

        for rank, r in enumerate(fts_results):
            item_id = r["id"]
            scores[item_id] = scores.get(item_id, 0) + self.fulltext_weight / (self.rrf_k + rank + 1)
            if item_id not in item_data:
                item_data[item_id] = {
                    "id": item_id, "title": r.get("title", ""), "content": r.get("content", ""),
                    "source_type": r.get("source_type", ""), "url": r.get("url"), "created_at": r.get("created_at"),
                }
            else:
                item_data[item_id].setdefault("title", r.get("title", ""))
                item_data[item_id].setdefault("url", r.get("url"))
                item_data[item_id].setdefault("created_at", r.get("created_at"))

        # DB 批量补全 + quality_score 加权（一次查询替代 N+1）
        all_ids = list(scores.keys())
        db_rows = {row["id"]: row for row in self.db.get_items_batch(all_ids)}
        for item_id in all_ids:
            db_item = db_rows.get(item_id)
            if db_item:
                data = item_data.setdefault(item_id, {"id": item_id})
                data.setdefault("title", db_item["title"])
                data.setdefault("url", db_item.get("url"))
                data.setdefault("source_type", db_item["source_type"])
                data.setdefault("created_at", db_item["created_at"])
                # NULL 未评分 → 不降权(1.0)；0.0 真实低质量 → 按实际值降权
                quality = db_item.get("quality_score")
                if quality is None:
                    quality = 1.0
                scores[item_id] *= (0.8 + 0.2 * quality)

        # 只保留 DB 中存在的条目（排除软删除/已归档的幽灵向量）
        sorted_ids = sorted(
            (iid for iid in scores if iid in db_rows),
            key=lambda x: scores[x],
            reverse=True,
        )[:top_k]
        results = []
        for item_id in sorted_ids:
            data = item_data.get(item_id, {"id": item_id})
            data["score"] = scores[item_id]
            results.append(data)
        return results
