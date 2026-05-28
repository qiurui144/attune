# v1.0 GA + v1.0.x 缺口闭环 Roadmap

> **用户原话**:「做好规划。尽快 v1.0 定版,而后开始把刚才的缺口完全补全」
>
> 触发:2026-05-25 软件工程 11 维度 gap audit + 升级策略缺口诊断后。

## 目录

- [1. 目标定位](#1-目标定位)
- [2. 范围边界](#2-范围边界)
- [3. 架构数据流(v1.0 + 升级策略)](#3-架构数据流v10--升级策略)
- [4. 模块边界(12 minor 拆解)](#4-模块边界12-minor-拆解)
- [5. API 契约](#5-api-契约)
- [6. 扩展点](#6-扩展点)
- [7. 错误处理 + 边界 case](#7-错误处理--边界-case)
- [8. 成本契约](#8-成本契约)
- [9. 测试矩阵](#9-测试矩阵)
- [10. 向后兼容](#10-向后兼容)
- [11. 风险登记](#11-风险登记)
- [附录 A:切片表(SSOT)](#附录-a切片表-ssot)
- [附录 B:Task tracker 链](#附录-btask-tracker-链)

---

## 1. 目标定位

**用户痛点**:
- 5/25 v1.0 GA tag 必须今天 push(不能再拖,user 已两次重申)
- v1.0 GA 后 11 维度 SE gap(升级策略 / observability / DR / legal / ...)必须**有计划闭环**,不能"GA 完就散"
- v1.0.1 起每 minor 必须有 distinct deliverable,不允许"5 个 feature 攒一起塞 v1.0.1"

**产品 positioning 对齐**:
- 隐私 / 本地优先 / 分层成本 / 混合智能 — 4 个 GA 后 minor 都不能破坏这 4 项
- 升级策略尤其要保**用户 vault 数据 0 丢失**(per § Secrets + 私有 AI 知识伙伴定位)

## 2. 范围边界

**v1.0.0 GA 5/25 做什么**:
- CI 3 fail 修齐(in-flight #162)
- 3 仓 ceremony tag push:`attune v1.0.0` + `attune-pro v1.0.0` + `cloud-v2.2.0`
- desktop tag `desktop-v1.0.0` 触发 GH Actions 多平台 install pkg build
- RELEASE.md v1.0.0 节填齐(Highlights / Breaking / Migration / Known Limitations per RC Gate 4)

**v1.0.0 GA 5/25 不做什么**:
- ❌ 升级策略 SSOT(推 v1.0.1)
- ❌ Tauri auto-updater 私钥签名(推 v1.0.1)
- ❌ VLM provider 接入(推 v1.1.0)
- ❌ defamation v3 prompt cloud verify(推 v1.0.1 #71)
- ❌ DB rename PostgreSQL alembic(推 v1.0.2)

**v1.0.1 - v1.0.12 做什么**:
- 11 维度 SE gap 按 minor 拆,每个 minor 一个 distinct deliverable(per § 拆解原则)
- 切片表见附录 A

**全部不做**(推 v1.1+):
- macOS .dmg(per CLAUDE.md "暂不做")
- Linux aarch64 通用 ARM(K3 = riscv64,不是 aarch64)
- pluginhub 第三方插件商业分成

## 3. 架构数据流(v1.0 + 升级策略)

### v1.0.0 GA ceremony 路径

```
[CI 3 fail 修齐(#162)]
      ↓ all green
[develop snapshot]
      ↓ git checkout main && git merge --no-ff develop
[main HEAD]
      ↓ git tag v1.0.0 + desktop-v1.0.0 push
[GH Actions]
      ↓ rust-release.yml(server / CLI tarball ×5 平台)
      ↓ desktop-release.yml(Tauri NSIS / MSI / deb / rpm / AppImage)
[Release page artifacts × 7]
      ↓ user download + install
[v1.0.0 live]
```

### v1.0.1+ 升级数据流(Tauri auto-updater)

```
[user 装 v1.0.0]
      ↓ Tauri 启动每 N 小时 GET https://github.com/qiurui144/attune/releases/latest/download/latest.json
[latest.json: {version, signature, pub_date, url}]
      ↓ updater verify signature(minisign with attune private key)
[user dialog: "v1.0.1 available, upgrade?"]
      ↓ user 同意
[downloader: 下 .deb / .msi]
      ↓ verify pubkey 一致
[installer 启动]
      ↓ pre-upgrade backup: ~/.local/share/Attune/backups/vault.db.bak.YYYYMMDD
      ↓ migration check: schema version compare → run alembic upgrade if needed
[v1.0.1 live]
```

### Rollback 数据流(任一升级失败)

```
[user 升级后 app 不启动 / panic]
      ↓ user 跑 attune --rollback(新增 CLI 子命令,v1.0.1 必备)
[rollback 流程]
      ↓ 检查 ~/.local/share/Attune/backups/vault.db.bak.YYYYMMDD
      ↓ 恢复 vault.db
      ↓ 卸载新版 → 装回 backup/attune-v0.6.3.deb(预 cache 老安装包)
      ↓ 用户回 v0.6.3
[app 启动]
```

## 4. 模块边界(12 minor 拆解)

| 仓 | 模块 | v1.0.0 | v1.0.1 | v1.0.2-12 |
|----|------|--------|--------|----------|
| attune | apps/attune-desktop | tauri.conf.json:54 latest.json endpoint OK | **生成 minisign 私钥 + publish-latest-json.yml + UPGRADING.md** | — |
| attune | rust/crates/attune-server | OK(v1.0 final) | + `/api/v1/version` GET 主动通知 + `--rollback` CLI 子命令 | + metrics(v1.0.3)+ observability(v1.0.3)|
| attune | rust/crates/attune-core | OK | + vault schema_version 升级路径 | + log aggregation(v1.0.3)|
| attune-pro | plugins/*-pro | OK(14 agent ship) | + plugin upgrade test fixture | — |
| cloud | docker compose | v2.2.0(LLM gateway 5 provider)| **zero-downtime upgrade 蓝绿 path** + alembic upgrade head guard | + monitoring stack(v1.0.3)|
| cloud | accounts | OK(per #6521a93)| + DSAR / 数据导出 endpoint(v1.0.7)| + invoice 中国合规(v1.0.7)|
| 新 | docs/UPGRADING.md | — | **NEW SSOT(白名单允许)** | + 每 minor 升级节 |
| 新 | docs/ROLLBACK.md | — | **NEW playbook** | — |
| 新 | docs/SECURITY-OPS.md | — | — | + rotation playbook(v1.0.4)|

## 5. API 契约

**v1.0.0 GA**:无新 API(凝固 v0.7+ 现状)

**v1.0.1 升级策略新增**:

```
GET /api/v1/version
→ 200 OK
{
  "current": "1.0.0",
  "latest_available": "1.0.1",  // 主动 query GitHub release
  "upgrade_available": true,
  "upgrade_url": "https://github.com/qiurui144/attune/releases/tag/v1.0.1",
  "breaking_changes": false,
  "rollback_supported": true
}
```

```
CLI:
attune --rollback              # 列出可 rollback 的版本
attune --rollback v0.6.3       # rollback 到指定版本
attune --pre-upgrade-backup    # 强制创建 backup point(用户主动)
```

```
latest.json schema(Tauri auto-updater 规范):
{
  "version": "v1.0.1",
  "notes": "...",
  "pub_date": "2026-05-27T10:00:00Z",
  "platforms": {
    "linux-x86_64": {
      "signature": "<minisign signature>",
      "url": "https://github.com/qiurui144/attune/releases/download/desktop-v1.0.1/Attune_1.0.1_amd64.deb"
    },
    "windows-x86_64": {
      "signature": "...",
      "url": "..."
    }
  }
}
```

## 6. 扩展点

- 新 minor 加新升级 path → 加 section 进 `docs/UPGRADING.md`(SSOT 不分裂)
- 新 metric / log source → `attune-server::observability::metrics` crate(v1.0.3 引入)
- 新 security audit channel → `docs/SECURITY-OPS.md` rotation playbook

## 7. 错误处理 + 边界 case

| 场景 | exit code | 用户路径 |
|------|-----------|----------|
| latest.json fetch fail(网络) | non-fatal,silent retry | next session 再 check |
| signature verify fail | exit 11,**不安装** | UI 报 "Update integrity check failed" |
| vault schema migration fail | exit 12,**保留 backup** | UI 引导跑 `attune --rollback` |
| disk full during backup | exit 13 | UI 报 "Insufficient disk for backup,清理后重试" |
| rollback 找不到 backup | exit 14 | UI 报 "No backup,从 GitHub release 手动下载老版重装" |

## 8. 成本契约

- v1.0 GA:**零成本**(只是 tag + GH Actions free runner)
- v1.0.1 Tauri updater:**零成本**(GH releases 流量 free for public repo)
- v1.0.3 observability:🟡 本地算力(Prometheus 自托管)→ ⚡ 但用户开会让 user 自选 enable(默认关)
- v1.0.4 security audit:外部 pen test = 💰 大约 $5k-10k(企业服务,user 决策)
- v1.0.x 中所有 cloud 端 metric / log 收集**强制脱敏**,user 私人 vault 内容**永不**进 telemetry

## 9. 测试矩阵

per § 测试方案规范 8 场景覆盖:

| 场景 | v1.0.0 GA | v1.0.1 升级策略 |
|------|----------|----------------|
| happy path | ✅ install pkg 真装 | v0.7→v1.0→v1.0.1 链 |
| edge case | ✅ K3 riscv64 不 build .deb | latest.json 缺字段 |
| error case | ✅ CI fail blocking | signature verify fail / disk full |
| adversarial | ✅ prompt injection 3-layer | 篡改 latest.json url |
| 多并发 | — | 同时多 user 升级 |
| 资源耗尽 | — | disk full / network 断 |
| 国际化 | i18n key ≥ 5/15 | 升级 dialog 中英双语 |
| 降级 | LLM fail graceful | rollback playbook |

## 10. 向后兼容

**v1.0.0 GA**:与 v0.7 + v0.6.3 vault.db schema 兼容(已实测)
**v1.0.1 起**:vault.db schema 升级必带 `schema_version` 字段 + alembic migration script,不允许 destructive ALTER
**cloud-v2.2 → v2.3+**:DB rename(lawcontrol → attune_enterprise)在 v1.0.2 一次性 migration,**附 5/28 user-action 脚本 + rollback path**

## 11. 风险登记

| R | 描述 | 缓解 |
|---|------|------|
| R1 | 5/25 CI 修不齐 → GA 推到 5/26 | 备选 fallback:wiki-dispatch / cargo-audit disable 等 user 决策 |
| R2 | desktop-v1.0.0 tag 触发 build 在某平台 fail | per § Release 真链 P0 监控,GH Actions 任一平台 fail 立即 hotfix 重 tag |
| R3 | Tauri auto-updater 私钥泄露 | v1.0.1 onwards 私钥**只在 1 个 GH secret 存**,rotation playbook v1.0.4 落地 |
| R4 | v0.7 用户升 v1.0 vault schema 不兼容 | 5/25 GA 前**实测 v0.7 vault → v1.0 升级路径**(per § Release 验证) |
| R5 | latest.json publish workflow 漏写 | v1.0.1 spec 强制要求 publish-latest-json.yml CI + 真上传后 verify HEAD 返回 200 |
| R6 | 用户 disk 不够做 pre-upgrade backup | v1.0.1 加 disk space precheck,不够提示用户清理 |
| R7 | minor 节奏失控(v1.0.x 不收敛) | 每 minor 一个 distinct deliverable,**最多 12 个 minor**,超出强制升 v1.1 |

---

## 附录 A:切片表(SSOT)

per § 版本拆解能力 §4 强制产出 — 主题 + 交付 + 时间 + tag 位置 + 依赖

| 版本 | 主题 | 关键交付 | 完成日期 | tag 位置 | blockedBy |
|------|------|---------|---------|---------|-----------|
| **v1.0.0** | **GA 首发** | CI 3 fail 修齐 + 3 仓 ceremony tag push + 多平台 install pkg | **2026-05-25** today | **main** | #162(CI fix)|
| ⚠️ **5/26 hotfix** | **Legal P0 + Release Eng P1**(audit 拉出) | (a) ToS / Privacy Policy publish 到 engi-stack.com(SaaS 通用模板 + 律师 P3 审) (b) ICP 备案状态确认 / 决策(备案 vs 海外服务器) (c) winget / apt / rpm 干净环境真验证 | **2026-05-26** before 上架 | main | v1.0.0 |
| v1.0.1 | **升级策略 SSOT + 法定 DSAR + support template** | UPGRADING.md + ROLLBACK.md + Tauri minisign pubkey + publish-latest-json.yml + `/api/v1/version` + `attune --rollback` CLI + `.github/ISSUE_TEMPLATE` 3 yaml + `.github/PULL_REQUEST_TEMPLATE.md` + DSAR API(`GET /me/export` + `DELETE /me`)+ user quota dashboard | 2026-05-28 | main | v1.0.0 + 5/26-hotfix |
| v1.0.2 | **DB rename + SLA 分级 + DPA + i18n 债清** | PostgreSQL alembic lawcontrol→attune_enterprise migration(5/28 user-action 已 ready)+ docs/SUPPORT.md SLA P0/P1/P2/P3 分级 + DPA 模板(Notion / Linear 抄+本地化)+ ui/src i18n 硬编码中文 ~100 处迁完(grep 守卫 0)+ wiki-web en/ 内容审计 | 2026-05-31 | main | v1.0.1 |
| v1.0.3 | **Observability** | Prometheus metrics + log aggregation(loki)+ alert(LLM gateway 之外 5 service)+ user analytics opt-in | 2026-06-05 | main | v1.0.2 |
| v1.0.4 | **Security 持续** | pen test 外包 + cargo audit CI 加强 + secret rotation playbook + DSAR API | 2026-06-12 | main | v1.0.3 |
| v1.0.5 | **Performance scale** | 1000 user 并发 stress + 100GB vault test + cold-start opt + SLA 数值化(P99 chat < 5s)| 2026-06-18 | main | v1.0.4 |
| v1.0.6 | **DR / BCP** | backup restore 真演练 + region failover doc + SLA 99.5% 承诺 + status page | 2026-06-25 | main | v1.0.5 |
| v1.0.7 | **Payments billing** | 中国发票 + refund / chargeback + quota dashboard for user + DSAR export | 2026-07-02 | main | v1.0.6 |
| v1.0.8 | **Legal compliance** | ToS / Privacy Policy / DPA 律师审 + ICP 备案启动 + enterprise 合同模板 | 2026-07-10 | main | v1.0.7 |
| v1.0.9 | **i18n** | wiki 双语 + 货币 / 时区抽象 + UI 中英切换 e2e | 2026-07-15 | main | v1.0.8 |
| v1.0.10 | **Plugin marketplace** | pluginhub test cov 24% → 80% + 第三方 plugin 发布流程 + 审查机制 | 2026-07-22 | main | v1.0.9 |
| v1.0.11 | **Release engineering** | APT/RPM repo GH Pages + WinGet manifest 提交 microsoft/winget-pkgs + homebrew tap + scoop / aur | 2026-07-30 | main | v1.0.10 |
| **v1.1.0** | **VLM + defamation v3** | VLM provider OpenAI vision + defamation v3 cloud verify(F1 ≥ 0.85)+ ≥1 个 new connector(待定) | 2026-08-15 | main | v1.0.11 |

**总周期**:5/25 → 8/15 = 12 个 minor / 82 天(约 6.8 天/minor 平均)

**强配对约束**:
- attune ↔ attune-pro 同号同步打 tag(v1.0.0/v1.0.1/.../v1.1.0)
- cloud-v2.x 跟随但**独立编号**(v1.0.2 → cloud-v2.3.0;v1.0.5 → cloud-v2.4.0;v1.1.0 → cloud-v3.0.0)
- pluginhub 跟随 plugin 自身 SemVer(per § 强配对 §5)

## 附录 B:Task tracker 链

```
#35 v1.0.0 GA(5/25)
  └→ blockedBy: #162(CI fix)
#36 v1.0.1 升级策略(5/28)
  └→ blockedBy: #35(v1.0.0)
#37 v1.0.2 DB rename + support(5/31)
  └→ blockedBy: #36
#38 v1.0.3 Observability(6/05)
  └→ blockedBy: #37
... (12 个 task 串行)
```

**并行机会**(per § 并行开发):
- v1.0.4 / v1.0.5 / v1.0.6 三者**无真依赖**(security / perf / DR 独立 worktree)→ 6/12-6/25 可并行 3 agent
- v1.0.7 / v1.0.8(payments / legal)**无技术依赖**但有商务依赖(发票需法律意见)→ 串行
- v1.0.10 / v1.0.11 / v1.1.0 末段(marketplace / release / VLM)**可三并行**

**Critical path**:v1.0.0 → v1.0.1 → v1.0.2 → v1.0.3(observability foundation)
**Soft path**:v1.0.4-v1.1.0(可并行优化)
