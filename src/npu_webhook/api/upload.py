"""POST /upload - 原始文件上传端点"""
import logging
import time
from pathlib import Path

from fastapi import APIRouter, Form, HTTPException, UploadFile

from npu_webhook.app_state import state
from npu_webhook.config import settings
from npu_webhook.core.parser import parse_bytes

logger = logging.getLogger(__name__)

router = APIRouter(prefix="/api/v1", tags=["upload"])

# 支持的格式白名单
ALLOWED_EXTENSIONS = {".pdf", ".docx", ".md", ".txt", ".py", ".js", ".ts", ".jsx", ".tsx"}

SESSION_TTL = 86400  # 24h


@router.post("/upload")
async def upload_file(
    file: UploadFile,
    session_id: str | None = Form(None),
) -> dict:
    """接收原始文件，自动解析并两层入库（FTS5 立即可搜，向量搜索异步就绪）"""
    if not state.db:
        raise HTTPException(status_code=503, detail="Database not initialized")

    # 格式检查
    suffix = Path(file.filename or "").suffix.lower()
    if suffix not in ALLOWED_EXTENSIONS:
        raise HTTPException(status_code=415, detail=f"Unsupported file type: {suffix}")

    # 读取文件内容
    data = await file.read()

    # 大小检查
    max_bytes = settings.ingest.max_upload_mb * 1024 * 1024
    if len(data) > max_bytes:
        raise HTTPException(
            status_code=413,
            detail=f"File too large (max {settings.ingest.max_upload_mb}MB)",
        )

    # 解析
    try:
        title, content = parse_bytes(data, file.filename or "upload")
    except Exception as e:
        logger.exception("Failed to parse uploaded file: %s", file.filename)
        raise HTTPException(status_code=422, detail=f"Failed to parse file: {e}") from e

    if not content.strip():
        raise HTTPException(status_code=422, detail="File content is empty after parsing")

    # 存入 SQLite（FTS5 立即可搜）
    item_id = state.db.insert_item(
        title=title,
        content=content,
        source_type="file",
        metadata={"filename": file.filename, "upload_source": "browser"},
    )

    # 两层 embedding 入队
    chunks_queued = 0
    if state.chunker:
        sections = state.chunker.extract_sections(content, source_type="file")

        # Level 1: 章节
        for section_idx, section_text in sections:
            if section_text.strip():
                state.db.enqueue_embedding(
                    item_id=item_id,
                    chunk_index=section_idx,
                    chunk_text=section_text,
                    priority=1,
                    level=1,
                    section_idx=section_idx,
                )
                chunks_queued += 1

        # Level 2: 段落块
        chunk_counter = 0
        for section_idx, section_text in sections:
            chunks = state.chunker.chunk(section_text)
            for chunk_text in chunks:
                state.db.enqueue_embedding(
                    item_id=item_id,
                    chunk_index=chunk_counter,
                    chunk_text=chunk_text,
                    priority=1,
                    level=2,
                    section_idx=section_idx,
                )
                chunk_counter += 1
                chunks_queued += 1

    # 记录 session_upload_ids（用于注入加权）
    if session_id and state.session_upload_ids is not None:
        now = time.time()
        expired = [k for k, ts in state.session_upload_ids.items() if now - ts > SESSION_TTL]
        for k in expired:
            del state.session_upload_ids[k]
        state.session_upload_ids[item_id] = now

    logger.info("Uploaded and indexed: %s (%d queue tasks)", title, chunks_queued)
    return {
        "id": item_id,
        "title": title,
        "chunks_queued": chunks_queued,
        "status": "processing",
    }
