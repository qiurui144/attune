# Attune v1.0.0 GA Checklist

与 `scripts/ga-ceremony.sh` 配套的人工确认清单。在执行 `--execute` 之前，按 Phase 顺序逐项核查。

## 目录 (Table of Contents)

- [Phase A — 前置条件（ceremony 前手工核查）](#phase-a--前置条件ceremony-前手工核查)
- [Phase B — ceremony 执行](#phase-b--ceremony-执行)
- [Phase C — 上架后验证](#phase-c--上架后验证)
- [签字确认](#签字确认)

---

## Phase A — 前置条件（ceremony 前手工核查）

在运行 `bash scripts/ga-ceremony.sh --execute` 之前，以下项目必须全部 ✅。

### A1. 三仓 working tree clean

| 仓库 | 命令 | 预期 | 结果 |
|------|------|------|------|
| attune | `git -C /data/company/project/attune status --short \| grep -v '^??'` | 无输出 | [ ] |
| attune-pro | `git -C /data/company/project/attune-pro status --short \| grep -v '^??'` | 无输出 | [ ] |
| cloud | `git -C /data/company/cloud status --short \| grep -v '^??'` | 无输出 | [ ] |

### A2. develop 分支已 push 到 remote

| 仓库 | 命令 | 预期 | 结果 |
|------|------|------|------|
| attune | `git -C /data/company/project/attune rev-parse develop` vs `origin/develop` | SHA 一致 | [ ] |
| attune-pro | `git -C /data/company/project/attune-pro rev-parse develop` vs `origin/develop` | SHA 一致 | [ ] |

### A3. attune CI 全绿（develop 分支最近一次 run）

```bash
gh run list --repo qiurui144/attune --branch develop --limit 5
```

预期：最近一次 status=completed conclusion=success。

- [ ] `rust-ci.yml` 绿
- [ ] `desktop-release.yml`（若有 rc tag）绿

### A4. 版本字段三仓一致（均为 1.0.0）

| 文件 | 路径 | 预期 | 结果 |
|------|------|------|------|
| attune Cargo.toml | `rust/Cargo.toml` workspace version | `1.0.0` | [ ] |
| attune-pro law-pro Cargo.toml | `plugins/law-pro/Cargo.toml` | `1.0.0` | [ ] |
| attune-pro plugin.yaml | `plugins/law-pro/plugin.yaml` version | `1.0.0` | [ ] |
| attune-pro plugin.yaml | `plugins/law-pro/plugin.yaml` attune_min_version | `1.0.0` | [ ] |
| tauri.conf.json | `apps/attune-desktop/tauri.conf.json` | `1.0.0` | [ ] |

验证命令（ga-ceremony.sh --dry-run 会自动跑）：

```bash
bash scripts/ga-ceremony.sh --dry-run
```

### A5. 三仓 RELEASE.md 均有 v1.0.0 节

| 仓库 | 命令 | 预期 | 结果 |
|------|------|------|------|
| attune | `grep -c '## v1\.0\.0' rust/RELEASE.md` | ≥ 1 | [ ] |
| attune-pro | `grep -c '## v1\.0\.0' /data/company/project/attune-pro/RELEASE.md` | ≥ 1 | [ ] |
| cloud | `grep -c '## v2\.2\.0' /data/company/cloud/RELEASE.md` | ≥ 1 | [ ] |

### A6. Agent 6 类下限 gate（attune-pro）

```bash
ATTUNE_ENFORCE_SIX_CATEGORY_FLOOR=1 cargo test -p attune-law-pro --release 2>&1 | tail -5
```

预期：全部 PASS，无 FAILED。

- [ ] Golden gate 1.00 pass rate（deterministic agents）
- [ ] LLM gate F1 ≥ 0.85（fact_extractor）

### A7. MANUAL_TEST_CHECKLIST 核心维度人工验收

参考 `tests/MANUAL_TEST_CHECKLIST.md`。GA 前必须至少覆盖：

- [ ] **A. 安装**：Linux deb 安装成功（A2）
- [ ] **B. Wizard 首启**：5 步走通，数据源配置保存（B1-B4）
- [ ] **C. 核心功能**：PDF 上传 + Chat RAG 引用 + 搜索（C1-C3）
- [ ] **D. OCR/ASR**：图片 OCR 成功（D1）
- [ ] **E. Plugin**：law-pro 安装 + 基本功能（E1-E2）
- [ ] **F. 异常**：密码错误 → 401（F1）

---

## Phase B — ceremony 执行

全部 Phase A 项目 ✅ 后，执行：

```bash
# Step 1: 再跑一次 dry-run 确认操作计划
bash scripts/ga-ceremony.sh --dry-run

# Step 2: 确认无误后执行（交互会再次确认）
bash scripts/ga-ceremony.sh --execute
```

脚本执行顺序：
1. 三仓预检（CI / clean / develop push / 版本一致 / RELEASE.md 节存在）
2. attune: `develop → main --no-ff` + tag `v1.0.0` + `desktop-v1.0.0`
3. attune-pro: `develop → main --no-ff` + tag `v1.0.0`
4. cloud: tag `cloud-v2.2.0`（无 merge，直接在 master HEAD）
5. push 三仓 + 所有 tag → 触发 GH Actions

执行中：

- [ ] `--dry-run` 输出计划符合预期（Step A1）
- [ ] `--execute` 交互确认后开始运行（Step A2）
- [ ] 脚本退出码 0，无 `[ERR]` 行（Step A3）

---

## Phase C — 上架后验证

tag push 后约 10-20 分钟，验证以下 GH Actions 产物。

### C1. attune GitHub Releases 页

```bash
gh release list --repo qiurui144/attune --limit 5
```

- [ ] `v1.0.0` release 存在，非 prerelease，含 server/CLI tarball（4 平台 × 2 文件 = 8 产物）
- [ ] `desktop-v1.0.0` release 存在，含 5 形态产物（NSIS exe / MSI / .deb / RPM / AppImage）× 2 平台 = 5 文件

### C2. attune-pro tag

```bash
gh release list --repo qiurui144/attune-pro --limit 3
```

- [ ] `v1.0.0` tag 存在

### C3. cloud tag

```bash
cd /data/company/cloud && git tag | grep cloud-v2.2.0
```

- [ ] `cloud-v2.2.0` tag 存在（cloud 仓无 GH Release 自动化，仅 tag）

### C4. container images（如有 docker-publish.yml）

```bash
gh run list --repo qiurui144/attune --workflow docker-publish.yml --limit 3
```

- [ ] `docker-publish.yml` 触发并完成（若未触发，手动 `workflow_dispatch`）

### C5. wiki-web 部署

- [ ] `https://wiki.attune.ai` 可访问，页面版本号显示 v1.0.0

### C6. official-web 部署

- [ ] `https://attune.ai` 可访问，下载页显示 v1.0.0 安装链接

### C7. accounts / pluginhub healthcheck

```bash
curl -sf https://accounts.attune.ai/health | python3 -m json.tool
curl -sf https://pluginhub.attune.ai/health | python3 -m json.tool
```

- [ ] accounts 返回 `{"status":"ok"}`
- [ ] pluginhub 返回 `{"status":"ok"}`

---

## 签字确认

| 角色 | 姓名 | 日期 | 签字 |
|------|------|------|------|
| 执行人 | | | |
| 复核人 | | | |

**GA 发布完成时间**：___________

**Release URL**：
- attune: `https://github.com/qiurui144/attune/releases/tag/v1.0.0`
- desktop: `https://github.com/qiurui144/attune/releases/tag/desktop-v1.0.0`
- attune-pro: `https://github.com/qiurui144/attune-pro/releases/tag/v1.0.0`
