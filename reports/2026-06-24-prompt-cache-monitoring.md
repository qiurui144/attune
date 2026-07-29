# Prompt-Cache Monitoring + Prefix Stability (audit #1, token-economy P0)

**Date**: 2026-06-24
**Worktree**: `/data/attune-wt-promptcache` (branch `feat/prompt-cache-monitoring`, off `origin/develop` @ `3ae9d27`, behind=0)
**Scope**: attune-core only. Request-side vendor prompt-cache observability + prefix stability. **Not pushed, not merged.**
**Source eval**: `docs/superpowers/specs/2026-06-24-ai-infra-architecture-eval.md` §1 G1 / §3 G7 (item #1 in the优化表).

---

## 1. Provider cache-semantics research (context7: DeepSeek + Anthropic)

| Provider (production via new-api gateway) | Cache model | Where the hit count lives in `usage` | Need `cache_control`? |
|---|---|---|---|
| **DeepSeek** (deepseek-v4) | **Automatic context cache** — stable prefix hits with no marker | **top-level** `usage.prompt_cache_hit_tokens` (+ `prompt_cache_miss_tokens`) | **No** — just keep the prefix stable |
| **OpenAI ≥ 2024-10** (and OpenAI-compat) | Automatic prefix cache | **nested** `usage.prompt_tokens_details.cached_tokens` | No |
| **Qwen** (via new-api) | OpenAI-compat passthrough | nested `prompt_tokens_details.cached_tokens` when forwarded; else absent | No |
| **Anthropic** (NON-production; Claude Code harness only) | **Manual** — needs explicit breakpoint | reports `cache_read_input_tokens` / `cache_creation_input_tokens` | **Yes** — `cache_control:{type:"ephemeral"}` on the last block of the cacheable prefix (system / fixed lead turns) |

**Conclusion**: the production backends (DeepSeek/Qwen) are **auto prefix-cache** — the only two levers are (a) **stable message prefix** and (b) **reading the right cached-token field**. The old parser read ONLY the nested OpenAI form, so **DeepSeek cache hits were silently dropped to 0** (DeepSeek populates only the top-level field). That alone made the §4.5-G3 signal unobservable for the primary backend.

(DeepSeek source: api-docs.deepseek.com `/api/create-chat-completion` + `/guides/kv_cache`. Anthropic source: platform.claude.com Messages API — `system` as text-block array, breakpoint on last prefix block.)

## 2. Prefix-stability audit (chat + retry-validate loop)

**Audited** `LlmProvider::chat_with_retry` (llm.rs:355-394). **Already prefix-stable** — no fix needed:
- attempt 1 → `chat(system, user)`; attempt ≥2 → `chat_with_history([system, user, assistant(prev), user(error-feedback), …])`.
- Validator feedback is **appended** (assistant + user pair), the `[system, user]` prefix is **never rewritten**. This is exactly what auto prefix-cache needs.

The eval's §4.5-G2 *separate* gap (flow_runner stateless single-turn, item #2) is **out of scope** for this task and untouched.

**Added a regression test** to PIN this invariant so a future refactor can't silently bust cache reuse: `retry_loop_keeps_stable_prefix_across_attempts` — asserts identical system+first-user across all 3 attempts, append-only growth (2→4→6 msgs), and attempt-N extends (not rewrites) attempt N-1. Backed by a new append-only `MockLlmProvider::call_log()`.

## 3. cache_hit_rate monitoring implementation

**Key correction**: `UsageSummary.cache_hit_rate` already existed but measures attune's **own full-response cache** (the `cache` Hit/Miss/Bypass column from governor L1/L2) — **NOT** the vendor prefix-cache. `cached_in` was persisted per-row (`usage_events`) yet **never aggregated**, so the §4.5-G3 vendor signal had no roll-up.

Implemented (store/usage.rs `usage_summary`):
- New field **`prompt_cache_hit_rate = sum(cached_in) / sum(tokens_in)`** — **token-weighted** (a 768-cached call and a 0-cached call are not "half cached"), 0.0 when no input tokens (no div-by-zero).
- Kept **distinct** from the response-cache `cache_hit_rate`. Both ride the existing `GET /api/v1/usage/summary` surface (camelCase `promptCacheHitRate`) — no new endpoint, UI/CLI consumers get it for free.
- §4.5-G3 target ≥ 0.50 on stable-prefix multi-turn is now **observable**.

## 4. Anthropic cache_control disposition (§4.5-G2 completeness)

Production never speaks native Anthropic (gateway proxies it as OpenAI-compat → auto cache). So implemented as an **opt-in, off-by-default** helper rather than touching the hot path:
- `build_anthropic_cached_request(messages, model)` behind feature `anthropic-cache` (Cargo.toml, default OFF).
- Emits Anthropic-native JSON: `system` = text-block array with `cache_control:{type:"ephemeral"}` on the prefix block; conversational turns as content-block arrays; system filtered out of `messages`; no marker leakage onto convo turns.
- Pure builder, zero new deps, **excluded from default build** → thin-deb unaffected.

## 5. Tests

| Test | What it locks |
|---|---|
| `usage_parses_openai_nested_cached_tokens` | nested `cached_tokens` → `cached_in` |
| `usage_parses_deepseek_top_level_cache_hit_tokens` | **DeepSeek** top-level `prompt_cache_hit_tokens` → `cached_in` (was 0) |
| `usage_parses_anthropic_gateway_cache_read_tokens` | `cache_read_input_tokens` → `cached_in` |
| `usage_cached_absent_is_zero` / `..._nested_takes_precedence...` | absent → 0; nested wins when both present |
| `usage_summary_prompt_cache_hit_rate_is_token_weighted` | token-weighted roll-up = 0.5, independent of response-cache |
| `usage_summary_prompt_cache_hit_rate_zero_when_no_input_tokens` | div-by-zero boundary |
| `retry_loop_keeps_stable_prefix_across_attempts` | retry appends, never rewrites prefix |
| `anthropic_request_puts_cache_breakpoint_on_system_prefix` (feature-gated) | breakpoint on system prefix only |

**Verification (real exit codes)**:
- `cargo test -p attune-core --lib` → **2613 passed, 0 failed, 2 ignored**.
- `cargo test -p attune-core --features anthropic-cache --lib llm::` → **40 passed, 0 failed**.
- `cargo clippy -p attune-core --all-targets -- -D warnings` → **exit 0** (after collapsing a pre-existing `if_same_then_else` in `platform/power.rs` surfaced by rust-1.95.0; unrelated to cache, fixed in its own commit, behavior-identical).
- `cargo build -p attune-core --no-default-features` → **clean**.

## 6. Honesty note

This work makes vendor prompt-cache **observable** and keeps the prefix **stable** + **fixes the DeepSeek parser blind spot**. It does **not** claim a measured cost reduction — that requires a real new-api gateway run (multi-turn, step ≥3) reading `promptCacheHitRate`. The tests prove the *monitoring + parsing logic* is correct, not a production hit-rate number.

## Commits (branch `feat/prompt-cache-monitoring`)

| SHA | Commit |
|---|---|
| `dfc538a` | fix(llm): parse DeepSeek/Anthropic top-level prompt-cache token fields |
| `f9c697d` | feat(usage): roll up vendor prompt-cache hit rate (§4.5-G3) |
| `796ad34` | test(llm): lock retry-loop prefix stability (prefix-cache prerequisite) |
| `bb61d4d` | feat(llm): feature-gated Anthropic cache_control request builder (§4.5-G2) |
| `0cdd89e` | fix(power): collapse identical if-branches in linux power probe (clippy) |
