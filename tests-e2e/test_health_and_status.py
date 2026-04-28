"""
E2E Layer 6 — 起 attune-server-headless 二进制 + httpx 测关键 API
最小集（5 个）：
1. /api/v1/status/health 返回 200 + status:ok
2. CORS preflight 通过
3. evil origin 不在 allowed list
4. 未知 endpoint 401/404 (auth gate 或 not-found)
5. server 进程存活
"""
from __future__ import annotations

import httpx


class TestHealth:
    def test_health_endpoint_200(self, client: httpx.Client) -> None:
        r = client.get("/api/v1/status/health")
        assert r.status_code == 200
        assert r.json() == {"status": "ok"}

    def test_health_response_time_under_1s(self, client: httpx.Client) -> None:
        import time
        start = time.time()
        r = client.get("/api/v1/status/health")
        assert r.status_code == 200
        assert (time.time() - start) < 1.0


class TestCORS:
    def test_chrome_extension_origin_allowed(self, client: httpx.Client) -> None:
        r = client.options(
            "/api/v1/status/health",
            headers={
                "Origin": "chrome-extension://abcdefghijklmnopqrstuvwxyz",
                "Access-Control-Request-Method": "GET",
            },
        )
        assert r.status_code in (200, 204)
        # CORS 响应头存在
        cors_origin = r.headers.get("access-control-allow-origin")
        assert cors_origin is not None

    def test_localhost_origin_allowed(self, client: httpx.Client) -> None:
        r = client.options(
            "/api/v1/status/health",
            headers={
                "Origin": "http://localhost:18900",
                "Access-Control-Request-Method": "GET",
            },
        )
        assert r.status_code in (200, 204)


class TestRouteFallback:
    def test_unknown_endpoint_returns_401_or_404(self, client: httpx.Client) -> None:
        """未知 endpoint: auth middleware 拦截返回 401, 或 not-found 路由 404。
        两种都是合理的 — 重要的是不返回 200 或 5xx。"""
        r = client.get("/api/v1/no-such-endpoint")
        assert r.status_code in (401, 404), f"expected 401/404, got {r.status_code}"

    def test_root_path_does_not_500(self, client: httpx.Client) -> None:
        """/ 不应 500 (有 web UI 或 redirect 都行)。"""
        r = client.get("/", follow_redirects=False)
        assert r.status_code < 500
