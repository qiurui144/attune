"""/index - 本地目录绑定"""

from fastapi import APIRouter

router = APIRouter(prefix="/api/v1", tags=["index"])


@router.post("/index/bind")
async def bind_directory() -> dict[str, str]:
    # TODO Phase 1
    return {"status": "not_implemented"}


@router.delete("/index/unbind")
async def unbind_directory() -> dict[str, str]:
    # TODO Phase 1
    return {"status": "not_implemented"}


@router.get("/index/status")
async def index_status() -> dict[str, str]:
    # TODO Phase 1
    return {"status": "not_implemented"}


@router.post("/index/reindex")
async def reindex() -> dict[str, str]:
    # TODO Phase 1
    return {"status": "not_implemented"}
