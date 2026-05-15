#!/usr/bin/env python3
"""v0.7 Memory Moat — v0.7 sprint 1 新路由真实 E2E。

/demo/load · /audit/log · /audit/log.csv · /chat/stream。

11 断言：demo/load 加载示例数据 + 幂等 / audit/log 结构化 / audit/log.csv 导出 /
chat/stream SSE 格式 + 空消息 400 + 超 32KB 413（R2 F1 fix 验证）。
chat/stream 当前是 buffered echo stub，不依赖 LLM。

前置：起隔离 server + vault setup（密码 e2e-pass-2026）。
用法：python3 tests/e2e/memory_moat_v07routes_e2e.py  → 期望 11 PASS / 0 FAIL"""
import json
import sys
import urllib.error
import urllib.request

BASE = "http://localhost:18905"
PASS = 0
FAIL = 0


def req(method, path, body=None, raw=False):
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json"} if body is not None else {}
    r = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(r, timeout=60) as resp:
            txt = resp.read().decode()
            return resp.status, (txt if raw else json.loads(txt))
    except urllib.error.HTTPError as e:
        txt = e.read().decode()
        try:
            return e.code, (txt if raw else json.loads(txt))
        except Exception:
            return e.code, txt


def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  PASS  {name}  {detail}")
    else:
        FAIL += 1
        print(f"  FAIL  {name}  {detail}")


print("=== v0.7 Memory Moat — v0.7 新路由 E2E ===\n")
req("POST", "/api/v1/vault/unlock", {"password": "e2e-pass-2026"})

# J1: /demo/load — 加载示例数据
print("J1: POST /api/v1/demo/load")
st, items_before = req("GET", "/api/v1/items?limit=200")
n_before = len(items_before.get("items", [])) if isinstance(items_before, dict) else 0
st, d = req("POST", "/api/v1/demo/load", {})
print(f"  → {st} {json.dumps(d, ensure_ascii=False)[:200]}")
check("demo/load → 200", st == 200, f"st={st}")
st, items_after = req("GET", "/api/v1/items?limit=200")
n_after = len(items_after.get("items", [])) if isinstance(items_after, dict) else 0
check("demo/load 后 items 增加", n_after > n_before, f"{n_before} → {n_after}")

# J2: /demo/load 幂等 — 再次调用不重复加载
print("\nJ2: demo/load 幂等性")
st, d = req("POST", "/api/v1/demo/load", {})
st, items_2nd = req("GET", "/api/v1/items?limit=200")
n_2nd = len(items_2nd.get("items", [])) if isinstance(items_2nd, dict) else 0
check("demo/load 二次调用幂等（items 不再翻倍）", n_2nd == n_after, f"{n_after} → {n_2nd}")

# J3: /audit/log — 审计日志
print("\nJ3: GET /api/v1/audit/log")
st, d = req("GET", "/api/v1/audit/log?limit=50")
check("audit/log → 200", st == 200, f"st={st}")
check("audit/log 返回结构化数据", isinstance(d, (dict, list)), type(d).__name__)

# J4: /audit/log.csv — CSV 导出
print("\nJ4: GET /api/v1/audit/log.csv")
st, csv = req("GET", "/api/v1/audit/log.csv", raw=True)
check("audit/log.csv → 200", st == 200, f"st={st}")
check("audit/log.csv 返回 CSV 文本（含逗号或换行）",
      isinstance(csv, str) and ("," in csv or "\n" in csv or csv == ""),
      f"len={len(csv) if isinstance(csv, str) else 'N/A'}")

# J5: /chat/stream — buffered SSE
print("\nJ5: POST /api/v1/chat/stream")
st, body = req("POST", "/api/v1/chat/stream", {"message": "测试流式响应内容"}, raw=True)
check("chat/stream → 200", st == 200, f"st={st}")
check("chat/stream 返回 SSE 格式（含 data: 行）",
      isinstance(body, str) and "data:" in body, f"len={len(body) if isinstance(body,str) else 0}")

# J6: chat/stream 空消息拒绝
st, body = req("POST", "/api/v1/chat/stream", {"message": ""}, raw=True)
check("chat/stream 空消息被拒（400）", st == 400, f"st={st}")

# J7: chat/stream 超长消息拒绝 (R2 F1 fix)
st, body = req("POST", "/api/v1/chat/stream", {"message": "x" * 40000}, raw=True)
check("chat/stream 超 32KB 消息被拒（413, R2 F1 fix）", st == 413, f"st={st}")

print(f"\n=== 结果: {PASS} PASS / {FAIL} FAIL ===")
sys.exit(0 if FAIL == 0 else 1)
