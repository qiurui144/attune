#!/usr/bin/env python3
"""
attune_mcp_shim.py — minimal stdio MCP shim wrapping Attune's REST API.

per W4 C2 decision (2026-04-27): we do not bundle a Rust MCP server crate.
This Python shim translates MCP JSON-RPC 2.0 (stdio) calls into
requests.get/post against http://localhost:18900/api/v1/*.

Tools exposed:
- attune_search(query: str, top_k: int = 10)
- attune_get_item(item_id: str)
- attune_chat(prompt: str)

Configure your MCP client (Claude Desktop / Cursor / Continue / Cherry / Lobe / open-webui)
to launch this script via stdio. See docs/mcp-integration.md for examples.

Privacy: vault stays on your machine. The shim only relays; Attune does the work.
"""
from __future__ import annotations

import json
import os
import re
import sys
from typing import Any

import urllib.error
import urllib.parse
import urllib.request

BASE_URL = os.environ.get("ATTUNE_BASE_URL", "http://localhost:18900").rstrip("/")
API_TOKEN = os.environ.get("ATTUNE_API_TOKEN", "")
TIMEOUT = float(os.environ.get("ATTUNE_TIMEOUT_SEC", "30"))
DEBUG = bool(os.environ.get("ATTUNE_DEBUG"))

PROTOCOL_VERSION = "2024-11-05"  # MCP spec version this shim was built against

TOOLS = [
    {
        "name": "attune_search",
        "description": "Hybrid (BM25 + vector) search across the user's local Attune vault. "
                       "Returns ranked items with title, snippet, and citation hints.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Natural language query"},
                "top_k": {"type": "integer", "default": 10, "minimum": 1, "maximum": 50},
            },
            "required": ["query"],
        },
    },
    {
        "name": "attune_get_item",
        "description": "Fetch the full content of one item from the user's Attune vault by id.",
        "inputSchema": {
            "type": "object",
            "properties": {"item_id": {"type": "string"}},
            "required": ["item_id"],
        },
    },
    {
        "name": "attune_chat",
        "description": "RAG-grounded chat: ask a question, get an answer citing items from "
                       "the user's local Attune vault. Use this when the user asks about their "
                       "own notes / documents / past conversations.",
        "inputSchema": {
            "type": "object",
            "properties": {"prompt": {"type": "string"}},
            "required": ["prompt"],
        },
    },
]


def _log(msg: str) -> None:
    if DEBUG:
        print(f"[attune_mcp_shim] {msg}", file=sys.stderr, flush=True)


# Attune item ids are UUIDs / content-hash slugs: alphanumeric plus `-` and `_`.
# A strict allowlist is the only safe way to interpolate a value into a URL path:
# it blocks path traversal (`../`, encoded `%2e%2e`), absolute-URL / scheme
# injection (`http://evil`, `//evil`, `file:`), query/fragment smuggling
# (`?`, `#`), and embedded slashes that would re-route the request (SSRF).
_ITEM_ID_RE = re.compile(r"\A[A-Za-z0-9_-]{1,128}\Z")


def sanitize_item_id(item_id: Any) -> str:
    """Validate an item_id for safe use in a URL path segment.

    Returns the id unchanged if it matches the strict allowlist; raises
    ValueError otherwise. Callers MUST NOT interpolate an unvalidated id into a
    URL — that is a path-traversal / SSRF vector (an attacker-supplied id like
    `../../admin` or `http://169.254.169.254/` would otherwise re-target the
    request).
    """
    if not isinstance(item_id, str):
        raise ValueError("item_id must be a string")
    if not _ITEM_ID_RE.match(item_id):
        raise ValueError(
            "item_id must be 1-128 chars of [A-Za-z0-9_-] (got an unsafe value)"
        )
    return item_id


def _http_call(method: str, path: str, payload: dict[str, Any] | None = None) -> dict[str, Any]:
    url = f"{BASE_URL}{path}"
    data = json.dumps(payload).encode("utf-8") if payload is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if API_TOKEN:
        req.add_header("Authorization", f"Bearer {API_TOKEN}")
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
            body = resp.read().decode("utf-8")
            return json.loads(body) if body else {}
    except urllib.error.HTTPError as e:
        msg = e.read().decode("utf-8", errors="replace") if e.fp else str(e)
        return {"_error": f"HTTP {e.code}: {msg[:200]}"}
    except urllib.error.URLError as e:
        return {"_error": f"connection failed: {e.reason}"}
    except Exception as e:  # noqa: BLE001
        return {"_error": f"unexpected: {type(e).__name__}: {e}"}


def call_attune_search(args: dict[str, Any]) -> dict[str, Any]:
    query = args.get("query", "")
    top_k = int(args.get("top_k", 10))
    return _http_call("GET", f"/api/v1/search?q={urllib.parse.quote(query)}&top_k={top_k}")


def call_attune_get_item(args: dict[str, Any]) -> dict[str, Any]:
    item_id = args.get("item_id", "")
    if not item_id:
        return {"_error": "item_id is required"}
    try:
        item_id = sanitize_item_id(item_id)
    except ValueError as e:
        return {"_error": f"invalid item_id: {e}"}
    # Allowlist already guarantees safety; quote() with empty safe set is
    # defence-in-depth so even a future allowlist relaxation can't break out
    # of the path segment.
    safe_id = urllib.parse.quote(item_id, safe="")
    return _http_call("GET", f"/api/v1/items/{safe_id}")


def call_attune_chat(args: dict[str, Any]) -> dict[str, Any]:
    prompt = args.get("prompt", "")
    if not prompt:
        return {"_error": "prompt is required"}
    return _http_call("POST", "/api/v1/chat", {"prompt": prompt})


HANDLERS = {
    "attune_search": call_attune_search,
    "attune_get_item": call_attune_get_item,
    "attune_chat": call_attune_chat,
}


def make_response(result: Any, request_id: Any) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": request_id, "result": result}


def make_error(code: int, message: str, request_id: Any) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message}}


def handle_request(req: dict[str, Any]) -> dict[str, Any] | None:
    method = req.get("method", "")
    request_id = req.get("id")
    params = req.get("params", {}) or {}

    _log(f"<< {method}")

    if method == "initialize":
        return make_response(
            {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "attune-mcp-shim", "version": "0.6.0"},
            },
            request_id,
        )

    if method == "notifications/initialized":
        return None  # notification, no response

    if method == "tools/list":
        return make_response({"tools": TOOLS}, request_id)

    if method == "tools/call":
        name = params.get("name", "")
        arguments = params.get("arguments", {}) or {}
        handler = HANDLERS.get(name)
        if handler is None:
            return make_error(-32601, f"unknown tool: {name}", request_id)
        try:
            payload = handler(arguments)
        except Exception as e:  # noqa: BLE001
            return make_error(-32603, f"tool execution failed: {e}", request_id)
        text = json.dumps(payload, ensure_ascii=False, indent=2)
        return make_response(
            {"content": [{"type": "text", "text": text}], "isError": "_error" in payload},
            request_id,
        )

    return make_error(-32601, f"unknown method: {method}", request_id)


def main() -> int:
    _log(f"starting; base_url={BASE_URL}, timeout={TIMEOUT}")
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError as e:
            sys.stderr.write(f"[attune_mcp_shim] bad JSON: {e}\n")
            continue
        resp = handle_request(req)
        if resp is None:
            continue
        sys.stdout.write(json.dumps(resp, ensure_ascii=False) + "\n")
        sys.stdout.flush()
        _log(f">> {resp.get('result', resp.get('error', {})).keys() if isinstance(resp.get('result', resp.get('error', {})), dict) else 'ok'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
