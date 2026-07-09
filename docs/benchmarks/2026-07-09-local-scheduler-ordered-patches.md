# Local Scheduler Ordered Patch Plan

Date: 2026-07-09

The worktree contains many unrelated existing changes. Review and commit the
local scheduler / long-text work in this order so behavior, build shape, and
test coverage stay understandable.

## Patch 1: Scheduler Runtime Boundary

Purpose: make Attune depend on the scheduler application API, not concrete local
workers.

Files:

- `rust/crates/attune-core/src/edge_cloud/scheduler.rs`
- `rust/crates/attune-core/src/edge_cloud/runtime_profile.rs`
- `rust/crates/attune-core/src/edge_cloud/kb_task.rs`
- `rust/crates/attune-server/src/local_scheduler.rs`
- `rust/crates/attune-server/src/scheduler_tasks.rs`
- `docs/local-scheduler-runtime-boundary.md`
- `scripts/scheduler-boundary-audit.sh`

Validation:

```bash
bash scripts/scheduler-boundary-audit.sh
cargo check --manifest-path rust/Cargo.toml -p attune-server --no-default-features --features scheduler-runtime
```

## Patch 2: Build Shape and RVV Audit

Purpose: keep target-specific optimization explicit and prove the shipped
RVA23 artifact has RVV evidence.

Files:

- `scripts/build-optimized.sh`
- `scripts/audit-rvv-vectorization.sh`
- `docs/build-optimization.md`
- `docs/benchmarks/2026-07-09-x100-build-artifact-analysis.md`

Validation:

```bash
bash scripts/build-optimized.sh --profile rva23 --package attune-server --features scheduler-runtime --no-default-features
bash scripts/audit-rvv-vectorization.sh rust/target/riscv64gc-unknown-linux-gnu/release/attune-server-headless
```

## Patch 3: Long-Text Retrieval and SRAS

Purpose: make long manual retrieval source-aware and resistant to duplicate
chunk hits.

Files:

- `rust/crates/attune-core/src/retrieval_plan.rs`
- `rust/crates/attune-core/src/search.rs`
- `rust/crates/attune-server/src/retrieval_policy.rs`
- `docs/benchmarks/2026-07-07-local-scheduler-long-context-architecture.md`

Validation:

```bash
cargo test --manifest-path rust/Cargo.toml -p attune-core source_hint_boost --lib
cargo test --manifest-path rust/Cargo.toml -p attune-core retrieval_plan
```

## Patch 4: Scheduler Answer Quality and Latency

Purpose: improve simple local KB response quality and p95 by avoiding slow LLM
generation when retrieval already identified the cited source.

Files:

- `rust/crates/attune-server/src/routes/chat.rs`
- `rust/crates/attune-server/ui/src/hooks/useChat.ts`
- `rust/crates/attune-server/ui/src/components/ChatMessage.tsx`
- `rust/crates/attune-server/ui/dist/index.html`

Validation:

```bash
cargo test --manifest-path rust/Cargo.toml -p attune-server local_scheduler_extractive_answer --lib
cargo check --manifest-path rust/Cargo.toml -p attune-server --no-default-features --features scheduler-runtime
npm run build --prefix rust/crates/attune-server/ui
```

## Patch 5: Airplane Manual Dataset and E2E

Purpose: make the long-text regression suite cover both API and Web UI paths
against a multi-thousand-page PDF knowledge base.

Files:

- `scripts/build-airplane-manual-longtext-dataset.py`
- `scripts/eval-airplane-manual-longtext-search.py`
- `scripts/eval-airplane-manual-longtext-chat.py`
- `tests/e2e/airplane_longtext_support.py`
- `tests/e2e/airplane_manual_longtext_cases.json`
- `tests/e2e/airplane_manual_longtext_e2e.py`
- `tests/e2e/playwright/airplane_manual_longtext_ui_e2e.py`
- `tests/e2e/playwright/airplane_manual_longtext_ui_e2e.js`
- `tests/e2e/README.md`
- `rust/tests/golden/airplane_manual_queries.json`

Validation:

```bash
python3 -m py_compile tests/e2e/airplane_manual_longtext_e2e.py tests/e2e/playwright/airplane_manual_longtext_ui_e2e.py
node --check tests/e2e/playwright/airplane_manual_longtext_ui_e2e.js
```

## Patch 6: Results and Architecture Records

Purpose: preserve benchmark context and decisions without mixing them into
runtime logic patches.

Files:

- `docs/benchmarks/2026-07-08-airplane-manual-longtext-kb-dataset.md`
- `docs/benchmarks/2026-07-09-local-scheduler-ordered-patches.md`
- any generated benchmark result JSON committed intentionally

Validation:

```bash
rg -n "k3|K3" docs rust tests scripts
```

The naming audit should only leave historical source-path references such as
`/data/RV/k3-scheduler` or externally named scheduler docs. Product-facing
Attune naming should use `local scheduler` / `scheduler`.
