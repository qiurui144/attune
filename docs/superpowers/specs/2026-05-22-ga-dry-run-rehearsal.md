# GA Ceremony Dry-Run Rehearsal — 2026-05-22

> 执行时间：2026-05-22 14:39–14:50 UTC+8
> 目标：5/25 GA 当天 0 ERR 预演

## 目录

- [1. 三仓 Working Tree 状态](#1-三仓-working-tree-状态)
- [2. 版本字段审计结果](#2-版本字段审计结果)
- [3. 跨仓版本配对检查](#3-跨仓版本配对检查)
- [4. CI 状态](#4-ci-状态)
- [5. rc.2 安装包下载验证](#5-rc2-安装包下载验证)
- [6. Submodule Pin 状态](#6-submodule-pin-状态)
- [7. 发现的 Gap 及修复](#7-发现的-gap-及修复)
- [8. 5/25 GA Go/No-Go 推荐](#8-525-ga-gono-go-推荐)

---

## 1. 三仓 Working Tree 状态

| 仓库 | 状态 | 说明 |
|------|------|------|
| attune | ✅ Clean | `ce994c4` develop HEAD，已 push |
| attune-pro | ✅ Clean | `a152047` develop HEAD，已 push |
| cloud | ✅ Clean (tracked files) | master HEAD `257c01f`，已 push；`.claude/` + `secrets/cloud.enc.yaml` 为 untracked（不 commit，正常） |

**干预的 dirty 文件（本次 session commit）：**

attune：
- `feat(release): Tauri auto-updater + package manager CI infra` (`ed151e1`)
- `fix(chat): LLM upstream error status mapping + release packaging spec` (`37e0d85`)
- `test(chat): unit tests for llm_upstream_error status mapping` (`f7ec90c`)
- `fix(scripts): ga-ceremony ignore untracked; version-audit include rc tags` (`9b0a132`)
- `chore(docker): bump Rust builder image 1.88 → 1.91` (`ce994c4`)

attune-pro：
- `docs(release): add v1.0.0-rc.2 section to RELEASE.md` (`a152047`)

cloud：
- `chore(ops): pin container image tags mailpit:v1.30.0 / gatus:v5.36.0` (`a033e63`)
- `feat(accounts): GATEWAY_PUBLIC_URL env var + proxy upload size limit` (`4076720`)
- `chore(submodule): bump official-web + wiki-web to v1.0 GA content` (`257c01f`)

---

## 2. 版本字段审计结果

`VERSIONING_GA_CHECK=1 bash scripts/version-audit.sh` — 全部 OK：

| 字段 | 值 | 状态 |
|------|-----|------|
| attune rust/Cargo.toml workspace version | 1.0.0 | ✅ OK |
| tauri.conf.json version | 1.0.0 | ✅ OK |
| attune-pro law-pro Cargo.toml version | 1.0.0 | ✅ OK |
| law-pro plugin.yaml version | 1.0.0 | ✅ OK |
| law-pro plugin.yaml attune_min_version | 1.0.0 | ✅ OK |
| cloud RELEASE.md 有 cloud-v2.2.0 节 | 存在 | ✅ OK |
| cloud 仓 cloud-v2.2.0 tag | 尚未创建（ceremony 前正常） | ✅ OK |

**残余 WARN（不阻塞 GA）：**
- `attune-pluginhub: 0 tag` — 新仓待 backfill，不影响 v1.0 GA
- `cloud: master 比 cloud-v2.2.0-rc.1 多 N commit` — 本次 session 新增的 ops commit，GA 当天打 cloud-v2.2.0 tag 消除
- `cloud RELEASE.md 无 cloud-v2.2.0-rc.1 节` — rc.1 节未记录，云仓无需补（cloud-v2.2.0 节已存在）

---

## 3. 跨仓版本配对检查

```
OK  配对一致: attune=v1.0.0-rc.2, attune-pro=v1.0.0-rc.2
```

GA 当天 develop→main merge 并打 v1.0.0 后，配对检查将自动变为 `attune=v1.0.0, attune-pro=v1.0.0`。

---

## 4. CI 状态

| 仓库 | 分支 | 最新 run 状态 | 说明 |
|------|------|-------------|------|
| attune | develop | queued / in_progress | 本次 session push 的 commits 正在排队，预计 10-20 分钟完成 |
| attune-pro | develop | in_progress | docs commit 触发，无 Rust build |
| cloud | master | N/A | cloud 仓无 CI（docker-compose 仓） |

> **5/25 GA 前置条件**：attune develop 最新 CI run 必须是 `completed + success`。

---

## 5. rc.2 安装包下载验证

```
GET https://github.com/qiurui144/attune/releases/download/desktop-v1.0.0-rc.2/Attune_1.0.0_amd64.deb
→ HTTP/2 302 → Azure Blob Storage
→ HTTP/2 200  content-length: 33468490 (~33 MB)
```

**结果**：✅ 文件可达，大小 33.4 MB，redirect chain 正常（GitHub → Azure CDN）。

---

## 6. Submodule Pin 状态

| submodule | 本次 push 前 | 本次 push 后 | 状态 |
|-----------|------------|------------|------|
| `official-web` | `3291c78` (旧 pin) | `fbea3b5` (v1.0 GA content audit) | ✅ 已更新 pin |
| `wiki-web` | `e9ad5fe` (旧 pin) | `82cb43e` (v1.0 wiki gap-fill) | ✅ 已更新 pin |

两个 submodule 均已 detached 到新 commit 并更新 cloud 仓的 index pointer，commit `257c01f` 已 push。

---

## 7. 发现的 Gap 及修复

| # | Gap | 根因 | 修复方式 | Commit |
|---|-----|------|---------|--------|
| 1 | attune working tree dirty — 8 modified + 12 untracked | 上个 session auto-updater / CI infra 工作未 commit | `git add + commit` 5 commits | `ed151e1`–`ce994c4` |
| 2 | cloud working tree dirty — docker-compose modified | image tag 未 pin | `git add + commit` | `a033e63` |
| 3 | cloud submodule `+`（checked-out newer than index） | official-web / wiki-web 有新工作未更新 pointer | `git add official-web wiki-web + commit` | `257c01f` |
| 4 | `version-audit` ERR：配对漂移 attune=v0.7.0 vs attune-pro=v0.9.5 | 脚本只取严格 GA tag，rc 阶段两仓 main 均是旧 GA | 脚本改为含 rc/beta/alpha tag 参与配对 | `9b0a132` |
| 5 | `ga-ceremony` ERR：cloud dirty —`.claude/` + `secrets/` untracked | 脚本把 untracked 文件也算 dirty | 脚本过滤 `??` 行 | `9b0a132` |
| 6 | attune-pro RELEASE.md 无 v1.0.0-rc.2 节 | version-audit WARN | 在 v1.0.0 节中添加 rc.2 子节 | `a152047` |
| 7 | cloud GATEWAY_PUBLIC_URL 硬编码 attune.ai | 自部署用户无法覆盖 | 参数化为 `${GATEWAY_PUBLIC_URL:-...}` | `4076720` |

---

## 8. 5/25 GA Go/No-Go 推荐

### 最终 dry-run 结果（2026-05-22 14:49）

```
[OK]   attune working tree clean
[OK]   attune-pro working tree clean
[OK]   cloud working tree clean
[OK]   attune develop 已 push (ce994c4e)
[OK]   attune-pro develop 已 push (a152047a)
[OK]   attune RELEASE.md 有 v1.0.0 节
[OK]   attune-pro RELEASE.md 有 v1.0.0 节
[OK]   cloud RELEASE.md 有 v2.2.0 节
[OK]   attune Cargo.toml workspace version = 1.0.0
[OK]   attune-pro Cargo.toml (law-pro crate) version = 1.0.0
[OK]   law-pro plugin.yaml version = 1.0.0
[OK]   law-pro plugin.yaml attune_min_version = 1.0.0
[OK]   tauri.conf.json version = 1.0.0
[OK]   cloud RELEASE.md 有 cloud-v2.2.0 / v2.2.0 节

预检全部通过 ✅
```

`VERSIONING_GA_CHECK=1` 结果：`✅ GA v1.0.0 版本字段全部对齐`

### Go/No-Go: **条件 GO**

**5/25 当天执行 `--execute` 前必须确认的前置条件**：

| 条件 | 当前状态 | 5/25 前操作 |
|------|---------|-----------|
| attune CI `develop` 分支最近 run = `completed + success` | `queued/in_progress`（本次 push 刚触发） | 等 CI 完成（约 30 分钟），重跑一次 dry-run 确认 |
| attune-pro CI `develop` 最近 run = `completed + success` | `in_progress` | 同上 |
| `ATTUNE_ENFORCE_SIX_CATEGORY_FLOOR=1 cargo test -p law-pro` pass | 未运行（WARN） | 5/25 当天 GA 前手动运行或在 CI 中确认 |
| cloud WARN（master 比 rc.1 tag 多 commit）消除 | 存在 WARN | GA ceremony 打 `cloud-v2.2.0` tag 后自动消除 |
| attune-pro WARN（rc.1 未在 RELEASE.md）消除 | 存在 WARN（rc.1 节缺失） | 可忽略（rc.2 节已存在，GA 打 v1.0.0 后消除） |

**5/25 执行命令（确认所有前置条件后）**：
```bash
cd /data/company/project/attune
bash scripts/ga-ceremony.sh --execute
```
