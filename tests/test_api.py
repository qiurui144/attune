"""API 测试"""

import pytest
from httpx import ASGITransport, AsyncClient

from npu_webhook.main import app


@pytest.mark.asyncio
async def test_health_check():
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        resp = await client.get("/api/v1/status/health")
        assert resp.status_code == 200
        assert resp.json() == {"status": "ok"}
