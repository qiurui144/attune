"""/models - 模型下载管理"""

from fastapi import APIRouter

router = APIRouter(prefix="/api/v1", tags=["models"])


@router.get("/models")
async def list_models() -> dict[str, str]:
    # TODO Phase 4
    return {"status": "not_implemented"}


@router.post("/models/download")
async def download_model() -> dict[str, str]:
    # TODO Phase 4
    return {"status": "not_implemented"}
