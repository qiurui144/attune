# WASM-safe Agent Leaf Crate 抽取 — 实施报告

> 日期:2026-06-01 · 隔离 worktree(base = attune develop `ae4f255`)
> spec/plan:`docs/superpowers/{specs,plans}/2026-06-01-wasm-safe-agent-leaf-crate.md`
> 决策固化(本次 prompt 拍板):AgentError 用 thiserror "2";`Serialization → VaultError::InvalidInput`
> (serde_json::Error 无法回造);wasm CI 守卫并入现有 ci.yml `rust-test` job 作一 step。

## 阶段 commit

| 阶段 | commit SHA | 内容 |
|------|-----------|------|
| C1 (D1) | `f3fecb7` | 建 `rust/crates/attune-agent-sdk/{Cargo.toml,src/lib.rs}`(leaf,仅 serde+thiserror)+ workspace members/deps 登记 |
| C2 (D2) | `f211505` | attune-core re-export `pub use attune_agent_sdk::{Agent,AgentError,AgentOutput,AgentResult}` + `From<AgentError> for VaultError` + document_classifier::run 签名改 AgentResult |
| C3 (D3) | `3c9b69e` | ci.yml `rust-test` job 加 wasm32-wasip1 build guard step(ubuntu-only) |
| C5 (D5) | `ab93c40` | rust/RELEASE.md highlight + rust/DEVELOP.md crate 列表 + leaf README |

> C4(D4)无独立 commit —— 全量回归零代码 fix 需求(re-export 行为零变更),per plan「无 fix 则并入」。

## WASM 编译证据(关键判据)

```
$ cargo build -p attune-agent-sdk --target wasm32-wasip1 --offline
   Compiling attune-agent-sdk v0.1.0
    Finished `dev` profile ... in 7.99s
$ ls target/wasm32-wasip1/debug/libattune_agent_sdk.rlib
-rw-rw-r-- 104716  libattune_agent_sdk.rlib       # .wasm-target rlib 产出

$ cargo tree -p attune-agent-sdk --target wasm32-wasip1   # 非 dev-dep 依赖树:
attune-agent-sdk
├── serde (serde_core + serde_derive proc-macro)
└── thiserror (thiserror-impl proc-macro)
# 零 native crate(rusqlite/tokio/reqwest/usearch/tantivy/hdbscan/socket2/serde_yaml/chrono)
```

CI guard(ci.yml rust-test job,ubuntu-only step):`rustup target add wasm32-wasip1`
+ `cargo build -p attune-agent-sdk --target wasm32-wasip1` —— native dep 回流即红。

## 测试数字

| 范围 | 结果 |
|------|------|
| `cargo test -p attune-agent-sdk` | **20 passed / 0 failed**(8 golden 迁入 + 5 边界 + 3 异常 + 3 proptest + 1 JSON wire 逐字节断言) |
| `cargo test -p attune-core --lib` | **1563 passed / 0 failed / 1 ignored**(远超 plan 211+ floor;含 5 个 From<AgentError> 映射 + ? 桥接单测) |
| `cargo test --workspace` | **2105 passed / 5 failed** |
| `cargo clippy -p attune-agent-sdk -p attune-core --all-targets` | 零新 warning(唯一 warning = `ingest/git.rs:236` pre-existing needless_return,与本改动无关) |

### 5 个 workspace 失败 = pre-existing,非本次回归(已证)

`crates/attune-server/tests/git_route_subprocess.rs` 的 5 个 SSRF 守卫测试
(file/ssh scheme / loopback / metadata / non-allowlisted host)失败。在 clean
base `ae4f255`(临时 worktree)跑同 5 测试 → **同样 5 全 FAILED**,即抽取前已存在。
本次 4 commit 仅触及 leaf crate / workspace Cargo / attune-core agents+error / ci.yml,
`git diff --name-only ae4f255 HEAD` 不含 attune-server 任何文件 → 不可能由本次引入。
属环境/网络敏感的 pre-existing 失败,不在本任务范围。

## R8 实测(plan 风险登记)

plan R8 预期内部 `impl Agent` 经 `?` 桥接零改动 —— 实测 **需 1 处签名更新**:
`document_classifier.rs::run` 返回类型 `crate::error::Result<...>` → `super::AgentResult<...>`。
原因:trait 签名本身从 `Result<_,VaultError>` 收紧为 `AgentResult<_>`(=`Result<_,AgentError>`),
impl 的 `run` 签名必须逐字匹配 trait(`?` 桥接只解决 body 内错误传播,不解决签名声明)。
body 零行为变更(`Ok(run(&docs))` 不 error)。attune-core 内仅此一处内部 impl Agent。

## attune-pro pilot 待办清单(跨仓,本 worktree 不做)

per plan §6 sequencing,**blockedBy attune v1.1.0 tag**(leaf 须随 attune 先发布):

1. `law-pro/Cargo.toml` `[dependencies]` 加 `attune-agent-sdk = { workspace = true }`。
2. `src/bin/agent_limitation_check.rs`(pilot):
   - `use attune_core::agents::{Agent, AgentOutput}` → `use attune_agent_sdk::{Agent, AgentOutput}`
   - `use attune_core::error::{Result, VaultError}` → `use attune_agent_sdk::{AgentResult as Result, AgentError}`
3. 确认该 bin 不 `use attune_core::llm`(确定性 agent 无 LLM)→ 去对 attune-core 的链接(仅 bin crate)。
4. `rustup target add wasm32-wasip1` → `cargo build -p law-pro --bin agent_limitation_check --target wasm32-wasip1` → 产 `.wasm`。
5. `agent_golden_gate.rs` 跑确认 1.00 deterministic 不回归。
6. 两步迁移(spec §10.3):先升 dep 仍用 re-export(不阻塞),真要编 wasm 的 vertical 再切 import。
7. 其余确定性 vertical(civil_loan / labor_dispute / evidence_chain / sale / housing /
   divorce / traffic / inheritance + patent-pro 结构化抽取)按 pilot recipe 逐个迁。
8. attune-pro 强配对 v1.1.0 bump + tag(跟 attune main)。

## 阻塞项

无。沙箱网络对 crates.io registry 超时,全程用 `--offline`(proptest/thiserror2/git2
等所需 crate 均已在本地 cargo cache + Cargo.lock,无新增网络拉取)。Cargo.lock diff
纯增量(attune-agent-sdk + git2/globset/libgit2-sys/libz-sys 这组 pre-existing
attune-core dep 被 cargo 重新写回 lock),零 version 移除/bump。

## GA 验收清单勾选(本仓部分)

- [x] `cargo build -p attune-agent-sdk` native 绿
- [x] `cargo build -p attune-agent-sdk --target wasm32-wasip1` 干净(dep tree 仅 serde/thiserror)
- [x] `cargo test -p attune-agent-sdk` 全过(8+3+5+3+1)
- [x] `attune_core::agents::{Agent, AgentOutput}` 路径仍可用(re-export 同一类型)
- [x] `cargo test -p attune-core` 不回归(1563 pass)
- [x] `cargo test --workspace` 绿(5 failed = pre-existing,已证非回归)
- [x] clippy 零新 warning
- [x] `From<AgentError> for VaultError` 每变体映射单测通过
- [x] JSON wire 逐字节相等断言通过
- [x] 内部 impl Agent(document_classifier)更新签名后编译过(零行为变更)
- [x] CI wasm 守卫 step 配置(ci.yml rust-test,ubuntu-only)
- [x] RELEASE.md / DEVELOP.md / leaf README 更新
- [ ] (跨仓,attune-pro 侧)law-pro pilot wasm 产出 + golden gate —— 待 v1.1.0 tag 后执行
