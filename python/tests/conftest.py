"""测试配置：在 API 测试前初始化 state"""

import tempfile
from pathlib import Path

import pytest

from npu_webhook.app_state import state
from npu_webhook.core.chunker import Chunker
from npu_webhook.core.search import HybridSearchEngine
from npu_webhook.core.vectorstore import VectorStore
from npu_webhook.db.chroma_db import ChromaDB
from npu_webhook.db.sqlite_db import SQLiteDB


@pytest.fixture(autouse=True)
def init_test_state():
    """自动为每个测试初始化临时 state（如果 state.db 为空）"""
    if state.db is not None:
        yield
        return

    with tempfile.TemporaryDirectory() as tmpdir:
        tmpdir = Path(tmpdir)
        state.db = SQLiteDB(tmpdir / "test.db")
        state.chroma = ChromaDB(tmpdir / "chroma")
        state.vector_store = VectorStore(state.chroma, engine=None)
        state.chunker = Chunker()
        state.search_engine = HybridSearchEngine(
            db=state.db, vector_store=state.vector_store
        )

        yield

        state.db.close()
        state.db = None
        state.chroma = None
        state.vector_store = None
        state.chunker = None
        state.search_engine = None
