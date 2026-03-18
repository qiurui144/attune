"""/status - 系统状态 + 健康检查"""

from fastapi import APIRouter

router = APIRouter(prefix="/api/v1", tags=["status"])


@router.get("/status")
async def system_status() -> dict[str, str]:
    """系统状态（NPU/模型/统计）"""
    # TODO Phase 1
    return {"status": "not_implemented"}
