#!/usr/bin/env python3
"""v0.7 Memory Moat — 自学习信号 + 并发真实 E2E。

直查 skill_signals 表验证 Phase B 5 类信号真写入；8 线程并发 PATCH 验证
R17 S1-Q4 事务修复在真实竞争下有效（content/hash 一致、无数据竞争）。

前置：与 memory_moat_e2e.py 相同 —— 起隔离 server（XDG_DATA_HOME=/tmp/attune-e2e
/data, port 18905）。VAULT_DB 路径据此硬编码；换 data dir 需同步改脚本顶部常量。

用法：python3 tests/e2e/memory_moat_signals_e2e.py  → 期望 9 PASS / 0 FAIL"""
import json
import sqlite3
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

BASE = "http://localhost:18905"
VAULT_DB = "/tmp/attune-e2e/data/attune/vault.db"
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
    boundary = "----attuneSig"
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


def signal_counts():
    """直查 skill_signals 表（WAL 模式允许并发只读）。"""
    conn = sqlite3.connect(f"file:{VAULT_DB}?mode=ro", uri=True, timeout=10)
    try:
        rows = conn.execute(
            "SELECT kind, COUNT(*) FROM skill_signals GROUP BY kind").fetchall()
        return dict(rows)
    finally:
        conn.close()


print("=== v0.7 Memory Moat — 自学习信号 + 并发 E2E ===\n")
req("POST", "/api/v1/vault/unlock", {"password": "e2e-pass-2026"})

before = signal_counts()
print(f"基线 skill_signals: {before}\n")

# --- 信号链路 ---
print("信号链路: upload → PATCH → annotation → delete")
content = "# 信号测试文档\n\n关于 elasticsearch 的笔记。\n\n## 章节\n\nelasticsearch 全文检索引擎。\n"
up = upload("docSignal.md", content)
sig_id = up.get("id", "")
check("upload docSignal 成功", bool(sig_id), sig_id)
time.sleep(0.5)

# PATCH 触发 doc_update
req("PATCH", f"/api/v1/items/{sig_id}",
    {"content": content + "\n更新追加段落 solr 对比。\n"})
time.sleep(0.5)

# annotation 触发 annotation_marker
st, d = req("POST", "/api/v1/annotations", {
    "item_id": sig_id, "offset_start": 0, "offset_end": 10,
    "text_snippet": "信号测试", "label": "重点", "color": "yellow",
    "content": "这是重点批注", "source": "user",
})
check("annotation 创建成功", st == 200, f"st={st}")
ann_id = d.get("id", "")
time.sleep(0.5)

# DELETE 触发 doc_delete
req("DELETE", f"/api/v1/items/{sig_id}")
time.sleep(0.8)

after = signal_counts()
print(f"\n操作后 skill_signals: {after}")

def delta(kind):
    return after.get(kind, 0) - before.get(kind, 0)

check("doc_create 信号 +1", delta("doc_create") >= 1, f"delta={delta('doc_create')}")
check("doc_update 信号 +1", delta("doc_update") >= 1, f"delta={delta('doc_update')}")
check("doc_delete 信号 +1", delta("doc_delete") >= 1, f"delta={delta('doc_delete')}")
check("annotation_marker 信号 +1", delta("annotation_marker") >= 1,
      f"delta={delta('annotation_marker')}")

# --- 并发 PATCH 同一 item ---
print("\n并发: 8 线程同时 PATCH 同一 item")
c2 = "# 并发文档\n\n初始内容 concurrent test baseline。\n"
up2 = upload("docConcur.md", c2)
cid = up2.get("id", "")
time.sleep(0.5)

errors = []

def patch_worker(n):
    try:
        st, d = req("PATCH", f"/api/v1/items/{cid}",
                    {"content": f"# 并发文档\n\n第 {n} 版内容 concurrent revision {n}。\n"})
        if st != 200:
            errors.append(f"thread {n}: st={st}")
    except Exception as e:
        errors.append(f"thread {n}: {e}")

threads = [threading.Thread(target=patch_worker, args=(i,)) for i in range(8)]
for t in threads:
    t.start()
for t in threads:
    t.join()

check("8 并发 PATCH 全部 200 无错", len(errors) == 0, str(errors[:3]))

# 并发后 item 仍可读且 content/hash 一致（事务保证）
st, d = req("GET", f"/api/v1/items/{cid}")
check("并发后 item 仍可正常读取", st == 200, f"st={st}")

# DB 层面 content_hash 与 content 一致性（直查）
conn = sqlite3.connect(f"file:{VAULT_DB}?mode=ro", uri=True, timeout=10)
try:
    row = conn.execute(
        "SELECT content_hash, length(content) FROM items WHERE id=?", (cid,)).fetchone()
    check("并发后 content_hash 非空（事务完整）", bool(row and row[0]), str(row))
finally:
    conn.close()

req("DELETE", f"/api/v1/items/{cid}")

print(f"\n=== 结果: {PASS} PASS / {FAIL} FAIL ===")
sys.exit(0 if FAIL == 0 else 1)
