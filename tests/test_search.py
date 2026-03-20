"""搜索引擎测试"""

import tempfile
from pathlib import Path

from npu_webhook.core.search import HybridSearchEngine
from npu_webhook.core.vectorstore import VectorStore
from npu_webhook.db.chroma_db import ChromaDB
from npu_webhook.db.sqlite_db import SQLiteDB


def test_hybrid_search_basic():
    """基础混合搜索：无 embedding 引擎，仅 FTS5"""
    with tempfile.TemporaryDirectory() as tmpdir:
        db = SQLiteDB(Path(tmpdir) / "test.db")
        chroma = ChromaDB(Path(tmpdir) / "chroma")
        vs = VectorStore(chroma, engine=None)
        engine = HybridSearchEngine(db=db, vector_store=vs)

        # 插入数据
        db.insert_item(title="Python 教程", content="Python 是一种编程语言", source_type="note")
        db.insert_item(title="JavaScript 入门", content="JS 是前端开发的核心语言", source_type="note")

        # 搜索
        results = engine.search("Python 编程")
        assert isinstance(results, list)

        db.close()


def test_fts_search():
    """FTS5 全文搜索"""
    with tempfile.TemporaryDirectory() as tmpdir:
        db = SQLiteDB(Path(tmpdir) / "test.db")

        db.insert_item(title="深度学习入门", content="深度学习是机器学习的一个分支", source_type="note")
        db.insert_item(title="Web 开发", content="前端和后端开发技术", source_type="note")

        results = db.fts_search("深度学习")
        assert len(results) >= 1
        assert "深度学习" in results[0]["title"]

        db.close()


def test_find_near_duplicate():
    """入库前近重复检测：前 200 字符相同视为重复"""
    with tempfile.TemporaryDirectory() as tmpdir:
        db = SQLiteDB(Path(tmpdir) / "test.db")

        content = "这是一段很长的测试文本内容，用于验证近重复检测逻辑。" * 20
        db.insert_item(title="原始条目", content=content, source_type="ai_chat")

        # 相同前缀 → 返回已有 id
        assert db.find_near_duplicate(content, "ai_chat") is not None

        # 不同 source_type → 不视为重复
        assert db.find_near_duplicate(content, "note") is None

        # 完全不同内容 → 不重复
        assert db.find_near_duplicate("完全不同的内容，没有任何相似之处。" * 10, "ai_chat") is None

        db.close()


def test_get_items_batch():
    """批量获取条目：一次查询替代 N+1"""
    with tempfile.TemporaryDirectory() as tmpdir:
        db = SQLiteDB(Path(tmpdir) / "test.db")

        ids = [
            db.insert_item(title=f"条目{i}", content=f"内容{i}" * 20, source_type="note")
            for i in range(5)
        ]

        results = db.get_items_batch(ids)
        assert len(results) == 5
        assert {r["id"] for r in results} == set(ids)
        assert db.get_items_batch([]) == []

        db.close()


def test_bulk_archive_stale():
    """批量软删除冷 ai_chat 条目"""
    with tempfile.TemporaryDirectory() as tmpdir:
        db = SQLiteDB(Path(tmpdir) / "test.db")

        item_id = db.insert_item(
            title="旧对话", content="这是一段很老的对话内容。" * 10, source_type="ai_chat"
        )
        # 模拟 31 天前创建
        db.conn.execute(
            "UPDATE knowledge_items SET created_at = datetime('now', '-31 days') WHERE id = ?",
            (item_id,),
        )
        db.conn.commit()

        archived = db.bulk_archive_stale(
            quality_threshold=0.2, unused_days=60, chat_unused_days=30
        )
        assert item_id in archived
        assert db.get_item(item_id) is None  # 软删除后 get_item 返回 None

        db.close()


def test_rrf_merge_excludes_deleted_items():
    """_rrf_merge 必须过滤软删除/已归档条目（即使向量库中仍有对应向量）"""
    with tempfile.TemporaryDirectory() as tmpdir:
        db = SQLiteDB(Path(tmpdir) / "test.db")
        chroma = ChromaDB(Path(tmpdir) / "chroma")
        vs = VectorStore(chroma, engine=None)
        engine = HybridSearchEngine(db=db, vector_store=vs)

        # 插入正常条目
        good_id = db.insert_item(title="正常条目", content="这是一个正常的知识条目", source_type="note")
        # 插入后立即软删除（模拟归档），但不清理向量（模拟清理失败场景）
        deleted_id = db.insert_item(title="已删除条目", content="这是一个已被删除的条目", source_type="note")
        db.delete_item(deleted_id)

        # 模拟向量库中仍存在已删除条目的向量
        dummy_emb = [0.1] * 10
        chroma.add(f"{deleted_id}:0", dummy_emb, metadata={"item_id": deleted_id}, document="已删除内容")
        chroma.add(f"{good_id}:0", dummy_emb, metadata={"item_id": good_id}, document="正常内容")

        # _rrf_merge 需过滤已删除条目（仅来自 FTS 的结果，因为 vs 无 embedding 引擎）
        results = engine.search("条目")
        result_ids = {r["id"] for r in results}
        assert deleted_id not in result_ids, "已软删除的条目不应出现在搜索结果中"

        db.close()


def test_bulk_archive_cancels_pending_embeddings():
    """bulk_archive_stale 应同时取消被归档条目的 pending embedding 任务"""
    with tempfile.TemporaryDirectory() as tmpdir:
        db = SQLiteDB(Path(tmpdir) / "test.db")

        item_id = db.insert_item(
            title="旧对话", content="这是一段很老的对话内容。" * 10, source_type="ai_chat"
        )
        db.conn.execute(
            "UPDATE knowledge_items SET created_at = datetime('now', '-31 days') WHERE id = ?",
            (item_id,),
        )
        db.conn.commit()

        # 模拟有 pending embedding 任务
        queue_id = db.enqueue_embedding(item_id, chunk_text="旧内容")
        assert db.pending_embedding_count() == 1

        # 批量归档应同时取消 embedding 任务
        archived = db.bulk_archive_stale(quality_threshold=0.2, unused_days=60, chat_unused_days=30)
        assert item_id in archived
        assert db.pending_embedding_count() == 0

        # 确认任务已被标记为 abandoned
        row = db.conn.execute(
            "SELECT status FROM embedding_queue WHERE id = ?", (queue_id,)
        ).fetchone()
        assert row is not None and row["status"] == "abandoned"

        db.close()


def test_fail_embedding_atomic_increment():
    """fail_embedding 应原子递增 attempts，超过阈值后标记为 abandoned"""
    with tempfile.TemporaryDirectory() as tmpdir:
        db = SQLiteDB(Path(tmpdir) / "test.db")

        item_id = db.insert_item(title="测试", content="内容", source_type="note")
        queue_id = db.enqueue_embedding(item_id, chunk_text="内容")

        # 前 2 次失败 → 保持 pending
        db.fail_embedding(queue_id, max_attempts=3)
        row = db.conn.execute(
            "SELECT status, attempts FROM embedding_queue WHERE id = ?", (queue_id,)
        ).fetchone()
        assert row["status"] == "pending" and row["attempts"] == 1

        db.fail_embedding(queue_id, max_attempts=3)
        row = db.conn.execute(
            "SELECT status, attempts FROM embedding_queue WHERE id = ?", (queue_id,)
        ).fetchone()
        assert row["status"] == "pending" and row["attempts"] == 2

        # 第 3 次失败 → abandoned
        db.fail_embedding(queue_id, max_attempts=3)
        row = db.conn.execute(
            "SELECT status, attempts FROM embedding_queue WHERE id = ?", (queue_id,)
        ).fetchone()
        assert row["status"] == "abandoned" and row["attempts"] == 3

        db.close()


def test_chroma_delete_by_item_ids():
    """ChromaDB 按 item_id 元数据删除所有 chunk，防止孤立向量"""
    with tempfile.TemporaryDirectory() as tmpdir:
        chroma = ChromaDB(Path(tmpdir) / "chroma")

        dummy_emb = [0.1] * 10
        chroma.add("item1:0", dummy_emb, metadata={"item_id": "item1"}, document="chunk0")
        chroma.add("item1:1", dummy_emb, metadata={"item_id": "item1"}, document="chunk1")
        chroma.add("item2:0", dummy_emb, metadata={"item_id": "item2"}, document="other")

        assert chroma.count() == 3
        chroma.delete_by_item_ids(["item1"])
        assert chroma.count() == 1  # 只剩 item2:0


def test_enqueue_embedding_with_level():
    """enqueue_embedding 接受 level / section_idx 参数"""
    with tempfile.TemporaryDirectory() as tmpdir:
        db = SQLiteDB(Path(tmpdir) / "test.db")
        item_id = db.insert_item(title="t", content="c" * 200, source_type="file")
        qid = db.enqueue_embedding(
            item_id, chunk_index=0, chunk_text="text", priority=1, level=1, section_idx=2
        )
        row = db.conn.execute(
            "SELECT level, section_idx FROM embedding_queue WHERE id = ?", (qid,)
        ).fetchone()
        assert row["level"] == 1
        assert row["section_idx"] == 2
        db.close()
