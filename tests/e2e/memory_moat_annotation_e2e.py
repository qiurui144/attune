#!/usr/bin/env python3
"""v0.7 Memory Moat — annotation 完整 CRUD + source 状态 + 信号 E2E。

15 断言：批注 创建/列/更新/删除 + source 状态契约（默认 user，人工编辑后仍 user）
+ annotation_marker 信号 create/update/delete 全覆盖（R21 S2-1 fix）
+ item 删除级联硬删批注 + projects API 可达。

注：annotation color 白名单仅 yellow/red/green/blue（ALLOWED_COLORS）。
前置：起隔离 server + vault setup（密码 e2e-pass-2026）。
用法：python3 tests/e2e/memory_moat_annotation_e2e.py  → 期望 15 PASS / 0 FAIL"""
import json
import sqlite3
import sys
import time
import urllib.error
import urllib.request

BASE = "http://localhost:18905"
VAULT_DB = "/tmp/attune-e2e/data/attune/vault.db"
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
        try:
            return e.code, json.loads(e.read().decode())
        except Exception:
            return e.code, {}


def upload(filename, content):
    boundary = "----attuneAnno"
    body = (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="{filename}"\r\n'
        f"Content-Type: text/markdown\r\n\r\n{content}\r\n--{boundary}--\r\n"
    ).encode()
    r = urllib.request.Request(
        BASE + "/api/v1/upload", data=body,
        headers={"Content-Type": f"multipart/form-data; boundary={boundary}"}, method="POST")
    with urllib.request.urlopen(r, timeout=60) as resp:
        return json.loads(resp.read().decode())


def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  PASS  {name}  {detail}")
    else:
        FAIL += 1
        print(f"  FAIL  {name}  {detail}")


def marker_count():
    conn = sqlite3.connect(f"file:{VAULT_DB}?mode=ro", uri=True, timeout=10)
    try:
        return conn.execute(
            "SELECT COUNT(*) FROM skill_signals WHERE kind='annotation_marker'").fetchone()[0]
    finally:
        conn.close()


print("=== v0.7 Memory Moat — annotation CRUD + source + 信号 E2E ===\n")
req("POST", "/api/v1/vault/unlock", {"password": "e2e-pass-2026"})

# 准备 item
up = upload("anno_doc.md", "# 批注测试文档\n\n这是一段用于测试批注功能的内容。\n")
item_id = up.get("id", "")
check("准备 item 成功", bool(item_id), item_id)

before = marker_count()

# A1: 创建批注 source=user
print("\nA1: 创建批注")
st, d = req("POST", "/api/v1/annotations", {
    "item_id": item_id, "offset_start": 0, "offset_end": 8,
    "text_snippet": "批注测试", "label": "重点", "color": "yellow",
    "content": "这是重点批注内容", "source": "user",
})
ann_id = d.get("id", "")
check("创建批注 → 200 + id", st == 200 and bool(ann_id), ann_id)

# A2: 列批注
print("\nA2: 列批注")
st, d = req("GET", f"/api/v1/annotations?item_id={item_id}")
anns = d.get("annotations", [])
check("列批注 → 1 条", len(anns) == 1, f"{len(anns)} 条")
if anns:
    check("批注 source=user（默认）", anns[0].get("source") == "user", anns[0].get("source"))
    check("批注 label=重点", anns[0].get("label") == "重点", anns[0].get("label"))

# A3: 更新批注 — 改 label
print("\nA3: 更新批注 label 重点→存疑")
st, d = req("PATCH", f"/api/v1/annotations/{ann_id}", {
    "label": "存疑", "color": "red", "content": "改为存疑", "source": "user",
})
check("更新批注 → 200", st == 200, f"st={st}")
st, d = req("GET", f"/api/v1/annotations?item_id={item_id}")
anns = d.get("annotations", [])
if anns:
    check("更新后 label=存疑", anns[0].get("label") == "存疑", anns[0].get("label"))
    check("人工编辑后 source 仍=user（CLAUDE.md 契约）",
          anns[0].get("source") == "user", anns[0].get("source"))

# A4: annotation_marker 信号 — create + update 各 +1（R21 S2-1 fix）
time.sleep(0.5)
after_cu = marker_count()
check("create+update 各写 annotation_marker 信号", after_cu - before >= 2,
      f"{before} → {after_cu}")

# A5: 删除批注
print("\nA5: 删除批注")
st, d = req("DELETE", f"/api/v1/annotations/{ann_id}")
check("删除批注 → 200", st == 200, f"st={st}")
st, d = req("GET", f"/api/v1/annotations?item_id={item_id}")
check("删除后列批注 → 0 条", len(d.get("annotations", [])) == 0)
time.sleep(0.5)
check("delete 也写 annotation_marker 信号（R21 S2-1）",
      marker_count() > after_cu, f"{after_cu} → {marker_count()}")

# A6: item 删除级联删批注
print("\nA6: item 删除级联删批注")
st, d = req("POST", "/api/v1/annotations", {
    "item_id": item_id, "offset_start": 0, "offset_end": 5,
    "text_snippet": "批注", "label": "亮点", "color": "green",
    "content": "级联测试批注", "source": "user",
})
check("再建一条批注用于级联测试", st == 200)
req("DELETE", f"/api/v1/items/{item_id}")
# item 删除后批注表应无该 item 的批注（store.delete_item 硬删 annotations）
conn = sqlite3.connect(f"file:{VAULT_DB}?mode=ro", uri=True, timeout=10)
try:
    n = conn.execute("SELECT COUNT(*) FROM annotations WHERE item_id=?", (item_id,)).fetchone()[0]
finally:
    conn.close()
check("item 删除级联硬删其所有批注", n == 0, f"残留 {n} 条")

# A7: projects API 可达
print("\nA7: projects API")
st, d = req("GET", "/api/v1/projects")
check("GET /api/v1/projects → 200", st == 200, f"st={st}")

print(f"\n=== 结果: {PASS} PASS / {FAIL} FAIL ===")
sys.exit(0 if FAIL == 0 else 1)
