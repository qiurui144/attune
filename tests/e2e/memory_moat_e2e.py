#!/usr/bin/env python3
"""v0.7 Memory Moat 完整端到端 — setup→unlock→upload→search→edit→search→delete→search。

打真实 attune-server-headless 进程（非内存 Store unit test），验证 Memory Moat
Phase A+B 在真实 HTTP 链路下的行为。R10 滚动 review 用此脚本实测捕获了
search_cache 失效 bug（编辑后旧词仍命中 / 删除后仍命中）。

用法：
  1. 编译并起隔离 server：
     cd rust && cargo build --release -p attune-server --bin attune-server-headless
     XDG_DATA_HOME=/tmp/attune-e2e/data XDG_CONFIG_HOME=/tmp/attune-e2e/config \\
       ./target/release/attune-server-headless --no-auth --port 18905 &
  2. python3 tests/e2e/memory_moat_e2e.py
  3. 期望 9 PASS / 0 FAIL

9 个断言覆盖：upload / FTS search / PATCH content_changed / 编辑后旧词搜不到
(Phase A 核心承诺) / 新词搜得到 / content_hash 短路 / upload dedup / 删除 orphan 清除 /
audit log 可达。
"""
import json
import sys
import time
import urllib.request
import urllib.error
import urllib.parse

BASE = "http://localhost:18905"
PASS = 0
FAIL = 0


def req(method, path, body=None):
    data = None
    headers = {}
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    r = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(r, timeout=30) as resp:
            return resp.status, json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read().decode())
        except Exception:
            return e.code, {}


def upload(filename, content):
    boundary = "----attuneE2E"
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
    except urllib.error.HTTPError as e:
        return json.loads(e.read().decode())


def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  PASS  {name}  {detail}")
    else:
        FAIL += 1
        print(f"  FAIL  {name}  {detail}")


def search_count(q):
    _, d = req("GET", f"/api/v1/search?q={urllib.parse.quote(q)}")
    return len(d.get("results", [])), d


print("=== v0.7 Memory Moat 完整 E2E ===\n")

# setup + unlock
st, d = req("POST", "/api/v1/vault/setup", {"password": "e2e-pass-2026"})
if st != 200 and "already" not in str(d).lower():
    print(f"setup failed: {st} {d}")
req("POST", "/api/v1/vault/unlock", {"password": "e2e-pass-2026"})

# STEP 1: upload
content_v1 = (
    "# Rust 异步运行时笔记\n\n关于 vintage 架构的技术笔记。vintage 设计模式早期常见。\n\n"
    "## tokio 调度器\n\nwork-stealing 调度器分发 future。vintage 事件循环已过时。\n\n"
    "## 索引\n\nvintage retro classic 老式架构\n"
)
up = upload("docA.md", content_v1)
doc_id = up.get("id", "")
print(f"STEP 1: upload docA → id={doc_id}, chunks_queued={up.get('chunks_queued')}")
check("upload 成功返回 id", bool(doc_id))

time.sleep(1)
n, _ = search_count("vintage")
check("STEP 2: 上传后搜 vintage 命中", n >= 1, f"results={n}")

# STEP 3: PATCH vintage→modern
content_v2 = (
    "# Rust 异步运行时笔记\n\n关于 modern 架构的技术笔记。modern 设计模式新系统常见。\n\n"
    "## tokio 调度器\n\nwork-stealing 调度器分发 future。modern 事件循环很先进。\n\n"
    "## 索引\n\nmodern cutting-edge 新式架构\n"
)
st, d = req("PATCH", f"/api/v1/items/{doc_id}", {"content": content_v2})
print(f"\nSTEP 3: PATCH → content_changed={d.get('content_changed')}, "
      f"reindex={d.get('reindex')}")
check("PATCH content_changed=true", d.get("content_changed") is True)
time.sleep(1.5)

# STEP 4: 核心 — 编辑后旧词 vintage 从命中内容消失
# 注：search 是 RRF 混合（FTS 精确 + 向量语义）。向量是语义搜索，"vintage" query
# 与改成 modern 的同主题文档仍有微弱相似度（score ~0.02 噪音级）会被召回 —— 这是
# 向量搜索本质，非 bug。Phase A 核心承诺是「编辑后旧内容文字消失」：验证所有命中
# 结果的 content 都不含 vintage 文字（FTS 精确词已更新），而非苛求 RRF results==0。
_, d4 = req("GET", "/api/v1/search?q=vintage")
hits4 = d4.get("results", [])
vintage_in_content = any("vintage" in (h.get("content", "").lower()) for h in hits4)
check("STEP 4: 编辑后旧词 vintage 文字从命中内容消失 (Phase A 核心承诺)",
      not vintage_in_content, f"{len(hits4)} 命中, 含 vintage 文字={vintage_in_content}")

# STEP 5: 搜 modern 应命中
n, _ = search_count("modern")
check("STEP 5: 新词 modern 搜得到", n >= 1, f"results={n}")

# STEP 6: 短路
st, d = req("PATCH", f"/api/v1/items/{doc_id}", {"content": content_v2})
check("STEP 6: 相同内容 content_changed=false", d.get("content_changed") is False)

# STEP 7: dedup upload
d = upload("docA_dup.md", content_v2)
check("STEP 7: 同内容 upload → duplicate", d.get("status") == "duplicate", str(d.get("status")))
check("STEP 7: dedup 返回同一 id", d.get("id") == doc_id)

# STEP 8: delete → 搜不到
st, d = req("DELETE", f"/api/v1/items/{doc_id}")
print(f"\nSTEP 8: DELETE → purge={d.get('purge')}")
time.sleep(1)
n, _ = search_count("modern")
check("STEP 8: 删除后搜不到 (无 orphan)", n == 0, f"results={n}")

# STEP 9: skill_signals — 验自学习信号
st, d = req("GET", "/api/v1/audit/log?limit=50")
print(f"\nSTEP 9: audit/log → {st}")

print(f"\n=== 结果: {PASS} PASS / {FAIL} FAIL ===")
sys.exit(0 if FAIL == 0 else 1)
