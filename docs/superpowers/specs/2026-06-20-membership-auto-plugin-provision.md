# Spec: 会员登录 → 按场景 (vertical) 自动下载并安装对应 pro 插件

> Date: 2026-06-20 · Status: DRAFT (待评审) · Owner: GA-1 (task #107)
> 关联: [[project_plugin_trust_chain]] · [[project_entitlement_signed_snapshot]] ·
> [[project_sec12_integration_state]] · `docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md`

## 0. 目录 (TOC)

- [1. 目标定位](#1-目标定位)
- [2. 范围边界](#2-范围边界)
- [3. 架构数据流](#3-架构数据流)
- [4. 模块边界](#4-模块边界)
- [5. API 契约](#5-api-契约)
- [6. 扩展点 / 插件接口](#6-扩展点--插件接口)
- [7. 错误 + 边界 case](#7-错误--边界-case)
- [8. 成本契约](#8-成本契约)
- [9. 测试矩阵](#9-测试矩阵)
- [10. 向后兼容](#10-向后兼容)
- [11. 风险登记](#11-风险登记)

---

## 1. 目标定位

**用户痛点**：当前会员登录后，要拿到自己场景（律师 / 医生 / 专利代理 / 售前 / 工程师 / 学者）
的 pro 能力，需要**手动**去 Marketplace 视图找到对应 vertical 插件、点安装、等下载验签。多数
个人行业用户不知道"我是律师就该装 law-pro"，上手摩擦大，且 Marketplace 列表对非技术用户是噪声。

**目标**：会员登录（账号密码 / 授权码激活 / login-token）成功后，**根据该会员账号绑定的
场景 (vertical)**，**自动下载 + 验签 + 安装**对应的 pro 插件，**零手动选择**。登录即得场景能力。

**与产品定位对齐**：
- 北极星「降低上手摩擦」(§ Attune Pro Membership Gateway「登录即用」的延伸：不只 LLM 网关零配置，
  连场景插件也零配置)。
- 三产品矩阵：`个人行业用户 = attune (OSS) + attune-pro/<vertical>-pro`。本 spec 让这个"="
  在登录时自动成立，而不是让用户手动拼装。
- OSS 边界不破：OSS attune 自身**不内置任何 vertical 知识**；vertical→plugin 映射的**权威在
  cloud**（plan/vertical → entitled_plugins），客户端只是按云端授权清单执行下载安装。

**关键事实（现状基线）**：自动安装机制**已存在** —— `plugin_sync::best_effort_sync_plugins`
在三条会员入口（`login_password` / `login_token` / `activate_license`）登录成功后已被调用，按
`License.entitled_plugins`（或授权码 `allowed_plugins`）自动下载安装。**缺的不是"自动安装"，
而是"按场景授予"**：cloud 当前用 `plugins_for_plan(plan)` 把**同一份** `["law-pro"]` 发给所有
pro 用户，无论其真实场景。本 spec 的核心增量 = **给会员账号加 vertical 维度，让 entitled
清单随场景而变**，自动安装机制原样复用。

---

## 2. 范围边界

### 做（this version）

1. **cloud 侧**：`User` 增加 `vertical` 字段（law/medical/patent/presales/tech/academic/`null`）；
   `entitled_plugins` 派生从 `plugins_for_plan(plan)` 改为 `plugins_for(plan, vertical)`；
   admin 发放 / 授权码 / Stripe 路径写入 vertical。
2. **cloud 侧**：`/api/v1/me`、`GET /licenses`、`POST /member/activate`、`POST /member/verify`
   响应携带 `vertical` + 按 vertical 过滤后的 entitled/allowed_plugins。
3. **client 侧**：`UserInfo` / `ActivateResult` 镜像 `vertical` 字段（serde default 容缺）；
   登录后自动 sync 已有路径**原样复用**（它已按 entitled 清单装），仅补充：UI 明确显示
   "已为你的场景〔律师〕安装 law-pro"。
4. **client 侧**：自动下载**强制官方签名验证**（复用现有 `verify_plugin_anchor` W1 allowlist +
   `verify_with_key` Ed25519 + SEC-1/2 签名快照 entitlement 门），不新增旁路。
5. **幂等**：已装的 vertical 插件不重复下载（复用 `installed_ids` 短路）。
6. **graceful degrade**：下载/验签/部分失败/离线 **绝不阻塞登录**（复用 `best_effort_*`
   never-Err 契约）；失败明确 surface 给用户 + 可手动重试。

### 不做（明确排除，写死防 scope creep）

- ❌ **免费用户自动装**：free tier 无 entitlement → 不自动装任何 pro 插件（Marketplace 仍可手动试用）。
- ❌ **非会员场景**：未登录 / LoggedOut 不触发。
- ❌ **多 vertical 同账号**：本版一个账号一个主 vertical（`enterprise` 例外见 §6）。多 vertical
  组合留 v.next。
- ❌ **客户端自行决定 vertical**：vertical 是 cloud 账号属性，client 不猜、不本地配置（防伪造越权，§11）。
- ❌ **自动卸载**：vertical 变更后旧插件不自动删（防误删用户自装/学习状态，复用现有"多余的留着"策略）。
- ❌ **新 vertical 插件的开发**：med-pro 等插件本体由 attune-pro 仓发布（前置依赖，非本 spec）。
- ❌ **OSS 内置 vertical→plugin 表**：违反 OSS 边界；映射只在 cloud。

---

## 3. 架构数据流

```
┌─────────────┐  ① login(email,pw) / activate(license_key) / login-token
│  Desktop    │ ───────────────────────────────────────────────►┌──────────────┐
│  attune     │                                                   │ cloud        │
│  client     │  ② 200 { plan, vertical, gateway_*,               │ accounts     │
│ (member.rs) │ ◄──── entitled_plugins[] (已按 vertical 过滤,      │ /api/v1/*    │
└─────┬───────┘        每条带 signing_pubkey_hex + download_url)   └──────┬───────┘
      │                                                                    │
      │ ③ best_effort_sync_plugins(cloud)  [spawn_blocking, 不阻塞登录]    │ plugins_for(
      │     for ep in license.entitled_plugins:                            │   plan, vertical)
      │       if ep.plugin_id ∈ installed_ids: skip (幂等)                 │ = vertical→[pids]
      │       else:                                                        │
      ▼                                                                    │
┌──────────────────────────────────────────────┐                          │
│ install_one_plugin(ep):                       │   ④ GET download_url     │
│  ④ download_to_file(ep.download_url, Bearer ──┼─────────────────────────►│ pluginhub
│     license_key)            → signed .tar.gz plugin package      │      (付费门 Bearer)      │ /api/v1/packages
│  ⑤ verify_plugin_anchor(ep)  ← W1 allowlist   │ ◄────────────────────────┤   /{id}-{v}.tar.gz
│     (signing_pubkey_hex ∈ OFFICIAL_ANCHORS?)  │      tar.gz bytes         └──────────────┘
│     miss → AnchorNotPinned (fail-closed)      │
│  ⑥ verify_with_key(pkg, ep.signing_pubkey_hex)│   Ed25519 sig over
│     fail → Crypto err (fail-closed)           │   sha256(plugin.yaml+\0+prompt.md)
│  ⑦ LoadedPlugin::from_dir_with_key (+decrypt) │
│  ⑧ copy → plugins_dir/<id>/  (atomic)         │
└──────────────┬───────────────────────────────┘
               │ ⑨ persist EntitlementRow → vault `plugin_entitlements`
               │    (ACP-6: vault DB only, 不写 plugins/<id>/)
               ▼
        SyncReport { installed, skipped_already_installed, failed[(id,reason)] }
               │ ⑩ 返回登录响应 plugin_sync 字段 → UI 显示 "已为〔律师〕场景安装 law-pro"
               ▼
   周期 re-verify worker (run_refresh_round): 每条 entitlement 转 Active 前
   过 SEC-1/2 签名快照门 (authorize_snapshot)；revoked/suspended → 降级落盘
```

### 涉及的表 / 字段

**cloud `accounts` DB（新增/改）**：
- `users.vertical VARCHAR(20) NULL` — 新列（alembic migration `0004_user_vertical`）。值域
  `law|medical|patent|presales|tech|academic|NULL`。NULL = 未指定场景（行为同今天：按 plan 默认）。
- `issued_licenses.entitled_plugins JSON` — **不改 schema**；其**内容**改为 `plugins_for(plan, vertical)`
  的派生结果（已是 JSON array，无 migration）。

**client vault（不改 schema，复用）**：
- `plugin_entitlements` 表 — install 时落 `EntitlementRow`（已有，§plugin_sync T7）。
- `app_settings` meta — gateway / trust_mode（已有）。
- `cloud-session.json`（config_dir）— login 后写的 session（已有）。

**client 内存态（不改）**：`MemberState::{Free,Paid}` / `EntitlementCache`。

---

## 4. 模块边界

### attune client（本仓）

| 文件 | 改动 |
|------|------|
| `rust/crates/attune-core/src/cloud_client.rs` | `UserInfo` / `ActivateResult` / `EntitlementSnapshot` 加 `#[serde(default)] vertical: Option<String>` 字段（容缺，老 cloud → None）。`License` 可选携带 `vertical` 用于 UI 展示。 |
| `rust/crates/attune-core/src/plugin_sync.rs` | **无逻辑改动**（按 entitled 清单装的机制不变）。可选：`SyncReport` 加 `vertical: Option<String>` 透传给 UI 文案。 |
| `rust/crates/attune-server/src/routes/member.rs` | 三入口响应 JSON 透传 `vertical`（已有 `tier`/`plan` 透传位）；`sync_report_to_json` 可选加 vertical 文案。**核心 sync 调用不变。** |
| `rust/crates/attune-server/ui/src/views/MarketplaceView.tsx` + `useMember.ts` | 显示"已为你的场景〔X〕自动安装：law-pro"；i18n key（zh+en 同步，§i18n 守卫）。 |
| `rust/crates/attune-core/src/plugin_anchor.rs` / `entitlement_anchor.rs` | **不改**（信任根复用；新 vertical 插件的官方锚加入 `OFFICIAL_PLUGIN_ANCHORS` 是发布时的数据更新，非本 spec 代码改动）。 |

### cloud（`/data/company/cloud/accounts`）

| 文件 | 改动 |
|------|------|
| `accounts/models.py` | `User` 加 `vertical` 列。 |
| `accounts/alembic/versions/0004_user_vertical.py` | 新 migration（add column, nullable）。 |
| `accounts/config.py` | `plan_plugins_map` → `vertical_plugins_map`（vertical→[pids]）+ `plugins_for(plan, vertical)` 函数（保留 `plugins_for_plan` 作 vertical=None 的回退）。`plan_plugin_meta` 加 med-pro/patent-pro 等条目（含 signing_pubkey_hex）。 |
| `accounts/services/activation.py` | `_entitled_plugins_for(plan)` → `_entitled_plugins_for(plan, vertical)`；activate / Stripe webhook 写 `user.vertical`。 |
| `accounts/api/member.py` | `/activate`、`/verify` 响应加 `vertical` + 按 vertical 过滤 allowed_plugins。 |
| `accounts/api/licenses.py` + `web.py`(`/me`) | 响应加 `vertical`。 |
| `accounts/api/admin.py` | admin 发放会员时可设 `vertical`（受控发放，§project_cloud_no_payment_membership_model）。 |

### pluginhub（`/data/company/cloud/pluginhub`）

- **无代码改动**。新 vertical 插件（med-pro 等）发布到 pluginhub 是**数据/运维**动作（走
  [[project_plugin_trust_chain]] 的 release SOP：cargo build bin → package-plugin.sh Ed25519 签 →
  upload）。本 spec 不改 pluginhub 路由/付费门逻辑。

---

## 5. API 契约

### cloud → client（响应增量，全部 `#[serde(default)]` 向后兼容）

**`POST /api/v1/login` / `GET /api/v1/me`（UserResponse）**：
```jsonc
{
  "id": 9, "email": "lawyer@x.com",
  "plan": "pro",
  "vertical": "law",                    // ← 新增；null/缺省 = 未指定场景
  "gateway_token": "sk-...", "gateway_url": "https://gateway.engi-stack.com/v1",
  "gateway_default_model": "deepseek-v4-flash"
}
```

**`GET /api/v1/licenses`（每条 License）**：
```jsonc
{
  "id": 42, "plan": "pro", "license_key": "lk-...", "license_id": 7,
  "vertical": "law",                    // ← 新增（UI 展示用，可选）
  "entitled_plugins": [                 // ← 内容现按 vertical 过滤
    { "plugin_id": "law-pro", "version": "1.0.6",
      "download_url": "https://hub.engi-stack.com/api/v1/packages/law-pro-1.0.6.tar.gz",
      "signing_pubkey_hex": "8866ae9b...", "decrypt_key": null }
  ]
}
```

**`POST /api/v1/member/activate`（授权码 → MemberActivateResponse）**：
```jsonc
{
  "plan": "pro", "expires_at": "2027-06-20T00:00:00Z",
  "vertical": "law",                    // ← 新增
  "allowed_plugins": ["law-pro"],       // ← 按 vertical 过滤
  "gateway_token": "sk-...", "gateway_url": "...", "gateway_default_model": "..."
}
```

**`POST /api/v1/member/verify`（周期 re-verify，SEC-1/2 签名覆盖面**不变**）**：
- `signed_payload` 仍是 `{status, allowed_plugins, expires_at, nonce, verified_at}`，
  `allowed_plugins` **已是 vertical 过滤后的清单**。`vertical` 本身可作**非签名**展示字段；
  **授权决策只信签名覆盖的 `allowed_plugins`**（vertical 仅 UI 文案，不参与门禁 → 即便伪造 vertical
  也无法越权，因为真正决定装什么的是签名快照里的 allowed_plugins）。

### client 内部（无新 REST endpoint）

- 现有 `POST /api/v1/member/{login-password,login-token,activate-license}` 响应体加 `vertical` 字段
  透传 + `plugin_sync` 报告（已有）。**不新增 endpoint。**
- 已有 `POST /api/v1/member/entitlements/refresh` 手动重试入口可作为"自动装失败后重试"的 UI 按钮挂载点。

### admin（cloud 受控发放，§无支付会员模型）

- `POST /admin/members`（已有）body 加可选 `vertical` 字段。

---

## 6. 扩展点 / 插件接口

**加一个新 vertical（如 medical）的完整步骤**：
1. **attune-pro 仓**：开发 `med-pro` 插件 → 走 release SOP 签名打包 → upload 到 pluginhub。
2. **cloud `config.py`**：
   - `vertical_plugins_map` 加 `"medical": ["med-pro"]`。
   - `plan_plugin_meta` 加 `"med-pro": {version, download_url, signing_pubkey_hex, decrypt_key}`。
3. **attune client `plugin_anchor.rs`**：若 med-pro 用**新签名 keypair**，把其公钥 prepend 进
   `OFFICIAL_PLUGIN_ANCHORS`（≤ 3 dual-pin 窗口）→ ship desktop release。若复用 law-pro 同一官方
   keypair（推荐：单一官方发布锚）→ **无需改 client**。
4. **cloud admin / Stripe**：发放该用户 `vertical="medical"` → 下次登录自动装 med-pro。

**enterprise 多 vertical**：`vertical_plugins_map` 可让 `plugins_for("enterprise", _)` 返回全集
（忽略 vertical，授予所有已发布 vertical 插件）——通过 plan 维度覆盖，不需 client 改动。

**设计不变量**：vertical→plugin 的真值唯一在 cloud config；client 永远只执行 cloud 下发的
entitled 清单。加 vertical = 改 cloud 数据 + （可能）加官方锚，client 自动安装逻辑零改动。

---

## 7. 错误 + 边界 case（graceful degrade，绝不阻塞登录）

| 场景 | 行为 | 实现锚点 |
|------|------|---------|
| **插件下载失败**（hub 5xx / 超时 / 网络断） | 该插件进 `SyncReport.failed[(id, reason)]`；**登录成功**；UI 提示"X 未能自动安装，点此重试"。 | `best_effort_sync_plugins` never-Err；`download_to_file` 120s timeout |
| **验签失败**（Ed25519 不匹配 / 包被篡改） | `verify_with_key` → `Crypto` err → `failed`，**不安装**（fail-closed），登录仍成功。 | `install_one_plugin` ⑥ |
| **anchor 不在 allowlist**（compromised server 换 pubkey） | `verify_plugin_anchor` → `AnchorNotPinned` → `failed` reason=`anchor-not-pinned`，**拒装**（fail-closed）。 | `verify_plugin_anchor` ⑤ |
| **部分成功**（3 装 2 成 1 败） | `installed=[2]`, `failed=[1]`；登录成功；UI 分别显示。 | `sync_plugins_with_store` 循环每条独立 |
| **完全离线**（cloud 不可达，login-token 路径无 session） | `list_licenses` Err → 空 SyncReport；登录态由本地 session 维持；不装任何插件，不报错。 | `best_effort_*` Err→empty report |
| **vertical=null / 老 cloud 无字段** | `plugins_for(plan, None)` 回退到 `plugins_for_plan(plan)`（今日行为）；client `vertical=None` → UI 不显示场景文案。 | serde default + config 回退 |
| **vertical 指向未发布插件**（map 有但 hub 无包） | 下载 404 → `failed`；登录成功。运维责任：map 只在插件已发布后加（§config 注释已立此约束）。 | download 404 → failed |
| **幂等**（已装 law-pro 再登录） | `installed_ids` 命中 → `skipped_already_installed`，不重下载；lazy backfill entitlement row。 | `sync_plugins_with_store` ③ |
| **vault 锁定**（登录时 vault 未解锁） | entitlement 快照写入 skip（warn）；插件仍可装到 plugins_dir（不依赖 vault）。 | `store_activation_entitlements` 容缺 |
| **vertical 变更**（law→medical 后登录） | 装 med-pro；law-pro **留着不删**（防误删学习状态）。用户可手动卸载。 | "多余的留着" 策略 |

**错误码（kebab，复用现有）**：`anchor-not-pinned` / `paid-verification-failed` /
`cloud-unreachable` / `activate-failed`。无需新增。

---

## 8. 成本契约

| 动作 | 资源层 | 谁买单 | UI 显示 |
|------|--------|--------|---------|
| vertical→entitled 解析 | cloud CPU 毫秒 | 🆓 零成本 | 无 |
| 插件下载（.tar.gz，law-pro 14.6MB） | 网络 | ⚡ 一次性流量（登录后台） | "正在为〔律师〕场景安装 law-pro…（一次性下载 ~15MB）" |
| 验签 + 解压 + 装载 | 本地 CPU 秒级 | 🆓 零成本 | spinner |
| 自动安装本身 | — | **不涉及 LLM**（无 token 花费） | 明确标注"本地安装，无 API 费用" |

**关键**：自动安装是 **第二层（本地算力 + 一次性网络）**，**永不升级到第三层 LLM**。下载在登录后
后台 spawn_blocking 跑，不阻塞 UI；顶栏后台任务可见（复用现有任务队列 UI）。

---

## 9. 测试矩阵

| # | 类型 | 场景 | 判据 | 视角 |
|---|------|------|------|------|
| T1 | happy | pro+vertical=law 登录 → 自动装 law-pro | `installed=["law-pro"]`，plugins_dir 有目录 | 灰盒 |
| T2 | happy | cloud `plugins_for("pro","medical")` | 返回 `["med-pro"]` | 白盒(cloud) |
| T3 | 幂等 | 已装 law-pro 再登录 | `skipped_already_installed=["law-pro"]`，不重下载 | 灰盒 |
| T4 | 验签失败 | 篡改包 / 错 pubkey | `failed` 含 sig 错，**不装**，登录仍成功 | 白盒 |
| T5 | adversarial | compromised server 发 off-allowlist pubkey | `AnchorNotPinned`，fail-closed（已有 `anchor_check_rejects_off_allowlist_key`） | 对抗 |
| T6 | adversarial | 客户端伪造 vertical（改 login 响应明文 vertical） | 装什么只由签名 `allowed_plugins` 决定 → 伪造 vertical 无法越权装未授权插件 | 对抗 |
| T7 | error | hub 5xx / 下载 404 | `failed[(id,reason)]`，登录成功（never-Err） | 黑盒 |
| T8 | 离线 | cloud 不可达 | 空 SyncReport，登录成功，不 panic（已有 `best_effort_sync_returns_empty_report`） | 黑盒 |
| T9 | 部分成功 | 2 插件 1 成 1 败 | installed/failed 各列对 | 灰盒 |
| T10 | 向后兼容 | 老 cloud 无 vertical 字段 | `vertical=None`，回退 `plugins_for_plan`，行为同今天 | 白盒 |
| T11 | free | free tier 登录 | 不自动装任何 pro 插件 | 黑盒 |
| T12 | 边界 | vertical=null / 未知 vertical 字符串 | 回退默认，不 panic | 白盒 |
| T13 | 升级保留 | vertical 变更/插件升级 | 用户 agent_state 不丢（已有 `plugin_upgrade_preserves_user_agent_state`） | 灰盒 |
| T14 | E2E | 真 cloud + 真 pluginhub + Playwright 登录 wizard → 看 UI"已安装 X" | §7.3 真 artifact 真路径 | 黑盒/E2E |
| T15 | cloud migration | `0004_user_vertical` up/down | 老数据 vertical=NULL，无丢失 | 白盒 |

**LLM 维度**：本 feature **不涉及 LLM agent**（纯下载/验签/装），无需 N=3 multi-seed F1 gate。
**deterministic 判据 pass rate = 1.00。**

---

## 10. 向后兼容

| 维度 | 策略 |
|------|------|
| **老 client + 新 cloud** | 老 client 无 vertical 解析 → 忽略 `vertical` 字段（serde 丢未知键）；仍按 `entitled_plugins` 自动装（cloud 已按 vertical 过滤好），**无感升级**。 |
| **新 client + 老 cloud** | 老 cloud 不返回 `vertical` → client `vertical=None` → 不显示场景文案；entitled 清单仍来自 `plugins_for_plan`（cloud 回退），**行为同今天**。 |
| **DB migration** | `0004_user_vertical` add column nullable，老用户 `vertical=NULL`，**零数据丢失**；down migration drop column 可逆。 |
| **已手动装的插件** | `installed_ids` 短路 → 不重装、不覆盖用户自装版本（`skipped_already_installed`）。 |
| **entitlement 签名契约** | SEC-1/2 签名覆盖面 `{status, allowed_plugins, expires_at, nonce, verified_at}` **不变**；vertical 是新的**非签名**展示字段，不破坏现有签名/anti-replay 测试。 |
| **plan_plugins_map → vertical_plugins_map** | cloud 保留 `plugins_for_plan` 作 `vertical=None` 回退；env override key 兼容（新 `ACCOUNTS_VERTICAL_PLUGIN_MAP`，旧 `ACCOUNTS_PLAN_PLUGINS_MAP` 作回退）。 |

---

## 11. 风险登记

| # | 风险 | 等级 | 缓解 |
|---|------|------|------|
| R1 | **供应链：自动下载扩大攻击面** —— 用户没点"安装"就有代码落盘，恶意/被篡改包风险放大 | **高** | **强制三道门，全部 fail-closed**：① W1 `verify_plugin_anchor`（signing_pubkey_hex ∈ 编译期 `OFFICIAL_PLUGIN_ANCHORS`，防 compromised server 换 key）② `verify_with_key` Ed25519 验签（防包篡改）③ 周期 re-verify 走 SEC-1/2 签名快照（防吊销逃逸）。任一失败 → 不装 + 进 `failed`。**不新增任何旁路**；tar 解压用净化 `..`/绝对路径的 Rust `tar` crate（防解压穿越，已有）。 |
| R2 | **entitlement 伪造越权** —— 攻击者改 login 响应明文 `vertical` 或 `allowed_plugins` 装未授权插件 | **高** | 装什么的**权威是签名快照里的 `allowed_plugins`**（SEC-1 Ed25519 + SEC-2 nonce anti-replay），`vertical` 仅 UI 文案不参与门禁；且每个 `signing_pubkey_hex` 必须 ∈ 官方锚 → 即便伪造清单，包也下不来/验不过。cloud_url 不接受客户端覆盖（已有 SSRF/paywall 防护）。 |
| R3 | **vertical 指向未发布插件** → 自动装 404 噪声 | 中 | config 注释立约束「map 只在插件已发布到 pluginhub 后才加 vertical」（已有同类注释）；404 → `failed` 非阻塞。 |
| R4 | **新 vertical 用新签名 key 但 client 没 ship 新锚** → 自动装全 `anchor-not-pinned` | 中 | 优先**复用单一官方发布 keypair**（law-pro 同锚）→ 新 vertical 无需改 client；若必须新 key，走 dual-pin（≤3）跨仓发布顺序（先 ship client 锚，再 cloud 加 map）。 |
| R5 | **登录被插件 sync 拖慢/阻塞** | 中 | sync 在 `spawn_blocking` 后台跑，登录响应**不等** sync 完成即可返回（当前是同线程返回报告——评审决策：是否改为 fire-and-forget + 异步报告，权衡"登录即见结果" vs "登录快"）。`download_to_file` 120s timeout 封顶。 |
| R6 | **vertical 与 plan 不一致**（free 账号误设 vertical） | 低 | `plugins_for(plan, vertical)` 中 plan 仍是付费门第一闸；free → []（无论 vertical），entitled 为空不装。 |
| R7 | **并发登录/多设备** 重复 sync | 低 | 幂等 `installed_ids` 短路；plugin install 原子 rename（已有 `.installing` staging）。 |
| R8 | **cloud DB migration 失败** | 低 | nullable add column 最低风险；down 可逆；§7.2 RC gate 跑 migration up/down 测试（T15）。 |

---

### 评审决策点（待用户/Panel 拍板）

1. **R5**：登录响应是否等 sync 完成？（当前同步返回报告。建议保留同步=用户登录即见"已安装 X"，
   但加 UI loading 态。）
2. **单锚 vs 多锚**：新 vertical 插件强制复用 law-pro 官方 keypair（单锚，client 零改动）还是允许
   per-vertical key（多锚，dual-pin 管理）？建议**单一官方发布锚**。
3. **vertical 值域**是否固化为 6 个枚举（law/medical/patent/presales/tech/academic）还是开放字符串？
   建议 cloud 端枚举校验，client 端容忍任意字符串（前向兼容未来 vertical）。
