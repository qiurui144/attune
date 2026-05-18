# attune 版本记录

## v0.7.0-dev (2026-05-18 sprint) — Bug fixes + 多层记忆系统（token 降本）

### Bug fixes

- **访问日志 token 脱敏**（安全）：`/ws/scan-progress?token=<session>` 的会话 token 原本
  以明文写入访问日志。现在日志中间件在输出前对查询参数 `token` 进行 `<redacted>` 替换，
  其他参数保持不变。(`middleware.rs`)

- **WebSocket 扫描进度连接修复**：`/ws/scan-progress` 握手因缺少 `?token=` 查询参数
  而 401 失败（WebSocket 握手不支持 `Authorization` 头）。现在 `ws.ts` 从
  `sessionStorage` 读取 token 并拼入 URL；无 token 时（vault 未解锁）跳过连接、等
  解锁后 `handleUnlock` / `handleWizardComplete` 重新调用 `startProgressWS()`。
  (`ws.ts`, `App.tsx`)

- **Settings → 关于 tab 网络搜索状态修复（outcome a）**：`/api/v1/ai_stack` 响应缺
  少 `web_search` 字段，导致前端 `stackStatus('web_search').ok` 恒为 `false`、始终
  显示"未就绪"。现已添加 `web_search.available` 字段，与 chat 路由实际使用的
  `state.web_search` Arc 一致：系统检测到 Chrome/Edge 则 `true`，否则附上安装提示。
  (`routes/ai_stack.rs`)

## v0.7.0-dev (2026-05-18 sprint) — 多层记忆系统（token 降本）

为 OSS attune-core 加多层记忆架构，让 chat 上下文装配按 query 形态发对的层、对的
粒度，而非永远 dump 原始 chunk。设计稿
`docs/superpowers/plans/2026-05-18-multilayer-memory.md`。

| 阶段 | 改动 |
|------|------|
| 数据模型 | `memories` 新增 `topic_key`/`cold`/`superseded_by`（幂等 ALTER 升级老 vault）；新表 `memory_vectors` —— embedding sidecar 让 L2/L3 摘要可向量检索 |
| L3 语义层 | `memory/semantic.rs` —— 把 episodic（L2）按主题 hdbscan 聚类，每簇 1 次 LLM 总结成 standing "用户对 X 的认知"。`topic_key` 幂等，refresh 时旧 subset 主题 supersede |
| 检索 | `memory/retrieval.rs` —— `MemoryVectorIndex`（专用 usearch 索引）+ `search_memories`：embed query → 排序 live 记忆 → 时间窗口过滤、冷记忆排除 |
| Tier-aware 装配 | `memory/assembler.rs` —— `classify_query_shape`（recall/overview/precise 零 LLM 启发式）+ `assemble_context`：recall→L2 / overview→L3 / precise→L0，coverage gate 保证命中弱即退回 L0，无回归 |
| 历史压缩 | `compact_history` —— 超窗的旧对话轮次不再静默丢弃，滚动摘要成 1 条并按 `sha256(dropped)` 缓存（找回原本丢失的信息 + 降 token） |
| worker | `start_memory_consolidator` 在 episodic pass 后跑分层：embed L2/L3 → L2→L3 语义周期 → 冷降级（纯 SQL） |

**成本契约**：建库不变（tier 1-2）；L2/L3 摘要 tier 3 + 配额治理；冷降级 tier 0；
读路径只选已建好的记忆、不触发 LLM。

**实测**（`memory_token_reduction_benchmark`）：recall+overview 子集注入 token 中位降幅
**78.7%**，precise 子集 0% 变化（precise 永不离开 L0）。

测试：46 unit + 5 集成（`multilayer_memory_integration`）+ 1 benchmark，全绿；
`cargo test -p attune-core` 28 套件 0 失败。

## v0.7.0-dev (2026-05-16 sprint) — law-pro 接入 + 证据可溯源强化

attune-pro 的 law-pro 律师插件接入 attune 主程序并端到端验证（Playwright 真 Chrome
37 元素 + 复杂证据链金额计算 12 断言全绿；本机 + AMD 部署机双环境）。围绕
「证据可溯源 / 抽取准确度 / 上下文预算 / 隐私」做 4 批次强化。

### 批次 1 — 证据可溯源地基

| 子项 | 改动 |
|------|------|
| A1 原始证据留存 | 新 `item_blobs` 表（AES-GCM 加密存上传原件）+ `GET /api/v1/items/{id}/original` 取回路由 —— 律师可回看原始扫描件核对 OCR。软删除时连坐清理（防"忘记"后原件残留）。 |
| B2 OCR 置信度 | `OcrOutput.avg_confidence`（长度加权 text_score）—— 下游判断证据 OCR 是否可信 |
| C1/C2 grounded 抽取 | law-pro `fact_extractor` —— LLM 抽取每字段强制附原文 quote，`verify_grounding` 校验 quote∈原文，幻觉 quote 作废为 null（"无依据不出数字"契约） |

### 批次 2 — 上下文预算管理器

`attune-core::context_budget` —— 按 LLM 模型名查上下文窗口（qwen 32K / gemini 1M /
claude 200K…），四段（system/知识/历史/消息）总账分配。替代写死的
`INJECTION_BUDGET=2000` / `MAX_HISTORY_DEPTH=20`，接入 ChatEngine + `/chat` 路由；
历史超窗按窗口裁剪并插省略说明。

### 批次 3 — 抽取准确度度量框架

law-pro `fact_extractor::accuracy` —— per-field 对/错/漏/多报 → precision/recall，
对照人工真值。优化抽取前先有度量基线。

### 批次 4 — 计算正确性 + 隐私

| 子项 | 改动 |
|------|------|
| D2 LPR date-aware | `interest_calculator` LPR 4 倍司法保护上限按起息日查历史 LPR 表，替代写死 0.138；`lpr_capped` 上限按 `rate_type` 换算到同周期 |
| F1 敏感案件本地 LLM | `LlmProvider::is_local()` + `/chat` 守卫：开启「强制本地」且注入证据时拦截云端 LLM（含压缩段 `summary_llm` 旁路） |

### 前端 — 变体 A · agent 结果面板

`ui/src/components/AgentResultPanel.tsx` —— 通用 agent 结果面板：基础事实值默认显示、
依据默认收起可展开（凭据卡片 + 多依据冲突横幅 + 来源标签 + 修正表单）、完整度计数器、
计算阻断态。接入 Drawer 系统。

### OCR / 前端修复

- `crypto.randomUUID` 仅安全上下文可用 → 新 `genId()` 降级，修复非安全上下文
  （LAN IP 明文 HTTP）下前端「启动失败」
- OCR EXIF orientation 归一（手机照片自动摆正再 OCR）+ `max_side_len` 改 `OcrProfile`
  可配（合同/流水 ≥3200 保留小字细节）

### 验证

- 单测：attune-core **908/0** · law-pro **41/0**
- 代码审查 2 轮，修复 4 bug（软删除孤儿 blob / 已删 item 原件可取回 / F1 压缩段旁路 /
  lpr_capped 年化上限 vs 周期利率单位不一致）
- E2E：复杂证据链 **12/0** · Playwright UI **37/0**

### Marketplace 安装链路补完（2026-05-17）

law-pro 经「pluginhub 发布 → attune Marketplace 下载」全产品路径接入并端到端验证。

| 子项 | 改动 |
|------|------|
| Marketplace 真实安装 | `marketplace::install_plugin` 原仅返回元数据（v0.7 半成品）→ 补完为真实下载落地：`hub.install_plugin` → `hub.download_plugin` → 解压验载落地 `plugins/<id>/`。新增 `plugin_sync::install_plugin_package`（白名单 id 校验 / staging+rename 原子替换）。新插件经一次重启由 registry 装载（B 方案）。 |
| 跨平台解压 | `extract_tarball` gzip 走纯 Rust `tar`+`flate2`（Windows P0 不依赖系统 tar），其余格式回退系统 tar。新增 `tar` / `flate2` 依赖。 |
| 自部署 hub license | 设置「自部署 cloud 后端」表单补 `pluginhub license key` 输入框 —— 自部署 pluginhub 需 url + license_key 两者齐全才切到 `HttpPluginHubProvider`。 |

- 单测：`plugin_sync` **11/0**（含 `install_plugin_package` 落地 / 路径穿越 / id 不匹配 / 覆盖安装 4 例）
- 代码审查 2 轮，修复 6 项（路径穿越白名单 / tar shell-out 跨平台 / 覆盖安装原子性 / magic 短读 / staging 泄漏 / 测试辅助依赖系统 tar）
- E2E：AMD cloud（pluginhub 真实发布）→ attune Marketplace 下载安装 law-pro → civil_loan_agent
  端到端；4 组证据链经前端 civil_loan 表单对账（标准 ¥19,200 / golden ¥24,065.75 /
  砍头息 ¥207,123.29 / 利率红线 LPR 封顶 ¥469,139.73）
- 全量前端套件 `tests/e2e/playwright/run_ui_all.sh`（真 Chrome，L0 Wizard ~ L5 law-pro）**45/0**

## v0.7.0-dev (2026-05-15 sprint) — 安全有效记忆护城河 Phase A+B

> **「优势不在于模型，而在于以安全有效的记忆」**（per 用户决策 2026-05-15）。
> 同样的 LLM，挂上 attune 比单跑模型答得更准 — 因为记忆是私有的、可审计的、随用户使用持续变好的。

### Phase A — 文档编辑嵌入功能完全有效（修 3 个 release-blocker）

之前各 update path 各写一份"删旧加新"流程，留下 3 个生产 bug：

| Bug (≤ v0.6.3) | 修法 (v0.7) |
|----------------|-------------|
| `routes/items.rs::update_item` 完全不 re-embed → UI 编辑后 search 永远返回旧内容 | UpdateOutcome 三态 + `reindex::reindex_item` 完整 pipeline |
| `routes/upload.rs` 同名重传不去重 | content_hash dedup 短路 |
| `routes/items.rs::delete_item` 不调 `vectors::delete_by_item_id` + `fulltext::delete_document` (这两个函数已实现但 0 处调用，**死代码**) | `reindex::purge_item_indexes` 先清后软删 |
| `scanner.rs` / `scanner_webdav.rs` 文件变更触发 `store.delete_item` 但拿不到 vectors lock | `reindex_queue` 表 + server `start_reindex_worker` 3s 轮询消费 |

**核心架构新增**：
- `attune-core::reindex` 模块 — 协调 store + vectors + fulltext + queue **事务式** cleanup
- `items.content_hash` 列（SHA-256 hex）+ migration + index — 短路条件
- `reindex_queue` 表 — defer 跨层 worker 的清理职责
- `AppState::start_reindex_worker` — 后台 3s 轮询 worker，vault unlock 时启动

### Phase B — 自学习闭环 3 hook

之前 skill_evolution 仅消费"搜索失败"信号（最低级），批注 / citation / 文档变更 / hit count / feedback 全部 NO。本 sprint 把 5 类信号汇聚到 `skill_signals` 表（kind 列区分）：

| Hook | kind | 写入位点 | 意义 |
|------|------|----------|------|
| 1 | `doc_create` | upload.rs | 新文档进入 → 喂入 search 词库 |
| 1 | `doc_update` | items.rs::update + scanner.rs | 内容改变 → 重新评估同义词 |
| 1 | `doc_delete` | items.rs::delete | 文档移除 → 清理过期词 |
| 2 | `citation_hit` | chat.rs (top-5) | chunk 被 LLM 引用 → **高质量信号**，扩展词学习时优先保留语义 |
| 3 | `annotation_marker` | annotations.rs | 用户标 ⭐ 重点 / 🤔 存疑 → 偏好信号 |

schema：`skill_signals` 加 `kind` + `ref_id` 列 + migration + composite index
API：`Store::record_signal_event(kind, ref_id, query_opt)` + `count_unprocessed_signals_by_kind`
向后兼容：老 `record_skill_signal` 内部固定 `kind='search_miss'`，不破坏现有 evolver

### Phase C — spec only（v0.7 后续 sprint）

`docs/specs/memory-moat-v07.md` — RICE 排序 5 项：C1 文档版本化记忆 / C2 编辑触发自动重标注 / C3 失败信号反推 project_recommender / C4 知识衰减曲线 / C5 embed_model_version 迁移工具链

### v0.7 sprint 1（5 agents 并行）— commit 71d82ee

- **attune-core**：cost / tools / demo / query_rewrite / entity_graph / skill_eval / report / reader / capture / sync / vlm / store::audit_log
- **attune-server** 路由：/audit/log + /audit/log.csv + /demo/load + /chat/stream
- 修 `parse_this_month_english` 测试硬编码常量错算 4 天 bug

### 30 轮 sprint + R1-R9 滚动 review（静态审查 + 单元测试）

W1-W4 30 轮 + R1-R9 滚动深度审计修 1 Critical + 5 P0 + 14 P1。详见
`docs/specs/memory-moat-v07.md` §6.5 / §6.6。

### Round A-H 真实场景 E2E（编译真实 server，全程 HTTP）

转向真实运行场景测试 — `tests/e2e/` 9 脚本 90 断言（见 `tests/e2e/README.md`）：

- Round A chat RAG / B Playwright UI / C 回归 / D 故障注入 + crash recovery /
  E annotation CRUD / F 持续压力泄漏监控 / G 套件 runner
- 真实测试净抓 4 个静态 review 遗漏的 bug：
  search_cache 失效 P0 / S3 embed worker 竞态 P1 / ws/scan-progress 403 P1 /
  PATCH body limit 死代码 P1
- `bash tests/e2e/run_all.sh` 一键跑全套，实测 **90 断言全绿**

### 验证

- workspace lib tests: **919 passed / 0 failed / 1 ignored**（10 cli + 893 core + 16 server）
- integration tests: memory_moat_integration **14 passed**
- E2E 套件: 9 脚本 **90 断言全绿**（含真实 Ollama RAG / Playwright UI / crash recovery）
- perf 实测（release）: 100KB reindex 834ms / 500KB 1.95s / 100KB upload ~1.1s
- `python/tests/MANUAL_TEST_CHECKLIST.md` 含 8 条 Memory Moat 验收

### Commits (cumulative, 21 commits)

- `71d82ee` feat(v07): 15 P0 缺口模块批量落地
- `50d994b` feat(memory-moat): v0.7 Phase A+B
- `6c6ce71`..`f022f56` W1-W4 30 轮 sprint（文档 / 代码 review / logic audit / 测试）
- `9358c02`..`bb2e2ee` R1-R9 滚动 review（perf / 安全 / 错误泄露 / 资源 / 兼容 / 死链）
- `82cd79d`..`2159d98` R10 + Round A-N 真实场景 E2E（抓 4 bug + 9 脚本 90 断言套件）

---

## v0.6.4 dev (post-GA) — 30 轮深度知识库 + 代码文档评阅 sprint (2026-05-15)

发布定位: **post-v0.6.3 GA 内功** — 知识库核心组件审计 + 文档化 + 5 ADR + 部署/插件/wizard 三文档归并.
本 sprint 主体不动 prod 代码 (仅 1 reference migration + lib.rs/chunker.rs //! crate doc),
重在沉淀团队约定 + 决策记录, 为 v0.7 PR 阶段铺路.

**12 轮知识库深度评阅**:

| 轮 | 模块 | 结论 |
|---|------|------|
| R1 | chunker.rs (741 LoC) | code fence balance ✓; 改进空间: chunk_size 512→1024 (中文) + sentence boundary 50→100 字符. 留 v0.7 reindex tool |
| R2 | parser.rs (1033 LoC) | pdf / docx / asr / code / OCR fallback 完整 |
| R3 | search.rs (RRF) | K=60 + cross-lang + cross-domain penalty + budget allocation. 设计良好 |
| R4 | vectors.rs (usearch HNSW) | f16 + cos metric; 默认 HNSW params, 可暴露 to settings (v0.7 advanced) |
| R5 | store/items.rs 加密 | content/tags BLOB 加密 ✓, title/url 明文 (list 性能 trade-off, doc 须明示) |
| R6 | embed.rs | Ollama HTTP provider; v0.7 候选: ONNX direct (bge-small offline) |
| R7 | rerank pipeline | bge-reranker-v2-m3 via Ort, lazy hf_hub fallback |
| R8 | classifier + clusterer | Ollama qwen + hdbscan; min_samples=5/min_cluster=5 暴露 to settings (v0.7) |
| R9 | context_compress | budget-aware + cite preserve; chat.rs F-Pro evidence flow ✓ |
| R10 | taxonomy + plugin 融合 | 3 source HashSet 去重 (前 PLG-1 fix). conflict resolution log (v0.7) |
| R11 | F-17 PII redact | 12 类全覆盖 ✓; audit_log 当前 tracing 占位, v0.7 真持久化 store::audit_log |
| R12 | web_search 三层 fallback | 系统 / cache / NeedsDownload (FIX-9 stage 1 已 ship API) |

**8 轮代码深度审计 (D-R13~D-R20)**:

| 轮 | 主题 | 结论 |
|---|------|------|
| D-R13 | AppError migration | status.rs::status 作 reference migrate; 其余 ~37 routes 渐进 v0.7 |
| D-R14 | ArcSwap actual swap | 评估 ArcSwapOption&lt;dyn Trait&gt; 不支持 load_full (372 编译错). v0.7 用 ArcSwap&lt;Arc&lt;dyn&gt;&gt; + NoopProvider |
| D-R15 | 模块归并 ai/ | 100+ import 改写涉及, v0.7 单独 PR |
| D-R16 | 测试覆盖 | attune-core 1 test/38 LoC, 中等偏上 ✓ |
| D-R17 | 内存泄漏 | 7 worker loop 通过 AtomicBool flag, broadcast capacity 64 自 drop. 无明显泄漏 ✓ |
| D-R18 | logging level | 51 info + 47 warn + 11 debug + 2 error 分布合理 ✓ |
| D-R19 | graceful shutdown | lib.rs:306 SIGTERM+SIGINT handler 已实施 ✓ |
| D-R20 | SQLite WAL | journal_mode=WAL + busy_timeout=5000 + wipe checkpoint+VACUUM ✓. v0.7 加 startup PRAGMA optimize |

**6 轮文档化 (D-R21~D-R26)**:

- **D-R21**: lib.rs `//!` crate doc 写完 + chunker.rs `//!` 模板. 其余 1127 doc gap 增量 v0.7
- **D-R22**: docs/adr/ 5 ADR — OSS×Pro / FormFactor / GitFlow Lite / AppError / F-17 PII
- **D-R23**: cargo doc -p attune-core 通 (15 warning, broken intra-doc 后续修)
- **D-R24**: docs/wizard-flow.md — 5 步首启 + 失败回退 4 行表
- **D-R25**: docs/plugin-development.md — yaml schema + signing + encryption + 4 vertical + 本地测试
- **D-R26**: docs/deploy.md — Laptop / NAS / K3 三形态 + 迁移 + 故障排查

**4 轮 cross-cutting (D-R27~D-R30)**:

- **D-R27**: 安全审计 — Argon2id + AES-GCM + Device Secret 设计优秀 ✓
- **D-R28**: perf baseline — 已有 perf_chunker_bench.rs (#[ignore]). 完整 criterion 矩阵 v0.7
- **D-R29**: Observability — tracing_subscriber 在; /metrics + JSON logging 选项 v0.7
- **D-R30**: 本 sprint 汇总入 RELEASE + commit/push

**v0.7 跟踪清单** (RICE 排序):

| 项 | RICE | Effort | Note |
|---|------|-------|------|
| ArcSwap 真 migration (D-R14) | high | 1 day | Arc<dyn>+NoopProvider 占位法 |
| 37 routes AppError migrate (D-R13) | high | 2 day | reference 已在 status.rs |
| audit_log 持久化 (R11/F-17) | high | 0.5 day | UI 入口已 wire |
| 模块归并 ai/ (D-R15) | medium | 1 day | 100+ import |
| Rustdoc 增量补 (D-R21) | medium | continuous | 每周 100 个 pub item |
| criterion bench 矩阵 (D-R28) | medium | 1 day | 3 form factor × N config |
| metrics endpoint (D-R29) | medium | 0.5 day | Prometheus-style |
| HNSW params expose to settings (D-R4) | low | 0.3 day | advanced 用户 |

---

## v0.6.1（2026-04-30）— 边界收敛 + FormFactor 形态分裂 + RUSTSEC patch

发布定位：v0.6.0 GA 后第一个 minor — 治理 + 安全 + 形态感知，非用户可见功能新增。

**核心变更**（commit 94b57ec merge → main）：
- **OSS × Pro 边界一致性收敛**（ee859a4）：三产品矩阵叙事正式落地 — attune (OSS 通用) / attune-pro (个人行业增强) / lawcontrol (B2B 律所)；删除 OSS attune 内 4 个 builtin 行业 yaml + EntityKind::CaseNo + CHAT_TRIGGER_KEYWORDS 律师专属 const，全部迁到 attune-pro/plugins/<vertical>-pro/。
- **FormFactor 形态感知**（461c4c7）：检测启动环境（Laptop / Server / K3Appliance / Unknown），分裂 LLM 默认路径 — Laptop/Server/Unknown → 远端 token，K3Appliance → 本地 Ollama。8 个新 unit test 覆盖端到端（4b6e205）。
- **rustls-webpki 0.103.10 → 0.103.13**（b4c7351）：修 3 个 RUSTSEC CVE（TLS 验证链路相关）。
- **GitFlow Lite 写入 CLAUDE.md**（eded077 / 07f57d0）：分支模型 + tag 双轨 + `--first-parent` 检查命令固化为行为标准。
- **文档同步**（f5152b8 / f006aed）：README.zh 补 4 章，RELEASE 版本号同步。

**Server 产物**：[v0.6.1](https://github.com/qiurui144/attune/releases/tag/v0.6.1) — Linux x86_64/aarch64 + macOS aarch64 + Windows x86_64 tarball + sha256。**Desktop 此版未发**（沿用 desktop-v0.6.0 安装包；v0.6.1 改动均不影响桌面侧体验）。

---

## v0.6.3（2026-05-14）— LLM 热重载 + Plugins 数据源 + PII 全路径 + Pro Vertical 验收 + 架构与质量 sprint

发布定位：bug fix patch + UI polish + 4 vertical 端到端验证 + 7 路 CI 修复 + 20 轮全量 quality review + 6 项架构优化 + 2 项产品化 feature。

### rc.2 → rc.5 sprint (2026-05-14) — 7 路 CI 修复 + 20 轮 quality + 架构优化

**CI / Windows 平台 7 路修复**（rc.1 desktop installer 失败一路追到底）:
- `6421de9` `.gitignore models/` 无锚误吞 Python 包子目录 → CI ImportError; web_search_browser Linux 路径假设, Windows fail
- `8291f6c` NSIS installer 删 install-time Ollama 下载 (inetc plugin 在 GHA runner 缺) → Wizard 接管检测
- `51be338` governor_integration 用 MockMonitor 绕开 GHA Windows 高负载 (SysinfoMonitor CPU% > budget 全 worker stuck)
- `35d593e` index_path_test Windows 路径语义 cfg(unix) (治标)
- `6c0ef83` validate_bind_path 改 `dunce::canonicalize` 修 Windows UNC `\\?\` 真因 (Windows 用户加 vault 路径首破)
- `86f1534` tauri.conf.json version 0.6.0 → 0.6.3 + desktop-release.yml softprops/action-gh-release@v2 自动 publish
- `f828d35` desktop-release.yml 补 `permissions: contents: write` (rc.3 403 fix)
- `0752294` Windows matrix `bundles: nsis,msi` (rc.4 MSI 缺失 regression fix)

**20 轮全量 quality review**（覆盖文档 / 测试 / 代码 / 安全 / 技术债 / 对抗视角）:
- 16 项 issue 按 RICE 排序, P0/P1 已落地（FIX-1 ~ FIX-8 + 安全补 `OsRng`, lru 0.13 修 RUSTSEC-2026-0002, Node 24 opt-in, chrome channel 等）
- P2 长期项留 v0.7 follow-up

**架构 sprint — 第二轮深度分析 + 6 项优化**:
- D1 AppState `std::sync::Mutex` 19 字段 → 加 lock-free accessor (read+clone Arc, µs 级临界区); ArcSwap 实际替换留 v0.7
- D5 `store/items.rs` 7 处 `prepare()` → `prepare_cached()`, 热查询 SQL 解析省去
- D3 `attune-core/src/async_fs.rs` 新模块 (read / write / create_dir_all / try_exists 等 spawn_blocking 包装); 路由 3 处 `std::fs` 改 async_fs
- D6 workspace `[profile.dev.package."*"] opt-level=1` + `[profile.release] lto=thin codegen-units=1 strip=symbols` + workspace.deps 扩 chrono/uuid/tracing
- D7 API path snake → kebab + alias 双 mount 后向兼容
- ARCH-A `attune-server/src/error.rs` `AppError` enum 10 variant + IntoResponse + From<io::Error>/<serde_json::Error>/<VaultError>; 38 routes 渐进 migration

**FEAT-1 cloud endpoint UI gap 关闭**:
- backend `settings.cloud.{accounts_url, gateway_url}` 字段 + UI Settings 会员 tab "高级 · 自部署 cloud 后端" 折叠区 + 3 URL 输入 + 保存即热重载 pluginhub
- 关闭前次 Cloud-Integ-1 发现的自部署用户 UX gap (硬编码 attune.ai 没法切到私有 cloud)

**FEAT-2 浏览器 fallback (FIX-9 阶段 1)**:
- `attune-core/web_search_browser.rs` 加 `browser_cache_dir()` / `cached_browser_path()` / `resolve_browser()` 三段式 API + `BrowserResolution` enum (System / Cached / NeedsDownload)
- 阶段 1 ship cache 路径 + 解析 API; 阶段 2-3 (实际下载逻辑 + wizard UI) 留 v0.7

**Release 产物**:
- Server `v0.6.3-rc.2` ✓ — 4 平台 tarball (Linux x86_64/aarch64 + macOS aarch64 + Windows x86_64) + sha256
- Desktop `desktop-v0.6.3-rc.4` ✓ — 4 installer (NSIS exe / AppImage / deb / rpm), prerelease=true; rc.5 后含 MSI 完成 5 installer 矩阵

---

### 原 v0.6.3 release-blocker fixes

发布定位：bug fix patch + UI polish + 4 vertical 端到端验证。

**Release-blocker fixes**（commit 508b49c + d388282 在 origin/develop）：

| ID | 修复 | 影响 |
|----|------|------|
| LLM-1 | `AppState::reload_llm()` + settings.rs PATCH 在 `body.get("llm")` 时触发热切；抽出 `build_llm_from_settings` 自由函数复用 | 之前 wizard 配云端 LLM → 必须重启 server 才能 chat。修复后即时生效 |
| PLG-1 | `GET /api/v1/plugins` 合并 `state.taxonomy.plugins` + `state.plugin_registry.plugins()`, HashSet 去重 | 之前 attune-pro 4 vertical 装在 plugins/ 目录但 marketplace UI 完全不可见 |
| PII-1 | routes/chat.rs 自己拼 messages 直调 `llm.chat_with_history`, 完全绕过 ChatEngine redact。加 `Redactor::default()` 全路径拦截 + outbound_audit 日志 | 隐私功能 UI ✓ 但服务端真发原文给云端 LLM。修复后 audit log 实见 F-17 触发 |
| VLT-1 | `forgot-password-reset` 未清 bound_dirs/indexed_files, 重绑 FK 失败。`wipe_all_user_data` 加 WAL checkpoint + post-assert, `bind_directory_with_domain` 改 UPDATE-or-INSERT | 重置后再绑文件夹直接报 SQL 错 |

**UI / UX**：About 5 节信息齐 / Settings 锁定 warning 集中 / Wizard 5 步信息密度优化 + ? Tooltip / 暗色模式 token / 中英双语 locale 持久化。

**Verified on AMD laptop (Ryzen 7 8845H, NPU+iGPU)** — deb-only 部署：
- 重置 vault → Wizard 5 步全中文无英文泄漏
- hiapi.online + gpt-4o-mini 真接通 (响应附 web search 3 引用)
- 4 vertical (law-pro / patent-pro / presales-pro / tech-pro) 全 marketplace 可见; loaded 9 plugins log
- 暗色 / 设置 / About 5 节 round-trip 全过

**Cloud 自部署可用性**：AMD 笔电 (Docker 29 + Compose v2) 跑通 pluginhub:9100 / accounts:8002 / llm-gateway:8001 三服务 + /health ✓。修了上游 cloud 仓 2 个 bug（Dockerfile copy 顺序 + alembic 0002 down_revision 链断，本地 commit 558df7c 待 push）。

**已知限制 (v0.7 候选)**：
- attune-desktop Settings/Wizard 没字段配置自定义 accounts/pluginhub URL — 默认硬编码 `attune.ai` 云端。私有 cloud 部署（自托管 / dev 环境）目前只能 SQL 直改 `app_settings`。`state.reload_plugin_hub` 后端已支持热切，缺 UI 入口
- Reader / 项目卷宗 round-trip 未在本轮 Playwright E2E 覆盖
- CI Python lint-and-test + Windows cargo 在 commit 6421de9 修复后转绿（修了 `models/` gitignore 误吞 Python 包 + Windows 浏览器路径假设）

---

## v0.6.x patch 流（2026-05-01）— 部署 + 4 必要底座

### 最新变更（摘要 LLM 拆分 + 密码恢复机制 + 会员账号登录）

**摘要 LLM 拆分（2026-05-12 完成）**：
- **核心目标**：从 chat LLM 中独立出专用的 summarizer LLM，摘要不再占用云端 token
- **新字段**：`settings.summary_model` (Option<String>，默认 `"qwen2.5:3b"`)，用户可 PATCH 修改
- **自动探测**：启动时若未配置 summary_model，自动探测 Ollama（按 `SUMMARIZER_MODELS` 梯队顺序）；探测失败时回退到 chat LLM
- **新状态字段**：`AppState::summary_llm: Mutex<Option<Arc<dyn LlmProvider>>>`，初始化完全独立于 chat_llm
- **Phase 2 调用改动**：上下文压缩阶段优先用 `summary_llm`；若不可用则回退到 chat_llm；都不可用则原文透传
- **压缩策略推荐**：鼓励用户用 `"accurate"` 策略（300字摘要 + 100字原文头），降低数据丢失风险
- **快速失败**：第 1 个 chunk 摘要生成失败后，后续 chunks 跳过 LLM，直接原文（避免串行卡住）
- **硬件推荐表** (per `HardwareProfile::recommended_summary_model()`)：
  - ≥32 GB + 加速器 → qwen2.5:7b
  - 16-32 GB → qwen2.5:3b
  - 8-16 GB → qwen2.5:1.5b
  - <8 GB → llama3.2:1b
- **向后兼容**：旧 vault/settings 无 `summary_model` 字段 → 启动时用默认值；现有 context_strategy 配置保留
- **测试**：10 个单元测试全部通过，settings 端点兼容性验证通过

**Vault 密码恢复（非破坏性重置）**：
- `vault/setup` 响应新增 `recovery_key` 字段，格式 `ATN-{16hex}-{16hex}`；Web UI 首次安装自动下载 `attune-recovery-key.txt`，CLI 打印到终端
- 新端点 `POST /api/v1/vault/reset-with-recovery-key`：使用恢复密钥重置主密码，DEK 保持不变，所有知识库数据零丢失
- 新端点 `POST /api/v1/vault/forgot-password-reset`：最后兜底方案，需 vault 处于 LOCKED 状态 + 发送 `"confirm":"RESET"` 确认，清空所有本地数据
- LoginScreen 新增"使用恢复密钥重置密码"和"无恢复密钥？清空并重置"两个操作入口，提示文案从"忘记密码无法找回"改为恢复路径说明

**会员账号密码登录**：
- 新端点 `POST /api/v1/member/login-password`：邮箱 + 密码登录 Attune cloud 账号，自动拉取 license，设置 MemberState
- Settings → 会员 Tab：未登录时展示邮箱 + 密码表单（支持 Enter 提交）；登录后展示账号、License、等级、登出按钮

**测试**：新增 `vault_recovery_test.rs` 集成测试（3 个 E2E 场景）验证 recovery_key 格式、旧密码失效、新密码解锁

---

### v0.6.x 历史变更（多格式解析 + 全面测试覆盖 + 格式校验强化）

**多格式文件解析（parse_bytes_with_profile 扩展）**：
- 新增格式支持：`.html/.htm` (scraper strip-tags) / `.epub` (ZIP 内 XHTML 拼接) / `.xlsx/.xls` (calamine 电子表格) / `.pptx` (ZIP 内 slide XML) / `.rtf` (去标记提取) / `.csv` (原文 UTF-8)
- OCR 格式：`.png/.jpg/.jpeg/.webp/.bmp/.tiff/.gif` → PP-OCRv5 mobile（7 内置场景 profile：contract/receipt/screenshot/ancient/table/form/card）
- ASR 格式：`.mp3/.wav/.m4a/.flac/.ogg/.aac/.opus/.wma` → whisper.cpp subprocess
- **格式校验强化**：`parse_bytes_with_profile` 和 `parse_file_with_profile` 的 catch-all 分支现在对已知不支持的二进制格式（`.mp4`/`.zip`/`.exe` 等）返回 `VaultError::InvalidInput("unsupported file format")` 而非静默当文本处理。只有代码文件（CODE_EXTENSIONS）和 `.md/.txt` 才走文本兜底。

**测试覆盖大幅提升（commit 7661daa）**：
- **parser.rs 单元测试 +30**：覆盖 HTML roundtrip、EPUB/PPTX/RTF/CSV bytes 解析、is_supported 校验、不支持格式返回错误等
- **server_test.rs 集成测试 +20**：upload API 10 个测试（成功路径 + 422 校验 + 403 锁仓 + 400 无字段 + 重复上传 + 可检索性）；annotations CRUD 4 个（创建/列表/颜色校验/snippet 长度）；tags/status/behavior/clusters 端点 6 个
- **OCR profile 计数修正**：内置 profile 数由旧断言 4 → 修正为 7

**当前测试基线**：237+ 全部通过（attune-core 210 + attune-server 27）

---

### b5b837f（2026-05-xx）— UI 构建修复 + Tauri 拖拽上传

**UI TypeScript 修复**：
- `store/api.ts`：补充 `put<T>(path, body, retry?)` 方法，支持 HTTP PUT（useOcrProfiles.updateOcrProfile 需要）
- `views/SettingsView.tsx`：修复全部 `toast.success/error()` → `toast('success'/'error', msg)` 调用（8 处）；`Section` 组件增加 `desc?: string` prop，relaxed children 类型（支持 `false | null`）
- `App.tsx`：修复 useEffect 代码路径返回值问题（early return 模式）

**Tauri 桌面拖拽上传**：
- `apps/attune-desktop/src/main.rs`：新增 `upload_dropped_paths(paths: Vec<String>)` Tauri command，读取本地文件路径 → multipart POST `http://127.0.0.1:18900/api/v1/upload`
- `apps/attune-desktop/Cargo.toml`：添加 `reqwest 0.12`（rustls-tls + multipart + json features）
- `App.tsx`：启动时检测 `window.__TAURI__?.event?.listen`，若在桌面模式则注册 `attune-file-drop` 监听器 → 调用 `upload_dropped_paths` command
- FileDrop 事件路径：系统文件拖入窗口 → Tauri 发出 FileDrop 事件 → main.rs emit `attune-file-drop` 至 WebView → App.tsx 调用 `upload_dropped_paths` → 文件上传至 `/api/v1/upload`

**Items 页面真实上传**：
- `views/ItemsView.tsx`：ItemsHeader 上传按钮接入隐藏 `<input type=file multiple>`，multipart FormData + Bearer token POST 至 `/api/v1/upload`（PDF/MD/TXT/DOCX/PNG/JPG）

**UI dist 重新构建**：171.80 kB（gzip: 48.62 kB），71 个模块，TypeScript 严格检查通过

---

**4 必要底座（CLAUDE.md "硬件感知的默认底座" 实装）**：

| 底座 | 默认引擎 | 体积 | 来源 |
|------|---------|------|------|
| Embedding | bge-m3 / bge-small (Ollama) | 1.2 GB / 200 MB | postinst `ollama pull` |
| Reranker | Xenova/bge-reranker-base (ONNX) | ~120 MB | 首查 lazy hf_hub 下载 |
| ASR | whisper-cli + ggml-large-v3-turbo-q5_0 | 2.6 MB binary + 574 MB model | binary 进 .deb bundle，model postinst 下载（中文 WER 5-7%） |
| OCR | PP-OCRv4 mobile (DBNet+CRNN+CLS+dict) | ~21 MB ONNX | postinst HF `SWHL/RapidOCR/PP-OCRv4/...` |

**LLM 不在底座**（2026-05-01 用户拍板，澄清版）：

核心原则：**云端为主，本地为辅；本地 LLM 当前研发成本过高，暂时不主推**。

Wizard 推荐顺序：
1. ★ **Attune Pro Membership**（默认）— `https://gateway.attune.ai/v1`，登录即用 token 配额
2. **BYOK**：用户已有付费会员 API key — OpenAI / Anthropic / Gemini / DeepSeek / Qwen
3. **本地 Ollama**（advanced，K3 一体机预装 qwen2.5:1.5b/3b 走本地）

不走第三方 "free API tier"（Gemini Free / Groq 等），避免误导用户。
不走 MCP backbone，至少 v0.7 不做，简化产品形态。

**Form factor 检测** (`detect_form_factor()` in `attune-core::platform`)：
1. `ATTUNE_FORM_FACTOR=k3` env var override（K3 镜像构建时 systemd-environment.d）
2. `/sys/class/dmi/id/product_name` 含 `k3` / `jetson`
3. 默认 `laptop`

**安装路径全平台覆盖**：
- `.deb` (Ubuntu/Debian) — preinst+postinst+prerm+postrm 4 hooks
- `.rpm` (Fedora/RHEL) — 共用 4 个 .sh hook
- `.AppImage` (universal Linux) — 无 hooks，运行时 wizard
- NSIS `.exe` (Windows) — installer.nsh 4 macros + inetc::get OllamaSetup.exe

**关键变更**：
- 单引擎 OCR — 删 tesseract，PP-OCRv5 mobile 唯一引擎（中文准确率 70-85% → 94-96%）
- LLM 不本地预装（笔电）— 用户在 wizard 配 cloud API 或 Ollama；K3 镜像例外
- whisper.cpp 2.6 MB 静态 binary 进 Tauri bundle resources（替代 apt 包）
- ROCm gfx1103 自动 HSA_OVERRIDE_GFX_VERSION=11.0.0 写 systemd drop-in
- graceful shutdown via SIGINT/SIGTERM oneshot（R35）
- 日志 daily rotation `~/.local/share/attune/logs/`（R37）
- vault export/import CLI（R38）
- Windows + Linux CI matrix（R18）+ 慢测试 nightly（R19）

**实测在 Ubuntu 26.04 LTS + AMD Ryzen 7 8845H + Radeon 780M (gfx1103)**：
- TRUE zero-state 安装 149 秒（含 600 MB Ollama install + 1.2 GB bge-m3 + 21 MB PP-OCR + 250 MB ASR）
- HSA_OVERRIDE_GFX_VERSION=11.0.0 自动注入 systemd Environment
- bge-m3 embed: 冷 1.6s / 热 74ms
- qwen2.5:3b（用户 wizard 后装）: 47.2 tokens/s（确认 ROCm 加速）
- 1024-dim embed + 端到端 RAG chat 跑通（24.7 t/s 稳定）

---

## v0.6.0 GA（2026-04-30）— 私有 AI 知识伙伴正式发版 🎉

**发布产物**（双轨）：
- [v0.6.0 (server/CLI)](https://github.com/qiurui144/attune/releases/tag/v0.6.0)：Linux x86_64/aarch64 + macOS aarch64 + Windows x86_64 二进制 tarball + sha256
- [desktop-v0.6.0 (Tauri 桌面)](https://github.com/qiurui144/attune/releases/tag/desktop-v0.6.0)：Win NSIS 16M + MSI 31M + Linux deb 27M + AppImage 94M

**核心能力**（累积自 v0.5）：
- 私有 vault：AES-256-GCM 字段级加密 + Argon2id KDF + Device Secret
- F-Pro 跨域污染防御 4 阶段（domain / chunk prefix / penalty / keyword intent）
- Phase A.5 PII 防护（L0 文件锁 / L1 正则脱敏 / L3 LLM 语义脱敏）
- Phase B 双赛道 benchmark（法律 Hit@10=0.80 / Rust 0.60 / 中文八股 1.00）
- Web UI（8 标签页 + Settings 模态 + Reader 模态 + Cmd+K 全局搜索）
- Tauri 2 桌面应用（系统托盘 + 4 平台安装器，原 v0.6 路线图项目落地）
- AI 自动分类 + HDBSCAN 聚类 + builtin 插件（编程/法律/专利/售前）
- Chrome 扩展（MV3 + Preact + Vite，与桌面 server 互通）
- TLS NAS 模式 + WebDAV 远程目录 + 行为画像导入/导出
- SkillClaw 后台技能进化 + 浏览器自动化网络搜索（chromiumoxide）

**测试统计**：
- 1240 测试通过（Unit 540 + Integration 668 + Smoke 5 + Corpus 4 + Quality 7 + E2E 16）
- lawcontrol_compat golden_qa: 24.80/25（excellent 10/12，99.2%）
- 20 轮全面健康检查: 17/20 PASS（案件证据链 5/5 全绿）
- attune-pro Phase D VLM 28 cases baseline: 23/28（82.1%）
- 30 轮 GA 前审查: 代码 A / 安全 A / 治理 B+

**依赖安全里程碑**：
- rustls 完整采用，openssl-sys / native-tls 100% 根除（rc.7 修）
- usearch 2.25 修 Windows MAP_FAILED build（rc.7 修）
- 4 平台 CI matrix（macOS Intel 走源码，Apple Silicon 已覆盖）

**双轨制 Release 流程固化**：
- `vX.Y.Z` 触发 rust-release.yml（server/CLI 二进制）
- `desktop-vX.Y.Z` 触发 desktop-release.yml（Tauri 安装器）
- 见 DEVELOP.md「Tag 双轨制 + Release Checklist」章节

**Tag 历史**：alpha.1 → alpha.2 → rc.4 → rc.5 → rc.7 → rc.8 → **v0.6.0 GA**
（rc.6 因 CI 资源问题 cancel 后重打 rc.7；详见 commit 历史）

---

## v0.6.0-rc.5（2026-04-28）— 三赛道 PRO + 5 维度满分

**关键交付**：检索 + 答案双 PRO 级，跨域污染防御、PII 脱敏、证据流端到端全部上线。

### Benchmark 数字
- Scen A 法律 (lawcontrol): Hit@10=**0.80**, MRR=0.50 ✅ PRO
- Scen B Rust (rust-book): Hit@10=**1.00**, MRR=**1.00** ✅ PRO 满分
- Scen C 中文八股 (cs-notes): Hit@10=**1.00**, MRR=**1.00** ✅ PRO 满分
- Lawcontrol golden_qa 5 维度: **25.00/25** (10/10 excellent，vs baseline +39%)

### Phase A.5 — PII 脱敏 + 隐私分级
- `attune-core::pii` 新模块：12 类格式化 PII（含 ISO 7064 身份证 / Luhn 信用卡 / 8 家 API key）+ 用户自定义词典 + 可逆 placeholder
- `attune-core::store::audit` + `routes::audit`：出网审计日志 + CSV 导出
- `items.privacy_tier` 字段 + per-file 🔒 标记 (L0/L1/L3)
- vertical plugin 在 `plugin.yaml::pii_patterns` 声明行业 PII（案号 / 病历号 / 专利号）

### Phase B — 双赛道 benchmark
- `scripts/parse-legal-dump.py`: lawcontrol PG dump → 10,677 .md 文件
- `scripts/bench-orchestrator.sh`: 一站式 vault setup + bind + index + query
- `scripts/run-final-eval.py`: 15 题 retrieval + 3 题 evidence flow 验证
- `attune-pro/law-pro/run_golden_qa`: 10 case × 5 维度评分
- `docs/benchmarks/dual-track-baseline.md`: 5 轮演化报告

### F-Pro — 跨域污染防御 4 stage
- Stage 1: `items.corpus_domain` + `bound_dirs.corpus_domain` 字段
- Stage 2: chunk text 头部注入 `[领域: legal]` prefix
- Stage 3: `CROSS_DOMAIN_PENALTY = 0.4`（query domain ≠ doc domain）
- Stage 4: `detect_query_domain` 关键词 4 domain × 12-30 词（零 LLM 调用）
- 效果：法律 0.60 → 0.80, Rust MRR 0.87 → 1.00

### 证据流端到端
- `chat.rs` route 4 处数据丢失 bug 修复（breadcrumb / chunk_offset / confidence）
- citation breadcrumb fallback 到 [item.title]
- `parse_confidence` + `strip_confidence_marker` 在 chat route 接入

### Embedding / LLM env vars
- `ATTUNE_EMBEDDING_BACKEND=ollama`：3.6× 提速 + F16 全精度
- `ATTUNE_CHAT_MODEL=<name>`：覆盖自动探测
- 默认 reranker 切 BAAI/bge-reranker-base 官方 ONNX（修 Xenova Expand bug）

### Schema migrations（幂等，自动）
- `chunk_breadcrumbs` 加密 + `embed_queue.task_type` + `items.privacy_tier/corpus_domain` + `bound_dirs.corpus_domain`

### 测试 / 文档
- attune-core lib: **537+ tests**
- 双语 release notes: `docs/v0.6-release-notes.md` + `.zh.md`
- 文档站章节: 见 wiki.your-company.com/attune

### 已知限制（v0.7 解决）
- L3 LLM 语义脱敏（trait scaffold 在 `pii::ner`）
- Settings → Privacy 完整 UI（当前是只读+导出）
- 122 routes 渐进迁移 `routes::errors` helper
- Phase D VLM 28 golden cases
- macOS

## 开发中

## W3 Batch C: K2 Parse Golden Set Baseline (2026-04-27)

12-week 战略 v4 W3 F-P0c batch C **收官**。建立 chunker / parser 质量门槛，防止后续 chunker 改动悄悄回归。来源：Readwise Reader 200 页 parsing benchmark + CI 95% 阈值方法论（per ACKNOWLEDGMENTS K 系列）。

### K2 Parse Golden Set baseline（5 fixtures）
- `tests/fixtures/parse_corpus/manifest.yaml` — 5 fixture 描述（id / file / source / pinned_version / license + expected: title_contains / min_text_chars / must_contain_phrases / section_count_min / section_paths_must_include）
- 5 个 markdown fixture（覆盖 4 个领域 + 双语）：
  - 001 rust-lang/book ch4 'What Is Ownership' (en, MIT/Apache-2.0)
  - 002 中华人民共和国民法典节选 (zh, 公开法律文本)
  - 003 tech blog post — microservices vs monolith (en, attune-internal)
  - 004 news article — EU AI Act (en, attune-internal)
  - 005 academic paper review — Attention Is All You Need (en, attune-internal)
- `tests/parse_golden_set_regression.rs`：8 测试（manifest loads + files exist + min_rate gate + 5 per-fixture pass）
- Regression gate：`min_pass_rate=1.0`（5 篇必须全过，扩 200 时降到 0.95）

### 与 J6 W4 benchmark 的关系
- J6 测**检索质量**（query → expected hits），用 `rust/tests/golden/queries.json`
- K2 测**parser/chunker 质量**（page → expected sections），用 `tests/fixtures/parse_corpus/`
- 两个 golden set 同期跑，构成 attune 完整质量基线

### W3 batch C 不做（推到 W4 / W5-6）
- ❌ 200 篇真实页面采集 — 需 1-2 天 corpus 工作（W4）
- ❌ GitHub Actions CI 集成（W4 与 J6 一起接入 benchmark CI）
- ❌ Per-language fixture 矩阵扩展（当前 5 篇含 1 zh + 4 en，足够 baseline）
- ❌ PDF parsing fixture（独立 golden set，W5-6）
- ❌ Readability.js style content extraction baseline（阻塞 G3，W5-6）

### W3 全量收官（A + B + C）
| Batch | Commit | 主交付 | 测试 +N |
|-------|--------|-------|---------|
| A `28bd691` | F2 placeholder 关闭 + C1 web cache + F1 + F4 | +16 lib + 7 集成 |
| B `674cf55` | G1 浏览信号全栈 + G5 隐私面板 + F3 E2E | +7 lib + 5 集成 |
| C 本次 | K2 Parse Golden Set | +8 集成 |
| **W3 总计** | 3 commits | +23 lib + **20 集成** |

attune-core lib 测试 415 (W2 末) → **438**（+23），集成测试套件 3 → **6**（+governor + memory + rag_w2 + rag_w3_batch_a + rag_w3_batch_b + parse_golden_set）。

## W3 Batch B: G1 + G2 + G5 + F3 (2026-04-27)

12-week 战略 v4 Phase 1 W3 F-P0c batch B 全栈交付。**Chrome 扩展从"AI 对话捕获器"升级为"通用浏览状态知识源"** + 隐私控制面板 + W2 batch 1 followup F3 关闭。所有抄袭点登记到 [`ACKNOWLEDGMENTS.md`](../ACKNOWLEDGMENTS.md)。

### ⚠️ Breaking change — 升级 action required
- Chrome 扩展 manifest 加 `<all_urls>` host_permission + `incognito: not_allowed` + 新 content script — **首次升级用户安装时 Chrome 会弹出权限重新授权对话框**（"读取所有网站数据"）。这是 G1 浏览捕获的硬要求；隐私默认完全 opt-out，需在扩展 popup → 浏览隐私 → 显式加 domain 才会捕获。
- attune-server 新增 `browse_signals` 表 — 老 vault 升级时 schema 自动 IF NOT EXISTS 创建空表，无需操作。
- attune-server 新增 `<incognito>` Chrome 扩展强制不加载 — 用户在 Chrome 设置启用了"在隐私窗口允许扩展"也会被拒绝（防御 content script JS 检查被绕过的攻击）。

### 用户视角的影响
- **新能力**：浏览任意网站时，停留 ≥3 分钟 + 滚动 ≥50% + 复制至少 1 次 → attune 自动记下"你在意什么"作为 SkillEvolver / Profile 的输入信号
- **隐私默认零捕获**：装好后什么都不发生，必须显式 opt-in 每个 domain
- **硬黑名单覆盖任何手动 opt-in**：银行 / 医疗 / 政府登录页 / 密码管理器永远不捕获
- 数据全部本机加密存储（DEK + AES-GCM）— `url` / `title` 加密，`domain_hash` HMAC + pepper（W4 升级到 vault salt 派生）

### G1 浏览信号捕获（后端 + 扩展全栈）
- 新表 `browse_signals`：url/title DEK 加密 + domain_hash HMAC-SHA256(pepper, domain) + dwell/scroll/copy/visit + ts，带索引
- `Store` API：record / list / count / clear_for_domain / clear_all
- attune-server 路由 `/api/v1/browse_signals` 三 method（POST batch / GET diagnostics / DELETE）
- Chrome 扩展 `extension/src/content/browse_capture.js`：visibilitychange dwell + IntersectionObserver scroll + copy 监听 + whitelist + HARD_BLACKLIST
- 30 秒周期 flush + 失败重入队 + 500 上限保护（先裁尾老数据再 unshift 新失败批，per reviewer I4）
- **隐私默认 opt-out**：用户必须在 popup 显式加 domain 才捕获。HARD_BLACKLIST 双层正则（hostname 银行/政府/密码管理器 + pathname /login //signin /password 等，per reviewer S2）
- **Incognito 硬阻断**：`chrome.extension.inIncognitoContext` 显式检查（per reviewer S1）
- 字段长度上限：URL ≤2048 / title ≤512 char，防恶意页面 1MB title 拖慢加密（per reviewer I3）

### G2 高 engagement 评分
- `is_high_engagement`：dwell ≥3 分钟 + scroll ≥50% + copy ≥1
- W3 batch B 仅计数返回 `high_engagement`，不创建 placeholder item（避免无内容污染知识库）
- W5-6 G3 引入 page extraction 后再 auto-bookmark with body

### G5 隐私控制面板
- 新组件 `extension/src/popup/Privacy.jsx`（Preact）
- per-domain whitelist 增删 + 全局 Pause toggle + 已捕获计数 + 清除按钮（全清/per-domain）
- "默认 opt-out / 数据仅本机 / 硬黑名单覆盖"三段式提示

### F3 J5 secondary retrieval E2E（关闭 W2 batch 1 followup）
- `tests/rag_w3_batch_b_integration.rs` 5 测试：高/低/默认 confidence + 中英 marker + serde 字段
- 关闭 W2 batch 1 reviewer P2 #5 留的 followup

### 遗留代码清理（per 用户 2026-04-27 要求"开源方案获取后做好遗留代码检查"）
- W3 batch B 引入的 3 项 warning 全清零（chunker doc / dead write / store unused imports）
- 删 `worker.js` 2 处 dev console.log
- 累积老死代码 + Chrome 扩展 console.log 5+4 项记入 `tmp/w3-batch-b-followups.md` → W4 单独"代码卫生"批次

### 工程
- 测试：attune-core lib 431 → **438** (+7 browse_signals) + W3 batch B 集成 5 = +12 测试
- attune-server lib: 3（零回归）
- **R1 单轮 review**（W3 节奏紧）：reviewer 找 2 严重（incognito + HARD_BLACKLIST 误报）+ 4 重要 + 7 建议；本批次修 6 项必修（S1 / S2 / I1 / I3 / I4 / N4 / N6），其余进 followup

### 不做（推到 W3 batch C / W4）
- ❌ G3 页面内容抽取（Readability.js）— W5-6
- ❌ G4 跨 session topic cluster — W7-8
- ❌ G5 角色化预设白名单 — W7-8
- ❌ Domain hash 完整 vault salt（当前用编译期 pepper）— W4
- ❌ Chrome 扩展 console.log 全清 + DEBUG 守护 — W4
- ❌ K2 Parse Golden Set — W3 batch C

## W3 Batch A: F1 + F2 + F4 + C1 (2026-04-27)

### ⚠️ Breaking change — schema migration
- W3 batch A 末（commit `28bd691`）→ W3 末（含 R04 P0-1 加密 + R07 P0 migration）：
  `chunk_breadcrumbs.breadcrumb_json TEXT` 列名改 `breadcrumb_enc BLOB` (DEK 加密)
- 老 vault 升级时 `migrate_breadcrumbs_encrypt` 自动 DROP + 重建表
- **老明文 breadcrumb 数据丢失**（acceptable — 下次 indexer ingest 自动 backfill 加密版本）
- **首次升级后第一次 chat 引用：Citation.breadcrumb 可能为空**直到 indexer 重建（< 1 分钟）

### 用户视角
- **F2 关闭 W2 placeholder**：现在 chat 引用真带 chunk path（`产品手册 > 第三章 > 3.2 假期`）
- **C1 web cache**：相同 query 30 天内自动复用，省 token + 加速

12-week 战略 v4 Phase 1 W3 F-P0c batch A 后端深做。**关闭 W2 batch 1 的 Citation placeholder 状态** + 加 web search 缓存层 + 关键可观测性日志。所有抄袭点登记到 [`ACKNOWLEDGMENTS.md`](../ACKNOWLEDGMENTS.md)。

### F2 关闭 W2 batch 1 placeholder（核心）
**之前**：`Citation.breadcrumb = Vec::new()` + `chunk_offset_* = None` 始终占位。
**现在**：从 indexer 透传到 Citation 真值。

- 新增 `chunk_breadcrumbs` sidecar 表（FK CASCADE + 软删除路径显式清理）
- `Store::upsert_chunk_breadcrumbs_from_content` 在 indexer pipeline 4 个调用点全部接入：`routes/upload.rs` / `routes/ingest.rs` / `scanner.rs` / `scanner_webdav.rs`
- `SearchResult` 加 `breadcrumb` / `chunk_offset_start/end` 字段（serde `skip_serializing_if` 保持 Chrome 扩展旧客户端兼容）
- `search_with_context` 在 item 解密后查 sidecar 填充
- `ChatEngine.chat()` 透传 SearchResult → Citation
- **Known limitation (v1)**：当前 offset 是 sidecar 内累计 char count，不严格对齐原文 char index — 适合 item 顶层导航；W5+ 真正按行号映射回原文。前端 Reader 精确高亮请等 W5
- `delete_item` (软删除) 同步清理 breadcrumbs，防止 stale data 透传

### C1 Web search 本地缓存
- 新增 `web_search_cache` 表：query_hash (SHA-256) 主键 + DEK 加密 query/results + 30 天默认 TTL
- `Store::get_web_search_cached / put_web_search_cached / clear_web_search_cache / web_search_cache_count`
- `ChatEngine.chat()` web fallback 路径：先查 cache miss 才发网络请求；fetch 后立即写 cache（含空结果 — TTL 自然失效）
- 用户后续可在 Settings 清空 web 缓存（route 待 batch B 加）
- **来源**：[吴师兄文章](https://mp.weixin.qq.com/s/YNcfSN0uv1c1LsLPzgB0jw) §6 高频 query 缓存 + Readwise/Linkwarden "fetch 时快照"模式

### F1 J5 二次检索可观测性
chat.rs 加 `J5 F1` 前缀日志区分 fallback 召回更多 / no-op / 失败三类，便于线上诊断。

### F4 RELEASE notes 同步
W2 batch 1 placeholder 状态 → 标记 RESOLVED in W3 batch A（本批次）。

### 工程
- 测试：attune-core lib 415 → **431** (+16)，新增 W3 集成测试 7（共 18 集成）
- **Two rounds of code review**：R1 找 2 严重 + 5 重要 + 6 建议（10 项必修，全修）；R2 找 P0-1 软删除漏 breadcrumbs（修，加 `soft_delete_clears_breadcrumbs` 产品安全测试）

### W3 batch A 不做（推到 batch B/C）
- ❌ G1 / G5 Chrome 扩展浏览捕获 — 全栈跨会话深做
- ❌ K2 Parse Golden Set 200 篇 — 语料采集 + CI 流水线
- ❌ J5 secondary retrieval E2E 测试（F3 推到 batch B 与 ChatEngine 完整构造一起做）
- ❌ Cache GC daemon / 空结果短 TTL / scanner 增量 file_hash 短路 — W4 backlog

## W2 Batch 1: J1 + J3 + J5 + B1 backend (2026-04-27)

12-week 战略 v4 Phase 1 W2，**第一波用户感知 RAG 质量**改造。配合 6 维度开源生态调研后明确"抄 vs 自研"边界，全部抄袭点登记到 [`ACKNOWLEDGMENTS.md`](../ACKNOWLEDGMENTS.md)。

### J1 Chunk 面包屑路径前缀
- `attune_core::chunker::extract_sections_with_path` 新增；输出 `SectionWithPath { section_idx, path, content }`，path 是文档根开始的标题层级
- `with_breadcrumb_prefix()` 把 path 用 Markdown blockquote `> A > B > C` 注入到 chunk 头部
- 旧 `extract_sections` 改为 wrapper 调新版（消除重复实现）
- Markdown 标题识别扩展到 H1-H6（CommonMark 标准）
- **来源**：[吴师兄文章](https://mp.weixin.qq.com/s/YNcfSN0uv1c1LsLPzgB0jw) §1

### J3 召回 cosine 阈值显式化
- `SearchParams::min_score: Option<f32>` 字段
- **`with_defaults` 默认 None**（保持 W2 前 Chrome 扩展 `/api/v1/search` 契约）
- **`with_defaults_for_rag` 默认 0.65**（chat 主路径专用，per 吴师兄经验值）
- vector 结果在 RRF 融合**之前**按 min_score 过滤；BM25 不过滤（score 不同尺度）
- **来源**：吴师兄文章 §2 0.65/0.72/0.78 三档曲线

### J5 强约束 Prompt + 置信度 + 二次检索
- `build_rag_system_prompt` 重写：明确禁用模糊措辞（"可能"/"大概"/"建议咨询"）+ 引用必带来源 + 末尾输出【置信度: N/5】
- `parse_confidence(response: &str) -> u8`：解析末尾 marker（中文【置信度: N/5】+ 英文 fallback [Confidence: N/5]）；缺失默认 3；取最后一个 marker（避开草稿中提到的示例）
- `strip_confidence_marker(response: &str) -> String`：剥离 marker（与 parse 对称取最后一个）
- `ChatResponse` 加 `confidence: u8` + `secondary_retrieval_used: bool` 字段
- **二次检索（CRAG §3.2 ambiguous 分支）**：confidence < 3 → 降阈值 0.65→0.55 二次本地召回 + LLM 重跑一次（**硬上限一次重试**，无论本地 / web 路径都允许 fallback）
- **来源**：[CRAG arXiv:2401.15884](https://arxiv.org/abs/2401.15884) §3.2 + [Self-RAG arXiv:2310.11511](https://arxiv.org/abs/2310.11511) confidence token 简化

### B1 backend: Citation 加 deep-link 数据
- `Citation` 加字段：`chunk_offset_start: Option<usize>` / `chunk_offset_end: Option<usize>` / `breadcrumb: Vec<String>`
- ~~**Known limitation**：当前 Citation.breadcrumb 永远 `vec![]`，offset 永远 `None`~~ **RESOLVED in W3 batch A**（per F2，indexer 透传完整闭环；offset 当前是 sidecar 累计 char，W5+ 真正按行号映射）
- 前端 Reader 模态高亮 / 滚动到 offset 是单独 PR（下次 Tauri/Preact 会话）

### 工程
- chat 模块 `pub(crate)` + `lib.rs` 精确 re-export `Citation` / `ChatEngine` / `ChatResponse` / `parse_confidence` / `strip_confidence_marker`（不暴露 ChatEngine 内部依赖）
- 新建 `ACKNOWLEDGMENTS.md` + `.zh.md` 项目根 attribution registry，每个抄袭点必须登记
- 测试：attune-core lib 394 → 415（+21）+ 11 集成 = 26 新测试。0 回归
- **Two rounds of code review**：R1 4 严重 + 5 重要全修；R2 conditional pass + P1 #2 strip 对称已修；剩余 P1/P2 followups 见 `tmp/w2-batch1-followups.md`

## A1 Memory Consolidation MVP (2026-04-27)

12-week 战略 v2 Phase 1 W1。引入**周期性 episodic memory** 数据模型 — chunk_summaries 按时间窗口聚合成"用户那天学了什么"的高层记忆，是 attune"自进化记忆"叙事（mem0 参考）的数据基石。详见 [`docs/superpowers/specs/2026-04-27-memory-consolidation-design.md`](../docs/superpowers/specs/2026-04-27-memory-consolidation-design.md)。

- **`memories` 表新增**：id / kind / window_start/end / source_chunk_hashes (JSON, sorted) / summary_encrypted / model / created_at；唯一索引 `uq_memories_source(kind, source_chunk_hashes)` 保证幂等
- **CHECK 约束已预先支持** `('episodic', 'semantic')`，W5+ 加 semantic 时无需 schema migration
- **`attune_core::memory_consolidation`** 新模块：三阶段 API（prepare/generate/apply）+ `generate_one_episodic_memory` 单 bundle helper（供 worker 按 bundle check H1 LLM 配额）
- **MVP 边界**：仅 episodic（按 1 天时间窗口）；semantic / chat 检索集成 / conflict detection / 用户面 UI 都明示推迟到 W5+
- **生产 worker** `attune-server::state::start_memory_consolidator`：6h 周期，三阶段锁释放（与 skill_evolver 同构），Phase 2 按 bundle 取 LLM 配额，Phase 3 重新 lock 复查 vault state + 重新取 dek 防 stale 密钥
- **30 天 lookback**：超过 30 天的老 chunk 当前不会被 consolidate（设计：留给 W5+ semantic memory）；现存 vault 升级时老历史不会自动 backfill
- **测试**：6 store CRUD（含幂等验证）+ 10 单元（prepare/generate/apply 三阶段 + 边界）+ 4 集成（真 Store + tempfile + MockLlm 完整周期）。attune-core 总测试 378 → 394
- **Test helper feature gate**：`__test_seed_chunk_summary` 用 `#[cfg(any(test, feature = "test-utils"))]` 守护，生产二进制不暴露；attune-core 自身 dev-dep 启用 test-utils，`cargo test` 无需额外 flag
- **Two rounds of code review**：R1 5 issues 全修（LLM 配额超发、Phase 3 stale dek、test helper 暴露、model name race、CHECK 约束等）；R2 conditional pass + P1-1 dev-dep fix

## H1 资源治理框架 (2026-04-27)

12-week 战略 v2 Phase 1 W1。引入**任务级协作式资源治理**，所有后台 worker 的"系统好公民"基础设施。详见 [`docs/system-impact.md`](../docs/system-impact.md)。

- **`attune_core::resource_governor`** 新模块：`Budget` / `Profile` / `TaskKind` / `TaskGovernor` / `GovernorRegistry` / `SysinfoMonitor`
- **三档预设**：`Conservative` / `Balanced`（默认）/ `Aggressive`，每档 × 10 任务种类共 30 组合配置
- **全局 CPU 阈值语义**：`cpu_pct_max` 是"系统全局 CPU 占用 > 阈值时本任务暂缓"，多 worker 共享同一全局指标，避免 budget 累加 > 100% 失真
- **顶栏 Pause 全局信号**：`global_registry().pause_all()` 1 秒内停所有 worker，集成测试验证
- **LLM 速率窗口**：SkillEvolution / MemoryConsolidation 类任务额外 `allow_llm_call()` 滑动小时窗口，防止 LLM 失败重试风暴
- **W1 已 retrofit**：`attune-server::state::start_{classify,rescan,queue,skill_evolver}_worker` 4 个生产 worker；`attune-core::queue::QueueWorker` 库路径
- **测试**：30 组合 snapshot + 28 单元 + 4 集成 + 1 ignored 真烧 CPU（本地实测 32 burner 线程 + Conservative 15% 阈值 → throttled=50/allowed=0，治理 100% 生效）
- **跨平台**：sysinfo 0.32 全 Linux/Windows/macOS；CPU 采样 250ms 缓存防 sysinfo MINIMUM_CPU_UPDATE_INTERVAL 退化
- 设计文档：[`docs/superpowers/specs/2026-04-27-resource-governor-design.md`](../docs/superpowers/specs/2026-04-27-resource-governor-design.md)

## 已发布

## 深度阅读 + 批注 + 上下文压缩 (2026-04-18)

本次包含 **6 个连续 batch**，每批经过 **2 轮独立 code review** + **Playwright E2E 回归**
（最终 10 phase / 57 断言全过）。总测试数 213 → **299 tests（+86）**。

### Batch 1：Settings 重构 · 硬件感知默认 · OCR 兜底

- **Settings UI 简化**：7 张卡 14 字段 → 4 张主卡 + 1 折叠"高级"
- **硬件感知摘要模型**：启动检测 CPU/RAM/GPU/NPU → `recommended_summary_model()` 按档位推荐
  （≥32GB+加速器 → qwen2.5:7b · 16-32GB → qwen2.5:3b · 8-16GB → qwen2.5:1.5b · <8GB → llama3.2:1b）
- **非 Linux RAM/CPU 检测**：macOS (sysctl) + Windows (wmic) + NVIDIA Windows 探测
- **扫描版 PDF OCR**：pdf_extract 失败或文字层 < 100 字 → 自动走 tesseract CLI + pdftoppm，中英双语
  `scripts/install-ocr-deps.sh` 一键装依赖（apt / dnf / pacman / brew）
- **上传 body limit** 20 → 100 MB，支持整本扫描版 PDF
- `AppState.hardware` 启动时检测一次并缓存，避免每次 `/settings` 请求重复读 `/proc` / sysctl

### Batch 2：顶栏 + 模态 Settings + 模型 chip

- 全局顶栏：logo · 🔒 锁定按钮 · 👤 头像菜单（设置 / 导出画像 / 导出设备密钥 / 锁定）
- Settings 从 tab 变成 ChatGPT 式模态对话框（对话模型 + 网络搜索 + 数据备份 + 高级）
- Chat tab 头部的 **模型 chip**：🟢 本地 / 🔵 云端 颜色区分，点击下拉切模型，"配置更多模型..." 直达设置
- 对话模型 provider radio（本地 Ollama / OpenAI / Claude / 自定义 OpenAI 兼容端点）条件展示 Key 字段
- provider 切换即时同步 token chip 颜色与成本估算
- 移除 Settings tab 中重复的 `btn-lock`（三入口收敛到两个）
- ESC 关模态（优先级：popup > reader > modal > dropdown）

### Batch A.1：用户批注 CRUD

- **新表** `annotations`：字符偏移 + snippet 双锚点 · content 加密 BLOB · `ON DELETE CASCADE`
- 5 个预设标签：⭐重点 / 📍待深入 / 🤔存疑 / ❓不懂 / 🗑过时
- 4 色高亮：yellow / red / green / blue
- **4 个 REST 路由**：POST / GET list / PATCH / DELETE
- **Reader 模态**：1080px 宽，左正文按偏移切片渲染高亮 + 右栏批注卡片（source dot 🔵 user / 🟣 ai 区分）
- **选中文字触发 popup**：5 标签按钮 + 4 色圆点 + 附注文本框 + 保存/取消
- 点高亮定位右栏卡片（scrollIntoView）

### Batch A.2：AI 批注（4 角度）

- 新模块 `attune_core::ai_annotator` —— LLM 驱动的批注生成器
- 4 个角度：⚠️ 风险 / 🕰 过时 / ⭐ 要点 / 🤔 疑点，各自独立 prompt + 默认色
- **三阶段 snippet 定位**：verbatim → 空白/全角半角归一化 → 前 10 字 prefix anchor（段落边界截断防越界）
- **JSON salvage 解析**：对 Ollama 截断响应，栈扫描 `{...}` pairs 逐个尝试 `serde_json::from_str::<RawFinding>`
- 字段 alias 兼容：`snippet` / `snpshot` / `text` / `quote` 都接收
- UTF-16 code unit 偏移（与前端 JS `String.length` 对齐）
- **Reader 模态新增** "🤖 AI 分析 ▾" 下拉：4 角度各标注"本地 · 约 15s"，分析中显示 loading 条
- AI 分析期间用户关 reader → 服务端批注仍落库；UI 静默无错误 toast（pinnedItemId 闭包保护）

### Batch B.1：上下文压缩流水线 + Token Chip

- **新表** `chunk_summaries`：`(chunk_hash, strategy)` 复合主键 · 加密 summary BLOB · 冗余 item_id 支持 soft-delete 级联
- 新模块 `attune_core::context_compress` —— Chat 前的 chunk 摘要化
- 3 种 strategy：`raw`（透传）/ `economical`（~150 字）/ `accurate`（~300 字+原文头）
- **三阶段锁释放**（chat route）：Phase 1 持锁查 cache → Phase 2 **无锁**跑 LLM → Phase 3 持锁批量写回
- **hash 源修复**：用全量 `content` 而非 `allocate_budget` 截断后的 `inject_content`（否则每次查询 hash 都不同，缓存永不命中）
- `needs_writeback` 标记只回写新生成摘要，跳过 cache hit 的冗余 REPLACE
- **Token Chip**：Chat 输入框旁常驻，实时估算 input token + 云端 $ 价格
  - 本地绿 🟢 免费 · 云端琥珀 🟡 带 $ 金额
  - CJK 1.2 tok/char（BPE 实测校正）· ASCII 0.25 tok/char
  - Tooltip 明示"仅 input · 2026-04 参考价 · 以 provider 账单为准"

### Batch B.2：批注加权 RAG + Token Chip 展开

- 新模块 `attune_core::annotation_weight` —— 🆓 零成本层（仅 DB 读 + 算数）
- `ScoreAdjust { Drop | Multiply(f32) }` + `compute_adjust(&[Annotation])`
- **精确 label 白名单**（避免子串匹配 footgun，如 "非过时" 触发 Drop）：
  - DROP: "过时" / "🗑过时" / "🕰 过时"
  - STRONG ×1.5: "重点" / "⭐重点" / "要点" / "⭐ 要点" / "风险" / "⚠️ 风险"
  - MEDIUM ×1.2: "待深入" / "存疑" / "不懂" / "疑点"（含对应 emoji 前缀变体）
- 多批注取 MAX 不累乘
- Chat 响应新增 `weight_stats { items_total, items_boosted, items_dropped, items_kept }` + `compression_stats`
- **Token Chip 展开 popover**：点击 chip 显示上次对话的"检索候选 / 最终注入 / boost / 剔除 / 压缩策略 / 缓存命中 / 原文字符"明细
- `items_kept = items_total - items_dropped` 解决"检索到 5 条但 chat 看到 3 条"的 UI 歧义

### 测试 / 回归

- 单元测试 **213 → 299**（+86），零回归
- 完整 Playwright 回归：**10 Phase / 57 显式断言 / 100% 通过**（最新报告见 `docs/e2e-test-report.md`）
- 每个 batch 两轮独立 code review，共 **12 轮审查**
  - 关闭 6 轮审查中的 **34+ 个 CRITICAL/IMPORTANT 问题**
  - 包括：prefix-anchor 终点越界 · soft-delete 孤立批注 · 子串匹配 footgun · vault 锁饥饿 · spawn_blocking silent drop · allocate_budget 导致缓存永不命中 · CJK token 2× 低估 · 等

### 契约守护

本次实现**贯彻**"成本感知与触发契约"（新增至 CLAUDE.md）：
- 🆓 层：批注 CRUD · 批注加权 · cache 命中 · OCR · RAG 检索
- ⚡ 层：embedding / 基础 classify / 首次摘要（建库阶段后台跑）
- 💰 层：Chat / AI 批注分析 / 深度分析（**必须用户显式触发**，永不后台偷跑）

所有 LLM 调用点全部审查：确认仅由用户点击路径触发（Chat 发送按钮 / AI 分析下拉），
**建库管道（ingest / upload / 文件夹监听 / classify worker / skill evolver）零 LLM 调用新路径**。

---

## Chat Session Management (2026-04-14)

### Chat Session Management

- POST /api/v1/chat 新增可选 `session_id` 字段，不传时自动创建新会话并返回 `session_id`
- GET /api/v1/chat/sessions — 分页获取会话列表（按 updated_at DESC）
- GET /api/v1/chat/sessions/:id — 获取会话详情 + 消息历史（内容字段级解密）
- DELETE /api/v1/chat/sessions/:id — 删除会话及其消息（CASCADE）
- 修复 chat.rs 中 search_with_context 管道；reranker 条件逻辑修复
- 消息内容字段级 AES-256-GCM 加密存储

### 测试

- 新增 3 个 Session CRUD 集成测试（`attune-server/tests/session_test.rs`）：lifecycle / pagination / updated_at 时序
- 总计 **213 tests**（attune-core: 174 + attune-server 各测试套件合计 39）

---

## Search Enhancement + Queue Worker + WebSocket (2026-04-14)

### 搜索增强

- **Reranker**：`VectorIndex::get_vector()` 取 item 均值向量，`rerank()` 以 `0.7×cosine + 0.3×rrf` 二次排序，当 `top_k <= 20` 时自动启用
- **LRU 搜索缓存**：256 条目、30s TTL，djb2 哈希键，命中时响应含 `"cached": true`；ingest 时自动清空
- **GET /api/v1/items/stale**：按 `days`（默认30）返回超期未更新条目，路由顺序在 `{id}` 之前
- **GET /api/v1/items/{id}/stats**：返回 chunk_count / embedding_pending / embedding_done 统计（无需解密内容）
- **POST /api/v1/feedback**：接收 `relevant/irrelevant/correction` 三种反馈，写入 feedback 表（含 CHECK 约束）

### Queue Worker + WebSocket

- **QueueWorker 自动启动**：vault setup/unlock 后通过 AtomicBool CAS 保证单实例启动，vault lock 后退出并重置 flag
- **WebSocket /ws/scan-progress**：每 2 秒推送 `{vault_state, pending_embeddings, pending_classify, bound_dirs}`，vault 锁定时持续推送锁定状态
- **Web UI 进度卡**：首页状态页新增实时进度显示，WebSocket 断线自动重连（clearTimeout + 3s 回退）

### 测试

- 新增约 17 个测试，总计 **156 tests**（attune-core: 144 + attune-server: 12）

---

## Phase 4 增量：搜索质量提升 + 本地推理层 (2026-04-14)

### Phase 4 增量：搜索质量提升 + 本地推理层

- `attune-core/src/infer/`: 新增本地 ONNX 推理模块（ort 2.x）
  - `OrtEmbeddingProvider`: Qwen3-Embedding-0.6B INT8，mean-pool + L2 归一化
  - `OrtRerankProvider`: bge-reranker-v2-m3 INT8，cross-encoder sigmoid 评分
  - `model_store`: hf-hub 自动下载，`~/.local/share/rust/models/` 缓存
  - `provider`: EP 自动选择（CUDA > CPU，`NPU_VAULT_EP` 环境变量覆盖）
- `platform.rs`: 新增 `models_dir()`, `NpuKind`, `detect_npu()`
- `search.rs`: `SearchParams` + `SearchContext` + `search_with_context` 三阶段管道
  - 修复：向量搜索硬编码 10 的 bug
  - Chat 和 Search 路径统一使用 `search_with_context`
- `llm.rs`: 新增 `OpenAiLlmProvider`（OpenAI-compat，支持 Ollama/OpenAI/LM Studio/vLLM）
- `routes/search.rs`: 新增 `initial_k` / `intermediate_k` 可选 query 参数
- `routes/chat.rs`: 修复 500 字符截断 bug（RAG 上下文不再被强制截断）

---

## Test Coverage Expansion (2026-04-14)

### 测试覆盖补全

- **Python 测试环境修复**：创建 `pytest.ini`（`pythonpath = src`），解决 `ModuleNotFoundError`，78 个测试正常收集
- **store.rs 单元测试**（+18）：3 个新模块覆盖 `bind_directory`、`unbind_directory`、`update_dir_last_scan`、`get/upsert_indexed_file`、完整 embedding 队列生命周期（enqueue/dequeue/done/failed/pending/checkpoint）
- **attune-server 集成测试框架**（+13）：导出 `build_router` 函数，`tests/server_test.rs` 通过 axum Router 直连测试核心路由；覆盖 vault 状态、setup/lock/unlock、ingest（成功/锁定403）、items（列表/查询/404/锁定403）

### 测试

- 总计 **197 tests**（attune-core: 157 + server_test: 13 + 集成测试: 27）

---

## Security Hardening (2026-04-13)

### 安全修复

- **CORS 白名单**：将 `CorsLayer::permissive()` 替换为仅允许 `chrome-extension://`、`localhost`、`127.0.0.1` 的白名单，并启用 `allow_credentials(true)`
- **Bearer token 默认开启**：`--require-auth` 默认值改为 `true`，新增 `--no-auth` 反向 flag（仅用于本地开发，启用时打印警告）
- **device-secret + change-password 强制认证**：`/api/v1/vault/device-secret/export`、`/api/v1/vault/device-secret/import`、`/api/v1/vault/change-password` 三个端点无论 `--no-auth` 状态均强制要求 Bearer token
- **NAS 模式 TLS 警告**：绑定非 loopback 地址且无 TLS 时，启动时输出 `⚠ WARNING`
- **路径边界验证**：`bind_directory` 新增 3 层验证（绝对路径、`canonicalize()` 规范化、home 目录边界），防止绑定 `/etc`、`/proc` 等系统目录
- **Zeroizing 中间缓冲**：`derive_master_key` 中的 password+device_secret 拼接 Vec 改用 `Zeroizing<Vec<u8>>`，函数返回前自动清零敏感数据
- **Token 吊销机制**：`lock()` 调用时递增 `token_nonce`（存储于 vault_meta），`verify_session` 验证 nonce 一致性，lock 后旧 token 立即失效
- **change_password 事务保护**：4 次 `set_meta` 写入（salt + 3 个 DEK）包进单个 SQLite 事务，防止中途失败导致数据不一致

### 测试

- 新增 38 个测试，总计 **138 tests**（attune-core: 129 + attune-server: 9）

---

### v0.5.0 — 全量子系统完成 (B + C + D + E + F1 + F3 + F4)

**子系统 B — 行为画像**:
- `search_history` + `click_events` 表，查询加密存储
- `Store::log_search`, `recent_searches`, `log_click`, `popular_items`
- API: `/behavior/click`, `/behavior/history`, `/behavior/popular`

**子系统 C — Web UI MVP**:
- 8 个标签页（搜索/录入/条目/分类/聚类/远程/历史/设置）
- 设置页新增：分类队列 drain、Profile 导出/导入
- 远程标签：WebDAV 目录绑定表单
- 历史标签：搜索历史 + 热门条目

**子系统 D — 运行时插件加载**:
- `Taxonomy::load_user_plugins(config_dir)` 从 `{config_dir}/plugins/*.yaml` 加载
- `/plugins` 端点区分 `source: builtin/user`
- init_search_engines 自动加载用户插件

**子系统 E — 画像导出/导入**:
- `GET /profile/export` 导出 VaultProfile JSON（tags + clusters + histograms）
- `POST /profile/import` 导入（合并语义，跳过不存在的 item_id）
- 用于跨设备迁移分类结果

**子系统 F1 — NAS WebDAV 远程目录**:
- `scanner_webdav.rs` — PROPFIND 列表 + GET 下载 + 增量去重
- `POST /index/bind-remote` 绑定 WebDAV URL 并扫描
- reqwest blocking client，支持 Basic Auth

**子系统 F3 — 分类队列 drain**:
- `AppState::drain_classify_batch(batch_size)` 手动处理分类任务
- `POST /classify/drain` 端点（替代后台线程，回避 Vault 所有权重构）

**子系统 F4 — 索引持久化加密**:
- `crypto::save_encrypted_file / load_encrypted_file` — AES-256-GCM 文件加密通用 helpers
- `VectorIndex::save_encrypted / load_encrypted` — usearch 索引打包 + 加密（长度前缀格式）
- tantivy 继续内存重建策略（从 items.content 恢复）

**子系统 F2 — Tauri 脚手架（待激活）**:
- `crates/attune-tauri/` 目录含 README + Cargo.toml.template + main.rs.template
- 文档化激活路径和架构方案

**测试**: 120 tests (114 unit + 6 integration), +11 from v0.4.0
**二进制**: attune-server 28 MB (+1 MB)

---

### v0.4.0 — 子系统 A: AI 自动分类

**attune-core 新增 5 个模块**:
- `llm.rs` — Ollama chat client，支持 qwen2.5 / llama3.2 / phi3 自动探测
- `taxonomy.rs` — 核心 5 维 + 通用扩展 3 维 + 插件机制，YAML 定义
- `classifier.rs` — 批量 LLM 分类 pipeline，MockLlmProvider 单元测试
- `clusterer.rs` — HDBSCAN 聚类 + LLM 命名
- `tag_index.rs` — 内存反向索引，unlock 时构建

**内置插件**:
- 编程/技术 (tech): stack_layer + language_tech + design_pattern
- 法律 (law): law_branch + doc_type + jurisdiction + risk_level

**HTTP API 新增**:
- `POST /classify/{id}`, `POST /classify/rebuild`, `GET /classify/status`
- `GET /tags`, `GET /tags/{dimension}`
- `GET /clusters`, `GET /clusters/{id}`, `POST /clusters/rebuild`
- `GET /plugins`

**Web UI**:
- 新增"分类"标签页：维度选择器 + 直方图浏览 + 重分类触发
- 新增"聚类"标签页：聚类卡片网格 + 重建按钮

**Store 迁移**:
- `embed_queue` 表新增 `task_type` 列（幂等迁移）
- 新方法: `update_tags`, `get_tags_json`, `enqueue_classify`, `list_all_item_ids`, `mark_task_pending`

**硬依赖**:
- 分类功能需要 Ollama 运行 + chat 模型（qwen2.5:3b 推荐）
- 无 chat 模型时分类端点返回 503，其他功能正常

**测试**: 28 unit + 3 integration = **109 tests** (103 attune-core unit + 6 integration)

**二进制大小变化**:
- attune-server 从 26 MB 增至约 27 MB（hdbscan crate + 插件 YAML）

---

### v0.3.0 — Phase 3: NAS 模式 + Web UI + Device Secret 迁移

**TLS + NAS 模式**：
- `axum-server` + `rustls` 纯 Rust TLS 栈，无 OpenSSL 依赖
- CLI 参数 `--tls-cert` / `--tls-key` 启用 HTTPS
- CLI 参数 `--require-auth` 启用 Bearer token 认证
- `bearer_auth_guard` 中间件：远程请求需携带 `Authorization: Bearer <session_token>`
- 公共白名单：`/status/health`, `/`, `/ui/*`, `/vault/setup`, `/vault/unlock`, `/vault/status`
- 双层中间件顺序：bearer_auth_guard → vault_guard → CORS

**嵌入式 Web UI**：
- 单页 HTML + vanilla JS，`include_str!` 编译进二进制
- 四个标签页：搜索 / 录入 / 条目 / 设置
- 响应式布局，移动浏览器友好
- DOM 纯脚本操作，无 innerHTML XSS 风险
- 支持 setup / unlock / lock、搜索、录入、条目列表、Device Secret 导出

**Device Secret 导出/导入**：
- `Vault::export_device_secret()` — 返回 64 字符 hex（32 字节），仅 UNLOCKED 状态
- `Vault::import_device_secret(hex)` — 导入前校验长度，写入 0o600 权限文件
- API: `GET /vault/device-secret/export` + `POST /vault/device-secret/import`
- 多设备迁移流程：导出旧设备的 device.key → 新设备 import → 用原密码 unlock → 数据解锁

**测试**: 75 unit + 3 integration = **78 tests**（vault 模块 13 → 16，新增 `export_device_secret_requires_unlocked`, `import_device_secret_writes_file`, `import_invalid_hex_fails`）

**二进制**: attune-cli 4.1 MB + attune-server 26 MB（TLS + Web UI 增量约 17 MB）

---

### v0.2.5 — 搜索集成 + Chrome 扩展兼容

**AppState 搜索引擎生命周期**：
- `AppState` 新增 `Mutex<Option<FulltextIndex>>` / `Mutex<Option<VectorIndex>>` / `Mutex<Option<Arc<dyn EmbeddingProvider>>>`
- `init_search_engines()` 在 `vault_setup` / `vault_unlock` 后调用：创建 FulltextIndex、VectorIndex(1024)、OllamaProvider
- `clear_search_engines()` 在 `vault_lock` 前调用：全部置 None
- 修复 OllamaProvider 嵌套 tokio runtime panic：搜索路由用 `spawn_blocking` 调用

**搜索路由集成**：
- `GET /search` 真实 tantivy BM25 + usearch 向量 + RRF 融合 + SQLite 解密
- `POST /search/relevant` 同上 + `allocate_budget()` 注入预算分配，返回 `inject_content`
- 搜索结果格式对齐 Chrome 扩展 `SearchResult` 接口

**Ingest 链路补全**：
- ingest 时同步加入 tantivy 全文索引
- 两层 embedding 入队：Level 1 章节 (`extract_sections`) + Level 2 段落 (`chunk`)

**Chrome 扩展兼容**：
- 补全 `/api/v1/items/{id}` PATCH（更新 title/content）
- 补全 `/api/v1/settings` GET/PATCH（存于 vault_meta，合并语义）
- 完整 18 个 API 端点覆盖 attune Python 原型协议

**测试**: 72 unit + 3 integration = 75 tests（保持不变）

---

### v0.2.0 — Phase 2b: 文件扫描 + Embedding 队列 + Upload API

**scanner.rs 文件扫描引擎**：
- `scan_directory()` — walkdir 递归/非递归遍历，file_types 过滤
- `process_single_file()` — SHA-256 hash 比对 indexed_files，未变化跳过，新增/变更入库
- `create_watcher()` / `watch_directory()` — notify-rs 实时监听（CrossPlatform）
- 只读保证：`File::open(Read)`，永不修改源文件
- 两层入队：Level 1 章节（priority-1）+ Level 2 段落块（priority=2）

**queue.rs Embedding 队列 Worker**：
- `QueueWorker::start()` — 后台线程轮询 pending 任务，批量 embed
- `QueueWorker::process_all()` — 同步处理（测试用）
- 批次大小 10，轮询间隔 2 秒，失败重试 3 次
- 结果写入 VectorIndex（所有 level）+ FulltextIndex（仅 Level 1 章节）

**attune-server 新增路由**：
- `POST /api/v1/index/bind` — 绑定目录 + 触发全量扫描
- `DELETE /api/v1/index/unbind` — 解绑目录（软删除）
- `GET /api/v1/index/status` — 绑定目录列表 + pending embedding 数
- `POST /api/v1/upload` — multipart 文件上传（最大 20 MB）

**Store 新增方法**：
- `bind_directory` / `unbind_directory` / `list_bound_directories` / `update_dir_last_scan`
- `get_indexed_file` / `upsert_indexed_file`
- `enqueue_embedding` / `dequeue_embeddings` / `mark_embedding_done` / `mark_embedding_failed` / `pending_embedding_count`

**测试**: 72 unit + 3 integration = 75 tests

---

### v0.1.5 — Phase 2a: Axum API Server + 搜索引擎基础

**attune-core 新增 6 个模块**：
- `chunker.rs` — 滑动窗口分块 + `extract_sections` 语义章节切割（Markdown 标题 / 代码 def / 段落）
- `parser.rs` — MD / TXT / 代码解析 + `parse_bytes` 内存解析 + `file_hash` SHA-256
- `embed.rs` — `EmbeddingProvider` trait + `OllamaProvider` (reqwest HTTP) + `NoopProvider` 降级
- `index.rs` — tantivy 0.22 全文索引封装，`tantivy-jieba` 中文分词，ReloadPolicy::Manual
- `vectors.rs` — usearch HNSW + cosine + f16 量化，外部 HashMap metadata 映射
- `search.rs` — RRF 融合（k=60）+ 动态注入预算（按 score 比例 + 最低 100 字保底）

**attune-server 新 crate**：
- Axum 0.8 HTTP server，Tokio 异步运行时
- `AppState = Mutex<Vault>` 共享状态
- `vault_guard` 中间件 — UNLOCKED 检查，SEALED/LOCKED 时返回 403
- 路由模块：vault / ingest / items / search / index / upload / status
- CORS 全开放（供 Chrome 扩展跨域调用）
- clap CLI args: `--host` / `--port`

**测试**: Phase 1 的 34 unit + 新增 28 unit (chunker:6, parser:6, embed:2, index:4, vectors:5, search:5) = 62 unit + 3 integration = **65 tests**

**二进制**: attune-cli 4.1 MB + attune-server 9.0 MB（尚未含 TLS）

---

### v0.1.0 — Phase 1: 加密存储引擎

**Cargo workspace 初始化**：
- `attune-core` library crate — 核心加密和存储
- `attune-cli` binary crate — 命令行管理工具

**attune-core 5 个基础模块**：
- `error.rs` — `VaultError` 统一错误类型（13 个变体），thiserror 派生，`Result<T>` 别名
- `platform.rs` — 跨平台路径：`data_dir()` / `config_dir()` / `db_path()` / `device_secret_path()`
- `crypto.rs` — 纯密码学原语：
  - `Key32` 32 字节密钥，`ZeroizeOnDrop` Drop 时清零
  - `derive_master_key` — Argon2id (m=64MB, t=3, p=4)
  - `encrypt` / `decrypt` — AES-256-GCM，格式 `nonce(12B) ‖ ciphertext ‖ tag(16B)`
  - `encrypt_dek` / `decrypt_dek` — DEK 加解密
  - `hmac_sign` / `hmac_verify` — HMAC-SHA256
- `store.rs` — rusqlite SQLite 封装：
  - Schema: vault_meta, items, embed_queue, bound_dirs, indexed_files, sessions
  - `PRAGMA journal_mode=WAL` + `foreign_keys=ON` + `busy_timeout=5000`
  - 字段级加密 CRUD：`insert_item` 加密 content/tags，`get_item` 解密返回
  - `checkpoint()` 刷 WAL 到主 DB（供加密验证测试使用）
- `vault.rs` — 顶层编排：
  - `VaultState` enum: Sealed / Locked / Unlocked
  - `setup(password)` — 生成 device.key (0o600) + salt + 3 DEK，自动 unlocked
  - `unlock(password)` — 校验 device_secret_hash → 派生 MK → 解密 DEK → 签发 session token
  - `lock()` — `UnlockedKeys` Drop → Key32 zeroize
  - `change_password(old, new)` — 重新加密 DEK，业务数据不动
  - `verify_session(token)` — HMAC 验证 + 过期检查

**attune-cli 7 个子命令**：
- `attune setup` — 首次设置主密码（`rpassword` 无回显输入 + 二次确认）
- `attune unlock` — 解锁 vault
- `attune lock` — 锁定 vault
- `attune status` — JSON 输出状态 + 条目数 + 路径
- `attune insert -t -c -s` — 插入知识条目
- `attune get <id>` — 获取单条目（解密）
- `attune list -l` — 列出条目摘要

**集成测试**：
- `e2e_full_lifecycle` — setup → insert → lock → unlock → verify → change_password → delete
- `e2e_content_encrypted_at_rest` — 验证 SQLite 原始字节不含明文
- `e2e_multiple_items` — 批量插入 + 分页

**测试**: 34 unit + 3 integration = 37 tests

**二进制**: attune-cli 3.8 MB（初版，仅 CLI）

---

## 路线图

> v0.6.0 Tauri 桌面客户端已发布（见上方 GA 章节），从路线图移除。

### v0.7.0 — Queue Worker 自动启动 + WebSocket 推送

- attune-server 启动时自动 `QueueWorker::start()`，在 unlock 后开始消费队列
- WebSocket `/ws/scan-progress` 推送扫描进度 + embedding 进度
- Web UI 实时显示后台处理状态

### v0.8.0 — 云同步（可选）

- 加密备份到任意 S3 兼容对象存储（或 WebDAV）
- 端到端加密：云端仅看到密文 blob
- 增量同步（按时间戳）

### v1.0.0 — 正式发布

- GitHub Actions CI/CD 全流水线（Linux/Windows/macOS/Android 构建矩阵）
- 安装引导页（首次启动向导）
- 完整中英双语文档
- 官网 + 下载页
- 签名证书（Windows MSI / macOS notarization）
