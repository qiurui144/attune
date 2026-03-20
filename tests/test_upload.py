"""POST /upload 端点测试"""
import io
import pytest
from httpx import ASGITransport, AsyncClient
from npu_webhook.main import app


@pytest.mark.asyncio
async def test_upload_markdown():
    """上传 Markdown 文件返回 item_id 和 chunks_queued"""
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        content = ("# 测试文档\n\n## 章节一\n" + "内容A " * 100 + "\n\n## 章节二\n" + "内容B " * 100).encode()
        resp = await client.post(
            "/api/v1/upload",
            files={"file": ("test.md", io.BytesIO(content), "text/markdown")},
        )
        assert resp.status_code == 200
        data = resp.json()
        assert "id" in data
        assert data["chunks_queued"] > 0
        assert data["status"] == "processing"


@pytest.mark.asyncio
async def test_upload_too_large():
    """文件超过 20MB 返回 413"""
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        large_content = b"x" * (21 * 1024 * 1024)  # 21MB
        resp = await client.post(
            "/api/v1/upload",
            files={"file": ("big.txt", io.BytesIO(large_content), "text/plain")},
        )
        assert resp.status_code == 413


@pytest.mark.asyncio
async def test_upload_unsupported_format():
    """不支持的格式返回 415"""
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        resp = await client.post(
            "/api/v1/upload",
            files={"file": ("image.png", io.BytesIO(b"\x89PNG"), "image/png")},
        )
        assert resp.status_code == 415


@pytest.mark.asyncio
async def test_upload_with_session_id():
    """带 session_id 的上传，item_id 应记录到 session_upload_ids"""
    from npu_webhook.app_state import state
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        content = ("# 会话测试\n\n" + "内容 " * 200).encode()
        resp = await client.post(
            "/api/v1/upload",
            files={"file": ("session_test.md", io.BytesIO(content), "text/markdown")},
            data={"session_id": "test-session-001"},
        )
        assert resp.status_code == 200
        item_id = resp.json()["id"]
        assert item_id in state.session_upload_ids
