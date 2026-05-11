"""索引管道：文件变更 → 解析 → 分块 → SQLite 存储 → 投递 embedding 队列"""

import hashlib
import json
import logging
from pathlib import Path

from npu_webhook.core.chunker import Chunker
from npu_webhook.core.parser import parse_file
from npu_webhook.core.vectorstore import VectorStore
from npu_webhook.db.sqlite_db import SQLiteDB

logger = logging.getLogger(__name__)


class IndexPipeline:
    """文件索引处理管道"""

    def __init__(self, db: SQLiteDB, chunker: Chunker, vector_store: VectorStore | None = None) -> None:
        self.db = db
        self.chunker = chunker
        self.vector_store = vector_store

    def process_file(self, file_path: str, dir_id: str = "", priority: int = 2) -> str | None:
        """处理单个文件：解析 → 分块 → 存储 → 投递 embedding 队列

        返回 item_id 或 None（跳过/失败）
        """
        path = Path(file_path).resolve()
        if not path.exists() or not path.is_file():
            return None

        # 计算文件 hash，检查是否已索引且未变更
        file_hash = self._file_hash(path)
        existing = self.db.get_indexed_file(str(path))
        if existing and existing["file_hash"] == file_hash:
            logger.debug("File unchanged, skipping: %s", path)
            return existing.get("item_id")

        # 解析文件
        try:
            title, content = parse_file(path)
        except Exception:
            logger.warning("Failed to parse file: %s", path, exc_info=True)
            return None
        if not content.strip():
            logger.debug("Empty content after parsing, skipping: %s", path)
            return None

        # 如果已有 item，更新；否则新建
        item_id = existing["item_id"] if existing else None
        if item_id:
            self.db.update_item(item_id, title=title, content=content)
            # 清理旧 chunk 向量（文件更新后 chunk 数可能减少，残留旧向量会污染搜索结果）
            if self.vector_store and self.vector_store.available:
                try:
                    self.vector_store.delete_by_item_ids([item_id])
                except Exception:
                    logger.warning("Failed to delete old vectors for item %s", item_id)
        else:
            item_id = self.db.insert_item(
                title=title,
                content=content,
                source_type="file",
                metadata={"file_path": str(path), "file_type": path.suffix},
            )

        # 分块并投递 embedding 队列
        chunks = self.chunker.chunk(content)
        for i, chunk_text in enumerate(chunks):
            self.db.enqueue_embedding(
                item_id=item_id,
                chunk_index=i,
                chunk_text=chunk_text,
                priority=priority,
            )

        # 记录文件索引
        self.db.upsert_indexed_file(dir_id or "manual", str(path), file_hash, item_id)

        logger.info("Indexed file: %s (%d chunks)", path.name, len(chunks))
        return item_id

    def scan_directory(self, dir_info: dict) -> int:
        """全量扫描目录，返回处理的文件数"""
        dir_path = Path(dir_info["path"])
        if not dir_path.is_dir():
            logger.warning("Directory not found: %s", dir_path)
            return 0

        file_types = json.loads(dir_info.get("file_types", '["md","txt"]'))
        recursive = bool(dir_info.get("recursive", 1))
        dir_id = dir_info["id"]

        count = 0
        # 规范化为小写，兼容用户配置大写扩展名（如 ".MD"）
        suffixes = {f".{ft.lower().lstrip('.')}" for ft in file_types}

        if recursive:
            files = (f for f in dir_path.rglob("*") if f.is_file() and f.suffix.lower() in suffixes)
        else:
            files = (f for f in dir_path.iterdir() if f.is_file() and f.suffix.lower() in suffixes)

        for file_path in files:
            if self.process_file(str(file_path), dir_id=dir_id, priority=2):
                count += 1

        self.db.update_directory_scan(dir_id)
        logger.info("Scanned directory: %s (%d files)", dir_path, count)
        return count

    @staticmethod
    def _file_hash(path: Path) -> str:
        """计算文件内容的 SHA-256 hash"""
        h = hashlib.sha256()
        with open(path, "rb") as f:
            for chunk in iter(lambda: f.read(8192), b""):
                h.update(chunk)
        return h.hexdigest()
