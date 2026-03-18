"""WebSocket 实时通道"""

from fastapi import APIRouter, WebSocket

router = APIRouter(tags=["ws"])


@router.websocket("/api/v1/ws")
async def websocket_endpoint(websocket: WebSocket) -> None:
    """WebSocket 实时通道（下载进度/通知）"""
    await websocket.accept()
    try:
        while True:
            data = await websocket.receive_text()
            await websocket.send_json({"type": "pong", "data": data})
    except Exception:
        pass
