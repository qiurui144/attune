"""FastAPI 入口 + lifespan 管理"""

import logging
import logging.handlers
from contextlib import asynccontextmanager
from typing import AsyncGenerator

from fastapi import FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse

from npu_webhook.app_state import state
from npu_webhook.config import settings


def _setup_logging() -> None:
    """配置日志：控制台 + 文件轮转"""
    log_dir = settings.data_dir / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)

    fmt = logging.Formatter("%(asctime)s [%(levelname)s] %(name)s: %(message)s")

    file_handler = logging.handlers.RotatingFileHandler(
        log_dir / "npu-webhook.log",
        maxBytes=settings.logging.max_size_mb * 1024 * 1024,
        backupCount=3,
        encoding="utf-8",
    )
    file_handler.setFormatter(fmt)

    console_handler = logging.StreamHandler()
    console_handler.setFormatter(fmt)

    root = logging.getLogger()
    root.setLevel(getattr(logging, settings.logging.level, logging.INFO))
    root.addHandler(file_handler)
    root.addHandler(console_handler)


logger = logging.getLogger(__name__)


@asynccontextmanager
async def lifespan(app: FastAPI) -> AsyncGenerator[None, None]:
    """应用生命周期管理"""
    _setup_logging()
    logger.info("Starting npu-webhook v0.1.0")

    # 1. 初始化 SQLite
    from npu_webhook.db.sqlite_db import SQLiteDB

    db_path = settings.data_dir / "knowledge.db"
    state.db = SQLiteDB(db_path)
    logger.info("SQLite initialized: %s", db_path)

    # 2. 初始化 ChromaDB
    from npu_webhook.db.chroma_db import ChromaDB

    chroma_dir = settings.data_dir / "chroma"
    chroma_dir.mkdir(parents=True, exist_ok=True)
    state.chroma = ChromaDB(chroma_dir)
    logger.info("ChromaDB initialized: %s", chroma_dir)

    # 3. 初始化 Embedding 引擎
    from npu_webhook.core.embedding import create_embedding_engine

    state.embedding_engine = create_embedding_engine(
        model_name=settings.embedding.model,
        device=settings.embedding.device,
        data_dir=settings.data_dir,
        max_length=settings.embedding.max_length,
    )
    if state.embedding_engine:
        logger.info("Embedding engine ready (dim=%d)", state.embedding_engine.get_dimension())
    else:
        logger.warning("Embedding engine not available - model not found. Search will use FTS5 only.")

    # 4. 初始化 VectorStore + SearchEngine
    from npu_webhook.core.vectorstore import VectorStore

    state.vector_store = VectorStore(state.chroma, state.embedding_engine)

    from npu_webhook.core.search import HybridSearchEngine

    state.search_engine = HybridSearchEngine(
        db=state.db,
        vector_store=state.vector_store,
        rrf_k=settings.search.rrf_k,
        vector_weight=settings.search.vector_weight,
        fulltext_weight=settings.search.fulltext_weight,
    )

    # 5. 初始化 Chunker + Pipeline
    from npu_webhook.core.chunker import Chunker

    state.chunker = Chunker(
        chunk_size=settings.chunk.chunk_size,
        overlap=settings.chunk.overlap,
    )

    from npu_webhook.indexer.pipeline import IndexPipeline

    state.pipeline = IndexPipeline(state.db, state.chunker, state.vector_store)

    # 6. 启动 Embedding Queue Worker
    from npu_webhook.scheduler.queue import EmbeddingQueueWorker

    state.queue_worker = EmbeddingQueueWorker(
        db=state.db,
        vector_store=state.vector_store,
        batch_size=settings.embedding.batch_size,
    )
    state.queue_worker.start()

    # 7. 启动知识库自动清理 Worker
    from npu_webhook.scheduler.cleaner import KnowledgeCleaner

    state.cleaner = KnowledgeCleaner(db=state.db, vector_store=state.vector_store)
    state.cleaner.start()

    # 8. 启动目录 Watcher
    from npu_webhook.indexer.watcher import DirectoryWatcher

    def _on_file_change(path: str, event: str) -> None:
        if state.pipeline:
            state.pipeline.process_file(path, priority=2)

    state.watcher = DirectoryWatcher(callback=_on_file_change)
    state.watcher.load_from_db(state.db.list_directories())
    state.watcher.start()

    logger.info(
        "npu-webhook ready: %d items, %d vectors, %d pending embeddings",
        state.db.count_items(),
        state.chroma.count(),
        state.db.pending_embedding_count(),
    )

    yield

    # 清理
    logger.info("Shutting down npu-webhook")
    if state.watcher:
        state.watcher.stop()
    if state.queue_worker:
        state.queue_worker.stop()
    if state.cleaner:
        state.cleaner.stop()
    if state.db:
        state.db.close()
    logger.info("Shutdown complete")


app = FastAPI(
    title="npu-webhook",
    description="个人知识库 + 记忆增强系统",
    version="0.1.0",
    lifespan=lifespan,
)

# CORS: 仅允许 Chrome 扩展 + localhost（移除 * 通配符，避免任意域名跨域凭据请求）
app.add_middleware(
    CORSMiddleware,
    allow_origins=[],
    allow_origin_regex=r"^(chrome-extension://.*|http://localhost(:\d+)?|http://127\.0\.0\.1(:\d+)?)$",
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)


# 认证中间件：localhost 免认证
@app.middleware("http")
async def auth_middleware(request: Request, call_next):  # type: ignore[no-untyped-def]
    client_host = request.client.host if request.client else "unknown"
    if client_host not in ("127.0.0.1", "::1", "localhost") and settings.auth.mode == "token":
        token = request.headers.get("X-API-Token", "")
        # 未配置 token 时 fail-closed（空 token == 误配置，拒绝所有非 localhost 请求）
        if not settings.auth.token or token != settings.auth.token:
            return JSONResponse(status_code=401, content={"detail": "Unauthorized"})
    return await call_next(request)


# 注册路由
from npu_webhook.api.ingest import router as ingest_router
from npu_webhook.api.search import router as search_router
from npu_webhook.api.items import router as items_router
from npu_webhook.api.index import router as index_router
from npu_webhook.api.status import router as status_router
from npu_webhook.api.settings import router as settings_router
from npu_webhook.api.model_routes import router as models_router
from npu_webhook.api.skills import router as skills_router
from npu_webhook.api.ws import router as ws_router
from npu_webhook.api.setup import router as setup_router
from npu_webhook.api.upload import router as upload_router

app.include_router(ingest_router)
app.include_router(search_router)
app.include_router(items_router)
app.include_router(index_router)
app.include_router(status_router)
app.include_router(settings_router)
app.include_router(models_router)
app.include_router(skills_router)
app.include_router(ws_router)
app.include_router(setup_router)
app.include_router(upload_router)


@app.get("/api/v1/status/health")
async def health_check() -> dict[str, str]:
    return {"status": "ok"}


def main() -> None:
    import uvicorn

    uvicorn.run(
        "npu_webhook.main:app",
        host=settings.server.host,
        port=settings.server.port,
        reload=False,
    )


if __name__ == "__main__":
    main()
