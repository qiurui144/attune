# attune-agent-sdk

WASM-safe leaf crate carrying the `Agent` trait + `AgentOutput<T>` + a wasm-safe
error type (`AgentError` / `AgentResult`) so attune deterministic agents (pure
computation — interest / limitation periods / evidence chains / patent claim
extraction, etc.) can compile to `wasm32-wasip1` and ship as a single `.wasm` that
runs on every platform (Windows / Linux / riscv64 K3) via the embedded wasmtime
runtime.

> 中文:这是 attune 的 wasm-safe agent leaf crate。承载 `Agent` trait +
> `AgentOutput<T>` + 轻量错误类型,使确定性 agent 可编 `wasm32-wasip1`,
> 享受"一包通吃所有平台"分发。设计见
> `docs/superpowers/specs/2026-06-01-wasm-safe-agent-leaf-crate.md`。

## Invariant — zero native dependencies

This crate depends on **only** `serde` + `thiserror` (a pure proc-macro with no
runtime native code). The following are a hard **deny-list** — none may ever be
added, because they pull native-only code that breaks the `wasm32-wasip1` build
(this is exactly why agents could not compile to wasm while living inside
`attune-core`):

```
rusqlite  tokio  reqwest  usearch  tantivy  hdbscan  socket2  serde_yaml  chrono
```

CI enforces this: the `rust-test` job runs
`cargo build -p attune-agent-sdk --target wasm32-wasip1`; any native dep flowing
back in turns the build red.

## Relationship to attune-core

`attune-core` depends on this crate and `pub use`-re-exports the **same** types,
so the existing `attune_core::agents::{Agent, AgentOutput}` import paths are
unchanged and downstream `impl Agent for ...` keep pointing at the same trait.
`attune-core` defines `From<AgentError> for VaultError` so internal agents that
return `crate::error::Result` continue to bridge automatically at `?` boundaries.

## Versioning

Independent SemVer (`0.1.0` first release, per the plugin/sub-crate version
independence rule). Bumped only when this crate has a real delta — not lockstep
with the attune main tag.
