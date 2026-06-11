# attune 主仓 lawcontrol → attune-enterprise 全面替换

**日期**: 2026-05-30
**指令**: 用户「以后不再有 lawcontrol 这个词，全面替换」
**范围**: attune 主仓 (`/data/company/project/attune`)，OSS 主线。cloud/pluginhub/attune-pro 由另一 agent 负责，本次不碰。
**分支/提交**: worktree (基于 origin/develop @ 54102e9) → commit `d46a448` → push `origin/develop` (fast-forward `54102e9..d46a448`，无 force)。

## 替换映射（实际采用）

| 类别 | 规则 |
|------|------|
| 品牌显示名 | `LawControl` → `Attune Enterprise` |
| slug / 产品引用 | `lawcontrol` → `attune-enterprise` |
| 历史陈述「原名 LawControl / 原 LawControl」 | 删除旧名提及（改名久已完成，不再提旧名） |
| benchmark corpus 标签 | `lawcontrol corpus` → `legal corpus`；`Lawcontrol golden_qa` → `Legal golden_qa` |
| 跨仓功能引用 | `lawcontrol_compat` → `attune_enterprise_compat`（对齐 attune-pro 现状 `tests/attune_enterprise_compat/`，已确认该目录已改名） |
| 企业产品磁盘路径 | 统一 `/data/company/project/attune-enterprise/`（该目录已存在；旧 `lawcontrol` 目录不存在） |
| 临时 scratch 路径 | `tmp/lawcontrol-corpus` → `tmp/legal-corpus`；`lawcontrol_seed.sql` → `legal_seed.sql`；`lawcontrol-20-sample` → `legal-20-sample` |
| `import-from-lawcontrol` | `import-from-enterprise` |

## 逐文件处理（38 文件）

**品牌/定位文档**
- `CLAUDE.md` (3 处): 删「原名 LawControl」历史注记 ×2 + 三产品矩阵表去旧名。
- `README.md` / `README.zh.md` (各 5 处): 三产品矩阵 `lawcontrol`→`attune-enterprise`；corpus 标签 `lawcontrol corpus`→`legal corpus`。
- `docs/adr/0001-oss-pro-boundary.md` (2 处, 永久 ADR): 删「原名 LawControl」，矩阵表去旧名。
- `docs/oss-pro-strategy.md` (5 处): 删全部「（原 LawControl）/ 原名 LawControl 改名」历史括注。
- `docs/wiki/index.md` (3 处): 产品表 `LawControl`→`Attune Enterprise`；baseline 标签→`legal`。

**代码（仅注释，无功能标识符变更）**
- `rust/crates/attune-cli/src/main.rs`: doc-comment `///` pluginhub 部署名。
- `rust/crates/attune-core/src/plugin_loader.rs`: doc-comment `///` 参考来源。
- `rust/crates/attune-core/src/store/mod.rs`: CREATE TABLE DDL 内 `--` SQL 注释（SQLite 忽略，DDL 不变）。
- `rust/crates/attune-core/tests/rag_quality_benchmark.rs`: `//!` / `//` 注释（corpus 标签 + 占位路径）。
- `rust/crates/attune-core/assets/plugins/ai_annotation_risk/plugin.yaml`: `#` 注释（未来发包名 `@attune-enterprise/schemas`，非运行时 schema）。
- `tests/e2e/lawpro_chains_e2e.py`: 注释（去「lawcontrol 借款合同样本」品牌前缀）。

**benchmark / wiki / 报告**
- `rust/RELEASE.md` (8 处)、`docs/benchmarks/2026-Q2.md`、`docs/benchmarks/dual-track-baseline.md`、`docs/wiki/benchmarks.md`、`docs/wiki/faq.md`、`docs/walkthroughs/plugin-end-to-end.md`、`python/tests/reports/phase6-*.md`：corpus 标签/路径/产品引用统一替换。

**specs（历史设计稿）**
- `2026-05-22-unified-pluginhub-architecture.md` (39 处)：本是「改名 spec」。改名相关段落（§9.7 / §10.2 / 风险表 / 附录 grep 命令 / DNS·docker·DB 迁移机制）逐行改写为「已完成历史态 + 去旧名」（避免出现「attune-enterprise 改名为 attune-enterprise」自指乱句）；产品引用统一 attune-enterprise。
- 其余 spec（positioning / industry-design / agent-self-learning / test-orchestrator / memory-moat-v07 / knowledge-base-rag-audit / se-gap-audit / v1-0-ga-roadmap / v1-0-1-upgrade）：产品引用 sed 替换。

**2026-05-30 审计报告（决策更新）**
- `doc-debt-cleanup.md` / `global-leftover-scan.md` / `release-gap-assessment.md`：原记录「lawcontrol 历史陈述不动」的决策，本次更新为「已全量替换 attune-enterprise」，保持 report 作为活记录的准确性。
- `sprint-closure` / `ga-gap-audit-final` / `524-consolidation` / `product-materials`：产品引用替换 + 修正自指乱句。

## 历史陈述改写方式

旧名出现在「attune-enterprise 原名 LawControl，2026-05-22 改名」「（原 LawControl）」这类历史括注中。按用户「不再有这个词」要求，**删除旧名提及**而非保留：保留主语 attune-enterprise + 当前事实，去掉「原名/改名」从句。改名 spec 中纯迁移机制句（如「clone lawcontrol --mirror」「DNS lawcontrol.example.com 301」）改写为「`<旧仓>` / 旧域名」泛指或标注「已完成」。

## 验证

- **grep 残留**: `git grep -in lawcontrol`（全仓，排除 `.git` / `target` / `node_modules` / `docs/screenshots/*.png` / gitignored `.playwright-mcp` `.remember`）= **0 残留**。
- **plugin.yaml**: `yaml.safe_load` 解析 OK。
- **e2e py**: `python3 -m py_compile` OK。
- **代码影响**: 6 处 .rs 改动 diff 确认全为注释（`///` `//!` `//` `--`），无功能标识符（变量/struct/column）变更，不影响编译；故未跑全量 cargo build（注释改动无法影响编译，且 /data 仅 186G 空闲避免 target 占盘）。
- **gitignored 未碰**: `.playwright-mcp/*.yml`（旧浏览器快照）+ `.remember/`（本地 memory）含 lawcontrol 但均 gitignored，按 §截图/临时规范不入库，未改。

## Push 证据

- commit: `d46a448` (parent = `54102e9` = 原 origin/develop)
- push: `origin/develop` `54102e9..d46a448`，fast-forward，无 force，remote = `https://github.com/qiurui144/attune.git`
- push 后 fetch 确认 `origin/develop == HEAD == d46a448`
- 未碰 main / 任何 tag。

## 收尾

- worktree 已 remove；`git worktree prune`。
- `df -h /data` 复核见执行日志。
