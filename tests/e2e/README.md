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

长文本知识库门禁需要显式开启：

```bash
ATTUNE_E2E_LONGTEXT=1 ATTUNE_LONGTEXT_PROFILE=local_scheduler_comprehensive \
  bash tests/e2e/run_all.sh
```

该流程会把 `airplane-manual-collection` 选定 PDF materialize 到
`~/attune-e2e-corpora/airplane-manual-collection`，通过
`POST /api/v1/index/bind` 让 Attune 构建向量库，等待
`pending_embeddings=0`，再跑检索、API 对话评估和 Web UI 评估。对话门禁
要求综合准确率、citation 命中率达标，多轮追问不能跨机型/跨手册漂移，真实飞行
操作请求必须拒答，并且 local scheduler 30B p95 响应延迟不超过 10s；Web UI 门禁
会打开浏览器验证条目页可见、对话框可问、答案/citation
可见，以及 本地调度器状态条在本地 local scheduler 路径下渲染。

本地 scheduler 试点可这样跑；local scheduler 是首个 profile，Windows/Linux x86 高性能平台应复用同一入口：

```bash
ATTUNE_E2E_LONGTEXT=1 \
ATTUNE_LONGTEXT_PROFILE=local_scheduler_comprehensive \
ATTUNE_E2E_LOCAL_SCHEDULER=http://127.0.0.1:8090 \
  bash tests/e2e/run_all.sh
```

`ATTUNE_E2E_LOCAL_SCHEDULER` 会派生本地 scheduler chat 路由和 embedding endpoint，
并默认使用 `llm-summary`、`embedding-int8`、512 维、`/kb/tasks/kb.query.embed`。
当前 scheduler 生产接口不是
尚未落地的 `/v1/embeddings` thin route；需要改 task 时可设
`ATTUNE_E2E_EMBEDDING_TASK=kb.ingest.embed_batch`。大体量 OCR/解析会让同步 bind
长时间占用，可用 `ATTUNE_LONGTEXT_BIND_TIMEOUT_SEC` 提高超时。

云端或其它 OpenAI-compatible LLM 可通过 runner 环境变量注入：

```bash
ATTUNE_E2E_LLM_ENDPOINT=https://example.com/v1 \
ATTUNE_E2E_LLM_MODEL=your-model \
ATTUNE_E2E_LLM_API_KEY="$API_KEY" \
ATTUNE_E2E_LONGTEXT=1 \
  bash tests/e2e/run_all.sh
```

## 脚本清单（基础 9 脚本 / 90 断言，另有可选长文本 gate）

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
| `airplane_manual_longtext_e2e.py` | gate | 可选长文本 KB E2E；Attune bind 目录建向量库；检索 Hit/Recall/MRR；chat 准确率/citation/10s p95；多轮来源连续性和安全拒答；Web UI 交互 |

长文本多轮 API 子门禁可单独运行：

```bash
python3 scripts/eval-airplane-manual-longtext-multiturn.py \
  --manifest /tmp/attune-airplane-longtext-local_scheduler_comprehensive.json \
  --base-url http://localhost:18905 \
  --profile local_scheduler_comprehensive \
  --fail-on-targets
```

`airplane_manual_longtext_e2e.py` 默认会在单轮 chat gate 后调用该脚本。调试纯
search/chat 单轮时可设 `ATTUNE_LONGTEXT_MULTITURN=0`。

生成型 scheduler answer worker 门禁可通过关闭抽取式 fast path 后运行：

```bash
ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER=0 \
ATTUNE_LONGTEXT_REQUIRE_SCHEDULER_GENERATION=1 \
ATTUNE_LONGTEXT_SCHEDULER_GENERATION_P95_MS_MAX=10000 \
python3 tests/e2e/airplane_manual_longtext_e2e.py
```

该模式要求单轮 chat gate 的成功样本全部走 scheduler answer worker
（当前任务名 `kb.query.ask`）生成路径，并在
结果 JSON 中汇总 scheduler 生成 latency、queue wait、cold-start wait，以及
scheduler 返回的 prompt-cache/cache metadata。若 scheduler 已提供 prompt-cache
字段，可额外设置 `ATTUNE_LONGTEXT_REQUIRE_PROMPT_CACHE_METADATA=1` 把它升级为硬门禁。

解锁后，server 会在后台预热本地 scheduler / scheduler-native 检索链路：
metadata source scan、典型 source lookup query 和 top-k item 解密会先跑一轮，
用于压低重启后首问冷启动延迟。默认只在本地 scheduler 或 scheduler-native
provider 配置下启用；可用 `ATTUNE_RETRIEVAL_WARMUP=0` 关闭，或用
`ATTUNE_RETRIEVAL_WARMUP_QUERIES="source manual reference;来源 手册 引用"`
覆盖预热 query。

长文本 Web UI 子门禁：

```bash
python3 tests/e2e/playwright/airplane_manual_longtext_ui_e2e.py \
  --manifest /tmp/attune-airplane-longtext-local_scheduler_comprehensive.json \
  --base-url http://localhost:18905 \
  --profile local_scheduler_comprehensive
```

RISC-V、Windows 或 Linux x86 平台如果 Python Playwright 不可用，使用同语义
Node fallback，并显式指向系统 Chrome/Chromium：

```bash
ATTUNE_LONGTEXT_UI_DRIVER=node \
ATTUNE_PLAYWRIGHT_EXECUTABLE=/usr/bin/chromium \
node tests/e2e/playwright/airplane_manual_longtext_ui_e2e.js \
  --manifest /tmp/attune-airplane-longtext-local_scheduler_comprehensive.json \
  --base-url http://localhost:18905 \
  --profile local_scheduler_comprehensive
```

`airplane_manual_longtext_e2e.py` 会在 `ATTUNE_PLAYWRIGHT_EXECUTABLE` 已设置且
`node` 可用时自动选择这个 fallback；也可显式设置
`ATTUNE_LONGTEXT_UI_DRIVER=node`。

`airplane_manual_longtext_e2e.py` 默认会在 API gate 后调用这个脚本。调试纯
API 层时可设 `ATTUNE_LONGTEXT_UI=0`。

## 前置依赖

- Rust 工具链（编译 attune-server-headless）
- Python 3（脚本用 stdlib urllib + sqlite3，无第三方依赖）
- chat E2E 额外需要：Ollama 运行 + 已 pull `qwen2.5:3b` + `bge-m3`
  （无 Ollama 时 runner 自动跳过 chat E2E）
- 长文本 E2E 额外需要：可访问 GitHub，磁盘空间足够 materialize 所选 PDF；
  本地 scheduler 模式需 `local-scheduler` 在 loopback `:8090` 可用，或通过
  `ATTUNE_E2E_LOCAL_SCHEDULER` 指到远端或本机 Windows/Linux x86 本地 scheduler；
  云端 LLM 可通过
  `ATTUNE_E2E_LLM_*` 注入。Web UI 子门禁需要
  Python Playwright 和 Chrome/Chromium；或 Node.js、`node-playwright`/Playwright
  package，以及可通过 `ATTUNE_PLAYWRIGHT_EXECUTABLE` 指定的系统浏览器。

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
