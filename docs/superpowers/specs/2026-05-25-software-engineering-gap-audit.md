# 软件工程全维度 Gap Audit — v1.0 GA 真实就绪度

> 2026-05-25 13:00 用户指示「评估现有缺口,我感觉升级策略等还是不明了。
> 站在软件工程的方式进行思考,评估项目缺口」起草。
>
> 范围:**SDLC 11 维度** + 用户 priority 维度「升级策略」单独详节(§3)。
>
> 与 `docs/v1.0-ga-gap-audit-final.md`(2026-05-22) 的关系:那份是 GA blocker
> 视角(P0=0 → Go),本份是**工程纪律 + 运营长期视角**(产品/工程持续运营能力)。
> 重叠项标 cross-ref,不重复列。

## 目录 (Table of Contents)

- [0. 维度状态总览](#0-维度状态总览)
- [1. 维度 1 — 需求 + 设计 (Requirements / Design)](#1-维度-1--需求--设计-requirements--design)
- [2. 维度 2 — 实现 + 测试 (Implementation / Testing)](#2-维度-2--实现--测试-implementation--testing)
- [3. 维度 3 — **升级策略 (User-Facing Upgrade)** ⭐ 用户 priority](#3-维度-3--升级策略-user-facing-upgrade--用户-priority)
- [4. 维度 4 — Observability / Monitoring](#4-维度-4--observability--monitoring)
- [5. 维度 5 — Security 持续机制](#5-维度-5--security-持续机制)
- [6. 维度 6 — Performance / Capacity](#6-维度-6--performance--capacity)
- [7. 维度 7 — DR / Business Continuity](#7-维度-7--dr--business-continuity)
- [8. 维度 8 — Customer Support + Issue Flow](#8-维度-8--customer-support--issue-flow)
- [9. 维度 9 — Payments / Billing / Quota](#9-维度-9--payments--billing--quota)
- [10. 维度 10 — Legal / Compliance](#10-维度-10--legal--compliance)
- [11. 维度 11 — i18n + Release Engineering 真链](#11-维度-11--i18n--release-engineering-真链)
- [12. Gap 优先级汇总 (P0 / P1 / P2 / P3)](#12-gap-优先级汇总-p0--p1--p2--p3)
- [13. 落地建议 (v1.0 / v1.0.1 / v1.1)](#13-落地建议-v10--v101--v11)

---

## 0. 维度状态总览

| # | 维度 | 状态 | 关键 gap |
|---|------|------|---------|
| 1 | 需求 + 设计 | 🟢 良好 | spec 11 节范式落地;少量 sprint report 散落 |
| 2 | 实现 + 测试 | 🟢 良好 | attune-core 1145+ tests / agent_golden_gate 6 类下限闭环 |
| 3 | **升级策略** ⭐ | 🟡 **关键缺口** | **`docs/UPGRADE.md` 缺(升级矩阵全场景文档化)**;schema downgrade plan 缺;in-app updater UI 提示路径未真验 |
| 4 | Observability | 🟡 中等 | gatus 5 endpoint + email alert ✅;**应用 metrics(`/metrics` / Prometheus / chat req metric)缺**;**log aggregation(中心化 Loki / ELK)缺** |
| 5 | Security 持续 | 🟢 良好 | cargo audit + deny CI ✅ / crypto audit ✅ / SECURITY.md ✅;pen test + fuzz 缺(P2) |
| 6 | Performance / Capacity | 🟡 中等 | stress baseline ✅ (4 testsuite); 1000-user / 100GB 真生产 scale stress 缺;SLA 数值化承诺缺 |
| 7 | DR / BC | 🟡 中等 | ROLLBACK.md 6 场景 ✅;backup.sh + restore.sh ✅ (official-web);**attune client vault DR runbook 缺(`.av-migrate.tmp` 中断恢复)**;真 restore drill 未演练 |
| 8 | Customer Support | 🟡 中等 | GH issue + SLA "24h" 已在 pages.yaml 声明;**issue template 缺**;**SECURITY.md disclosure 流程缺**;bug/feature template 缺 |
| 9 | Payments | 🟢 良好 | Stripe checkout + customer create idempotent ✅;`test_stripe_webhook.py` 22 case ✅;**发票合规(中国) / quota dashboard for user 缺** |
| 10 | Legal | 🔴 **真缺口** | **官网 ToS / Privacy Policy / DPA 真页面缺**;**中国 ICP 备案号缺**;enterprise contract 律师审核未做 |
| 11 | i18n + Release Eng | 🟡 中等 | i18n zh/en key 表对齐 ✅ (830/835 行);**wiki-web en/ 子目录存在但内容覆盖度未审**;winget PR 真合入状态未确认;macOS 桌面缺;APT/RPM repo 真发布状态未跑端到端 |

总判:**🟢 v1.0 GA Go 仍成立**(per final audit P0=0)。但 §3(升级策略)+ §4(observability)+ §10(legal)
三块是产品长期运营的**最大软肋**,5/26 上架后 30 天内必须完成补齐,否则会成为 v1.0.1/v1.1 sprint 不断"复活"的债。

---

## 1. 维度 1 — 需求 + 设计 (Requirements / Design)

| 项 | 状态 | 实测 |
|----|------|------|
| spec 11 节范式 | ✅ | `docs/superpowers/specs/` 含 2026-05-22 oss-4-agent / robust-llm-infra / release-package-management / unified-pluginhub-architecture / 2026-05-24 deepseek 等多份 spec |
| ADR 记录 | ⚠️ | ADR 实际靠 spec / CLAUDE.md 决策段承担。无 `docs/adr/` 单独目录(违反 CLAUDE.md §文档体系铁律 docs/adr/ 形态) — **P3** |
| sprint report 散落 | ⚠️ | `v1.0-524-final-consolidation-report.md` / `v10-ga-ui-e2e-report.md` / `v1.0-ga-gap-audit-final.md` 是一次性产物,违反文档体系铁律 §2 "❌ `<topic>-report.md` 一次性产物结论入 RELEASE.md 或 ADR" — **P2** |

**Gap 1.1 (P3)**:`docs/adr/` 目录建立 + 把已成型的决策(三产品矩阵 / GitFlow Lite / Cost & Trigger Contract / 加密模型)落档为 ADR-0001…0007。

**Gap 1.2 (P2)**:一次性 sprint report 文档归并 — 关键结论入 RELEASE.md,文档归档到 `docs/archive/<date>-<topic>.md` 或直接删除(若 git log 已留)。

---

## 2. 维度 2 — 实现 + 测试 (Implementation / Testing)

| 项 | 状态 | 实测 |
|----|------|------|
| attune-core lib test | ✅ | 1150 passed / 0 failed / 1 ignored |
| attune-pro law-pro lib test | ✅ | 278 passed / 0 failed / 0 ignored |
| agent_golden_gate | ✅ | 28 passed / 0 failed / 3 ignored (deterministic 1.00 pass rate) |
| stress nightly | ✅ | 4 testsuite(crash recovery / concurrent / OOM / large-scale)CI cron 02:00 UTC |
| Frontend E2E Playwright | ✅ | 45/0 (per RELEASE.md) |
| cross-repo E2E | ✅ | accounts ↔ pluginhub ↔ attune-server LLM gateway ↔ Stripe 端到端 |
| clippy `-D warnings` | ⚠️ | 5/22 audit 报 1 compile error(doc_lazy_continuation in llm.rs:457)— 需确认是否 5/23-5/24 sprint 已清 |
| pluginhub test coverage | ❌ | 历史 finding #155 报 24%(未在本 audit 重新验证 — **P2** 确认当前数) |
| OCR/ASR ratchet ENFORCE | ⚠️ | `office_ocr_golden_gate` / `office_asr_golden_gate` 当前 SKIP-only(YAML 5 / image 0),v1.0.1 backfill real-image fixture |

**Gap 2.1 (P1)**:clippy `-D warnings` 1 compile error 验证当前是否还在(5/22 audit 报);若仍在,GA 前清。

**Gap 2.2 (P2)**:pluginhub test coverage 重新跑 `pytest --cov`,若仍 ~24%,排入 v1.0.1 sprint 补充测试矩阵(API endpoint / 鉴权 / 上传 / quota / Stripe 联动)。

---

## 3. 维度 3 — **升级策略 (User-Facing Upgrade)** ⭐ 用户 priority

> 用户原话:「我感觉升级策略等还是不明了」。本节是这份 audit 的核心 — 把升级链路 user / dev / cloud 三视角的真实状态摊开。

### 3.1 三种升级场景

| 场景 | 当前状态 | 关键 gap |
|------|---------|---------|
| **A. v1.0.0 → v1.0.1 (笔电 user)** | Tauri auto-updater 全链已通(pubkey 嵌入 `tauri.conf.json` ✅ + GH Actions Secret `TAURI_SIGNING_PRIVATE_KEY` ✅ + `latest.json` endpoint = `releases/latest/download/latest.json` ✅);apt/rpm/winget workflow ✅ | **用户角度看不到「升级了什么」** — README "in-app auto-updater" 一句话带过,RELEASE.md 是开发者文档不是 release notes。**v1.0.1 GA 前**需写 `docs/release-notes/v1.0.1-user.md`(用户视角变更:bug fix / 新功能 / breaking)+ in-app updater dialog 显示该 URL。**目前 `dialog: false`(silent update),用户完全感知不到** |
| **B. v0.7 → v1.0 (老用户升级)** | rust/RELEASE.md `### Breaking changes(v0.7.x → v1.0.0)` 节明确"无对外协议层 breaking change"+ `### Migration` 说"自动幂等升级 + 桌面用户旧 data 目录延用"✅ | **vault schema migration 真升级路径未做用户级演练** — `attune-core/src/store/mod.rs` 走 lazy `ALTER TABLE ... ADD COLUMN` + idempotent CREATE INDEX,代码层正确;但**用户视角未验证**:v0.7 用户装 v1.0 后 vault 第一次解锁能否成功?升级失败(power off mid-migration)的恢复路径?`VACUUM INTO` migration tmp file 中断的语义?**v1.0 GA 前需 `tests/v07_to_v10_vault_upgrade.rs` E2E 测试 + 文档化中断恢复 SOP** |
| **C. cloud-v2.2.0 → cloud-v2.3.0 (cloud 自管运维)** | `docs/ROLLBACK.md` 6 场景 ✅ + `cloud.sh --upgrade` 流程化 ✅(per RELEASE 描述) + `detect-domain.sh` ✅(2026-05-25 验证存在) | **`cloud-upgrade.sh` 未独立存在**(只是 cloud.sh 子命令);v2.3.0 引入 submodule(accounts / pluginhub 拆出独立仓),老 v2.2.0 部署用户走 `git submodule update --init --recursive` 这步**在 RELEASE.md 写了但 ROLLBACK.md 没覆盖 submodule init 失败场景** |

### 3.2 用户视角的「升级矩阵」**完全缺**

**核心问题**:用户问"我现在装的是 v1.0.0,看到 v1.0.1 出来,我该怎么办"时,**找不到一份单一文档回答**。
当前信息散落在 4 个地方:

1. `README.md` L57:一句话"built-in auto-updater"
2. `docs/install-package-managers.md`:讲首装 + "后续升级 sudo apt upgrade",但不讲 in-app 路径与包管理器路径**如何选择**
3. `docs/auto-updater-setup.md`:**维护者运维手册**,不是用户文档
4. `RELEASE.md`:开发者 changelog,内部术语多

**Gap 3.1 (P1 - 用户 priority 核心)**:新建 **`docs/UPGRADE.md`**(用户视角单一文档),含:

```markdown
# Attune 升级指南

## 我装的是哪种?
- Windows winget:winget upgrade qiurui144.Attune
- Debian/Ubuntu apt:sudo apt update && sudo apt upgrade attune
- Fedora dnf:sudo dnf upgrade attune
- AppImage:重新下载替换
- Tauri 桌面 in-app:自动检测(每次启动 + 24h 间隔),弹窗确认 → 后台下载 → 重启生效

## 升级矩阵
| 当前版本 | 目标版本 | 数据兼容 | 操作步骤 | 失败恢复 |
|---------|---------|---------|---------|---------|
| v0.6.x | v1.0.0 | ⚠️ vault schema 需 migrate | (1) 备份 vault.db (2) 装新版 (3) 解锁 vault,自动 ALTER | 见 §恢复 |
| v0.7.x | v1.0.0 | ✅ 兼容 | 直接装新版 | — |
| v1.0.0 | v1.0.1 | ✅ patch | in-app 自动 / apt upgrade | — |

## 升级失败恢复 SOP(client 端)
1. vault.db.bak 在 `~/.local/share/attune/vault.db.bak`(每次 schema migrate 前自动产生)
2. 复原:停 attune → cp vault.db.bak vault.db → 重启
3. 报 issue 附 `~/.local/share/attune/logs/attune-server.log`

## attune-pro plugin 升级
1. attune client 升级到对应版本(plugin attune_min_version 强约束)
2. attune plugin uninstall law-pro && attune plugin install law-pro
3. 验证:attune plugin list 显示 stable

## cloud 自部署升级(operator only)
bash cloud.sh --upgrade  # 6 阶段升级 + ROLLBACK.md 6 场景兜底
```

**Gap 3.2 (P1)**:**schema_version 显式记录** — 当前 vault 走 idempotent ALTER 但**没有显式 `PRAGMA user_version` / `schema_version` 表**。升级失败诊断时只能靠 ALTER 表的 columns 检查反推。

具体落地:
- `attune-core::store::SCHEMA_VERSION = "1.0.0"` const
- 首次 init 时写 `PRAGMA user_version = 100` (1.0.0 编码为 100)
- 每次 lazy ALTER 后 bump
- `attune-cli vault status` 显示 schema version

**Gap 3.3 (P2)**:**downgrade plan** — 当前文档隐含"只能升不能降"。用户万一装错版本(v1.0.1 装到 v0.7 用户机器)无对应路径。
建议:
- `attune-cli vault export --format compat` 导出兼容老版本 schema 的中间表示
- Or 明示"v1.0+ 不支持降级,务必先备份 vault"

**Gap 3.4 (P1)**:**in-app updater dialog 用户体验** — `tauri.conf.json` 当前 `"dialog": false`(silent update,代码改 main.rs 触发)。**这意味着用户感知不到升级**。
建议:
- 改 `"dialog": true` 让 Tauri 原生 dialog 弹窗
- 或在 Settings 加一个 "更新" tab 显示当前版本 + 检查更新按钮 + changelog 摘要
- changelog 摘要 fetch from `https://github.com/qiurui144/attune/releases/latest`

### 3.3 跨仓升级配对

| 跨仓 | 当前 | gap |
|------|------|-----|
| attune ↔ attune-pro 强配对 | ✅ v1.0.0-rc.2 == v1.0.0-rc.2 | `version-audit.sh` 已有 — 持续运行 |
| attune ↔ cloud 兼容矩阵 | ✅ `cloud-v2.2.0` RELEASE.md "兼容性矩阵" 节明确支持 attune v1.0.x | Wiki 给终端用户的"哪个 cloud 配哪个 client" 表缺 — **P2** |
| attune-pro ↔ pluginhub plugin 协议 | ✅ `attune_min_version: 1.0.0` 锁定 | hub 端 plugin upload 时 server 校验 attune_min_version 兼容(防止用户安装一个 attune_min_version: 2.0 plugin 到 v1.0)— 需验证 — **P2** |

---

## 4. 维度 4 — Observability / Monitoring

### 4.1 当前状态

✅ **已有**:
- gatus 5 endpoint(会员中心 / LLM Gateway / PluginHub 存活 + 插件健康 / 状态监控页),SMTP email alert 全链通(`monitor/config/config.yaml` + `docker-compose.yml`)
- `llm-gateway/scripts/cost-alert.sh` daily cron $10/day 阈值 SMTP 告警
- attune-core `store::audit_log` 表(`ensure_audit_log_table` lazy create + RFC3339 ts + route/category/kind/redacted_count)— client 端审计日志

### 4.2 真缺口

**Gap 4.1 (P1)**:**应用层 metrics endpoint 缺**。grep `metrics router attune-server` = 0 hit。
attune-server / accounts / pluginhub 没有 `/metrics`(Prometheus) endpoint。Gatus 是黑盒 healthcheck,无法回答:
- chat req/s P50/P95/P99 latency
- LLM gateway 各 provider cost trend (本地 + DeepSeek + OpenAI)
- vault unlock 失败率(暴力破解检测)
- agent dispatch 成功/失败率 by agent_id
- pluginhub plugin download QPS

建议:
- attune-server 加 `prometheus = "0.13"` + `axum-prometheus` 中间件
- accounts (Python) 加 `prometheus-client` + `/metrics` route
- pluginhub 同上
- cloud/monitor/ 加 prometheus container + scrape config + grafana(可选,先 prometheus stand-alone)

**Gap 4.2 (P1)**:**log aggregation 缺**。当前各容器各 `docker logs`,Stripe webhook 失败 / vault unlock 异常 / LLM gateway 5xx 等事件**没有中心化 query**。
建议:
- Loki + Promtail 加进 monitor stack(轻量,与 gatus 共存)
- 或先用最低成本:`docker logs --tail 10000 > daily.log` 每天 cron rotate,user 可下载

**Gap 4.3 (P2)**:**用户行为 analytics 缺**(privacy-friendly)。当前没有任何 product analytics — 不知道"用户首装后第几天到达 vault 第一条 item"、"wizard 完成率"、"哪个 plugin 卸载率最高"。
建议:
- 选 PostHog self-hosted(privacy-friendly,GDPR-compatible) — 单 docker container 起;或
- 简单:attune-server 自己写 anonymized event 表 + 用户**显式 opt-in 才传**到 cloud telemetry endpoint

**Gap 4.4 (P3)**:**alert 多通道** — 当前只 SMTP email。Slack/Discord/Telegram bot webhook 是 v1.1 路径(gatus 原生支持,改 config.yaml 就行)。

---

## 5. 维度 5 — Security 持续机制

| 项 | 状态 | 实测 |
|----|------|------|
| cargo audit | ✅ | `.github/workflows/ci.yml` `cargo-audit` job (rustsec/audit-check@v2 + cargo-deny) |
| dependency CVE scan | ✅ | 上述 |
| crypto audit | ✅ | `docs/v1.0-crypto-security-audit.md` (AES-GCM nonce / Argon2id / vault session token / device secret / zeroize) |
| SECURITY.md | ✅ | per RELEASE.md "v1.0 New: cargo audit + deny.toml + SECURITY.md" |
| secret rotation 自动化 | ❌ | `.sops.yaml` ✅ 但 age key rotation 是手动流程,无周期 cron |
| pen test 外部红队 | ❌ | 未做 |
| fuzz testing (REST surface) | ❌ | 未做。attune-server `/api/v1/*` 没有 fuzz harness |
| dependabot / renovate | ❌ | grep 无配置(只 `algo-base` 不相关项目命中) |

**Gap 5.1 (P2)**:**dependabot.yml** 加 `.github/dependabot.yml` 跟踪 cargo + npm + pip(accounts) — 自动 PR + cargo audit CI 已经 gate 安全升级。

**Gap 5.2 (P2)**:**fuzz harness** — `cargo-fuzz` for attune-server REST endpoint;v1.1 sprint 之一。

**Gap 5.3 (P3)**:**pen test 外部红队**(预算允许);v1.1 之后。

**Gap 5.4 (P2)**:**secret rotation cron** — `cloud/.sops.yaml` age key 每 90 天提示轮换;脚本化(`cloud/scripts/rotate-secrets.sh`)。

---

## 6. 维度 6 — Performance / Capacity

| 项 | 状态 | 实测 |
|----|------|------|
| stress baseline | ✅ | `docs/v1.0-stress-baseline.md` + `.github/workflows/stress-nightly.yml` 4 testsuite |
| crash recovery test | ✅ | PASS |
| concurrent stress | ✅ | 10 thread / 1000 vector / 0 race |
| OOM behavior | ✅ | PASS |
| large-scale FTS/Vector P99 | ⚠️ | "待 nightly 首跑测定" — **数据仍空** |
| 1000-user production stress | ❌ | 未做 |
| SLA 数值化承诺 | ❌ | RELEASE / pages.yaml 未承诺具体 P99 数字 |

**Gap 6.1 (P1)**:`stress_large_scale_test` nightly 已配,**真数据需 first run 入 baseline**。当前文档"待测定" 5/26 上架前必须有数。

**Gap 6.2 (P2)**:**SLA 文档化** — `docs/SLA.md`:
- attune client:vault unlock P99 < 200ms (笔电 SSD)
- chat:P99 < 5s(本地 qwen2.5:3b) / P99 < 8s(DeepSeek)
- search:P99 < 500ms(10k item) / P99 < 1s(100k item)
- cloud uptime:gatus 5 endpoint 月度 99.5%(单 region,无 redundancy)

**Gap 6.3 (P2)**:**capacity planning** — 单 cloud server 能支撑多少 user?accounts / pluginhub / llm-gateway 各容器内存 / CPU 上限;到达 X user 时如何扩容(垂直 vs 水平)。

---

## 7. 维度 7 — DR / Business Continuity

| 项 | 状态 | 实测 |
|----|------|------|
| cloud ROLLBACK.md 6 场景 | ✅ | 5075 bytes / 全 6 场景齐 |
| official-web backup.sh + restore.sh | ✅ | wp DB + wp-content 每日 cron |
| accounts / pluginhub backup | ⚠️ | accounts 是 git submodule + postgres DB,backup 流程未独立文档化 |
| **真 restore drill** | ❌ | 未演练 — 必须做一次"假装 cloud 挂了,从备份恢复" |
| attune client vault DR | ⚠️ | `.av-migrate.tmp` 中断恢复代码层有,**用户级 SOP 文档缺**(per §3.1 B) |
| region fail | ❌ | 单 region cloud,无 failover |

**Gap 7.1 (P1)**:**accounts / pluginhub DB backup runbook** — backup.sh 类似 official-web 但 PostgreSQL 而非 MySQL;`cloud/scripts/backup-accounts.sh` + cron + S3/B2 远端备份(防 region fail)。

**Gap 7.2 (P1)**:**真 restore drill** — 在测试环境跑一次 full restore,记录耗时 + 数据完整度;`docs/dr-drill-2026-05-26.md`。

**Gap 7.3 (P2)**:**attune client vault DR SOP**(已在 §3.2 Gap 3.1 covered)。

**Gap 7.4 (P3)**:**multi-region failover** — v2.0 cloud 路径;v1.0 单 region 接受。

---

## 8. 维度 8 — Customer Support + Issue Flow

| 项 | 状态 | 实测 |
|----|------|------|
| 支持渠道指定 | ✅ | `pages.yaml` 声明 GitHub Issues `qiurui144/attune` 24h response |
| SLA(P0/P1/P2) | ⚠️ | pages.yaml 只承诺 24h response,**不分级别** |
| issue template | ❌ | `.github/ISSUE_TEMPLATE` 不存在(per audit grep) |
| PR template | ❌ | `.github/PULL_REQUEST_TEMPLATE.md` 不存在 |
| SECURITY.md disclosure | ✅ | 存在(per RELEASE.md) |
| bug bounty | ❌ | 未做 |

**Gap 8.1 (P1 - 5/26 上架前修)**:`.github/ISSUE_TEMPLATE/` 加 3 个 yaml:
- `bug_report.yml`(版本 / OS / 重现步骤 / 期望 / 实际 / 日志)
- `feature_request.yml`(用例 / 价值 / 替代方案)
- `support_question.yml`(问题描述 / 已尝试方案 + 引导到 wiki FAQ)

**Gap 8.2 (P1)**:`.github/PULL_REQUEST_TEMPLATE.md` — 描述 / 关联 issue / 测试 / 截图 / breaking change。

**Gap 8.3 (P2)**:**SLA 分级承诺** — `docs/SUPPORT.md`:
- P0 vault 解锁失败 / 数据丢失:24h
- P1 chat / search 不工作:48h
- P2 UI / 文档:1 周
- P3 feature request:无 SLA(会评估)

---

## 9. 维度 9 — Payments / Billing / Quota

| 项 | 状态 | 实测 |
|----|------|------|
| Stripe checkout | ✅ | `accounts/api/billing.py` POST `/api/v1/billing/checkout` |
| customer create idempotent | ✅ | `_ensure_stripe_customer` `idempotency_key=cust-create-<user.id>` |
| webhook 测试 | ✅ | `test_stripe_webhook.py` 22 case |
| billing portal | ✅ | `accounts/api/billing.py` GET /billing |
| 中国发票合规 | ❌ | 无 |
| chargeback handling | ⚠️ | `accounts/web/email_templates/payment_failed_*.html` ✅ 但 chargeback 流程未独立 |
| user quota dashboard | ❌ | 用户看不到"我这个月用了多少 token / 还剩多少" |
| DSAR (data export per GDPR) | ❌ | grep `dsar|gdpr` 只命中 ORM relationship cascade — 无显式 user export/delete API |
| account delete | ⚠️ | `cascade="all, delete-orphan"` 数据库层有,**API endpoint 未确认**(`DELETE /api/v1/users/me`?)|

**Gap 9.1 (P1)**:**user quota dashboard** — `accounts/web/templates/upgrade.html` 上面加一个"本月用量"区块,显示 token 余额 + Stripe 月度账单链接。

**Gap 9.2 (P1 - 法律强约束)**:**DSAR API** —
- `GET /api/v1/users/me/export`(JSON dump: profile / licenses / billing history)
- `DELETE /api/v1/users/me`(soft delete + 30 天后 hard delete + Stripe 取消订阅)
- 配套 `docs/data-rights.md`(用户可见 url:`https://engi-stack.com/data-rights`)

**Gap 9.3 (P2)**:**中国发票合规** — 走 Stripe 不发中国 增值税专票/普票。若 5/26 上架后有中国企业用户,得接 third-party(诺诺 / 百望)或先声明"暂不开发票,需开发票请联系 sales@"。

---

## 10. 维度 10 — Legal / Compliance 🔴 真缺口

| 项 | 状态 | 实测 |
|----|------|------|
| LICENSE (attune OSS) | ✅ | Apache 2.0 |
| LICENSE (attune-pro) | ✅ | LicenseRef-Proprietary |
| 官网 ToS 页面 | ❌ | grep 仅 wiki-web `external/attune-enterprise/` 内有 `京ICP证010230-17`(attune-enterprise 老备案,不是 attune) |
| 官网 Privacy Policy 页面 | ❌ | 同上,无 attune 自己的 |
| DPA (Data Processing Agreement) | ❌ | 企业合同需要,缺 |
| 中国 ICP 备案号 | ❌ | engi-stack.com 域名是否 ICP 备案未确认;若服务器在大陆 + 域名 engi-stack.com 解析到大陆 IP,**法定必须备案** |
| GDPR 合规 path | ⚠️ | 部分 — privacy 设计层面 OK(本地优先 / 端到端加密),但 user-facing DSAR API 缺 |
| 中国个人信息保护法 | ⚠️ | 同上 |
| enterprise 合同律师审核 | ❌ | 未做 |
| 开源依赖合规扫描 | ✅ | cargo deny check (licenses + bans) |

**Gap 10.1 (P0 - 5/26 上架阻断)**:**ToS / Privacy Policy 真页面缺**。
官网 `https://engi-stack.com/tos` + `https://engi-stack.com/privacy` 必须有真内容。建议:
- 参考 Notion / Linear 的 ToS / Privacy 草稿(SaaS 通用)
- 找律师审一遍(中国数据主体 + 海外用户双 jurisdiction)
- 5/26 上架前必须 publish — **这是 P0**

**Gap 10.2 (P0 - 法定)**:**ICP 备案** — 若 engi-stack.com 解析到大陆服务器,工信部备案是法定义务。若不备案,大陆访问会被阻断。
- 5/26 上架前确认:服务器位置 + 域名解析 + 是否已备案
- 未备案选项:(a) 备案(7-20 工作日) (b) 服务器迁海外(港/美) — 各有权衡

**Gap 10.3 (P1)**:**DPA 模板** — 企业客户(尤其欧洲)谈合同会要。Notion DPA / Linear DPA 模板抄一份适配 + 律师审。

**Gap 10.4 (P2)**:**律师审 enterprise contract** — v1.0 上架后接到第一个企业询单时再做也来得及,但**别等签合同那天才找律师**。

---

## 11. 维度 11 — i18n + Release Engineering 真链

### 11.1 i18n

| 项 | 状态 |
|----|------|
| ui/src/i18n/zh.ts | ✅ 830 行 |
| ui/src/i18n/en.ts | ✅ 835 行 |
| key 集合对齐 | ⚠️ 5/15 后还有 ~100 处硬编码中文待迁移(per CLAUDE.md i18n 规范)— 状态需 grep 复核 |
| wiki-web i18n/en/ | ⚠️ 存在但内容覆盖度未审 — 中文写完英文是否同步? |
| official-web 双语 | ✅ pages.yaml 双语 |

**Gap 11.1 (P2)**:跑 CLAUDE.md 强制 grep 守卫:
```bash
cd rust/crates/attune-server/ui/src
grep -rnP "(toast\([^)]*'[^']*[\x{4e00}-\x{9fff}]|(title|placeholder|label|description|aria-label)=\"[^\"]*[\x{4e00}-\x{9fff}]|>[^<{]*[\x{4e00}-\x{9fff}])" --include="*.tsx" . | grep -v "/i18n/"
```
报告 0 输出才算 i18n 债清。v1.0.1 sprint 清零。

**Gap 11.2 (P2)**:**wiki-web i18n/en/ 内容审计** — `i18n/en/docusaurus-plugin-content-docs/current/` 是否覆盖所有中文 doc?5/26 国际用户来 wiki 看到中文页面会流失。

### 11.2 Release Engineering 真链

| 项 | 状态 |
|----|------|
| rust-release.yml | ✅ 4 平台(linux-x64 / linux-arm64 / mac-arm64 / win-x64) |
| desktop-release.yml | ✅ 2 OS × 5 bundle |
| apt-rpm-repo.yml | ✅ |
| winget.yml | ✅ workflow 存在 |
| docker-publish.yml | ✅ |
| **winget PR 真合入** | ❓ 历史 v0.6.x 是否真上架到 microsoft/winget-pkgs?搜索 `winget search Attune` 是否返回? |
| **APT/RPM 真验证** | ❓ `https://qiurui144.github.io/attune/apt` 真 publish 状态?用户跑 `sudo apt install attune` 是否真能 install? |
| macOS Intel | ❌ Removed from matrix(per audit "macos-x86_64 removed") |
| macOS aarch64 | ✅ in matrix |
| Linux ARM64 desktop .deb | ❌ "不在 desktop-release matrix"(per RELEASE 已知限制) |
| Tauri signing key revocation plan | ❌ pubkey 嵌入 binary,key 泄漏需 force-update 老 client — 未文档化 |

**Gap 11.3 (P1 - 5/26 上架阻断)**:**winget / apt / rpm 端到端真验证**:
- winget:`winget search Attune` 在干净 Windows 11 跑一次
- apt:`apt install attune` 在干净 Ubuntu 24.04 跑一次
- rpm:`dnf install attune` 在干净 Fedora 40 跑一次
- 3 个验证截图入 `docs/screenshots/v1.0-package-managers/`

**Gap 11.4 (P2)**:**Tauri signing key compromise SOP** — `docs/auto-updater-setup.md` 加一节:
- key 泄漏检测信号(GitHub Actions log + secret 仓库 access)
- 应急:rotate keypair → 嵌入新 pubkey → v1.0.x+1 patch release → 老 client 升级时用旧 pubkey 验签新 pubkey commit → 老 client 升一次后,后续走新 pubkey
- 注:老老 client(没升的)永远卡在旧版,得手动重装 — 文档化

**Gap 11.5 (P2)**:**macOS Intel + Linux ARM64 desktop** — v1.1 路径声明 + cross-compile 验证。

---

## 12. Gap 优先级汇总 (P0 / P1 / P2 / P3)

### P0 — 5/26 上架阻断

| # | Gap | 维度 | Effort |
|---|-----|------|--------|
| 10.1 | 官网 ToS + Privacy Policy 真页面 | Legal | 1 天(律师审 + content) |
| 10.2 | ICP 备案 OR 服务器迁海外 决策 | Legal | 1 天调研 + 7-20 天备案(若选 ICP) |

### P1 — 5/26 GA 当日或 5/26 后 7 天内修(否则上架体验/可见度受影响)

| # | Gap | 维度 | Effort |
|---|-----|------|--------|
| 3.1 | `docs/UPGRADE.md` 用户视角升级矩阵 | 升级 ⭐ | 0.5 天 |
| 3.2 | `PRAGMA user_version` schema_version 显式 | 升级 ⭐ | 0.5 天 |
| 3.4 | in-app updater dialog UX(改 `dialog: true` 或 Settings tab) | 升级 ⭐ | 0.5 天 |
| 2.1 | clippy `-D warnings` 0 error 验证 | 测试 | 0.5 天 |
| 4.1 | 应用层 `/metrics` Prometheus endpoint | Observ | 1 天 |
| 4.2 | log aggregation(Loki + Promtail) | Observ | 1 天 |
| 6.1 | `stress_large_scale_test` 真 baseline 数据 | Perf | 1 天 |
| 7.1 | accounts / pluginhub backup runbook | DR | 0.5 天 |
| 7.2 | 真 restore drill 演练 | DR | 1 天 |
| 8.1 | `.github/ISSUE_TEMPLATE/` 3 个 yaml | Support | 0.5 天 |
| 8.2 | `.github/PULL_REQUEST_TEMPLATE.md` | Support | 0.5 天 |
| 9.1 | user quota dashboard | Billing | 1 天 |
| 9.2 | DSAR API(export / delete user) | Billing | 1 天 |
| 11.3 | winget / apt / rpm 端到端真验证 | Release | 0.5 天 |

P1 总计 ~9 天 effort。可并行(独立 worktree)在 5/26-6/2 sprint 完成。

### P2 — v1.0.1 / v1.0.2 跟进

| # | Gap | 维度 |
|---|-----|------|
| 1.2 | sprint report 文档归并 | 需求 |
| 2.2 | pluginhub test coverage 24% → 60%+ | 测试 |
| 3.3 | downgrade plan / vault export compat | 升级 |
| 4.3 | PostHog product analytics(opt-in) | Observ |
| 5.1 | dependabot.yml | Security |
| 5.2 | cargo-fuzz harness | Security |
| 5.4 | secret rotation cron | Security |
| 6.2 | `docs/SLA.md` | Perf |
| 6.3 | capacity planning doc | Perf |
| 8.3 | `docs/SUPPORT.md` P0/P1/P2 分级 SLA | Support |
| 9.3 | 中国发票合规(诺诺/百望 接入 OR sales@ 兜底) | Billing |
| 10.3 | DPA 模板 + 律师审 | Legal |
| 11.1 | i18n grep 守卫 0 输出 | i18n |
| 11.2 | wiki-web i18n/en/ 内容审计 | i18n |
| 11.4 | Tauri signing key compromise SOP | Release |
| 11.5 | macOS Intel + Linux ARM64 desktop | Release |

### P3 — v1.1+

| # | Gap | 维度 |
|---|-----|------|
| 1.1 | `docs/adr/` ADR 落档 | 需求 |
| 4.4 | alert 多通道(Slack/Discord) | Observ |
| 5.3 | pen test 外部红队 | Security |
| 7.4 | multi-region failover | DR |
| 10.4 | enterprise contract 律师审 | Legal |

---

## 13. 落地建议 (v1.0 / v1.0.1 / v1.1)

### 5/25-5/26 GA 窗口

**唯一 GA-blocker** = P0(10.1 + 10.2)。

- 10.1 ToS + Privacy:**今天 5/25 起草,5/26 上架前 publish**。先 internal review + 律师后续审
- 10.2 ICP 备案/服务器决策:**今天 5/25 决策**。若选海外服务器,5/26 上架可行;若选 ICP 备案,5/26 仅在海外可访,大陆访问 7-20 天后通

P0 解完 → **5/26 上架 Go**(per final audit P0=0 → GA Go 仍成立)。

### v1.0.1 sprint(5/27-6/2,1 周)

把上述 P1 的 14 项分成 ~4 个并行 worktree:

| Worktree | P1 项 | 主题 |
|----------|-------|------|
| `feat-upgrade-ux` | 3.1 / 3.2 / 3.4 | 升级策略 UX(用户 priority ⭐) |
| `feat-observability` | 4.1 / 4.2 / 6.1 | metrics / log / stress |
| `feat-billing-rights` | 9.1 / 9.2 | quota dashboard + DSAR |
| `feat-ops-hardening` | 7.1 / 7.2 / 8.1 / 8.2 / 11.3 | DR drill + issue/PR template + 端到端真验证 |
| (主仓 develop) | 2.1 | clippy 0 error |

per CLAUDE.md "并行开发 + 串行 tag",merge 顺序定 v1.0.1 / v1.0.2 / v1.0.3 / v1.0.4。

### v1.1 sprint(6 月)

P2 全 16 项 + tech-pro / presales-pro / patent-pro 框架激活 + DeepSeek VLM provider。

---

## 14. 与 5/22 final audit 的差异

| 视角 | 5/22 final audit | 本 audit |
|------|-----------------|---------|
| 目的 | **GA Go/No-Go**(P0=0 → Go) | **工程纪律 + 运营长期视角** |
| 维度 | 8(client / pro / cloud / GH workflows / 文档 / GA ceremony / 已知 issue / 上架 readiness) | 11 SDLC 维度 |
| P0 项 | 0 | 2(legal — 5/22 audit 未把 Legal 拉成独立维度) |
| 升级策略 | 散落各维度 | **§3 独立详节**(用户 priority) |
| Observability | 维度 3 cloud 节简单一句 | **§4 独立维度**(metrics + log + analytics) |
| Legal/Compliance | 未单列 | **§10 独立维度**(ToS / Privacy / ICP / DPA / DSAR) |

**结论**:5/22 final audit "P0=0 Go" 仍成立(GA blocker 视角),但本 audit 把 legal 拉成 P0 是因为**上架公开 SaaS 不能没有 ToS/Privacy + ICP**(法定 + 用户信任) — 5/22 audit 是工程 GA 视角,本 audit 是产品上架视角,两个 P0 不冲突,**两者并行修**。

---

## 附录 A — 用户原话与 audit 映射

| 用户原话 | 落地段 |
|---------|--------|
| 「评估现有缺口」 | §0 维度总览 + §12 P0/P1/P2/P3 |
| 「升级策略等还是不明了」 | **§3 独立详节** + Gap 3.1 / 3.2 / 3.3 / 3.4 |
| 「软件工程的方式进行思考」 | 11 维度 SDLC 框架(需求 / 实现 / 测试 / 升级 / 观测 / 安全 / 性能 / DR / 支持 / 合规 / release eng) |
| 「评估项目缺口」 | §12 优先级汇总 + §13 落地建议 |

---

> **本 audit 不引入新规划/新 spec,只评估**。所有 Gap 已映射到具体 ROI(P0/P1/P2/P3)
> 和 effort 估算。v1.0.1 sprint 是否纳入哪些 P1 由用户拍板。
