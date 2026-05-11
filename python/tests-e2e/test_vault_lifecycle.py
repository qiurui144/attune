"""
E2E vault lifecycle — 验证 vault 解锁前/后的 API 行为。

测试 server 在 --no-auth 模式启动时:
- /api/v1/status 应该可访问（无需 unlock）
- /api/v1/items 在 vault 未初始化时应返回明确错误而非 500

注: --no-auth 跳过 auth middleware, 不等于 vault 已 unlock。
真正的 vault unlock 需要 --no-auth + 数据库已初始化 + master key 已派生。
本测试只验证"未初始化状态下不崩溃"。
"""
from __future__ import annotations

import httpx


class TestStatusEndpoint:
    def test_status_endpoint_reachable(self, client: httpx.Client) -> None:
        """/api/v1/status 应该返回响应（200 或 401 取决于实现）。"""
        r = client.get("/api/v1/status")
        # 在 --no-auth 模式下 401 仍合理（vault locked 而非 auth missing）
        assert r.status_code in (200, 401), f"unexpected {r.status_code}: {r.text[:200]}"

    def test_status_response_is_json(self, client: httpx.Client) -> None:
        r = client.get("/api/v1/status")
        # 如果返回 JSON 应解析成功
        if r.status_code == 200:
            data = r.json()
            assert isinstance(data, dict)


class TestVaultErrorHandling:
    def test_items_when_vault_uninitialized(self, client: httpx.Client) -> None:
        """vault 未初始化时 GET /api/v1/items 应明确报错而非 500。"""
        r = client.get("/api/v1/items")
        # 期望: 401 (vault locked) / 503 (not ready) / 200 (空列表)
        assert r.status_code != 500, f"server crashed: {r.text[:200]}"

    def test_settings_endpoint_safe(self, client: httpx.Client) -> None:
        """settings endpoint 即使 vault locked 也应优雅响应。"""
        r = client.get("/api/v1/settings")
        assert r.status_code != 500
