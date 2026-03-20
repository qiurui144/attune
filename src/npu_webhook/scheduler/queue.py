"""Embedding 队列 Worker：后台消费 embedding 任务"""

import logging
import threading
import time

from npu_webhook.core.vectorstore import VectorStore
from npu_webhook.db.sqlite_db import SQLiteDB

logger = logging.getLogger(__name__)


class EmbeddingQueueWorker:
    """后台 Embedding 队列消费者

    从 SQLite 队列取出任务，批量生成 embedding，写入 ChromaDB。
    """

    def __init__(
        self,
        db: SQLiteDB,
        vector_store: VectorStore,
        batch_size: int = 16,
        poll_interval: float = 2.0,
    ) -> None:
        self.db = db
        self.vector_store = vector_store
        self.batch_size = batch_size
        self.poll_interval = poll_interval
        self._stop_event = threading.Event()
        self._thread: threading.Thread | None = None

    def start(self) -> None:
        """启动后台 worker 线程"""
        if not self.vector_store.available:
            logger.warning("Embedding engine not available, queue worker disabled")
            return
        self._stop_event.clear()
        self._thread = threading.Thread(target=self._run, daemon=True, name="embedding-worker")
        self._thread.start()
        logger.info("Embedding queue worker started (batch_size=%d)", self.batch_size)

    def stop(self) -> None:
        """停止 worker"""
        self._stop_event.set()
        if self._thread:
            self._thread.join(timeout=10)
            if self._thread.is_alive():
                logger.warning("Embedding queue worker did not stop within 10s")
            else:
                logger.info("Embedding queue worker stopped")

    def _run(self) -> None:
        while not self._stop_event.is_set():
            try:
                processed = self._process_batch()
                if processed == 0:
                    self._stop_event.wait(self.poll_interval)
            except Exception:
                logger.exception("Error processing embedding batch")
                self._stop_event.wait(5.0)

    def _process_batch(self) -> int:
        """处理一批 embedding 任务，返回处理数量"""
        tasks = self.db.dequeue_embeddings(self.batch_size)
        if not tasks:
            return 0

        doc_ids = []
        texts = []
        metadatas = []
        task_ids = []

        # 批量预加载 item 元数据，避免循环中 N+1 查询
        unique_ids = list({t["item_id"] for t in tasks})
        items_map = {row["id"]: row for row in self.db.get_items_batch(unique_ids)}

        for task in tasks:
            item_id = task["item_id"]
            chunk_index = task["chunk_index"]
            chunk_text = task["chunk_text"]
            item = items_map.get(item_id)

            if not chunk_text:
                # chunk_text 为空时从 item 获取完整内容
                if item:
                    chunk_text = item["content"]
                else:
                    logger.warning("Item %s not found for embedding task %s", item_id, task["id"])
                    self.db.fail_embedding(task["id"])
                    continue

            doc_id = f"{item_id}:{chunk_index}"
            doc_ids.append(doc_id)
            texts.append(chunk_text)
            metadatas.append({
                "item_id": item_id,
                "chunk_index": chunk_index,
                "source_type": item["source_type"] if item else "",
                "created_at": item["created_at"] if item else "",
                "level": task.get("level", 2),             # 两层索引：摘要层(1) / 细节层(2)
                "section_idx": task.get("section_idx", 0), # 所属章节编号
            })
            task_ids.append(task["id"])

        if doc_ids:
            try:
                self.vector_store.add_batch(doc_ids, texts, metadatas=metadatas)
                for tid in task_ids:
                    self.db.complete_embedding(tid)
                logger.debug("Processed %d embeddings", len(doc_ids))
            except Exception:
                logger.exception("Failed to process embedding batch")
                for tid in task_ids:
                    self.db.fail_embedding(tid)

        return len(task_ids)

    def process_immediate(self, item_id: str, text: str) -> bool:
        """P0 即时处理：直接 embed 并存储，不经过队列"""
        if not self.vector_store.available:
            return False
        doc_id = f"{item_id}:0"
        item = self.db.get_item(item_id)
        metadata = {
            "item_id": item_id,
            "chunk_index": 0,
            "source_type": item["source_type"] if item else "",
            "created_at": item["created_at"] if item else "",
        }
        return self.vector_store.add(doc_id, text, metadata=metadata)
