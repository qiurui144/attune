#!/usr/bin/env python3
"""v0.7 Memory Moat — 异常/故障注入 E2E。

畸形输入 / 超限 / 坏 JSON / 404 / vault lock 中途操作 — 验证 server 优雅拒绝不崩。

13 断言：空内容拒绝 / 3.5MB content PATCH 成功（验 body limit 100MB）/
超长 title 413 / 超长 id 400 / 坏 JSON 4xx / 删&取不存在 item 404 /
畸形轰炸后 server 健康 / vault lock 后操作 403 / unlock 恢复。

前置：起隔离 server + vault setup（密码 e2e-pass-2026）。
用法：python3 tests/e2e/memory_moat_fault_e2e.py  → 期望 13 PASS / 0 FAIL"""
import json
import sys
import urllib.error
import urllib.request

BASE = "http://localhost:18905"
PASS = 0
FAIL = 0


def raw(method, path, body_bytes=None, ctype="application/json"):
    headers = {"Content-Type": ctype} if body_bytes is not None else {}
    r = urllib.request.Request(BASE + path, data=body_bytes, headers=headers, method=method)
    try:
        with urllib.request.urlopen(r, timeout=30) as resp:
            return resp.status, resp.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()
    except Exception as e:
        return -1, str(e)


def req(method, path, body=None):
    b = json.dumps(body).encode() if body is not None else None
    return raw(method, path, b)


def upload_raw(filename, content):
    boundary = "----attuneFault"
    body = (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="{filename}"\r\n'
        f"Content-Type: text/markdown\r\n\r\n{content}\r\n--{boundary}--\r\n"
    ).encode()
    return raw("POST", "/api/v1/upload", body,
               ctype=f"multipart/form-data; boundary={boundary}")


def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  PASS  {name}  {detail}")
    else:
        FAIL += 1
        print(f"  FAIL  {name}  {detail}")


print("=== v0.7 Memory Moat — 异常/故障注入 E2E ===\n")
req("POST", "/api/v1/vault/unlock", {"password": "e2e-pass-2026"})

# F1: 空文档 upload → 应拒绝
print("F1: 空内容 upload")
st, body = upload_raw("empty.md", "   \n  \n")
check("空内容 upload 被拒（非 200）", st != 200, f"st={st}")
check("server 未崩（返回了 HTTP 状态码）", st > 0, f"st={st}")

# F2: PATCH 3.5MB content — 验证 body limit 已从默认 2MB 提到 100MB
# （Round D 修复：PATCH /items 路由加 DefaultBodyLimit(100MB) 与 upload 对齐）
print("\nF2: PATCH 3.5MB content (验证 body limit 100MB, 旧默认 2MB 会拒)")
st, body = upload_raw("base.md", "# 基础文档\n\n正常内容用于 PATCH 测试。\n")
base_id = json.loads(body).get("id", "") if st == 200 else ""
big_content = "# 大文档\n\n" + ("abcdefghij " * 350_000)  # ~3.5MB ASCII > 旧 2MB 上限
st, body = req("PATCH", f"/api/v1/items/{base_id}", {"content": big_content})
check("3.5MB content PATCH 成功 (body limit 已 100MB)", st == 200, f"st={st}")
del big_content

# F3: PATCH title 超 1024 → 413
print("\nF3: PATCH title 超长")
st, body = req("PATCH", f"/api/v1/items/{base_id}", {"title": "T" * 2000})
check("超长 title PATCH 被拒 (413)", st == 413, f"st={st}")

# F4: id 超 64 → 400
print("\nF4: 超长 id")
st, body = req("PATCH", "/api/v1/items/" + ("a" * 100), {"title": "x"})
check("超长 id PATCH 被拒 (400)", st == 400, f"st={st}")

# F5: 坏 JSON body
print("\nF5: 畸形 JSON body")
st, body = raw("PATCH", f"/api/v1/items/{base_id}", b'{"content": broken json}')
check("坏 JSON 被拒（4xx）", 400 <= st < 500, f"st={st}")
check("坏 JSON 后 server 未崩", st > 0)

# F6: 删不存在的 item → 404
print("\nF6: 删除不存在的 item")
st, body = req("DELETE", "/api/v1/items/nonexistent12345")
check("删不存在 item → 404", st == 404, f"st={st}")

# F7: GET 不存在的 item → 404
st, body = req("GET", "/api/v1/items/nonexistent12345")
check("GET 不存在 item → 404", st == 404, f"st={st}")

# F8: server 健康（一连串畸形请求后）
print("\nF8: 畸形请求轰炸后 server 仍健康")
st, body = req("GET", "/health")
check("畸形请求后 server /health 正常", st == 200, f"st={st}")

# F9: vault lock 中途操作 → 403
print("\nF9: vault lock 后操作应 403")
req("POST", "/api/v1/vault/lock", {})
st, body = req("GET", f"/api/v1/items/{base_id}")
check("vault locked 后 GET item → 403", st == 403, f"st={st}")
st, body = upload_raw("afterlock.md", "# 锁后上传\n\n内容\n")
check("vault locked 后 upload → 403", st == 403, f"st={st}")

# F10: unlock 恢复 → 操作正常
print("\nF10: unlock 后恢复正常")
req("POST", "/api/v1/vault/unlock", {"password": "e2e-pass-2026"})
st, body = req("GET", f"/api/v1/items/{base_id}")
check("unlock 后 GET item 恢复正常", st == 200, f"st={st}")

# 清理
req("DELETE", f"/api/v1/items/{base_id}")

print(f"\n=== 结果: {PASS} PASS / {FAIL} FAIL ===")
sys.exit(0 if FAIL == 0 else 1)
