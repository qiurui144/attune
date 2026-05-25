# Unified Pluginhub 架构 + lawcontrol → attune-enterprise Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 统一付费 plugin 分发管道（attune-pro + attune-enterprise 都走 pluginhub），lawcontrol 改名 attune-enterprise（保留业务逻辑，仅做发布/品牌/标识改造），不破坏 GA 与上架节奏。

**Architecture:** pluginhub schema 扩 `Plugin.visibility` + `Plugin.org_id` + `License.org_id`；index/download/admin API 加可见性过滤；attune-pro plugin 标 `visibility=pro`；lawcontrol 现有 plugin 抽出元信息标 `visibility=enterprise + org_id`；改名走 staged migration（GA 前不动 / GA 后逐步切 / hotfix 阶段才动 DB rename）。

**Tech Stack:** pluginhub = Python FastAPI + Alembic + SQLAlchemy + pytest；attune-pro = Rust plugins + YAML manifest；attune-enterprise (原 lawcontrol) = Django + Postgres + docker-compose；attune (OSS) = Rust serde 兼容。

**Spec reference:** `docs/superpowers/specs/2026-05-22-unified-pluginhub-architecture.md` (commit `5512fac`).

**Inventory reference:** `/tmp/lawcontrol-rename-inventory.md` (~780 行改动 / ~60 个文件 / 7 个 docker image / 4 个跨仓引用)。

**Deadline / Window:** 7 天 wall-clock（**2026-05-23 → 2026-05-30**），跨越 v1.0 GA（5/25）+ 上架日（5/26）+ v1.0.1 hotfix（5/28）。

**核心节奏硬约束（与 5 天 GA roadmap 协调）:**

| 日期 | 状态 | pluginhub | rename | 备注 |
|------|------|-----------|--------|------|
| **5/23 (D1, GA-2)** | dev | schema migration + JWT fix（**non-disruptive**） | 不动 | dev-only 改动，不影响 GA |
| **5/24 (D2, GA-1)** | dev | API filter logic + attune-pro yaml + sample ent plugin | 不动 | scope 收口，留缓冲给 GA |
| **5/25 (D3, GA)** | **freeze** | **不动** | **不动** | 只 tag `v1.0.0` + `desktop-v1.0.0` |
| **5/26 (D4, 上架)** | release | pluginhub v1.0 GA + 双仓并存 | 老仓 freeze + GitHub remote 已改名 (前置) | cloud / wiki / official-web 上架 |
| **5/27 (D5)** | rename | - | 全量 rename：代码 / image / submodule / Gitea / 本地 dir（保留 DB 名 `lawcontrol`） | 4 跨仓引用同步 |
| **5/28 (D6, v1.0.1 hotfix)** | hotfix | - | **DB rename hotfix window**：停服 ≤5 min + pg_dump + ALTER DATABASE + restart | 唯一 downtime |
| **5/29-30 (D7)** | cleanup | - | 清理 + 验证 + sign-off + post-mortem | grep 0 残留 |

---

## 目录 (Table of Contents)

- [Phase D1 (5/23) — pluginhub schema + JWT bug fix](#phase-d1-523--pluginhub-schema--jwt-bug-fix)
- [Phase D2 (5/24) — pluginhub API filter + plugin manifest](#phase-d2-524--pluginhub-api-filter--plugin-manifest)
- [Phase D3 (5/25) — GA freeze, pluginhub 不动](#phase-d3-525--ga-freeze-pluginhub-不动)
- [Phase D4 (5/26) — 上架日 + pluginhub v1.0 GA](#phase-d4-526--上架日--pluginhub-v10-ga)
- [Phase D5 (5/27) — 全量 rename（代码 / image / submodule / Gitea / 本地 dir）](#phase-d5-527--全量-rename代码--image--submodule--gitea--本地-dir)
- [Phase D6 (5/28) — v1.0.1 hotfix：DB rename downtime window](#phase-d6-528--v101-hotfix-db-rename-downtime-window)
- [Phase D7 (5/29-30) — 清理 + 验证 + sign-off](#phase-d7-529-30--清理--验证--sign-off)
- [12 Commit 分批清单](#12-commit-分批清单)
- [风险登记 + Rollback Matrix](#风险登记--rollback-matrix)
- [GA + 上架验收清单](#ga--上架验收清单)
- [测试矩阵（per Agent 验证铁律 6 类下限）](#测试矩阵per-agent-验证铁律-6-类下限)
- [跨仓协调点](#跨仓协调点)

---

## Phase D1 (5/23) — pluginhub schema + JWT bug fix

目标：pluginhub backend 加字段 + 修一处历史 JWT tier 判断 bug（License.plan 读不对）。**全部 non-disruptive**：旧 client / 旧 license / 旧 plugin 行为零变化。GA-2 日做，留 GA-1 给 API 逻辑。

### Task D1.1: Alembic migration — 加 visibility / org_id 字段

**Files (attune-pluginhub):**
- New: `alembic/versions/2026_05_23_add_visibility_org_id.py`
- Modify: `pluginhub/models.py`（`Plugin` + `License` 各加字段）
- Test: `tests/test_migration_visibility.py`

- [ ] **Step 1: 写 Alembic upgrade/downgrade**

加字段（per spec §10.1）：
- `plugin.visibility` String(20) NOT NULL DEFAULT 'public'
- `plugin.org_id` String(64) NULL
- `license.org_id` String(64) NULL
- index `ix_plugin_visibility_org` on (visibility, org_id)

downgrade 必须可执行（drop column + drop index）。

- [ ] **Step 2: 更新 ORM model**

`pluginhub/models.py`:
```python
class Plugin(Base):
    # ... 已有字段
    visibility = Column(String(20), nullable=False, default='public')
    org_id = Column(String(64), nullable=True)

class License(Base):
    # ... 已有字段
    org_id = Column(String(64), nullable=True)
```

- [ ] **Step 3: Backfill SQL（一次性，从 staging dry-run 后再 prod 跑）**

```sql
-- 旧 pro plugin 标 visibility=pro
UPDATE plugin SET visibility = 'pro' WHERE min_plan = 'pro';
-- 旧 enterprise plugin 标 visibility=enterprise
UPDATE plugin SET visibility = 'enterprise' WHERE min_plan = 'enterprise';
```

- [ ] **Step 4: Migration test**

`tests/test_migration_visibility.py` 用 in-memory SQLite + alembic.config 验证 upgrade → downgrade → upgrade 三次幂等。

**Acceptance:** `alembic upgrade head` + `alembic downgrade -1` + `alembic upgrade head` 三次跑通；旧数据零损坏；ORM model 反映新字段。

### Task D1.2: 修 License JWT tier bug

**Background:** Spec §1.1 提到「pluginhub 已有 `License.plan = individual / pro / enterprise` 三档，但 enterprise 档无实际 plugin 路由进来」。审计期间发现 JWT 解码时把 `max_installs` 启发式当 tier 推断（历史短路逻辑），需要改成读 `License.plan` 字段。

**Files:**
- Modify: `pluginhub/auth/jwt_decoder.py`
- Test: `tests/test_jwt_tier.py`

- [ ] **Step 1: 找出 tier 启发式**

`grep -n "max_installs" pluginhub/auth/*.py` 定位旧启发式（约 1 处）。

- [ ] **Step 2: 替换为读 License.plan**

```python
# Before
tier = 'pro' if license.max_installs > 1 else 'individual'

# After
tier = license.plan  # 'individual' | 'pro' | 'enterprise'
assert tier in ('individual', 'pro', 'enterprise'), f"unknown plan: {tier}"
```

- [ ] **Step 3: 加 plan-not-set fallback**

旧 license（migration 前发的）若 `plan IS NULL` → 兜底 `'individual'`（最低权限），并 audit log 一行。

- [ ] **Step 4: 测试 7 个 plan 组合**

| plan | max_installs | 期望 tier |
|------|-------------|----------|
| individual | 1 | individual |
| individual | 5 | individual（不再被 max_installs 误判） |
| pro | 1 | pro |
| pro | 50 | pro |
| enterprise | 50 | enterprise |
| NULL（旧 license） | 1 | individual (fallback) |
| NULL（旧 license） | 50 | individual (fallback + log) |

**Acceptance:** JWT decoder 在 7 个 fixture 下都返回正确 tier；audit log 记录 fallback case。

### D1 Verification

- [ ] `cd attune-pluginhub && alembic upgrade head` 成功
- [ ] `pytest tests/test_migration_visibility.py tests/test_jwt_tier.py` 全绿
- [ ] staging 跑 backfill SQL 后 `SELECT visibility, COUNT(*) FROM plugin GROUP BY visibility` 显示新分布
- [ ] **Commit C1 + C2**（见 §「12 Commit 分批清单」）
- [ ] dev push（**不发 prod**，等 D2 完整 API 后一起）

---

## Phase D2 (5/24) — pluginhub API filter + plugin manifest

目标：API 层加 visibility 过滤 + attune-pro 现有 plugin 标 `visibility=pro` + lawcontrol 抽一个 sample enterprise plugin 注册到 pluginhub（不动 lawcontrol 仓代码）。GA-1 日，所有改动仅影响付费用户路径，OSS 用户路径零变化。

### Task D2.1: index.json API 加 visibility 过滤

**Files:**
- Modify: `pluginhub/api/index.py`
- New: `pluginhub/auth/visibility_filter.py`（抽出可重用 logic）
- Test: `tests/test_index_visibility.py`

- [ ] **Step 1: 实现 filter_visible_plugins**

`pluginhub/auth/visibility_filter.py`（per spec §5.1 pseudo code）：

```python
def filter_visible_plugins(
    session: Session,
    license: Optional[License]
) -> list[Plugin]:
    q = session.query(Plugin).filter(Plugin.yanked.is_(False))
    if license is None:
        return q.filter(Plugin.visibility == 'public').all()

    visible_levels = ['public']
    if license.plan in ('pro', 'enterprise'):
        visible_levels.append('pro')
    if license.plan == 'enterprise':
        visible_levels.append('enterprise')

    q = q.filter(Plugin.visibility.in_(visible_levels))
    q = q.filter(Plugin.min_plan_ord <= PLAN_ORD[license.plan])

    if license.org_id is None:
        q = q.filter(Plugin.org_id.is_(None))
    else:
        q = q.filter(or_(Plugin.org_id.is_(None), Plugin.org_id == license.org_id))

    if license.allowed_plugins:
        q = q.filter(Plugin.id.in_(license.allowed_plugins))

    return q.all()
```

- [ ] **Step 2: 接入 GET /api/v1/index.json**

修改 `pluginhub/api/index.py` 调用 `filter_visible_plugins(session, current_license)`，并在每个 plugin 返回值加 `visibility` + `org_id` 字段（serialize）。

- [ ] **Step 3: 12 个 golden fixture 测试**

per spec §9.1 golden case 12 个 fixture（`tests/golden/license-plugin-matrix/01-anon-public.yaml` ~ `12-revoked-license.yaml`），驱动 `test_index_visibility.py` 全覆盖。

**Acceptance:** 12 fixture 全过；匿名调用只返 public；pro license 不见 enterprise；enterprise org_id 隔离严格。

### Task D2.2: download API 加可见性校验

**Files:**
- Modify: `pluginhub/api/download.py`
- Test: `tests/test_download_visibility.py`

- [ ] **Step 1: 服务端重跑 filter（防 url 拼装绕过）**

per spec §7.2 边界 R5（红色风险）：individual 用户即使知道 plugin id，直接拼 url 也必须 403。

```python
@router.get('/plugin/{plugin_id}/download')
def download(plugin_id: str, license: Optional[License] = Depends(get_current_license)):
    visible = filter_visible_plugins(session, license)
    if not any(p.id == plugin_id for p in visible):
        raise HTTPException(403, detail={'code': 'plugin-visibility-mismatch'})
    # ... 签 presigned URL, 5 min 短时效
```

- [ ] **Step 2: 错误码 kebab-case**

per spec §7.1，全部 7 个错误码 kebab；client `code` 字段稳定。

- [ ] **Step 3: presigned URL 短时效**

S3 presign 5 min（per spec §11 R5 缓解）。

- [ ] **Step 4: 安全测试（红色风险 R5）**

`tests/test_security_visibility.py`：
- individual user 拼 url → 403 visibility-mismatch
- pro user 拼 enterprise plugin url → 403 visibility-mismatch
- enterprise A 拼 enterprise B 的 plugin url → 403 visibility-mismatch（org_id 隔离）

**Acceptance:** 3 个安全 case 全 403；audit log 记录越权尝试。

### Task D2.3: admin/licenses API 加 org_id 入参

**Files:**
- Modify: `pluginhub/api/admin/licenses.py`
- Test: `tests/test_admin_license_org.py`

- [ ] **Step 1: 入参 schema 加 org_id**

per spec §5.1 + §7.1：
- `plan = enterprise` AND `org_id IS NULL` → 422 enterprise-requires-org-id
- `plan ∈ {individual, pro}` AND `org_id IS NOT NULL` → 422 individual-license-cannot-have-org
- `org_id` regex `^[a-z0-9][a-z0-9-]{1,63}$` → 422 invalid-org-id-format
- org_id 唯一性校验（spec §11 R6）

- [ ] **Step 2: JWT claim 写 org_id**

License.create 时把 `org_id` 写进 JWT claims（per spec §5.3）。

- [ ] **Step 3: 测试 4 个边界**

- 缺 org_id → 422
- 错填 plan + org_id 组合 → 422
- org_id unicode/emoji → 422
- org_id 重复 → 409 org-id-already-exists

**Acceptance:** 4 个边界全过；admin web UI 显示二次确认（per R7 缓解）。

### Task D2.4: attune-pro plugin yaml 全部加 visibility=pro

**Files (attune-pro 仓):**
- Modify: `plugins/*/plugin.yaml`（~6 个 plugin）

- [ ] **Step 1: 列出所有 plugin**

`find attune-pro/plugins -name 'plugin.yaml'` → 预计 6 个（law-pro / tech-pro / patent-pro / presales-pro / medical-pro / academic-pro 任意子集）。

- [ ] **Step 2: 每个 yaml 加两字段**

```yaml
# 新增
visibility: pro
org_id: null
```

- [ ] **Step 3: 发布 script 兜底**

新建 `attune-pro/scripts/publish-to-pluginhub.sh`（per spec §6.1）：
- 读 plugin.yaml；若无 `visibility` 字段 → 按 `min_plan` 映射
- POST 到 pluginhub admin API
- 鉴权用 attune-pro CI secrets

**Acceptance:** 所有 yaml 含 visibility=pro；publish script dry-run 在 staging pluginhub 上能注册 plugin。

### Task D2.5: lawcontrol sample enterprise plugin 注册

**Files (lawcontrol 仓，仅元信息，不动业务代码):**
- New: `plugins/skills/contract_review/plugin.yaml`（仅 manifest，源码原地不动）

- [ ] **Step 1: 选一个 sample plugin**

挑 lawcontrol `plugins/skills/contract_review` 作为 v1 sample（其他 enterprise plugin 推 v1.1）。

- [ ] **Step 2: 写 plugin.yaml**

```yaml
id: contract-review-enterprise
version: 0.1.0-alpha
visibility: enterprise
min_plan: enterprise
org_id: acme-law-firm  # 示例 org，prod 由各客户分配
description: 合同审查 enterprise plugin（lawcontrol 原生迁移）
```

- [ ] **Step 3: 通过 publish-to-pluginhub.sh 发布到 staging**

```bash
cd /data/company/project/attune-enterprise  # 注意：D5 才改名，D2 仍是 lawcontrol
bash scripts/publish-to-pluginhub.sh plugins/skills/contract_review staging
```

**Acceptance:** staging pluginhub 上有该 plugin；enterprise license（org_id=acme-law-firm）能拉到；其他 org 拿不到（spec §9.1 fixture `08-enterprise-mismatch-org`）。

### D2 Verification

- [ ] 全部 D1 + D2 测试合计 ≥30 case 全过
- [ ] staging pluginhub e2e：admin 创建 enterprise license → attune CLI 用该 license list → install → SHA256 校验通过 → 心跳上报
- [ ] **Commit C3 + C4 + C5**
- [ ] dev push（staging only，**不发 prod pluginhub**）

---

## Phase D3 (5/25) — GA freeze, pluginhub 不动

目标：**GA day 不动 pluginhub，不动 lawcontrol rename**。只 tag `v1.0.0` + `desktop-v1.0.0`（attune 仓），develop → main `--no-ff` merge。本 phase 的所有改动**前置在 D1+D2 完成**，D3 只做发版动作。

### Task D3.1: GA freeze check

- [ ] **Step 1: 确认 attune 仓 develop 通过所有 CI**
- [ ] **Step 2: develop → main `--no-ff` merge**

```bash
cd /data/company/project/attune
git checkout main
git merge --no-ff develop -m "merge: develop → main (v1.0.0 GA)"
git tag -a v1.0.0 -m "v1.0.0 GA"
git tag -a desktop-v1.0.0 -m "desktop-v1.0.0 GA"
git push origin main v1.0.0 desktop-v1.0.0
```

- [ ] **Step 3: attune-pro 配对 tag `v1.0.0`**
- [ ] **Step 4: cloud `cloud-v2.2.0`**（声明兼容 attune v1.0.x）

**Acceptance:** 三仓 tag 同号；`git log origin/main --first-parent` 末尾纯 `merge:` 前缀。

### D3 Verification

- [ ] D3 不引入新代码改动；仅 tag + merge
- [ ] pluginhub 与 lawcontrol 各自仓 develop **不动**（GA freeze）
- [ ] **Commit C 数：0**（D3 仅有 merge commit + tag）

---

## Phase D4 (5/26) — 上架日 + pluginhub v1.0 GA

目标：cloud / wiki-web / official-web 三件套上架；pluginhub 也走 v1.0 GA（发 prod）；lawcontrol 仓**只做远端引用更新**（GitHub remote 早已改名，本地暂保留）。

### Task D4.1: pluginhub prod migration

**Files (attune-pluginhub):**
- Run: `alembic upgrade head` on prod DB（per spec §11 R2 缓解）
- Run: backfill SQL

- [ ] **Step 1: pg_dump prod 全量备份**

```bash
pg_dump -U pluginhub_user pluginhub_prod > /backups/pluginhub_prod_pre_v1_$(date +%s).sql
```

存放 `/data/company/backups/`，验证可 restore。

- [ ] **Step 2: staging dry-run（再次）**

```bash
psql -U pluginhub_user pluginhub_staging -c "BEGIN; ALTER ...; ROLLBACK;"
```

- [ ] **Step 3: prod migration window（15 分钟）**

```bash
alembic upgrade head
psql -f backfill_visibility.sql
# 验证
psql -c "SELECT visibility, COUNT(*) FROM plugin GROUP BY visibility;"
```

- [ ] **Step 4: pluginhub tag v1.0.0**

```bash
cd /data/company/project/attune-pluginhub
git tag -a v1.0.0 -m "pluginhub v1.0 GA — unified visibility/org_id"
git push origin v1.0.0
```

**Acceptance:** prod DB 含新字段；`SELECT visibility, COUNT(*)` 显示 public/pro/enterprise 都有数据；旧 client 调 `/index.json` 仍返 200。

### Task D4.2: cloud / wiki / official-web 上架

**Files (attune-cloud):**
- 走 cloud 自身的 `cloud-v2.2.0` tag 部署流程（本 plan 不展开 cloud 部署细节）

- [ ] **Step 1: cloud accounts service 部署**
- [ ] **Step 2: wiki-web 部署**
- [ ] **Step 3: official-web 部署**（**注意**：products.yaml 中 `slug: lawcontrol` D4 暂留，D5 才改 + 加 301 redirect）
- [ ] **Step 4: 三件套 healthcheck 全绿**

**Acceptance:** cloud accounts / wiki-web / official-web 三个 health endpoint 返 200。

### Task D4.3: lawcontrol 仓状态确认（不动代码）

- [ ] **Step 1: 确认 GitHub remote `github` 已是 `qiurui144/attune-enterprise.git`**（per inventory §0，已完成）
- [ ] **Step 2: 老 GitHub 仓 `qiurui144/lawcontrol` 已自动 301 redirect**（GitHub native，无需配置）
- [ ] **Step 3: 内网 Gitea origin 仍是旧名**（D5 改）

**Acceptance:** D4 lawcontrol 仓物理改动 = 0；GitHub side 已就绪。

### D4 Verification

- [ ] pluginhub prod migration 成功；旧 client 行为不变
- [ ] cloud 三件套上架 healthcheck 绿
- [ ] **Commit C6**（pluginhub v1.0 tag，不算 code commit）
- [ ] 复盘 D1-D4：scope 是否漂移？任何顺手做的扩 → 推 v1.1（per spec §11 R10）

---

## Phase D5 (5/27) — 全量 rename（代码 / image / submodule / Gitea / 本地 dir）

目标：把 `lawcontrol` 字符串从代码 / docker image / submodule / Gitea origin / 本地目录全部改成 `attune-enterprise`。**保留 DB 名 `lawcontrol`**（DB rename 在 D6 hotfix window 做）。同步改 4 跨仓引用。

### Task D5.1: 仓内代码 grep-replace

**Files (lawcontrol → attune-enterprise 仓，~60 文件，~780 行):**

- [ ] **Step 1: pre-rename git tag**

```bash
cd /data/company/project/attune-enterprise
git tag -a pre-rename-2026-05-27 -m "snapshot before lawcontrol → attune-enterprise rename"
git push github pre-rename-2026-05-27
```

回滚锚点（per R1 缓解）。

- [ ] **Step 2: 跑 sed 批量替换（按 inventory §F Step 3）**

```bash
find . -type f \( -name "*.py" -o -name "*.yml" -o -name "*.yaml" -o -name "*.sh" \
                   -o -name "*.md" -o -name "*.json" -o -name "*.toml" \) \
  -not -path "./.git/*" -not -path "./.venv/*" -not -path "./frontend/node_modules/*" \
  | xargs sed -i \
    -e 's/lawcontrol-backend/attune-enterprise-backend/g' \
    -e 's/lawcontrol-audio/attune-enterprise-audio/g' \
    -e 's/lawcontrol-doc/attune-enterprise-doc/g' \
    -e 's/lawcontrol-reranker/attune-enterprise-reranker/g' \
    -e 's/lawcontrol-pipeline/attune-enterprise-pipeline/g' \
    -e 's/lawcontrol-frontend-dev/attune-enterprise-frontend-dev/g' \
    -e 's/lawcontrol-playwright-manager/attune-enterprise-playwright-manager/g' \
    -e 's/LAWCONTROL_API_KEY/ATTUNE_ENTERPRISE_API_KEY/g' \
    -e 's/LAWCONTROL_API_BASE/ATTUNE_ENTERPRISE_API_BASE/g' \
    -e 's/LAWCONTROL_GIT_REPO/ATTUNE_ENTERPRISE_GIT_REPO/g' \
    -e 's/admin@lawcontrol\.local/admin@attune-enterprise.local/g' \
    -e 's/backend@lawcontrol\.local/backend@attune-enterprise.local/g' \
    -e 's/lawcontrol\.internal/attune-enterprise.internal/g'
```

**注意**：**不替换** `APP_DB_NAME=lawcontrol`（D6 才改），不替换 `name = "lawcontrol-backend"` package name（避免 dist-info 冲突，D5 末尾 pip reinstall 自动更新）。

- [ ] **Step 3: LawControl → Attune Enterprise 品牌名**

```bash
find . -type f ... | xargs sed -i 's/LawControl/Attune Enterprise/g'
```

- [ ] **Step 4: 手工 review 注释中的 lawcontrol**

```bash
grep -rn "lawcontrol" . --include="*.py" --include="*.md" --include="*.yml" | grep -v "\.git/" | grep -v "\.venv/"
```

逐行 review：业务术语 / 历史 incident 引用 / git log 写法 → 保留；命名引用 → 改。

**Acceptance:** `grep -rn "lawcontrol"` 残留 < 5 行（仅注释 / 历史引用）；DB 名相关引用保留。

### Task D5.2: Docker image rebuild + retag

**Files (lawcontrol → attune-enterprise 仓):**
- Build: 7 个 image 重新 build 并 push

- [ ] **Step 1: rebuild 7 images**

```bash
docker-compose build  # 用新 image name
docker tag attune-enterprise-backend:latest registry/attune-enterprise-backend:1.0.0
# ... 7 个 image
```

- [ ] **Step 2: 保留旧 tag 30 天（per spec §10.2）**

旧 image `lawcontrol-*:latest` 不删，30 天 grace period。

**Acceptance:** 7 个新 image 都在 registry；compose up 拉到新 image 启动成功。

### Task D5.3: 跨仓引用 update

**Files (attune):**
- `CLAUDE.md` 10 处
- `docs/oss-pro-strategy.md` 10+ 处
- `docs/adr/0001-oss-pro-boundary.md` 6 处
- `docs/specs/memory-moat-v07.md` 2 处
- `docs/TESTING.md` 1 处
- `docs/v1.0-product-materials.md` 6 处
- `docs/k3-ai-service/README.md` + `K3_AI_SERVICE_DEPLOY.md` 2 处

**Files (attune-pro):**
- `CLAUDE.md` 6 处
- `INTEGRATION.md` 2 处
- `README.zh.md` 6 处
- `plugins/law-pro/src/lib.rs` 1 处（注释）
- `plugins/law-pro/src/bin/run_golden_qa.rs` 2 处
- `plugins/law-pro/src/bin/run_evidence_classify.rs` 1 处
- `plugins/law-pro/tests/quality_scorer.rs` 4 处
- **`plugins/law-pro/tests/bank_aggregator_test.rs` 1 处（hardcoded 绝对路径 `/data/company/project/attune-enterprise/data/test_evidence/任其坤-梁素燕`，必须先改）**
- `plugins/law-pro/tests/vision_quality_scorer.rs` 1 处
- `plugins/law-pro/tests/lawcontrol_compat/` 目录 → `mv` 为 `attune_enterprise_compat/`（更新所有引用）

**Files (cloud):**
- `CLAUDE.md` 1 处（symlink 路径）
- `ARCHITECTURE.md` 8 处
- `secrets/README.md` 1 处
- `official-web/THEME_INTERFACE.md` 1 处
- `official-web/tests/backend/test_content_contract.py` 2 处
- `official-web/content/README.md` 3 处
- `official-web/content/solutions.yaml` 3 处
- `official-web/content/homepage.yaml` 5 处
- `official-web/content/pages.yaml` 3 处
- `official-web/content/products.yaml` 含 slug
- `official-web/content/branding.yaml` 待检
- `official-web/content/blog.yaml` 待检

**Files (全局):**
- `/home/qiurui/.claude/CLAUDE.md` 行 713

- [ ] **Step 1: attune-pro 绝对路径优先改**

`plugins/law-pro/tests/bank_aggregator_test.rs:72` 把 `/data/company/project/attune-enterprise/` 改成 `/data/company/project/attune-enterprise/`，否则本地 dir mv 后该测试立即 broken。

- [ ] **Step 2: attune-pro `tests/lawcontrol_compat/` mv**

```bash
cd /data/company/project/attune-pro/plugins/law-pro/tests
git mv lawcontrol_compat attune_enterprise_compat
# 更新所有引用该目录的 Rust 代码
grep -rln "lawcontrol_compat" .. | xargs sed -i 's/lawcontrol_compat/attune_enterprise_compat/g'
```

- [ ] **Step 3: 跑跨仓 sed**

```bash
# attune 仓
cd /data/company/project/attune
grep -rln "lawcontrol" CLAUDE.md docs/ | xargs sed -i \
  -e 's/lawcontrol/attune-enterprise/g' \
  -e 's/LawControl/Attune Enterprise/g'

# attune-pro 仓（同上）
# cloud 仓（同上）

# 全局 CLAUDE.md（line 713 一处）
sed -i 's/- lawcontrol(B2B SaaS)/- attune-enterprise(B2B SaaS, 原 lawcontrol)/' /home/qiurui/.claude/CLAUDE.md
```

**Acceptance:** 4 跨仓 + 全局 CLAUDE.md `grep -rn "lawcontrol"` 残留 0（除历史 / git log / 故意保留的合理 context）。

### Task D5.4: official-web slug 改名 + 301 redirect

**Files (cloud/official-web):**
- Modify: `content/products.yaml`（`slug: lawcontrol` → `slug: attune-enterprise`）
- New: `content/redirects.yaml` 或 nginx config 配 `/products/lawcontrol` → 301 → `/products/attune-enterprise`

- [ ] **Step 1: 改 slug**
- [ ] **Step 2: 配 301**
- [ ] **Step 3: 测试老 URL 301 到新 URL**

**Acceptance:** 老 URL `https://engi-stack.com/products/lawcontrol` 返 301，跳到 `/products/attune-enterprise`；新 URL 内容正确。

### Task D5.5: Submodule 重定向 + Gitea rename + 本地 dir mv

- [ ] **Step 1: cloud/pluginhub submodule 路径**

```bash
cd /data/company/cloud
git submodule deinit pluginhub
git submodule add <new-pluginhub-url> pluginhub  # 或 update .gitmodules URL
git commit -am "chore: pluginhub submodule path update post-rename"
```

- [ ] **Step 2: Gitea 内网仓 rename**

在 Gitea web UI `http://qiurui-114.goho.co:3000/qiurui/working-ai-control` settings rename 为 `attune-enterprise`。

```bash
cd /data/company/project/attune-enterprise
git remote set-url origin http://qiurui-114.goho.co:3000/qiurui/attune-enterprise.git
git remote -v  # 验证
```

- [ ] **Step 3: 本地 dir mv**

```bash
cd /data/company/project
mv lawcontrol attune-enterprise
# 验证 cloud symlink
ls -la /data/company/cloud/pluginhub  # 应该跟着新路径
```

**Acceptance:** 本地 `/data/company/project/attune-enterprise/` 存在；`git remote -v` 显示新 URL；cloud pluginhub submodule 正常。

### Task D5.6: Memory 迁移

**Files:**
- `/home/qiurui/.claude/projects/-data-company-project-attune/memory/MEMORY.md`（含 lawcontrol 引用上下文）
- 检查：`/home/qiurui/.claude/projects/-data-company-project-attune-pro/memory/`

- [ ] **Step 1: grep + replace**

```bash
grep -rln "lawcontrol" /home/qiurui/.claude/projects/ | xargs sed -i 's/lawcontrol/attune-enterprise/g'
```

注意：**保留**「2026-04 lawcontrol design borrowed plugin.yaml pattern」等历史引用（合理 context）。

- [ ] **Step 2: 无 `-data-company-project-lawcontrol/` 独立目录**（per inventory §C，无需 mv）

- [ ] **Step 3: 抽检 AI prompt**

冷启动一次 Claude，问「attune-enterprise 是什么」→ 应回答正确含义；问「lawcontrol」→ 应能回忆是旧名。

**Acceptance:** memory grep 0 残留（除历史引用）；AI 上下文 sanity check 过。

### D5 Verification

- [ ] 仓内 `grep -rn "lawcontrol"` 残留 ≤ 5 行（历史 / git log / 合理保留）
- [ ] 4 跨仓引用全改完
- [ ] 7 docker image 都新名 + push 完
- [ ] 本地 dir mv 完；cloud symlink 正常
- [ ] **Commit C7 + C8 + C9 + C10 + C11**
- [ ] push lawcontrol 远端（github + 新 gitea）
- [ ] push attune / attune-pro / cloud 各自远端

---

## Phase D6 (5/28) — v1.0.1 hotfix：DB rename downtime window

目标：把 PostgreSQL DB 名 `lawcontrol` → `attune_enterprise`。**唯一 downtime window，停服 ≤ 5 min**。先备份再动，可回滚。Tag attune-enterprise `v1.0.1`。

### Task D6.1: Pre-rename 备份

- [ ] **Step 1: pg_dump 全量备份**

```bash
ssh enterprise-prod
pg_dump -U postgres -Fc lawcontrol > /backups/lawcontrol_v1_pre_rename_$(date +%s).dump
pg_dump -U postgres lawcontrol > /backups/lawcontrol_v1_pre_rename_$(date +%s).sql
# 验证 restore（在 staging）
pg_restore -U postgres -d lawcontrol_restore_test /backups/lawcontrol_v1_pre_rename_<ts>.dump
```

- [ ] **Step 2: 客户通告（若有外部客户）**

预定 5/28 00:00 - 00:05（或非业务时段）downtime window，邮件/IM 通告。

**Acceptance:** 备份文件存在；restore 到 staging DB 验证数据一致；通告发出。

### Task D6.2: ALTER DATABASE 重命名

- [ ] **Step 1: 停服**

```bash
cd /data/company/project/attune-enterprise
docker-compose stop backend qcluster scheduler  # 所有连 lawcontrol DB 的服务
```

- [ ] **Step 2: kill 残留连接**

```sql
SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = 'lawcontrol';
```

- [ ] **Step 3: ALTER DATABASE**

```sql
ALTER DATABASE lawcontrol RENAME TO attune_enterprise;
```

- [ ] **Step 4: 更新 .env + docker-compose**

```bash
sed -i 's/APP_DB_NAME=lawcontrol/APP_DB_NAME=attune_enterprise/' .env
sed -i 's/APP_DB_NAME=lawcontrol/APP_DB_NAME=attune_enterprise/' .env.example
sed -i 's/APP_DB_NAME=lawcontrol/APP_DB_NAME=attune_enterprise/' docker-compose.yml
```

- [ ] **Step 5: 启服 + smoke test**

```bash
docker-compose up -d backend qcluster scheduler
# wait healthcheck
docker-compose logs --tail=50 backend
# smoke: 跑 Django 几个 endpoint
curl http://localhost:8000/api/health
curl http://localhost:8000/api/cases?limit=1  # 验证 DB 连得上
```

**Acceptance:** Downtime ≤ 5 min；smoke test 5 个 endpoint 全绿；DB 名 `attune_enterprise` 在 `\l` 列表里。

### Task D6.3: Rollback drill（dry-run，不真执行除非失败）

- [ ] **Step 1: rollback SQL 准备好**

```sql
-- 回滚（如果 D6.2 失败）
ALTER DATABASE attune_enterprise RENAME TO lawcontrol;
```

- [ ] **Step 2: rollback .env**

`git revert <APP_DB_NAME commit>`

**Acceptance:** rollback 步骤文档化；非 emergency 不执行。

### Task D6.4: v1.0.1 tag

```bash
cd /data/company/project/attune-enterprise
git tag -a v1.0.1 -m "v1.0.1 hotfix — DB rename lawcontrol → attune_enterprise"
git push github v1.0.1
git push origin v1.0.1
```

**Acceptance:** v1.0.1 tag 在两个 remote 上都有。

### D6 Verification

- [ ] DB rename 成功；smoke test 全绿
- [ ] **Commit C10 + (tag commit)**
- [ ] downtime ≤ 5 min（用 `date` 记录 stop / start 时间戳）

---

## Phase D7 (5/29-30) — 清理 + 验证 + sign-off

目标：grep 全 0 残留（除合理保留）；6 类下限测试全覆盖；GA + 上架验收清单全勾；post-mortem 归档。

### Task D7.1: 全量 grep 残留扫描

- [ ] **Step 1: 5 仓 grep**

```bash
for repo in /data/company/project/attune /data/company/project/attune-pro \
            /data/company/project/attune-enterprise /data/company/project/attune-pluginhub \
            /data/company/cloud; do
  echo "=== $repo ==="
  grep -rn "lawcontrol" "$repo" --include="*.py" --include="*.md" \
       --include="*.yml" --include="*.yaml" --include="*.sh" --include="*.rs" \
       --include="*.ts" --include="*.tsx" --include="*.json" --include="*.toml" \
       | grep -v "\.git/" | grep -v "\.venv/" | grep -v "node_modules/" \
       | grep -v "# 历史"  | grep -v "# legacy"
done
```

- [ ] **Step 2: 全局 CLAUDE.md + memory**

```bash
grep -n "lawcontrol" /home/qiurui/.claude/CLAUDE.md
grep -rn "lawcontrol" /home/qiurui/.claude/projects/
```

**Acceptance:** 总残留 < 10 行，全部是合理的历史 / git log 引用，无任何 active code path 含 `lawcontrol`。

### Task D7.2: 6 类下限测试全覆盖（per Agent 验证铁律）

per 项目 CLAUDE.md「Agent 验证铁律」+ spec §9：

| 类 | 下限 | 当前覆盖 | 补齐 |
|----|------|---------|------|
| Golden case | ≥10 | 12 fixture（spec §9.1） | ✅ |
| 属性测试 | ≥3 | 3 prop（spec §9.2） | ✅ |
| 边界 case | ≥5 | 5 boundary（spec §9.3） | ✅ |
| 异常 / 错误 | ≥3 | 3 error（spec §9.4） | ✅ |
| 集成 E2E | ≥1 | `test_pluginhub_full_flow.py` | ✅ |
| 回归 fixture | ≥1 | rename 4 R 回归（spec §9.7） | ✅ |

- [ ] **Step 1: 跑全套 pluginhub 测试**

```bash
cd /data/company/project/attune-pluginhub
pytest tests/ -v
```

- [ ] **Step 2: 跑 attune client e2e**

```bash
cd /data/company/project/attune
cargo test --test marketplace_e2e -- --nocapture
```

- [ ] **Step 3: 跑 attune-pro plugin loadable test**

```bash
cd /data/company/project/attune-pro
cargo test --test plugin_registry
```

**Acceptance:** 三仓测试 pass rate 100%；任何 fail 立即修不放过。

### Task D7.3: GA + 上架验收清单（per spec 附录 A）

- [ ] pluginhub `GET /api/v1/index.json?plan=enterprise&org_id=acme` 仅返 enterprise plugin（不漏 individual）
- [ ] personal user 不能 see enterprise plugin（强测，url 拼装也不行）
- [ ] enterprise A 不能拿 enterprise B plugin（org_id 隔离）
- [ ] 6 类下限测全过（D7.2）
- [ ] attune CLI plugin install → end-to-end 跑通
- [ ] attune-enterprise 原生功能（卷宗 / RPA / Intent Router）不 break
- [ ] 跨仓 grep 0 残留 lawcontrol（除 legacy 注释，per D7.1）
- [ ] cloud 三件套 healthcheck 绿
- [ ] DB rename hotfix downtime ≤ 5 min
- [ ] 4 个红色风险（R1 / R2 / R5 / R7）mitigation 全执行

### Task D7.4: Post-mortem

写在 attune-enterprise RELEASE.md v1.0.1 节（**不开独立 .md**，per CLAUDE.md「文档体系铁律」）：

```markdown
## v1.0.1 (2026-05-28)

### Changes
- 改名 lawcontrol → attune-enterprise（仓 / image / submodule / 本地 dir / 跨仓引用 / memory）
- PostgreSQL DB 名 `lawcontrol` → `attune_enterprise`（hotfix downtime 5 min）
- 接入 attune-pluginhub v1.0：enterprise plugin 通过 pluginhub 分发 + license + org_id 隔离

### Migration
旧客户：
1. `docker-compose pull`（拉新 image tag）
2. 把 `APP_DB_NAME=lawcontrol` 改为 `attune_enterprise`
3. 重启服务

### Post-mortem
- D1-D2 pluginhub schema + API 改造 non-disruptive，0 incident
- D5 全量 rename 落地，sed 批量替换无 false positive
- D6 DB rename 实际 downtime X 分钟（vs 预期 ≤ 5 min）
- 改善：下次类似 rename 走 staged blue-green，downtime 可压到 0
```

- [ ] **Commit C12**：cleanup + post-mortem

**Acceptance:** RELEASE.md v1.0.1 节落地；attune CLAUDE.md「三产品矩阵」节品牌名已改。

### D7 Verification

- [ ] D1-D6 所有验收勾全部 ✅
- [ ] Post-mortem 入 RELEASE.md
- [ ] **三仓 sign-off**：attune-pluginhub v1.0 / attune-enterprise v1.0.1 / attune v1.0.0 + 配套

---

## 12 Commit 分批清单

每个 commit ≤ 200 行（除 sed 批量改），独立可回滚。

| ID | 仓 | Commit msg | 文件清单 | 行数估算 | Phase |
|----|----|-----------|---------|---------|-------|
| **C1** | attune-pluginhub | `feat(schema): add Plugin.visibility / Plugin.org_id / License.org_id (migration)` | `alembic/versions/2026_05_23_*.py` + `pluginhub/models.py` + `tests/test_migration_visibility.py` | ~150 行 | D1 |
| **C2** | attune-pluginhub | `fix(jwt): read License.plan instead of max_installs heuristic` | `pluginhub/auth/jwt_decoder.py` + `tests/test_jwt_tier.py` | ~80 行 | D1 |
| **C3** | attune-pluginhub | `feat(api): visibility / org_id filter in index + download endpoints` | `pluginhub/api/index.py` + `pluginhub/api/download.py` + `pluginhub/auth/visibility_filter.py` + `tests/test_index_visibility.py` + `tests/test_download_visibility.py` + `tests/test_security_visibility.py` + `tests/golden/license-plugin-matrix/*.yaml` (12 fixture) | ~400 行 | D2 |
| **C4** | attune-pro | `chore(plugins): mark all plugins visibility=pro + add publish-to-pluginhub script` | `plugins/*/plugin.yaml` (~6 files) + `scripts/publish-to-pluginhub.sh` | ~80 行 | D2 |
| **C5** | lawcontrol (D2) | `feat(plugin): contract_review enterprise plugin manifest` | `plugins/skills/contract_review/plugin.yaml` | ~20 行 | D2 |
| **C6** | attune-pluginhub | `feat(admin): org_id required for enterprise license + audit log` | `pluginhub/api/admin/licenses.py` + `tests/test_admin_license_org.py` | ~150 行 | D2 |
| **C7** | lawcontrol → attune-enterprise | `chore(rename): lawcontrol → attune-enterprise in code / configs / images (sed batch)` | ~60 文件 / ~780 行（sed 批量） | ~780 行 | D5 |
| **C8** | attune | `docs(rename): update CLAUDE.md / oss-pro-strategy / ADR for attune-enterprise` | `CLAUDE.md` + `docs/oss-pro-strategy.md` + `docs/adr/0001-*.md` + `docs/specs/memory-moat-v07.md` + `docs/TESTING.md` + `docs/v1.0-product-materials.md` + `docs/k3-ai-service/*.md` | ~50 行 | D5 |
| **C9** | attune-pro | `chore(rename): law-pro absolute path + lawcontrol_compat dir + docs` | `plugins/law-pro/tests/bank_aggregator_test.rs` + `plugins/law-pro/tests/attune_enterprise_compat/` (`git mv`) + `CLAUDE.md` + `INTEGRATION.md` + `README.zh.md` + `plugins/law-pro/src/lib.rs` + `plugins/law-pro/src/bin/*.rs` + `plugins/law-pro/tests/*.rs` | ~80 行 | D5 |
| **C10** | cloud | `chore(rename): pluginhub submodule + ARCHITECTURE + official-web slug + 301 redirect` | `CLAUDE.md` + `ARCHITECTURE.md` + `secrets/README.md` + `official-web/THEME_INTERFACE.md` + `official-web/tests/backend/test_content_contract.py` + `official-web/content/*.yaml` + redirect config | ~120 行 | D5 |
| **C11** | attune-enterprise (D6) | `fix(db): rename APP_DB_NAME lawcontrol → attune_enterprise (v1.0.1 hotfix)` | `.env` + `.env.example` + `docker-compose.yml` + `RELEASE.md` (v1.0.1 节) | ~30 行 | D6 |
| **C12** | attune-enterprise | `chore(cleanup): post-rename verify + post-mortem` | `RELEASE.md` (post-mortem 段) | ~50 行 | D7 |

**额外 commit（不计 C 编号，正常 release 流程）:**
- attune merge: `merge: develop → main (v1.0.0 GA)` (D3)
- attune-pluginhub tag commit `v1.0.0` (D4)
- attune-enterprise tag commit `pre-rename-2026-05-27` (D5)
- attune-enterprise tag commit `v1.0.1` (D6)
- 全局 CLAUDE.md sed `line 713` 改名（per inventory §B 全局 CLAUDE.md 部分） — 不在 git 仓内，本地 sed

---

## 风险登记 + Rollback Matrix

per spec §11，12 个 risk，每个标 mitigation + rollback。红色 4 个（R1 / R2 / R5 / R7）单独 task 化。

| ID | Risk | Mitigation | Rollback step | 责任 |
|----|------|-----------|---------------|------|
| **R1** 🔴 | lawcontrol 改名破坏现有客户部署 | (1) D5 pre-rename git tag; (2) 30 天 grace docker tag alias; (3) GitHub native 301; (4) Gitea rename 后保留 alias 30 天 | (a) `git reset --hard pre-rename-2026-05-27`; (b) docker re-tag 旧 image 重发; (c) Gitea web UI rename 回旧名 | DevOps |
| **R2** 🔴 | pluginhub schema migration 破生产数据 | (1) D1 staging dry-run; (2) D4 prod 跑前 pg_dump; (3) downgrade migration 可执行; (4) BEGIN/ROLLBACK staging 验证 | (a) `alembic downgrade -1`; (b) 若数据已 backfill 错 → restore from pg_dump | pluginhub 维护者 |
| **R3** 🟡 | enterprise plugin 抽出后 lawcontrol 现有功能 break | (1) **不动业务代码**，仅 plugin.yaml 注册元信息; (2) lawcontrol 内部 import 路径保持不变; (3) pluginhub 是分发管道非 SoT | (a) 删 plugin.yaml（回到「不发布」状态），不影响 lawcontrol 本地运行 | attune-enterprise 维护者 |
| **R4** 🟡 | CLAUDE.md / memory 迁移漏文件 | (1) D7.1 5 仓全量 grep; (2) case-insensitive `grep -irln`; (3) AI 抽检 prompt; (4) 跑两遍 verify | (a) 漏改文件再跑一次 sed; (b) memory 错改 → 从 `~/.claude/projects/*.bak` 恢复 | AI 工作流维护者 |
| **R5** 🔴 | individual user 拼 url 越权访问 enterprise plugin | (1) D2.2 服务端重跑 filter; (2) presigned URL 5 min 短时效; (3) S3 bucket 不允许 public list; (4) 3 个安全测试 case 全覆盖 | (a) 若发现越权 → 立即 yanked 该 plugin + audit; (b) rotate S3 bucket presign 密钥 | pluginhub 维护者 |
| **R6** 🟡 | org_id 命名冲突（两家 ACME） | (1) admin 创建 license 时唯一性校验; (2) 命名建议「company-suffix」; (3) admin UI 含告警 | (a) rename org_id（涉及客户通告 + license re-issue） | pluginhub admin |
| **R7** 🔴 | enterprise plugin org_id 误配（数据泄漏） | (1) admin UI 二次确认; (2) audit log 记录每次改动; (3) 客户端 install 显示「来源 org_id」 | (a) yanked 错配 plugin; (b) 通告影响客户; (c) audit log 取证 | pluginhub admin |
| **R8** 🟡 | 老 license JWT 无 org_id 误判 | (1) C2 fallback 严格 AND; (2) fixture `08-enterprise-mismatch-org` 覆盖 | (a) re-issue license with 显式 `org_id` | pluginhub 维护者 |
| **R9** 🟢 | DNS / docker rename 期间 client cache 旧地址 | (1) client endpoint 走 config 文件; (2) auto-update push 新 config | (a) 用户手动改 config; (b) hardcode 兜底 30 天 | client 维护者 |
| **R10** 🟡 | scope 蠕变 | 拒绝任何顺手扩，推 v1.1 新 spec | (a) revert 顺手 commit | spec 评审 owner |
| **R11** 🟢 | OSS attune 用户误装 enterprise plugin | 服务端过滤后 client 看不到 enterprise plugin（path A 数据流） | N/A（不出现该 case） | pluginhub 维护者 |
| **R12** 🟢 | 双仓 publish script 重复维护 | 抽公共 logic 到 `attune-pluginhub/scripts/cli.py` | (a) DRY 重构 | attune-pluginhub 维护者 |

### R5 红色风险单独 task 化

**Task R5-mitigation: download 安全测试**

`tests/test_security_visibility.py` 必含：
- individual user 拼 enterprise plugin url → 403
- pro user 拼 enterprise plugin url → 403
- enterprise A 拼 enterprise B url → 403
- presign URL > 5 min 拒绝
- audit log 记录每次越权尝试

**Task R7-mitigation: admin UI 二次确认 + audit**

- admin 改 plugin.visibility 或 plugin.org_id 时弹「确认」对话框（显示前后值）
- audit_log 表插记录（actor / target / before / after / timestamp）
- 客户端首次 install 在 marketplace 详情页显示「来源 org_id: acme-law-firm」

---

## GA + 上架验收清单

per spec §9 + 附录 A，统一勾选式：

### pluginhub 正确性

- [ ] `GET /api/v1/index.json`（匿名）只返 visibility=public
- [ ] `GET /api/v1/index.json` Bearer pro license → 返 public + pro（不见 enterprise）
- [ ] `GET /api/v1/index.json` Bearer enterprise license（org=acme） → 返 public + pro + acme 的 enterprise（不见 beta 的 enterprise）
- [ ] `GET /api/v1/plugin/<enterprise-id>/download` 用 pro license → 403 `plugin-visibility-mismatch`
- [ ] `GET /api/v1/plugin/<enterprise-id>/download` 用错 org_id enterprise license → 403 `plugin-visibility-mismatch`
- [ ] `POST /api/v1/admin/licenses` enterprise plan 缺 org_id → 422 `enterprise-requires-org-id`
- [ ] `POST /api/v1/admin/licenses` individual plan 含 org_id → 422 `individual-license-cannot-have-org`
- [ ] `POST /api/v1/admin/licenses` org_id unicode → 422 `invalid-org-id-format`
- [ ] 心跳上报 200；license revoke 后 403 `license-revoked`

### attune client 集成

- [ ] OSS attune（无 license） marketplace 显示 public plugin
- [ ] attune-pro user（pro license）marketplace 显示 public + pro plugin
- [ ] attune-enterprise user（enterprise license + org_id）marketplace 显示 public + pro + 自家 enterprise plugin
- [ ] `attune plugin install <id>` 下载 + SHA256 校验 + 解压成功
- [ ] 老 client 行为不变（plugin index 多字段 serde 容错）

### attune-enterprise 改名

- [ ] 仓内 grep `lawcontrol` 残留 ≤ 5（合理保留）
- [ ] 4 跨仓 grep 残留 ≤ 10（合理保留）
- [ ] 7 docker image 都新名 + push
- [ ] cloud official-web 旧 URL 301 → 新 URL
- [ ] Gitea 内网仓 rename 完
- [ ] 本地 dir `/data/company/project/attune-enterprise/` 存在
- [ ] DB 名 `attune_enterprise`（D6 后）
- [ ] attune-enterprise 原生功能（卷宗 / RPA / Intent Router）smoke test 全绿
- [ ] **6 类下限测试 pass rate 100%**（per Agent 验证铁律）

### 红色风险 mitigation

- [ ] R1 pre-rename git tag 存在 + 推到远端
- [ ] R2 prod pg_dump 备份 + restore 验证
- [ ] R5 3 安全测试 case 全过
- [ ] R7 admin UI 二次确认 + audit_log 表有数据

### 上架

- [ ] cloud accounts service health 绿
- [ ] wiki-web health 绿
- [ ] official-web health 绿（含 301 redirect 老 URL）

### 时间窗口

- [ ] D3 GA day pluginhub + lawcontrol 不动（freeze 守住）
- [ ] D6 DB rename downtime ≤ 5 min（wall-clock 用 `date` 记录）

---

## 测试矩阵（per Agent 验证铁律 6 类下限）

| 类型 | 下限 | 实现位置 | 覆盖文件 / fixture |
|------|------|---------|------------------|
| **Golden case** | ≥10 | `attune-pluginhub/tests/golden/license-plugin-matrix/` | `01-anon-public.yaml` ~ `12-revoked-license.yaml`（12 个） |
| **Property test** | ≥3 | `attune-pluginhub/tests/test_visibility_proptest.py` | prop1 矩阵自洽 / prop2 org 反射 / prop3 白名单 AND |
| **Boundary** | ≥5 | `attune-pluginhub/tests/test_boundary.py` | B1 exp=now / B2 max_installs=0 / B3 allowed_plugins=[] / B4 plan case / B5 org_id unicode |
| **Error case** | ≥3 | `attune-pluginhub/tests/test_jwt_errors.py` | E1 sig 错 / E2 缺 plan / E3 缺 exp |
| **Integration E2E** | ≥1 | `attune-pluginhub/tests/e2e/test_pluginhub_full_flow.py` | docker-compose up → admin create license → upload plugin → CLI install → revoke → re-check |
| **Regression** | 每修一 bug + 1 | rename 4 R 回归 | R1 docker tag alias / R2 dns 301 / R3 db view alias / R4 grep 0 残留 |

合计 ≥ 24 case + 4 rename regression = 28 case 跑全套，pass rate 必 100%。

---

## 跨仓协调点

**强配对（必须同步）：**
- attune（OSS）v1.0.0 ↔ attune-pro v1.0.0（同号 tag，per CLAUDE.md「跨仓版本配对」）
- attune-pluginhub v1.0.0 单独 release（无强配对，但客户端兼容性必须验证）

**独立但需声明兼容范围：**
- cloud cloud-v2.2.0 RELEASE.md 必声明「兼容 attune v1.0.x」
- attune-enterprise v1.0.1 RELEASE.md 必声明「依赖 attune-pluginhub v1.0.0+」

**Submodule 链：**
- cloud/pluginhub → attune-pluginhub（D5 update submodule URL）
- cloud 上架前 verify submodule 指针正确

**Memory / CLAUDE.md：**
- 4 仓 CLAUDE.md（attune + attune-pro + attune-pluginhub + attune-enterprise + cloud）+ 1 全局 CLAUDE.md = 5 处同步
- 1 处 memory（attune memory MEMORY.md）
- D5 完成后 AI 冷启动 sanity check

**反向兼容（D4-D5 重叠窗口）：**
- D4 lawcontrol 仓本地名仍是 `lawcontrol/`，但 cloud 部署用新 image name → cloud 必须在 D5 dir mv 前先用 docker image registry path（不走本地 dir）
- 客户端老 `lawcontrol.example.com` DNS 301 → `enterprise.engi-stack.com` 保留 30 天

---

## 红线（违反即拒绝）

per CLAUDE.md「架构级别设计铁律」+ 用户指令：

- ❌ D3 GA day 动 pluginhub / lawcontrol（freeze 守住）
- ❌ 跳过 pg_dump 直接 ALTER DATABASE
- ❌ 跳过 staging dry-run 直接上 prod migration
- ❌ scope 蠕变（顺手做「跨 vertical plugin」「企业自管签名」等）
- ❌ commit > 200 行（除 sed 批量）
- ❌ 6 类下限测试不达标即放行
- ❌ DB rename downtime > 5 min（除非有正式 incident 报告 + 用户同意）
- ❌ AI 自主 `gh pr create`（per AI 推 PR 流程，attune / attune-pro / attune-enterprise / pluginhub / cloud 五仓都允许 push，但**不允许 AI 自主开 PR**）

---

## 后续 plan 锚点（v1.1+）

per spec §2.2 推后的 scope，下一轮 spec 起草触发：

- 跨 vertical pro plugin（1 license 解锁多 vertical）
- pluginhub SaaS 多租户化
- Plugin 编译 / 签名 / supply chain 加固
- attune-pro / attune-enterprise plugin SDK 统一
- Enterprise plugin 商业化定价 / billing
- Office helper 改造 + 其他 attune-core capability 接入 pluginhub

**Plan 落盘路径**：`docs/superpowers/plans/2026-06-XX-pluginhub-v11-*.md`

---

**Plan 起草完毕。等用户评审。禁止直接进 implementation。**
