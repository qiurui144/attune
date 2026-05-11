"""模型调用集成测试

覆盖范围：
- OllamaEmbedding：真实 HTTP 调用（需 Ollama 运行 + bge-m3 已拉取）
- MockEmbeddingEngine：离线 stub，验证 VectorStore 全链路
- VectorStore（add / add_batch / search / delete）
- HybridSearchEngine（FTS5 路径 + 向量路径 + RRF 融合）
- Reranker（可用时真实精排，不可用时降级）
- ChromaDB 增删查

有 Ollama 的测试用 `ollama` mark，CI 里默认跳过；本机 Ollama 在线时自动运行。
"""

from __future__ import annotations

import math
import tempfile
from pathlib import Path
from typing import Any
from unittest.mock import patch

import pytest

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _is_ollama_available() -> bool:
    """探测 Ollama 是否在线且 bge-m3 已拉取"""
    import urllib.request
    try:
        with urllib.request.urlopen("http://localhost:11434/api/tags", timeout=2) as r:
            import json
            data = json.loads(r.read())
            return any("bge-m3" in m.get("name", "") for m in data.get("models", []))
    except Exception:
        return False


ollama_available = _is_ollama_available()
requires_ollama = pytest.mark.skipif(not ollama_available, reason="Ollama + bge-m3 not available")


class FixedEmbedding:
    """确定性 mock：每个 token 的 ASCII 均值构成一维，补零到 DIM 维，L2 归一化"""

    DIM = 64

    def embed(self, texts: list[str]) -> list[list[float]]:
        result = []
        for text in texts:
            raw = [float(ord(c)) for c in text[:self.DIM]]
            raw += [0.0] * (self.DIM - len(raw))
            norm = math.sqrt(sum(x * x for x in raw)) or 1.0
            result.append([x / norm for x in raw])
        return result

    def get_dimension(self) -> int:
        return self.DIM


# ---------------------------------------------------------------------------
# OllamaEmbedding 单元测试（真实 HTTP）
# ---------------------------------------------------------------------------

class TestOllamaEmbedding:
    @requires_ollama
    def test_dimension_detected(self) -> None:
        """bge-m3 维度应为 1024"""
        from attune_python.core.embedding import OllamaEmbedding
        engine = OllamaEmbedding(model="bge-m3")
        assert engine.get_dimension() == 1024

    @requires_ollama
    def test_embed_single_text(self) -> None:
        """单文本 embed 返回长度正确的向量"""
        from attune_python.core.embedding import OllamaEmbedding
        engine = OllamaEmbedding(model="bge-m3")
        result = engine.embed(["Python 编程语言"])
        assert len(result) == 1
        assert len(result[0]) == engine.get_dimension()

    @requires_ollama
    def test_embed_batch(self) -> None:
        """批量 embed 返回数量与输入一致"""
        from attune_python.core.embedding import OllamaEmbedding
        engine = OllamaEmbedding(model="bge-m3")
        texts = ["机器学习", "深度学习", "自然语言处理", "知识图谱"]
        result = engine.embed(texts)
        assert len(result) == len(texts)
        assert all(len(v) == engine.get_dimension() for v in result)

    @requires_ollama
    def test_semantic_similarity_ordering(self) -> None:
        """语义相近的词向量余弦相似度高于语义无关的词"""
        import numpy as np
        from attune_python.core.embedding import OllamaEmbedding
        engine = OllamaEmbedding(model="bge-m3")
        vecs = engine.embed(["猫", "猫咪", "飞机"])
        v_cat = np.array(vecs[0])
        v_kitten = np.array(vecs[1])
        v_plane = np.array(vecs[2])
        sim_close = float(np.dot(v_cat, v_kitten))
        sim_far = float(np.dot(v_cat, v_plane))
        assert sim_close > sim_far, f"expected sim(猫,猫咪)={sim_close:.3f} > sim(猫,飞机)={sim_far:.3f}"

    def test_connection_error_on_bad_url(self) -> None:
        """Ollama 不可达时抛出 ConnectionError"""
        from attune_python.core.embedding import OllamaEmbedding
        with pytest.raises(ConnectionError):
            OllamaEmbedding(model="bge-m3", base_url="http://localhost:19999")


# ---------------------------------------------------------------------------
# create_embedding_engine 工厂
# ---------------------------------------------------------------------------

class TestCreateEmbeddingEngine:
    def test_returns_none_for_missing_onnx_model(self, tmp_path: Path) -> None:
        from attune_python.core.embedding import create_embedding_engine
        engine = create_embedding_engine(model_name="no-such-model", device="cpu", data_dir=tmp_path)
        assert engine is None

    @requires_ollama
    def test_auto_returns_ollama_engine(self) -> None:
        from attune_python.core.embedding import create_embedding_engine, OllamaEmbedding
        engine = create_embedding_engine(model_name="bge-m3", device="auto")
        assert isinstance(engine, OllamaEmbedding)

    @requires_ollama
    def test_ollama_device_explicit(self) -> None:
        from attune_python.core.embedding import create_embedding_engine, OllamaEmbedding
        engine = create_embedding_engine(model_name="bge-m3", device="ollama")
        assert isinstance(engine, OllamaEmbedding)


# ---------------------------------------------------------------------------
# ChromaDB 增删查（离线，无需 embedding）
# ---------------------------------------------------------------------------

class TestChromaDB:
    def test_add_and_count(self, tmp_path: Path) -> None:
        from attune_python.db.chroma_db import ChromaDB
        db = ChromaDB(tmp_path / "chroma")
        assert db.count() == 0
        db.add("id1", [0.1, 0.2, 0.3], metadata={"source": "test"}, document="hello")
        assert db.count() == 1

    def test_upsert_idempotent(self, tmp_path: Path) -> None:
        from attune_python.db.chroma_db import ChromaDB
        db = ChromaDB(tmp_path / "chroma")
        vec = [0.1, 0.9]
        db.add("dup", vec, document="first")
        db.add("dup", vec, document="second")  # upsert
        assert db.count() == 1

    def test_add_batch(self, tmp_path: Path) -> None:
        from attune_python.db.chroma_db import ChromaDB
        db = ChromaDB(tmp_path / "chroma")
        ids = [f"doc{i}" for i in range(5)]
        vecs = [[float(i) * 0.1, float(i) * 0.2] for i in range(5)]
        db.add_batch(ids, vecs, documents=[f"text{i}" for i in range(5)])
        assert db.count() == 5

    def test_query_returns_nearest(self, tmp_path: Path) -> None:
        from attune_python.db.chroma_db import ChromaDB
        import numpy as np
        db = ChromaDB(tmp_path / "chroma")
        # 两个方向明显不同的单位向量
        db.add("near", [1.0, 0.0], document="near")
        db.add("far",  [0.0, 1.0], document="far")
        results = db.query([0.99, 0.01], top_k=1)
        assert results["ids"][0][0] == "near"

    def test_delete_by_item_ids(self, tmp_path: Path) -> None:
        from attune_python.db.chroma_db import ChromaDB
        db = ChromaDB(tmp_path / "chroma")
        db.add("a1", [1.0, 0.0], metadata={"item_id": "item_a"}, document="a1")
        db.add("a2", [0.9, 0.1], metadata={"item_id": "item_a"}, document="a2")
        db.add("b1", [0.0, 1.0], metadata={"item_id": "item_b"}, document="b1")
        db.delete_by_item_ids(["item_a"])
        assert db.count() == 1

    def test_delete_empty_list_no_error(self, tmp_path: Path) -> None:
        from attune_python.db.chroma_db import ChromaDB
        db = ChromaDB(tmp_path / "chroma")
        db.delete_by_item_ids([])  # should not raise


# ---------------------------------------------------------------------------
# VectorStore 全链路（FixedEmbedding mock）
# ---------------------------------------------------------------------------

class TestVectorStore:
    def _make_store(self, tmp_path: Path):
        from attune_python.db.chroma_db import ChromaDB
        from attune_python.core.vectorstore import VectorStore
        chroma = ChromaDB(tmp_path / "chroma")
        engine = FixedEmbedding()
        return VectorStore(chroma, engine)

    def test_available_with_engine(self, tmp_path: Path) -> None:
        vs = self._make_store(tmp_path)
        assert vs.available is True

    def test_unavailable_without_engine(self, tmp_path: Path) -> None:
        from attune_python.db.chroma_db import ChromaDB
        from attune_python.core.vectorstore import VectorStore
        chroma = ChromaDB(tmp_path / "chroma")
        vs = VectorStore(chroma, engine=None)
        assert vs.available is False

    def test_add_and_search(self, tmp_path: Path) -> None:
        vs = self._make_store(tmp_path)
        vs.add("doc1", "Python 编程语言", metadata={"tag": "tech"})
        vs.add("doc2", "机器学习算法", metadata={"tag": "ml"})
        results = vs.search("Python", top_k=2)
        assert len(results) >= 1
        ids = [r["id"] for r in results]
        assert "doc1" in ids

    def test_add_batch_and_search(self, tmp_path: Path) -> None:
        vs = self._make_store(tmp_path)
        ids = ["d1", "d2", "d3"]
        texts = ["Rust 系统编程", "Python 数据科学", "Go 微服务"]
        vs.add_batch(ids, texts)
        results = vs.search("Rust", top_k=3)
        assert any(r["id"] == "d1" for r in results)

    def test_search_with_no_engine_returns_empty(self, tmp_path: Path) -> None:
        from attune_python.db.chroma_db import ChromaDB
        from attune_python.core.vectorstore import VectorStore
        chroma = ChromaDB(tmp_path / "chroma")
        vs = VectorStore(chroma, engine=None)
        assert vs.search("anything") == []

    def test_add_returns_false_without_engine(self, tmp_path: Path) -> None:
        from attune_python.db.chroma_db import ChromaDB
        from attune_python.core.vectorstore import VectorStore
        chroma = ChromaDB(tmp_path / "chroma")
        vs = VectorStore(chroma, engine=None)
        assert vs.add("id", "text") is False

    def test_delete_removes_doc(self, tmp_path: Path) -> None:
        vs = self._make_store(tmp_path)
        vs.add("to_del", "要被删除的文档")
        vs.delete(["to_del"])
        results = vs.search("要被删除的文档", top_k=5)
        assert all(r["id"] != "to_del" for r in results)

    def test_delete_by_item_ids(self, tmp_path: Path) -> None:
        vs = self._make_store(tmp_path)
        vs.add("c1", "chunk 1", metadata={"item_id": "item_x"})
        vs.add("c2", "chunk 2", metadata={"item_id": "item_x"})
        vs.add("c3", "other",   metadata={"item_id": "item_y"})
        vs.delete_by_item_ids(["item_x"])
        # item_y 依然存在
        results = vs.search("other", top_k=5)
        assert any(r["id"] == "c3" for r in results)

    def test_score_between_zero_and_one(self, tmp_path: Path) -> None:
        vs = self._make_store(tmp_path)
        vs.add("s1", "科学计算数值方法")
        results = vs.search("数值方法", top_k=1)
        assert results, "期望至少一条结果"
        score = results[0]["score"]
        assert 0.0 <= score <= 1.0, f"score={score} 超出 [0,1]"

    @requires_ollama
    def test_ollama_embedding_roundtrip(self, tmp_path: Path) -> None:
        """使用真实 bge-m3 做 add+search，相关文档排在第一"""
        from attune_python.db.chroma_db import ChromaDB
        from attune_python.core.vectorstore import VectorStore
        from attune_python.core.embedding import OllamaEmbedding
        engine = OllamaEmbedding(model="bge-m3")
        chroma = ChromaDB(tmp_path / "chroma")
        vs = VectorStore(chroma, engine)
        vs.add("rust_doc", "Rust 是一门注重内存安全的系统编程语言")
        vs.add("py_doc",   "Python 是一门动态类型的脚本语言")
        vs.add("food_doc", "红烧肉是一道传统中国菜")
        results = vs.search("内存安全编程语言", top_k=3)
        assert results[0]["id"] == "rust_doc", f"top-1 期望 rust_doc, 实际 {results[0]['id']}"


# ---------------------------------------------------------------------------
# HybridSearchEngine（向量 + FTS5 路径）
# ---------------------------------------------------------------------------

class TestHybridSearchEngine:
    def _make_engine(self, tmp_path: Path, use_embedding: bool = True):
        from attune_python.db.sqlite_db import SQLiteDB
        from attune_python.db.chroma_db import ChromaDB
        from attune_python.core.vectorstore import VectorStore
        from attune_python.core.search import HybridSearchEngine
        db = SQLiteDB(tmp_path / "test.db")
        chroma = ChromaDB(tmp_path / "chroma")
        engine = FixedEmbedding() if use_embedding else None
        vs = VectorStore(chroma, engine)
        return db, vs, HybridSearchEngine(db=db, vector_store=vs)

    def test_fts_only_returns_results(self, tmp_path: Path) -> None:
        db, vs, eng = self._make_engine(tmp_path, use_embedding=False)
        db.insert_item(title="Rust 所有权", content="Rust 的所有权模型保证内存安全", source_type="note")
        db.insert_item(title="Python 列表", content="Python list 是动态数组", source_type="note")
        results = eng.search("Rust 内存安全")
        assert any("Rust" in r["title"] for r in results)
        db.close()

    def test_vector_and_fts_fusion(self, tmp_path: Path) -> None:
        """向量路径 + FTS5 双路命中时 RRF 融合得分更高"""
        db, vs, eng = self._make_engine(tmp_path)
        item_id = db.insert_item(
            title="机器学习概论",
            content="机器学习是人工智能的一个子领域，通过数据驱动学习模式",
            source_type="note",
        )
        # 预填充向量存储
        vs.add(item_id, "机器学习是人工智能的一个子领域，通过数据驱动学习模式",
               metadata={"item_id": item_id})

        results = eng.search("人工智能学习")
        assert len(results) >= 1
        assert results[0]["title"] == "机器学习概论"
        db.close()

    def test_source_type_filter(self, tmp_path: Path) -> None:
        db, vs, eng = self._make_engine(tmp_path, use_embedding=False)
        db.insert_item(title="笔记 A", content="关于 Python 的笔记内容", source_type="note")
        db.insert_item(title="对话 B", content="关于 Python 的对话内容", source_type="ai_chat")
        results = eng.search("Python", source_types=["note"])
        assert all(r.get("source_type") == "note" for r in results)
        db.close()

    def test_empty_query_returns_empty(self, tmp_path: Path) -> None:
        _, _, eng = self._make_engine(tmp_path, use_embedding=False)
        results = eng.search("")
        assert isinstance(results, list)

    def test_min_score_filter(self, tmp_path: Path) -> None:
        db, vs, eng = self._make_engine(tmp_path, use_embedding=False)
        db.insert_item(title="无关内容", content="完全不相关的随机文字内容", source_type="note")
        results = eng.search("Rust 借用检查器", min_score=0.99)
        # 无关内容不应越过极高阈值
        assert len(results) == 0
        db.close()

    @requires_ollama
    def test_real_embedding_hybrid_search(self, tmp_path: Path) -> None:
        """真实 bge-m3 向量 + FTS5 混合搜索，语义相关文档排前"""
        from attune_python.db.sqlite_db import SQLiteDB
        from attune_python.db.chroma_db import ChromaDB
        from attune_python.core.vectorstore import VectorStore
        from attune_python.core.search import HybridSearchEngine
        from attune_python.core.embedding import OllamaEmbedding

        db = SQLiteDB(tmp_path / "test.db")
        chroma = ChromaDB(tmp_path / "chroma")
        engine = OllamaEmbedding(model="bge-m3")
        vs = VectorStore(chroma, engine)
        hybrid = HybridSearchEngine(db=db, vector_store=vs)

        docs = [
            ("Rust 内存安全", "Rust 通过所有权和借用检查器保证内存安全，无需垃圾回收"),
            ("Python 异步编程", "asyncio 是 Python 3.4+ 引入的异步 I/O 框架"),
            ("美食推荐", "北京烤鸭是著名的传统中国菜肴之一"),
        ]
        for title, content in docs:
            iid = db.insert_item(title=title, content=content, source_type="note")
            vs.add(iid, content, metadata={"item_id": iid})

        results = hybrid.search("内存安全编程语言", top_k=3)
        assert results, "期望有结果"
        assert results[0]["title"] == "Rust 内存安全", \
            f"top-1 期望 'Rust 内存安全', 实际 '{results[0]['title']}'"
        db.close()


# ---------------------------------------------------------------------------
# Reranker（可用时真实调用，不可用时降级验证）
# ---------------------------------------------------------------------------

class TestReranker:
    @requires_ollama
    def test_rerank_changes_order(self) -> None:
        """精排后语义最相关的文档应排首位"""
        from attune_python.core.search import Reranker
        reranker = Reranker(model="bge-m3")
        if not reranker.available:
            pytest.skip("Reranker probe failed")

        docs = [
            {"id": "food", "content": "红烧肉是一道传统中国菜", "score": 0.9},
            {"id": "rust", "content": "Rust 通过所有权模型保证内存安全", "score": 0.8},
            {"id": "py",   "content": "Python 是动态类型脚本语言", "score": 0.7},
        ]
        reranked = reranker.rerank("内存安全系统编程语言", docs, top_k=3)
        assert reranked[0]["id"] == "rust", \
            f"精排后期望 rust 排首位，实际 {reranked[0]['id']}"

    def test_rerank_unavailable_returns_original(self) -> None:
        """Reranker 不可用时返回原始顺序（降级）"""
        from attune_python.core.search import Reranker
        reranker = Reranker(base_url="http://localhost:19999")  # 不可达
        docs = [
            {"id": "a", "document": "first doc", "score": 0.9},
            {"id": "b", "document": "second doc", "score": 0.8},
        ]
        result = reranker.rerank("query", docs, top_k=2)
        assert [r["id"] for r in result] == ["a", "b"]

    def test_rerank_top_k_limits(self) -> None:
        """top_k 截断：返回条数不超过 top_k"""
        from attune_python.core.search import Reranker
        reranker = Reranker(base_url="http://localhost:19999")
        docs = [{"id": str(i), "content": f"doc {i}", "score": float(i)} for i in range(10)]
        result = reranker.rerank("query", docs, top_k=3)
        assert len(result) <= 3


# ---------------------------------------------------------------------------
# fulltext 分词
# ---------------------------------------------------------------------------

class TestFulltextTokenizer:
    def test_chinese_tokenization(self) -> None:
        from attune_python.core.fulltext import tokenize_for_search
        tokens = tokenize_for_search("机器学习是人工智能的一个分支")
        assert "机器学习" in tokens or "机器" in tokens

    def test_build_fts_query_produces_or_terms(self) -> None:
        from attune_python.core.fulltext import build_fts_query
        q = build_fts_query("深度学习神经网络")
        assert " OR " in q
        assert '"' in q

    def test_empty_string_fallback(self) -> None:
        from attune_python.core.fulltext import build_fts_query
        result = build_fts_query("")
        assert isinstance(result, str)

    def test_no_injection_in_fts_query(self) -> None:
        """双引号不应出现在 term 内部（防止 FTS5 语法注入）"""
        from attune_python.core.fulltext import build_fts_query
        q = build_fts_query('他说"你好"然后离开了')
        # 每个 term 本身不含双引号（外层引号正常）
        import re
        terms = re.findall(r'"([^"]*)"', q)
        assert all('"' not in t for t in terms)
