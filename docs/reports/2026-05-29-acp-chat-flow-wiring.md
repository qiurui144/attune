# ACP-5 Chat-Path Flow Wiring — Production Assembly Report

**Date**: 2026-05-29
**Branch**: `acp-chat-flow-wiring` (worktree, off `origin/develop` @ `612df46`)
**Spec**: `docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md` §5.3b
**Concern addressed**: ACP-6 report concern #1 — wire the ready flow router into the chat path.

## 1. What was the gap

The ACP autonomous-flow engine (ACP-1…7) was fully shipped to `origin/develop`:
`flow.rs` (executor + 4 guarantees), `agent_flows.toml` (declarative DAG + typed
handoff), `resolve_flow` (intent→flow), production `GovernedStepRunner` (governor +
usage), and the ACP-7 cost-aware scheduler. **But `routes/chat.rs` still called the
old `IntentRouter` as a websocket observer only** — the flow engine was built but the
chat entrypoint never invoked it. A real chat request never flowed through
`resolve_flow → GovernedStepRunner → run_flow`.

## 2. The assembly (files + lines)

| File | Change |
|------|--------|
| `rust/crates/attune-core/src/agents/mod.rs` | + `locate_workspace_file()` (shared workspace-file locator) + `load_workspace_flows()` (load registry+flows, validate typed-handoff chain). Graceful `None`/`Err` when files absent. |
| `rust/crates/attune-server/src/acp_chat.rs` | **NEW** — `run_chat_flow()` production assembly: `resolve_flow` → (declared multi-step only) → `GovernedStepRunner` → `run_flow` along the typed-handoff DAG. `ChatFlowOutcome` (status + per-step trace + final payload) = the chat-response `acp_flow` block. Single-agent / no-match → `None` (backward compatible). |
| `rust/crates/attune-server/src/lib.rs` | + `pub mod acp_chat;` |
| `rust/crates/attune-server/src/state.rs` | + `AppState.agent_flows: Option<Arc<(FlowSet, AgentRegistry)>>`, loaded + validated once at startup via `load_workspace_flows`. `None` → chat uses free-form RAG only (never hard-fail). |
| `rust/crates/attune-server/src/routes/chat.rs` | After LLM resolution (≈line 196), call `run_chat_flow` in `spawn_blocking` (sync run_flow may issue governed LLM calls), entitlement derived from `member_state.is_paid()`, ACP-3 disabled set from vault settings. Attach `acp_flow` to `response_json` (additive — RAG/grounding/citations/cost untouched). |

### Design decision: augmentation, not replacement

The chat path's free-form RAG is the load-bearing OSS base capability. The flow
wiring is **purely additive**: only a *declared multi-step flow* (e.g.
`legal_defamation`) triggers the autonomous flow; a single-agent or no-match
resolution returns `None` and the unchanged RAG path runs. The `acp_flow` block is
attached to the response; RAG/grounding/citations/cost surfaces are never altered.
This guarantees zero regression to existing chat while delivering autonomous flow.

In an OSS server process there are no embedded law-pro agent binaries, so the
deterministic-dispatch closure returns an error → the flow degrades gracefully to a
partial result (spec §7 / §11 R8) and the trace records why. The LLM lead steps
still run + are telemetered.

## 3. TDD test list (RED → GREEN)

**attune-core `agents::tests` (Task 1, 4 tests):**
- `locate_workspace_file_finds_registry` — locator finds the SSOT registry.
- `locate_workspace_file_missing_is_none` — missing file → graceful `None`.
- `load_workspace_flows_loads_and_validates` — loads + validates typed-handoff chain (guarantee ①); legal_defamation present.
- `load_workspace_flows_missing_is_err` — missing files → `Err` (caller degrades), never panic.

**attune-server `acp_chat::tests` (Task 2, 6 tests):**
- `declared_flow_runs_end_to_end_with_trace` — ① flow runs through GovernedStepRunner, both steps trace `ran=true`, final type `Award`.
- `declared_flow_records_telemetry_for_llm_step` — ① usage aggregator records the LLM `extractor` step.
- `single_agent_returns_none_for_freeform_fallback` — ② single-agent → `None` (no regression).
- `no_match_returns_none` — ② irrelevant message → `None`.
- `quota_block_degrades_not_silent` — ③ free user → paid step blocked by entitlement → status != complete, block reason in trace (no silent quality-drop).
- `deterministic_dispatch_error_degrades_no_panic` — ④ deterministic dispatch error → partial, no panic, no cascade.

**attune-server `tests/acp5_chat_flow_wire_test.rs` (Task 3 integration, 3 tests):**
- `workspace_flows_present_for_chat_wire` — the chat wire has a real flow to route to.
- `defamation_message_runs_flow_through_governed_runner` — exact chat-route component graph (real provider + cache + usage + entitlement) runs legal_defamation, LLM step makes upstream call + records telemetry.
- `irrelevant_message_falls_back_to_freeform` — non-agent chat → no flow, zero LLM calls (no regression).

## 4. User-first verification (§2.2)

<!-- FILLED IN §6 below -->

## 5. Commits (each task independently committed + pushed for durability)

| Task | SHA | Pushed |
|------|-----|--------|
| 1 — workspace flow loader | `65cb4ee` | `origin/acp-chat-flow-wiring` |
| 2 — run_chat_flow module | `e274a25` | `origin/acp-chat-flow-wiring` |
| 3 — chat.rs + state.rs wire | `<TASK3_SHA>` | `origin/acp-chat-flow-wiring` |

(Per §worktree isolation: commits live on the feature branch and are pushed there
for crash-durability; final `develop` integration is the merge step at completion,
left to the controller per the task's tag/merge guard.)

## 6. Green gate (real numbers)

<!-- FILLED IN below -->

## 7. v1.1.0 ready assessment

<!-- FILLED IN below -->
