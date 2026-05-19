# ADR 0003: GitFlow Lite + 双 tag (server / desktop 独立)

- **Status**: Accepted
- **Date**: 2026-04-30

## Context

attune 双产物:
- **Server tarball** (4 平台, 由 `rust-release.yml` 构建, `v*.*.*` tag 触发)
- **Desktop installer** (Win NSIS + MSI + Linux deb + AppImage + rpm,
  由 `desktop-release.yml` 构建, `desktop-v*` tag 触发)

挑战:
- 两条 release 节奏可能不同步 (e.g. server v0.6.1 出来不一定 desktop 同发)
- main 分支应保持 GA stable, 不能被 RC 污染
- alpha/beta/rc 走 develop 分支节奏快, 主线只 merge GA

## Decision

**GitFlow Lite (两长期分支)**:

- `main` — 稳定 GA 线, 用户克隆默认看到. **`vX.Y.Z` (无后缀) tag 只打这条**
- `develop` — 集成线. **`-alpha.N` / `-beta.N` / `-rc.N` pre-release tag 在这**

**Tag 双轨**:
- `vX.Y.Z` → `rust-release.yml` (server tarball)
- `desktop-vX.Y.Z` → `desktop-release.yml` (Tauri installer)

**版本号同步**: 两条同 release 共享 X.Y.Z 数字, 共用 RELEASE.md changelog.

**develop → main 时机**:
- 发布型 merge: 准备发新 release, 必须打 tag
- 治理对齐型 merge: 大量文档/小特性累积, README 漂移可触发, 不打 tag

**铁律**: `main` 的 first-parent 历史**永远不出现非 merge commit**. 进 main
的代码必须先经 develop → `--no-ff` merge.

## Consequences

**好处**:
- RC tag 不污染 main (主线干净)
- 用户克隆默认拿 GA stable
- 治理 commit 不被迫 tag (避免 v0.6.0a v0.6.0b 这种丑陋编号)
- Server / Desktop 可独立小版本号 (e.g. desktop-v0.6.0 一直用, server v0.6.1 升级)

**代价**:
- 双 tag 容易忘记 (本会话 desktop-v0.6.3-rc.1 → rc.5 走了 5 个 rc 才搞定双 release)
- 检查 main 异常状态须 `git log --first-parent` (非默认), 误用导致误报

## Implementation 落地

- v0.6.1 (commit eded077 / 07f57d0): 写入 CLAUDE.md "Git 分支管理标准 GitFlow Lite"
  作为行为标准, 含 first-parent 检查命令
- v0.6.3-rc.5: 完成 5 平台 desktop installer 自动 publish via softprops/action-gh-release
  + matrix bundles=nsis,msi + permissions: contents:write
