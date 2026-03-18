"""CRUD /items - 知识条目管理"""

from fastapi import APIRouter

router = APIRouter(prefix="/api/v1", tags=["items"])


@router.get("/items")
async def list_items() -> dict[str, str]:
    # TODO Phase 1
    return {"status": "not_implemented"}


@router.get("/items/{item_id}")
async def get_item(item_id: str) -> dict[str, str]:
    # TODO Phase 1
    return {"status": "not_implemented"}


@router.patch("/items/{item_id}")
async def update_item(item_id: str) -> dict[str, str]:
    # TODO Phase 1
    return {"status": "not_implemented"}


@router.delete("/items/{item_id}")
async def delete_item(item_id: str) -> dict[str, str]:
    # TODO Phase 1
    return {"status": "not_implemented"}
