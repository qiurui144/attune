# Agent Self-Learning Design — Per-User Adaptation Without Touching Correctness

> Status: **DESIGN PROPOSAL** — for review before implementation. Modifies nothing.
> Scope: attune-pro member agents (`law-pro` today; vertical-neutral by construction).
> Author intent (用户原话): "自学习 agents 一定要实现,因为 agents 的训练结果并不一定符合每个人的使用习惯。"
> Companion to: `attune-pro/docs/agent-reliability-framework.md` (the correctness spine this design must not break).

---

## 0. The honest premise

The user is right that a shipped agent will not match every person's working style. But the
demand "agents must self-learn" hides a fork that this design exists to make explicit:

- **An agent has a *correctness* surface and a *preference* surface.** They are not the same
  kind of thing and must never be learned by the same mechanism.
- The reliability framework already proves the correctness surface is *deterministic and
  gated*: `interest_calculator`, `limitation`, `evidence_chain`, `bank_aggregator` are pure
  functions with a 1.00 golden gate. A wrong answer there is a **bug**, never "a different
  user's habit".
- What genuinely varies per user is **everything that is not correctness**: which followups
  surface first, how verbose the output is, default form values, retrieval ranking
  preferences, the phrasing of the LLM extraction prompt. Adapting those is safe and is what
  the user actually wants.

So the design's promise is precise and deliberately narrow:

> **An agent self-learns a bounded, inspectable, reversible set of *preferences* — per user,
> in the vault. It never self-learns a single thing that the reliability gate verifies. The
> gate is therefore preserved by construction, not by hope.**

This is the same honesty discipline as the reliability framework: state the limit up front,
then design within it.

---

## 1. What exists today (substrate audit)

attune already has the machinery; the gap is that it is **skill-level**, not **agent-level**.

| Building block | Where | What it does | Reusable for agent learning? |
|----------------|-------|--------------|------------------------------|
| `skill_signals` table + `record_signal_event(kind, ref_id, query)` | `attune-core/src/store/signals.rs` | append-only event log; `KNOWN_SIGNAL_KINDS` whitelist (typo guard) | ✅ **the signal substrate** — add agent-scoped kinds |
| SkillClaw evolution (3-phase: prepare/generate/apply) | `attune-core/src/skill_evolution.rs` | search-miss signals → LLM → `learned_expansions` in `app_settings` | ✅ **the pattern** — copy the 3-phase lock-release shape |
| `start_skill_evolver` worker (4h, governor-gated, 3-phase lock release) | `attune-server/src/state.rs` | background worker that never holds the vault lock across an LLM call | ✅ **the worker template** |
| `merge_expansions_into_settings` (dedup, `truncate(8)` bound) | `skill_evolution.rs` | bounded merge — never lets the LLM blow up the table | ✅ **the bounding pattern** |
| Memory consolidation (idempotent, `INSERT OR IGNORE`) | `attune-core/src/memory_consolidation.rs` | per-day episodic memory; same 3-phase shape | ✅ reference for idempotency |
| `AgentOutput<T>` contract | `attune-core/src/agents/mod.rs` | `computation`, `confidence`, `red_lines_violated`, `missing_evidence`, `followups`, `audit_trail` | ✅ `followups` ordering + `confidence` display are adaptable surface |
| `submit_feedback` route + `insert_feedback` | `attune-server/src/routes/feedback.rs` | search-result feedback (item_id, feedback_type, query) | ✅ extend to carry agent feedback |
| `behavior/click` log | `attune-server/src/routes/behavior.rs` | click-through behavior log | ✅ a passive preference signal |
| `app_settings` meta blob (`get_meta`/`set_meta`) | `Store` | per-vault encrypted JSON settings | ✅ **the storage** — add a sibling `agent_preferences` blob |

**Gap:** all of the above learns *skills* (keyword expansion) or *memory* (episodic
summaries). Nothing learns a per-user *agent preference*. And critically — nothing today
even *could* learn the wrong thing, because none of it touches an agent's computation. This
design must keep that property when it adds the agent-level layer.

**Hard constraint from the reliability framework:** `civil_loan_agent::run` (read at
`law-pro/src/civil_loan_agent.rs`) calls `calculate(input.facts)` — a pure deterministic
function. The agent's `Input` (`CivilLoanInput`) carries *facts*, not tunable parameters.
The LPR table, the ×4 cap, the formula selection are `const`. **There is no parameter on
the correctness path that a learning mechanism could reach** — and the design's job is to
make sure it stays that way (§6).

---

## 2. The safe-vs-forbidden surface (the critical boundary)

This table is the heart of the proposal. A change is admissible into the self-learning
mechanism **if and only if** it appears in the left column.

### 2.1 MUST NOT self-learn — the forbidden surface

| Forbidden | Where | Why a learned mutation here is a catastrophe |
|-----------|-------|----------------------------------------------|
| Deterministic formulas | `interest_calculator::calculate`, simple/compound interest math | Silently changing how interest is computed = wrong legal number presented as authoritative. Legal-liability event. |
| Red lines & their thresholds | `interest_calculator` 4 red lines; `hard_red_lines` in `plugin.yaml` | A red line is a *safety stop*. Learning to relax it = the agent stops refusing cases it must refuse. |
| Statute / constant mappings | `ONE_YEAR_LPR_TABLE`, 3-year/20-year limitation, LPR×4 cap, `CIVIL_LOAN_EXPECTED_KINDS` | These are law, not preference. They change only via the PBOC/legislature, never via one user's habit. |
| Evidence-sufficiency logic | `evidence_chain` relation/gap logic, `bank_aggregator` 笔数-zero-error rule | Determines whether evidence is adequate. A learned shortcut here fabricates sufficiency. |
| `confidence` *computation* (not display) | `AgentOutput.confidence` derivation | Confidence drives CP-1 deferral. Learning to inflate it = the lawyer-in-the-loop is bypassed. |
| The grounding contract | `fact_extractor::verify_grounding` ("无依据不出数字") | A wrong extraction must still be voided. Learning to skip grounding = hallucinated numbers flow into calculations. |
| Golden-gate thresholds | `thresholds.yaml`, `τ` | Changing the gate is a governance act (CP-7), never an automatic one. |

**One-line rule for code review:** *if a value is verified by the golden gate, it is not
learnable. Full stop.*

### 2.2 CAN safely self-learn — the adaptive surface

Every item here is a **preference**: getting it "wrong" produces a less convenient output,
never an *incorrect* one. None is read by `calculate()` or any deterministic agent's logic.

| Adaptive | What adapts | Signal that drives it | Where stored |
|----------|-------------|----------------------|--------------|
| **Followup ordering** | reorder `AgentOutput.followups` so the ones this user acts on appear first | user clicks / dismisses a followup | `agent_preferences.followup_rank` |
| **Followup suppression** | demote (never delete) a followup the user has dismissed N times | repeated dismiss | `agent_preferences.followup_rank` (negative weight, floored) |
| **Output verbosity** | terse vs detailed `audit_trail` / explanation rendering | user expands/collapses detail; explicit verbosity toggle | `agent_preferences.verbosity` |
| **Form default values** | pre-fill `civil_loan` form fields with the user's common values (e.g. their usual jurisdiction, a frequently-entered rate *as a default the user still sees and can change*) | accepted form submissions | `agent_preferences.form_defaults.<agent>` |
| **Retrieval / ranking preference** | per-user re-rank weighting of evidence retrieval feeding an agent (which doc *kinds* this user trusts first) | citation_hit, click_through, feedback | `agent_preferences.retrieval_weights` |
| **Followup phrasing** *(LLM-step only)* | for LLM-based agents (`fact_extractor`), adjust the *prompt phrasing* of the extraction step toward the user's domain vocabulary | corrections on extracted fields | `agent_preferences.prompt_hints.<agent>` |
| **UI defaults** | which agent panel is expanded, default `case_kind`, default chat-trigger sensitivity | UI interaction | `agent_preferences.ui` |

**Why form defaults are safe even though they touch the calculator's input:** a learned
default is a *pre-filled, visible, editable* value. The user confirms or overrides it before
the agent runs. The calculator still receives whatever the user submitted — the learning
changed the *starting point of a form*, not the *computation*. The boundary holds because
the human is in the loop between the learned default and the deterministic call. If a
future change ever made a learned value flow into `calculate()` *without* a user
confirmation step, that change crosses into §2.1 and must be rejected in review.

### 2.3 The structural test

A proposed adaptation is safe **iff** all three hold:

1. **It is not in the golden gate.** Grep the C2/C3 scorer inputs — if the value is scored,
   it is forbidden.
2. **It is read on the *presentation* path, not the *computation* path.** `followups`
   ordering, `audit_trail` verbosity, form pre-fill, retrieval re-rank → presentation.
   `calculate()` arguments, red-line checks, `confidence` derivation → computation.
3. **A wrong learned value degrades convenience, not correctness.** If you can construct an
   input where a wrong learned value makes the agent output a *wrong number/verdict*, it is
   forbidden.

If any test fails, the item does not enter the mechanism. This test is added to the PR
review checklist (§7) and, where possible, enforced by a compile-time boundary (§6.2).

---

## 3. Signal → adaptation mechanism

### 3.1 Learning signals (extend the existing substrate, do not invent)

Add agent-scoped kinds to `KNOWN_SIGNAL_KINDS` in `signals.rs` (the whitelist already
rejects typos):

| New kind | Emitted when | `ref_id` | `query` |
|----------|--------------|----------|---------|
| `agent_followup_accept` | user clicks/acts on a followup | `<agent_id>:<followup_hash>` | — |
| `agent_followup_dismiss` | user dismisses a followup | `<agent_id>:<followup_hash>` | — |
| `agent_field_correct` | user edits an LLM-extracted field | `<agent_id>:<field_name>` | corrected value (bounded) |
| `agent_output_expand` / `agent_output_collapse` | user toggles `audit_trail` detail | `<agent_id>` | — |
| `agent_form_submit` | user submits an agent form | `<agent_id>` | (form values go to a separate bounded path, not `query`) |
| `agent_reject` | user explicitly marks an agent result unhelpful | `<agent_id>` | — |

These are **append-only, non-blocking** (`let _ = store.record_signal_event(...)` — the
existing convention; a failed signal write never breaks the agent). Existing kinds
(`citation_hit`, `click_through`, `feedback`) are reused for the retrieval-weight signal.

**Critical isolation:** `get_unprocessed_signals` already filters `kind='search_miss'` (the
R17 P0 fix). The agent kinds therefore **do not pollute the SkillClaw evolver** — they are
consumed by a *separate* worker (§3.3). This separation is free because the substrate was
already built kind-aware.

### 3.2 What gets adapted, and how (bounded, monotone-safe transforms)

Each adaptation is a **small, bounded, reversible transform** — never an LLM rewriting the
agent. The bounding patterns are copied directly from `merge_expansions_into_settings`
(dedup + `truncate`).

- **Followup rank** — a per-`(agent_id, followup_hash)` integer weight in `[-3, +3]`.
  `accept` → `+1` (capped at +3), `dismiss` → `-1` (floored at -3). Rendering sorts
  `followups` by weight desc, stable. A floored-at-`-3` followup is *demoted to the bottom*,
  **never removed** — the agent's safety followups (e.g. "请律师核实") must always remain
  visible. **Hard rule: followups originating from `red_lines_violated` or
  `red_line_warnings` are exempt from ranking entirely** — they render first, always,
  regardless of learned weight.
- **Verbosity** — a 3-state enum `{terse, normal, detailed}`, moved one step by a 5-event
  hysteresis (5 consecutive expands → step toward detailed). Discrete, tiny state space,
  trivially inspectable.
- **Form defaults** — last-N accepted submissions per field; the *mode* (most frequent
  value) becomes the default, only if it appears in ≥60% of the last 10 submissions
  (a stability gate, so one-off entries don't become defaults). Stored as a plain value the
  UI pre-fills and the user sees.
- **Retrieval weights** — per-evidence-`kind` multiplier in `[0.5, 1.5]`, nudged ±0.05 per
  `citation_hit` / `feedback`, clamped. Applied as a re-rank *after* the base retrieval
  score — it reorders candidates, never removes them, and never feeds the agent a different
  *set* of facts, only a different *order*.
- **Prompt hints (LLM agents only)** — an *additive* hint string appended to the
  `fact_extractor` extraction prompt: a short, deduped, `truncate(5)` list of
  user-vocabulary terms derived (LLM-assisted, like SkillClaw) from `agent_field_correct`
  signals. It influences *phrasing*, never the grounding contract — `verify_grounding` still
  voids any field whose quote is not in the source. The prompt hint can make the LLM *try
  harder*; it cannot make a hallucination pass.

**Why no LLM rewrites the agent:** the only LLM use is the same as SkillClaw — turning a
batch of correction signals into a bounded term list. The agent's *code* is never
generated, mutated, or selected by the learning loop.

### 3.3 The worker — clone the SkillClaw 3-phase shape

Add `start_agent_adapter` in `state.rs`, structurally identical to `start_skill_evolver`:

```
loop every 6h (governor-gated: TaskKind::SkillEvolution shares the budget):
  Phase 1 (vault lock): read unprocessed agent_* signals → bundle by agent_id
  Phase 2 (no lock):    pure transforms (§3.2); LLM call ONLY for prompt-hint derivation
  Phase 3 (vault lock): merge into agent_preferences blob (bounded), mark signals processed
```

Same lock discipline (never hold the vault lock across the LLM call), same governor gating
(global Pause stops it; H1 LLM quota applies), same idempotency (signals marked
`processed=1`). Most adaptations (rank, verbosity, form defaults, retrieval weights) are
*pure arithmetic* and need no LLM at all — only the prompt-hint path touches the LLM, so the
worker is cheap and the cost contract (§5) is naturally satisfied.

---

## 4. Storage, transparency, reset

### 4.1 Storage — per-user, in the vault, encrypted, bounded

A new top-level key in the existing `app_settings` blob (or a sibling `agent_preferences`
meta key — implementation choice), encrypted at rest by the vault DEK like all
`get_meta`/`set_meta` data. **Never leaves the device.** Never synced to any external
product (the attune data-isolation rule in `CLAUDE.md` — no lawcontrol bridge, no cloud).

```json
{
  "agent_preferences": {
    "schema_version": 1,
    "followup_rank":    { "civil_loan_agent:<hash>": 2, "...": -1 },
    "verbosity":        { "civil_loan_agent": "detailed" },
    "form_defaults":    { "civil_loan_agent": { "jurisdiction": "CN" } },
    "retrieval_weights":{ "borrowing_doc": 1.15, "bank_statement": 0.95 },
    "prompt_hints":     { "fact_extractor_agent": ["民间借贷", "月息"] },
    "ui":               { "default_case_kind": "civil-loan" }
  }
}
```

Every sub-tree is **size-bounded** (rank map capped at 200 entries LRU; hint lists
`truncate(5)`; retrieval weights clamped; form defaults one value per field). The blob
cannot grow unboundedly — a poisoned signal stream can at most fill bounded slots, never
exhaust storage or degrade beyond the clamps.

`schema_version` lets a future agent version migrate or discard preferences cleanly.

### 4.2 Transparency — the 1Password-style value, made literal

A new Settings tab **"Agent learning"** (`attune-server/ui/src/`, all strings via `t()` per
the i18n rule). It must show, in plain language, **everything the agent has learned about
this user**:

- "This agent now shows *‹followup X›* first because you acted on it 4 times."
- "Output is set to **detailed** because you usually expand the reasoning."
- "Form field *jurisdiction* defaults to **CN** (your last 9 of 10 submissions)."
- "Evidence of kind *借条* is ranked slightly higher for you."
- A read-only view of `prompt_hints` ("learned vocabulary: 民间借贷, 月息").

Nothing is learned that cannot be rendered as a human-readable sentence here. If an
adaptation cannot be explained in one sentence, it does not belong in the mechanism. This is
the design's transparency *gate*: explainability is a precondition for an adaptation, not an
afterthought.

The view also shows **what is NOT learned** — a short static note: *"Legal formulas, red
lines, and statute mappings are fixed and verified — they never change based on your
usage."* This sets the correct user expectation and matches the reliability framework's
honesty.

### 4.3 Reset — granular and total

- **Per-item reset** — remove one learned preference (e.g. un-rank one followup).
- **Per-agent reset** — clear all preferences for one agent.
- **Total reset** — `agent_preferences` back to empty `{}`; the agent reverts to its
  shipped behavior exactly.

Reset is a plain `set_meta` write of the pruned blob — instant, local, no LLM. Because the
agent's *correctness path never read the preferences in the first place*, reset can never
"break" an agent; it only changes presentation back to the default.

**Reversibility is structural:** preferences are a *separate, optional, presentation-layer
overlay*. The agent runs identically with the overlay empty. There is no migration, no
retraining, no irreversible state — deleting the blob is a complete, safe undo.

---

## 5. Cost-contract compliance

Per `CLAUDE.md` §"成本感知与触发契约", self-learning must not become a third-tier
(LLM/money) cost that runs behind the user's back.

- **Signal recording** — tier 🆓 (a SQLite insert). Always on.
- **Adaptation worker** — tier ⚡ at most. Rank/verbosity/form/retrieval transforms are
  pure CPU, sub-millisecond. The *only* LLM use is prompt-hint derivation, which is
  batched, 6-hourly, governor-gated, and shares the existing `TaskKind::SkillEvolution` LLM
  budget — it does not add a new uncapped cost. It obeys the global "暂停后台任务" switch.
- **No tier 💰 surprise** — the learning loop never triggers a Chat call, never calls a
  paid API per user action. It rides the same background quota SkillClaw already uses.
- The Agent-learning Settings tab can show "last adapted: ‹time›" so even the background
  activity is visible — consistent with "顶栏后台任务队列可见 + 可暂停".

---

## 6. Interaction with the reliability gate (why correctness is preserved by construction)

This is the section the reviewer should scrutinize hardest.

### 6.1 The claim

> A self-learned change can never fail the golden-set regression gate, because the gate
> verifies the *computation* surface and the mechanism only writes the *preference* surface.
> The two surfaces are disjoint. Therefore the gate is preserved automatically — not by
> running it against learned state, but because learned state is *invisible* to it.

### 6.2 Why it holds — and how to *enforce* that it holds

The claim is only true if the disjointness is real and stays real. Three enforcement
layers, strongest first:

1. **The deterministic agents have no learnable input.** `calculate()` takes `InterestFacts`
   — facts the user supplied. The LPR table, cap, formulas are `const`. The learning
   mechanism writes `agent_preferences`; **no deterministic agent reads `agent_preferences`
   at all.** This is a structural fact today and the design preserves it: the preference
   blob is read by the *rendering layer* and the *retrieval re-ranker*, never passed into an
   `Agent::run` computation path.

2. **A compile-time boundary.** Put `agent_preferences` access behind a typed
   `PreferenceLayer` accessor that is only constructible in the server's presentation/route
   layer, not in `attune-core/src/agents/` or any `law-pro` agent crate. An agent crate that
   tries to read a preference fails to compile. This turns the §2.3 rule into a property the
   compiler checks, not a convention reviewers must remember.

3. **The golden gate runs on the agent, preference-free.** `agent_golden_gate.rs` (C3)
   constructs the agent and feeds it golden-case facts. It does **not** load a user vault,
   so `agent_preferences` is structurally absent in the gate run. The gate measures the
   agent exactly as shipped. A user's learned preferences cannot reach it.

### 6.3 The one place that needs an explicit guard — the LLM prompt hint

`fact_extractor` *is* on the gated surface (C3 nightly LLM lane) **and** the prompt-hint
adaptation touches its prompt. This is the single crossing point and must be contained:

- The prompt hint is **additive phrasing only** — appended user-vocabulary terms. It cannot
  remove or weaken `verify_grounding`. A field whose quote is absent from the source is
  still voided. So the hint can raise recall on a user's domain terms; it **cannot** make a
  wrong extraction pass — the grounding contract is downstream of the hint and is itself on
  the forbidden surface (§2.1).
- **Gate guard:** the C3 nightly `fact_extractor` lane runs with `prompt_hints` **empty**
  (the gate's "shipped agent" baseline) **and additionally** with a *synthetic adversarial
  hint set* (irrelevant/noisy terms) to prove the precision floor (≥0.95) holds even under a
  pathological hint. If precision drops below floor with an adversarial hint, the prompt-hint
  feature is *capped harder* (shorter `truncate`, stricter dedup) or disabled — the gate
  governs the feature, exactly as the reliability framework intends. This makes the prompt
  hint a *gated* adaptation, not an ungoverned one.

### 6.4 Net effect

For the four deterministic agents, self-learning and the 1.00 gate are *non-interacting* —
provably, by the compile-time boundary. For the one LLM agent, self-learning is *gated* —
the nightly lane proves the precision floor survives any hint. Either way, **"self-learning
degrades correctness" is not a residual risk to monitor; it is a state the design makes
unreachable.**

---

## 7. Honest risk assessment

| Risk | Severity | Containment |
|------|----------|-------------|
| Scope creep — a future PR adds a "learnable" value that is actually on the computation path | **High** | §2.3 three-test checklist in PR review **+** §6.2 compile-time `PreferenceLayer` boundary. The compiler is the real defense. |
| Signal poisoning — erratic clicks teach a bad preference | Low | All transforms clamped/floored/hysteresis-gated; worst case is a mildly inconvenient ordering, fully visible in the Settings tab, one-click reset. Safety followups are rank-exempt. |
| User confusion — "why did the agent change?" | Medium | The transparency tab (§4.2) renders every learned item as a sentence; explainability is a precondition (§4.2), not optional. |
| Prompt-hint drift lowering `fact_extractor` precision | Medium | §6.3 adversarial-hint gate guard in the nightly lane; gate caps or kills the feature if the floor breaks. |
| Preference blob growth | Low | Every sub-tree size-bounded (§4.1); a poisoned stream fills bounded slots, never unbounded. |
| Cross-user / cross-product leakage | **High if it happened** | Stored only in the per-user vault, DEK-encrypted; never synced (CLAUDE.md isolation rule). No code path writes it outside `get_meta`/`set_meta`. |
| Reset doesn't fully revert | Low | Reset = `set_meta` of pruned blob; agent correctness path never read it, so revert is structurally complete. |

**The one risk that matters:** scope creep. Every other risk degrades convenience and is
visible + resettable. Scope creep is the only one that could touch correctness — and it is
contained by making the boundary a *compile error*, not a *guideline*.

---

## 8. Phased implementation plan

Ordered by risk-reduction per unit effort; each phase independently shippable.

### Phase 1 — Boundary first (no learning yet)

Build the *containment* before the *capability*, so learning can never be added unsafely.

- Add `agent_preferences` schema (`schema_version: 1`) + `get`/`set`/`reset` on `Store`.
- Add the `PreferenceLayer` typed accessor with the compile-time boundary (§6.2) — agent
  crates physically cannot read it.
- Add the new `agent_*` kinds to `KNOWN_SIGNAL_KINDS` (typo guard already enforces it).
- Confirm `agent_golden_gate.rs` (reliability framework C3) runs preference-free — assert it
  in a test (`no vault → no preferences`).

*Exit:* the storage + boundary exist; nothing learns yet; the gate is proven preference-blind.

### Phase 2 — Pure-arithmetic adaptations (no LLM, lowest risk)

- Emit `agent_followup_accept/dismiss`, `agent_output_expand/collapse`, `agent_form_submit`
  signals from the UI/routes.
- `start_agent_adapter` worker (3-phase, governor-gated) doing **only** the pure transforms:
  followup rank, verbosity hysteresis, form defaults. No LLM call in this phase.
- Rendering layer reads `followup_rank` / `verbosity` via `PreferenceLayer`; red-line
  followups rank-exempt.
- Settings "Agent learning" tab: view + per-item / per-agent / total reset.

*Exit:* agents adapt followup order, verbosity, form defaults per user; fully visible and
resettable; zero LLM cost; gate untouched.

### Phase 3 — Retrieval re-ranking

- Reuse `citation_hit` / `click_through` / `feedback` signals → `retrieval_weights`.
- Apply clamped per-kind re-rank *after* base retrieval feeding agents.
- Extend the transparency tab.

*Exit:* per-user evidence-kind ranking preference; still presentation-only, still gate-free.

### Phase 4 — LLM prompt hints for `fact_extractor` (the one gated adaptation)

Only after the reliability framework's Phase 3 (the `fact_extractor` nightly LLM gate) is
live — the gate must exist before this feature ships.

- `agent_field_correct` signal → batched LLM-assisted vocabulary derivation (SkillClaw
  pattern) → bounded `prompt_hints`.
- Additive prompt-hint injection into the `fact_extractor` extraction step.
- §6.3 gate guard: nightly lane runs empty-hint baseline **and** adversarial-hint stress;
  precision floor (≥0.95) must hold both ways or the feature is capped/disabled.

*Exit:* the LLM agent adapts phrasing to the user's vocabulary, *proven* by the nightly gate
not to lose precision.

---

## 9. Summary — the promise, precisely

- **Self-learning is real and per-user** — followup order, verbosity, form defaults,
  retrieval ranking, and (for the LLM agent) prompt phrasing all adapt to the individual.
  This is genuinely what the user asked for.
- **It is bounded, inspectable, reversible** — every learned value is clamped, rendered as a
  plain sentence in a Settings tab, and resettable to shipped behavior in one click.
- **It cannot degrade correctness — by construction.** The deterministic agents do not read
  the preference layer (compile-time enforced); the golden gate runs preference-blind; the
  one LLM crossing point is itself gated by an adversarial-hint nightly check.
- **The honest limit:** self-learning makes the agent *more convenient for this user*. It
  does not, and must not, make the agent *more correct* — correctness is the gate's job, and
  the gate stays exactly where the reliability framework put it.

The design contains its own central risk (scope creep) by turning the safe/forbidden
boundary into a compiler error rather than a reviewer's memory. That is the difference
between "we intend not to break correctness" and "correctness cannot be broken here".
