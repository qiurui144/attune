# Doc-Debt Cleanup + ACP Closeout + attune-pro 分支债评估 (2026-05-30)

> 依据: `docs/reports/2026-05-30-global-leftover-scan.md` (P0=0, P1=5, P2=5)。
> Scope: attune + attune-pro 两仓 (不碰 cloud — 另有 agent 工作中)。
> 用户指令: "除 payment 其他全量检查"。分支策略 (attune-pro main/develop) 仅评估不执行。

---

## 1. 文档债 P1 清理 (attune, develop)

### 1.1 删 `docs/release-notes-v1.0.0-drafts/` 整目录 (7 文件, §3.2 白名单违规)
- `git rm -r` 删除: attune-v1.0.0 / v1.0.1 / attune-pro-v1.0.0 / cloud-v2.2.0 / cloud-v2.2.1-or-v2.3.0 / desktop-v1.0.0 / v1.0.1 (-826 行)。
- **唯一引用**: `rust/RELEASE.md:318` 指向 `attune-v1.0.1.md` → 已删该悬空指针行 (v1.0.1 节本身已在 RELEASE.md 上方, 信息未丢)。
- grep 验证: 仅 scan 报告自身引用该目录, 删后无悬空链接。

### 1.2 双 spec 目录合并 (`docs/specs/` → 消除)
- `docs/specs/attune-plugin-protocol.md` → **`git mv docs/plugin-protocol.md`** (白名单 `<feature>.md`, 协议 API 契约 SSOT, rename 保留 history)。
- `docs/specs/memory-moat-v07.md` → **`git mv docs/superpowers/specs/2026-05-19-memory-moat-v07.md`** (并入唯一 spec 目录)。
- `docs/specs/` 目录已 `rmdir` 移除 (空)。
- **引用全部更新** (grep 验证 0 残留 stale 路径):
  - `attune-accounts/src/lib.rs:6` (doc comment, §10 endpoint 索引) → `docs/plugin-protocol.md`
  - `README.md:103,601` / `python/tests/MANUAL_TEST_CHECKLIST.md:161,239` / `rust/RELEASE.md:722,733` / `docs/reports/v1.0-product-materials.md:781` → memory-moat 新路径
  - `chat.rs:1057` 用短名 "attune-plugin-protocol §3"(概念引用非路径)— 保留不动

### 1.3 sprint-closure-report 误放 specs
- `docs/superpowers/specs/2026-05-28-v1-0-x-sprint-closure-report.md` → **`git mv docs/reports/`** (per §3.2 `*-report.md` 属 reports 不属 specs)。
- **关键修正**: scan 称 "reports/ 已有同名副本" — 实测 **不存在**。该 report 含唯一 tag→SHA 映射 (v1.0.5=b091f4c / attune-pro v1.0.5=1f30f9d / cloud-v2.3.0=ad24b1d) 未入 RELEASE.md。**未删, 改为移动**, 避免丢唯一信息 (per §6.3 删旧 doublecheck)。

## 2. ACP spec closeout (§3.1 流程对齐)
- `docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md` 顶部 Status 改:
  `DESIGN PROPOSAL` → **`已实施并 GA (v1.1.0, 2026-05-30; acp.1-7 + chat wiring 已 merge main e6b9b47)。本 spec 为 ACP 设计 SSOT 保留`** (历史 DESIGN PROPOSAL 字样保留作痕迹)。
- spec 未删 (它是 ACP 设计权威, per 任务要求)。
- **未新建 ADR**: 任务列为可选; ACP spec 自身已是设计 SSOT, 现有 ADR 0001-0005 体系无 ACP 决策缺口需补 (避免新增冗余文档, per §3.2)。

## 3. 文档债 P2 清理 (attune, develop)

### 3.1 旧品名 → `attune-enterprise`
- 描述性赛道标签 (如 `docs/TESTING.md` "法律 …") 统一为 "法律 legal-track" / "legal corpus" (非磁盘路径, 不破坏 corpus 加载)。
- ADR-0001 / oss-pro-strategy.md / industry-vertical-design 等里的旧品名历史陈述 → 已全量替换为 `attune-enterprise` (2026-05-30 全面替换 sprint, 不再保留旧名提及)。
- 代码内旧品名 (plugin_loader 注释 / corpus 路径 / pluginhub 部署名 / e2e 样本名) → 已统一为 `attune-enterprise` / `legal` 标识。

### 3.2 `docs/reports/` test-pyramid 4 副本
- 4 份同 commit (66d6422, 2026-05-12/13) 自动快照 → 删 3 旧 (`20260512_174706/175912/181208`), 保留最新 `20260513_104633` 作代表。grep 验证无引用。

### 3.3 已 ship plan 删除 (§3.2 完成即删)
- attune 删 3 份 (已 ship): `cache-context-token-api` + `hybrid-token-routing` (ACP v1.1.0 GA) + `privacy-logic-implementation` (v1.0.7 hotfix 017ab81)。
- **保留 2 份** (未 ship, future window): `kb-bench-integration` (target v1.0.6 2026-06-05) + `web-plugin-ingest-only` (target v1.1.0 reframe 2026-08-15, ingest endpoint 尚未进 routes)。
- web-plugin plan:1102 对已删 privacy-logic plan 的 cross-ref → 改指向 spec `2026-05-28-privacy-logic-strategy.md` + 注明已 ship (避免悬空文件引用)。

### 3.4 Python 根 RELEASE.md stale 版本
- `RELEASE.md:22` "最新版本 v0.7.0 GA" → "**v1.1.0 GA (2026-05-30, Agent Control Plane)**" + 注明 rust/RELEASE.md 为版本 SSOT、根 RELEASE 记 Python 原型线历史。

## 4. attune-pro 文档债 (develop)
- **无** drafts / `docs/specs/` 双目录 / reports 堆积 (scan 仅命中 plan)。
- 删 2 份已 ship plan: `2026-05-18-lawpro-agents-enhancement` (civil_loan/evidence_chain agent 已在 `plugins/law-pro/src/`) + `2026-05-19-civil-loan-evidence-chain-agent`。
- **保留** `2026-05-28-k1-secrets-simplification.md`: plan 头明示 "本 sprint 不实施…仅落档备查…待 v1.0.6 评审" = 未实施 backlog, 不删。
- `RELEASE.md:288` 对已删 lawpro plan 的链接 → 软化为 "git log 见" (避免悬空)。
- 旧品名 refs 已全量替换为 `attune-enterprise` (2026-05-30 全面替换 sprint)。

---

## 5. attune-pro 分支债评估 (只评估, 不执行 merge — 留用户决策)

### 现状
- `main` @ `8e0e27b` (test(law-pro) civil_loan golden set), `develop` @ `680142a` (release v1.1.0 配对)。
- `main..develop` = **149 commits** 未 merge。
- tag 落点: v1.1.0 / v1.0.7 **在 develop**; v1.0.5 **在 main**; (v1.0.1 未在两者直接命中)。
- 与 CLAUDE.md GitFlow "正式版 tag 只在 main" 声称 **不符** (v1.0.7/v1.1.0 GA tag 实际打在 develop)。

### 两条路径 + 风险

**选项 A — 补一次 develop→main 对齐 (推荐, 符合既定 GitFlow)**
- 操作: `git checkout main && git merge --no-ff develop` 把 149 commits 并入, main 重新成为稳定发布线。历史 tag 不动 (tag = 不可变锚点)。
- 收益: main 恢复 "对外默认分支 = 最新稳定", attune (main=e6b9b47 已含 v1.1.0) 与 attune-pro 强配对对齐。
- 风险: (1) 149 commits 一次性 merge, 若 main 有 develop 没有的独立 commit 会冲突 — 需先 `git log develop..main` 确认 main 无独立提交 (大概率 fast-forward-able, 但 `--no-ff` 保边界)。(2) 历史 GA tag 仍在 develop, "tag 在 main" 仅对**未来** tag 成立, 过去无法追溯修正。

**选项 B — 修订 CLAUDE.md 分支政策, 承认 develop-tag 模式**
- 操作: 改 attune-pro CLAUDE.md「Tag 打在哪条分支」节, 明示 "attune-pro 历史采用 develop-centric tag (v1.0.7/v1.1.0 在 develop), main 滞后为 release-checkpoint"。
- 收益: 零 git 风险, 文档与现实一致。
- 风险: 与 attune 主仓 GitFlow 双仓共用声称分叉 (CLAUDE.md 顶部明示 "attune + attune-pro 双仓共用"), 长期双标准易混淆; 对外 main 永久滞后 149 commits, GitHub 访客 clone main 拿到的是旧码。

### 建议
**倾向选项 A** (补 develop→main 对齐) — 理由: ① attune 主仓 main 已含 v1.1.0, 强配对要求 attune-pro main 同步; ② "对外默认分支 = 旧码" 对私有商业仓也是隐患; ③ 双仓共用 GitFlow 是已固化标准, 分叉成本更高。**但这是分支策略决定, 留控制器/用户拍板, 本次不执行 merge。** 若选 A, 执行前必 `git log develop..main --oneline` 确认 main 无 develop 缺失的独立 commit。

---

## 6. Commit + Push 证据

| 仓 | 分支 | commit SHA | push 状态 |
|----|------|-----------|-----------|
| attune | develop | `94a09ce` | ✅ pushed (`ddba9be..94a09ce`, https://github.com/qiurui144/attune.git) |
| attune-pro | develop | `2cc580c` | ⚠️ 本地已 commit, push 被 TLS 瞬断阻塞 (`gnutls_handshake() failed: TLS connection was non-properly terminated`), 已重试 5+ 次 + 30s 退避仍失败 — 待网络恢复后 `git push origin develop` 重推 (commit 已安全落本地) |

> 注: attune push 成功后同窗口 attune-pro push 持续 TLS handshake 失败 (含 fetch), 属环境瞬断非 commit 问题。attune-pro commit 2cc580c 已落 develop 本地, 内容无丢失风险。
