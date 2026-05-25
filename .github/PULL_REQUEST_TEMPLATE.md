# Pull Request

## 描述 / Description

<!-- 简述这个 PR 做了什么。链接相关 issue / spec / plan。 -->

Closes #<!-- issue 号 -->

相关 spec / plan(若有):
- `docs/superpowers/specs/<date>-<feature>.md`
- `docs/superpowers/plans/<date>-<feature>.md`

## 改动类型 / Type of change

- [ ] Bug fix(非 breaking)
- [ ] New feature(非 breaking,新增功能)
- [ ] Breaking change(会让现有 API / behavior 失效)
- [ ] 文档更新(README / DEVELOP / RELEASE / spec / plan)
- [ ] 重构(无 behavior 变化)
- [ ] 测试 backfill(只加测试,不改源码)
- [ ] CI / build 流程改动

## 测试 / Testing

<!-- 必填(per CLAUDE.md § 代码变更后的强制流程)。每条勾选都要有真实证据。 -->

- [ ] `cargo test --workspace --release` 全过
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` 干净
- [ ] 新增测试 case(若是新 feature)
- [ ] manual smoke test(若涉及 UI / CLI / 网络请求)
- [ ] 截图(若 UI 改动)→ 归 `docs/screenshots/<topic>/`
- [ ] Agent 改动:`agent_golden_gate.rs` 1.00 pass rate(deterministic)或 F1 ≥ 0.85(LLM)

测试输出(粘贴关键 log 或 commit SHA):
```
<paste 关键测试输出>
```

## Breaking change

- [ ] 是 — 详细描述影响 + migration 路径
- [ ] 否 — backward compatible

若是 breaking,**必须**同时更新:
- `RELEASE.md` 对应版本节的 **Breaking** 段
- `docs/UPGRADING.md` migration 章节(若涉及 user 数据 / schema)

## 自查 / Self-check

- [ ] 代码符合 `DEVELOP.md` 编码规范
- [ ] 文档已同步更新(若 API / behavior 变化)
- [ ] commit message 符合 [Conventional Commits](https://www.conventionalcommits.org/zh-hans/v1.0.0/) 规范
- [ ] 没有 hardcode secrets(per CLAUDE.md § Secrets 严禁硬编码)
- [ ] 没有跨 OSS / Pro / Enterprise 边界(per `docs/oss-pro-strategy.md`)

## Screenshots / 截图

<!-- UI 改动必填。拖拽图片或贴 imgur 链接。 -->

## 其他备注 / Notes for reviewer

<!-- 任何需要 reviewer 注意的事项。 -->
