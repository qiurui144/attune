# v1.0.1 升级策略 — Implementation Plan

> **配套 spec**:`docs/superpowers/specs/2026-05-26-v1-0-1-upgrade-strategy-and-support.md`
> **总周期**:5/26 → 5/28(3 天)
> **commit 数**:6(C1-C6)+ 可选 C7
> **跨仓**:attune 仓 + cloud(accounts)仓
> **user 1-time 动作**:1 次(minisign keypair 生成 + GH secret 配)

## 目录

- [日历(3 天 × 小时切片)](#日历3-天-小时切片)
- [Commit 拆解](#commit-拆解)
- [跨仓协调](#跨仓协调)
- [测试策略(TDD)](#测试策略tdd)
- [风险登记 + 缓解](#风险登记--缓解)
- [GA 验收清单](#ga-验收清单)
- [回头改 spec 触发条件](#回头改-spec-触发条件)

---

## 日历(3 天 × 小时切片)

### Day 1 — 2026-05-26 周一(文档 + GitHub templates + version endpoint)

| 时段 | 任务 | commit |
|------|------|--------|
| 09:00-10:30 | spec review + plan refresh + worktree 建 `feature/v1-0-1-upgrade-docs` | — |
| 10:30-12:30 | **C1**:UPGRADING.md + ROLLBACK.md + SUPPORT.md placeholder | C1 |
| 14:00-15:30 | **C2**:.github/ISSUE_TEMPLATE 3 yaml + config.yml + PR template | C2 |
| 15:30-18:00 | **C3 (TDD)**:write `tests/version_route.rs` red → impl `routes/version.rs` green → refactor;mod 注册 | C3 |
| 18:00-19:00 | push develop + PR review:#C1 / #C2 / #C3 自 review | — |

### Day 2 — 2026-05-27 周二(CLI 子命令 + backup/rollback 模块)

| 时段 | 任务 | commit |
|------|------|--------|
| 09:00-10:30 | spec §3.2 rollback 数据流 re-read + edge case 列表 | — |
| 10:30-13:00 | **C4 (TDD)**:write `attune-core/tests/backup_rollback.rs` 9 case red → impl `backup.rs` + `rollback.rs` green | C4 (impl) |
| 14:00-16:00 | **C4 续**:`attune-server/src/main.rs` clap Subcommand 加 `Rollback` + `PreUpgradeBackup` + 接 backup/rollback 模块 | C4 (cli) |
| 16:00-17:00 | manual smoke test(本机真跑 `attune --pre-upgrade-backup` + `attune --rollback`,vault.db 真备份真恢复)| — |
| 17:00-19:00 | push develop;cargo clippy + workspace test 全过 | — |

### Day 3 — 2026-05-28 周三(cloud DSAR + publish workflow + user 1-time + RELEASE)

| 时段 | 任务 | commit |
|------|------|--------|
| 09:00-09:30 | **user 跑 1-time**(per spec 附录 B):minisign keypair 生成 + GH secret 配 + tauri.conf.json pubkey 替换 | (user) |
| 09:30-11:30 | **C5 (跨仓,cloud accounts)** (TDD):write `accounts/tests/dsar_integration.rs` 8 case red → impl `routes/dsar.rs` + `cron/hard_delete.rs` green | C5 |
| 11:30-12:30 | cloud RELEASE 节 v2.2.1 + cloud 仓 push | — |
| 14:00-15:30 | **C6**:`.github/workflows/publish-latest-json.yml` workflow + tauri.conf.json pubkey 已替换 verify | C6 |
| 15:30-16:30 | workflow dry-run:推 `desktop-v1.0.1-rc.0` 测试 tag → 看 workflow 跑通 → 删 rc.0 tag | — |
| 16:30-18:00 | **C7**(可选 collapse):RELEASE.md v1.0.1 节 + MANUAL_TEST_CHECKLIST 节 | C7 |
| 18:00-19:00 | 全链 manual E2E(真装 v1.0.0 → 升 v1.0.1 → rollback → DSAR)+ 截图归 `docs/screenshots/v1-0-1-verification/` | — |
| 19:00-20:00 | develop → main `--no-ff` merge + tag `v1.0.1` + `desktop-v1.0.1` push + push develop | — |

---

## Commit 拆解

### C1: `docs(v1.0.1): UPGRADING + ROLLBACK + SUPPORT placeholder SSOT`

**文件**(per spec 附录 A):
- `docs/UPGRADING.md` (~150 行)
- `docs/ROLLBACK.md` (~120 行)
- `docs/SUPPORT.md` (~30 行 placeholder)

**内容大纲**:

**UPGRADING.md**:
- §1 自动升级(Tauri auto-updater 工作原理 + N=4h check)
- §2 手动升级(从 GH release 下载 install pkg)
- §3 升级前 backup(`attune --pre-upgrade-backup`)
- §4 升级失败 → §5 ROLLBACK.md
- §5 K3 一体机升级(镜像 reflash)
- §6 v1.0.0 → v1.0.1 升级节(本 minor 具体变化)
- §7 future minor section(占位,后续 v1.0.2+ 追加)

**ROLLBACK.md**:
- §1 何时需要 rollback(升级后 app panic / 启不来)
- §2 `attune --rollback` CLI 用法
- §3 OS-specific install pkg downgrade(deb / rpm / msi / nsis)
- §4 离线 rollback(无 internet,从 release 手动下老版)
- §5 K3 一体机 rollback(镜像 reflash 老 image)
- §6 vault.db corruption recovery(`.bak` 文件链)

**SUPPORT.md**(placeholder,v1.0.2 填齐):
- §1 报 bug(link `.github/ISSUE_TEMPLATE/bug_report.yaml`)
- §2 SLA(TBD — v1.0.2 填 P0/P1/P2/P3)
- §3 contact(email / community link)

**测试**:doc lint(markdownlint).

**review 重点**:link 进 README.md;无内部 broken link;符合 § 文档体系铁律白名单。

### C2: `chore(github): ISSUE_TEMPLATE 3 yaml + PR template`

**文件**:
- `.github/ISSUE_TEMPLATE/bug_report.yaml`
- `.github/ISSUE_TEMPLATE/feature_request.yaml`
- `.github/ISSUE_TEMPLATE/question.yaml`
- `.github/ISSUE_TEMPLATE/config.yml`(`contact_links` 指向 wiki / community)
- `.github/PULL_REQUEST_TEMPLATE.md`

**bug_report.yaml 必含字段**:
- OS(linux x86_64 / windows / K3 riscv64 / etc)
- attune version(`attune --version` 输出)
- vault size(MB)
- LLM provider(ollama local / Pro gateway / BYOK)
- reproduce steps(numbered)
- expected vs actual
- logs(`~/.local/share/Attune/logs/*.log` 节选)

**PULL_REQUEST_TEMPLATE.md**:
- linked issue / spec 引用
- changes summary
- testing(必填 — per § 代码变更后的强制流程)
- screenshots(UI 改动)
- breaking changes(yes / no)

**测试**:GitHub UI 手动 verify(`https://github.com/qiurui144/attune/issues/new/choose` 真显示 3 template).

### C3: `feat(server): GET /api/v1/version + active version notification`

**文件**:
- `rust/crates/attune-server/src/routes/version.rs` (~120 行)
- `rust/crates/attune-server/src/routes/mod.rs` (+2)
- `rust/crates/attune-server/tests/version_route.rs` (~120 行)

**实施(TDD)**:
1. **Red**:写 `tests/version_route.rs` 5 case(happy / offline / ETag cache / no auth / no LLM dep)
2. **Green**:impl `routes/version.rs::get_version` — `axum::routing::get` + `tokio::sync::Mutex<CachedVersion>` 6h TTL + reqwest GH API call
3. **Refactor**:抽 `fetch_latest_from_github()` 单独函数;cache key 用 ETag

**route 注册**:`routes/mod.rs` 加 `pub mod version;` + `app.route("/api/v1/version", get(version::get_version))`.

**关键约束**:
- 不依赖 LLM provider(per § 测试矩阵 D1)
- 6h cache 防 GH rate limit(60/h unauthenticated)
- offline 时 graceful return null

**测试**:5 case 1.00 pass;cargo clippy 干净.

### C4: `feat(core+server): attune --rollback + --pre-upgrade-backup CLI + backup/rollback 模块`

**文件**:
- `rust/crates/attune-core/src/backup.rs` (~150 行)
- `rust/crates/attune-core/src/rollback.rs` (~100 行)
- `rust/crates/attune-core/src/lib.rs` (+2 mod)
- `rust/crates/attune-server/src/main.rs` (+80)
- `rust/crates/attune-core/tests/backup_rollback.rs` (~180 行)

**实施(TDD)**:

**Red**:`tests/backup_rollback.rs` 9 case(per spec §9.1-9.6):
- H2: `test_pre_upgrade_backup_creates_file`
- H3: `test_rollback_restores_vault`
- E3: `test_empty_vault_backup`
- E4: `test_huge_vault_backup` (`#[ignore]` slow)
- E5: `test_concurrent_lock`
- Er2: `test_disk_full_exit_12`
- A3: `test_corrupt_backup_sha256_mismatch`
- E6: `test_rollback_index_out_of_bounds`
- R3: `test_huge_vault_memory_exhaust` (`#[ignore]` slow)

**Green**:impl `backup.rs`:
- `pub fn create_pre_upgrade_backup(vault_path: &Path) -> Result<PathBuf, BackupError>`
- 用 SQLite `VACUUM INTO` 原子复制(无需停服)
- 计算 SHA256 写 .sha256 同伴文件
- backup 文件名:`vault.db.bak.YYYYMMDD-HHMM`
- backup 目录:`~/.local/share/Attune/backups/`
- precheck disk space(< 100MB → exit 12)

**Green**:impl `rollback.rs`:
- `pub fn list_backups() -> Result<Vec<BackupEntry>, RollbackError>`
- `pub fn restore_backup(index: usize, vault_path: &Path) -> Result<(), RollbackError>`
- 验 SHA256 match → 检测 vault.db 锁 → cp backup → done
- 不做 install pkg downgrade(那部分走 ROLLBACK.md 引导)

**Refactor**:`main.rs` clap Subcommand:

```rust
#[derive(Subcommand)]
enum Cli {
    Rollback { index: Option<usize> },
    PreUpgradeBackup,
}
```

**测试**:9 case 1.00 pass(7 fast + 2 `#[ignore]` slow);clippy 干净.

**manual smoke test**(必跑):
1. 本机真起 attune-server
2. 跑 `attune --pre-upgrade-backup` → 验 `~/.local/share/Attune/backups/vault.db.bak.YYYYMMDD-HHMM` 真存在
3. 跑 `attune --rollback` → 真列 backup
4. 跑 `attune --rollback 1` → 真恢复(`diff` vault.db 与 backup 一致)

### C5: `feat(accounts/dsar): GET /me/export + DELETE /me + hard_delete cron`

**仓**:cloud(accounts subrepo)— **跨仓 commit**

**文件**:
- `accounts/src/routes/dsar.rs` (~200 行)
- `accounts/src/cron/hard_delete.rs` (~80 行)
- `accounts/tests/dsar_integration.rs` (~150 行)
- cloud RELEASE 节 v2.2.1 (+40)

**实施(TDD)**:

**Red**:`accounts/tests/dsar_integration.rs` 8 case(per spec §9.1-9.4):
- H5: `test_export_returns_complete_json`
- H6: `test_delete_schedules_30_day_cron`
- E7: `test_restore_in_30_day_window`
- E8: `test_small_and_huge_user_export`
- Er4: `test_rate_limit_1_per_hour`
- Er5: `test_confirmation_mismatch_400`
- Er6: `test_already_scheduled_409`
- A4: `test_export_abuse_rate_limit`

**Green**:impl `routes/dsar.rs`:
- `pub async fn export_me(State(db): State<DbPool>, AuthJwt(jwt): AuthJwt) -> impl IntoResponse`
- query profile + quota + chat_history_metadata + audit_log(last 90 days)
- 返回 schema_version "dsar-1.0" JSON
- rate limit(用 `tower-governor` or in-memory HashMap with TTL)

- `pub async fn delete_me(State(db): State<DbPool>, AuthJwt(jwt): AuthJwt, Json(req): Json<DeleteReq>) -> impl IntoResponse`
- verify `req.confirmation == "DELETE my account"` → 400 if mismatch
- check is_deleted → 409 if already
- UPDATE users SET is_deleted = true, deleted_at = NOW(), deletion_scheduled_for = NOW() + 30 days
- invalidate jwt(push to deny-list)
- 发 restore email(SMTP / mock in test)
- return 202

**Green**:impl `cron/hard_delete.rs`:
- 每日 02:00 跑(用 `tokio-cron-scheduler` or systemd timer)
- SELECT user_id WHERE deletion_scheduled_for < NOW()
- DELETE FROM users + cascade tables
- audit log "dsar_hard_delete" 保留事件元信息

**Refactor**:抽 `DsarError` enum 统一错误处理.

**测试**:8 case 1.00 pass(mock time 用 `mockall` or `tokio::time::pause()`).

**跨仓提交**:本 commit 推 cloud 仓 `develop` 分支,attune 仓 RELEASE.md 引用 cloud-v2.2.1 节即可.

### C6: `ci(release): publish-latest-json.yml + tauri pubkey replacement`

**文件**:
- `.github/workflows/publish-latest-json.yml` (~80 行)
- `apps/attune-desktop/tauri.conf.json` (-1/+1 pubkey 替换 — **user 1-time 已做**,本 commit 只 verify 替换)

**workflow 逻辑**:

```yaml
name: publish-latest-json

on:
  release:
    types: [published]
  workflow_run:
    workflows: ["desktop-release"]
    types: [completed]

jobs:
  publish:
    if: startsWith(github.event.release.tag_name, 'desktop-v')
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install minisign
        run: sudo apt install -y minisign
      - name: Restore minisign private key
        run: |
          echo "${{ secrets.TAURI_PRIVATE_KEY }}" | base64 -d > /tmp/minisign.key
          chmod 600 /tmp/minisign.key
      - name: Generate latest.json
        env:
          TAG: ${{ github.event.release.tag_name }}
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          # 1. download .deb / .exe from release
          # 2. minisign sign each
          # 3. assemble latest.json
          # 4. upload to release as asset
          bash scripts/publish-latest-json.sh "$TAG"
      - name: Verify upload
        run: |
          curl -fI "https://github.com/qiurui144/attune/releases/download/$TAG/latest.json"
          # exit 1 if not 200
```

**scripts/publish-latest-json.sh**(新文件 ~50 行):
- 从 release assets 列 .deb / .exe URL
- 每个跑 `minisign -S -s /tmp/minisign.key -m <asset> -t "v$VERSION"`
- 拼 JSON 写 `/tmp/latest.json`
- `gh release upload "$TAG" /tmp/latest.json`

**测试**:dry-run mode — 推 `desktop-v1.0.1-rc.0` 测试 tag → workflow 跑通 → 删 rc.0 tag → 正式 desktop-v1.0.1 推.

### C7(可选 collapse 进 C6): `docs(release): RELEASE.md v1.0.1 节 + MANUAL_TEST_CHECKLIST.md`

**文件**:
- `RELEASE.md` (+60)
- `tests/MANUAL_TEST_CHECKLIST.md` (+50)

**RELEASE.md v1.0.1 节**:
- Highlights:auto-updater latest.json publish workflow / `/api/v1/version` / `attune --rollback` CLI / DSAR endpoints / `.github/ISSUE_TEMPLATE` / UPGRADING.md / ROLLBACK.md
- Breaking changes:none
- Migration:从 v1.0.0 升级**自动**(Tauri auto-updater 检测),手动 path 见 docs/UPGRADING.md
- Known Limitations:
  - K3 一体机不走 auto-updater(走镜像 reflash)
  - private key rotation playbook 推 v1.0.4
  - SLA 数值化(P0/P1/P2/P3 turnaround time)推 v1.0.2

**MANUAL_TEST_CHECKLIST.md v1.0.1 节**:
- `[ ]` 真装 v1.0.0 → 等 4h auto-update → click upgrade → vault 完整
- `[ ]` 真跑 `attune --pre-upgrade-backup` → backup 文件存在 + SHA256 一致
- `[ ]` 真跑 `attune --rollback` → 列 backup → restore → vault 一致
- `[ ]` 真访问 `GET /api/v1/version` → 返回 current+latest
- `[ ]` 真跑 `GET /me/export` → JSON 字段全
- `[ ]` 真跑 `DELETE /me` 输错 confirmation → 400
- `[ ]` 真跑 `DELETE /me` → 202 → 30 天(mock time)→ hard delete

---

## 跨仓协调

### attune 仓 ↔ cloud 仓

| commit | 仓 | 依赖 |
|--------|----|------|
| C1-C4 | attune 仓 | 无 |
| C5 | cloud(accounts) | 无,独立 |
| C6 | attune 仓 | C5 完成(workflow 文档引 DSAR endpoint)|
| C7 | attune 仓 | C5 完成(RELEASE 引 cloud-v2.2.1) |

**merge 顺序**:
1. attune 仓 C1-C4 → develop
2. cloud 仓 C5 → develop(tag cloud-v2.2.1-rc.0 测)
3. attune 仓 C6-C7 → develop
4. attune develop → main `--no-ff` + `v1.0.1` + `desktop-v1.0.1` tag
5. cloud develop → main + `cloud-v2.2.1` tag

**强配对约束**(per § 跨仓版本配对):attune v1.0.1 RELEASE.md 声明 "Compatible with cloud-v2.2.1+";cloud-v2.2.1 RELEASE.md 声明 "Compatible with attune v1.0.1+".

### attune-pro 仓

**不参与本 spec**(per § 范围边界 — plugin upgrade 推 v1.0.10).

---

## 测试策略(TDD)

### 总览(全合 1.00 pass + clippy 干净)

| commit | 测试文件 | case 数 | 阻塞 PR |
|--------|---------|---------|--------|
| C1 | doc lint | — | markdown lint 不报 error |
| C2 | manual GitHub UI verify | 3 template + PR | UI 真显示 |
| C3 | `tests/version_route.rs` | 5 | 1.00 pass + clippy |
| C4 | `tests/backup_rollback.rs` | 9(7 fast + 2 ignore) | 7 fast 1.00 pass + clippy |
| C5 | `accounts/tests/dsar_integration.rs` | 8 | 8 pass + clippy |
| C6 | workflow dry-run rc.0 tag | manual | 真上传后 curl -I 200 |
| C7 | MANUAL_TEST_CHECKLIST 7 项 | 真本机 | 全 ✓ + 截图归档 |

### 黑盒视角 user-first(per § Bug reproduce 第一步必须 user 视角)

C7 manual checklist 必须**真装 v1.0.0 .deb**(GH release artifact)→ 真升 v1.0.1 → 真 rollback → 真 DSAR。**禁止** `cargo build --release` 替代 install pkg.

### 6 类下限 + adversarial / 多并发 / 资源耗尽 / 国际化 / 降级

per spec §9.1-9.9 矩阵:

- happy: H1-H6
- edge: E1-E8
- error: Er1-Er6
- adversarial: A1-A4
- 多并发: C1-C2
- 资源耗尽: R1-R3
- 国际化: I1-I3
- 降级: D1-D2

**Must reach** before tag v1.0.1:**所有 H + E + Er + A + D 必须实现 + 1.00 pass**;C / R / I 可标 `#[ignore]` slow lane 但仍要 implement.

### `agent_golden_gate` 等价 harness

本 spec **不引入新 agent**,无 agent_golden_gate 触发.若后续 v1.0.x 加 agent,沿用 attune-pro `agent_golden_gate.rs` pattern.

---

## 风险登记 + 缓解(plan 角度)

| R | 风险 | 严重度 | plan 角度缓解 |
|---|------|--------|--------------|
| R1 | **minisign 私钥泄露** | 🔴 critical | user 1-time 严格执行(spec 附录 B);**禁** AI 起草 / echo / 写 spec 任何私钥内容;`*.key` 进 .gitignore;trufflehog pre-commit |
| R2 | publish-latest-json workflow 漏写 | 🟡 medium | C6 dry-run 用 rc.0 tag 测;真 `curl -I` 200 后才 promote 正式 tag |
| R3 | disk full backup fail | 🟡 medium | C4 写 `test_disk_full_exit_12`,真 mock 小磁盘场景(`tempfile` + 写满到限) |
| R4 | rollback no backup | 🟡 medium | C4 写 `test_rollback_index_out_of_bounds` exit 14;ROLLBACK.md §4 离线 path 教学 |
| R5 | i18n 漏 | 🟡 medium | I1 + I3 测试强制;C1 UPGRADING.md / ROLLBACK.md **只 zh**(per § 文档体系铁律) |
| R6 | DSAR delete 误操作 | 🟡 medium | C5 写 `test_confirmation_mismatch_400` 严测;UI restore email 强制 |
| R7 | bandwidth | 🟢 low | C5 rate limit 1/h 测;大 user 异步 path 推 v1.0.7 |
| R8 | K3 user 困惑 | 🟢 low | C1 ROLLBACK.md §5 K3 节;wiki 推 v1.0.9 i18n minor |
| R9 | workflow forget platforms key | 🟡 medium | C6 workflow 内 hardcoded platforms list + step 检查 release assets 都 exist 才 commit JSON |
| R10 | GH API rate limit | 🟢 low | C3 ETag cache 6h;`If-None-Match` header |

### plan 层级风险(spec 不涵盖)

| P | 风险 | 缓解 |
|---|------|------|
| P1 | 3 天 sprint 时间紧 — D2 cross-repo C5 易卡 cloud accounts 仓 maintainer 沟通 | C5 前置 D1 完成;cloud 仓改动 self-review 走 D2 上午;若 D2 卡 → C5 推 v1.0.2 但 C1-C4 + C6 仍 ship |
| P2 | minisign keypair 生成涉及 user 时间(D3 09:00 等 user 1-time) | spec 已写清楚 step-by-step,user 5 分钟可完成 |
| P3 | publish-latest-json.yml 在 desktop-v* tag 触发但 desktop-release.yml 必须先跑完(build .deb / .msi);workflow `workflow_run` trigger 不一定 deterministic | C6 用 `on: release: types: [published]` 而非 workflow_run — release 发布事件触发更可靠 |
| P4 | DSAR 法律口径 — "soft delete + 30 天 hard delete" 是否满足 GDPR / 个保法 turnaround SLA | spec §5.4 已明示 30 天窗口;**v1.0.8 律师审 ToS / Privacy Policy** 时再 verify 法律完整性 — v1.0.1 实施合规框架,不 final 法律口径 |
| P5 | RC 阶段纪律 — v1.0.1 是 patch 还是 minor?是否走 rc cycle? | per § 发版纪律 §2 patch 只用于 bug fix,本 spec 是 minor(引入新 endpoint + CLI + DSAR);**走 minor + rc** 路径 — D3 推 `desktop-v1.0.1-rc.0` workflow dry-run,通过 → develop → main → `v1.0.1` + `desktop-v1.0.1`;若 rc.0 漏 bug → rc.1 |
| P6 | 跨仓 cloud 仓改动 user 是否授权 push | per CLAUDE.md § Git push 权限 cloud 仓未明授权 push,**C5 完成后须 user 确认 push** — plan D2 下午预留 user check-in 时段 |

---

## GA 验收清单(rc → GA)

per § RC 阶段纪律 4 节门:

### Gate 1: 文档审计

- [ ] README.md link 进 UPGRADING.md + ROLLBACK.md + SUPPORT.md
- [ ] RELEASE.md v1.0.1 节齐(Highlights / Breaking / Migration / Known Limits)
- [ ] DEVELOP.md 引 .github/PULL_REQUEST_TEMPLATE.md 一致
- [ ] CLAUDE.md 不需要改(本 spec 不引入新铁律)
- [ ] tauri.conf.json `version` = "1.0.1"(D3 替换占位时一并改)

### Gate 2: 代码审计

- [ ] `cargo test --workspace --release` 全过
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` 干净
- [ ] 新 `#[ignore]` test 个数 ≤ 2(E4 + R3 slow lane)
- [ ] no WIP / TODO / FIXME-CRITICAL in commit msg

### Gate 3: 功能预期对齐

- [ ] H1-H6 happy path 真实 install pkg + 真启服 + 真触发(per § Release 验证铁律)
- [ ] manual GitHub UI verify 3 issue template 显示
- [ ] workflow dry-run rc.0 真上传 latest.json + `curl -I` 200
- [ ] 截图归 `docs/screenshots/v1-0-1-verification/`(per § 截图存放规范)

### Gate 4: 缺口登记

- [ ] RELEASE.md Known Limitations 列:
  - K3 一体机不走 auto-updater
  - private key rotation playbook 推 v1.0.4
  - SLA P0/P1/P2/P3 推 v1.0.2

---

## 回头改 spec 触发条件

per § 架构设计铁律 §plan 评审过 implementation 严格按 plan 走;偏离 plan 要回头改 plan(不允许 silent drift):

**若实施时发现**:
- C4 backup 用 `VACUUM INTO` 在 K3 riscv64 上 fail / 兼容性问题 → 回头改 spec §3.2 数据流 + plan C4
- C5 DSAR `chat_history_metadata` 字段无法从 accounts DB 获取(需 cross-call attune-server)→ 回头改 spec §3.4 + plan C5 scope
- C6 workflow trigger `on: release` 不可靠 → 回头改 spec §5.3 + plan C6 trigger 设计

**禁止**:
- 实施时静默改设计不更新 spec
- 实施时发现 spec 11 节缺漏直接绕开(必须先回头补 spec,再继续 plan)
- scope 涨了直接扩 implementation(超 scope = 新 feature,要新 spec / 推 v1.0.2)

---

**plan 完。3 天后(2026-05-28 周三)v1.0.1 GA tag push。**
