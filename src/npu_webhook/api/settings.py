"""/settings - 配置管理"""

from fastapi import APIRouter

router = APIRouter(prefix="/api/v1", tags=["settings"])


@router.get("/settings")
async def get_settings() -> dict[str, str]:
    # TODO Phase 1
    return {"status": "not_implemented"}


@router.patch("/settings")
async def update_settings() -> dict[str, str]:
    # TODO Phase 1
    return {"status": "not_implemented"}
