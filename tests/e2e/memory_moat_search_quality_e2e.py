#!/usr/bin/env python3
"""v0.7 Memory Moat — search RRF 混合检索召回质量 E2E。

6 主题语料（Rust/Python/数据库/网络/加密/ML）+ 针对性 query，验证 RRF 混合检索
（FTS 精确 + 向量语义）top-1 召回正确主题文档。8 断言：6 query top-1 命中
（中文语义/精确词/混合）+ 跨主题区分度 + 语料上传。

用 item_id 精确匹配（不依赖 title — title 取文档 H1，可能与外部命名不一致）。
前置：起隔离 server + vault setup（密码 e2e-pass-2026）+ Ollama bge-m3（向量分量）。
用法：python3 tests/e2e/memory_moat_search_quality_e2e.py  → 期望 8 PASS / 0 FAIL"""
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
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json"} if body is not None else {}
    r = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(r, timeout=30) as resp:
            return resp.status, json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        return e.code, {}


def upload(filename, content):
    boundary = "----attuneSQ"
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


def top_item(q):
    """返回 top-1 命中的 item_id（用 id 精确匹配，不依赖 title 字符串）。"""
    _, d = req("GET", f"/api/v1/search?q={urllib.parse.quote(q)}")
    rs = d.get("results", [])
    if not rs:
        return None, 0
    return rs[0].get("item_id", ""), len(rs)


print("=== v0.7 Memory Moat — search RRF 召回质量 E2E ===\n")
req("POST", "/api/v1/vault/unlock", {"password": "e2e-pass-2026"})

# 6 个不同主题的语料文档
print("准备 6 主题语料 ...")
corpus = {
    "rust_async": ("Rust 异步编程指南",
        "# Rust 异步编程\n\nRust 的 async/await 基于 Future trait。tokio 是最流行的"
        "异步运行时，提供 work-stealing 调度器。executor 负责 poll future 直到完成。\n"),
    "python_data": ("Python 数据分析",
        "# Python 数据分析\n\npandas 提供 DataFrame 做表格数据处理。numpy 是数值计算基础库，"
        "提供 ndarray 多维数组。matplotlib 负责数据可视化绘图。\n"),
    "database_index": ("数据库索引原理",
        "# 数据库索引\n\nB+ 树索引是关系数据库最常见的索引结构。聚簇索引决定数据物理存储顺序。"
        "覆盖索引可以避免回表查询。慢查询通常源于缺失索引。\n"),
    "network_tcp": ("TCP 网络协议",
        "# TCP 协议\n\nTCP 三次握手建立连接。滑动窗口实现流量控制。拥塞控制算法防止网络过载。"
        "四次挥手关闭连接。\n"),
    "crypto_aes": ("AES 加密算法",
        "# AES 加密\n\nAES 是对称加密算法，分组长度 128 位。GCM 模式提供认证加密。"
        "密钥派生用 Argon2id 抵抗暴力破解。\n"),
    "ml_transformer": ("Transformer 模型",
        "# Transformer\n\nTransformer 基于自注意力机制 self-attention。多头注意力并行捕捉"
        "不同子空间特征。位置编码注入序列顺序信息。\n"),
}
ids = {}
for key, (title, content) in corpus.items():
    up = upload(f"{key}.md", content)
    ids[key] = up.get("id", "")
check("6 主题语料全部 upload 成功", all(ids.values()), f"{sum(1 for v in ids.values() if v)}/6")

print("等待 embedding worker 处理全部 chunk ...")
time.sleep(12)

# 针对性 query — 验证 top-1 召回正确主题
print("\n召回质量验证（top-1 命中正确主题文档）")
cases = [
    ("tokio 异步运行时调度器", "rust_async", "中文语义 — Rust 异步"),
    ("pandas DataFrame 数据处理", "python_data", "中文语义 — Python"),
    ("B+ 树 聚簇索引", "database_index", "精确词 — 数据库"),
    ("三次握手 滑动窗口", "network_tcp", "精确词 — 网络"),
    ("对称加密 GCM 认证", "crypto_aes", "中文语义 — 加密"),
    ("self-attention 多头注意力", "ml_transformer", "混合 — ML"),
]
for query, expect_key, desc in cases:
    iid, n = top_item(query)
    check(f"[{desc}] '{query[:20]}' → top-1 命中正确主题",
          iid == ids[expect_key], f"got {iid[:12] if iid else None} (期望 {ids[expect_key][:12]}, {n} 命中)")

# 跨主题区分度 — 搜一个主题不该让无关主题排第一
print("\n跨主题区分度")
iid, _ = top_item("Argon2id 密钥派生")
check("加密专属 query top-1 是加密文档", iid == ids["crypto_aes"],
      f"top-1={iid[:12] if iid else None}")

# 清理
for iid in ids.values():
    if iid:
        req("DELETE", f"/api/v1/items/{iid}")

print(f"\n=== 结果: {PASS} PASS / {FAIL} FAIL ===")
sys.exit(0 if FAIL == 0 else 1)
