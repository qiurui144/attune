# Attune 20 轮深度清理日志（2026-04-25）

**触发**：Sprint 0 + 0.5 完成（14 commits / 377 tests pass）。用户指令"完整代码清理 + 重组 + 缺口检查 + 冗余查询 + git 清理，循环 20 轮，超过 2 小时确保覆盖"。

**Worktree**：`/data/company/project/attune/.worktrees/sprint-0-tauri/`
**Branch**：`feature/sprint-0-tauri-shell`
**Baseline**：377 tests passed, 0 failed, 5 ignored

**约束**：
- 测试不退化（每轮跑 cargo test --workspace 验证）
- 不留兼容包袱（开发期）
- 不 push（用户全局规则）
- 文档保持简洁（不新增 .md 文件，扩 README 等已有）

## 20 轮 plan

### 第一组：代码清理 (R1-4)
- **R1** dead_code warning 全扫 + 修
- **R2** 未使用 dependencies (Cargo.toml 里没 import 的 crate)
- **R3** 重复 import / unused import / clippy auto-fix
- **R4** 注释清洗（过期 TODO / TBD / // FIXME）

### 第二组：重组 (R5-8)
- **R5** 文件粒度审查（哪些 .rs 太大该拆 / 太小该并）
- **R6** 模块 visibility (pub vs pub(crate) vs private) audit
- **R7** Rust workspace 一致性（features / lints / version pinning）
- **R8** docs/ 目录整理（删废弃 spec / merge 重复）

### 第三组：缺口检查 (R9-12)
- **R9** API endpoint 完整性（/api/v1/* 是否全有错误处理）
- **R10** 测试覆盖 gap（哪些 module 测试薄弱）
- **R11** 错误处理 gap（unwrap / expect 在 prod path）
- **R12** 跨平台 gap（Win 上还有什么没验）

### 第四组：冗余查询 (R13-16)
- **R13** 重复函数 / 重复逻辑（lib 内是否有等价实现）
- **R14** Python 线 vs Rust 线对比 — 已迁移功能是否双线维护
- **R15** extension/ 内冗余（旧 detector 适配器）
- **R16** 模型 / 数据库 schema 冗余字段

### 第五组：Git 清理 (R17-19)
- **R17** 主仓库 develop 分支未提交改动审查 + 处理
- **R18** 旧 worktree 清理（.worktrees/phase3-long-text）
- **R19** stale local branches（已 merge / 久未动）+ 清理

### 收尾 (R20)
- **R20** 全测试 + AppImage smoke + 写本日志末尾"总结"

## 进度记录

每轮一段，含 status / commit SHA / key findings / fixes / 测试数。

---

