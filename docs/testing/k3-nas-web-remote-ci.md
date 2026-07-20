# K3/NAS Web Remote CI Strategy

本文档定义 K3/NAS Web `.deb` 演示和远端 CI 的边界，目标是避免 runner、本机
scheduler、NAS 服务器三者路径/地址混用导致误测。
这些规则是 cross-host CI 的验收边界。

## ATTUNE_K3_LONGTEXT_MANIFEST

`ATTUNE_K3_LONGTEXT_MANIFEST` 指向 CI runner 本地可读的 long-text benchmark JSON。
现有标准样例是 `tests/e2e/airplane_manual_longtext_cases.json`，由
`scripts/build-airplane-manual-longtext-dataset.py` 生成。

必需内容：

- `source_root`: 长文本语料根目录。跑 API longtext E2E 时必须是执行该脚本的机器可见路径。
- `source_root_env`: 可覆盖 `source_root` 的环境变量名，例如 `AIRPLANE_MANUAL_COLLECTION_ROOT`。
- `selection.profiles`: profile 到文档 id 列表的映射，例如 `smoke`、`edge_scheduler_30b`、`edge_scheduler_comprehensive`。
- `documents[]`: 每个文档的 `id`、`file`、`title`、`manufacturer`、`aircraft`、`manual_type`、`index_partition`。
- `queries[]`: 每个评测 query 的 `id`、`query`、`acceptable_hits`、`acceptable_files`、`expect_any`、`expected_behavior`。
- `evaluation_targets.vector_search`: hit/recall/MRR/latency 目标。
- `evaluation_targets.rag_answer`: answer accuracy、citation、unsafe advice、scheduler generation latency 目标。
- `evaluation_targets.context_admission`: 每个 profile 的 context document/chunk 上限。
- `web_e2e`: UI gate 默认 query 和必须出现的 Web surface。

远端 K3 演示中的注意点：

- `ATTUNE_K3_LONGTEXT_MANIFEST` 只给 runner 上的 Playwright/UI gate 读取 query 和目标阈值。
- UI gate 假设 long-text API gate 已经把语料 materialize、bind、drain embedding queue。
- 如果 Attune server 跑在 NAS/K3 上，long-text API gate 必须在 NAS/K3 本机执行，或使用明确的
  NAS/K3 server-side corpus path；不能把 runner 本地路径传给 `/api/v1/index/bind`。

最小 smoke manifest 可直接使用仓库样例并选择 `smoke` profile。完整演示使用
`edge_scheduler_comprehensive`，但必须预留足够磁盘、下载时间和 embedding drain 时间。

## scheduler self-test

scheduler self-test 是 scheduler 包/仓库的责任。Attune 只消费公开 contract 和任务接口。
scheduler 自测至少覆盖：

- package/service: scheduler `.deb` 安装、重启、开机自启、日志、端口监听。
- readiness: `/health`、`/ready`、`/ready?hot=1`。
- public contract: `/benchmark/contract`、`/models`、`/capacity`，包含 schema version、
  model state、context token caps、prompt-cache metadata、refusal policy、jobs API。
- task correctness: `/kb/tasks/kb.query.embed`、`kb.query.rerank`、`kb.query.ask`、
  `kb.document.ocr_recognize`、`kb.meeting.asr_frontend` 以及 TTS/VLM 等已声明任务。
- async jobs: job id、`/jobs/{job_id}` polling、terminal status、cancel、TTL/expired、failed error schema。
- performance owned by scheduler: p50/p95/throughput、queue wait、cold start、cache hit/tokens、
  RVV/IME/EP acceleration metadata。
- fault model: busy/rate-limited/unavailable/transport/invalid-json/job-failed 必须结构化返回，
  不允许假装成功。
- security: private worker endpoints 不对 Attune 暴露；需要 loopback-only 的接口只能通过本机自测或 SSH tunnel 由 CI runner 探测。
- stress: 并发 submit/poll/cancel、长上下文 admission、模型 reload、服务重启恢复。

Attune 侧只运行：

- `scripts/probe-edge-scheduler-contract.py --strict`
- `scripts/release/test-k3-rvv-runtime-gate.sh`
- scheduler repo 的 `tools/worker_benchmark_gate.py`，但从 scheduler checkout 目录执行。

## Remote CI topology

远端 CI 必须显式区分三个地址：

- `ATTUNE_K3_BASE_URL`: CI runner/browser 访问 Attune Web/API 的地址，例如
  `http://192.168.100.233:18900`。
- `ATTUNE_K3_SCHEDULER_URL`: CI runner 访问 scheduler 的地址。若 scheduler 只监听 loopback，
  runner 必须先建立 SSH tunnel，例如 `127.0.0.1:18090 -> 127.0.0.1:8090`。
- `ATTUNE_K3_SERVER_SCHEDULER_BASE`: Attune server 在 NAS/K3 上访问 scheduler 的地址，
  co-located 部署通常是 `http://127.0.0.1:8090`。这个值会写入 `/api/v1/settings`。

路径规则：

- `/api/v1/index/bind` 的 `path` 永远是 Attune server 可见的 NAS/K3 server-side path。
- `--skip-install` 只跳过 deb 安装，不跳过远端 fixture 创建。
- manifest/corpus/API longtext gate 必须和其路径归属一致：runner-local manifest 可以驱动 UI，
  server-side corpus 才能驱动 bind/index。

CI 分层：

- Hosted PR CI: 只跑脚本语法、dry-run、contract report 生成、Rust/TS 单测，不连接真实 K3。
- Hardware nightly CI: 自托管 runner 或 LAN runner 连接 K3，安装 `.deb`，创建 NAS-side fixture，
  跑 NAS Web API contract、scheduler contract、worker correctness、Attune vector/search/chat gates。
- Scheduler CI: 在 scheduler 机器/仓库内从零 E2E，产出 worker/perf/acceleration 报告；Attune CI 不重新归因 scheduler 性能。

## NAS Web API contract

`scripts/release/probe-nas-web-api-contract.py` 是 K3 `.deb` 演示的严格接口门。默认由
`scripts/release/test-k3-nas-web-demo.sh` 在 vault unlock 和 scheduler settings 写入后调用。

默认覆盖：

- health/UI shell: `/health`、`/api/v1/status/health`、`/`。
- vault: status/setup/unlock/token。
- settings/scheduler: `PATCH /api/v1/settings` 写入 `local_scheduler` LLM/embedding，
  `POST /api/v1/llm/probe-edge-scheduler` 验证 Web 设置页探测接口。
- UI backing reads: status、diagnostics、ai-stack、index status、items、background、member、
  privacy、plugins、marketplace、skills、skill-runtime、projects、tags、clusters、jobs、
  folder-links、audit、suggestions、accounts、scenarios、diagnostics capabilities。
- write/download paths: multipart upload、server-side folder bind、search、export CSV。
- chat: `/api/v1/chat`，需要 scheduler 时校验 `local_scheduler` metadata 并 polling job 到 terminal success。

报告根对象包含 `scheduler_observations`。这里集中暴露 Attune 在 scheduler 接入链路上观测到的
不稳定因素，例如 discovery 失败、scheduler-backed chat/job 失败、job 缺少 latency/queue telemetry，
以及实际 job latency/queue_wait 样本。这里不设置 scheduler 性能阈值；阈值归 scheduler CI。

不纳入默认强制门的接口：

- 需要真实第三方凭据或外部服务的 Email/WebDAV/RSS/cloud account 登录。
- 需要付费会员态或插件包真实安装的高级 agent/plugin 行为。
- scheduler 私有 worker endpoints；这些属于 scheduler self-test。
