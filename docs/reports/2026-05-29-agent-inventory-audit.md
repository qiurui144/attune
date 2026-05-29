# Attune Agent Inventory Audit (read-only, agent A)

**Date**: 2026-05-29 · **Scope**: attune OSS (`attune-core`) + attune-pro 4 plugins (law/tech/patent/presales) · **Method**: read-only grep/read; no build/test/mutate.

> Evidence path convention: `attune` = `/data/company/project/attune`, `pro` = `/data/company/project/attune-pro`.

---

## 0. Executive numbers

- **Total agents (shipped + scaffold-declared)**: **22**
  - OSS attune: **6** (1 in `agents/`, 5 cross-cutting modules)
  - attune-pro law-pro: **14** (11 deterministic + 3 LLM extractor) + 1 VLM capability (evidence_classifier)
  - attune-pro tech-pro: **1** (`code-reviewer`, alpha)
  - attune-pro patent-pro: **0** (scaffold, `agents: []`)
  - attune-pro presales-pro: **0 shipped** (scaffold; 4 capability dirs declared but plugin `agents: []`)
- **Free (OSS) vs paid (pro)**: 6 free / 16 paid (all pro plugins `pricing.tier: paid`).
- **Has golden gate**: law-pro 14/14 (`pro/plugins/law-pro/tests/agent_golden_gate.rs`); tech-pro code-reviewer has golden/proptest/boundary coverage declared but **no CI-blocking gate harness yet**. OSS 6 agents have unit tests but **no `agent_golden_gate.rs` exists in attune-core** (only in attune-pro). → ~14/22 (64%) under a real CI gate.
- **Top 3 findings**: (1) no OSS `agent_golden_gate` harness despite CLAUDE.md "free=pro discipline"; (2) `defamation` capability split across 2 agents (deterministic `defamation_agent` + LLM `defamation_extractor`) — borderline duplication; (3) capability voids: tech/patent/presales are scaffold/alpha — entire verticals have 0–1 working agent.

---

## 1. Full agent matrix

### 1.1 OSS attune (free, in-process, all deterministic except LLM-call-once helpers)

| agent | file (evidence) | type | golden gate? | model tier | cloud token | F1 / pass-rate | single-point boundary | chat_trigger |
|---|---|---|---|---|---|---|---|---|
| `document_classifier` | `attune/.../src/agents/document_classifier.rs` | deterministic (Agent trait) | unit tests only (`mod.rs` tests L145/L177) | none | 🆓 zero | n/a (no published F1) | classify doc → evidence kind + confidence + followup | none (invoked by pipeline) |
| memory consolidation (`run_consolidation_cycle`) | `attune/.../src/memory_consolidation.rs:250` | **LLM** (1 call per day-bundle) | unit (MockLlmProvider L268) | any LLM (degrades: LLM-off → skip) | ⚡/💰 LLM per bundle | none published | chunk_summaries → episodic memory, time-windowed, idempotent | none (periodic background) |
| knowledge linker (`compute_links_for_item`) | `attune/.../src/linker/agent.rs` | **deterministic** (entity/vector co-mention) | unit | none | 🆓 zero | none published | emit LinkKind between items (how, not why) | none (background) |
| chat-reliability (`evaluate_response`) | `attune/.../src/chat_reliability/agent.rs` | **deterministic** (citation/contradiction/hallucination extractors) | unit | none | 🆓 zero | none published | grade RAG answer grounding, pure fn | none (post-chat hook) |
| `self_evolving_skill_agent` | `attune/.../src/skill_evolution/agent.rs:1,144` | hybrid: **zero-cost heuristic path + optional LLM** | unit | optional (works LLM-off) | 🆓 heuristic / ⚡ LLM | none published | per-query learned search expansions (SkillClaw) | none (search-side hook) |
| legacy skill evolution (`run_evolution_cycle`) | `attune/.../src/skill_evolution/mod.rs` | **LLM-only** topic clustering | unit | any LLM | 💰 LLM | none published | cluster failed queries → `learned_expansions` blob | none (background) |

Note: `ai_annotator.rs`, `chat.rs`, `classifier.rs` are capabilities/engines, not Agent-trait agents. Internal `skills/` (parse_chinese_date, extract_entities, classify_chunk_kind, summarize_text) are pure single-call skills, **not agents** (`skills/mod.rs`: "不编排多个 skill").

### 1.2 attune-pro law-pro (paid, `pricing.tier: paid`, trial_quota 10; `pro/plugins/law-pro/plugin.yaml` v1.0.5)

11 deterministic (subprocess `rust_binary`, `llm_tokens: 0`) + 3 LLM extractor. Per-agent resource cap inherited: `total_max_llm_tokens_per_call: 10000`, `cpu_seconds: 30`.

| # | agent id | type | golden gate | model tier | cloud token | F1 / pass-rate (evidence: pro/RELEASE.md) | boundary | chat_trigger keywords |
|---|---|---|---|---|---|---|---|---|
| 1 | `civil_loan_agent` | deterministic | ✅ 1.00 | none | 🆓 (llm_tokens:0) | 1.00 (det gate L108) | 借贷本息合规计算 + 红线 | 本金/利息/借贷/应付/应收/本息 |
| 2 | `bank_aggregator_agent` | deterministic | ✅ 1.00 | none | 🆓 | 1.00 | 银行流水聚合 + 交叉验证 | 流水/银行流水/交易记录/到账/转账明细 |
| 3 | `fact_extractor_agent` | **LLM** | ✅ holdout | all-tier (qwen3b OK) | 💰 ~2000 tok | **F1 1.0000±0** (RELEASE L100; earlier 0.9828 L173/184) | 借条 OCR → 本息事实 (grounded) | 抽取事实/借条信息/识别本金/提取要素 |
| 4 | `limitation_agent` | deterministic | ✅ 1.00 | none | 🆓 | 1.00 | 诉讼时效日期算术 | 诉讼时效/时效/过了时效/三年时效 |
| 5 | `evidence_chain_agent` | deterministic | ✅ 1.00 | none | 🆓 | 1.00 | 证据链印证/矛盾/缺口 (描述-only) | 证据链/证据关系/证据缺口/印证/证据矛盾 |
| 6 | `labor_dispute_agent` | deterministic | ✅ 1.00 | none | 🆓 | 1.00 | 劳动经济补偿金/赔偿金 (劳法47/87) | 经济补偿金/赔偿金/违法解除/劳动争议/工龄 |
| 7 | `traffic_accident_agent` | deterministic | ✅ 1.00 | none | 🆓 | 1.00 | 交通责任比例+伤残赔偿 | 交通事故/责任比例/伤残等级/事故认定/医疗费 |
| 8 | `sale_contract_agent` | deterministic | ✅ 1.00 | none | 🆓 | 1.00 | 买卖合同违约金 (民法典585) | 买卖合同/违约金/合同价款/实际损失/第585条 |
| 9 | `housing_rent_agent` | deterministic | ✅ 1.00 | none | 🆓 | 1.00 | 房屋租赁押金/违约金/已付租金 | 房屋租赁/租房/押金/退租/租赁合同/月租金 |
| 10 | `inheritance_agent` | deterministic | ✅ 1.00 | none | 🆓 | 1.00 | 法定继承份额 (民法典1127/1130/1131) | 继承/法定继承/遗产/继承人/份额/1127/1130 |
| 11 | `defamation_agent` | deterministic | ✅ 1.00 | none | 🆓 | 1.00 | 名誉权物质损失+精神抚慰金区间 | 名誉权/名誉损害/诽谤/侮辱/精神损害/侵权 |
| 12 | `interest_calculator` | **library** (no dispatch) | n/a | none | 🆓 | n/a | LPR 计算引擎,civil_loan 内部调用 | (inventory completeness only) |
| 13 | `divorce_extractor_agent` | **LLM** | ✅ holdout | all-tier | 💰 LLM | **F1 0.9894±0.0072** (RELEASE L101; earlier 0.9710 L174/184) | 离婚案情抽取,binary 与 agent_divorce 共享 | (shares divorce case_kind) |
| 14 | `defamation_extractor` | **LLM** | ⚠️ holdout, near floor | **≥ gpt-4o-mini** (qwen3b 卡能力) | 💰 LLM | **F1 0.8683±0.0176** (RELEASE L102) — historically 0.56 → 0.72 → now ⚠️Beta near 0.85 floor (L78/175/179/249) | 名誉侵权 LLM 抽取 | (shares defamation case_kind) |

Plus VLM **capability** (not in `agents:`): `law_pro.evidence_classifier` — `cost_tier: money`, `llm: vision` (qwen2.5vl/llava/claude-vision), `min_ram_gb: 12`, trigger `manual`, 28-class golden (`pro/plugins/law-pro/capabilities/evidence_classifier/plugin.yaml`).

### 1.3 attune-pro tech-pro (`pro/plugins/tech-pro/plugin.yaml` v0.2.0, status alpha, paid trial_quota 0)

| agent id | type | golden gate | model tier | cloud token | pass-rate | boundary | chat_trigger |
|---|---|---|---|---|---|---|---|
| `code-reviewer` | deterministic-with-llm-scaffold (`runtime: in_process`, `binary: null`) | declared coverage: golden 11 / error 3 / proptest 3 / boundary 6 / integration 2 / regression 1 — **no CI-blocking gate harness found** | none now (LLM judge layer scaffolded, real-LLM deferred v1.2) | 🆓 deterministic now | not published as F1 | unified-diff review (Rust/Go/Python/TS pattern + secret/large-file + team-rule YAML) | 项目/代码库/PR/issue |

### 1.4 patent-pro / presales-pro (scaffold)

- **patent-pro** (`plugin.yaml`): `status: scaffold`, `agents: []`, "NOT published to pluginhub. NOT entitled to any plan." → **0 agents**.
- **presales-pro** (`plugin.yaml` v0.1.0): `status: scaffold`, `agents: []`. But 4 capability subdirs exist with own plugin.yaml (`competitive_analysis`, `bant_qualification`, `poc_proposal`, `quote_builder`) — declared planned capabilities, **not wired into the plugin `agents:` list**, so not dispatchable. → **0 shipped agents**.

---

## 2. Six key questions answered

### Q1. Full inventory — how many agents, where?
**22 total.** OSS attune = 6 (document_classifier in `agents/`; memory_consolidation; linker; chat_reliability; self_evolving_skill_agent; legacy skill_evolution cycle). law-pro = 14 (11 deterministic binaries + fact/divorce/defamation LLM extractors; note `interest_calculator` is a library, `divorce_extractor` shares `bin/agent_divorce`). tech-pro = 1 (code-reviewer alpha). patent-pro = 0, presales-pro = 0 (both scaffold). Plus law-pro `evidence_classifier` VLM capability (counted separately as capability, not Agent-trait agent).

### Q2. Free vs paid boundary — how is entitlement judged?
- **OSS attune agents are free** — built into attune-core, no entitlement check. They are base/cross-cutting capabilities (classification, memory, linking, reliability, skill learning), not domain agents.
- **All pro agents are paid** — every pro `plugin.yaml` has `pricing.tier: paid`. Entitlement is at the **plugin level**, not per-agent: law-pro `trial_quota: 10`, tech/patent/presales `trial_quota: 0`. patent/presales additionally "NOT entitled to any plan" (scaffold).
- Entitlement enforcement: plugin loaded via `attune-core::plugin_loader::from_dir_with_key` + `plugin_sig` Ed25519 verification + Argon2id+AES-GCM encrypted yaml for paid tier (law-pro plugin.yaml build/dist notes). JWT tier gating noted as deferred to v1.0.1 (pro/RELEASE.md L269 `#91 JWT tier`).
- **Gap**: entitlement is plugin-granular only. No per-agent tier flag in the schema — can't sell e.g. "fact_extractor only" without the whole law-pro pack.

### Q3. Cloud-token work scheduling (per Cost & Trigger Contract 3 tiers)
- **🆓 zero-token (deterministic, local CPU)**: 11/14 law-pro agents (`llm_tokens: 0`), OSS linker / chat_reliability / document_classifier, self_evolving_skill heuristic path. These can run freely / background.
- **💰 cloud-LLM required**: law-pro fact_extractor (~2000 tok), divorce_extractor, defamation_extractor (extractors); OSS memory_consolidation + legacy skill_evolution. `defamation_extractor` **must hit ≥ gpt-4o-mini tier** (qwen3b caps at F1 0.56–0.72); others degrade to qwen2.5:3b OK (all-tier).
- **Downgradeable to local qwen**: fact/divorce extractors (all-tier verified), self_evolving_skill (heuristic fallback when LLM off), memory_consolidation (skips bundle on LLM failure, no panic).
- **Routing to gateway**: per attune CLAUDE.md the cloud path is the Attune Pro Membership Gateway (`gateway.engi-stack.com/v1`) or BYOK. Per-call cap `total_max_llm_tokens_per_call: 10000` enforced by resource governor. **Current state**: LLM extractors are user-triggered (analysis tier), deterministic agents auto-run — consistent with the contract ("建库阶段不升级第三层，分析阶段等用户开口").

### Q4. Quality (golden-gate pass-rate / F1, with evidence)
- **law-pro 11 deterministic**: `deterministic_agent_golden_gate` 178/178 PASS, 1.00 ENFORCE (pro/RELEASE.md L108).
- **law-pro LLM extractors (qwen2.5:3b, 3-run mean, latest L100-102)**: fact_extractor F1 **1.0000±0** ✅; divorce_extractor F1 **0.9894±0.0072** ✅; defamation_extractor F1 **0.8683±0.0176** ⚠️ Beta (near 0.85 floor; historical climb 0.56→0.72→0.87, L78/179/249).
- **OSS 6 agents**: unit-tested with MockLlmProvider but **no published F1 / no agent_golden_gate harness** in attune-core. CLAUDE.md acknowledges "OSS 当前无 domain agent... 未来加 agent 同走此纪律" — but cross-cutting agents (memory/linker/reliability) already ship without the gate.
- **tech-pro code-reviewer**: coverage counts declared in plugin.yaml (golden 11 etc.) but no CI-blocking gate found; LLM judge layer not real-LLM tested (deferred v1.2).

### Q5. Single-point boundaries — overlap / voids
- **Boundaries are mostly clean**: each law-pro deterministic agent owns one case_kind formula; OSS agents own distinct concerns.
- **Overlap / borderline duplication**:
  1. `defamation_agent` (deterministic damages calc) vs `defamation_extractor` (LLM extraction) — same case_kind `defamation`, split by det-calc vs LLM-extract. Intentional (det part comments "抽取走 defamation_extractor") but two agents on one domain.
  2. `divorce_extractor_agent` shares `bin/agent_divorce` with the deterministic divorce path — one binary, two agent ids.
  3. OSS `self_evolving_skill_agent` (per-query) **coexists with** legacy `run_evolution_cycle` (topic-keyed) — explicitly "both paths coexist", functional overlap on query-expansion learning.
- **Capability voids (TOP)**: tech-pro = 1 alpha agent only (no codebase-scan, GitHub-PR, arch-diagram — all deferred v1.2); patent-pro = entirely empty scaffold; presales-pro = 4 planned capabilities not wired (0 dispatchable). 3 of 4 pro verticals are effectively non-functional.

### Q6. chat_trigger routing — query → agent
- **Router**: `attune-core/src/intent_router.rs` — `IntentRouter::route()` is a pure-function keyword/regex matcher (no LLM), reads each loaded plugin's `chat_trigger.keywords`, requires `min_keyword_match`, returns candidates sorted by `priority` desc.
- **Project recommender**: `attune-core/src/project_recommender.rs` — `recommend_for_chat(message, keywords)`; keywords aggregated by the route handler from each vertical plugin's `chat_trigger.project_keywords`. **Bare OSS (no vertical plugin) → keywords=[] → never triggers** (intentional).
- **Per-agent triggers** live in each law-pro agent's `chat_trigger.keywords` (priority 8–10, e.g. 借贷 priority 10, fact_extractor priority 8). Plugin-level chat_trigger (priority 5) covers project-scope (案件/诉讼/客户). So a query routes: agent-level keyword (pri 10) > plugin project keyword (pri 5).

---

## 3. Recommendations for governance spec (data-only, not prescriptive)

1. **OSS gate parity**: attune-core has no `agent_golden_gate.rs` though it ships 6 agents (2 LLM-driven). CLAUDE.md "free=pro 同纪律" is currently aspirational — copying the harness from attune-pro would close it.
2. **Per-agent entitlement**: schema only has plugin-level `pricing.tier`; consider agent-level tier flags if granular monetization is wanted.
3. **defamation_extractor floor**: only LLM agent below safe margin (0.87 vs 0.85 floor, std 0.018 → near-overlap with floor); flagged ⚠️Beta requiring ≥gpt-4o-mini — the one quality risk in shipped agents.
4. **Scaffold cleanup**: patent-pro/presales-pro declare capabilities but expose 0 agents — inventory vs reality drift.
