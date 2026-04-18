# Attune 会话交接备忘录

**截止时间**：2026-04-18
**工作目录**：`/data/company/project/npu-webhook`（也可用 symlink `/data/company/project/attune`）
**测试机**：Ryzen AI (192.168.100.201 · user `qiurui` · pw `123123`)—— 当前 **dev server 已停**

## 本次会话完成的工作

### 功能批次（6 batch，每批都经过两轮 code review + Playwright 回归）

1. **Batch 1** — Settings 重构 + 硬件感知默认摘要模型 + 扫描版 PDF OCR 兜底
2. **Batch 2** — 顶栏 + 模态 Settings + 模型 chip（ChatGPT 风格）
3. **Batch A.1** — 用户批注 CRUD（5 标签 × 4 色 + Reader 模态）
4. **Batch A.2** — AI 批注 4 角度分析（Ed25519-ready 插件结构）
5. **Batch B.1** — 上下文压缩流水线（摘要缓存 + 三阶段锁释放 + Token Chip）
6. **Batch B.2** — 批注加权 RAG（精确 label 白名单）+ Token Chip 展开

### 基础设施（P0 + P1）

- `LICENSE` — Apache-2.0
- `NOTICE` — 开源核心 vs 商业插件分界声明
- `CHANGELOG.md` — Keep a Changelog 格式
- `.github/workflows/rust-release.yml` — 5 平台自动打包（Linux x86_64/aarch64 · macOS Intel/Silicon · Windows x86_64）
- `.github/workflows/ci.yml` — 加入 `rust-test` job
- `rust/crates/attune-core/src/plugin_sig.rs` — Ed25519 签名校验骨架（9 tests）
- `rust/crates/attune-core/src/plugin_loader.rs` — 通用插件加载器（7 tests）
- `rust/crates/attune-core/assets/plugins/ai_annotation_*/` — 4 个内置 plugin.yaml + prompt.md
- `rust/crates/attune-core/src/ai_annotator.rs` — 重构为 plugin-driven（不再 hardcode prompt）

### 审计修复（20 轮全项目审计）

- R11 安全：`/api/v1/settings` redact `api_key`，deep-merge 保留原值
- R15 安全：`llm.endpoint` URL 白名单（http/https），`browser_path` 拒绝 `-` 前缀
- R4 迁移：`profile/export` v1→v2，加 annotations
- R19 CI：rust-test job 进 CI
- CRITICAL C1：skill evolver 三阶段锁释放（修 vault 锁 15s+ 阻塞问题）
- `/ingest` 补 `chunks_queued` 字段（与 `/upload` 对齐）
- Enter 键重入守卫（防 chat 双发）
- 文档一致性修正（标签页 9→8 + Chrome 扩展端点数）

### 产品协同规划

- `docs/product-collaboration-plan.md` — 285 行完整规划
  - Attune × lawcontrol 能力矩阵（谁做什么 / 不做什么）
  - 共用云管平台（PluginHub / 官网 / SSO / 监控）
  - 数据流硬边界（Attune Chat 永不入律所库）
  - 商业协同（律所买 lawcontrol 赠 Attune Pro）
  - 技术互学清单（双向 3+3 项）

## 测试状态

- **Workspace tests**：376 passed · 0 failed · 5 ignored（最终状态）
- 从入会话的 213 → 376（+163）
- **Playwright 回归**：10 Phase × 57 显式断言 100% 过（docs/regression-report-2026-04-18.md）

## 关键决策记录（product-level）

1. **开源许可**：Apache-2.0（不是 MIT —— 需专利授权防反诉）
2. **商业模式**：Lite 免费 / Pro ¥29-49/月 / Pro+ / Team / Enterprise 四档；硬件捆绑 3 年会员
3. **第一批行业插件**：律师 + 售前
4. **律师插件定位**：面向**个人律师 / in-house / solo**，多人协作引导到 lawcontrol
5. **插件分发**：复用 lawcontrol 的 PluginHub（不建平行系统）
6. **数据边界**：Attune 个人批注/Chat 永不同步到 lawcontrol，防私人观点成为证据
7. **成本/触发契约**：LLM 永不后台偷跑；UI 透明显示每次调用成本

## 未完成（可从下一会话继续）

### 产品层需决策（我已列问题，用户未拍板）

- 品牌中文名？（给律师客户看）
- 云管平台部署地（阿里云 / AWS / Cloudflare）
- 公司注册：个体工商户起步 vs 直接有限公司
- 律师执业证验证：Pro 验 vs lawcontrol 统一验

### 技术层可接续

- 激活码离线校验骨架（HMAC-SHA256(plan, expiry, device_fp)）—— 跟 plugin_sig 同一风格
- CLI `attune plugin install/verify/list` 命令
- lawcontrol pluginhub 扩 `product_line` 字段（需跨仓库协作）
- Attune 律师插件 MVP（schema 对齐 lawcontrol `contract_review.output.schema`）
- Keycloak SSO 部署
- `docs/LAWCONTROL_INTEGRATION.md`（本次已在 `product-collaboration-plan.md` 里覆盖）

### Defer 项（audit 20 轮中标记为 IMPORTANT 但本次未处理）

- **R5 备份命令**：无 `attune backup` 子命令（用户手工复制 WAL .db 有风险）
- **R7 PRAGMA user_version 迁移骨架**：未来改 schema 会炸老用户
- **R10 OCR 超时** + 大 PDF 磁盘限制
- **R12 批注编辑**（目前只能删除重建）· AI 批注 accept/ignore 按钮（memory spec 要求）
- **R14 会话导出**（markdown / txt）
- **R16 跨设备同步协议** —— 等产品层决策云管平台
- **R18 前端压缩**（97KB HTML 未 minify）
- **R20 crash 上报**（Sentry 自托管 / GlitchTip）

## Git 状态

- **25 个修改** + **39 个新文件**（本次会话产出），**均未 commit**
- 新文件归类：
  - 源码：plugin_sig.rs / plugin_loader.rs / ai_annotator.rs（重构）/ annotations.rs / annotation_weight.rs / context_compress.rs
  - 内置插件 YAML/MD：4 个 annotation angle
  - 文档：LICENSE / NOTICE / CHANGELOG / docs/* 几份
  - 测试截图已归档到 `docs/screenshots/`（22 张）
- `.gitignore` 加了 `/*.png` 和 `.playwright-mcp/`

### 何时 commit

用户多次强调"不要擅自 commit"。**下次会话询问用户后再按 batch 分多条 commit**，建议分法：
1. `docs: LICENSE + NOTICE + CHANGELOG` (1 commit)
2. `feat(core): Batch 1 — hardware-aware defaults + OCR fallback` (1)
3. `feat(ui): Batch 2 — topbar + modal Settings + model chip` (1)
4. `feat(core,ui): Batch A.1/A.2 — annotations + AI analysis` (1-2)
5. `feat(core,ui): Batch B.1/B.2 — context compression + annotation weighting` (1-2)
6. `fix(core,server): 20-round audit fixes` (1)
7. `feat(core): plugin_sig + plugin_loader + ai_annotator plugin migration` (1)
8. `ci: Rust test + release pipeline` (1)
9. `docs: product collaboration + regression + audit reports` (1)

## 下次会话开场提示

在 `/data/company/project/attune/` (或 `npu-webhook/`) 目录开 Claude，建议开场白：

> 继续 Attune 开发。上次会话（2026-04-18）完成了 6 个功能 batch + 20 轮审计 + P0/P1
> 基础设施（LICENSE + Rust release CI + plugin_sig + plugin_loader）。详见
> `docs/session-handoff-2026-04-18.md`。当前所有变更**未 commit**，先和我讨论提交
> 策略（按 batch 分 9 条 or 合并）再动手。

Claude Code 会自动加载：
- `CLAUDE.md`（含成本契约 + 产品协同规则）
- 4 个 memory 文件（`~/.claude/projects/-data-company-project-npu-webhook/memory/*.md`）

## 环境状态

- **Ryzen AI (192.168.100.201)**：dev server 已停；vault 在 `~/.local/share/attune/` 含一个未 setup 的空 vault（密码 `regress-pw-2026` 在 regress 期间设的，重开后不再使用）
- **Ollama**：保持运行，qwen2.5:3b + bge-m3 仍可用
- **本地 `rust/target/`**：release 二进制 59MB，编译缓存 3GB+（可 `cargo clean` 回收）

## 产出文档清单

| 文件 | 用途 |
|------|------|
| `LICENSE` | Apache-2.0 |
| `NOTICE` | 双许可声明 |
| `CHANGELOG.md` | 版本记录（Keep a Changelog）|
| `docs/product-collaboration-plan.md` | Attune × lawcontrol 285 行战略规划 |
| `docs/regression-report-2026-04-18.md` | 10 Phase Playwright 回归 |
| `docs/audit-20-rounds-2026-04-18.md` | 20 轮全项目审计 213 行 |
| `docs/session-handoff-2026-04-18.md` | 本文件 |
| `docs/screenshots/` | 22 张 dev 截图归档 |
| `CLAUDE.md` + 4 个 memory 文件 | 会话级持久规则 |
