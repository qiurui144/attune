"""POST /ingest - 知识注入"""

from fastapi import APIRouter

router = APIRouter(prefix="/api/v1", tags=["ingest"])


@router.post("/ingest")
async def ingest() -> dict[str, str]:
    """接收浏览器推送的内容（对话/网页/选中文本）"""
    # TODO Phase 1
    return {"status": "not_implemented"}
