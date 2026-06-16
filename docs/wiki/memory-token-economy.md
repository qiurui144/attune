# 记忆与省 token 架构（Memory & Token Economy）

> 技术专题文档（团队工作语言：中文）。本页讲清 attune **如何省 token**：
> 通过分层缓存、深度总结、上下文压缩、可见计费四个相互叠加的机制，把"把整篇文档塞进
> prompt"的 naive 成本降下来，并把节省量**测量出来而非声称出来**。
>
> 所有数据均引代码路径:行号 或 `reports/runs` 日志（per CLAUDE.md §6.3）。

## 0. 目录

- [1. 设计原则:成本分层 + 缓存复用](#1-设计原则成本分层--缓存复用)
- [2. chunk 摘要缓存复用（chunk_hash）](#2-chunk-摘要缓存复用chunk_hash)
- [3. 上下文压缩（context_compress）](#3-上下文压缩context_compress)
- [4. 深度总结省 token（deep_summary map-reduce）](#4-深度总结省-tokendeep_summary-map-reduce)
- [5. 计费可见（token_bill）](#5-计费可见token_bill)
- [6. reindex 与缓存失效一致性](#6-reindex-与缓存失效一致性)
- [7. 记忆固化（memory_consolidation）](#7-记忆固化memory_consolidation)
- [8. multi-turn 与 prompt cache 现状（诚实标注）](#8-multi-turn-与-prompt-cache-现状诚实标注)
- [9. 实测节省量](#9-实测节省量)

---

## 1. 设计原则:成本分层 + 缓存复用

attune 的省 token 不是单一技巧，而是一条 zero-LLM → cheap-LLM → reasoning-LLM 逐级升级的流水线，
每一级都尽量在更便宜的层完成。核心思想（`document_intelligence/deep_summary.rs:1-14`）：

> 三个独立的省 token 杠杆叠加：(1) 本地 extractive 预切（零 LLM）；
> (2) `chunk_summaries` 缓存复用；(3) cheap-map / reasoning-reduce 拆分。

token 估算本身也是零成本本地计算（`context_compress.rs:245-256`）：CJK 按 1.2 token/char、
ASCII 按 0.25 token/char 的整数运算近似，仅用于 UI 显示与 naive baseline 计算，不调任何 tokenizer 服务。

---

## 2. chunk 摘要缓存复用（chunk_hash）

**机制**:每个 chunk 的摘要按 `sha256(chunk_text)` 作为缓存键持久化（`context_compress.rs:73-78`），
配合一个 `strategy` 维度（`economical` / `accurate` / `deepsum:<level>`）。chunk 内容一变，hash 变，
缓存自然失效（`context_compress.rs:16-17` 注释）。

**复用收益**:第二次读同一文档时，所有 chunk 摘要直接命中缓存，**零新增 LLM 调用**。这在
`deep_summary.rs` 的 STAGE 2 实现（`deep_summary.rs:206-217`）——缓存查询特意放在"短块短路"之前，
保证 re-read 时连短块也走缓存，否则短文档每次都会重付 reduce 的输入成本（注释 `deep_summary.rs:207-209`）。

**测试佐证**:`deep_summary.rs::test_cache_hit_zero_new_tokens_on_second_run`（`:558-597`）断言第二次跑
同一输入 + 同一 `item_id`：`cheap.call_count() == 0`（零 map LLM 调用）、`map_llm_tokens.r#in == 0`、
`new_chunks == 0`、`cache_hit_chunks > 0`。

---

## 3. 上下文压缩（context_compress）

**触发时机**:在**用户发起 chat 时**（RAG 检索后、Prompt 组装前）压缩，不在建库流水线里主动生成
摘要——因为多数 chunk 永远不会被查询到，提前压缩浪费算力（`context_compress.rs:5-7`）。这是
CLAUDE.md 成本契约「建库阶段永不升级到 LLM 层」的直接落地。

**三档策略**（`context_compress.rs:26-60`）:
- `raw` — 不压缩，全文透传（纯本地/免费模式）；
- `economical` — ~150 字摘要，保留数字/专名（云端默认）；
- `accurate` — ~300 字摘要 + 原文前 100 字（长文 chat）。

**短块短路**:`original_chars <= target_chars` 时直接原文返回，不触 LLM（`context_compress.rs:96-103`）——
摘要可能比原文还长，压缩反而亏。

**graceful degrade**（per CLAUDE.md §4.5.E）:LLM 不可用时退化为截断原文，不让 chat 流程挂掉
（`context_compress.rs:135-144`）；批量压缩中单条失败降级为 raw，不中断整批（`context_compress.rs:225-234`）。

**隐私 + 省 token 同时满足**:压缩前对 chunk 文本做 PII 脱敏再送 LLM（`context_compress.rs:148-150` /
`:192-194`，F-17-PRIVACY），既省 token 又不把原文 PII 送出网。

**web 结果不入缓存**:无 `item_id` 的 chunk（如 web 搜索结果）不确定何时失效，退回 raw 避免云端反复计费
（`context_compress.rs:107-115`，测试 `empty_item_id_web_chunk_no_cache` `:334-343`）。

---

## 4. 深度总结省 token（deep_summary map-reduce）

`deep_summary.rs` 是省 token 的旗舰流水线，把 `routes/chat.rs` 验证过的 cache-map-reduce 泛化成可测量的多级总结
（`deep_summary.rs:1-7`）。流水线阶段（`deep_summary.rs:9-14`）:

| Stage | 动作 | 成本层 |
|-------|------|--------|
| -1 | 短文单调用旁路（naive < 1500 tok） | 1 次 reasoning |
| 0 | 章节切分 + chunk + chunk_hash | 零 LLM |
| 1 | 本地 extractive 预切（短块透传） | 零 LLM |
| 2 | 缓存查询（chunk_hash, deepsum:level） | 零 LLM |
| 3 | bounded MAP：cheap 模型压缩 miss 块并写回 | cheap 模型，批量 |
| 4 | bounded REDUCE：reasoning 模型合成 ×1（或 ≤⌈n/FANIN⌉） | reasoning 模型，少量 |

**cheap/reasoning 拆分**:大 token 量的 map 走 cheap 模型，少量调用的 reduce 走 reasoning 模型
（`model_routing.rs:17-27` 的 `ModelRole::Cheap` / `Reasoning`）——成本杠杆最大处用最便宜的模型。
测试 `test_map_uses_cheap_reduce_uses_reasoning`（`:474-500`）断言 map 调用全是 cheap 模型、reduce 全是
reasoning 模型。

**短文旁路（STAGE -1）**:naive baseline < `DEEPSUM_MIN_TOK = 1500`（`deep_summary.rs:73`）时，map-reduce
的多阶段开销反而超过单次 naive 调用，于是退化为一次标准 summarize（`deep_summary.rs:162-170` /
`single_call_bypass :296-344`）。这个 1500 阈值在**编译期**用 `const _: () = assert!(...)`（`:80-87`）
锁死在 T-12 证据带内（最大净负短文 441 tok < 1500 < 最小受益长文 9873 tok），避免阈值悄悄漂移把旗舰流水线
在生产里误关。

**weak-model 兜底**（§4.5.E）:某个 map 块调用失败/返空时，降级为该块的 extractive 候选作为摘要，
不让一个噪声块拖垮整篇总结（`deep_summary.rs:233-246`，测试 `test_map_empty_output_degrades_to_extractive_no_crash` `:502-523`）。

---

## 5. 计费可见（token_bill）

省了多少 token 不能靠声称，要**测量**（`token_bill.rs:1-8`）。每次 deep-summary / compare / chapter 操作都产出一个
`TokenBill`（`token_bill.rs:48-73`），携带:

- `naive_baseline_tokens` — 全文一次性塞进 reasoning 模型的基线（`deep_summary.rs:146-150`，等于
  `cost::estimate_tokens(full_text, reasoning_model)`）；
- `map_llm_tokens` / `reduce_llm_tokens` — 每条腿的实际 in/out（按模型分腿，便于 USD 计算）；
- `cache_read_tokens` / `cache_hit_chunks` / `new_chunks` — 缓存可观测性；
- `path` — `"map-reduce"` 或 `"single-call"`，标明走了哪条路。

**两个口径**（`token_bill.rs:87-114`）:
- `savings_ratio_by_token()` = `1 − actual_billable / naive_baseline`，**主指标**（模型无关、免受定价漂移影响，per spec §8.5）；
- `savings_ratio_by_usd()` = 按模型定价折算，因 cheap/reasoning 拆分而**更大**，但定价敏感故作次指标。

**安全约束**（G1 panel flag，CLAUDE.md §1.4）:`TokenBill` 每个字段都是 count 或 USD 金额，
**没有任何字段持有 api_key / gateway token**（`token_bill.rs:10-13`）。结构性守卫
`test_no_secret_field_only_counts`（`:223-240`）序列化后断言 JSON 不含 `apiKey` / `api_key` / `secret` /
`Bearer` / sentinel gateway token。UI 据此渲染 naive-vs-actual 成本对比条，让用户一眼看到"谁买单"。

---

## 6. reindex 与缓存失效一致性

省 token 缓存若不及时失效会导致"答旧内容"，因此 `reindex.rs` 把 DB / 向量 / FTS / embed 队列 /
chunk_summary **作为一组事务式协调**（`reindex.rs:1-28`）。内容变更后:

1. 删旧向量 → 删旧 FTS → 清旧 embed 队列（`reindex.rs:67-69`）；
2. **删旧 chunk_summary**（`reindex.rs:70-72`，QW-5）——内容变了 chunk 边界也变了，旧摘要按新 hash 永远命不中，
   直接清掉避免孤儿；
3. 写新 FTS（搜索立即可用，不等 embedding）+ 入队 Level 1 章节 + Level 2 段落块（`reindex.rs:73-89`）。

删除路径 `purge_item_indexes`（`reindex.rs:98-111`）做 defense-in-depth 同样清 chunk_summary。
测试 `reindex_purges_chunk_summaries`（`:153-171`）/ `purge_indexes_removes_chunk_summaries`（`:173-184`）
断言 reindex / purge 后 `chunk_summary_count() == 0`。这保证省 token 缓存与最新内容**一致**。

---

## 7. 记忆固化（memory_consolidation）

把高频的 `chunk_summaries` 进一步聚合成低频的 episodic memory，是更长周期的省 token:
后续 chat 可引用一条「过去一天学习焦点」总结，而非重新读几十条 chunk。

三阶段（与 skill_evolution 同构，`memory_consolidation.rs:1-12`）:
1. `prepare_consolidation_cycle`（**持 vault 锁**）— 扫 chunk_summaries → 按天分桶 → 解密 → 过滤已固化（幂等）；
2. `generate_episodic_memories`（**无锁**）— 每 bundle 一次 LLM 调用，失败 bundle 返 None 不影响其他；
3. `apply_consolidation_result`（**持 vault 锁**）— `INSERT OR IGNORE` 写 memories，幂等。

**调用风暴防护**:`MAX_BUNDLES_PER_CYCLE = 4`（`memory_consolidation.rs:27`）、单 bundle `MAX_CHUNKS = 50`
（`:25`）、只看最近 30 天（`LOOKBACK_SECS`，`:31`），把 LLM 调用次数硬性封顶。
幂等键 = 排序后的 `chunk_hash` 列表 JSON（`:51-54` / `:98-104`），重跑不重复消耗 token
（测试 `apply_is_idempotent_on_rerun` `:451-470`）。

---

## 8. multi-turn 与 prompt cache 现状（诚实标注）

per CLAUDE.md §4.5G，sequential decision agent 应走真 multi-turn + provider prompt cache。attune 现状:

- **真 multi-turn 已具备**:`LlmProvider::chat_with_history(&[ChatMessage])` 维护完整 prior turns
  （`llm.rs:228`），retry-validate 循环把校验错误作为新 turn 追加再 call（`llm.rs:347-398`，
  `chat_with_retry` 最多 3 次，per §4.5B）；few-shot 走 `[system, ex1, ..., user]` 真消息序列
  （`llm.rs:401-417`，per §4.5C）。memory_consolidation / deep_summary 的 reduce 均用
  `chat_with_history`（`memory_consolidation.rs:161`、`deep_summary.rs:371-375`）。
- **TokenUsage 已含 `cached_in` 字段**:`token_bill.rs:266`（map 腿累加 `usage.cached_in` 到
  `cache_read_tokens`），结构上预留了 provider prompt-cache 读计数。
- **PENDING-VERIFY**:截至本页撰写，attune-core 源码中**未检索到** `cache_control: {"type":"ephemeral"}`
  显式开关（grep `cache_control` 0 命中，见调研记录）。即 Anthropic 显式 prompt-cache 标记尚未在客户端落地；
  当前省 token 主要靠**应用层缓存**（chunk_summary）+ cheap/reasoning 拆分，而非 provider 层 prefix cache。
  此项作为后续 §4.5G 完善点登记，**不声称已实现**。

---

## 9. 实测节省量

数据源:`reports/2026-06-06_deepsum-savings.md`（T-12，确定性 harness `tests/deepsum_savings.rs`，RecordingMockLlm）。
主指标 = `savings_ratio_by_token`。

**冷缓存首次读（cold run）**:8 文档分布 mean **23.8% ± 16.3%**（n=8）。长文档收益更高:

| 文档 | 字符 | naive_tok | actual_tok | 节省 |
|------|------|-----------|------------|------|
| long-zh-30ch | 20292 | 9873 | 6524 | 33.9% |
| long-en-30ch | 63792 | 15948 | 6950 | **56.4%** |
| long-zh-50ch | 45032 | 21908 | 13590 | 38.0% |

**暖缓存二次读（warm run，同 item_id）**:长文档接近上限:

| 文档 | 二次读节省 | map 调用 |
|------|-----------|---------|
| long-zh-30ch | **95.0%** | 0（全缓存命中）|
| long-en-30ch | **93.3%** | 0 |
| long-zh-50ch | **96.2%** | 0 |

**诚实的旗舰目标结论**（report §48-70，G3 已上报人工）:spec §9.1 原定"冷跑 ≥80% 文档 ≥60% 节省"
**未达成且结构上不可达**——MAP 必须读 extractive 候选（≥40% token），冷跑 actual 约 40-100% baseline。
该 harness **真正 gate 的、诚实成立的**是:(a) `actual ≤ naive` 恒成立（账单正确性，且曾抓出并修复一处 CJK
tokenizer 口径不一致 `deep_summary.rs:248-265`）；(b) 暖缓存 ≥ 冷缓存（缓存永不变差）；(c) 长文档 re-read ≥60%。
by-USD 节省因 cheap/reasoning 拆分而显著更高，但定价敏感，故 by-token 为主指标。

---

## 关联

- benchmark 量化证据 → `docs/benchmarks/2026-06-quality-proof-points.md`
- 视觉理解流水线 → `docs/wiki/vision-understanding-pipeline.md`
- 成本与触发契约（产品规则）→ 项目 `CLAUDE.md` §成本感知与触发契约
