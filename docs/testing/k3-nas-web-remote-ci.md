# K3/NAS Web Remote CI Strategy

本文档定义 K3/NAS Web `.deb` 演示和远端 CI 的边界，目标是避免 runner、本机
scheduler、NAS 服务器三者路径/地址混用导致误测。
这些规则是 cross-host CI 的验收边界。

## ATTUNE_K3_LONGTEXT_MANIFEST

标准 long-text 数据源是两个并列 GitHub 仓库：

- airplane manual corpus:
  `https://github.com/shiroinekotfs/airplane-manual-collection.git`，固定 commit
  `afe8288495338880e165f77bb9afe9946f366a52`。
- mechanical design handbook corpus:
  `https://github.com/GEQfa/handbook-of-mechanical-design.git`，固定 commit
  `86832fd643cb1f9cfa1188d242d34b62dd52e41f`。

`scripts/build-airplane-manual-longtext-dataset.py` 和
`scripts/build-mechanical-design-longtext-dataset.py` 分别生成 manifest；默认完整演示
profile 是 `edge_scheduler_comprehensive`。mechanical design 仓库的 PDF 是 Git LFS
对象，NAS host 必须安装并启用 `git-lfs`，否则 builder 会拒绝把 133 字节 LFS 指针当
PDF 入库。

`ATTUNE_K3_LONGTEXT_E2E=1` 会在 NAS host 本机运行
`tests/e2e/longtext_corpora_e2e.py`，默认覆盖 `ATTUNE_K3_LONGTEXT_CORPORA=airplane,mechanical_design`。
每个 corpus 都会 materialize、server-side bind、embedding/vector drain、search、
chat、multiturn。随后 `scripts/eval-longtext-corpora-suite.py` 会按
`ATTUNE_K3_LONGTEXT_REPEAT_CHAT`（默认 3）重复单轮 chat 和多轮 chat，输出稳定性与性能汇总。
默认只把 scheduler generation coverage 作为报告项暴露，不作为 Attune long-text
阻断门；如果需要把每个非安全类 chat query 都强制要求走 scheduler 生成，显式设置
`ATTUNE_K3_LONGTEXT_REQUIRE_SCHEDULER_GENERATION=1`。

long-text gate 默认保留 server-side PDF OCR
（`ATTUNE_K3_LONGTEXT_PDF_OCR=1`），用于同时覆盖 Attune 的 bind、OCR 摄入、
向量生成、检索、grounded chat、多轮和 UI。只想隔离向量/search/chat 行为时，
显式设置 `ATTUNE_K3_LONGTEXT_PDF_OCR=0`；脚本会在运行 corpus bind 前重启 NAS
侧 `attune-server` 并禁用 server-side PDF OCR。OCR 正确性/性能仍应同时由 scheduler
的 `kb.document.ocr_recognize` self-test 和 Attune OCR 专项门归档。

`ATTUNE_K3_LONGTEXT_MANIFEST` 指向 CI runner 本地可读的 long-text benchmark JSON，
用于可选 UI gate。现有标准样例是 `tests/e2e/airplane_manual_longtext_cases.json` 和
`tests/e2e/mechanical_design_longtext_cases.json`。UI gate 默认仍使用 airplane manifest；
mechanical design 的 API/search/chat/multiturn 和 repeat-suite 数据是强制 API gate。
如果先跑了 `ATTUNE_K3_LONGTEXT_E2E=1`，K3 脚本会把 NAS 生成的 manifest 拷回
runner 并复用它做 UI gate。

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
- `multiturn`: 可选的 corpus-specific 多轮 chat 配置；mechanical design 用它定义中文追问、
  来源连续性和 forbidden source 漂移词。

远端 K3 演示中的注意点：

- `ATTUNE_K3_LONGTEXT_E2E=1` 的 API 建库和评测必须在 NAS/K3 本机执行。
- `ATTUNE_K3_LONGTEXT_CORPORA` 默认是 `airplane,mechanical_design`；只隔离单 corpus
  调试时才收窄为 `airplane` 或 `mechanical_design`。
- `ATTUNE_K3_MECHANICAL_DESIGN_LONGTEXT_CORPUS_DIR` 是 NAS/K3 上的 mechanical design
  Git LFS corpus 目录，默认 `$REMOTE_TMP/handbook-of-mechanical-design`。
- `ATTUNE_K3_LONGTEXT_REPEAT_CHAT` 默认 3，用于稳定性数据；低于 3 只适合 smoke，不适合
  作为演示性能结论。
- `ATTUNE_K3_LONGTEXT_MANIFEST` 只给 runner 上的 Playwright/UI gate 读取 query 和目标阈值。
- UI gate 假设 long-text API gate 已经把语料 materialize、bind、drain embedding queue。
- scheduler 生成覆盖率不归 Attune 默认阻断；Attune 默认阻断 search、chat grounding/safety、
  answer budget、multiturn 和 UI。
- `ATTUNE_K3_LONGTEXT_PDF_OCR=1` 是默认综合策略；设置为 `0` 才是显式 CI 隔离策略。
  开启 OCR 时要把 PDF OCR 卡顿、scheduler OCR timeout、layout/line-limit strip fallback
  和 metadata-only degradation 单独归档，不和向量/search/chat 性能混算。scheduler
  transport/unavailable 或重启导致的 job-status miss 应先由 Attune 在单页长预算内同页重试；
  若仍失败，需要在报告中作为 scheduler 可用性因素暴露，而不是归因为文档不可 OCR。
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
  首次绑定或更新 file type/corpus 配置用 `bind`；已绑定目录的快速刷新用
  `POST /api/v1/index/rescan {"dir_id": "..."}`，返回 `scan.deleted` 以显式暴露本地删除收敛。
- `--skip-install` 只跳过 deb 安装，不跳过远端 fixture 创建。
- manifest/corpus/API longtext gate 必须和其路径归属一致：runner-local manifest 可以驱动 UI，
  server-side corpus 才能驱动 bind/index。

CI 分层：

- Hosted PR CI: 只跑脚本语法、dry-run、contract report 生成、Rust/TS 单测，不连接真实 K3。
- Hardware nightly CI: 自托管 runner 或 LAN runner 连接 K3，安装 `.deb`，创建 NAS-side fixture，
  跑 NAS Web API contract、scheduler contract、worker correctness、Attune vector/search/chat gates。
- Scheduler CI: 在 scheduler 机器/仓库内从零 E2E，产出 worker/perf/acceleration 报告；Attune CI 不重新归因 scheduler 性能。

## Attune RAG eval framework

Attune RAG eval is the manifest-driven acceptance layer for industry scenarios,
multi-document scale, Chat RAG, Summary RAG, and failure attribution. The
operator guide is `docs/testing/attune-rag-evaluation-framework.md`.

Local/PR smoke:

```bash
bash scripts/test-pyramid.sh --with-eval-smoke
```

This runs `scripts/eval/validate-manifests.py --suite pr_rag_smoke` and
`scripts/eval/run-suite.py --suite pr_rag_smoke --dry-run`; it does not connect
to K3.

K3 release smoke suite id:

```bash
k3_rag_release_smoke
```

K3 `.deb` release script integration:

```bash
ATTUNE_K3_EVAL_SUITE=k3_rag_release_smoke \
ATTUNE_K3_EVAL_OUT=reports/release/k3-rag-release-smoke.json \
bash scripts/release/test-k3-nas-web-demo.sh \
  --deb dist/release/riscv64-server-deb/attune-server_<version>_riscv64.deb
```

If `ATTUNE_K3_EVAL_SUITE` is unset, the K3 release script keeps the existing
NAS Web API, scheduler, long-text, and optional UI gates only. If it is set, the
manifest-driven RAG eval suite runs after the NAS Web API contract and before
the script-local bind/chat smoke; failures block the `.deb` validation.

Scale suite ids:

```bash
k3_rag_scale_thousand
k3_rag_scale_ten_thousand
```

Both scale suites are single-industry security gates. They must not use
`mixed_enterprise` or cross-industry documents to satisfy the thousand /
ten-thousand document count. Mixed corpora are reserved for source-drift and
routing专项 only.

`kb-web-demo` is the standard frontend simulation surface. Its contract and
live browser runner cover upload, vector chunk display, Chat RAG, Summary RAG,
citations, and timing display:

```bash
bash tests/scripts/eval_web_demo_frontend_contract_test.sh
python3 tests/e2e/playwright/kb_web_demo_eval_frontend_e2e.py \
  --base-url http://<nas-ip>:8890 \
  --api-url http://<nas-ip>:8889 \
  --out reports/release/kb-web-demo-frontend.json
```

The generic runner currently supports generated Markdown corpora directly and
polls async scheduler jobs surfaced by `/api/v1/chat`. Legacy airplane and
mechanical-design long-text gates remain covered by `ATTUNE_K3_LONGTEXT_E2E=1`
until their full live ingestion path is migrated to the generic eval runner.

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
- write/download paths: multipart upload、server-side folder bind/rescan、search、
  embedding/vector queue drain、export CSV。
- chat: `/api/v1/chat`，需要 scheduler 时校验 `local_scheduler` metadata 并 polling job 到 terminal success。

报告根对象包含 `scheduler_observations`。这里集中暴露 Attune 在 scheduler 接入链路上观测到的
不稳定因素，例如 discovery 失败、scheduler-backed chat/job 失败、job 缺少 latency/queue telemetry，
以及实际 job latency/queue_wait 样本。这里不设置 scheduler 性能阈值；阈值归 scheduler CI。

不纳入默认强制门的接口：

- 需要真实第三方凭据或外部服务的 Email/WebDAV/RSS/cloud account 登录。
- 需要付费会员态或插件包真实安装的高级 agent/plugin 行为。
- scheduler 私有 worker endpoints；这些属于 scheduler self-test。
