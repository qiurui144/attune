"""/setup - 首次安装引导页"""

from fastapi import APIRouter
from fastapi.responses import HTMLResponse

router = APIRouter(tags=["setup"])


@router.get("/setup", response_class=HTMLResponse)
async def setup_page() -> str:
    """首次安装引导页面"""
    # TODO Phase 5: 完整引导页
    return "<html><body><h1>npu-webhook 安装引导</h1><p>TODO</p></body></html>"
