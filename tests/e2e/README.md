# Memory Moat E2E 测试套件

打**真实 attune-server-headless 进程**的端到端测试 — 非内存 Store 单元测试。
验证 v0.7 Memory Moat（文档编辑嵌入 + 自学习闭环）在真实 HTTP 链路下的行为。

## 一键运行

```bash
bash tests/e2e/run_all.sh
```

runner 自动：编译 server → 起隔离 server（独立 XDG dir，port 18905）→
setup+unlock vault → 配 cloud/scheduler LLM（若显式配置）→ 顺序跑全部脚本 → 汇总 → 清理。
退出码 0 = 全绿。

长文本知识库门禁需要显式开启：

```bash
ATTUNE_E2E_LONGTEXT=1 ATTUNE_LONGTEXT_PROFILE=edge_scheduler_comprehensive \
  bash tests/e2e/run_all.sh
```

该流程会把 `airplane-manual-collection` 选定 PDF materialize 到
`~/attune-e2e-corpora/airplane-manual-collection`，通过
`POST /api/v1/index/bind` 让 Attune 构建向量库，等待
`pending_embeddings=0`，再跑检索、API 对话评估和 Web UI 评估。对话门禁
要求综合准确率、citation 命中率达标，多轮追问不能跨机型/跨手册漂移，真实飞行
操作请求必须拒答，并且 edge scheduler 30B 级 profile p95 响应延迟不超过 10s；Web UI 门禁
会打开浏览器验证条目页可见、对话框可问、答案/citation
可见，以及 scheduler 状态条在 edge scheduler 路径下渲染。
最终 prompt admission 默认还会受 `ATTUNE_CONTEXT_ADMISSION_MAX_INPUT_TOKENS=65536`
约束；即使云端模型宣称 1M token 窗口，长文本门禁也要求先检索/筛选/压缩出小证据包。

edge scheduler 试点可这样跑；RISC-V/X100 只是首个落地平台，Windows/Linux x86 高性能平台应复用同一入口：

```bash
ATTUNE_E2E_LONGTEXT=1 \
ATTUNE_LONGTEXT_PROFILE=edge_scheduler_comprehensive \
ATTUNE_E2E_LOCAL_SCHEDULER=http://127.0.0.1:8090 \
  bash tests/e2e/run_all.sh
```

`ATTUNE_E2E_LOCAL_SCHEDULER` 会派生本地 scheduler chat 路由和 embedding endpoint，
并默认使用 `llm-summary`、`embedding-int8`、512 维、`/kb/tasks/kb.query.embed`。
配置 scheduler 时，runner 会先执行 `scripts/probe-edge-scheduler-contract.py`；
默认严格要求 schema_versions、prompt cache 元数据和 `scheduler_refusal_v1`。临时兼容旧
scheduler 可设 `ATTUNE_E2E_SCHEDULER_STRICT=0`。
当前 scheduler 生产接口不是
尚未落地的 `/v1/embeddings` thin route；需要改 task 时可设
`ATTUNE_E2E_EMBEDDING_TASK=kb.ingest.embed_batch`。本地 scheduler 长文本门禁默认把
Attune embedding queue batch 和 scheduler-native embedding task batch 设为 512；
两者都可升到 2048，且 scheduler-native provider 会在 scheduler 报 physical
batch limit 时二分重试。长文本门禁默认开启 scheduler OCR 能力发现；普通 e2e 仍默认关闭
OCR，可用 `ATTUNE_SCHEDULER_OCR_ENABLED=0/1` 显式覆盖。PDF page OCR 独立受控并默认开启，
通过页数、总耗时、单页耗时与 DPI 上限保持有界；可显式设置 `ATTUNE_SCHEDULER_PDF_OCR_ENABLED=0` 关闭。
开启后仍有通用保护：`ATTUNE_SCHEDULER_PDF_OCR_MAX_PAGES` 默认 4，
`ATTUNE_SCHEDULER_PDF_OCR_MAX_TOTAL_MS` 默认 12000ms，连续空 OCR 页也计入
`ATTUNE_SCHEDULER_PDF_OCR_MAX_CONSECUTIVE_FAILURES`，到阈值后诚实降级为
metadata-only；如果 scheduler OCR 返回 `unsupported_payload` 等不会随页变化的
fatal payload/schema 错误，Attune 会在第一页后直接停止该 PDF 的 page OCR，而不是
继续扫完整本 PDF。airplane longtext runner 对大语料 `/api/v1/index/bind`
默认使用后台扫描，随后等待 `/api/v1/index/status.background_scans` 进入 `done`
并继续执行 embedding drain、search、chat；如需兼容旧同步语义，可设置
`ATTUNE_LONGTEXT_BIND_BACKGROUND=0`。后台扫描会启用长预算 PDF OCR：文档级默认
无硬截止（`ATTUNE_BACKGROUND_PDF_OCR_MAX_TOTAL_MS=0`；如显式设置则最小提升到
`180000ms`），单页 scheduler async job 默认按 `180000ms`
轮询到终态（`ATTUNE_BACKGROUND_PDF_OCR_PAGE_TIMEOUT_MS`，范围
`30000..180000`）。后台扫描还会把 PDF 页数预算切换为异步 ingest 语义：已知页数
默认尝试全页（`ATTUNE_BACKGROUND_PDF_OCR_MAX_PAGES=0`），未知页数兜底 16 页；
交互/同步路径仍默认 4 页。每个 DPI render 另有 deadline，默认 `30000ms`（
`ATTUNE_BACKGROUND_PDF_OCR_RENDER_TIMEOUT_MS`，范围 `10000..60000`），高 DPI render
超时、输出图像过大，或 scheduler 返回 layout/line limit 类终态错误时，会继续尝试低
DPI 候选，后台路径默认最低降到 24dpi。后台失败页和连续失败阈值默认等于本次 page
limit，只有 background/async 专用环境变量会收紧它们。

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
| `memory_moat_chat_e2e.py` | 9 | legacy direct-Ollama RAG 问答；citation 引用；citation_hit 信号落库（默认 runner 不执行）|
| `airplane_manual_longtext_e2e.py` | gate | 可选长文本 KB E2E；Attune bind 目录建向量库；检索 Hit/Recall/MRR；chat 准确率/citation/10s p95；多轮来源连续性和安全拒答；Web UI 交互 |

长文本多轮 API 子门禁可单独运行：

```bash
python3 scripts/eval-airplane-manual-longtext-multiturn.py \
  --manifest /tmp/attune-airplane-longtext-edge_scheduler_comprehensive.json \
  --base-url http://localhost:18905 \
  --profile edge_scheduler_comprehensive \
  --fail-on-targets
```

`airplane_manual_longtext_e2e.py` 默认会在单轮 chat gate 后调用该脚本。调试纯
search/chat 单轮时可设 `ATTUNE_LONGTEXT_MULTITURN=0`。

生成型 scheduler answer worker 门禁可通过关闭抽取式 fast path 后运行：

```bash
ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER=0 \
ATTUNE_SCHEDULER_CONTEXT_CHUNK_MAX_CHARS=128 \
ATTUNE_SCHEDULER_ASK_MAX_OUTPUT_TOKENS=28 \
ATTUNE_LONGTEXT_REQUIRE_SCHEDULER_GENERATION=1 \
ATTUNE_LONGTEXT_SCHEDULER_GENERATION_P95_MS_MAX=10000 \
python3 tests/e2e/airplane_manual_longtext_e2e.py
```

该模式要求单轮 chat gate 的成功样本全部走 scheduler answer worker
（当前任务名 `kb.query.ask`）生成路径，并在
结果 JSON 中汇总 scheduler 生成 latency、queue wait、cold-start wait，以及
scheduler 返回的 prompt-cache/cache metadata。若 scheduler 已提供 prompt-cache
字段，可额外设置 `ATTUNE_LONGTEXT_REQUIRE_PROMPT_CACHE_METADATA=1` 把它升级为硬门禁。
显式输出上限仍可用于压测简单查询；跨文档/多来源查询会由 Attune 自动抬到
`ATTUNE_SCHEDULER_SOURCE_DIVERSE_MIN_OUTPUT_TOKENS`（默认 40），避免过短输出造成
截断式误判。

解锁后，server 会在后台预热本地 scheduler / scheduler-native 检索链路：
metadata source scan、典型 source lookup query 和 top-k item 解密会先跑一轮，
用于压低重启后首问冷启动延迟。默认只在本地 scheduler 或 scheduler-native
provider 配置下启用；可用 `ATTUNE_RETRIEVAL_WARMUP=0` 关闭，或用
`ATTUNE_RETRIEVAL_WARMUP_QUERIES="source manual reference;来源 手册 引用"`
覆盖预热 query。

长文本 Web UI 子门禁：

```bash
python3 tests/e2e/playwright/airplane_manual_longtext_ui_e2e.py \
  --manifest /tmp/attune-airplane-longtext-edge_scheduler_comprehensive.json \
  --base-url http://localhost:18905 \
  --profile edge_scheduler_comprehensive
```

RISC-V、Windows 或 Linux x86 平台如果 Python Playwright 不可用，使用同语义
Node fallback，并显式指向系统 Chrome/Chromium：

```bash
ATTUNE_LONGTEXT_UI_DRIVER=node \
ATTUNE_PLAYWRIGHT_EXECUTABLE=/usr/bin/chromium \
node tests/e2e/playwright/airplane_manual_longtext_ui_e2e.js \
  --manifest /tmp/attune-airplane-longtext-edge_scheduler_comprehensive.json \
  --base-url http://localhost:18905 \
  --profile edge_scheduler_comprehensive
```

`airplane_manual_longtext_e2e.py` 会在 `ATTUNE_PLAYWRIGHT_EXECUTABLE` 已设置且
`node` 可用时自动选择这个 fallback；也可显式设置
`ATTUNE_LONGTEXT_UI_DRIVER=node`。

`airplane_manual_longtext_e2e.py` 默认会在 API gate 后调用这个脚本。调试纯
API 层时可设 `ATTUNE_LONGTEXT_UI=0`。
Web UI 和 UI e2e 驱动对 scheduler answer job 使用 250ms 轮询，避免 1s+
轮询颗粒度吞掉 10s SLA 的尾部预算；需要压测较慢平台时可通过
`ATTUNE_LONGTEXT_UI_POLL_INTERVAL_MS` 覆盖。

## 有头 Web UI 测试

有头测试用于人工观察真实页面、焦点、后台任务条、scheduler 状态 chip、引用渲染和
长文本回答延迟。它不是 CI 近路；必须满足：

- 使用真实 Chrome/Chromium，设置 `ATTUNE_HEADLESS=0`。
- 有可用图形会话：Linux 需要 `DISPLAY`，Windows/macOS 直接打开系统浏览器。
- 打真实 server URL，不用 mock UI。若验证 release，必须使用真安装包启动的服务。
- 长文本有头测试仍要先完成 API 建库，或直接让 `airplane_manual_longtext_e2e.py`
  全流程触发 Web UI 子门禁。

当前仓库推荐的本地有头 smoke：

```bash
ATTUNE_HEADLESS=0 \
ATTUNE_BASE_URL=http://localhost:18905 \
ATTUNE_PLAYWRIGHT_CHANNEL=chrome \
python3 tests/e2e/playwright/v10_ga_ui_e2e.py
```

长文本有头 UI gate（要求对应 manifest 的 corpus 已经完成 bind/index）：

```bash
ATTUNE_HEADLESS=0 \
ATTUNE_LONGTEXT_UI_POLL_INTERVAL_MS=250 \
python3 tests/e2e/playwright/airplane_manual_longtext_ui_e2e.py \
  --manifest /tmp/attune-airplane-longtext-edge_scheduler_comprehensive.json \
  --base-url http://localhost:18905 \
  --profile edge_scheduler_comprehensive
```

若要完整跑长文本并打开可见浏览器：

```bash
ATTUNE_HEADLESS=0 \
ATTUNE_E2E_LONGTEXT=1 \
ATTUNE_LONGTEXT_PROFILE=edge_scheduler_comprehensive \
ATTUNE_E2E_LOCAL_SCHEDULER=http://127.0.0.1:8090 \
bash tests/e2e/run_all.sh
```

### K3 平台有头测试拓扑

K3 平台测试必须使用 K3 自己的文件系统和存储：

- Attune server 运行在 K3。
- vault.db、vectors、Tantivy、airplane manual corpus、后台 bind 目录都在 K3。
- scheduler 运行在 K3。
- 其它主机只能作为浏览器/Playwright driver 访问 K3 Web URL，不能把前端主机路径传给
  `/api/v1/index/bind`。

当前 K3 已部署服务时，可从其它主机这样跑有头 UI gate：

```bash
# 先在 K3 准备一个小的后台 bind 目录；路径必须是 K3 上的真实路径。
ssh root@192.168.100.233 'rm -rf /root/attune-e2e-home-100233/attune-e2e-corpora/ui-background-bind-k3-headed && \
  mkdir -p /root/attune-e2e-home-100233/attune-e2e-corpora/ui-background-bind-k3-headed && \
  for i in $(seq -w 0 63); do \
    printf "# K3 headed background bind %s\n\nattune-ui-background-indexing-gate\n" "$i" \
      > /root/attune-e2e-home-100233/attune-e2e-corpora/ui-background-bind-k3-headed/k3-ui-$i.md; \
  done'

ATTUNE_HEADLESS=0 \
ATTUNE_BASE_URL=http://192.168.100.233:18945 \
ATTUNE_LONGTEXT_UI_BACKGROUND_BIND_CREATE=0 \
ATTUNE_LONGTEXT_UI_BACKGROUND_BIND_DIR=/root/attune-e2e-home-100233/attune-e2e-corpora/ui-background-bind-k3-headed \
python3 tests/e2e/playwright/airplane_manual_longtext_ui_e2e.py \
  --manifest /tmp/attune-airplane-longtext-100233.json \
  --base-url http://192.168.100.233:18945 \
  --profile local_scheduler_comprehensive
```

如果 K3 当前运行的 Attune 二进制早于 background bind 快速返回改造，可临时加
`ATTUNE_LONGTEXT_UI_BACKGROUND_BIND=0` 做人工/有头 chat 验证；部署最新 Attune
二进制后必须重跑 strict 命令。K3 strict gate 必须包含后台 bind 可见性检查。

如果只人工自测，不跑脚本，浏览器打开 K3 URL 后输入 K3 测试 vault 密码即可；不要在前端主机重新起
`attune-server-headless`。

## 前置依赖

- Rust 工具链（编译 attune-server-headless）
- Python 3（脚本用 stdlib urllib + sqlite3，无第三方依赖）
- `memory_moat_chat_e2e.py` 是 legacy direct-Ollama 脚本，不再属于默认 runner；
  当前标准路径下本地模型应经 scheduler，云模型应经配置的 cloud/OpenAI-compatible
  endpoint。需要验证 chat 能力时优先使用 scheduler/cloud 长文本 gate。
- 长文本 E2E 额外需要：可访问 GitHub，磁盘空间足够 materialize 所选 PDF；
  edge scheduler 模式需 scheduler 在 loopback `:8090` 可用，或通过
  `ATTUNE_E2E_LOCAL_SCHEDULER` 指到远端或本机 Windows/Linux x86 edge scheduler；
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
