#!/usr/bin/env python3
"""v0.7 Memory Moat — 大文档 + 边界文档真实压力 E2E。

验证 chunker 边界（无换行长行 / code fence 不平衡 / 多语言 emoji）+ 大文档
reindex 在真实 server 下不 panic、搜索正确、耗时可测。

R10 滚动 review 用此脚本捕获 S3 update 竞态：100KB 文档 1278 chunk，PATCH 后
embedding worker 仍在异步写 PATCH 前的旧 chunk 向量 → 编辑后旧关键词仍搜得到。
根治：embed worker 写向量前查 embed_task_exists（被 reindex purge 删的 chunk
任务 → 跳过）。

前置：起隔离 server（XDG_DATA_HOME=/tmp/attune-e2e/data, port 18905）。
用法：python3 tests/e2e/memory_moat_stress_e2e.py  → 期望 11 PASS / 0 FAIL"""
import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

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
        with urllib.request.urlopen(r, timeout=60) as resp:
            return resp.status, json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read().decode())
        except Exception:
            return e.code, {}


def upload(filename, content):
    boundary = "----attuneStress"
    body = (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="{filename}"\r\n'
        f"Content-Type: text/markdown\r\n\r\n{content}\r\n--{boundary}--\r\n"
    ).encode()
    r = urllib.request.Request(
        BASE + "/api/v1/upload", data=body,
        headers={"Content-Type": f"multipart/form-data; boundary={boundary}"}, method="POST")
    try:
        with urllib.request.urlopen(r, timeout=120) as resp:
            return resp.status, json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read().decode())
        except Exception:
            return e.code, {}


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
    return len(d.get("results", []))


def keyword_in_hits(q, keyword):
    """search q，返回命中结果的 content 是否含 keyword 文字。
    用于"编辑后旧词消失"验证 —— search 是 RRF 混合，向量分量会语义模糊召回
    同主题文档（score 噪音级），故验"命中内容不含旧词文字"而非 results==0。"""
    _, d = req("GET", f"/api/v1/search?q={urllib.parse.quote(q)}")
    return any(keyword in h.get("content", "") for h in d.get("results", []))


print("=== v0.7 Memory Moat — 大文档 + 边界文档压力 E2E ===\n")
req("POST", "/api/v1/vault/unlock", {"password": "e2e-pass-2026"})

# --- 大文档 ---
print("CASE 1: 100KB 大文档 upload + PATCH reindex")
para = ("Rust 异步编程涉及 future、executor、waker 等概念。tokio 是主流运行时。"
        "BENCHMARK_KEYWORD_ALPHA 是本文档独特标识。\n\n")
big = "# 大文档测试\n\n"
h = 1
while len(big.encode()) < 100_000:
    big += f"## 章节 {h}\n\n{para}"
    h += 1
print(f"  文档大小: {len(big.encode())} bytes, {h} 章节")

t0 = time.time()
st, up = upload("big.md", big)
up_ms = (time.time() - t0) * 1000
big_id = up.get("id", "")
check("100KB 文档 upload 成功", st == 200 and bool(big_id), f"{up_ms:.0f}ms, chunks={up.get('chunks_queued')}")

time.sleep(1)
check("大文档搜独特关键词命中", search_count("BENCHMARK_KEYWORD_ALPHA") >= 1)

big2 = big.replace("BENCHMARK_KEYWORD_ALPHA", "BENCHMARK_KEYWORD_BETA")
t0 = time.time()
st, d = req("PATCH", f"/api/v1/items/{big_id}", {"content": big2})
patch_ms = (time.time() - t0) * 1000
check("100KB PATCH reindex 成功", st == 200 and d.get("content_changed") is True,
      f"{patch_ms:.0f}ms, reindex={d.get('reindex')}")
time.sleep(1.5)
check("大文档编辑后旧关键词文字从命中内容消失",
      not keyword_in_hits("BENCHMARK_KEYWORD_ALPHA", "BENCHMARK_KEYWORD_ALPHA"),
      "RRF 向量分量可能模糊召回，验内容不含旧词")
check("大文档编辑后新关键词搜得到", search_count("BENCHMARK_KEYWORD_BETA") >= 1)
req("DELETE", f"/api/v1/items/{big_id}")

# --- 边界文档 ---
print("\nCASE 2: 边界文档 — 不 panic 不死循环")

# 2a 无换行超长行
nolf = "# 无换行\n\n" + "x" * 50_000 + " BOUNDARY_NOLF_MARK"
st, up = upload("nolf.md", nolf)
check("无换行 50KB 长行 upload 不崩", st == 200, f"st={st}")
nolf_id = up.get("id", "")

# 2b code fence 不平衡
unbal = "# Fence\n\n正常段落 BOUNDARY_FENCE_MARK。\n\n```rust\nfn x() {}\n// 没有闭合 fence\n\n更多文字\n"
st, up = upload("unbal.md", unbal)
check("code fence 不平衡 upload 不崩", st == 200, f"st={st}")
unbal_id = up.get("id", "")

# 2c 多语言 emoji 混排
multi = "# 多语言\n\n中文 English 日本語 한국어 🚀🔥✨ émojis BOUNDARY_MULTI_MARK\n\n## 节\n\nمرحبا Ω≈ç√∫\n"
st, up = upload("multi.md", multi)
check("多语言+emoji upload 不崩（无 char boundary panic）", st == 200, f"st={st}")
multi_id = up.get("id", "")

time.sleep(1.5)
check("边界文档可搜 — 无换行长行", search_count("BOUNDARY_NOLF_MARK") >= 1)
check("边界文档可搜 — 多语言", search_count("BOUNDARY_MULTI_MARK") >= 1)

# server 仍健康（边界文档没把 server 搞挂）
st, _ = req("GET", "/api/v1/vault/status")
check("边界文档处理后 server 仍健康", st == 200, f"st={st}")

for x in (nolf_id, unbal_id, multi_id):
    if x:
        req("DELETE", f"/api/v1/items/{x}")

print(f"\n=== 结果: {PASS} PASS / {FAIL} FAIL ===")
print(f"perf 实测: 100KB upload={up_ms:.0f}ms / PATCH-reindex={patch_ms:.0f}ms")
sys.exit(0 if FAIL == 0 else 1)
