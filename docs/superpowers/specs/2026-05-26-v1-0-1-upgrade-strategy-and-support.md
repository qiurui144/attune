# v1.0.1 升级策略 + Support Surface SSOT (Spec)

> **触发**:2026-05-25 v1.0.0 GA tag push 后,RC Gate 4 Known Limitations 记入「升级策略
> SSOT 缺失 / Tauri auto-updater latest.json 私钥未生成 / Pre-upgrade vault backup 无
> / rollback 无 path / `.github/ISSUE_TEMPLATE` 缺 / DSAR 法定义务无 endpoint」。本 spec
> 把这些 P0 闭环到 v1.0.1 (5/26-28 落地)。
>
> **范围**:仅 spec,不实施代码。implementation plan 见
> `docs/superpowers/plans/2026-05-26-v1-0-1-upgrade-strategy.md`。
>
> **上位 SSOT**:`docs/superpowers/specs/2026-05-25-v1-0-ga-and-v1-0-x-gap-closure-roadmap.md`
> 附录 A v1.0.1 行展开。

## 目录

- [1. 目标定位](#1-目标定位)
- [2. 范围边界](#2-范围边界)
- [3. 架构数据流](#3-架构数据流)
- [4. 模块边界](#4-模块边界)
- [5. API 契约](#5-api-契约)
- [6. 扩展点 / 插件接口](#6-扩展点--插件接口)
- [7. 错误处理 + 边界 case](#7-错误处理--边界-case)
- [8. 成本契约](#8-成本契约)
- [9. 测试矩阵](#9-测试矩阵)
- [10. 向后兼容](#10-向后兼容)
- [11. 风险登记](#11-风险登记)
- [附录 A:文件清单与命名](#附录-a文件清单与命名)
- [附录 B:user 1-time 动作清单](#附录-buser-1-time-动作清单)

---

## 1. 目标定位

### 用户痛点(GA 后 5/26-28 之内必须闭环)

| # | 痛点 | 当前状态 | v1.0.1 后状态 |
|---|------|----------|---------------|
| P0-1 | **Tauri auto-updater silent fail** — 启动每 N 小时 GET latest.json 但 release 流水线**从不生成 latest.json**;updater 永远 404 → 用户永远收不到升级通知 | tauri.conf.json:54 endpoint OK / pubkey 占位 | publish-latest-json.yml workflow 自动生成 + minisign sign + push 到 release;真访问 200 |
| P0-2 | **Pre-upgrade vault backup 无 path** — 升级失败 → vault.db schema dirty / migration 半途 → user 数据丢失 | 无 backup hook | `attune --pre-upgrade-backup` CLI + 升级前自动 hook |
| P0-3 | **Rollback 无 path** — install pkg 升上去后 panic / 启动失败,user 只能 GitHub 找老版手装,vault 仍 dirty | 无 rollback CLI | `attune --rollback [version]` 子命令 + ROLLBACK.md playbook |
| P0-4 | **Active version notification 无** — server 不知道 GitHub 最新版,UI 无法显式 surface "有新版本" | 仅靠 Tauri updater silent check | `GET /api/v1/version` endpoint:current / latest / upgrade_available |
| P0-5 | **`.github/ISSUE_TEMPLATE` 缺** — 5/26 上架后 community 提 issue 没 template → repro 信息缺(OS / vault size / 版本 / 复现步骤)→ triage 成本高 | 仓内无 .github/ISSUE_TEMPLATE/ 目录 | 3 yaml template:bug_report / feature_request / question + PR template |
| P0-6 | **DSAR 法定义务缺** — GDPR Art 15/17 / 个保法第 45 条 user 有 export / delete 权;cloud accounts 仓无对应 endpoint → 5/26 上架后任一 user 法定请求 = compliance hot incident | accounts 仓只有 register/login/quota | `GET /me/export` JSON dump + `DELETE /me`(soft delete + 30 天 hard delete cron) |

### 产品 positioning 对齐

- **隐私 / 本地优先**:升级数据流必须保 "user vault 0 丢失";pre-upgrade backup 是本规则的物理 SSOT 落地
- **混合智能**:本 spec 全部新增 endpoint 都不涉及 LLM call(零成本路径 per § 成本契约)
- **分层成本**:auto-updater 走 GH release(public repo 流量 free);DSAR JSON dump 是本地 SQLite export(零外部 API)
- **私有 AI 知识伙伴**:rollback playbook 必须假定 user 不能上 GitHub(局域网部署 / K3 一体机 form factor),所以需 **离线 rollback path**(预 cache 老 install pkg)

---

## 2. 范围边界

### v1.0.1 做什么

1. **`docs/UPGRADING.md` SSOT** — Tauri 升级机制说明 + 每 minor 升级节(后续 v1.0.2+ 追加新节,不分裂文件)
2. **`docs/ROLLBACK.md` playbook** — 升级失败排查 + rollback CLI 用法 + 离线 rollback path(预 cache 老 install pkg 流程)
3. **`docs/SUPPORT.md` placeholder** — 留 SLA 分级位(v1.0.2 填齐 P0/P1/P2/P3 数值);v1.0.1 只占位,避免上架 5/26 issue 涌入时 user 找不到"如何报 bug"入口
4. **`.github/ISSUE_TEMPLATE/`** — bug_report.yaml / feature_request.yaml / question.yaml(3 yaml,与 attune-pro 风格统一)
5. **`.github/PULL_REQUEST_TEMPLATE.md`** — 与现有 CONTRIBUTING.md / DEVELOP.md 对齐
6. **`GET /api/v1/version`** endpoint(`attune-server` route)— 本地知道当前 + GitHub 最新版
7. **`attune --rollback [version]` + `attune --pre-upgrade-backup`** CLI 子命令(`attune-server` bin 入口扩展)
8. **DSAR endpoints**(`accounts` 仓,跨仓 work):
   - `GET /api/v1/users/me/export` — JSON dump(user profile + quota + chat history meta + 不含 vault content,vault 在本地不在 cloud)
   - `DELETE /api/v1/users/me` — soft delete(`is_deleted = true` + `deleted_at = now`)+ 30 天后 cron hard delete
9. **`.github/workflows/publish-latest-json.yml`** — desktop-vX.Y.Z tag 触发 → 自动生成 latest.json + minisign sign(用 GH secret `TAURI_PRIVATE_KEY`)+ push 到 release assets
10. **tauri.conf.json `pubkey` 字段 user 实生成 key 后替换占位** — user 1-time 动作(详见附录 B)

### v1.0.1 不做什么(推后)

- ❌ **zero-downtime cloud upgrade 蓝绿** — 推 v1.0.2(per roadmap 附录 A)
- ❌ **vault schema_version 字段 + alembic migration** — 推 v1.0.2(v1.0.0 vault schema = v1.0.1 vault schema,GA 周期内不动)
- ❌ **observability metrics / loki / alert** — 推 v1.0.3
- ❌ **security pen test 外包** — 推 v1.0.4
- ❌ **VLM provider / defamation v3 cloud verify** — 推 v1.1.0
- ❌ **macOS .dmg / aarch64 Linux** — per CLAUDE.md "暂不做"
- ❌ **plugin auto-update** — 推 v1.0.10(plugin marketplace 节)

### v1.0.2+ 才做(避免范围漂移)

- DB rename(lawcontrol → attune_enterprise)alembic migration
- DSAR 实测 user 法定请求满足 ≤ 30 天 turnaround SLA 数值化
- SLA 分级 P0/P1/P2/P3 真定义 + status page

---

## 3. 架构数据流

### 3.1 升级正向 happy path

```
[user 装 v1.0.0]
    │
    │ Tauri auto-updater 启动 hook(N=4 小时一次 silent check)
    ↓
GET https://github.com/qiurui144/attune/releases/latest/download/latest.json
    │
    ├─ 200 latest.json {version:"1.0.1", signature, url, pub_date}
    │       ↓
    │   minisign verify(pubkey 从 tauri.conf.json:58 编译时嵌入)
    │       │
    │       ├─ verify OK
    │       │     ↓
    │       │   UI dialog "v1.0.1 available, upgrade now?"(dialog=false 模式下需 webview 自渲染,见 5.3)
    │       │     ↓
    │       │   user click "Upgrade"
    │       │     ↓
    │       │   attune-server 内调用 `attune --pre-upgrade-backup`
    │       │     ↓
    │       │   ~/.local/share/Attune/backups/vault.db.bak.20260527-1030 生成
    │       │     ↓
    │       │   Tauri 下载 .deb / .msi → verify signature 再次 → installer 启动
    │       │     ↓
    │       │   新版 v1.0.1 启动 → vault.db open(同 schema 兼容)
    │       │     ↓
    │       │   [v1.0.1 live]
    │       │
    │       └─ verify FAIL(signature 不对 / pubkey 不对)
    │             ↓
    │           silent abort + log "Update integrity check failed" + UI 不弹窗
    │           (避免用户被钓鱼 latest.json)
    │
    └─ 404 / network fail
            ↓
        silent retry next session(non-fatal)
```

### 3.2 Rollback 数据流(任一升级失败)

```
[user 升 v1.0.1 后 app 不启动 / 启动后 panic]
    │
    │ user 跑 `attune --rollback`(从 terminal,因 GUI 起不来)
    ↓
[rollback CLI]
    │
    ↓
扫 ~/.local/share/Attune/backups/ 列所有 vault.db.bak.YYYYMMDD-HHMM
    │
    ↓
显式提示 "Found backups: 20260527-1030 (pre v1.0.1 upgrade) / 20260520-0900 (pre v1.0.0 upgrade)"
    │
    ↓
user 选要 rollback 到哪个 (e.g. 20260527-1030)
    │
    ↓
prompt "This will: (1) restore vault.db (2) downgrade app via apt/dpkg/msi rollback. Continue? [y/N]"
    │
    ↓
[(1) restore vault.db]
~/.local/share/Attune/vault.db ← cp ~/.local/share/Attune/backups/vault.db.bak.20260527-1030
    │
    ↓
[(2) downgrade install pkg]
case OS:
  linux deb → sudo dpkg -i ~/.cache/attune/old-pkgs/attune_1.0.0_amd64.deb (预 cache,见 ROLLBACK.md)
            或 sudo apt install attune=1.0.0 (若 apt-rpm-repo 还 host 老版)
  linux rpm → sudo rpm -U --oldpackage ~/.cache/attune/old-pkgs/attune-1.0.0.rpm
  windows msi → msiexec /uninstall {GUID} && msiexec /i 老 msi
  windows nsis → uninstaller.exe /S && 老 .exe /S /VERYSILENT
    │
    ↓
user 启 attune → v1.0.0 起来 + vault 完整
```

### 3.3 Active version notification 数据流

```
[user 打开 attune Web UI / Settings → About 页]
    ↓
fetch GET /api/v1/version
    ↓
[attune-server::routes::version::get]
    │
    │ current = env!("CARGO_PKG_VERSION")(编译时嵌入,不依赖运行时 query)
    │ latest = cached_or_fetch_github(每 6 小时 cache,避免 GH API rate limit)
    │           ↓
    │       GET https://api.github.com/repos/qiurui144/attune/releases/latest
    │           ↓
    │       parse tag_name → latest_version
    │
    ↓
return {
  "current": "1.0.0",
  "latest_available": "1.0.1",
  "upgrade_available": true,
  "upgrade_url": "https://github.com/qiurui144/attune/releases/tag/v1.0.1",
  "breaking_changes": false,
  "rollback_supported": true
}
    ↓
UI Settings/About 显示 "v1.0.0 (latest v1.0.1 available) [Upgrade] [Read changelog]"
```

### 3.4 DSAR JSON export 数据流(accounts 仓)

```
[user Settings → Privacy → Export My Data]
    ↓
GET /api/v1/users/me/export (Bearer JWT)
    ↓
[accounts::routes::dsar::export_me]
    │
    │ SELECT profile + quota + chat_history_metadata + audit_log
    │ FROM users / quotas / chat_logs / audit_events
    │ WHERE user_id = jwt.sub
    │
    ↓
return application/json:
{
  "schema_version": "dsar-1.0",
  "exported_at": "2026-05-28T10:00:00Z",
  "user_id": "u-xxx",
  "profile": { "email": "...", "created_at": "...", ... },
  "quota": { "monthly_tokens_used": ..., "plan": "free" },
  "chat_history_metadata": [ { "session_id": ..., "started_at": ..., "title": ..., "model": ... } ],
  "audit_log": [ ... last 90 days ... ]
}
    ↓
UI 下载为 attune-dsar-export-{user_id}-{date}.json
```

注:vault content / 文档 / 批注 **不在 cloud 端** (per § 三产品矩阵 + 边界 "数据完全隔离");
DSAR JSON 只覆盖 cloud 持有的 profile + quota + chat meta;vault 本身的 export 走 attune
desktop 已有的 `attune --export-vault`(v0.7+ 已实装,本 spec 不重复)。

### 3.5 DSAR delete 数据流

```
[user Settings → Privacy → Delete My Account]
    ↓
warning dialog (强制 user 输 "DELETE my account" 验证字符串)
    ↓
DELETE /api/v1/users/me (Bearer JWT)
    ↓
[accounts::routes::dsar::delete_me]
    │
    │ UPDATE users SET is_deleted = true, deleted_at = NOW(), deletion_scheduled_for = NOW() + 30 days
    │ WHERE user_id = jwt.sub
    │
    │ jwt 立即 invalidate (push to deny-list)
    │
    ↓
return 202 Accepted:
{
  "status": "scheduled_for_deletion",
  "scheduled_hard_delete_at": "2026-06-27T10:00:00Z",
  "restoration_window_days": 30,
  "restore_email_sent_to": "u***@gmail.com"
}
    ↓
[accounts::cron::hard_delete_expired_users] (每日 02:00 跑)
    │
    │ SELECT user_id WHERE deletion_scheduled_for < NOW()
    │
    ↓
真删:DELETE FROM users / quotas / chat_logs / audit_events WHERE user_id IN (...)
日志:audit "dsar_hard_delete" event 保留(合规要求保留删除事件元信息)
```

---

## 4. 模块边界

| 仓 | 路径 | 改动 | 行数估计 |
|----|------|------|---------|
| **attune** | `docs/UPGRADING.md` | NEW(白名单允许 — per CLAUDE.md docs/ 白名单 `<feature-area>.md` 单一主题) | ~150 |
| **attune** | `docs/ROLLBACK.md` | NEW | ~120 |
| **attune** | `docs/SUPPORT.md` | NEW placeholder | ~30(v1.0.2 扩到 ~200)|
| **attune** | `.github/ISSUE_TEMPLATE/bug_report.yaml` | NEW | ~50 |
| **attune** | `.github/ISSUE_TEMPLATE/feature_request.yaml` | NEW | ~30 |
| **attune** | `.github/ISSUE_TEMPLATE/question.yaml` | NEW | ~25 |
| **attune** | `.github/ISSUE_TEMPLATE/config.yml` | NEW(`contact_links`)| ~15 |
| **attune** | `.github/PULL_REQUEST_TEMPLATE.md` | NEW | ~60 |
| **attune** | `.github/workflows/publish-latest-json.yml` | NEW | ~80 |
| **attune** | `rust/crates/attune-server/src/routes/version.rs` | NEW route module | ~120 |
| **attune** | `rust/crates/attune-server/src/routes/mod.rs` | + `pub mod version;` + router 注册 | +2 |
| **attune** | `rust/crates/attune-server/src/main.rs` | + `--rollback` + `--pre-upgrade-backup` CLI 子命令(clap subcommand) | +80 |
| **attune** | `rust/crates/attune-core/src/backup.rs` | NEW(vault.db backup logic — 用 SQLite `VACUUM INTO`,原子,无需停服)| ~150 |
| **attune** | `rust/crates/attune-core/src/rollback.rs` | NEW(rollback 扫描 + 选择 logic,**不**做 install pkg downgrade — 那部分走 ROLLBACK.md 引导 user 手动)| ~100 |
| **attune** | `apps/attune-desktop/tauri.conf.json` | `pubkey` 字段 user 替换占位(见附录 B) | -1/+1 |
| **attune** | `rust/crates/attune-server/tests/version_route.rs` | NEW integration test | ~120 |
| **attune** | `rust/crates/attune-core/tests/backup_rollback.rs` | NEW integration test | ~180 |
| **attune** | `tests/MANUAL_TEST_CHECKLIST.md` | + 升级 / rollback / DSAR 节 | +50 |
| **attune** | `RELEASE.md` | + v1.0.1 节(Highlights / Migration / Known Limits) | +60 |
| **cloud(accounts)** | `accounts/src/routes/dsar.rs` | NEW(`/me/export` + `/me` DELETE) | ~200 |
| **cloud(accounts)** | `accounts/src/cron/hard_delete.rs` | NEW(每日 02:00 hard delete cron) | ~80 |
| **cloud(accounts)** | `accounts/tests/dsar_integration.rs` | NEW | ~150 |
| **cloud** | `docs/RELEASE.md` 或 `cloud/CHANGELOG.md` | + v2.2.1 节(DSAR endpoints) | +40 |

**跨仓边界**:attune 端 P0-1/2/3/4/5 + cloud(accounts)端 P0-6。两仓改动可并行,merge 顺序不强依赖。

**不动**:
- 任何 `attune-pro/plugins/*-pro/` 代码(plugin 不参与本 spec)
- 任何 LLM provider / OllamaProvider / VLM(per § 范围边界 推 v1.1)
- 任何 vault.db schema(per § 范围边界 推 v1.0.2)

---

## 5. API 契约

### 5.1 `GET /api/v1/version`(attune-server)

```http
GET /api/v1/version HTTP/1.1
Authorization: (optional; 无 auth 也可访问 — 公开信息)
```

Response 200:

```json
{
  "current": "1.0.0",
  "latest_available": "1.0.1",
  "upgrade_available": true,
  "upgrade_url": "https://github.com/qiurui144/attune/releases/tag/v1.0.1",
  "breaking_changes": false,
  "rollback_supported": true,
  "checked_at": "2026-05-28T10:00:00Z"
}
```

Response 200(无网络 / 6 小时内 cache 未过期):

```json
{
  "current": "1.0.0",
  "latest_available": null,
  "upgrade_available": false,
  "rollback_supported": true,
  "error": "github_api_unreachable",
  "checked_at": "2026-05-28T04:00:00Z"
}
```

**契约保证**:
- `current` 永远存在(编译时常量,不依赖网络)
- `latest_available` 可为 null(网络 fail)
- `upgrade_available` 为 boolean,网络 fail 时为 false
- `rollback_supported` 永远 true(v1.0.1+)
- 不需要 auth — 版本号是公开信息

### 5.2 CLI 子命令

```bash
# 列可用 backup
attune --rollback
# Output:
# Available backups (newest first):
#   1. 20260527-1030  vault.db.bak (3.2 MB)  Created before v1.0.1 upgrade
#   2. 20260520-0900  vault.db.bak (3.1 MB)  Created before v1.0.0 upgrade
# Run: attune --rollback <number> to restore

attune --rollback 1
# Output:
# Restoring vault.db from 20260527-1030 ...
# [✓] vault.db restored (3.2 MB)
# Next step: downgrade install pkg. See docs/ROLLBACK.md §3 for OS-specific instructions.
# Exit code: 0

# 强制 pre-upgrade backup(用户手动 / installer hook)
attune --pre-upgrade-backup
# Output:
# Creating pre-upgrade backup ...
# [✓] vault.db.bak.20260528-1500 created (3.3 MB)
# Exit code: 0
```

### 5.3 latest.json schema(Tauri auto-updater 规范)

```json
{
  "version": "1.0.1",
  "notes": "v1.0.1 minor update — see CHANGELOG: ...",
  "pub_date": "2026-05-28T10:00:00.000Z",
  "platforms": {
    "linux-x86_64": {
      "signature": "untrusted comment: signature from minisign secret key\nRWRwRyVK9XisxxxxxxxxxxxxxxxBASE64xxxxxxxxxxxxxxx=\ntrusted comment: timestamp:1716894000\nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx==",
      "url": "https://github.com/qiurui144/attune/releases/download/desktop-v1.0.1/Attune_1.0.1_amd64.deb"
    },
    "windows-x86_64": {
      "signature": "...",
      "url": "https://github.com/qiurui144/attune/releases/download/desktop-v1.0.1/Attune_1.0.1_x64-setup.exe"
    }
  }
}
```

**publish-latest-json.yml workflow 自动生成**:tag `desktop-v*` 触发,从 release assets 读 .deb / .exe URL + 各自 minisign sign 文件,组装 JSON 后 upload 为 release asset `latest.json`。

### 5.4 DSAR endpoints(accounts 仓)

```http
GET /api/v1/users/me/export HTTP/1.1
Authorization: Bearer <jwt>
```

Response 200 application/json(见 §3.4)

```http
DELETE /api/v1/users/me HTTP/1.1
Authorization: Bearer <jwt>
Content-Type: application/json

{
  "confirmation": "DELETE my account"
}
```

Response 202(见 §3.5)

错误响应:

| code | scenario | http |
|------|----------|------|
| `confirmation_mismatch` | `confirmation` 字段 ≠ "DELETE my account" | 400 |
| `already_scheduled` | 已 schedule for deletion,30 天窗口内 | 409 |
| `unauthorized` | jwt invalid | 401 |

---

## 6. 扩展点 / 插件接口

### 6.1 后续 minor 加新升级路径

**SSOT 不分裂**:`docs/UPGRADING.md` 加新 section,例如:

```markdown
## v1.0.1 → v1.0.2 升级(2026-05-31)

**新增**:vault.db schema_version 字段
**自动 migration**:启动时 attune-server 检测 schema_version,自动跑 ALTER TABLE
**rollback compat**:v1.0.2 vault → v1.0.1 兼容(schema 新增字段 nullable)

## v1.0.2 → v1.0.3 升级(2026-06-05)

**新增**:Prometheus metrics endpoint (opt-in)
...
```

**禁止**:`docs/UPGRADING-v1-0-2.md` 单独文件(per CLAUDE.md § 文档体系铁律 — "同主题最多 1 份")。

### 6.2 后续 minor 加新 DSAR 字段

`accounts/src/routes/dsar.rs::export_me` 返回 JSON 加新字段时:
- 增 `schema_version`:从 "dsar-1.0" → "dsar-1.1"
- 保持向后兼容:新字段必须 nullable / 默认值
- 在 `docs/UPGRADING.md` DSAR 节附 schema diff

### 6.3 后续 minor 加新 CLI 子命令

`rust/crates/attune-server/src/main.rs` 用 `clap` Subcommand enum 扩展:

```rust
#[derive(Subcommand)]
enum Cli {
    Rollback { version: Option<String> },
    PreUpgradeBackup,
    // future v1.0.x:
    // ExportLogs,        // v1.0.3 observability
    // RotateSecrets,     // v1.0.4 security
}
```

---

## 7. 错误处理 + 边界 case

### 7.1 Exit codes

| exit code | scenario | 用户路径 |
|-----------|----------|----------|
| 0 | 正常完成 | — |
| 10 | latest.json fetch fail (network) | non-fatal,UI silent retry next session |
| 11 | latest.json signature verify fail | UI 报 "Update integrity check failed — possible tampering";**不安装** |
| 12 | vault.db backup fail (disk full / IO error) | UI 报 "Backup failed: insufficient disk space (need ≥ 100MB free)";abort upgrade |
| 13 | vault.db backup fail (db locked) | UI 报 "Close all Attune instances and retry" |
| 14 | rollback: no backups found | CLI 输出 "No backups available — restore manually from cloud sync (if enabled)" |
| 15 | rollback: backup file corrupted | CLI 输出 "Backup file corrupted (SHA256 mismatch) — try older backup" |
| 16 | rollback: vault.db restore fail (target locked) | CLI 输出 "Close all Attune processes first (kill `attune-server`)" |
| 20 | DSAR export: db query fail | 500 + retry |
| 21 | DSAR delete: confirmation mismatch | 400 client error |
| 22 | DSAR delete: already scheduled | 409 |

### 7.2 边界 case 矩阵

| case | 处理 |
|------|------|
| latest.json 字段缺失(version 缺) | 视为 verify fail,silent abort |
| latest.json `version` 比 current 老(降级) | abort,日志 warn,UI 不弹窗 |
| latest.json `version` == current | abort,no-op |
| disk full(< 100MB)before backup | exit 12 + UI 引导清理 |
| user vault.db = 0 字节(corrupt) | backup 仍跑(`VACUUM INTO` 会 fail,exit 13) |
| user 同时跑 2 个 attune CLI(同 vault) | rollback CLI 检测 sqlite 锁 → exit 16 |
| user 在 30 天 deletion 窗口内 login → restore | accounts API 自动 restore(`is_deleted=false`)+ 邮件确认 |
| 离线 user(企业内网 / 无 internet) | `/api/v1/version` 返回 `error: "github_api_unreachable"`;升级走 user 手动 download install pkg + rollback playbook |
| K3 riscv64 form factor | **不**走 Tauri auto-updater(无 .deb / .msi build for riscv64,K3 走镜像化部署);ROLLBACK.md 单独有 "K3 一体机升级 / rollback"节 |
| 极小 vault(< 1 KB) | `VACUUM INTO` 仍跑(SQLite 标准行为),正常 |
| 极大 vault(> 10 GB) | backup 可能耗时 > 30s;UI 加进度条;不 timeout abort |

### 7.3 Adversarial(per § 测试方案规范)

| 攻击 | 防御 |
|------|------|
| 篡改 latest.json url 指向恶意 .deb | minisign verify fail → exit 11 |
| 重放老 latest.json(版本号回退) | check `version > current`,否则 abort |
| DDoS GH API rate limit attune-server | 6 小时 cache + `If-None-Match` ETag |
| DSAR export 大 user 数据 dump 滥用(大 user 反复 export 撑爆 bandwidth) | rate limit per user:1 export / hour;`Retry-After` header |
| DSAR delete 误操作 | confirmation 字段强制输 "DELETE my account" 字面量 |
| 钓鱼 latest.json(MITM) | https + minisign sig 双层防御 |
| 私钥泄露(GH secret 被 leak) | rotation playbook v1.0.4(本 spec 只 surface 风险,实施推后) |

---

## 8. 成本契约

| 操作 | 层级 | 谁买单 | UI 显示 |
|------|------|--------|---------|
| Tauri auto-updater check latest.json | 🆓 零成本 | GH(public repo free) | UI 不显式 |
| publish-latest-json.yml workflow | 🆓 零成本 | GH Actions free runner | — |
| `/api/v1/version` cache hit | 🆓 零成本 | 本地 | — |
| `/api/v1/version` cache miss → GH API | 🆓 零成本(< 60 req/hour GH unauthenticated quota / cache 6h)| GH | — |
| `attune --pre-upgrade-backup` | ⚡ 本地算力(SQLite VACUUM INTO,~1s/100MB) | 本地 disk | CLI 进度条 |
| `attune --rollback` | 🆓 零成本(纯 file ops) | 本地 disk | CLI 输出 |
| DSAR export(JSON dump) | ⚡ 本地算力(cloud DB query,< 100KB 数据) | cloud DB | UI 下载进度 |
| DSAR delete(soft → cron hard) | 🆓 零成本(DB ops) | cloud DB | UI 即时返回 |

**没有任何 LLM call 引入本 spec**(per § 范围边界 推 v1.1)。

---

## 9. 测试矩阵

per § 测试方案规范 8 场景覆盖。每个 case 必须有 `tests/<...>.rs` 实测,**禁止只跑 happy path**。

### 9.1 happy path

| # | 场景 | test 文件 | gate |
|---|------|-----------|------|
| H1 | latest.json fetch → verify → install pkg → vault 保留 | `tests/upgrade_happy_path.rs`(本地 mock GH server) | E2E |
| H2 | `attune --pre-upgrade-backup` 跑成功 + 3 个 backup 文件存 | `tests/backup_rollback.rs` | integration |
| H3 | `attune --rollback 1` 恢复 vault | `tests/backup_rollback.rs` | integration |
| H4 | `GET /api/v1/version` 返回正确 schema | `tests/version_route.rs` | route |
| H5 | DSAR export 返回 JSON 含所有 user 字段 | `accounts/tests/dsar_integration.rs` | integration |
| H6 | DSAR delete → 30 天后 cron 真 hard delete | `accounts/tests/dsar_integration.rs`(mock time) | cron |

### 9.2 edge case

| # | 场景 | test 文件 |
|---|------|-----------|
| E1 | latest.json 缺 `version` 字段 | `tests/upgrade_edge_cases.rs` |
| E2 | latest.json `version` 比 current 老 | 同上 |
| E3 | vault.db = 0 字节 | `tests/backup_rollback.rs::test_empty_vault_backup` |
| E4 | vault.db > 1 GB 大文件 backup | 同上,marked `#[ignore]` slow lane |
| E5 | 同时 2 个 attune-server 跑(sqlite lock) | `tests/backup_rollback.rs::test_concurrent_lock` |
| E6 | rollback 选 backup index 超界(`attune --rollback 99`) | CLI subprocess test |
| E7 | DSAR user 30 天窗口内 re-login → restore | `accounts/tests/dsar_integration.rs` |
| E8 | user 极小 / 极大数据 export | `accounts/tests/dsar_integration.rs` |

### 9.3 error case

| # | 场景 | test 文件 |
|---|------|-----------|
| Er1 | latest.json signature fail | `tests/upgrade_edge_cases.rs::test_sig_fail_exit_11` |
| Er2 | disk full during backup | `tests/backup_rollback.rs::test_disk_full_exit_12` |
| Er3 | network fail → GH API unreachable | `tests/version_route.rs::test_offline` |
| Er4 | DSAR export rate limit(2nd within 1h) | `accounts/tests/dsar_integration.rs::test_rate_limit` |
| Er5 | DSAR delete confirmation mismatch | `accounts/tests/dsar_integration.rs::test_confirmation_400` |
| Er6 | DSAR delete already scheduled | `accounts/tests/dsar_integration.rs::test_already_409` |

### 9.4 adversarial

| # | 场景 | test 文件 |
|---|------|-----------|
| A1 | 篡改 latest.json url 指向不同 .deb | `tests/upgrade_edge_cases.rs::test_tampered_url_exit_11` |
| A2 | 重放老 latest.json | 同上 |
| A3 | corrupt backup file SHA256 不对 | `tests/backup_rollback.rs::test_corrupt_backup` |
| A4 | DSAR export 滥用(rate limit 测) | E2 same |

### 9.5 多并发

| # | 场景 | test 文件 |
|---|------|-----------|
| C1 | 100 user 同时 DSAR export | `accounts/tests/dsar_load.rs`(criterion benchmark) |
| C2 | 同时多 attune client 检查 version(同 GH API rate limit 行为) | `tests/version_route.rs::test_concurrent_check` |

### 9.6 资源耗尽

| # | 场景 | test 文件 |
|---|------|-----------|
| R1 | disk full | Er2 same |
| R2 | network 断 | Er3 same |
| R3 | sqlite memory map exhausted(极大 vault) | `tests/backup_rollback.rs::test_huge_vault`(`#[ignore]` slow) |

### 9.7 国际化

| # | 场景 | test 文件 |
|---|------|-----------|
| I1 | 升级 dialog 文案有 zh + en 双 i18n key | `apps/attune-desktop/src/ui/UpgradeDialog.spec.ts`(vitest)|
| I2 | UPGRADING.md 中英文 — 决策:**只 zh**,per CLAUDE.md § 文档体系铁律 双语副本只 README | 文档 review |
| I3 | latest.json `notes` 字段 zh + en bilingual fallback | publish-latest-json.yml 双 changelog 拼接 |

### 9.8 降级(rollback playbook)

| # | 场景 | test 文件 |
|---|------|-----------|
| D1 | LLM gateway 5 provider 全 fail → /version 仍能跑(不依赖 LLM) | `tests/version_route.rs::test_no_llm_dependency` |
| D2 | rollback playbook E2E:v1.0.1 装 → rollback CLI → 老 .deb 装 → v1.0.0 起 | `tests/MANUAL_TEST_CHECKLIST.md` 节 + 真 GH Actions matrix(Linux x86_64) |

### 9.9 黑盒视角(user-first,per § Bug reproduce 第一步必须 user 视角)

**强制**:除上述代码 test,RELEASE 前必须有 user 视角 reproduce:

1. **真装 v1.0.0 .deb**(GH release artifact,**禁止** dev cargo build)
2. **真 fake v1.0.1 .deb** push 到 test release tag
3. **真等 Tauri 4 小时 hook**(或手动 trigger `__TAURI__.updater.checkUpdate()`)
4. **真 click "Upgrade"** → 看 backup 生成 → 看新版起来
5. **真模拟 panic**(v1.0.1 起来后 kill / crash)→ 真跑 `attune --rollback` → 真 dpkg downgrade
6. **真访问 cloud /me/export** → 真下载 JSON → 真 diff 字段
7. **真 DELETE /me** → 真等 30 天 cron(测试用 mock time)→ 真 verify hard delete

每步**截屏归 `docs/screenshots/v1-0-1-verification/`**,**禁止** 写仓库根目录。

---

## 10. 向后兼容

### 10.1 v1.0.0 → v1.0.1 数据兼容

- **vault.db schema**:不变(v1.0.2 才动 schema_version)
- **chat history JSON**:不变
- **plugin.yaml manifest**:不变
- **config TOML / YAML**:不变

**old client + new server**:OK(无 schema 变化)
**new client + old server**:OK(`/api/v1/version` 是 GET only,old server 没有 endpoint 时 client 收到 404 → UI fallback 显示 "v1.0.0 (version check unavailable)")

### 10.2 latest.json schema 演进

`latest.json` schema(v1.0.1 引入):

```json
{
  "version": "...",
  "notes": "...",
  "pub_date": "...",
  "platforms": { ... }
}
```

后续 minor 加字段必须**保持向后兼容**:
- 加新 platforms key(e.g. `macos-aarch64`)— 旧 client 忽略,正常
- 加新顶层字段(e.g. `mandatory: true`)— 旧 client 忽略 → 默认 false 行为
- **禁止**改 `version` / `signature` / `url` 字段语义

### 10.3 DSAR JSON schema 演进

`schema_version: "dsar-1.0"` 是显式版本,后续:
- dsar-1.1 加新字段 nullable → 兼容
- dsar-2.0 不兼容(structural rewrite) → 必须 v1.1.0 minor 才能改,**且** RELEASE.md surface as Breaking Change

### 10.4 CLI 子命令演进

新子命令(`--export-logs` / `--rotate-secrets`)加到 v1.0.3 / v1.0.4,与现有 `--rollback` / `--pre-upgrade-backup` **不冲突**,clap subcommand 是 enum 扩展兼容形式。

### 10.5 K3 一体机 form factor

K3 (riscv64) **不走** Tauri auto-updater(无 .deb / .msi build for riscv64 — per CLAUDE.md "K3 form factor riscv64 镜像化部署");K3 升级走**镜像化重装**:

- old image: attune-k3-v1.0.0.img
- new image: attune-k3-v1.0.1.img
- user dd-to-sd-card 重 flash + vault.db migrate via export/import

**ROLLBACK.md 单独有节** "K3 一体机升级与 rollback"。

---

## 11. 风险登记

| R | 风险 | 严重度 | 缓解 |
|---|------|--------|------|
| R1 | **minisign 私钥泄露**(GH secret 被 leak / user 误 commit 私钥到 git) | 🔴 critical | (a) 私钥**只**在 1 个 GH secret `TAURI_PRIVATE_KEY` 存 (b) 本仓 `.gitignore` 强加 `*.key` `minisign.key` `*.pem` (c) pre-commit hook 用 `trufflehog` 扫 (d) rotation playbook 在 v1.0.4 落地;v1.0.1 RELEASE.md 显式标"私钥 rotation 推 v1.0.4 — 高度谨慎托管" |
| R2 | **latest.json publish workflow 漏写** | 🟡 medium | publish-latest-json.yml CI 真上传后立即 `curl -I` verify 200;workflow fail blocks release tag promotion |
| R3 | **disk full during pre-upgrade backup** | 🟡 medium | 加 disk space precheck(< 100MB free → exit 12 不开始);UI 引导清理 |
| R4 | **rollback no backup**(user 从未升级 / 老 backup 被清理) | 🟡 medium | ROLLBACK.md surface "**离线 rollback** — 从 GitHub release 手动下载老版 install pkg 重装";CLI exit 14 引导 user |
| R5 | **Tauri auto-updater dialog 自渲染漏 / 国际化漏** | 🟡 medium | tauri.conf.json `"dialog": false` 配合 webview 自渲染 UpgradeDialog component;**强制**测 i18n zh/en 双语显示 |
| R6 | **DSAR delete 误操作(user 后悔)** | 🟡 medium | 30 天 restoration 窗口 + 立即邮件确认含 restore link;confirmation 字符串强制输 "DELETE my account" |
| R7 | **DSAR export bandwidth 撑爆** | 🟢 low | rate limit 1 export / hour / user;`Retry-After` header;大 user(> 100 MB)异步处理 + 邮件发链接 |
| R8 | **K3 riscv64 不走 auto-updater 用户困惑** | 🟢 low | UPGRADING.md / ROLLBACK.md K3 节明示"K3 form factor 走镜像 reflash"+ wiki 单独教程 |
| R9 | **publish-latest-json.yml 在 desktop-v* tag 触发但忘改 platforms key** | 🟡 medium | workflow 内 hardcoded platforms list,加 step 检查 release assets 都存在再 commit latest.json |
| R10 | **GH API rate limit(60 req/hour unauthenticated)击穿** | 🟢 low | 6 小时 cache + `If-None-Match` ETag;authenticated 5000/hour 可选 user 配 token(但默认不启)|

---

## 附录 A:文件清单与命名

按 commit 分组,与 implementation plan 1:1 对应。

### C1: docs/UPGRADING.md + docs/ROLLBACK.md + docs/SUPPORT.md placeholder

```
docs/UPGRADING.md            (~150 行)
docs/ROLLBACK.md             (~120 行)
docs/SUPPORT.md              (~30 行 placeholder)
```

### C2: GitHub templates

```
.github/ISSUE_TEMPLATE/bug_report.yaml         (~50 行)
.github/ISSUE_TEMPLATE/feature_request.yaml    (~30 行)
.github/ISSUE_TEMPLATE/question.yaml           (~25 行)
.github/ISSUE_TEMPLATE/config.yml              (~15 行)
.github/PULL_REQUEST_TEMPLATE.md               (~60 行)
```

### C3: `GET /api/v1/version` endpoint

```
rust/crates/attune-server/src/routes/version.rs           (~120 行)
rust/crates/attune-server/src/routes/mod.rs               (+2)
rust/crates/attune-server/tests/version_route.rs          (~120 行)
```

### C4: `attune --rollback` + `--pre-upgrade-backup` CLI + backup/rollback 模块

```
rust/crates/attune-server/src/main.rs                     (+80)
rust/crates/attune-core/src/backup.rs                     (~150 行)
rust/crates/attune-core/src/rollback.rs                   (~100 行)
rust/crates/attune-core/src/lib.rs                        (+2 mod 注册)
rust/crates/attune-core/tests/backup_rollback.rs          (~180 行)
```

### C5: DSAR endpoints(cloud accounts 仓)

```
[cross-repo] accounts/src/routes/dsar.rs                  (~200 行)
[cross-repo] accounts/src/cron/hard_delete.rs             (~80 行)
[cross-repo] accounts/tests/dsar_integration.rs           (~150 行)
[cross-repo] cloud docs RELEASE 节 v2.2.1                 (+40)
```

### C6: publish-latest-json.yml workflow

```
.github/workflows/publish-latest-json.yml                 (~80 行)
apps/attune-desktop/tauri.conf.json                       (-1/+1 pubkey 替换 — user 1-time)
```

### C7(可选 collapse 到 C6): RELEASE.md + MANUAL_TEST_CHECKLIST.md + tauri.conf.json pubkey

```
RELEASE.md                                                (+60 v1.0.1 节)
tests/MANUAL_TEST_CHECKLIST.md                            (+50)
```

---

## 附录 B:user 1-time 动作清单

**user 必须参与的 1-time 步骤**(spec 描述,user 在 v1.0.1 release 前手动跑一次):

### B.1 生成 minisign keypair

```bash
# 装 minisign(若未装)
sudo apt install minisign

# 生成 keypair
minisign -G -p ~/.config/attune/minisign.pub -s ~/.config/attune/minisign.key

# 设强密码保护私钥(强制)
# minisign 会 prompt 输密码 — 强密码 ≥ 20 字符
```

### B.2 私钥进 GH secret

1. cat ~/.config/attune/minisign.key | base64
2. GitHub repo qiurui144/attune → Settings → Secrets → Actions
3. New repository secret:
   - Name: `TAURI_PRIVATE_KEY`
   - Value: 粘贴 base64 结果
4. New repository secret:
   - Name: `TAURI_PRIVATE_KEY_PASSWORD`
   - Value: 上一步设的强密码

### B.3 公钥替换 tauri.conf.json:58 占位

```bash
# 读公钥 base64
cat ~/.config/attune/minisign.pub | base64 -w 0

# 替换 apps/attune-desktop/tauri.conf.json:58 `pubkey` 字段
# 注意:base64 含 minisign 文件头 "untrusted comment: ..." 必须整文件 base64,不只 key 部分
```

### B.4 私钥本地备份(critical)

```bash
# 私钥**只**存 2 处:
# 1. GH secret TAURI_PRIVATE_KEY(GitHub 端,生产用)
# 2. user 本地加密 backup(USB / 离线介质)

# 例:加密 backup 到离线 U 盘
gpg --symmetric --output minisign.key.gpg ~/.config/attune/minisign.key
# 把 minisign.key.gpg 拷到 USB,**销毁** ~/.config/attune/minisign.key 副本(可选 — 但若 user 想本地签也保留)

# 私钥**永不**:
# - commit 到 git(任一仓)
# - 上传到 cloud(Dropbox / iCloud / WebDAV)
# - 贴 chat / log / spec / commit msg
```

### B.5 verify workflow 跑通

1. user push **真**的 `desktop-v1.0.1-rc.1` tag(rc 先测,不直接打 GA)
2. 等 publish-latest-json.yml workflow 跑完
3. 访问 https://github.com/qiurui144/attune/releases/download/desktop-v1.0.1-rc.1/latest.json
4. 用公钥 `minisign -V -p ~/.config/attune/minisign.pub -m latest.json -x latest.json.minisig`
5. verify OK → workflow 验收;v1.0.1 正式 GA tag 可推

---

## 验收清单(per RC Gate 4 Known Limitations)

v1.0.1 ship 前必须:

- [ ] spec 11 节齐全(本文件)
- [ ] implementation plan 含 5-6 commit 拆解 + 风险登记 + 文件清单
- [ ] minisign keypair user 1-time 已生成 + GH secret 已配
- [ ] tauri.conf.json pubkey 占位已替换为真公钥
- [ ] publish-latest-json.yml workflow 跑 dry-run 通过
- [ ] `GET /api/v1/version` integration test 1.00 pass rate
- [ ] `attune --rollback` + `--pre-upgrade-backup` CLI subprocess test 1.00 pass
- [ ] DSAR `/me/export` + `/me` DELETE accounts 仓 integration test 1.00 pass
- [ ] hard_delete cron 30 天 mock time test pass
- [ ] 9.9 黑盒视角 user-first reproduce 全跑 + 截图归 `docs/screenshots/v1-0-1-verification/`
- [ ] RELEASE.md v1.0.1 节 Highlights / Migration / Known Limitations 齐全
- [ ] UPGRADING.md / ROLLBACK.md / SUPPORT.md 3 文档 link 进 README.md
- [ ] tests/MANUAL_TEST_CHECKLIST.md 升级 / rollback / DSAR 节添加

---

**spec 完。implementation plan 见同目录 `2026-05-26-v1-0-1-upgrade-strategy.md`。**
