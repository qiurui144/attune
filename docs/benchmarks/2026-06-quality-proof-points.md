# 2026-06 质量证据点（Quality Proof Points）

> 技术证据汇编（团队工作语言：中文）。本页汇编 attune / attune-pro 经**真实验证**的硬证据，
> 围绕三个维度:**先进性**（架构/算法）、**稳定性**（并发/CI/降级）、**准确性**（agent F1）。
>
> **§6.3 证据纪律**:每条均引 commit SHA / `reports/runs` 日志 / 代码路径:行号。
> 跨仓证据（attune-pro）显式标注来源仓。未达 SSOT 登记或仅 mock 的项标 **PENDING-EXPERT** / **PENDING-VERIFY**，
> **不声称已验证**。日期口径以引用报告为准（撰写日 2026-06-16）。

## 0. 目录

- [1. 先进性（架构 / 算法）](#1-先进性架构--算法)
- [2. 稳定性（并发 / CI / 降级）](#2-稳定性并发--ci--降级)
- [3. 准确性（agent 真 LLM F1）](#3-准确性agent-真-llm-f1)
- [4. PENDING 登记（诚实缺口）](#4-pending-登记诚实缺口)
- [5. 证据来源索引](#5-证据来源索引)

---

## 1. 先进性（架构 / 算法）

### 1.1 深度总结省 token（map-reduce + 编译期阈值锁）

| 项 | 实测 | 来源 |
|----|------|------|
| 长文档冷读节省（by-token） | long-en 56.4% / long-zh-50ch 38.0% / long-zh-30ch 33.9% | `reports/2026-06-06_deepsum-savings.md`（attune 本仓） |
| 长文档**暖缓存**二次读节省 | long-zh-50ch **96.2%** / 30ch 95.0% / en 93.3%（map 调用 0） | 同上 §35-46 |
| 8 文档冷读分布 | mean 23.8% ± 16.3%（n=8） | 同上 §22 |
| 阈值防漂移 | `DEEPSUM_MIN_TOK=1500` 编译期 `const assert!` 锁在 [441, 9873) 证据带 | `document_intelligence/deep_summary.rs:80-87` |

诚实结论:spec §9.1 原"冷跑 ≥80% 文档 ≥60%"目标**结构上不可达**（MAP 必读 extractive 候选），已 G3 上报人工。
该 harness 真正 gate 的是 `actual ≤ naive`（账单正确性）、暖≥冷、长文 re-read ≥60%（report §48-70）。
机制详解 → `docs/wiki/memory-token-economy.md`。

### 1.2 共享视觉理解架构（先进性 = 单一视觉核心，非 per-plugin fork）

VLM 薄适配器复用任意 `LlmProvider` 的 `chat_multimodal`，不重写 vision 协议（`attune-core/src/vlm.rs:33-84`）；
per-stage 模型路由（cheap/reasoning/vision 分组，非"tier=最贵"，`document_intelligence/model_routing.rs:1-27`）。
图像源才走 VLM、文本源 0 VLM 调用（`vlm_extract.rs:51-79` 测试断言）。架构详解 → `docs/wiki/vision-understanding-pipeline.md`。

---

## 2. 稳定性（并发 / CI / 降级）

### 2.1 Store::open 并发开库竞争修复 ⭐（attune 本仓）

**commit `8b76e13`** `fix(store): office routes 503-not-404 on fresh runner — Store::open not concurrent-open safe`

- **根因（已证，非 SQLite-busy 假设）**:同一 `vault.db` 在 boot 时被并发打开（install_job_store +
  init_search_engines + workers；office 测试经 process-global `set_var(XDG_DATA_HOME)` 放大）。两处缺陷:
  (1) check-then-`ALTER ADD COLUMN` 非原子 TOCTOU → 输方 `duplicate column name: task_type`；
  (2) fresh-vault VACUUM 无 busy_timeout → `database is locked`。任一使 `Store::open` Err → job_store=None →
  office ASR/OCR 路由返 503 而非 404。
- **修复（治本）**:(a) per-DB-path 进程内 open 锁串行化 create+WAL+VACUUM+migration 临界区（sub-ms，
  稳态连接不受影响）；(b) SCHEMA_SQL + migrations 包进单个 `BEGIN IMMEDIATE` 事务；(c) autovacuum 加 busy_timeout +
  fresh-vault VACUUM 失败视为良性。
- **证据**:新回归测试 `store::tests::concurrent_open_same_fresh_path_all_succeed` 原码 **3/3 FAIL → 修后 5/5 PASS**；
  8×20 并发开库压测 **46 errors → 0**；office offline 套件 82 tests green（无 SIGBUS）；275 store unit tests pass；
  0 new clippy。报告 `reports/2026-06-16-office-model-absence-fix.md`。

### 2.2 CI 4 根因修复（attune 本仓，近期 develop）

| commit | 内容 | 根因类型 |
|--------|------|---------|
| `8b76e13` | office 路由 503→404（见 2.1） | 并发开库竞争 |
| `3524c2d` | office ubuntu shards fail — job-wide `HF_HUB_OFFLINE` | 请求路径误下载 hf-hub 模型 |
| `2fa431a` | scenarios/plugins 路由测试 Windows CI fail — XDG override 仅 Linux | 跨平台 test 隔离 |
| `22eec99` | office-tests 阻断 hf-hub 下载 + 隔离阻塞 probe | 测试请求路径下载 |

### 2.3 全链降级（graceful degrade，§4.5.E）

VLM provider 不支持 vision→文本退化、单 map 块失败→extractive 兜底、压缩 LLM 不可用→截断原文、
retry 耗尽→Err 不 panic、memory bundle 失败→None 不影响其他。代码路径表见
`docs/wiki/vision-understanding-pipeline.md` §4。均有 `#[test]` 断言（如 `deep_summary.rs:502-523` /
`context_compress.rs:373-381` / `vlm.rs:267-274`）。

### 2.4 记忆延续 TDD（attune 本仓）

- **memory continuity golden gate**:12 golden case（`tests/golden/memory_continuity/01..12`）+ 13 个 golden_gate
  测试函数（`tests/memory_continuity_golden_gate.rs`）+ 7 个 E2E 函数（`tests/memory_continuity_e2e.rs`）。
  覆盖 reindex recall + export/import round-trip + 错误口令/损坏 bundle。
- **organize（文件夹→案卷聚类）**:12 golden case（`tests/golden/organize/01..12`）+ golden gate
  （`tests/organize_golden_gate.rs`）+ route/e2e 测试（`tests/organize_route_test.rs` 7 用例、
  `tests/organize_e2e_test.rs` 3 用例，含 idempotent + heavy bind-ingest-organize 全链）。
- 幂等性:`memory_consolidation::apply_is_idempotent_on_rerun`（`memory_consolidation.rs:451-470`）、
  reindex 缓存一致性 `reindex_purges_chunk_summaries`（`reindex.rs:153-171`）。

---

## 3. 准确性（agent 真 LLM F1）

> 来源仓 = **attune-pro**（私有商业线）。OSS attune 当前无 domain agent（chat/search/RAG 是 base capability）。
> 真 LLM = DeepSeek（per §4.5H 产品内嵌 agent 默认），multi-seed N=3（per §2.3）。

| agent（插件） | 真 DeepSeek F1 | seed | 状态 | 来源 |
|---------------|----------------|------|------|------|
| code_reviewer（tech-pro） | **mean 0.958** | N=3 nightly | PASS（6 类下限齐 + G7 语义 grounding） | attune-pro `reports/2026-06-15-tech-pro-gap-audit.md:54-55` |
| claim_extractor（patent-pro） | **1.00** | 真 DeepSeek 2026-06-15 | PASS（≥0.85 floor） | attune-pro `reports/2026-06-15-pro-plugins-completion-roadmap.md:19` |
| term_translation（patent-pro） | **micro_F1 0.9796**（grounding_precision 1.0） | seed 1 | 见 §4 注（口径待 SSOT 登记） | attune-pro `reports/runs/20260611T073217Z_patent-pro-r13-official/term-multiseed.log:7` |
| defamation_extractor（law-pro） | 0.90 PASS（worst-seed 0.8764 watch） | multi-seed | PASS（旧 qwen2.5:3b 的 0.72 已被真 DeepSeek 取代） | attune-pro `reports/2026-06-15-pro-plugins-completion-roadmap.md:19` |
| medical_dispute（law-pro） | 0.963 | multi-seed | PASS | 同上 |
| record_structure（patent-pro） | 1.00 | multi-seed | PASS | 同上 |

**code_reviewer G7 grounding 增强**（commit `f0c4fa5`，attune-pro）:把 grounding 从"line_refs 存在"扩展到
**语义校验** + over-report trim + 16-case hardened holdout（real-DeepSeek N=3）。
**claim_extractor 强化**（commit `0b4830f`，attune-pro）:注册 claim_extractor real-LLM N=3 + harder cases（test-fix-verify）。

> ⚠️ §6.3 口径更正:任务下发文案中的 "code_reviewer F1 0.9434→0.9697" 与 on-disk 证据**不符**——
> 0.9434 实为 **medical-pro symptom_extract**（0.8025→0.9434，commit `caf39c7`）的数；code_reviewer 真实口径为
> real-DeepSeek **mean 0.958**。本页采信 on-disk 报告口径。

---

## 4. PENDING 登记（诚实缺口）

| 项 | 状态 | 说明 |
|----|------|------|
| **exam_divergence（patent-pro）** | **PENDING-EXPERT** | 0.8889 在 on-disk 证据中**查无该 agent 此分**（0.889 命中均为 law defamation/medical-dispute 的单 case）。2026-06-11 health 报告判定 exam_divergence **UNGATED / 零 domain golden**（`reports/2026-06-11-project-health.md:82`），仅 mock gate。真值待 patent domain expert 提供 GT 后方可登记。**不声称 0.8889 已验证。** |
| **claim_extractor 标准 CI** | nightly `#[ignore]` | 标准套件 `claim_extractor_real_llm_micro_f1_floor` 是 `#[ignore]` real-DeepSeek nightly lane（需 `ATTUNE_CHAT_*` + secrets），CI deterministic lane 跑 mock=1.0。真 1.00 来自 2026-06-15 真 DeepSeek 单次运行。 |
| **term_translation SSOT** | 部分 | 0.9796 有 runs 日志，但 2026-06-11 health 曾标该 agent P0 零 golden；2026-06-15 update 报 PASS。RELEASE.md SSOT 登记一致性待最终核对。 |
| **medical-pro symptom_extract** | below-floor watch | 0.8073 ± 0.047 < 0.85 floor（nightly data-collection lane，非阻断），attune-pro 唯一真未达标项。 |
| **prompt cache（§4.5G）** | PENDING-VERIFY | attune-core 源码未检索到 `cache_control: ephemeral` 显式开关；当前省 token 靠应用层 chunk_summary 缓存，非 provider prefix cache。 |

---

## 5. 证据来源索引

**attune 本仓**:
- `reports/2026-06-06_deepsum-savings.md` — deep-summary token 节省 T-12
- `reports/2026-06-16-office-model-absence-fix.md` — Store::open 并发修复（commit `8b76e13`）
- 代码:`crates/attune-core/src/document_intelligence/{deep_summary,token_bill,model_routing}.rs`、
  `context_compress.rs`、`reindex.rs`、`memory_consolidation.rs`、`vlm.rs`、
  `document_intelligence/vlm_extract.rs`、`llm.rs`
- 测试:`crates/attune-core/tests/{memory_continuity_golden_gate,organize_golden_gate}.rs`、
  `crates/attune-server/tests/{memory_continuity_e2e,organize_route_test,organize_e2e_test}.rs`

**attune-pro 仓**（跨仓，私有商业线）:
- commit `f0c4fa5`（code_reviewer G7 grounding）、`0b4830f`（claim_extractor real-LLM N=3）、`caf39c7`（symptom_extract）
- `reports/2026-06-15-tech-pro-gap-audit.md`、`reports/2026-06-15-pro-plugins-completion-roadmap.md`、
  `reports/2026-06-11-project-health.md`、`reports/runs/20260611T073217Z_patent-pro-r13-official/term-multiseed.log`

---

## 关联

- 记忆与省 token 架构 → `docs/wiki/memory-token-economy.md`
- 视觉理解与稳定输出流水线 → `docs/wiki/vision-understanding-pipeline.md`
- 历史 / dual-track benchmark → `docs/benchmarks/README.md`
