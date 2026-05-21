# OSS attune 4 agent — real-LLM verification report (v1.0 GA gate)

**Date**: 2026-05-21
**Author**: AI verification run, attune develop @ post-`af415fe`
**Model**: Ollama `qwen2.5:3b` (Q4_K_M, 1.93 GB) on localhost:11434
**Spec basis**: `attune-pro/CLAUDE.md` 「Agent 验证铁律」 + `attune/CLAUDE.md`
**Test entry**: `rust/crates/attune-core/tests/oss_agent_real_llm_gate.rs`

## 0. Why this report exists

`attune-pro` law-pro #54 incident exposed the **"mock-only gate = false sense of
security"** trap: `defamation_extractor` passed every mock test but on real
Ollama `qwen2.5:3b` yielded F1=0.0923 with 4/10 JSON parse errors. The same
drill is required for OSS attune's 4 agents shipping in v1.0 GA so we don't
ship the same chest of confidence we shipped in law-pro.

## 1. Agent → LLM use audit

I started by checking each agent's source-of-truth code (`src/`) rather than
re-using the task's prior assumption that all 4 agents call an LLM. The result:

| # | Agent | Module | Production fn | Takes `&dyn LlmProvider`? |
|---|-------|--------|---------------|---------------------------|
| 1 | memory_consolidation | `memory_consolidation` | `generate_one_episodic_memory` | ✅ yes |
| 2 | internal_knowledge_linker | `linker::compute_links_for_item` | signature has **no** `LlmProvider` param | ❌ no — by design |
| 3 | chat_reliability | `chat_reliability::agent::evaluate_response` | signature has **no** `LlmProvider` param | ❌ no — by design |
| 4 | self_evolving_skill | `skill_evolution::agent::llm_expansion` | `&dyn LlmProvider` | ✅ yes (heuristic path also exists, no-LLM) |

Citations:
- `rust/crates/attune-core/src/linker/mod.rs:19-21` — “No LLM call on this path.”
- `rust/crates/attune-core/src/chat_reliability/mod.rs:19-21` — “must never accept
  an `LlmProvider` in its public API”.
- `rust/crates/attune-core/src/memory_consolidation.rs:158-165` — `llm.chat_with_history(...)` call site.
- `rust/crates/attune-core/src/skill_evolution/agent.rs:451-472` — `llm_expansion(&dyn LlmProvider, ...)`.

**Consequence**: only 2 of the 4 are LLM-dependent. The other 2 are deterministic
by explicit design contract and have no `chat()` to fail; their gate is
`linker_golden_gate.rs` / `chat_reliability_golden_gate.rs` running the real
(deterministic) code, no mocks. The task brief had assumed all 4 use an LLM —
this report corrects that and runs the right tests on the right agents.

## 2. Test harness

One integration test file: `rust/crates/attune-core/tests/oss_agent_real_llm_gate.rs`.

- 2 `#[ignore]` tests hit real Ollama — `agent_memory_consolidation_real_llm`,
  `agent_self_evolving_skill_real_llm`. Reproduce with:
  ```bash
  cargo test --test oss_agent_real_llm_gate -p attune-core \
    -- --ignored --nocapture --test-threads=1
  ```
- 2 regular tests are compile-time guards proving the deterministic agents
  retain a no-LLM signature — they cast the public `fn` pointer to a type with
  no `LlmProvider` parameter; any future refactor that adds an LLM dependency
  breaks the build.
- Prompts mirror **production** prompts byte-for-byte (memory:
  `memory_consolidation.rs::build_prompt`, skill:
  `skill_evolution/agent.rs::llm_expansion`). `parse_llm_terms` is `pub(crate)`
  so the test reimplements it inline with a comment about keeping the
  reimplementation in sync with prod (drift detection).
- **No production code was modified** — only test added.

## 3. Acceptance thresholds (set ex ante, never relaxed)

| Agent | Threshold | Per-case check |
|-------|-----------|----------------|
| memory_consolidation | ≥ 4/5 bundles produce a non-empty Chinese summary | 80 ≤ char-count ≤ 600 AND ≥ 30 CJK chars |
| self_evolving_skill | ≥ 4/5 fail-queries yield ≥ 2 keyword expansions | JSON parses, each term ≤ 30 chars, no echo of query |

Threshold rationale: 80% (4/5) leaves room for typical small-model
hallucination / JSON parse miss while still gating us above the law-pro-#54
shipping-broken-agent floor. If real LLM falls below 4/5, RELEASE.md must label
the agent **Beta** or defer to **v1.1** — the threshold is NOT moved.

## 4. Real-LLM results (2026-05-21 run)

### 4.1 Agent 1: memory_consolidation — **5/5 PASS**

5 real bundles seeded from realistic dev workflows (Rust ownership / SQLite WAL /
交通事故 / K3 IME INT8 / attune product positioning). Each bundle 5-6 chunks
matching `MIN_CHUNKS_PER_BUNDLE`.

| case | window | chunks | output chars | first 80 chars | verdict |
|------|--------|--------|--------------|----------------|---------|
| 1 | 1_780_000_000 | 6 | 290 | 用户这段时间学习的重点在于Rust语言中的所有权概念及其应用。首先深入理解了Rust Book第4章… | ✅ |
| 2 | 1_780_086_400 | 5 | 274 | 用户这段时间的关注点主要集中在SQLite的WAL模式及其在并发读写性能提升和实际应用中的表现… | ✅ |
| 3 | 1_780_172_800 | 6 | 240 | 用户这段时间重点关注了交通事故责任划分及赔偿处理的相关法律知识… | ✅ |
| 4 | 1_780_259_200 | 5 | 313 | 用户这段时间的关注点主要集中在SpacemiT K3的IME自定义指令vmadotu上… | ✅ |
| 5 | 1_780_345_600 | 5 | 255 | 用户在这段时间内重点关注attune与ChatGPT Desktop的差异化特性… | ✅ |

Mean length 274 chars (prompt asks for ~200; LLM trends a bit longer — within
the 80-600 acceptance window). All 5 are Chinese, single-paragraph, third-person
voice as the prompt requests. **No JSON parsing involved on this path**
(prompt asks for free-form prose), so the agent is not vulnerable to the
law-pro-#54 JSON parse-error failure mode.

### 4.2 Agent 4: self_evolving_skill — **5/5 PASS**

5 realistic search misses spanning tech / ML / ops / legal / office.

| case | query | parsed terms (count) | verdict |
|------|-------|---------------------|---------|
| 1 | `rust ownership` | ownership-rust, rust-ref-counts, borrow-cells, move-semantics, box-type (5) | ✅ |
| 2 | `transformer attention` | transformer机制, 注意力机制, 自注意力, Transformer神经网络, 注意力权重 (5) | ✅ |
| 3 | `k8s ingress nginx` | nginx ingress controller, k8s nginx ingresses, ingress controller kubernetes, kubernetes ingress resources, nginx ingress examples (5) | ✅ |
| 4 | `交通事故 主责` | 主導責任, 主要責任方, 車禍主責, 事故主因, 交通事故責任 (5) | ✅ |
| 5 | `vlookup 跨表` | 跨表查找, 交叉引用, 多表查询, 外部查找, 跨表匹配 (5) | ✅ |

100 % JSON parse success (vs law-pro-#54 60%). All 5/5 returned the maximum 5
terms, no echoes, all ≤ 30 chars. Case 4 emitted Traditional-character
synonyms (主導/車禍) for a Simplified-character query — semantically valid but
worth noting for future skill-evolution tuning if "term language must match
query language" is added as a hard constraint.

### 4.3 Agent 2: internal_knowledge_linker — N/A (deterministic, no LLM)

Compile-time guard `agent_internal_knowledge_linker_no_llm_dependency` passes:
public signature is

```rust
fn compute_links_for_item(
    store: &Store,
    vectors: Option<&VectorIndex>,
    new_item_id: &str,
    title: &str,
    content: &str,
    url: Option<&str>,
    thresholds: &LinkThresholds,
) -> Result<LinkerStats>
```

— zero `LlmProvider`. Existing gate `linker_golden_gate.rs` already runs the
real (deterministic) extractors against real fixtures, so the v1.0 ship gate for
this agent is **already** real-code-path verified.

### 4.4 Agent 3: chat_reliability — N/A (deterministic, no LLM)

Same story as 4.3. `evaluate_response(response, chunks, query, config) ->
ChatReliabilityReport`, no `LlmProvider` arg. Existing gate
`chat_reliability_golden_gate.rs` runs the real heuristic against real
fixtures.

## 5. Mock-vs-real behavior delta

The 2 deterministic agents have no mock-vs-real delta (they have no mock — they
run their actual code in tests).

For the 2 LLM agents:

| Agent | Mock-only golden gate result | Real-LLM result | Discrepancy? |
|-------|------------------------------|-----------------|--------------|
| memory_consolidation | passes with `MockLlmProvider` fixed responses | 5/5 real qwen2.5:3b produce on-target Chinese summaries | none — mock testing exercised the apply / idempotency path; real-LLM run confirmed the prompt yields usable output. **No JSON path means no #54-style JSON parse cliff.** |
| self_evolving_skill | passes with `MockLlmProvider` returning preset JSON | 5/5 real qwen2.5:3b produce valid 5-term JSON | none — but real LLM gives wider semantic variety (case 2's 注意力机制 set vs the Englishish synonyms tests had to invent). The `parse_llm_terms` fence-stripping path was exercised: real LLM output was fence-less plain `{...}` so the fence path stays untested by real LLM here. |

The single **real risk** law-pro-#54 surfaced — small models producing
non-JSON or partial JSON — would have shown up as 0-2 terms parsed; it did
not occur in this run. Confidence is moderate: 5 queries is a small sample,
but the bottom-of-distribution behavior (low term count, bad JSON) is the
failure mode that would matter, and we observed zero such cases.

## 6. v1.0 GA ship decision per agent

| # | Agent | Decision | Rationale |
|---|-------|----------|-----------|
| 1 | memory_consolidation | **Production (v1.0 GA ship)** | 5/5 real-LLM pass; no JSON parse cliff; prompt is structurally simple (one free-form paragraph). |
| 2 | internal_knowledge_linker | **Production (v1.0 GA ship)** | Deterministic by design, no LLM dependency, golden gate already covers real path. |
| 3 | chat_reliability | **Production (v1.0 GA ship)** | Same as 2. |
| 4 | self_evolving_skill | **Production (v1.0 GA ship) — recommend opt-in** | 5/5 real-LLM pass + the agent itself defaults to `enable_llm: false` per `SkillAgentConfig` (heuristic path is the default; LLM is an opt-in upgrade). Real-LLM works when users opt in. |

**No agent recommended for Beta or v1.1 deferral.**

## 7. Risks & known caveats (carried into RELEASE.md)

1. **Sample size** — 5 cases per LLM agent is the law-pro-#54 ceiling, not a
   statistical sample. Failure modes that occur at < 20 % rate could slip
   through. Mitigation: this gate is `#[ignore]`-runnable from the CLI so
   maintainers can re-run with larger inputs ad-hoc.
2. **Single-model verification** — only `qwen2.5:3b` Q4_K_M tested. attune
   wizard supports many providers (OpenAI / Anthropic / Gemini / other local
   Ollama models). The 2 LLM agents are designed for prompt-resilience, but
   we have not verified against other providers at GA. RELEASE.md should
   say "verified on qwen2.5:3b local; behavior on other providers/models may
   vary".
3. **No latency / cost telemetry** — this run was correctness-only. End-to-end
   memory consolidation cycle on real Ollama took 92 s for 5 bundles in
   sequence (~18 s/bundle on this dev box). That is consistent with the
   `MAX_BUNDLES_PER_CYCLE = 4` cap shipped in production. Production worker
   already serializes calls to respect Ollama queue.
4. **Test reimplements `parse_llm_terms`** because the prod fn is
   `pub(crate)`. If the prod parser is changed (e.g. stricter validation),
   the test copy must be updated in lockstep; otherwise verification drifts
   from production behaviour. Comment in the test file documents this.
5. **Skill agent multilingual** — case 4 (`交通事故 主责`, Simplified) returned
   Traditional-character synonyms. Not a bug under the current API
   contract, but if v1.x adds "term language matches query language", that
   becomes a real constraint to add to `parse_llm_terms` + this gate.

## 8. Reproduction commands

```bash
# Prerequisites:
ollama serve &
ollama pull qwen2.5:3b   # 1.93 GB, Q4_K_M

cd attune/rust

# Run the 2 deterministic-guard tests (no Ollama needed) — should always pass:
cargo test --test oss_agent_real_llm_gate -p attune-core
# expected: 2 passed; 2 ignored

# Run the 2 real-LLM gates:
cargo test --test oss_agent_real_llm_gate -p attune-core \
  -- --ignored --nocapture --test-threads=1
# expected: 2 passed; 0 failed
# wall-clock: ~90 s on a dev box, dominated by memory_consolidation (18s/bundle x 5)
```

## 9. Conclusion

**All 4 OSS attune v1.0 GA agents pass their reality check.** The 2 LLM-bound
agents (`memory_consolidation`, `self_evolving_skill`) get 5/5 on real Ollama
qwen2.5:3b matching their production prompts byte-for-byte. The 2 deterministic
agents (`internal_knowledge_linker`, `chat_reliability`) have no LLM dependency
to verify — their real-code-path is already covered by golden gate tests.

The law-pro-#54 failure mode (JSON parse errors / hallucinated `defamation`
verdicts) does **not** reproduce in OSS attune because:
- `memory_consolidation` doesn't ask for JSON (free-form prose only)
- `self_evolving_skill` asks for a much simpler 5-term JSON than law-pro's
  multi-field defamation extraction, and `parse_llm_terms` tolerates fenced /
  malformed output

This report attaches to v1.0 GA release notes as the "Agent reliability
evidence" entry. Test file `tests/oss_agent_real_llm_gate.rs` lands on
develop in the same commit so future maintainers can re-run on demand.
