"""GET /search - 混合搜索"""

from fastapi import APIRouter

router = APIRouter(prefix="/api/v1", tags=["search"])


@router.get("/search")
async def search() -> dict[str, str]:
    """混合搜索（向量+全文, RRF融合）"""
    # TODO Phase 1
    return {"status": "not_implemented"}


@router.post("/search/relevant")
async def search_relevant() -> dict[str, str]:
    """获取注入用相关知识（Content Script 调用）"""
    # TODO Phase 1
    return {"status": "not_implemented"}
