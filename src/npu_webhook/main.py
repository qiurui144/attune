"""FastAPI 入口 + lifespan 管理"""

from contextlib import asynccontextmanager
from typing import AsyncGenerator

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware

from npu_webhook.config import settings


@asynccontextmanager
async def lifespan(app: FastAPI) -> AsyncGenerator[None, None]:
    """应用生命周期管理：启动时初始化，关闭时清理"""
    # TODO Phase 1: 初始化 SQLite + ChromaDB + Embedding 引擎 + 后台队列
    yield
    # TODO Phase 1: 关闭后台任务 + 释放资源


app = FastAPI(
    title="npu-webhook",
    description="个人知识库 + 记忆增强系统",
    version="0.1.0",
    lifespan=lifespan,
)

# CORS: 允许 Chrome 扩展 + localhost
app.add_middleware(
    CORSMiddleware,
    allow_origins=[
        "chrome-extension://*",
        "http://localhost:*",
        "http://127.0.0.1:*",
    ],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)


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
