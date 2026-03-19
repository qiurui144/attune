"""CRUD /items - 知识条目管理"""

import json

from fastapi import APIRouter, HTTPException, Query

from npu_webhook.app_state import state
from npu_webhook.models.schemas import ItemListResponse, ItemUpdateRequest, KnowledgeItem

router = APIRouter(prefix="/api/v1", tags=["items"])


def _row_to_item(row: dict) -> KnowledgeItem:
    return KnowledgeItem(
        id=row["id"],
        title=row["title"],
        content=row["content"],
        url=row.get("url"),
        source_type=row["source_type"],
        domain=row.get("domain"),
        tags=json.loads(row.get("tags", "[]")),
        metadata=json.loads(row.get("metadata", "{}")),
        created_at=row["created_at"],
        updated_at=row["updated_at"],
    )


@router.get("/items", response_model=ItemListResponse)
async def list_items(
    offset: int = Query(0, ge=0),
    limit: int = Query(20, ge=1, le=100),
    source_type: str | None = Query(None),
) -> ItemListResponse:
    if not state.db:
        raise HTTPException(status_code=503, detail="Database not initialized")
    rows = state.db.list_items(offset=offset, limit=limit, source_type=source_type)
    total = state.db.count_items()
    return ItemListResponse(
        items=[_row_to_item(r) for r in rows],
        total=total,
        offset=offset,
        limit=limit,
    )


@router.get("/items/stale")
async def stale_items(
    days: int = Query(30, ge=1),
    limit: int = Query(50, ge=1, le=200),
) -> dict:
    """查找过期/低质量/冷知识条目"""
    if not state.db:
        raise HTTPException(status_code=503, detail="Database not initialized")
    items = state.db.list_stale_items(days=days, limit=limit)
    return {
        "items": items,
        "total": len(items),
        "criteria": f"未使用 >{days} 天 或 quality_score < 0.3",
    }


@router.get("/items/{item_id}", response_model=KnowledgeItem)
async def get_item(item_id: str) -> KnowledgeItem:
    if not state.db:
        raise HTTPException(status_code=503, detail="Database not initialized")
    row = state.db.get_item(item_id)
    if not row:
        raise HTTPException(status_code=404, detail="Item not found")
    return _row_to_item(row)


@router.patch("/items/{item_id}")
async def update_item(item_id: str, req: ItemUpdateRequest) -> dict:
    if not state.db:
        raise HTTPException(status_code=503, detail="Database not initialized")
    if not state.db.get_item(item_id):
        raise HTTPException(status_code=404, detail="Item not found")

    kwargs = {}
    if req.title is not None:
        kwargs["title"] = req.title
    if req.tags is not None:
        kwargs["tags"] = req.tags
    if req.metadata is not None:
        kwargs["metadata"] = req.metadata

    if kwargs:
        state.db.update_item(item_id, **kwargs)
    return {"status": "ok"}


@router.delete("/items/{item_id}")
async def delete_item(item_id: str) -> dict:
    if not state.db:
        raise HTTPException(status_code=503, detail="Database not initialized")
    if not state.db.get_item(item_id):
        raise HTTPException(status_code=404, detail="Item not found")
    state.db.delete_item(item_id)
    # 删除所有关联 chunk（item 可能有多个分块向量）
    if state.vector_store:
        state.vector_store.delete_by_item_ids([item_id])
    return {"status": "ok"}


@router.get("/items/{item_id}/stats")
async def item_stats(item_id: str) -> dict:
    """获取条目的使用统计和质量分数"""
    if not state.db:
        raise HTTPException(status_code=503, detail="Database not initialized")
    stats = state.db.get_item_stats(item_id)
    if not stats:
        raise HTTPException(status_code=404, detail="Item not found")
    return stats
