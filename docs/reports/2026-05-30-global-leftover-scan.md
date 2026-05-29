# Global Leftover Scan — attune 生态四仓只读审计

> **Date**: 2026-05-30  **Mode**: 只读(grep/find/git log/ls,无 build / 无起服 / 无改文件)
> **Scope**: attune(OSS 主线)· attune-pro(私有插件)· cloud(SaaS)· cloud/wiki-web
> **方法**: per [[feedback-dont-trust-agent-claims]] 三形态(CI 孤儿 / 代码 drift / 文档债)+ 跨切面
> **诚信**: 未发现的类别明确标注,不硬凑。

---

## 执行摘要

| 类别 | 发现数 | 最高优先级 |
|------|-------|-----------|
| 1. CI 孤儿 | 0 真孤儿(已闭环) | — |
| 2. 代码 drift | 0(ACP 全链已 wired,非孤儿) | — |
| 3. 文档债 §3.2 | **7 项** | P1 |
| 4. disabled/残留 | 0 | — |
| 5. 版本漂移 | **3 项** | P1 |
| 6. 跨仓配对漂移 | 0(符合插件版本独立政策) | — |
| 7. TODO/FIXME/WIP | 0 | — |
| 8. secrets 硬编码 | 0 真泄露(全 test fixture) | — |

**总计真遗留: 10 项**(P0=0 / P1=5 / P2=5)。**无 GA 阻断 P0**。主轴是文档债(§3.2 白名单违规 + 一次性报告堆积 + stale 引用)。

---

## 1. CI 孤儿 — 未发现真孤儿

排查结论: 测试基本都在 CI 硬门内,且历史孤儿已被显式修复。

- attune `.github/workflows/ci.yml`: 跑 `parse_golden_set_regression` + `agent_gate_orchestrator` + `cargo test --workspace --release` + clippy(non-blocking)+ 慢 E2E nightly lane(`--include-ignored`)。
- attune-pro `ci.yml`: `cargo test --workspace --release` + `law-pro agent_golden_gate`(deterministic lane);`nightly-real-llm.yml` 跑 real-LLM golden gate。
- **正向证据**(非孤儿): develop commit `ad287ae` "ci(acp): wire OSS real-LLM gate into nightly CI — **orphan fix**" 显示团队已主动闭合一个 CI 孤儿。
- cloud: `ci.yml` / `pytest-suite.yml` / `cargo-audit-cloud-rust.yml` / `trivy-scan.yml` / `load-smoke.yml` / `docker-publish.yml` 齐全。

**建议**: 无。clippy 为 non-blocking(`-D warnings` 但 continue-on-error 语义需确认) — 若希望硬门可后续评估,非遗留。

---

## 2. 代码 drift — ACP 已全链 wired,非孤儿

重点核查 v1.1.0 ACP(Agent Control Plane)是否"实现了但没接入生产路径":

- **结论: 已 wired,不是孤儿**。`governor` 被 `agents/flow_runner.rs` / `agents/flow.rs` / `queue.rs` / `skill_evolution/mod.rs` / `llm.rs` 引用;chat route 已接 `run_chat_flow`(commit `2236c33` "wire run_chat_flow into chat route + AppState")。
- `resource_governor/` + `governor/` 两个目录并存(`governor.rs` 单文件 + `governor/` 目录 + `resource_governor/` 目录)。**潜在 drift 信号**: 命名近似的三处治理模块,需确认是否有一处是旧实现残留。

**建议**: P2 — 确认 `resource_governor/` vs `governor/` vs `governor.rs` 三者职责不重叠;若 `resource_governor/`(2026-04-27 spec)已被新 `governor/`(ACP)取代,应删旧。**仅登记,需 owner 读码确认,本扫描只读未深入比对函数级**。

---

## 3. 文档债(§3.2)— 7 项 P1/P2

### 3.1 [P1] `docs/release-notes-v1.0.0-drafts/` 整目录违反白名单
路径: `/data/company/project/attune/docs/release-notes-v1.0.0-drafts/`
7 个文件: `attune-v1.0.0.md` `attune-v1.0.1.md` `attune-pro-v1.0.0.md` `cloud-v2.2.0.md` `cloud-v2.2.1-or-v2.3.0.md` `desktop-v1.0.0.md` `desktop-v1.0.1.md`
**为何遗留**: §3.2 明令 `v<X.Y.Z>-release-notes.md` 禁止,release notes 进各仓 RELEASE.md 对应版本节。这是 draft 残留,且 v1.0.0~v1.0.7 均已 ship(对应节已在 RELEASE.md)。
**建议**: 删整个目录(内容已并入 RELEASE.md)。**优先级 P1**(对外 GitHub 访客可见的过时草稿)。

### 3.2 [P1] `docs/specs/` 与 `docs/superpowers/specs/` 双 spec 目录
`docs/specs/` 含 2 文件: `attune-plugin-protocol.md` `memory-moat-v07.md`;主 spec 目录是 `docs/superpowers/specs/`(40+ 文件)。
**为何遗留**: 两个 spec 落点违反单一主题 SSOT;`memory-moat-v07.md` 是 v0.7 期产物且引用已改名的 `lawcontrol`(见 §3.5)。
**建议**: P1 — `attune-plugin-protocol.md` 若仍是协议 SSOT 应迁 `docs/`(白名单 `<feature>.md`)或并入 `plugin-development.md`;`memory-moat-v07.md` 是过期设计稿,结论入 ADR 后删。

### 3.3 [P1] `2026-05-28-v1-0-x-sprint-closure-report.md` 误放在 specs 目录
路径: `docs/superpowers/specs/2026-05-28-v1-0-x-sprint-closure-report.md`(同名也在 `docs/reports/` 有一份)
**为何遗留**: `*-report.md` per §3.2 是一次性产物,不该进 specs;且与 reports/ 重复。
**建议**: P1 — 删 specs 下这份,保留/确认 reports 版本(或结论入 RELEASE.md 后两份都删)。

### 3.4 [P2] `docs/reports/` 一次性报告堆积(22 份)
含 4 份带时间戳的 `test-pyramid-20260512_*.md` / `test-pyramid-20260513_104633.md`(同主题 4 副本,2026-05-12/13)、`v1.0-*` / `v10-ga-*` 8+ 份 GA 验收报告、`2026-05-29-*` 5 份 ACP audit。
**为何遗留**: §3.2 sprint report / gap analysis 应进 PR description 或 RELEASE.md 节,不留独立 .md;`test-pyramid-*` 4 个时间戳副本是典型 "同主题多副本"。
**建议**: P2 — `test-pyramid-*` 只保留最新一份(或全删,结论已固化);`v1.0-*` / `v10-ga-*` GA 验收报告已发版,结论入 RELEASE 后归档/删;`docs/reports/` 保留为短期 sprint 落档区可接受,但需定期清(per §3.2 周期审计)。

### 3.5 [P2] stale `lawcontrol` 引用(产品已 2026-05-22 改名 attune-enterprise)
命中文件(非 corpora):`docs/specs/memory-moat-v07.md:158,162` · `docs/TESTING.md:210` · `docs/oss-pro-strategy.md` · `docs/adr/0001-oss-pro-boundary.md` · 多份 `docs/reports/v1.0-*.md` · 多份 `docs/superpowers/specs/*.md`
**为何遗留**: CLAUDE.md 明示 "attune-enterprise 原名 LawControl,自 2026-05-22 改名"。文档里 `lawcontrol` 是过时品名。
**建议**: P2 — 活文档(oss-pro-strategy / TESTING / ADR)应改 `attune-enterprise`;一次性 report/spec 里的留作历史痕迹可忽略(随归档一并消失)。注意 ADR `0001` 是永久文档,应更新。

### 3.6 [P2] `docs/superpowers/plans/` 完成的 plan 未删(3 份)
`2026-05-18-lawpro-agents-enhancement.md`(92KB)· `2026-05-19-civil-loan-evidence-chain-agent.md`(79KB)· `2026-05-28-k1-secrets-simplification.md`(attune-pro)
attune 主仓 plans 目录: `2026-05-28-cache-context-token-api.md`(109KB)· `2026-05-28-hybrid-token-routing.md`(94KB)等。
**为何遗留**: §3.2 生命周期表 — 实施 plan "实施完成后立即删"。lawpro-agents / civil-loan 对应的 agent 早已 ship(v0.8~v1.0)。
**建议**: P2 — 已 ship 的 plan 删(git log 留痕);ACP cache-context/hybrid-token plan 若 v1.1.0 仍在实施则保留。

### 3.7 [P2] Python 根 `RELEASE.md` stale 版本声明
`/data/company/project/attune/RELEASE.md:22`: "最新版本 **v0.7.0 GA**(2026-05-19),1260+ tests"
**为何遗留**: Rust 商用线实际已到 v1.0.7(2026-05-28),Python RELEASE 引用的"最新版本"指向 v0.7.0,与 rust/RELEASE.md(v1.0.7 / v1.1.0-dev)漂移。
**建议**: P2 — 更新该行指向 v1.0.7,或明确该数字仅描述"迁移到 Rust 时的快照"。注意根 RELEASE.md 是 Python 原型线,rust/RELEASE.md 是商用线 SSOT,两者职责需 README 顶部说清。

---

## 4. disabled / 残留文件 — 未发现

- 无 `.disabled` / `.bak` / `.orig` 文件(四仓 find 0 命中,排除 target/node_modules)。
- 无 `tmp/` 残留(三仓 tmp 目录为空或不存在)。
- cloud `admin/.venv/` 存在于 working tree 但 **git 未跟踪**(`git ls-files .venv/` = 0),非遗留(本地虚拟环境,应在 .gitignore)。**建议 P2**: 确认 `.venv` 在 .gitignore(grep 未在根 .gitignore 命中 venv 关键字,可能靠 admin 子模块自己的 ignore — 需确认)。

---

## 5. 版本漂移 — 3 项

### 5.1 [P1] attune RELEASE.md `v1.1.0` 节存在但无 tag、Cargo.toml 仍 1.0.7
`rust/RELEASE.md:3`: `## v1.1.0 (2026-XX-XX) — Agent Control Plane (ACP)`;`rust/Cargo.toml version = "1.0.7"`;最新 tag = `v1.1.0` 不存在,最新是 `v1.0.7`。develop 领先 main **33 commit**(全 ACP `feat(acp)`,未 merge 到 main)。
**为何**: 这是**进行中的 v1.1.0 开发**(develop 上 ACP-1~7),属设计如此(GitFlow:feature 在 develop)。**但** RELEASE.md 已写满 v1.1.0 完成态条目 + commit `7c4eb95` "v1.1.0 RELEASE 节补全(Gate1 前置)" — 在未 tag 前把未发布版本写成完成态,是 §1.1.4 "tag 后同步"的反向风险(读者以为已发)。
**建议**: P1 — 确认 RELEASE.md v1.1.0 节标注 "(开发中/未发布)" 或 `2026-XX-XX` 占位明显;merge main + tag 时才转完成态。**非 bug,是 doc-drift 监控点**。

### 5.2 [P2] cloud RELEASE.md 大量 "**待用户授权 tag**" 节(v3.0.0 全链 alpha→GA)
cloud RELEASE.md 有 v3.0.0-alpha.1 / alpha.2 / beta.1 / rc.1 / rc.2 / rc.3 / GA 共 7 节标 "待用户授权 tag",但 `git tag` 实际已有 `cloud-v3.0.0-alpha.1` ~ `cloud-v3.0.0-rc.3`(rc.3 已打,GA 未打)。
**为何**: RELEASE.md 文字 "待用户授权 tag" 与实际 tag 状态漂移 — alpha/beta/rc 其实已 tag,只有 GA 真未授权。
**建议**: P2 — 更新已打 tag 的节去掉 "待授权" 字样,只 GA 节保留。

### 5.3 [P2] 根 Python RELEASE.md 版本快照过时(同 §3.7)
见 §3.7。归此处版本视角重复登记。

---

## 6. 跨仓配对漂移(§7.1.5)— 符合政策,无漂移

- attune main 最新 tag `v1.0.7` ↔ attune-pro 最新 tag `v1.0.7` — **项目级强配对对齐**(同号)。
- attune-pro plugin Cargo.toml 版本独立(law-pro 1.0.5 等不随项目 tag bump)— per CLAUDE.md §1.1.8 "插件版本独立 vs 项目版本配对" 政策,**设计如此,非漂移**。RELEASE.md v1.0.7 节已明示该政策。
- cloud 独立版本线(cloud-v3.x),per §7.1.5 独立配对,非强配对。

**建议**: 无。这是正确状态。

---

## 7. TODO / FIXME-CRITICAL / WIP — 未发现

- attune rust crates 无 `FIXME-CRITICAL` / `TODO-CRITICAL` / `unimplemented!()` / `todo!()`(排除 tests)。
- develop commit message 无 "WIP" 前缀。

**建议**: 无。

---

## 8. secrets 硬编码(§1.4)— 无真泄露

grep `sk-* / AKIA* / ghp_* / BEGIN PRIVATE KEY` 命中全部为**合法 test fixture / vendored lib**:
- `attune-core/src/pii/patterns.rs:333,341` + `pii/mod.rs:690,709`: PII redaction 测试用的**假 key**(`sk-abcdef...` / `sk-1234567890ABCDEF...`)— 测试 PII 脱敏逻辑本身,设计如此。
- `cloud/admin/.venv/.../PIL/ImageFont.py` + `cryptography/.../ssh.py`: 第三方库 vendored 代码(`.venv` git 未跟踪)。

**建议**: 无真泄露。**唯一行动 P2**: 确认 cloud `.venv` 确在 .gitignore(防未来误 commit)。

---

## 附: 优先级汇总

| # | 优先级 | 类别 | 一句话 |
|---|--------|------|--------|
| 3.1 | **P1** | 文档 | 删 `docs/release-notes-v1.0.0-drafts/`(7 文件,已并入 RELEASE) |
| 3.2 | **P1** | 文档 | `docs/specs/` 双 spec 目录,2 文件迁移/归档 |
| 3.3 | **P1** | 文档 | sprint-closure-report 误放 specs,与 reports 重复 |
| 5.1 | **P1** | 版本 | RELEASE v1.1.0 写完成态但未 tag,标注 "未发布" |
| 3.5 | P2 | 文档 | 活文档(ADR/oss-pro/TESTING)stale `lawcontrol` → attune-enterprise |
| 3.4 | P2 | 文档 | `docs/reports/` test-pyramid 4 副本 + GA 报告归档 |
| 3.6 | P2 | 文档 | 已 ship plan 未删(lawpro/civil-loan) |
| 3.7/5.3 | P2 | 版本 | Python RELEASE 仍写 v0.7.0 GA |
| 5.2 | P2 | 版本 | cloud RELEASE 已 tag 节仍标 "待授权" |
| 2 | P2 | 代码 | governor/resource_governor/governor.rs 三处治理模块,确认无旧残留 |

**P0(GA 阻断): 0**。
