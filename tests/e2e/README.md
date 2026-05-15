# Memory Moat E2E 测试套件

打**真实 attune-server-headless 进程**的端到端测试 — 非内存 Store 单元测试。
验证 v0.7 Memory Moat（文档编辑嵌入 + 自学习闭环）在真实 HTTP 链路下的行为。

## 一键运行

```bash
bash tests/e2e/run_all.sh
```

runner 自动：编译 server → 起隔离 server（独立 XDG dir，port 18905）→
setup+unlock vault → 配 LLM（若 Ollama 可用）→ 顺序跑全部脚本 → 汇总 → 清理。
退出码 0 = 全绿。

## 脚本清单（9 脚本 / 90 断言）

| 脚本 | 断言 | 覆盖 |
|------|-----|------|
| `memory_moat_e2e.py` | 9 | upload→search→编辑→search→删除全链路；编辑后旧词消失（Phase A 核心承诺）；content_hash 短路；upload dedup |
| `memory_moat_signals_e2e.py` | 9 | 5 类自学习信号真落库（doc_create/update/delete + annotation_marker）；8 线程并发 PATCH 无数据竞争 |
| `memory_moat_stress_e2e.py` | 11 | 100KB 大文档 reindex；无换行长行/code fence 不平衡/多语言 emoji 边界文档不 panic |
| `memory_moat_fault_e2e.py` | 13 | 空内容/超长/坏 JSON/404 优雅拒绝；vault lock 中途操作 403；3.5MB content PATCH（body limit 验证）|
| `memory_moat_annotation_e2e.py` | 15 | annotation CRUD；source 状态契约（人工编辑后仍 user）；annotation_marker 信号 create/update/delete 全覆盖；级联删除 |
| `memory_moat_v07routes_e2e.py` | 11 | v0.7 新路由 demo/load（+幂等）、audit/log、audit/log.csv、chat/stream（SSE + 超长拒绝）|
| `memory_moat_search_quality_e2e.py` | 8 | RRF 混合检索召回质量 — 6 主题语料 + 针对性 query top-1 命中 + 跨主题区分度 |
| `memory_moat_stress_loop_e2e.py` | 5 | 120 轮持续操作（600 HTTP 调用）；RSS/FD 监控验证无内存/句柄泄漏 |
| `memory_moat_chat_e2e.py` | 9 | 真实 Ollama qwen2.5:3b RAG 问答；citation 引用；citation_hit 信号落库（需 Ollama）|

## 前置依赖

- Rust 工具链（编译 attune-server-headless）
- Python 3（脚本用 stdlib urllib + sqlite3，无第三方依赖）
- chat E2E 额外需要：Ollama 运行 + 已 pull `qwen2.5:3b` + `bge-m3`
  （无 Ollama 时 runner 自动跳过 chat E2E）

## 单独运行某脚本

```bash
# 1. 起隔离 server
XDG_DATA_HOME=/tmp/attune-e2e/data XDG_CONFIG_HOME=/tmp/attune-e2e/config \
  rust/target/release/attune-server-headless --no-auth --port 18905 &
# 2. setup vault（密码 e2e-pass-2026）— 见各脚本头部说明
# 3. 跑脚本
python3 tests/e2e/memory_moat_e2e.py
```

各脚本顶部 docstring 有独立的前置说明与期望结果。

## 历史价值

R10 滚动 review 用本套件捕获了 2 个静态 code review 漏掉的 bug：
- `search_cache` 编辑/删除后不失效（P0）
- S3 embed worker 异步竞态写 stale 向量（P1）

R10-G 滚动 review 进一步捕获：ws/scan-progress vault_guard 403、
PATCH body limit 死代码、E2E flaky 断言（RRF 向量语义分量）。

真实场景测试是静态分析无法替代的 —— cache 失效、异步竞态、UI 交互问题
必须真跑才能暴露。
