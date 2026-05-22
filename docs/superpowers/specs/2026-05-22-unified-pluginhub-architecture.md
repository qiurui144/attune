# Unified Pluginhub 架构 spec — 三产品矩阵 + lawcontrol → attune-enterprise 改名

**日期**：2026-05-22
**状态**：DRAFT — 等用户评审，未进 implementation
**作者**：spec 起草 AI
**触发**：用户原话「把 attune-pluginhub 和 attune-pro（还有 lawcontrol 中的插件也要融入 pro 体系，这些都是付费插件，都通过 pluginhub 的下载），做好架构的处理。仓库处理好之后，lawcontrol 改名为 attune-enterprise，把相关 claude 和记忆也都做好迁移」
**关联**：全局 CLAUDE.md「架构级别设计铁律」，项目 CLAUDE.md「三产品矩阵 + 边界」

---

## 目录

- [节 1：目标定位](#1-目标定位)
- [节 2：范围边界](#2-范围边界)
- [节 3：架构数据流](#3-架构数据流)
- [节 4：模块边界](#4-模块边界)
- [节 5：API 契约](#5-api-契约)
- [节 6：扩展点 / 插件接口](#6-扩展点--插件接口)
- [节 7：错误处理 + 边界 case](#7-错误处理--边界-case)
- [节 8：成本契约](#8-成本契约)
- [节 9：测试矩阵](#9-测试矩阵)
- [节 10：向后兼容 / migration path](#10-向后兼容--migration-path)
- [节 11：风险登记](#11-风险登记)
- [附录 A：评审 checklist](#附录-a评审-checklist)
- [附录 B：后续 plan 锚点](#附录-b后续-plan-锚点)

---

## 1. 目标定位

### 1.1 用户痛点

当前三产品矩阵（attune OSS / attune-pro / lawcontrol）的付费 plugin 分发存在三组结构性矛盾：

| 矛盾 | 现状 | 后果 |
|------|------|------|
| **分发分散** | attune-pro plugin 通过 pluginhub 走 license + JWT 下发；lawcontrol plugin 嵌在 `lawcontrol/plugins/` 直接读文件系统，无 license 校验 | 同一团队两个产品两套分发逻辑，license / 审计 / 撤回机制不统一 |
| **付费体系断层** | pluginhub 已有 `License.plan = individual / pro / enterprise` 三档，但 enterprise 档无实际 plugin 路由进来；lawcontrol 内部 plugin 没有 enterprise tier 概念 | enterprise 用户付了 enterprise license 但拿不到 enterprise-only plugin（如 law_firm 行业包） |
| **产品定位不清晰** | 用户原话「lawcontrol 改名为 attune-enterprise」反映：lawcontrol 不是独立产品，而是 attune 矩阵的企业形态。但当前名字暗示「law-only B2B」，限制非律所企业场景 | 销售错位 + 客户认知偏差 + 后续扩 medical / patent / finance 企业版无名字空间 |

### 1.2 解决目标

1. **统一付费 plugin 分发管道**：attune-pro plugin（个人付费）+ attune-enterprise plugin（企业付费）共用 pluginhub backend，统一 license / 撤回 / 审计 / 心跳
2. **明示三档 visibility**：`public`（OSS 免费）/ `pro`（个人订阅）/ `enterprise`（企业 license + org_id 匹配）
3. **lawcontrol → attune-enterprise 改名**：明示产品矩阵地位，腾出 vertical 扩展空间（law 是 attune-enterprise 第一个垂直，后续 medical / patent / finance 同管道接入）

### 1.3 与产品 positioning 对齐

per 项目 CLAUDE.md「三产品矩阵 + 边界」：

- **attune (OSS)**：零行业绑定，免费，pluginhub 公开端点拿 `visibility=public` plugin
- **attune-pro**：个人行业付费，pluginhub 走 `License.plan ≥ pro` 路径
- **attune-enterprise（原 lawcontrol）**：B2B 小团队，pluginhub 走 `License.plan = enterprise` + `org_id` 匹配

技术独立硬约束不变：attune OSS 不调用 pluginhub 之外的任何 attune-pro / attune-enterprise API；数据完全隔离；任何配套关系通过用户主动 install / export / import 完成。

### 1.4 非目标

- ❌ 改 attune OSS 任何行为或边界（OSS 用户体验完全不变）
- ❌ 改 pluginhub 的 license-key 颁发机制（admin token / customer flow 不变）
- ❌ 把 lawcontrol 的业务逻辑（卷宗 / RPA / Intent Router）迁出当前仓 — 只改名，不动核心
- ❌ 引入 plugin 跨产品互通（attune-pro plugin 不能装到 attune-enterprise，反之亦然，除非显式 cross-tier 设计）

---

## 2. 范围边界

### 2.1 In Scope（v1：本 spec 的承诺面）

- pluginhub schema 增 `Plugin.visibility` + `Plugin.org_id`（可空）字段
- pluginhub API 增 `visibility` / `org_id` 过滤逻辑（`/api/v1/index.json` + `/api/v1/plugin/<id>/download`）
- attune-pro 现有 plugin 全部标 `visibility = pro`（batch update）
- lawcontrol 现有 plugin 抽出（保留源码在原仓不动，仅做发布管道改造）→ pluginhub 中标 `visibility = enterprise`
- lawcontrol → attune-enterprise 改名全链：
  - git repo（远端 + 本地 worktree）
  - docker image / docker-compose service name
  - DNS / domain（如 `lawcontrol.example.com` → `enterprise.attune.ai`）
  - service-discovery / k8s namespace
  - database schema 字段（凡含 `lawcontrol_*` 命名的表 / 列，加迁移脚本 rename）
  - 仓内 CLAUDE.md / README.md / DEVELOP.md / RELEASE.md
- CLAUDE.md 链 + memory 迁移：
  - `/data/company/project/attune/CLAUDE.md`「三产品矩阵」节 lawcontrol → attune-enterprise
  - `/data/company/project/attune-pro/CLAUDE.md` 同步
  - `/home/qiurui/.claude/projects/-data-company-project-attune/memory/` 全部 grep + rename
  - `/data/company/project/lawcontrol/CLAUDE.md` 迁到新 repo 路径

### 2.2 Out of Scope（推 v1.1+）

- ❌ 跨 vertical pro plugin（一个 license 同时解锁 law-pro + medical-pro）— 当前 1 license = 1 vertical
- ❌ pluginhub SaaS 多租户化（单 instance 服务多家 enterprise org）— 当前 enterprise license 隔离靠 org_id 字段，但 pluginhub 部署仍是单租户 backend
- ❌ Plugin 编译 / 签名 / supply chain 加固 — 沿用现状（SHA256 + admin token 发布）
- ❌ attune-pro / attune-enterprise plugin SDK 统一 — 当前各自 Python / Rust 双栈并存
- ❌ enterprise plugin 商业化定价 / billing — 仅技术管道改造，定价由商务侧决策
- ❌ Office helper / 其他 attune-core 内部 capability 改造

### 2.3 Scope 锁定

本 spec implementation 期间 **scope 不允许扩**。任何「顺手做一下」的扩展 → 推下个 minor 的新 spec。

---

## 3. 架构数据流

### 3.1 三产品矩阵 + pluginhub 总图

```
                                     ┌─────────────────────────────────┐
                                     │   attune-pluginhub (SaaS)        │
                                     │                                  │
                                     │   Plugin table:                  │
                                     │     - id, version, sha256        │
                                     │     - visibility: public|pro|ent │
                                     │     - org_id: nullable           │
                                     │     - min_plan: individual|...   │
                                     │                                  │
                                     │   License table:                 │
                                     │     - key (JWT)                  │
                                     │     - plan: individual|pro|ent   │
                                     │     - org_id: nullable           │
                                     │     - allowed_plugins: []        │
                                     └──────────┬──────────┬────────────┘
                                                │          │
                          ┌─────────────────────┘          └────────────────────┐
                          │ visibility=public               visibility={pro,    │
                          │ License: 无 / 任意               enterprise}         │
                          │                                  License: 校验       │
                          ▼                                                      ▼
              ┌──────────────────┐                                ┌─────────────────────────┐
              │   attune (OSS)   │                                │   attune (OSS) +        │
              │  桌面 / 扩展      │                                │   付费 plugin 装载         │
              │                  │                                │                          │
              │  个人通用用户      │                                │  ┌─ pro plugin ──┐       │
              │  零行业绑定        │                                │  │ law-pro       │       │
              │                  │                                │  │ medical-pro    │       │
              │  仅消费 public    │                                │  │ patent-pro     │       │
              │  plugin          │                                │  │ ... (个人用户)  │       │
              └──────────────────┘                                │  └────────────────┘       │
                                                                  │                          │
                                                                  │  ┌─ enterprise ──┐       │
                                                                  │  │ law-firm-ent  │ ◄── attune-enterprise (B2B SaaS, 原 lawcontrol) │
                                                                  │  │ medical-ent   │     │
                                                                  │  │ ... (企业团队) │     │
                                                                  │  └────────────────┘    │
                                                                  └────────────────────────┘
```

### 3.2 关键数据流路径

#### 路径 A：attune OSS 个人用户拉取免费 plugin

```
attune client ─[ GET /api/v1/index.json (no Authorization) ]→ pluginhub
                                                                 │
                                                                 ├─ filter: visibility=public
                                                                 └─ return: [public plugins only]

attune client ─[ GET /api/v1/plugin/<id>/download ]→ pluginhub
                                                       │
                                                       ├─ check: visibility=public → 允许
                                                       └─ return: package binary + SHA256
```

#### 路径 B：attune-pro 个人付费用户拉取 pro plugin

```
attune client ─[ GET /api/v1/index.json
                  Authorization: Bearer <license_jwt> ]→ pluginhub
                                                            │
                                                            ├─ decode JWT → plan=pro, org_id=null
                                                            ├─ filter: visibility ∈ {public, pro}
                                                            │          AND License.plan ≥ Plugin.min_plan
                                                            └─ return: [public ∪ pro plugins matching license]

attune client ─[ GET /api/v1/plugin/law-pro/download
                  Authorization: Bearer <license_jwt> ]→ pluginhub
                                                            │
                                                            ├─ decode JWT → plan=pro
                                                            ├─ check: plugin.visibility=pro AND plugin.min_plan≤pro → 允许
                                                            ├─ License.allowed_plugins 校验（若非空）
                                                            ├─ install quota 校验（max_installs）
                                                            └─ return: package binary
```

#### 路径 C：attune-enterprise B2B 团队拉取 enterprise plugin

```
attune client (企业部署) ─[ GET /api/v1/index.json
                              Authorization: Bearer <enterprise_license_jwt> ]→ pluginhub
                                                                                  │
                                                                                  ├─ decode JWT → plan=enterprise, org_id="acme-law-firm"
                                                                                  ├─ filter:
                                                                                  │    visibility ∈ {public, pro, enterprise}
                                                                                  │    AND (Plugin.org_id IS NULL OR Plugin.org_id = "acme-law-firm")
                                                                                  │    AND License.plan ≥ Plugin.min_plan
                                                                                  └─ return: [public ∪ pro ∪ acme enterprise plugins]

attune-enterprise SaaS (admin) ─[ POST /api/v1/admin/licenses
                                    Authorization: Bearer <admin_token> ]→ pluginhub
                                                                              │
                                                                              ├─ create License with plan=enterprise, org_id="acme-law-firm"
                                                                              └─ return: license_key (JWT)
```

#### 路径 D：License 校验 + 撤回（统一审计）

```
attune-pro client ─[ POST /api/v1/heartbeat
                       Authorization: Bearer <license_jwt> ]→ pluginhub
                                                                 │
                                                                 ├─ 记录 install_node + last_seen
                                                                 ├─ 若 License.is_active=false → 返回 403 + revoke 信号
                                                                 └─ 客户端收到 revoke → 本地禁用 plugin
```

### 3.3 数据库 / 缓存层

- **pluginhub DB（SQLite/Postgres）**：`Plugin` / `PluginVersion` / `License` / `LicensePlugin`（多对多）/ `InstallNode`
- 本 spec 加字段：`Plugin.visibility` (string, default='public')、`Plugin.org_id` (string nullable)
- **客户端缓存**：attune 本地 plugin index 缓存 1h，stale-while-revalidate；plugin 包 SHA256 永久缓存
- **CDN / 对象存储**：plugin 包二进制走对象存储（S3 / OSS），pluginhub 只签 presigned URL

### 3.4 跨仓边界

```
attune-pluginhub (独立仓)
  ├─ API 服务（FastAPI）
  ├─ DB schema
  └─ admin web UI

attune-pro (独立仓)
  └─ plugins/ ──[ scripts/publish-to-pluginhub.sh ]──→ pluginhub /admin/plugins POST

attune-enterprise (改名后，原 lawcontrol)
  └─ plugins/ ──[ scripts/publish-to-pluginhub.sh ]──→ pluginhub /admin/plugins POST

attune (OSS)
  └─ Rust client ──[ HTTPS GET ]──→ pluginhub /api/v1/index.json
                                                 /api/v1/plugin/<id>/download

cloud/pluginhub (submodule)
  └─ deploys attune-pluginhub via docker-compose
```

---

## 4. 模块边界

### 4.1 涉及的 git 仓库

| 仓库 | 当前路径 | 改造类型 | 改名 |
|------|---------|---------|------|
| `attune-pluginhub` | `/data/company/project/attune-pluginhub` | schema 扩展 + API 扩展 | ❌ 不改名 |
| `attune-pro` | `/data/company/project/attune-pro` | plugin metadata 增 `visibility: pro` + publish script | ❌ |
| `attune-enterprise`（原 `lawcontrol`） | `/data/company/project/lawcontrol` → `/data/company/project/attune-enterprise` | **改名 + plugin 抽取发布** | ✅ rename |
| `attune` (OSS) | `/data/company/project/attune` | client 端 visibility 不感知（只看 plugin id） | ❌ 几乎零改 |
| `attune-cloud` | `/data/company/project/attune-cloud` | pluginhub 部署 wrapper（submodule 引用） | ❌ |

### 4.2 跨仓接口（强制对齐点）

| 接口 | 提供方 | 消费方 | 当前状态 |
|------|--------|--------|---------|
| `GET /api/v1/index.json` | attune-pluginhub | attune / attune-pro 客户端 | 已有，需扩 visibility / org_id 过滤 |
| `GET /api/v1/plugin/<id>/download` | attune-pluginhub | attune 客户端 | 已有，需扩 License plan 校验 |
| `POST /api/v1/admin/licenses` | attune-pluginhub | attune-enterprise SaaS（管理员侧）/ attune-pro 订阅后端 | 已有，需扩 `org_id` 字段 |
| `POST /api/v1/heartbeat` | attune-pluginhub | 所有客户端 | 已有，无 schema 变更 |
| `scripts/publish-to-pluginhub.sh` | attune-pro / attune-enterprise | pluginhub admin API | **新增**，CI 集成 |

### 4.3 模块责任分配

| 模块 | 责任 | 不做 |
|------|------|------|
| **pluginhub** | plugin 元数据存储 / license 颁发 / 下载分发 / 心跳审计 | 不实现 plugin 业务逻辑、不解析 plugin 内容 |
| **attune-pro** | 个人付费 plugin 源码 + 测试 + 打包 + 发布脚本 | 不直接服务客户端，统一走 pluginhub |
| **attune-enterprise** | B2B SaaS 主体（卷宗 / RPA / Intent Router）+ enterprise plugin 源码 | SaaS 本身仍是企业部署形态，enterprise plugin 通过 pluginhub 分发到 attune 桌面 |
| **attune (OSS)** | 客户端 plugin install/uninstall/update（不区分 visibility，由 pluginhub 过滤后返回的 index 决定可见性） | 不存 license 校验逻辑（pluginhub 服务端校验） |

### 4.4 文件 / crate 级清单（implementation 时 plan 细化）

**pluginhub 改动文件**：
- `pluginhub/models.py` — `Plugin` 加 `visibility` + `org_id` 字段
- `pluginhub/api/index.py` — index 接口加过滤逻辑
- `pluginhub/api/download.py` — download 接口加 visibility 校验
- `pluginhub/api/admin/licenses.py` — license 创建接口加 `org_id` 入参
- `alembic/versions/` — 新增 migration script
- `tests/` — 新增 visibility / org_id 测试

**attune-pro 改动文件**：
- `plugins/*/plugin.yaml` — 每个 plugin 加 `visibility: pro`
- `scripts/publish-to-pluginhub.sh`（新增）
- `.github/workflows/release-plugins.yml`（新增或扩展）

**attune-enterprise 改动文件**（改名 + 抽 plugin 发布）：
- 全仓 `lawcontrol` 字符串 grep + rename（除合理保留的 law 业务术语，如 `law_firm` 行业代码）
- `docker-compose*.yml` service name rename
- `Dockerfile` image name 改
- `backend/settings.py` / `backend/manage.py` 等 Django 模块路径不变（避免大量 import 重写），但顶层标识改
- `plugins/skills/*` / `plugins/workflows/*` 中要发布到 pluginhub 的 → 加 `plugin.yaml` 含 `visibility: enterprise` + `org_id: <client>`
- `CLAUDE.md` / `README.md` / `DEVELOP.md` / `RELEASE.md` 全部改名

**attune (OSS) 改动文件**：
- `rust/crates/attune-core/src/plugins/marketplace.rs` — 无 schema 变更，仅适配新返回字段（可选感知 `visibility`）
- `rust/crates/attune-server/ui/src/views/MarketplaceView.tsx` — 可选显示 visibility 标签

---

## 5. API 契约

### 5.1 现有 endpoint 扩展

#### `GET /api/v1/index.json` （扩 visibility / org_id 过滤）

**当前**：
```json
{
  "plugins": [
    {"id": "law-pro", "version": "1.0.0", "min_plan": "pro", ...}
  ]
}
```

**新增**：返回值加 `visibility` + `org_id` 字段；服务端按调用方 license 过滤。

```json
{
  "plugins": [
    {
      "id": "law-pro",
      "version": "1.0.0",
      "min_plan": "pro",
      "visibility": "pro",          // ★ 新字段
      "org_id": null,               // ★ 新字段
      ...
    },
    {
      "id": "acme-law-firm-bundle",
      "version": "2.1.0",
      "min_plan": "enterprise",
      "visibility": "enterprise",
      "org_id": "acme-law-firm",    // 仅 org_id 匹配的 license 可见
      ...
    }
  ]
}
```

**过滤逻辑（pseudo）**：
```python
def filter_visible_plugins(license: Optional[License]) -> list[Plugin]:
    q = session.query(Plugin)
    if license is None:
        # 匿名调用：只返回 public
        return q.filter(Plugin.visibility == 'public').all()

    # 有 license：按 plan 上限 + org_id 匹配
    visible_levels = ['public']
    if license.plan in ('pro', 'enterprise'):
        visible_levels.append('pro')
    if license.plan == 'enterprise':
        visible_levels.append('enterprise')

    q = q.filter(Plugin.visibility.in_(visible_levels))
    q = q.filter(Plugin.min_plan_ord <= license.plan_ord)

    if license.org_id is None:
        # 个人 license：仅看 org_id IS NULL 的 plugin
        q = q.filter(Plugin.org_id.is_(None))
    else:
        # 企业 license：看 org_id IS NULL 或 org_id 匹配的 plugin
        q = q.filter((Plugin.org_id.is_(None)) | (Plugin.org_id == license.org_id))

    # allowed_plugins 白名单（若非空）
    if license.allowed_plugins:
        q = q.filter(Plugin.id.in_(license.allowed_plugins))

    return q.all()
```

#### `GET /api/v1/plugin/<id>/download` （扩 visibility 校验）

服务端在签 presigned URL 前必须重跑 `filter_visible_plugins` 校验。即便客户端直接拼 plugin id，也不能绕过过滤。

返回错误：
- `403 plugin-visibility-mismatch` — license plan 不够 / org_id 不匹配
- `403 plugin-quota-exceeded` — max_installs 达到上限
- `404 plugin-not-found` — plugin 不存在或被 yanked

#### `POST /api/v1/admin/licenses` （增 `org_id` 入参）

**Request**：
```json
{
  "customer_name": "ACME Law Firm",
  "plan": "enterprise",
  "org_id": "acme-law-firm",      // ★ 新字段 (enterprise plan 必填)
  "allowed_plugins": [],
  "max_installs": 50,
  "expires_at": "2027-05-22T00:00:00Z"
}
```

**校验规则**：
- `plan = enterprise` AND `org_id IS NULL` → 422 enterprise-requires-org-id
- `plan ∈ {individual, pro}` AND `org_id IS NOT NULL` → 422 individual-license-cannot-have-org
- `org_id` 格式：`^[a-z0-9][a-z0-9-]{1,63}$` (kebab-case，DNS-safe)

### 5.2 新增 endpoint

#### `POST /api/v1/admin/plugins/<id>/visibility` （管理员调整 visibility）

```json
{
  "visibility": "enterprise",
  "org_id": "acme-law-firm"
}
```

仅 admin token 可调。

### 5.3 JWT License 字段扩展

License key 解码后的 claims：

```json
{
  "license_id": 42,
  "customer_name": "ACME Law Firm",
  "plan": "enterprise",           // 已有
  "org_id": "acme-law-firm",      // ★ 新字段
  "allowed_plugins": [],
  "max_installs": 50,
  "exp": 1779456000,
  "iat": 1747920000
}
```

向后兼容：旧 license JWT 无 `org_id` 字段时按 `null` 处理（= 个人 license）。

### 5.4 CLI（attune 客户端）

无 API 变更。`attune plugin list` / `attune plugin install <id>` 行为不变，只是后台 pluginhub 返回的 plugin 集合受 license 影响。

---

## 6. 扩展点 / 插件接口

### 6.1 未来加新 vertical 流程

加 `medical-pro` 或 `patent-pro` plugin 流程：

1. 在 attune-pro 仓 `plugins/medical-pro/` 写源码 + `plugin.yaml`
2. `plugin.yaml` 标 `visibility: pro` + `min_plan: pro`
3. 跑 `scripts/publish-to-pluginhub.sh medical-pro 1.0.0`
4. pluginhub 自动接收（admin token 鉴权）
5. 任何 `License.plan ≥ pro` 的客户端下次 index refresh 即看到

加企业 vertical（如 `medical-ent` 给某医院集团）：

1. 在 attune-enterprise 仓 `plugins/medical-ent/` 写源码
2. `plugin.yaml` 标 `visibility: enterprise` + `min_plan: enterprise` + `org_id: <hospital-org-id>`
3. 发布到 pluginhub
4. 该 hospital 的 enterprise license（包含 `org_id`）即可拉到

### 6.2 Plugin signing key 路径（enterprise 自管）

- **个人 plugin（pro）**：pluginhub 统一签名（`pluginhub_release_key`）
- **企业 plugin（enterprise）**：可选企业自管签名
  - 企业 admin 上传公钥到 pluginhub
  - 发布时本地签名 + 上传签名文件
  - 客户端拉取后用对应 org 的公钥验签

v1 范围：仅做统一签名（pluginhub key），企业自管签名推 v1.2。

### 6.3 多 SDK 兼容

attune-pro Python plugin 当前 / attune-enterprise Python+TS 混合 / 未来可能加 Rust plugin SDK — pluginhub 不感知 plugin 内部语言，只看包 SHA256 + metadata。

### 6.4 hook 点：客户端 plugin lifecycle

未来扩展 hook：
- pre-install hook（license 校验后、解压前）
- post-install hook（解压后、首次启动前）
- pre-uninstall hook（清理 plugin 数据）

v1 范围：不实现 hook，保留接口空间。

---

## 7. 错误处理 + 边界 case

### 7.1 错误码表（kebab-case）

| HTTP | code | 触发场景 | 客户端行为 |
|------|------|---------|----------|
| 401 | `missing-license` | 调下载 endpoint 无 Authorization | 提示用户登录 / 输入 license |
| 401 | `invalid-license-jwt` | JWT 解析失败 / 签名错 | 提示重新登录 |
| 403 | `license-expired` | `exp < now` | 提示续费 |
| 403 | `license-revoked` | `is_active = false` | 本地禁用对应 plugin |
| 403 | `plugin-visibility-mismatch` | `plan < plugin.min_plan` 或 `org_id` 不匹配 | 显示「升级套餐」CTA |
| 403 | `plugin-quota-exceeded` | `used_installs ≥ max_installs` | 提示「联系管理员」 |
| 404 | `plugin-not-found` | id 不存在或 yanked | 从客户端 marketplace 移除显示 |
| 409 | `version-not-found` | plugin 存在但指定 version 不存在 | 回退到最新可用版本 |
| 422 | `enterprise-requires-org-id` | enterprise license 创建未传 `org_id` | admin 表单校验 |
| 422 | `individual-license-cannot-have-org` | individual/pro license 传了 `org_id` | admin 表单校验 |
| 422 | `invalid-org-id-format` | `org_id` 不符合 kebab-case regex | admin 表单校验 |

### 7.2 边界 case

| 场景 | 期望行为 |
|------|---------|
| individual user 直接拼 url 访问 enterprise plugin | 403 visibility-mismatch（即便知道 plugin id 也拿不到） |
| enterprise user A 拼 url 访问 enterprise user B 的 plugin | 403 visibility-mismatch（org_id 不匹配） |
| pro license 在 enterprise plugin 上 install | 403（min_plan 校验） |
| license 即将过期（剩 7 天）| 200 但 response 加 `warning: license-expiring-soon` |
| pluginhub 网络不通 | 客户端用缓存 index，已下载 plugin 继续可用，新 plugin 提示「离线模式」 |
| plugin 版本回滚（旧版本被 yanked） | 客户端继续用本地已装版本，更新检查跳过 yanked 版本 |
| License 同时含 `allowed_plugins` 白名单 AND `visibility` 跨档 | 白名单和 visibility 过滤 **AND** 关系（白名单非空时严格收紧） |

### 7.3 Graceful degradation

- **pluginhub 完全不可达**：attune 仅依赖本地已安装 plugin，UI marketplace 标灰「离线模式」
- **license 撤回但客户端心跳未更新**：客户端在下次心跳收到 revoke 信号后禁用，已 cache 的 plugin 包不删除（保留 audit）
- **plugin 包损坏（SHA256 不匹配）**：拒绝加载 + 提示重新下载 + 上报 pluginhub `report-corruption` endpoint（v1.1）

---

## 8. 成本契约

per 项目 CLAUDE.md「Cost & Trigger Contract」三层成本模型。

### 8.1 OSS attune 用户（免费 tier）

| 操作 | 成本层 | 谁付 |
|------|-------|------|
| 浏览 marketplace（拉 index.json） | 🆓 零成本（pluginhub CDN） | pluginhub 运营方 |
| 下载 public plugin 包 | 🆓 零成本（CDN bandwidth） | pluginhub 运营方 |
| 心跳上报 | 🆓 零成本（毫秒级请求） | pluginhub 运营方 |

OSS 用户无任何金钱成本 / token 成本。

### 8.2 attune-pro 个人订阅用户

| 操作 | 成本层 | 谁付 |
|------|-------|------|
| 订阅 attune-pro | 💰 月费 / 年费 | 用户 |
| 下载 pro plugin | 🆓（包含在订阅内） | pluginhub 运营方（订阅费摊销） |
| plugin 内部 LLM 调用 | 💰 用户 token（BYOK / 平台 gateway） | 用户 |
| plugin 内部本地推理 | ⚡ 用户硬件 | 用户硬件 |

UI 显示：marketplace 上 pro plugin 标「需 Pro 订阅」，订阅按钮显示「¥X/月」「¥Y/年」。

### 8.3 attune-enterprise B2B 用户

| 操作 | 成本层 | 谁付 |
|------|-------|------|
| enterprise license（含 N seats） | 💰 年度合同 + per-seat | 企业 |
| 下载 enterprise plugin | 🆓（包含 license） | pluginhub 运营方 |
| SSO / 审计 / 撤回 | 💰 包含 license | 企业 |
| 内部 plugin LLM 调用 | 💰 企业 token（统一计费） | 企业 |

UI 显示：enterprise plugin marketplace 标「企业版」，禁止个人订阅入口。

### 8.4 触发规则

- pluginhub index 拉取：客户端 1h cache，stale-while-revalidate，**不主动 push**
- plugin 包下载：用户**显式**点「Install」触发（不自动后台下载）
- 心跳：每 24h 一次，**后台静默**（视为基础设施成本）

---

## 9. 测试矩阵

per 全局 CLAUDE.md「Agent 验证铁律」6 类下限，适配 pluginhub backend + client 集成：

### 9.1 Golden case（≥10 真实组合）

固化 `tests/golden/license-plugin-matrix/` YAML fixture：

| Fixture | License plan | License org_id | Plugin visibility | Plugin org_id | Plugin min_plan | 期望 |
|---------|-------------|---------------|------------------|--------------|----------------|------|
| `01-anon-public` | (no license) | - | public | null | individual | ✅ 200 |
| `02-anon-pro` | (no license) | - | pro | null | pro | ❌ 401 |
| `03-individual-public` | individual | null | public | null | individual | ✅ 200 |
| `04-individual-pro` | individual | null | pro | null | pro | ❌ 403 visibility-mismatch |
| `05-pro-pro` | pro | null | pro | null | pro | ✅ 200 |
| `06-pro-enterprise` | pro | null | enterprise | "acme" | enterprise | ❌ 403 visibility-mismatch |
| `07-enterprise-match-org` | enterprise | "acme" | enterprise | "acme" | enterprise | ✅ 200 |
| `08-enterprise-mismatch-org` | enterprise | "acme" | enterprise | "beta-corp" | enterprise | ❌ 403 visibility-mismatch |
| `09-enterprise-pro` | enterprise | "acme" | pro | null | pro | ✅ 200（enterprise ⊃ pro） |
| `10-pro-allowed-plugins-whitelist` | pro (`allowed=[a]`) | null | pro | null | pro | ✅ 200 if id=a else 403 |
| `11-expired-license` | pro (expired) | null | pro | null | pro | ❌ 403 license-expired |
| `12-revoked-license` | pro (is_active=false) | null | pro | null | pro | ❌ 403 license-revoked |

### 9.2 Property tests（≥3 per agent，proptest）

- prop1：随机生成 (plan, org_id, visibility, plugin_org) 组合，验证可见性矩阵自洽（individual ⊂ pro ⊂ enterprise）
- prop2：org_id 匹配是反射性的（`org A` 看到 `org A` plugin，不看到 `org B`）
- prop3：白名单 + visibility 永远是 AND（更严，不会放宽）

### 9.3 Boundary cases（≥5）

- B1：license `exp` 正好 `now` → 视为过期（边界严格）
- B2：`max_installs = 0` → 不限（特殊语义）
- B3：`allowed_plugins = []` → 空数组视为「全部允许」（不是「全部禁止」）
- B4：plan 字符串大小写（`Pro` vs `pro`）→ 强制 lowercase 入库
- B5：org_id 含 unicode / emoji → 422

### 9.4 Error cases（≥3）

- E1：JWT 签名密钥不匹配 → 401 invalid-license-jwt
- E2：JWT 缺 `plan` claim → 422
- E3：JWT 缺 `exp` claim → 拒绝（不允许永不过期 JWT）

### 9.5 Integration E2E（≥1 subprocess test）

`tests/e2e/test_pluginhub_full_flow.py`：

1. 启动 pluginhub docker-compose
2. admin 创建 enterprise license（org_id="acme"）
3. 上传 enterprise plugin（visibility=enterprise, org_id="acme"）
4. attune CLI 用 license 跑 `attune plugin list` → 看到该 plugin
5. attune CLI 跑 `attune plugin install <id>` → 下载 + 解压 + SHA256 校验通过
6. 模拟 license revoke → 客户端心跳后该 plugin 在 marketplace 标 revoked

### 9.6 Regression fixture

每修一个 bug 必加 fixture：
- 例：若发现「pro license 拿到 enterprise plugin」漏洞 → fixture `13-pro-cannot-see-enterprise-org-null` 写入 golden set
- ratchet rule：fixture 集合只升不降

### 9.7 改名 regression（lawcontrol → attune-enterprise）

特殊回归测试：
- R1：旧 `lawcontrol_*` docker image tag 仍存档（30 天过渡期，期间 docker pull 兼容）
- R2：旧 `lawcontrol.example.com` DNS 加 301 redirect 到 `enterprise.attune.ai`
- R3：旧 `db_table = lawcontrol_*` SQL alias view（保留 1 release，期间双写）
- R4：CLAUDE.md / memory 全 grep 后 0 残留 lawcontrol 字串（除合理保留的 law 业务术语）

---

## 10. 向后兼容 / migration path

### 10.1 pluginhub schema migration

**Alembic migration**：

```python
# alembic/versions/2026_05_22_add_visibility_org_id.py
def upgrade():
    op.add_column('plugin', sa.Column('visibility', sa.String(20), nullable=False, server_default='public'))
    op.add_column('plugin', sa.Column('org_id', sa.String(64), nullable=True))
    op.add_column('license', sa.Column('org_id', sa.String(64), nullable=True))
    op.create_index('ix_plugin_visibility_org', 'plugin', ['visibility', 'org_id'])

def downgrade():
    op.drop_index('ix_plugin_visibility_org')
    op.drop_column('license', 'org_id')
    op.drop_column('plugin', 'org_id')
    op.drop_column('plugin', 'visibility')
```

**默认值策略**：
- 旧 plugin → `visibility = 'public'`（与旧 `is_public = true` 等价）
- 旧 plugin where `is_public = false` AND `min_plan = pro` → 升级到 `visibility = 'pro'`（运行一次性 backfill SQL）
- 旧 license 无 `org_id` → 保持 NULL（视为 individual / pro）

**Backfill SQL（手工跑一次）**：
```sql
-- 旧 pro plugin 标 visibility=pro
UPDATE plugin SET visibility = 'pro' WHERE min_plan = 'pro';
-- 旧 enterprise plugin 标 visibility=enterprise（如果有）
UPDATE plugin SET visibility = 'enterprise' WHERE min_plan = 'enterprise';
-- attune-pro 现有 plugin 整体标 visibility=pro
UPDATE plugin SET visibility = 'pro' WHERE id IN (SELECT id FROM plugin WHERE category IN ('law-pro', 'tech-pro', 'patent-pro', 'presales-pro'));
```

### 10.2 lawcontrol → attune-enterprise 改名 path（staged）

**阶段 1（spec 评审后第 1 周）**：准备
- 在 GitHub 创建 `attune-enterprise` 仓（保留 `lawcontrol` 仓作 archive，README 指向新仓）
- 跑 `git clone lawcontrol --mirror && git push attune-enterprise --mirror`（含全 history）
- 仓内 `README.md` / `CLAUDE.md` 改名

**阶段 2（第 2 周）**：双仓并存
- 老仓 freeze（main 分支锁定，只接 hotfix）
- 新仓 develop 上 rebrand commit（image name / service name / docker-compose）
- 部署侧用 alias DNS（`lawcontrol.* → enterprise.attune.ai`，301 redirect）

**阶段 3（第 3-4 周）**：客户迁移
- 现有客户 SaaS 端逐个切换（每客户一个 maintenance window）
- license key 不变（pluginhub 侧 plan 仍是 enterprise）
- docker image rename：保留旧 tag 30 天，新部署用新 tag

**阶段 4（第 5+ 周）**：清理
- 30 天 grace period 后老仓 archive
- 旧 docker image 删除
- 旧 DNS 留 redirect 永久

### 10.3 attune-pro plugin metadata migration

每个 `plugins/*/plugin.yaml` 加：

```yaml
# Before
id: law-pro
version: 1.0.0
min_plan: pro

# After
id: law-pro
version: 1.0.0
min_plan: pro
visibility: pro       # ★ 新字段
org_id: null          # ★ 显式 null（个人 plugin 不绑 org）
```

旧 yaml 无 `visibility` 字段 → publish script 自动按 `min_plan` 映射（`min_plan=pro → visibility=pro`）。

### 10.4 CLAUDE.md / memory 迁移 plan

**文件清单**：

```bash
# Step 1: grep all lawcontrol references
grep -rln "lawcontrol" \
  /data/company/project/attune/CLAUDE.md \
  /data/company/project/attune-pro/CLAUDE.md \
  /data/company/project/attune-pluginhub/CLAUDE.md \
  /data/company/project/lawcontrol/CLAUDE.md \
  /home/qiurui/.claude/projects/-data-company-project-attune/memory/ \
  /home/qiurui/.claude/projects/-data-company-project-attune-pro/memory/ \
  /home/qiurui/.claude/projects/-data-company-project-lawcontrol/memory/
```

**rename 规则**：

| 原文 | 改为 |
|------|------|
| `lawcontrol` (repo / product 引用) | `attune-enterprise` |
| `lawcontrol` (URL / 部署) | `enterprise.attune.ai` |
| `lawcontrol` (业务术语 / 历史 incident 引用) | 保留（如「2026-04 lawcontrol design borrowed plugin.yaml pattern」） |
| `lawcontrol/plugins/skills/contract_review` | `attune-enterprise/plugins/skills/contract_review` |
| `/data/company/project/lawcontrol` | `/data/company/project/attune-enterprise` |
| `B2B 律所` / `律所 SaaS` | `B2B 企业团队`（更通用，law 是第一垂直） |

**memory 迁移命令（不直接执行，待评审）**：
```bash
# 重命名 memory 目录
mv ~/.claude/projects/-data-company-project-lawcontrol \
   ~/.claude/projects/-data-company-project-attune-enterprise

# 全 grep + sed 替换（手工 review）
find ~/.claude/projects/-data-company-project-attune-enterprise -type f -name "*.md" \
  -exec sed -i 's|/data/company/project/lawcontrol|/data/company/project/attune-enterprise|g' {} \;
```

### 10.5 客户端兼容（attune OSS）

- 当前 attune client 不感知 visibility / org_id，只看 pluginhub 返回的 plugin list
- 新字段对老客户端透明（JSON 多字段不影响 deserialize，client 用 `serde(default)` 容错）
- 老 license JWT（无 `org_id`）服务端按 null 处理，兼容

---

## 11. 风险登记

| ID | 风险 | 等级 | 触发场景 | 缓解措施 | 责任方 |
|----|------|------|---------|---------|--------|
| **R1** | lawcontrol 改名破坏现有客户部署 | 🔴 高 | 客户现网用 `lawcontrol.example.com` / `docker pull lawcontrol/*` / SQL 含 `lawcontrol_*` 表名 | 阶段化 migration（10.2）+ 30 天 grace + DNS 301 + docker tag alias + DB view alias | DevOps + 商务 |
| **R2** | pluginhub schema migration 破生产数据 | 🔴 高 | Alembic upgrade 跑挂 / backfill SQL 错刷数据 | (1) prod 跑前 staging 全量 dry-run；(2) `pg_dump` 完整备份；(3) `BEGIN; ... ROLLBACK;` 在 staging 验证；(4) downgrade migration 必须可执行 | pluginhub 维护者 |
| **R3** | enterprise plugin 抽出后 lawcontrol 现有功能 break | 🟡 中 | lawcontrol 内部 skill 当前直接 import `plugins/skills/*` Python module，抽到 pluginhub 后路径变 | (1) 不真正物理移动 plugin 源码，仅在 pluginhub 上注册元信息 + 包；(2) lawcontrol 内部 import 路径保持 attune-enterprise/plugins/* 不变；(3) pluginhub 是「分发管道」而非「源码 SoT」 | attune-enterprise 维护者 |
| **R4** | CLAUDE.md / memory 迁移漏文件 → AI 上下文丢 | 🟡 中 | grep / sed 漏文件 / 漏字符串变种 | (1) grep 必须 case-insensitive `grep -irln`；(2) 跑两遍（自己 + 第二人 review）；(3) commit 后跑 verify script 确认 0 残留；(4) AI 会话冷启动后人工抽检几个 prompt | AI 工作流维护者 |
| **R5** | individual user 通过拼 url 越权访问 enterprise plugin | 🔴 高（安全） | 客户端层不校验 visibility，仅依赖服务端 | (1) 所有 download endpoint 必须服务端重跑 `filter_visible_plugins`；(2) presigned URL 短时效（5 min）；(3) S3 / OSS bucket 不允许直接 public list | pluginhub 维护者 |
| **R6** | org_id 命名冲突（两家 ACME 公司） | 🟡 中 | 不同企业同名 → org_id 重复 | (1) org_id 由 pluginhub admin 创建 license 时分配 + 唯一性校验；(2) 命名规则建议「company-suffix」（如 `acme-us` / `acme-cn`）；(3) admin UI 含 org_id 已存在告警 | pluginhub admin |
| **R7** | enterprise plugin org_id 误配（错给到别家） | 🔴 高（数据泄漏） | admin 创建 plugin 时 org_id 错填 | (1) admin UI 二次确认；(2) audit log 记录每次 visibility / org_id 改动；(3) 客户端首次 install 显示「来源 org_id」让用户验证 | pluginhub admin |
| **R8** | 老 license JWT 没 `org_id` claim 视为 null → 个人 license 误判为「全 org plugin 可见」 | 🟡 中 | 老 license fallback 逻辑写错 | (1) fallback `org_id=null` AND `plan ∈ {individual, pro}` AND `visibility ∈ {public, pro}` 三条件严格 AND；(2) 测试 fixture `08-enterprise-mismatch-org` 必须覆盖 | pluginhub 维护者 |
| **R9** | DNS / docker rename 期间客户端 cache 旧地址 | 🟢 低 | 客户端硬编码 `lawcontrol.example.com` | (1) 客户端 endpoint 走 config 文件（不硬编码）；(2) auto-update 推送新 config | client 维护者 |
| **R10** | scope 蠕变：实施期间增「跨 vertical plugin」「企业自管签名」等需求 | 🟡 中 | 用户 / PM 要求顺手做 | 拒绝。推 v1.1 新 spec，本 spec scope 锁定 | spec 评审 owner |
| **R11** | attune OSS 用户误以为升级后能用 enterprise plugin | 🟢 低（产品） | UI marketplace 显示 enterprise plugin 但 install 失败 | 服务端过滤后客户端根本看不到 enterprise plugin（path A 数据流）→ 不出现该 case | pluginhub 维护者 |
| **R12** | 双仓（attune-pro + attune-enterprise）publish-to-pluginhub script 重复维护 | 🟢 低 | 两份 script 漂移 | 抽公共 logic 到 `attune-pluginhub/scripts/cli.py`，pro / enterprise 仓只放 `.github/workflows/release.yml` 调用 | attune-pluginhub 维护者 |

### 11.1 红色风险（高优先级）汇总

R1 / R2 / R5 / R7 是**必须在 implementation plan 阶段单独立 mitigation task** 的红色风险。每个 task 含：dry-run / 备份 / 回滚步骤 / 责任人 / 验证方式。

### 11.2 与历史 incident 关联

- **2026-05-21 attune-pro Phase 2 4 agent dispatch worktree 共享撞车**：本 spec §4 模块边界 + §10.2 阶段化 migration 吸取了「跨仓改动需要先定边界再动手」的教训
- **2026-05-20 attune doc cleanup**：本 spec §10.4 CLAUDE.md / memory 迁移用 grep + verify script 防漏，避免「以为改完了实际有残留」

---

## 附录 A：评审 checklist

评审本 spec 时请逐条 ✓ / ✗：

- [ ] 11 节是否齐全？每节是否有实质内容（不是占位）？
- [ ] §2 scope 边界是否明确（in / out / 锁定）？
- [ ] §3 数据流图是否覆盖三产品矩阵全部路径？
- [ ] §5 API 契约是否兼容老客户端？
- [ ] §7 错误码是否完整 + kebab-case + 与 attune-server `AppError` 规范一致？
- [ ] §9 测试矩阵是否覆盖 6 类下限（per Agent 验证铁律）？
- [ ] §10 migration path 是否可回滚（downgrade migration）？
- [ ] §11 红色风险是否都有 mitigation？责任方是否明确？
- [ ] lawcontrol → attune-enterprise rename 是否考虑了客户现网部署的 disruption？
- [ ] org_id 命名 / 冲突 / 错配是否有防护？

---

## 附录 B：后续 plan 锚点

spec 评审通过后，下一步触发 `superpowers:writing-plans` 出 implementation plan。Plan 必含：

1. **日历**：天 × 小时分块（建议 5 个工作日）
   - D1：pluginhub schema migration + backfill
   - D2：pluginhub API 扩展（index / download / admin）+ 单测
   - D3：attune-pro publish script + plugin yaml backfill
   - D4：attune-enterprise rename（阶段 1-2）
   - D5：E2E 测试 + CLAUDE.md/memory 迁移 + verify
2. **文件清单**：精确到 file path + 改动行数估算
3. **commit 分批**：每个 commit ≤ 200 行，独立可回滚
4. **风险登记**：每个红色风险绑定一个独立 task + dry-run 步骤
5. **GA 验收清单**：每个测试矩阵 fixture + E2E 通过

**Plan 落盘路径**：`docs/superpowers/plans/2026-05-XX-unified-pluginhub-implementation.md`

---

**spec 起草完毕。等用户评审。禁止直接进 implementation。**
