"""API 端点测试"""

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


@pytest.mark.asyncio
async def test_system_status():
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        resp = await client.get("/api/v1/status")
        assert resp.status_code == 200
        data = resp.json()
        assert "version" in data
        assert "total_items" in data


@pytest.mark.asyncio
async def test_get_settings():
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        resp = await client.get("/api/v1/settings")
        assert resp.status_code == 200
        data = resp.json()
        assert data["server_port"] == 18900


@pytest.mark.asyncio
async def test_ingest_too_short():
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        resp = await client.post("/api/v1/ingest", json={
            "title": "test",
            "content": "short",
        })
        assert resp.status_code == 400


@pytest.mark.asyncio
async def test_ingest_and_list():
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        # 注入
        content = "这是一段足够长的测试内容，用于验证知识注入功能是否正常工作。" * 5
        resp = await client.post("/api/v1/ingest", json={
            "title": "测试知识",
            "content": content,
            "source_type": "note",
        })
        assert resp.status_code == 200
        item_id = resp.json()["id"]

        # 获取
        resp = await client.get(f"/api/v1/items/{item_id}")
        assert resp.status_code == 200
        assert resp.json()["title"] == "测试知识"

        # 列表
        resp = await client.get("/api/v1/items")
        assert resp.status_code == 200
        assert resp.json()["total"] >= 1

        # 更新
        resp = await client.patch(f"/api/v1/items/{item_id}", json={"tags": ["test"]})
        assert resp.status_code == 200

        # 删除
        resp = await client.delete(f"/api/v1/items/{item_id}")
        assert resp.status_code == 200


@pytest.mark.asyncio
async def test_search():
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        resp = await client.get("/api/v1/search", params={"q": "测试"})
        assert resp.status_code == 200
        assert "results" in resp.json()


@pytest.mark.asyncio
async def test_search_relevant():
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        resp = await client.post("/api/v1/search/relevant", json={
            "query": "测试知识",
            "top_k": 3,
        })
        assert resp.status_code == 200


@pytest.mark.asyncio
async def test_index_status():
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        resp = await client.get("/api/v1/index/status")
        assert resp.status_code == 200
        assert "directories" in resp.json()


@pytest.mark.asyncio
async def test_stale_items_route_not_shadowed():
    """回归测试：/items/stale 不应被 /items/{item_id} 路由拦截（路由顺序修复）"""
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        resp = await client.get("/api/v1/items/stale")
        # 应返回 200（stale 列表），而非 422/404（item_id 路由）
        assert resp.status_code == 200
        data = resp.json()
        assert "items" in data
        assert "total" in data


@pytest.mark.asyncio
async def test_items_pagination_total_matches_source_type():
    """回归测试：分页 total 需与 source_type 过滤一致（count_items 修复）"""
    content = "用于分页测试的足够长内容，重复多次确保超过最短长度限制。" * 5
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        # 注入 2 条 note 类型
        ids = []
        for i in range(2):
            resp = await client.post("/api/v1/ingest", json={
                "title": f"分页测试 {i}",
                "content": content,
                "source_type": "note",
            })
            assert resp.status_code == 200
            ids.append(resp.json()["id"])

        # 列表：total 应等于 note 类型的实际条目数
        resp = await client.get("/api/v1/items", params={"source_type": "note"})
        assert resp.status_code == 200
        data = resp.json()
        listed = len(data["items"])
        total = data["total"]
        assert total == listed, f"total({total}) 与实际条目数({listed})不一致"

        # 清理
        for item_id in ids:
            await client.delete(f"/api/v1/items/{item_id}")


@pytest.mark.asyncio
async def test_patch_settings_fields():
    """PATCH /settings 修改字段后 GET 应返回新值"""
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        # 获取原始值
        orig = (await client.get("/api/v1/settings")).json()
        orig_batch = orig["embedding_batch_size"]

        new_batch = orig_batch + 1
        resp = await client.patch("/api/v1/settings", json={"embedding_batch_size": new_batch})
        assert resp.status_code == 200
        assert resp.json()["status"] == "ok"

        # 修改应即时可见
        updated = (await client.get("/api/v1/settings")).json()
        assert updated["embedding_batch_size"] == new_batch

        # 还原
        await client.patch("/api/v1/settings", json={"embedding_batch_size": orig_batch})
