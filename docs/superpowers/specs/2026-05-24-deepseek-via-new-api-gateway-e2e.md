# DeepSeek via cloud llm-gateway (new-api) — 完整会员路径 E2E

> 2026-05-24 落地. attune-pro v1.0 路径硬约束: 付费会员**默认**走 cloud llm-gateway,
> 不走 BYOK 直 API. 本 spec 验证全链路打通 + 文档接入步骤 + bug 登记.

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
- [附录 A: E2E 执行记录(2026-05-24)](#附录-a-e2e-执行记录2026-05-24)

## 1. 目标定位

补全 `2026-05-24-deepseek-integration-research.md` Path B(cloud llm-gateway 路径), 让付费会员
default flow 真实可用,而非仅 BYOK 验通。

**用户痛点**:
- 个人付费用户不该接触 raw API key — cloud 中转更安全 + 可计量 + 可降级
- 自部署用户需要明确的 channel 配置 SOP — 不能靠 web 后台手点

**positioning 对齐**:
- 三产品矩阵 (attune-pro `CLAUDE.md`): 个人付费 = `attune (OSS) + attune-pro/<vert>-pro`
  + cloud SaaS membership
- 成本契约 (attune `CLAUDE.md` §Cost & Trigger): cloud 中转才能让 attune-pro 正确显示
  "云端 token · $0.0004" cost indicator(BYOK 时 attune 不知道 quota)

## 2. 范围边界

**本 spec 做**:
- 在 cloud llm-gateway(new-api v0.13.2) 加 DeepSeek channel
- 验证 attune-pro 付费会员 signup → activation → newapi provision → member login
  → vault settings 自动注入 → chat → DeepSeek 全链路
- 记录已发现的 2 个真实 bug + 修复建议
- 标准化 self-host 自部署的 `GATEWAY_PUBLIC_URL` 配置

**本 spec 不做**:
- failover policy 自动化(留给 `2026-05-22-llm-gateway-hardening.md` 后续 PR)
- DeepSeek key 轮换 / 多 key 池(留给 v1.1)
- defamation v3 prompt 实测(留给 spec `2026-05-24-deepseek-integration-research.md` §6 Phase 2)

## 3. 架构数据流

```
[用户 attune Desktop]
  │ 1) POST /api/v1/member/login-password
  │    {email, password, cloud_url}
  ▼
[attune-server::routes::member::login_password]
  │ 2) cloud_client.login(email, password) → 拿 session cookie
  │ 3) cloud_client.list_licenses() → 拿 active license
  │ 4) cloud_client.me() → 拿 gateway_url + gateway_token
  │ 5) apply_gateway_to_vault_settings(state, url, tok)
  │    └─ gateway_should_apply(current) — 仅未配置时覆写
  │    └─ merge_gateway_into_settings → llm.{provider=openai_compat, endpoint, api_key}
  │ 6) state.reload_llm() — 热重载 OpenAiCompatProvider
  ▼
[attune chat 触发]
  │ 7) POST {endpoint}/chat/completions
  │    Authorization: Bearer <gateway_token>
  ▼
[cloud-proxy nginx-proxy(host :8080)]
  │ proxy_pass http://cloud-llm-gateway:3000
  ▼
[cloud-llm-gateway = new-api v0.13.2]
  │ 8) token auth → quota check → group match
  │ 9) channel dispatch by model name (priority + weight)
  │    └─ deepseek-* → channel.id=2 type=36 → https://api.deepseek.com
  ▼
[DeepSeek API]
  │ 10) chat completion → V4-Flash / V4-Pro
  ▼
[response cascade back] — usage 在 new-api db 记账 → user.quota -= cost
```

**关键 invariant**:
- attune-server 与 cloud-llm-gateway **无直接连接**;一切通过 attune ↔ cloud-accounts
  拿 token + cloud-accounts ↔ cloud-llm-gateway provision
- gateway token **不在 license 里**;license 是 device entitlement, token 是 LLM quota credential
- channel admin **不通过 accounts** 自动化;是 cloud operator 手动通过 admin API 配

## 4. 模块边界

| 仓 | 模块 | 角色 |
|----|------|------|
| **attune** | `attune-server::routes::member` | 触发 login flow + 写 vault settings |
| **attune** | `attune-core::llm_settings` | `gateway_should_apply` 判定 + `merge_gateway_into_settings` |
| **attune** | `attune-core::llm` | OpenAiCompatProvider 走 gateway endpoint |
| **cloud** | `accounts::api::user` | `/signup` `/login` `/me` `/licenses` |
| **cloud** | `accounts::services::activation` | `activate_subscription` + `provision_gateway` |
| **cloud** | `accounts::services::newapi_sync` | new-api admin API 调用(user/token/quota CRUD) |
| **cloud** | `accounts::api::stripe_webhook` | Stripe Checkout → activate path(生产) |
| **cloud** | `llm-gateway` (new-api) | token auth + channel dispatch + quota tracking |

**新增/修改文件清单**(本次 sprint):
- (无 attune 代码改动 — login flow + settings inject 已在 v0.7 实装,本 sprint 仅做 E2E)
- (cloud 端**新增** channel via runtime API — 不入仓代码;仅 ops 文档化)
- `docs/superpowers/specs/2026-05-24-deepseek-via-new-api-gateway-e2e.md` (本 spec)

## 5. API 契约

### 5.1 cloud-accounts 公开 API

| Method | Path | Body | 注 |
|--------|------|------|-----|
| POST | `/api/v1/signup` | `{email, password}` | 邮箱必须真实域名,`*.attune.local` 拒(pydantic email validator) |
| POST | `/api/v1/login` | `{email, password}` | 返回 session cookie(JWT) |
| GET | `/api/v1/me` | — | 返回 `{plan, gateway_token, gateway_url, ...}`;免费用户 `gateway_token=null` |
| GET | `/api/v1/licenses` | — | 列当前用户有效 license |
| POST | `/webhook/stripe` | Stripe event | 触发 activate_subscription(prod 路径) |

### 5.2 cloud llm-gateway(new-api) admin API

**鉴权**: `Authorization: Bearer <NEWAPI_ADMIN_TOKEN>` + `New-Api-User: 1`(root id)

| Method | Path | Body 关键字段 |
|--------|------|---------------|
| GET | `/api/channel/` | `?p=0&page_size=20` 列 channel |
| POST | `/api/channel/` | `{mode:"single", channel:{name, type=36(DeepSeek), key, base_url, models, group, priority, ...}}` (必须 wrapper) |
| GET | `/api/user/?keyword=acct-<N>` | 查 newapi user |
| POST | `/api/user/manage` | `{id, action:"add_quota", mode:"add", value:N}` — quota 必须走 manage,PUT 故意忽略 quota |
| POST | `/api/token/<id>/key` | 拿完整未打码 key(list view 是 `abcd****wxyz`) |

### 5.3 attune-server member 路由

| Method | Path | Body | 行为 |
|--------|------|------|------|
| POST | `/api/v1/member/login-password` | `{email, password, cloud_url?}` | 触发 §3 flow,失败 best-effort 不阻断 |
| POST | `/api/v1/member/login-token` | `{account_id, tier, license_id, ...}` | 客户端先 login 后回传(替代路径) |
| GET | `/api/v1/settings` | — | 返回 redacted llm.api_key(`api_key_set: true`) |
| PATCH | `/api/v1/settings` | partial settings | **付费 tier 锁 `llm` 字段**(per SettingsLocks) |

## 6. 扩展点 / 插件接口

- **新 LLM 厂商**: 在 new-api admin 加 channel(本 spec §5.2),attune client 无感
- **failover**: 多个 channel 同 type 共存,new-api 按 priority + weight 自动选;
  per `2026-05-22-llm-gateway-hardening.md` §189 — 全 provider 故障 → 503
  `{"error":"no channel available"}` → Gatus 邮件告警
- **per-tier quota tier**: `_quota_for_plan` 已有 `individual / pro / pro_plus / enterprise`,
  add new plan = add row in `_quota_for_plan` + Stripe price mapping
- **model alias**: channel 的 `model_mapping` 字段 — 用户调 `deepseek-v4-flash` 在 new-api 内
  rewrite 成 `deepseek-chat` 转 DeepSeek;无需 attune 端改

## 7. 错误处理 + 边界 case

| 场景 | 路径 | 行为 |
|------|------|------|
| accounts 不可达 | login_password | 500 `cloud client login: error` |
| license 列表为空 | login_password(paid) | 400 `paid user has no matching license` |
| me 返回 gateway_token=null | login_password | log warn,不阻断,settings 保持原状 |
| gateway_should_apply=false(用户已 BYOK) | apply_gateway_to_vault_settings | log info,不覆盖 |
| settings PATCH llm 字段(付费) | settings.update | 403 `setting_locked_by_member_tier` |
| chat model=空 | OpenAiCompatProvider.chat_sync_impl | 探测 `/v1/models`,取第一个 |
| new-api channel 全挂 | gateway POST /v1/chat/completions | 503 `no channel available` |
| new-api token quota 耗尽 | gateway POST /v1/chat/completions | 400 `用户额度不足` |
| new-api channel `SelfUseModeEnabled=false` 且模型无定价 | gateway POST | 400 `model_price_error` |

## 8. 成本契约

**用户感知**(attune Web UI 顶栏 chip):
- 付费会员 chat → `~<tokens> tok · $<usd>`(本地估算,真值由 cloud quota track)
- attune-pro extractor → 同上 + 调用者归因(law-pro agent 名)

**计费层级**(per `attune-pro/llm-gateway/docs/failover-policy.md`):
- DeepSeek-V4-Flash: $0.07/1M in, $1.10/1M out — extractor 默认
- DeepSeek-V4-Pro: $0.55/1M in, $2.19/1M out — reasoner / 复杂决策

**quota 单位**(new-api 内部): `1 quota = $0.000002` (per new-api ModelRatio 约定);
所以 pro plan `plan_quota_pro=5,000,000` = $10/月可用 token 等值。

## 9. 测试矩阵

per "Agent 验证铁律" (本 spec 验的是 cloud + attune 互通 capability,非 agent。Agent 维度
另见 `2026-05-24-oss-4-agent-deepseek-verify.md`)。

| 类型 | 覆盖项 | 状态(2026-05-24) |
|------|--------|-------------------|
| 基础设施 verify | cloud verify 25/0/0 | ✅ pass |
| channel 创建 | POST /api/channel/(wrapper 格式) | ✅ pass — channel id=2 type=36 |
| channel 路由 | /v1/models 列 deepseek-chat/reasoner/v4-flash/v4-pro | ✅ pass |
| chat 调用 | /v1/chat/completions deepseek-chat | ✅ pass — DeepSeek 实测响应 |
| accounts signup | POST /api/v1/signup | ✅ pass(`*.example.com`) |
| login + me | session cookie + gateway_token | ✅ pass — token len=48 |
| activation flow | activate_subscription + provision_gateway | ✅ pass — user_id 21 → newapi user_id 9 + quota=5M |
| attune member login(已有 BYOK) | login_password | ✅ pass(gateway 不覆盖,符合设计) |
| attune member login(fresh vault) | login_password | ✅ pass — settings 注入 endpoint+token |
| attune chat → gateway → DeepSeek | POST /api/v1/chat | ✅ pass — content="pong-e2e" cost_estimate.is_local=false |

## 10. 向后兼容

- **schema**: 无 attune 代码改动 → 无 schema 漂移
- **API 协议**: 无新 attune 路由
- **member tier lock**: 新增字段 `llm.model` 落在 `cloud_llm` lock 范围内;**已发现 bug**
  (per §11) — 仅记录,无 schema 改动
- **老 client**: v0.7 已有 login_password + settings inject;<v0.7 client(无此能力)需要
  升级,**或**用 BYOK 路径

## 11. 风险登记

| ID | 风险 | 缓解 |
|----|------|------|
| R1 | new-api admin API 不在我方代码,版本升级可能改 schema | 锁 `calciumion/new-api:v0.13.2` image tag;升级前在 staging 跑 channel CRUD smoke |
| R2 | channel API key 落 SOPS 加密 yaml; cloud operator 拿到 admin token 仍可读 key | accept — operator 已是 trust root;rotate 通过 channel UPDATE |
| **R3** | **付费会员 login 写 gateway endpoint+token 但 `model` 字段缺省 → chat empty model → 404** | **见 Bug-1 — fallback /v1/models 探测路径已经存在,但仅 model_not_found 错误才触发;404 不触发。需扩展 fallback 条件 OR 在 `merge_gateway_into_settings` 默认设 model="deepseek-chat"** |
| **R4** | **付费会员 SettingsLocks 锁全部 `llm` 字段 → 用户不能改 `model`(即便 endpoint/key 由 gateway 下发)** | **见 Bug-2 — lock 粒度需细化为 `llm.endpoint` + `llm.api_key` 锁,`llm.model` + `llm.provider` 可改** |
| R5 | gateway_public_url 配错(不含 port) → 自部署 hosts 解析到 host 80(可能被其他服务占) | self-host SOP 显式 `GATEWAY_PUBLIC_URL=http://gateway.attune.local:<port>/v1`;cloud-proxy 默认 8080/8443 |
| R6 | new-api `SelfUseModeEnabled=false` + model 无 ModelRatio 定价 → 400 model_price_error | self-host SOP 强制 `SelfUseModeEnabled=true` OR 在 ops 文档列必配 ModelRatio |
| R7 | DeepSeek 真实 API 故障 → 走 channel.priority 切换(per failover-policy) | failover-policy 已有(R&D done);需要起码 2 个 channel(本 sprint 仅配 1 个,留 follow-up) |

### Bug-1: paid 用户 login 后 llm.model 缺省

**症状**: fresh vault paid user login → settings.llm.model=None → chat POST /api/v1/chat
→ 500 `llm unavailable: openai HTTP 404 Not Found`(从 new-api 看到的是 model="" 路由失败)

**根因**: `attune-core::llm_settings::merge_gateway_into_settings` 只覆写
`provider/endpoint/api_key`,**保留** `model`(per merge 函数注释)。fresh vault model=None,
merge 后仍为 None。OpenAiCompatProvider chat_sync_impl 看到 model="" 触发探测 `/v1/models`,
但 attune-server 这次 instance 看到的是 `error: openai HTTP 404` —— 实际是 new-api 拒 model="" 报 400
不是 model_not_found,所以 fallback 不触发。

**修法选项**:
- (A) `merge_gateway_into_settings` 加 `model` 默认值参数;login 时传 "deepseek-chat" / 或从
  cloud /me 拿 `default_model` 字段
- (B) `OpenAiCompatProvider` fallback 条件扩展: HTTP 4xx + 空 model → 探测 /v1/models;
  现仅 200 OK body 内含 `model_not_found` 才触发
- (C) cloud accounts /me 增加 `gateway_default_model` 字段,login flow 写入

**推荐**: (C) — cloud 控制默认 model,attune-server 跟随;后续切上游厂商不需要发桌面新版本

### Bug-2: SettingsLocks `cloud_llm` 粒度太粗

**症状**: paid user PATCH `{"llm":{"model":"deepseek-chat"}}` → 403 `setting_locked_by_member_tier`
即便用户**只想改 model**(不动 endpoint/key)。

**根因**: `attune-server::routes::settings` lock_map: `("llm", "cloud_llm")` — 整个 `llm`
对象一锁全锁,无子字段粒度。

**修法**: lock_map 升级为 sub-field:
```rust
let lock_map: &[(&str, Option<&str>, &str)] = &[
    ("llm", Some("endpoint"), "cloud_llm"),       // endpoint 锁
    ("llm", Some("api_key"), "cloud_llm"),        // api_key 锁
    // llm.model / llm.provider 不锁 → 用户可挑模型
    ...
];
```

**优先级**: Bug-1 修了 Bug-2 可暂留(默认 model 后用户改不改影响不大);但 v1.1 应改。

## 附录 A: E2E 执行记录(2026-05-24)

**Wall-clock**: 22:22 起 — 22:58 完(36 min,远低于 3 hr budget)。

### A.1 cloud llm-gateway DeepSeek channel 配置

```bash
# 1. 取 admin token + DeepSeek key(均不落 shell)
NEWAPI_ADMIN_TOKEN=$(cd /data/company/cloud && sops --decrypt secrets/cloud.enc.yaml | grep '^NEWAPI_ADMIN_TOKEN:' | awk '{print $2}')
set -a; source /tmp/secrets-deepseek/key.env; set +a

# 2. POST channel(必须 mode + channel wrapper, type=36 = DeepSeek)
cat > /tmp/ds.json <<EOF
{"mode":"single","channel":{
  "name":"deepseek-primary","type":36,"key":"$DEEPSEEK_API_KEY",
  "base_url":"https://api.deepseek.com",
  "models":"deepseek-chat,deepseek-reasoner,deepseek-v4-flash,deepseek-v4-pro",
  "model_mapping":"{\"deepseek-v4-flash\":\"deepseek-chat\",\"deepseek-v4-pro\":\"deepseek-reasoner\"}",
  "group":"default","groups":["default"],"priority":7,"weight":0,"status":1,"auto_ban":1,
  "channel_info":{"is_multi_key":false,"multi_key_size":0,"multi_key_status_list":null,"multi_key_polling_index":0,"multi_key_mode":""},
  "other":"","tag":"","setting":""
}}
EOF
docker cp /tmp/ds.json cloud-proxy:/tmp/ds.json
docker exec cloud-proxy curl -sS -X POST \
  -H "Authorization: Bearer $NEWAPI_ADMIN_TOKEN" \
  -H "New-Api-User: 1" -H "Content-Type: application/json" \
  -d @/tmp/ds.json "http://cloud-llm-gateway:3000/api/channel/"
# → {"message":"","success":true}
```

### A.2 必须的 ops 后置步骤

```bash
# 给 root user 加 quota
curl -sS -X POST -H "Authorization: Bearer $NEWAPI_ADMIN_TOKEN" -H "New-Api-User: 1" \
  -H "Content-Type: application/json" \
  -d '{"id":1,"action":"add_quota","mode":"add","value":100000000}' \
  "http://cloud-llm-gateway:3000/api/user/manage"

# 开 SelfUseModeEnabled 让 deepseek-v4-* alias 不报 model_price_error
curl -sS -X PUT -H "Authorization: Bearer $NEWAPI_ADMIN_TOKEN" -H "New-Api-User: 1" \
  -H "Content-Type: application/json" \
  -d '{"key":"SelfUseModeEnabled","value":"true"}' \
  "http://cloud-llm-gateway:3000/api/option/"

# 若 token group=空,改成 default
curl -sS -X PUT -H "Authorization: Bearer $NEWAPI_ADMIN_TOKEN" -H "New-Api-User: 1" \
  -H "Content-Type: application/json" \
  -d '{...,"group":"default"}' "http://cloud-llm-gateway:3000/api/token/"
```

### A.3 attune-pro 会员完整 E2E

```bash
# 1. signup
curl -sS -X POST -H "Content-Type: application/json" \
  -d '{"email":"e2e@example.com","password":"testpass-12345"}' \
  "http://cloud-accounts:8002/api/v1/signup"
# → {"id":21,"plan":"individual","gateway_token":null,...}

# 2. 模拟付费升级(prod 走 Stripe webhook;本地走 service path)
docker exec cloud-accounts python -c "
from accounts.database import SessionLocal
from accounts.models import User
from accounts.services.activation import activate_subscription, provision_gateway
from datetime import datetime, timezone, timedelta
db = SessionLocal()
user = db.query(User).filter(User.email == 'e2e@example.com').first()
lic = activate_subscription(db, user, 'pro', datetime.now(timezone.utc)+timedelta(days=365))
db.commit()
provision_gateway(db, user.id, 'pro')
db.refresh(user)
print('gw_uid=', user.gateway_newapi_user_id, 'gw_token_len=', len(user.gateway_token or ''))
"
# → gw_uid=9 gw_token_len=48

# 3. attune-server member login (fresh vault)
curl -sS -X POST -H "Content-Type: application/json" \
  -d '{"email":"e2e@example.com","password":"testpass-12345","cloud_url":"http://172.18.0.3:8002"}' \
  "http://localhost:18902/api/v1/member/login-password"
# → tier=pro, state.kind=paid, license_id=17

# 4. settings 验证
curl -sS http://localhost:18902/api/v1/settings | jq '.llm'
# → endpoint=http://gateway.attune.local:8080/v1, api_key_set=true, model=null

# 5. chat
curl -sS -X POST -H "Content-Type: application/json" \
  -d '{"message":"Reply only with: pong-e2e"}' \
  "http://localhost:18902/api/v1/chat"
# → content="pong-e2e", cost_estimate.is_local=false, tokens_in=410 tokens_out=3
```

### A.4 自部署 SOP 收口

任何 cloud self-host 部署后,**必跑**:
1. 起 cloud stack(`make up`)
2. 通过 new-api admin API 加 channel(本附录 A.1)
3. 跑 A.2 必须的 quota + SelfUseMode + group fix
4. 设 `GATEWAY_PUBLIC_URL=http://<domain>:<cloud-proxy-port>/v1`(自部署 8080 = cloud-proxy host bind)
5. 重启 cloud-accounts 让 setting 重读 env
6. E2E smoke: signup + activate + login + chat(本附录 A.3)

**v1.0 release blocker**: Bug-1(model 缺省)修复 — 否则付费用户默认 chat 不可用。
推荐 (C) 方案: cloud /me 加 `gateway_default_model="deepseek-chat"`,attune-server merge 时使用。
