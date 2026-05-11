"""文件索引测试"""

import tempfile
from pathlib import Path

from npu_webhook.core.chunker import Chunker
from npu_webhook.core.parser import parse_file
from npu_webhook.db.sqlite_db import SQLiteDB
from npu_webhook.indexer.pipeline import IndexPipeline


def test_chunker_short_text():
    chunker = Chunker(chunk_size=100, overlap=20)
    chunks = chunker.chunk("这是一段短文本")
    assert len(chunks) == 1


def test_chunker_long_text():
    chunker = Chunker(chunk_size=50, overlap=10)
    text = "这是一段较长的文本。" * 20
    chunks = chunker.chunk(text)
    assert len(chunks) > 1


def test_chunker_empty():
    chunker = Chunker()
    assert chunker.chunk("") == []
    assert chunker.chunk("  ") == []


def test_parse_markdown():
    with tempfile.NamedTemporaryFile(suffix=".md", mode="w", delete=False, encoding="utf-8") as f:
        f.write("# 测试标题\n\n这是测试内容。\n")
        f.flush()
        title, content = parse_file(f.name)
        assert title == "测试标题"
        assert "测试内容" in content


def test_parse_text():
    with tempfile.NamedTemporaryFile(suffix=".txt", mode="w", delete=False, encoding="utf-8") as f:
        f.write("第一行作为标题\n第二行是内容\n")
        f.flush()
        title, content = parse_file(f.name)
        assert "第一行" in title


def test_pipeline_process_file():
    with tempfile.TemporaryDirectory() as tmpdir:
        db = SQLiteDB(Path(tmpdir) / "test.db")
        chunker = Chunker(chunk_size=100, overlap=20)
        pipeline = IndexPipeline(db, chunker)

        # 创建测试文件
        test_file = Path(tmpdir) / "test.md"
        test_file.write_text("# 知识库测试\n\n这是一篇测试文档，内容足够长。" * 5, encoding="utf-8")

        item_id = pipeline.process_file(str(test_file), dir_id="test-dir")
        assert item_id is not None

        # 验证存储
        item = db.get_item(item_id)
        assert item is not None
        assert item["title"] == "知识库测试"

        # 验证 embedding 队列
        assert db.pending_embedding_count() > 0

        # 再次处理同文件应跳过（hash 未变）
        item_id2 = pipeline.process_file(str(test_file), dir_id="test-dir")
        assert item_id2 == item_id

        db.close()
