"""/settings - 配置管理"""

from fastapi import APIRouter

from npu_webhook.config import save_settings, settings
from npu_webhook.models.schemas import SettingsResponse, SettingsUpdateRequest

router = APIRouter(prefix="/api/v1", tags=["settings"])


@router.get("/settings", response_model=SettingsResponse)
async def get_settings() -> SettingsResponse:
    return SettingsResponse(
        server_host=settings.server.host,
        server_port=settings.server.port,
        embedding_model=settings.embedding.model,
        embedding_device=settings.embedding.device,
        embedding_batch_size=settings.embedding.batch_size,
        ingest_min_length=settings.ingest.min_content_length,
        excluded_domains=settings.ingest.excluded_domains,
    )


@router.patch("/settings")
async def update_settings(req: SettingsUpdateRequest) -> dict:
    if req.embedding_model is not None:
        settings.embedding.model = req.embedding_model
    if req.embedding_device is not None:
        settings.embedding.device = req.embedding_device
    if req.embedding_batch_size is not None:
        settings.embedding.batch_size = req.embedding_batch_size
    if req.ingest_min_length is not None:
        settings.ingest.min_content_length = req.ingest_min_length
    if req.excluded_domains is not None:
        settings.ingest.excluded_domains = req.excluded_domains
    save_settings(settings)
    return {"status": "ok"}
