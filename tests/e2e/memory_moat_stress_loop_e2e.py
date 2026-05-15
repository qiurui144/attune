#!/usr/bin/env python3
"""v0.7 Memory Moat — 持续操作压力 + 内存泄漏监控。

循环 upload→search→PATCH→search→delete 120 轮（600 HTTP 调用），监控 server
进程 RSS + FD，验证无内存/句柄泄漏。

5 断言：零错误 / 后半程 RSS 增长 < 50MB / FD 稳定 / server 健康 / vault 可用。
实测基准（i9-14900K）：120 轮 ~11s，RSS 后半程涨 ~0.2MB、FD 恒定。

前置：起隔离 server + vault setup（密码 e2e-pass-2026）。
用法：python3 tests/e2e/memory_moat_stress_loop_e2e.py  → 期望 5 PASS / 0 FAIL"""
import json
import subprocess
import sys
import time
import urllib.error
import urllib.request

BASE = "http://localhost:18905"
ROUNDS = 120
PASS = 0
FAIL = 0


def req(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json"} if body is not None else {}
    r = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(r, timeout=30) as resp:
            return resp.status, json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        return e.code, {}
    except Exception:
        return -1, {}


def upload(filename, content):
    boundary = "----attuneLoop"
    body = (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="{filename}"\r\n'
        f"Content-Type: text/markdown\r\n\r\n{content}\r\n--{boundary}--\r\n"
    ).encode()
    r = urllib.request.Request(
        BASE + "/api/v1/upload", data=body,
        headers={"Content-Type": f"multipart/form-data; boundary={boundary}"}, method="POST")
    try:
        with urllib.request.urlopen(r, timeout=30) as resp:
            return json.loads(resp.read().decode())
    except Exception:
        return {}


def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  PASS  {name}  {detail}")
    else:
        FAIL += 1
        print(f"  FAIL  {name}  {detail}")


def server_rss_kb():
    """读 server 进程 /proc/<pid>/status VmRSS。"""
    try:
        pid = subprocess.check_output(
            ["pgrep", "-f", "attune-server-headless.*18905"]).decode().split()[0]
        for line in open(f"/proc/{pid}/status"):
            if line.startswith("VmRSS:"):
                return int(line.split()[1])
            if line.startswith("FDSize:"):
                pass
        return -1
    except Exception:
        return -1


def server_fd_count():
    try:
        pid = subprocess.check_output(
            ["pgrep", "-f", "attune-server-headless.*18905"]).decode().split()[0]
        return len(subprocess.check_output(["ls", f"/proc/{pid}/fd"]).decode().split())
    except Exception:
        return -1


print(f"=== v0.7 Memory Moat — 持续操作压力 {ROUNDS} 轮 + 内存监控 ===\n")
req("POST", "/api/v1/vault/unlock", {"password": "e2e-pass-2026"})

rss0 = server_rss_kb()
fd0 = server_fd_count()
print(f"基线: RSS={rss0/1024:.1f} MB, FD={fd0}\n")

samples = []
errors = 0
t_start = time.time()

for i in range(ROUNDS):
    # upload
    up = upload(f"loop{i}.md",
                f"# 循环文档 {i}\n\nLOOPMARK{i} 内容段落 tokio rust async。\n\n## 节\n\n更多内容。\n")
    iid = up.get("id", "")
    if not iid:
        errors += 1
        continue
    # search
    req("GET", f"/api/v1/search?q=LOOPMARK{i}")
    # PATCH
    req("PATCH", f"/api/v1/items/{iid}",
        {"content": f"# 循环文档 {i} 改\n\nLOOPMARK{i}EDIT 新内容 tokio。\n"})
    # search again
    req("GET", f"/api/v1/search?q=tokio")
    # delete
    st, _ = req("DELETE", f"/api/v1/items/{iid}")
    if st != 200:
        errors += 1

    if (i + 1) % 30 == 0:
        rss = server_rss_kb()
        fd = server_fd_count()
        samples.append((i + 1, rss, fd))
        print(f"  轮 {i+1:3d}: RSS={rss/1024:7.1f} MB, FD={fd}, errors={errors}")

elapsed = time.time() - t_start
rss_final = server_rss_kb()
fd_final = server_fd_count()
print(f"\n{ROUNDS} 轮完成, 耗时 {elapsed:.0f}s ({ROUNDS*5} HTTP 调用)")
print(f"RSS: {rss0/1024:.1f} → {rss_final/1024:.1f} MB, FD: {fd0} → {fd_final}\n")

# 验证
check("持续操作零错误", errors == 0, f"errors={errors}")

# 内存增长：warmup 后应趋稳。比较后半程样本斜率。
if len(samples) >= 3:
    mid_rss = samples[len(samples) // 2][1]
    growth_late = (rss_final - mid_rss) / 1024  # 后半程 MB 增长
    check("后半程 RSS 增长 < 50MB（无明显泄漏）", growth_late < 50,
          f"后半程增长 {growth_late:.1f} MB")

# FD 不应持续增长（句柄泄漏）
check("FD 句柄数稳定（增长 < 50）", fd_final - fd0 < 50, f"{fd0} → {fd_final}")

# server 仍健康
st, _ = req("GET", "/health")
check("压力测试后 server /health 正常", st == 200, f"st={st}")

# vault 仍可正常操作
up = upload("final.md", "# 最终验证\n\nFINAL_CHECK 内容。\n")
check("压力后 vault 仍可正常 upload", bool(up.get("id")), up.get("id", ""))
if up.get("id"):
    req("DELETE", f"/api/v1/items/{up['id']}")

print(f"\n=== 结果: {PASS} PASS / {FAIL} FAIL ===")
sys.exit(0 if FAIL == 0 else 1)
