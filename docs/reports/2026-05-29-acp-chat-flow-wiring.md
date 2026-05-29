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

**Real product entry, real HTTP, real server — not mock.**

1. Built the real `attune-server-headless` binary (`target/debug/attune-server-headless`, 250 MB).
2. Booted it with an isolated `HOME=/tmp/acp5-verify-home`, `--no-auth --port 18977`.
   - **Startup log (live evidence)**: `ACP-5: loaded 1 agent flows, 22 agents from workspace` — the wiring loads + validates the flow DAG at boot.
3. Drove the **product HTTP API** (not a backdoor): `POST /api/v1/vault/setup` → `POST /api/v1/vault/unlock` → `PATCH /api/v1/settings {llm}` → `POST /api/v1/chat`.
4. Sent a real defamation message: `"对方在朋友圈公开诽谤侮辱我，损害我的名誉权，我能索赔精神损害吗"`.

**Result (HTTP 200):**
```json
{
  "acp_flow": {
    "flow_id": "legal_defamation",
    "status": "degraded",
    "steps": [
      { "agent_id": "fact_extractor",       "ran": false, "note": "blocked (entitlement): agent fact_extractor requires a paid plan (tier…" },
      { "agent_id": "defamation_extractor", "ran": false, "note": "blocked (entitlement): agent defamation_extractor requires a paid plan…" }
    ],
    "final_type": "RawCaseText"
  }
}
```
Evidence JSON: `docs/reports/acp5-verify/chat-response-evidence.json`.

**What this proves:**
- The chat path now **routes through the flow engine**: a real defamation message resolved to the declared multi-step `legal_defamation` flow and ran through `run_chat_flow` → `run_flow`.
- **Entitlement gating works in production** (ACP-7): the test user is free (not paid), so the paid agent steps were blocked by entitlement and the flow **degraded** — recorded in the trace with the reason, **not a silent quality-drop** (§2.3). No panic, no cascade (§11 R8).
- **No regression**: the same response carried a 292-char `content`, a `citations` array, `cost_estimate`, and `grounding` — the free-form RAG path is fully intact; `acp_flow` is purely additive.
- The LLM-step *execution* path (paid → step runs + telemetry) is proven by the integration test `defamation_message_runs_flow_through_governed_runner` (upstream LLM call made + usage_events row written). The live free-user run proves the routing + degrade gate; together they cover both branches.

Test server stopped + isolated home discarded after verification (§4.3).

## 5. Commits (each task independently committed + pushed for durability)

| Task | SHA | Pushed |
|------|-----|--------|
| 1 — workspace flow loader | `65cb4ee` | `origin/acp-chat-flow-wiring` |
| 2 — run_chat_flow module | `e274a25` | `origin/acp-chat-flow-wiring` |
| 3 — chat.rs + state.rs wire | `2236c33` | `origin/acp-chat-flow-wiring` |

(Per §worktree isolation: commits live on the feature branch and are pushed there
for crash-durability; final `develop` integration is the merge step at completion,
left to the controller per the task's tag/merge guard.)

## 6. Green gate (real numbers)

| Suite | Result |
|-------|--------|
| `cargo test -p attune-core --lib` | **1499 passed, 0 failed, 1 ignored** (was 1496; +3 new agents loader tests + my locator) |
| `cargo test -p attune-server --lib` | **101 passed, 0 failed** (includes 6 new `acp_chat::tests`) |
| `cargo test -p attune-server --tests --no-fail-fast` (all integration binaries) | **261 passed, 0 failed**, 0 FAILED across ~30 test binaries (incl. new `acp5_chat_flow_wire_test` 3/3 + `acp4_governor_wire_test` 2/2) |
| `cargo clippy -p attune-core -p attune-server --lib -- -D warnings` | **clean, EXIT 0** |

Note: a full `cargo test -p attune-server` (lib + all integration binaries in one invocation) exceeds 30 min wall-clock due to ollama-touching + model-download integration tests; it was split (lib + `--tests --no-fail-fast`) and both are green. The only clippy *warning* anywhere is a **pre-existing** `unsafe { set_var }` in `tests/privacy_endpoints_test.rs:44` (untouched by this work; same pattern as the existing `acp4_governor_wire_test`) — not a regression, not in any modified file.

`agent_golden_gate`: not applicable — OSS attune-core ships no domain agent / golden gate (per CLAUDE.md). This work routes only (§2.3 — never reimplements an agent's computation), so no gate is affected.

## 7. v1.1.0 ready assessment

**The ACP chain on `origin/develop` (+ this branch) is now complete and green:** ACP-1…7 shipped, and the previously-missing chat-path wiring (ACP-6 concern #1) is closed — real chat requests flow through `resolve_flow → GovernedStepRunner → run_flow` with all 4 guarantees, graceful degrade, and entitlement gating, verified live on the real server.

**Ready for v1.1.0 tag consideration**, pending the §7.2 four-gate review by the controller:
- **Gate 1 (docs)**: this report added; RELEASE.md needs a `v1.1.0` Highlights/Breaking/Migration/Known-Limitations section before tag (NOT done here — left to release step).
- **Gate 2 (code)**: tests green (1499 + 101 + 261), clippy `-D warnings` clean on lib. ✅
- **Gate 3 (functional)**: live user-first verification of the headline feature (autonomous chat flow) passed end-to-end. ✅
- **Gate 4 (gaps / Known Limitations)**: (a) the canonical `legal_defamation` flow's agents are all `law-pro` (paid) — in OSS-only installs every step blocks on entitlement → the flow always degrades (correct, but means OSS users see no *completed* multi-step flow until attune-pro is installed and the user is paid). (b) deterministic agent steps in the server process have no embedded binary → degrade; full deterministic execution needs the agent-binary dispatch path (a follow-up). (c) the `acp_flow` block is wired into the JSON response but the **Web UI does not yet render it** — a frontend surfacing task (out of scope here).

**Recommendation**: merge `acp-chat-flow-wiring` → `develop`, then the controller decides on `develop → main --no-ff` + `v1.1.0` after adding the RELEASE.md section (Gate 1) and confirming the four gates. **This agent did NOT merge main or tag** (per task instruction — left to controller).
