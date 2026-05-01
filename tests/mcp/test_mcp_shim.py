"""
F-15-MCP automated tests — protocol-level (subprocess + JSON-RPC over stdio).

Per `docs/FEATURES.md` F-15-MCP: `tools/attune_mcp_shim.py` is a stdio MCP
server wrapping attune's REST API. It accepts JSON-RPC 2.0 line-delimited
on stdin, emits responses on stdout.

This test verifies **shim protocol correctness** without requiring a live
attune-server backend:
- `initialize` returns capabilities + serverInfo
- `notifications/initialized` is acknowledged silently (no response)
- `tools/list` returns the 3 declared tools (search / get_item / chat)
- `tools/call` with unknown tool returns -32601 method-not-found
- malformed JSON line is logged but doesn't crash the shim

What this does NOT test (manual checklist + future v0.7+):
- Actual backend HTTP calls (would need attune-server live)
- Real tool execution (attune_search / attune_chat etc.)
- Cross-client integration with Claude Desktop / Cursor / Cline

These higher-level scenarios are documented in `tests/MANUAL_TEST_CHECKLIST.md`
MCP section. Cross-language harness (Python + Rust + JS clients) is v0.7+ work.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

import pytest


SHIM_PATH = Path(__file__).resolve().parents[2] / "tools" / "attune_mcp_shim.py"
PROTOCOL_VERSION = "2024-11-05"


@pytest.fixture
def shim_proc():
    """Spawn the shim as a subprocess. yields (proc, send, recv) helpers.

    `send(req: dict)` writes a JSON line to stdin.
    `recv(timeout=2.0)` reads one JSON line from stdout (or raises TimeoutError).
    """
    if not SHIM_PATH.exists():
        pytest.skip(f"shim not found: {SHIM_PATH}")

    # Set base_url to a port that is *not* listening — for protocol-level tests
    # we never reach the HTTP backend. tools/call tests would need a live server.
    env = os.environ.copy()
    env["ATTUNE_BASE_URL"] = "http://127.0.0.1:1"  # unused for these tests
    env["ATTUNE_DEBUG"] = "1"

    proc = subprocess.Popen(
        [sys.executable, str(SHIM_PATH)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        text=True,
        bufsize=1,
    )

    def send(req: dict[str, Any]) -> None:
        line = json.dumps(req)
        assert proc.stdin is not None
        proc.stdin.write(line + "\n")
        proc.stdin.flush()

    def recv(timeout: float = 2.0) -> dict[str, Any]:
        deadline = time.monotonic() + timeout
        # readline() blocks until newline; we accept that as the protocol guarantees
        # one response per request. Use a separate thread to enforce timeout.
        import threading

        result: list[str | None] = [None]

        def reader():
            assert proc.stdout is not None
            result[0] = proc.stdout.readline()

        t = threading.Thread(target=reader, daemon=True)
        t.start()
        t.join(timeout)
        if t.is_alive():
            raise TimeoutError(f"no response within {timeout}s")
        line = result[0]
        if not line:
            raise EOFError("shim stdout closed unexpectedly")
        return json.loads(line)

    try:
        yield proc, send, recv
    finally:
        if proc.poll() is None:
            assert proc.stdin is not None
            proc.stdin.close()
            try:
                proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()


def test_initialize_returns_capabilities(shim_proc):
    """covers F-15-MCP MCP `initialize` handshake compliance."""
    proc, send, recv = shim_proc

    send({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"protocolVersion": PROTOCOL_VERSION, "clientInfo": {"name": "test", "version": "0.1"}},
    })
    resp = recv()

    assert resp["jsonrpc"] == "2.0"
    assert resp["id"] == 1
    assert "error" not in resp
    result = resp["result"]
    assert result["protocolVersion"] == PROTOCOL_VERSION
    assert "tools" in result["capabilities"]
    assert result["serverInfo"]["name"] == "attune-mcp-shim"


def test_notifications_initialized_no_response(shim_proc):
    """covers F-15-MCP per MCP spec, notifications/initialized has no response."""
    proc, send, recv = shim_proc

    # Send a notification (no `id` field per JSON-RPC 2.0 notification semantic)
    send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    # Then send a real request to prove the shim is still alive
    send({"jsonrpc": "2.0", "id": 99, "method": "initialize", "params": {}})
    resp = recv(timeout=3.0)
    assert resp["id"] == 99, "shim must continue processing after notifications/initialized"


def test_tools_list_returns_three_tools(shim_proc):
    """covers F-15-MCP tool registry: 3 declared tools per spec."""
    proc, send, recv = shim_proc

    send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})
    recv()  # consume initialize response

    send({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
    resp = recv()

    assert resp["id"] == 2
    tools = resp["result"]["tools"]
    tool_names = {t["name"] for t in tools}
    assert tool_names == {"attune_search", "attune_get_item", "attune_chat"}

    # Each tool MUST have name + description + inputSchema (MCP spec)
    for t in tools:
        assert "name" in t
        assert "description" in t
        assert "inputSchema" in t
        schema = t["inputSchema"]
        assert schema["type"] == "object"
        assert "properties" in schema


def test_unknown_tool_returns_method_not_found(shim_proc):
    """covers F-15-MCP error handling: unknown tool → -32601."""
    proc, send, recv = shim_proc

    send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})
    recv()

    send({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": "no_such_tool", "arguments": {}},
    })
    resp = recv()

    assert resp["id"] == 2
    assert "error" in resp
    assert resp["error"]["code"] == -32601
    assert "no_such_tool" in resp["error"]["message"]


def test_unknown_method_returns_method_not_found(shim_proc):
    """covers F-15-MCP unknown JSON-RPC method → -32601 (not crash)."""
    proc, send, recv = shim_proc

    send({"jsonrpc": "2.0", "id": 1, "method": "completely/made_up", "params": {}})
    resp = recv()

    assert resp["id"] == 1
    assert "error" in resp
    assert resp["error"]["code"] == -32601


def test_malformed_json_does_not_crash_shim(shim_proc):
    """covers F-15-MCP robustness: bad JSON line → stderr log + continue."""
    proc, send, recv = shim_proc
    assert proc.stdin is not None

    # Direct write to bypass JSON encoding
    proc.stdin.write("this is not json{{{\n")
    proc.stdin.flush()

    # Shim should swallow the line + log to stderr, then accept next valid request
    send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})
    resp = recv(timeout=3.0)
    assert resp["id"] == 1
    assert proc.poll() is None, "shim must still be alive after bad JSON"


def test_tools_call_with_dead_backend_returns_error(shim_proc):
    """covers F-15-MCP: when attune-server is unreachable, tool call returns
    structured error (not crash). ATTUNE_BASE_URL=http://127.0.0.1:1 in fixture
    points at a non-listening port."""
    proc, send, recv = shim_proc

    send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})
    recv()

    send({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": "attune_search", "arguments": {"query": "test"}},
    })
    resp = recv(timeout=10.0)  # network errors may take a moment to surface

    assert resp["id"] == 2
    # Either:
    #   - error code (e.g., -32603 internal error)
    #   - result with isError=true (per shim's error envelope on line 168)
    assert ("error" in resp) or (resp.get("result", {}).get("isError") is True), \
        f"backend unreachable should produce error or isError=true, got: {resp}"
