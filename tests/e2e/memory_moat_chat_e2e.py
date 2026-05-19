#!/usr/bin/env python3
"""v0.7 Memory Moat — chat RAG 真实端到端。

真实 Ollama qwen2.5:3b 推理 + bge-m3 检索，验证 RAG 链路 + citation_hit 信号落库。

前置（除起隔离 server 外）：
  - Ollama 运行，已 pull qwen2.5:3b + bge-m3
  - server 配 LLM provider（脚本运行前先 PATCH /api/v1/settings）：
    {"llm":{"provider":"openai_compat","endpoint":"http://localhost:11434/v1",
            "model":"qwen2.5:3b","api_key":"ollama"}}
    settings.llm 变更触发 server LLM provider 热重载（commit d388282）。

注意：chat answer 在 response 的 `content` 字段（非 response/answer）。
9 断言：LLM 连通 / upload / chat 200 / answer 非空 / RAG 答案命中知识库 /
citations 引用 / citation 指向正确文档 / citation_hit 信号落库 / 多轮问答。
用法：python3 tests/e2e/memory_moat_chat_e2e.py  → 期望 9 PASS / 0 FAIL"""
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


def req(method, path, body=None, timeout=30):
    data = None
    headers = {}
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    r = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(r, timeout=timeout) as resp:
            return resp.status, json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read().decode())
        except Exception:
            return e.code, {}


def upload(filename, content):
    boundary = "----attuneChat"
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


def citation_hit_count():
    conn = sqlite3.connect(f"file:{VAULT_DB}?mode=ro", uri=True, timeout=10)
    try:
        return conn.execute(
            "SELECT COUNT(*) FROM skill_signals WHERE kind='citation_hit'").fetchone()[0]
    finally:
        conn.close()


print("=== v0.7 Memory Moat — chat RAG 真实端到端 ===\n")
req("POST", "/api/v1/vault/unlock", {"password": "e2e-pass-2026"})

# LLM 连通性
print("STEP 1: LLM 连通性测试")
st, d = req("POST", "/api/v1/llm/test", {
    "endpoint": "http://localhost:11434/v1", "api_key": "ollama", "model": "qwen2.5:3b",
}, timeout=60)
print(f"  llm/test → {st} {json.dumps(d, ensure_ascii=False)[:200]}")
check("LLM (qwen2.5:3b) 连通", st == 200 and d.get("ok") is True, str(d.get("ok", d)))

# upload 知识文档
print("\nSTEP 2: upload 知识文档")
kb = (
    "# Attune 技术架构\n\n"
    "## 向量检索\n\n"
    "Attune 的向量索引使用 usearch HNSW 算法，采用 f16 量化以节省内存。"
    "向量相似度用 cosine 度量。\n\n"
    "## 全文检索\n\n"
    "全文检索由 tantivy 引擎负责，中文分词用 tantivy-jieba。\n\n"
    "## 加密\n\n"
    "数据用 Argon2id 派生密钥 + AES-256-GCM 字段级加密。\n"
)
up = upload("attune_arch.md", kb)
kb_id = up.get("id", "")
check("知识文档 upload 成功", bool(kb_id), kb_id)
# 等 embedding worker 处理（向量检索需要）
print("  等待 embedding worker 处理 chunk...")
time.sleep(8)

before_cite = citation_hit_count()

# chat 问答
print("\nSTEP 3: chat 问答 — RAG 检索 + LLM 推理")
question = "Attune 的向量索引用什么算法？"
print(f"  问: {question}")
t0 = time.time()
st, d = req("POST", "/api/v1/chat", {"message": question}, timeout=120)
chat_ms = (time.time() - t0) * 1000
answer = d.get("content", "")
citations = d.get("citations", [])
print(f"  → {st} ({chat_ms:.0f}ms)")
print(f"  答: {answer[:300]}")
print(f"  citations: {len(citations)} 条")

check("chat 返回 200", st == 200, f"st={st}")
check("chat 有 answer 文本", len(answer) > 0, f"len={len(answer)}")
check("RAG 答案命中知识库内容 (提到 usearch/HNSW)",
      "usearch" in answer.lower() or "hnsw" in answer.lower(),
      "答案应基于上传的文档")
check("chat 返回 citations 引用", len(citations) >= 1, f"{len(citations)} 条")
if citations:
    cited_ids = [c.get("item_id") for c in citations]
    check("citation 指向上传的知识文档", kb_id in cited_ids, f"{cited_ids}")

# citation_hit 信号
time.sleep(1)
after_cite = citation_hit_count()
check("citation_hit 信号写入 skill_signals",
      after_cite > before_cite, f"{before_cite} → {after_cite}")

# 第二问 — 验证多轮
print("\nSTEP 4: chat 第二问 — 加密机制")
st, d = req("POST", "/api/v1/chat", {"message": "Attune 用什么加密算法？"}, timeout=120)
answer2 = d.get("content", "")
print(f"  答: {answer2[:200]}")
check("第二问 RAG 命中加密内容 (AES/Argon2)",
      "aes" in answer2.lower() or "argon" in answer2.lower(), "")

req("DELETE", f"/api/v1/items/{kb_id}")

print(f"\n=== 结果: {PASS} PASS / {FAIL} FAIL ===")
sys.exit(0 if FAIL == 0 else 1)
