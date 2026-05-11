"""CRUD /skills - 技能管理"""

from fastapi import APIRouter

router = APIRouter(prefix="/api/v1", tags=["skills"])


@router.get("/skills")
async def list_skills() -> dict[str, str]:
    # TODO Phase 3
    return {"status": "not_implemented"}


@router.post("/skills")
async def create_skill() -> dict[str, str]:
    # TODO Phase 3
    return {"status": "not_implemented"}


@router.post("/skills/{skill_id}/execute")
async def execute_skill(skill_id: str) -> dict[str, str]:
    # TODO Phase 3
    return {"status": "not_implemented"}
