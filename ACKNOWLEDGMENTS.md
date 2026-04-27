# Acknowledgments

[English](ACKNOWLEDGMENTS.md) · [简体中文](ACKNOWLEDGMENTS.zh.md)

Attune stands on the shoulders of an enormous community of open-source projects, papers, and dev blogs. This file lists the **specific design patterns** we adopted and where each came from. We are deeply grateful.

For full software-license attribution of dependencies, see `Cargo.lock` (Rust) and `package.json` (Chrome extension / desktop). This document focuses on **design and algorithmic provenance**, not transitive deps.

---

## Foundational Inspiration

| Layer | Influence | Why |
|-------|-----------|-----|
| Encrypted vault metaphor | **1Password**（design language only) | "Lock-screen-on-screensaver" + Argon2id key derivation UX |
| Local-first PKM ergonomics | [**Obsidian**](https://obsidian.md), [**Logseq**](https://github.com/logseq/logseq) | Backlinks, local Markdown ownership, "your notes belong to you" stance |
| Single-binary distribution | [**Caddy**](https://caddyserver.com), [**SQLite**](https://sqlite.org) | Zero-runtime-deps Rust binary philosophy |

---

## Per-Feature Attribution

### H1 Resource Governor (commit `2bc558c`, 2026-04-27)

| Source | What we adopted |
|--------|----------------|
| [GuillaumeGomez/sysinfo](https://github.com/GuillaumeGomez/sysinfo) (MIT) | Cross-platform CPU/RAM sampling primitives |
| Linux `nice(1)` / `ionice(1)` | "Cooperative citizen" scheduling philosophy — pause when the system is busy, not based on per-task quota |
| [logseq/logseq](https://github.com/logseq/logseq) (negative example) | We explicitly designed against the index-rebuild lag complaints common to Logseq/Obsidian — hence per-task budget caps + topbar Pause |

### A1 Memory Consolidation MVP (commit `71a714f`, 2026-04-27)

| Source | What we adopted |
|--------|----------------|
| [mem0ai/mem0](https://github.com/mem0ai/mem0) (Apache-2.0) | "Memory layer for AI" framing; episodic memory as a discrete data model above raw chunks |
| [skill_evolution.rs] (attune internal, 2026-03) | Three-stage lock release pattern (prepare → generate → apply) was already established by SkillEvolver; A1 mirrors it directly |

### W3 Batch B — G1 / G2 / G5 / F3 (2026-04-27)

| Source | What we adopted |
|--------|----------------|
| [linkwarden/linkwarden](https://github.com/linkwarden/linkwarden) (AGPL-3.0) | G1 浏览状态 capture pattern — fetch-on-engagement + privacy 默认 opt-out |
| [ArchiveBox/ArchiveBox](https://github.com/ArchiveBox/ArchiveBox) (MIT) | G1 信号建模思路 — URL + dwell + engagement 作为元数据，非全量内容 |
| [standardnotes/app](https://github.com/standardnotes/app) (AGPL-3.0) | G5 隐私控制面板 UX — "数据仅本机不上传" + per-domain 控制 + 显式 opt-out |
| [bitwarden/clients](https://github.com/bitwarden/clients) (GPL-3.0) | G5 默认 opt-out 模式 — 用户必须显式启用每个 domain，非默认开启 |
| 行业 SRE 常识 | HARD_BLACKLIST 域名清单（banks / medical / gov / password managers / OAuth） — 无单一来源，行业共识 |
| attune 自有 `MockLlmProvider` | F3 secondary retrieval E2E 测试 mocking pattern |

### W3 Batch A — F1 / F2 / F4 / C1 (2026-04-27)

| Source | What we adopted |
|--------|----------------|
| [吴师兄 article](https://mp.weixin.qq.com/s/YNcfSN0uv1c1LsLPzgB0jw) §6 高频 query 缓存 | C1 web_search_cache table — SHA-256(query) key + DEK encrypted results + 30-day TTL pattern |
| [linkwarden/linkwarden](https://github.com/linkwarden/linkwarden) (AGPL-3.0) | "Snapshot at fetch time" mental model — once a query is cached, treat it as immutable for the TTL window |
| attune internal `chunk_summaries` table | F2 sidecar pattern — independent table keyed on `(item_id, chunk_idx)` rather than extending core schemas (avoids `.encbin` migration risk for existing vaults). Pioneered for chunk_summaries (W2 ago), reused for chunk_breadcrumbs (W3 batch A) |

### J Series — RAG Production Quality (W2-W4 in progress)

| Source | What we adopted |
|--------|----------------|
| [吴师兄: "鹅厂面试官追问：你的 RAG 能跑通 Demo？那让它在 5000 份文档里稳定答对，试试看"](https://mp.weixin.qq.com/s/YNcfSN0uv1c1LsLPzgB0jw) | The 8 production-hardening levers; specifically J1 (chunk breadcrumb path), J3 (explicit threshold tuning curve 0.65/0.72/0.78), J5 (strict prompt + confidence), J6 (公开召回率 + 答非所问率 as KPI). Quantitative anchors: recall 0.62→0.91, hallucination 18%→7% |
| [explodinggradients/ragas](https://github.com/explodinggradients/ragas) (Apache-2.0) | J6 metric names + formulas: Faithfulness, Answer Relevancy, Context Precision, Context Recall — adopting industry standard rather than inventing |
| [CRAG paper (arXiv:2401.15884)](https://arxiv.org/abs/2401.15884) | J5 three-class retrieval gating: correct / incorrect / ambiguous → branched action (re-retrieve with lowered threshold) |
| [Self-RAG paper (Asai et al.)](https://arxiv.org/abs/2310.11511) | J5 token-level confidence (1-5 scale instead of generation tokens, simpler for chat use) |
| [stanfordnlp/dspy](https://github.com/stanfordnlp/dspy) (MIT) | Inspiration only for offline threshold tuning. We do **not** run DSPy compile in production (per attune cost contract) |

### K Series — Open-Source Landscape Adoption (W5-W10 planned)

| Feature | Source | What we adopt |
|---------|--------|---------------|
| **K1 Sleeptime evolution agent** | [letta-ai/letta](https://github.com/letta-ai/letta) (Apache-2.0) — "sleeptime agent" pattern; [noahshinn/reflexion](https://github.com/noahshinn/reflexion) (MIT) — verbal feedback long-term buffer | Primary chat agent never blocks for memory compaction; a separate background agent runs cross-session reflection |
| **K2 Parse Golden Set** | [Readwise Reader](https://blog.readwise.io/the-next-chapter-of-reader-public-beta/) (commercial, design only) | 200-page parsing benchmark methodology; CI regression < 95% accuracy blocks release |
| **K3 AGENTS.md compatibility** | [continuedev/continue](https://github.com/continuedev/continue) (Apache-2.0) — `.continue/checks/*.md` + `create_rule_block`; [PatrickJS/awesome-cursorrules](https://github.com/PatrickJS/awesome-cursorrules) (MIT) | Plugin SDK reads attune `plugin.yaml` AND community `AGENTS.md` / `.continue/rules/*.md` — zero-cost ecosystem onboarding |
| **K4 CRDT multi-device sync** | [anytype-io/any-sync](https://github.com/anytype-io/any-sync) (MIT for the protocol; Anytype client uses Anytype Tech License) | AnySync architecture as reference for v0.7+ exploration — not yet committed |
| **K5 Items Key revocation** | [standardnotes/app](https://github.com/standardnotes/app) (AGPL-3.0) — 004 spec hierarchical key model | Master key encrypts items keys, items key encrypts data; per-Project / per-Note independent keys enable selective cloud-backup revocation |

### Hybrid Intelligence (C series)

| Source | What we adopt |
|--------|---------------|
| [searxng/searxng](https://github.com/searxng/searxng) (AGPL-3.0) | C1 self-hosted meta-search backend (when user enables web augmentation) — instead of building our own search aggregator |
| [assafelovic/gpt-researcher](https://github.com/assafelovic/gpt-researcher) (MIT) | C3 wrapping their MCP server rather than re-implementing autonomous research |
| Article: ["Cherry Studio vs LobeChat 2026"](https://openalternative.co/compare/anythingllm/vs/cherry-studio) | Validated that "MCP server-as-distribution-channel" is 2026's largest single growth lever |

### Industry Plugin Ecosystem (E series)

| Source | What we adopt |
|--------|---------------|
| [langchain-ai/langgraph](https://github.com/langchain-ai/langgraph) (MIT) | E2 plugin SDK: StateGraph + Node/Edge + checkpointing concepts (we keep our own minimal implementation, not a langgraph dependency) |
| [All-Hands-AI/OpenHands](https://github.com/All-Hands-AI/OpenHands) (MIT) | E1 marketplace YAML schema for plugin manifests |

---

## Negative Examples (Explicitly Avoided)

These projects taught us **what not to do** — equally valuable:

| Anti-pattern | Source | Our counter |
|--------------|--------|-------------|
| Memory as opaque LLM-managed function calls (fails on <3B local models) | Letta v1 agent loop | Attune keeps deterministic indexer + restricts LLM to sleeptime / explicit analysis phases |
| Compile-time prompt optimization at $20-50/run | DSPy default `compile()` | Attune uses DSPy ideas only offline during dev; runtime pipeline is hand-tuned + RAGAS-validated |
| Default deep-research mode that makes 5-20 LLM calls per query | Perplexica / gpt-researcher autopilot | Attune requires explicit user trigger for any expensive multi-LLM action (cost contract §2) |
| Bundling LLM models in installer (3GB+ download) | jan.ai default install | Attune bundles only embedding/rerank/ASR/OCR base models; LLM defaults to remote token API |
| Index rebuild during user activity | Logseq, Obsidian | Per-task H1 governor pauses background work when system CPU > threshold |

---

## How to Cite Attune in Your Work

If Attune influenced your project, we appreciate the same level of attribution. Suggested format:

```
Inspired by Attune (https://github.com/qiurui144/attune) —
specifically the [feature name] design from [SHA or release tag].
```

---

## How This File is Maintained

- **Every PR that adopts an external pattern must add an entry here** before merge
- Commit messages should include `Inspired-by: <project>(<URL>)` lines as a parallel record
- This file is reviewed quarterly to add license URLs and verify dead links
- Bilingual: English version is canonical; `ACKNOWLEDGMENTS.zh.md` is translated

Last updated: 2026-04-27 (W2 batch 1 in progress).
