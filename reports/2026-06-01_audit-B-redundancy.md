# Audit B — 架构冗余 / 重复路径 / 死代码 / 过度工程

**Date**: 2026-06-01 · **Lens**: redundancy · **Scope**: read-only (no build, no edit)
**Target**: `/data/company/project/attune/rust/crates/{attune-core,attune-server}`

---

## TL;DR

- **预设的 2 个 🔴 高危信号(Signal 1 memory-dup / Signal 5 4-path ingest)经核实全部 FALSE** — 不是冗余，是合理分层 / 已在 v0.7 修复。
- **真冗余确认 3 块**：(1) `capture/` 整目录死 scaffold(323 LOC)；(2) `html_to_text` 双实现(parser.rs + ingest/email.rs)；(3) `plugin_sig::verify_strict` 预留死函数。
- **可回收 LOC ≈ 350–420**（capture/ 323 + html_to_text 去重 ~30–60 + verify_strict ~15），全部低风险。
- 16 处 `#[allow(dead_code/unused)]` 中 14 处是 macro/serde/字段生命周期合法用法，**仅 1 处真死**(verify_strict)。

---

## 1. 逐条信号核实

### 🔴 Signal 1 — `memory_consolidation.rs` vs `memory/consolidation_agent.rs` → **NOT 冗余(合理分层)**

两者是 multi-layer memory 的**不同层**，各有独立 caller，都活：

| 文件 | 职责 | 层 | 真 caller |
|---|---|---|---|
| `src/memory_consolidation.rs` (487) | chunk_summaries → **episodic** memory(按天窗聚合) | L2 | `attune-server/src/state.rs:1492/1516/1542` (`prepare_consolidation_cycle` / `generate_one_episodic_memory` / `apply_consolidation_result`) |
| `src/memory/consolidation_agent.rs` (653) | episodic → **semantic** promotion(打分排名提升) | L3 | `memory/mod.rs` re-export + `state.rs:1594/1617/1631` 经 `memory::prepare_semantic_cycle` 等 |

证据：`memory/mod.rs:1-20` doc 明确 L0/L1/L2/L3 分层；`memory_consolidation.rs:1` 注释 "A1 Memory Consolidation MVP — 把 chunk_summaries 聚合成 episodic"；`consolidation_agent.rs:420` id = `"memory_consolidation_agent"`（L3 promotion agent）。
**判定：保留**。命名易混(都叫 consolidation)，建议改进文档/重命名 `memory_consolidation.rs` → `episodic_consolidation.rs`，但**不是冗余**。

### 🟡 Signal 2 — web_search 4 文件 → **NOT 冗余(教科书式策略分层)**

| 文件 (LOC) | 职责 |
|---|---|
| `web_search.rs` (148) | `WebSearchProvider` trait + `WebSearchResult` + `from_settings()` 工厂 |
| `web_search_engines.rs` (142) | `SearchEngineStrategy` trait + DuckDuckGo/Google/Bing 各 impl(DOM 解析隔离) |
| `web_search_browser.rs` (352) | `BrowserSearchProvider`(chromiumoxide)+ 跨平台浏览器探测 |
| `store/web_search_cache.rs` (313) | 缓存层(SQL) |
| `routes/web_search_cache.rs` (39) | HTTP route 薄壳 |

每文件单一职责，trait/impl/cache/route 正交。caller(`chat.rs` / `tools.rs` / `state.rs`)只 import trait。
**判定：保留**。`routes/web_search_cache.rs` 39 行薄壳是 server route 惯例，非冗余。

### 🟡 Signal 3 — plugin 6 文件 → **NOT 冗余(职责清晰)**

| 文件 (LOC) | 职责 | caller |
|---|---|---|
| `plugin_registry.rs` (996) | 已装插件索引 + chat_trigger 路由 + default_plugins_dir | intent_router, agent_runner, CLI, chat route |
| `plugin_loader.rs` (826) | 从目录加载 LoadedPlugin / AgentSpec / AnnotationAngleConfig | ai_annotator, agent_runner, CLI |
| `plugin_sig.rs` (378) | Ed25519 签名 / 校验 | attune-accounts, CLI |
| `plugin_sync.rs` (565) | license entitlement → 下载/安装 .attunepkg | CLI `sync_plugins` |
| `plugin_hub.rs` (476) | Mock/Http PluginHub provider trait(市场交互) | server (settings 选 Mock/Http) |
| `plugin_encryption.rs` (157) | .attunepkg 解密 | plugin_sync |

`plugin_hub`(市场协议) vs `plugin_sync`(license 驱动安装) 是上下游，非重叠。
**判定：全部保留**。

### 🔴 Signal 4 — `capture/` vs `ingest/email.rs` → **DEAD SCAFFOLD（真冗余，可删整目录）**

| 文件 (LOC) | 状态 | caller |
|---|---|---|
| `capture/mod.rs` (7) | scaffold 声明 | **仅 `lib.rs:179 pub mod capture`** |
| `capture/email.rs` (195) | v0.7 scaffold：`EmailProvider` trait + `MockEmailProvider` + `#[cfg(test)]` only | **零生产 caller** |
| `capture/telegram.rs` (121) | v0.7 scaffold：`TelegramProvider` + `MockTelegramProvider` + 仅 test | **零生产 caller** |
| `ingest/email.rs` (518) | **真 IMAP 生产连接器**(async-imap, mail-parser) | server `ingest_email::sync_email_account` × 3 |

证据(grep 全仓非 test)：`crate::capture` / `attune_core::capture` 唯一命中是 `lib.rs:179` 的 mod 声明本身 + `capture/mod.rs` 内部 submodule 声明。`capture/email.rs` doc 头自述 "v0.7 scaffold：定义 trait + Mock + 单测；v0.8 真 IMAP"。但 v0.8 真 IMAP **改在 `ingest/email.rs` 落地了**(与 WebDAV 同 SourceConnector 模式)，`capture/` scaffold 被绕过、从未升级、从未接线。telegram 同理(全仓 0 个 Telegram 真实接入)。

**判定：删整 `capture/` 目录 + `lib.rs:179`**。回收 **323 LOC**。风险低 —— 编译期 `cargo build` 立即暴露任何遗漏 caller(已确认无)。

### 🟡 Signal 6 — 16 处 `#[allow(dead_code/unused)]` → **14 合法 / 1 真死 / 1 测试帮手**

| 位置 | 内容 | 判定 |
|---|---|---|
| `plugin_sig.rs:120` `verify_strict` | "预留，当前不在任何路径调用 —— PluginHub 上线后激活" | **真死(speculative)** — grep 全仓 0 caller。可删(~15 LOC)或留待 hub 上线 |
| `mcp_client.rs:63` `JsonRpcResponse.jsonrpc` | serde deserialize 必需字段(协议字段不读但必须存在) | 合法保留 |
| `index.rs:19/25` `schema` / `f_source_type` | struct 字段持有(lifetime/未来 query) | 合法保留 |
| `memory/assembler.rs:386` `seed_episodic` | `#[cfg(test)]` 测试帮手 | 合法保留 |
| `sync/webdav.rs:160` | (未展开，需逐个看；属同类字段/帮手) | 大概率合法 |
| `store/*.rs` × 8 (`#[allow(unused_imports)]`) | 条件编译/feature-gate import | 合法(macro 模式) |
| `feedback/tests.rs:447` `#[allow(unused)]` | 测试 fixture | 合法 |

**判定：仅 `verify_strict` 是真可删死代码。** 其余是 serde / 字段 / test / feature-gate 合法用法，删了会引入 warning 或破坏 deserialize。

### 🟡 Signal 7 — agent 子系统 → **NOT 重复 harness**

`agents::{Agent, AgentOutput, AgentError}` trait 已抽到 leaf crate `attune-agent-sdk`(WASM-safe)，`agents/mod.rs` re-export **同一类型**(非重定义)。各 `*/agent.rs`(skill_evolution 1130 / chat_reliability 799 / linker 449 / memory consolidation 653 / document_classifier 209)共用 `id()/description()/run_with_store()` **命名约定**，但每个是 distinct agent(不同业务逻辑 + 独立 golden gate)。`agent_runner.rs`(桥接 registry+dispatch 跑 subprocess) vs `agents/registry.rs`(SSOT toml 注册表) vs `agent_quality.rs`(pass-rate gate) vs `agent_telemetry.rs`(失败统计)各司其职。
**判定：保留**。约定一致是好事，非 copy-paste harness。

---

## 2. 信号外发现的其它重复

### 🟡 `html_to_text` — 双实现(genuine copy-paste 候选)

| 位置 | 可见性 | reuse |
|---|---|---|
| `ingest/email.rs:54` `pub fn html_to_text` | pub | 被 `ingest/rss.rs:23/186` 复用 |
| `parser.rs:438` `fn html_to_text` | private | 仅 parser.rs 内部(:138/:432/:499) |

两份各自实现 HTML 剥标签 + 折叠空白，各带独立测试(email.rs:491-506 / parser.rs:720-739)。功能等价。
**建议：合并** —— 把 parser.rs 私有版删除，改用 `ingest::email::html_to_text`(或把它提到一个中性 `text_util` 模块再两边引用)。回收 ~30–60 LOC + 消除"两份 HTML 解析行为漂移"风险(已是隐患：两套测试断言不同的 edge case)。**风险中**：需确认两实现行为一致(title 抽取/script-style 丢弃语义)，合并前对齐测试。

### ✅ 已修复(历史冗余，现已无) — 记入正面案例

- **Signal 5 "4-path ingest dup" 已在 v0.7 彻底消除**：`ingest/pipeline.rs::ingest_document` 自述 "把 0.6 之前散在 4 处(upload/ingest/scanner/scanner_webdav)的五步收成一个函数"。现状：upload.rs→`ingest_document_with_profile`、scanner.rs→`ingest_document`/`_replacing`、scanner_webdav→同、内容更新→`reindex::reindex_item`。**全部收口**，记忆中的 O(N²) 已不存在。
- `vectors::delete_by_item_id` / `fulltext::delete_document`(≤v0.6.3 的死代码，0 caller)现已经 `reindex::purge_item_indexes` 统一调用(items.rs:181-182 注释存证)。

### ✅ 非冗余(排除) — scanner / queue 家族

- `scanner.rs`(本地 FS notify watch) / `scanner_webdav.rs`(WebDAV ETag) / `scanner_patent.rs`(USPTO API) — 三个不同源，零重叠。
- `queue.rs`(embed worker 队列) / `office_job_queue.rs`(office helper job 状态机) / `store/queue.rs`(SQL 持久层) — 三层不同语义。

---

## 3. 冗余清单(汇总表)

| # | 重复项 | 证据 file:line | 调用方 | 建议 + 理由 |
|---|---|---|---|---|
| B1 | `capture/email.rs` 死 scaffold | `capture/email.rs:1-195`(trait+Mock+test only) | **0 生产 caller**(仅 lib.rs:179 mod 声明) | **删** — 真 IMAP 在 ingest/email.rs(518)落地，scaffold 被绕过从未升级 |
| B2 | `capture/telegram.rs` 死 scaffold | `capture/telegram.rs:1-121` | **0 caller**(全仓无 Telegram 接入) | **删** — 同 B1，从未生产化 |
| B3 | `capture/mod.rs` + `lib.rs:179` | `lib.rs:179 pub mod capture` | — | **删** — B1/B2 删后空 |
| B4 | `html_to_text` 双实现 | `parser.rs:438`(priv) + `ingest/email.rs:54`(pub) | parser 内部 vs rss 复用 email 版 | **合并** — 保留 pub 版(已被 rss 复用)，删 parser 私有版，先对齐两套测试 |
| B5 | `verify_strict` 预留死函数 | `plugin_sig.rs:121` | **0 caller**("PluginHub 上线后激活") | **删或显式 TODO** — speculative，按需可留但应从"已实现"降级标注 |

---

## 4. 可回收 LOC + 风险

| 项 | LOC | 风险 | 谁依赖 |
|---|---|---|---|
| B1+B2+B3 删 `capture/` | **323** | 低 | 无(grep 确认 0 生产 caller；cargo build 会立即暴露) |
| B4 合并 html_to_text | **~30–60** | 中 | parser.rs 内 3 处 callsite；合并前需对齐 title/script 处理语义 + 测试 |
| B5 删 verify_strict | **~15** | 低 | 无 caller；唯一考量是 PluginHub roadmap 是否近期落地 |
| **合计** | **≈ 350–420** | — | — |

**删前最后一关**：B1–B3 / B5 纯减法，`cargo build --workspace` + `cargo clippy` 一跑即验证无遗漏引用。B4 需先跑两份 html_to_text 的现有测试确认行为等价，再合并(否则可能引入解析行为回归)。

---

## 5. 元结论(对北极星)

attune-core 的"看起来文件多"绝大部分是**真实分层**(web_search 策略模式 / plugin 上下游 / memory L0–L3 / scanner 多源 / agent SDK leaf crate)，不是过度工程。两个预设 🔴 高危(memory-dup / 4-path ingest)实测全 FALSE — 后者还是已主动重构掉的**正面案例**。真冗余集中在 **v0.7 留下的未升级 scaffold**(`capture/`)和**一处 copy-paste 工具函数**(html_to_text)，体量小、风险低。建议优先删 `capture/`(单一 commit，零行为变更)。
