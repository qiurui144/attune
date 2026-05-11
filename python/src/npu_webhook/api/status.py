"""/status - 系统状态 + 健康检查"""

from fastapi import APIRouter

from npu_webhook.app_state import state
from npu_webhook.config import settings
from npu_webhook.models.schemas import SystemStatus

router = APIRouter(prefix="/api/v1", tags=["status"])


@router.get("/status", response_model=SystemStatus)
async def system_status() -> SystemStatus:
    """系统状态（NPU/模型/统计）"""
    return SystemStatus(
        version="0.1.0",
        device=settings.embedding.device,
        model_name=settings.embedding.model,
        embedding_available=state.vector_store.available if state.vector_store else False,
        total_items=state.db.count_items() if state.db else 0,
        total_vectors=state.chroma.count() if state.chroma else 0,
        pending_embeddings=state.db.pending_embedding_count() if state.db else 0,
        bound_directories=len(state.db.list_directories()) if state.db else 0,
    )
