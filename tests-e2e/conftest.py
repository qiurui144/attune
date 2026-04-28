"""
e2e_rust 测试共用 fixture — 启动 attune-server-headless + 提供 base_url。

设计原则:
- 每个 test session 启动一次 server（autouse session fixture），所有测试共享
- server 监听 18901 端口避免冲突生产 18900
- session teardown 时优雅 kill + 输出日志预览
- 使用临时数据目录 (XDG_DATA_HOME) 隔离测试 vault
"""
from __future__ import annotations

import os
import shutil
import socket
import subprocess
import tempfile
import time
from pathlib import Path

import httpx
import pytest

PROJECT_DIR = Path(__file__).resolve().parents[1]
BINARY = PROJECT_DIR / "rust" / "target" / "release" / "attune-server-headless"
TEST_PORT = int(os.environ.get("ATTUNE_E2E_PORT", "18901"))
TEST_HOST = "127.0.0.1"


def _port_open(host: str, port: int, timeout: float = 0.3) -> bool:
    try:
        with socket.create_connection((host, port), timeout=timeout):
            return True
    except OSError:
        return False


@pytest.fixture(scope="session")
def base_url() -> str:
    return f"http://{TEST_HOST}:{TEST_PORT}"


@pytest.fixture(scope="session", autouse=True)
def server_proc(base_url: str):
    """启动 attune-server-headless 子进程，整个 session 共享。"""
    if not BINARY.exists():
        pytest.fail(
            f"binary not built: {BINARY}\n"
            f"  build first: cargo build --release --bin attune-server-headless --manifest-path rust/Cargo.toml"
        )

    # 临时 XDG 数据目录（每次 session 独立 vault，无副作用）
    tmp_data = tempfile.mkdtemp(prefix="attune-e2e-data-")
    env = os.environ.copy()
    env["XDG_DATA_HOME"] = tmp_data
    env["ATTUNE_DATA_HOME"] = tmp_data  # 兼容自定义 env var
    env["ATTUNE_NO_AUTH"] = "1"  # 测试模式跳过 auth

    log_path = Path(tempfile.gettempdir()) / "attune-e2e-server.log"
    log_fd = open(log_path, "w")

    proc = subprocess.Popen(
        [str(BINARY), "--host", TEST_HOST, "--port", str(TEST_PORT), "--no-auth"],
        env=env,
        stdout=log_fd,
        stderr=subprocess.STDOUT,
    )

    # 等待端口就绪 (最多 20s)
    deadline = time.time() + 20
    while time.time() < deadline:
        if proc.poll() is not None:
            log_fd.close()
            log_text = log_path.read_text()[-2000:]
            pytest.fail(f"server exited prematurely (code {proc.returncode})\n{log_text}")
        try:
            r = httpx.get(f"{base_url}/api/v1/status/health", timeout=1.0)
            if r.status_code == 200:
                break
        except (httpx.ConnectError, httpx.ReadTimeout):
            pass
        time.sleep(0.5)
    else:
        proc.terminate()
        log_fd.close()
        log_text = log_path.read_text()[-2000:]
        pytest.fail(f"server did not become ready in 20s\n{log_text}")

    yield proc

    # Teardown
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
    log_fd.close()
    shutil.rmtree(tmp_data, ignore_errors=True)


@pytest.fixture
def client(base_url: str):
    """每个 test 的 httpx client。"""
    with httpx.Client(base_url=base_url, timeout=10.0) as c:
        yield c
