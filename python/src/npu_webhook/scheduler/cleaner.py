"""知识库自动清理 Worker：定期淘汰低质量/冷数据，保持知识库新鲜"""

import logging
import threading

from npu_webhook.core.vectorstore import VectorStore
from npu_webhook.db.sqlite_db import SQLiteDB

logger = logging.getLogger(__name__)

_CLEAN_INTERVAL = 24 * 60 * 60  # 24h


class KnowledgeCleaner:
    """定期清理低质量、长期未使用的知识条目

    策略（均为软删除，不可恢复时由用户在 Side Panel 确认）：
    - quality_score < 0.2 AND 60+ 天未使用 → 归档
    - ai_chat 类型 AND use_count == 0 AND 30+ 天未创建 → 归档

    同时从向量库中删除对应向量，避免污染搜索结果。
    """

    def __init__(self, db: SQLiteDB, vector_store: VectorStore) -> None:
        self.db = db
        self.vector_store = vector_store
        self._stop_event = threading.Event()
        self._thread: threading.Thread | None = None

    def start(self) -> None:
        self._stop_event.clear()
        self._thread = threading.Thread(target=self._run, daemon=True, name="knowledge-cleaner")
        self._thread.start()
        logger.info("Knowledge cleaner started (interval=24h)")

    def stop(self) -> None:
        self._stop_event.set()
        if self._thread:
            self._thread.join(timeout=5)

    def _run(self) -> None:
        # 启动后等待 1 小时再首次清理，避免启动时占用资源
        if self._stop_event.wait(3600):
            return
        while not self._stop_event.is_set():
            try:
                self.run_once()
            except Exception:
                logger.exception("Knowledge cleaner error")
            self._stop_event.wait(_CLEAN_INTERVAL)

    def run_once(self) -> dict:
        """执行一次清理，返回统计信息"""
        archived_ids = self.db.bulk_archive_stale(
            quality_threshold=0.2,
            unused_days=60,
            chat_unused_days=30,
        )

        # 删除所有关联向量（每个 item 可能有多个分块向量，用元数据过滤确保全部清理）
        if archived_ids and self.vector_store.available:
            try:
                self.vector_store.delete_by_item_ids(archived_ids)
                logger.debug("Deleted vectors for %d archived items", len(archived_ids))
            except Exception:
                logger.warning("Failed to delete vectors for archived items", exc_info=True)

        return {"archived": len(archived_ids)}
