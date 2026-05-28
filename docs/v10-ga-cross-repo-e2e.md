# v1.0 GA — Cross-repo E2E: accounts → license → attune desktop pickup → cloud LLM chat

**Date**: 2026-05-21
**Author**: cross-repo E2E orchestrator (Claude agent)
**Scope**: 三仓真实联动 — `/data/company/cloud` (accounts + new-api gateway) + `/data/company/project/attune` (attune-server + CLI) + `/data/company/project/attune-pro` (商业增强，本次未直接触发)

---

## 1. TL;DR — Go/No-Go

| 维度 | 评估 | 状态 |
|------|------|------|
| **核心链路** (signup → webhook → provision → member login → settings inject → chat 走 cloud gateway) | **6/8 stage PASS，1 个外部 transient，1 个 product bug** | ⚠️ **No-Go for GA**（详见 Finding-A） |
| accounts ↔ new-api gateway 同步 | OK (acct-20 user 自动创建 + access_token 注入) | ✅ |
| stripe webhook dev-mode bypass | OK (无 STRIPE_WEBHOOK_SECRET 时直接 parse JSON) | ✅ |
| attune-server `/api/v1/member/login-password` 接收云端凭据 | OK (200, plan=pro, license_id=16) | ✅ |
| vault settings 自动注入 `llm.endpoint` + `api_key` | OK (`http://172.31.0.8:3000/v1` + token written, api_key_set=true) | ✅ |
| **vault settings 自动注入 `llm.model`** | **NOT SET** (字段为 null) — 是 GA blocker | ❌ |
| chat 真走 cloud gateway 并消费 newapi quota | **OK，但只有用户手动设 model 后才能 chat**；OpenAI/hiapi/gpt-4o-mini 真返回 `"content":"PONG"`；acct-20 used_quota 0→21 | ⚠️ |

**结论**: 三仓 wiring 实际打通了，新会员"开箱即用"的最后 1cm 没接好 — `merge_gateway_into_settings()` 不写 model 字段，而 paid 会员的 llm 设置被锁，用户没办法补；newapi 拒空 model 名导致 chat 返回 400。**GA 阻塞，需要先合 Finding-A 的 fix**。

---

## 2. 环境（本次跑的版本基线）

| 组件 | 版本 / commit |
|------|--------------|
| attune binary | `target/release/attune` + `attune-server-headless` (本仓 develop @ `7e53767`) |
| cloud accounts | `cloud-accounts:latest` (FastAPI, in-container) |
| new-api (LLM gateway) | `cloud-llm-gateway` v0.13.2 |
| upstream LLM channel | `hiapi` (gpt-4o-mini / gpt-4o / gpt-3.5-turbo), channel.status=1, hiapi.online |
| stripe | `STRIPE_SECRET_KEY` / `STRIPE_WEBHOOK_SECRET` 未配（dev mode → webhook 跳过验签 + plan fallback "pro"） |
| sops 密钥 | secrets/cloud.enc.yaml v3.13.1 通过 secrets-check |

E2E 运行机：本机 host 直访 docker 内 container IP (`172.31.0.7:8002` accounts、`172.31.0.8:3000` new-api)。

---

## 3. 准备工作里发现并修复的运维问题

| # | 问题 | 根因 | 修复 |
|---|------|------|------|
| P1 | `cloud-accounts` 容器 restart loop | DB 密码未注入（先前 `make up` 没经 sops 包装重启过） | `make up-accounts` (走 secrets.sh 解密 sops 注入 env) → up |
| P2 | nginx-proxy 路由 `accounts.attune.local` 返回 503 | `accounts_accounts-network` 在 cloud-proxy 容器外不可达，docker-gen 把它选为 upstream | 绕过 proxy，host 直访容器 IP（本地 E2E 完全合法路径） |
| P3 | `gateway_public_url` 默认 `https://gateway.engi-stack.com/v1` 在本地 unreachable | accounts 容器无 `GATEWAY_PUBLIC_URL` env 覆盖默认值 | 临时在 `accounts/docker-compose.yml` 加一行 `GATEWAY_PUBLIC_URL` env，跑完测试后 revert（已 restored） |

P1+P2 是 cloud 部署环境问题（与代码无关）。P3 揭示了 **docker-compose 缺一个 env 占位** — 任何本地 E2E / 自部署用户都会撞到，建议补默认 placeholder 配置（attune-pro 上云线 / 文档补充）。

---

## 4. 8-Stage 详细流水（按时间顺序）

完整运行日志 `/tmp/cross-repo-e2e/result.json` 已落盘；下表是每一步的关键耗时与判定。

| Stage | 操作 | 实测响应 | 耗时 | 判定 |
|-------|------|---------|------|------|
| **1_signup** | `POST {acc}/api/v1/signup` `{email,password}` | `{id:20, plan:individual, gateway_token:null, gateway_url:"http://172.31.0.8:3000/v1"}` | 0.44s | ✅ |
| **2_stripe_webhook** | `POST {acc}/webhook/stripe` 伪造 `checkout.session.completed`（dev 跳验签） | `{received:true, plan:"pro"}` | 0.21s | ✅ |
| **3_verify_provision** | poll `{acc}/api/v1/me` until plan=pro & gateway_token 不空 | 2s 内变成 `{plan:"pro", plan_expires:"2027-05-21", gateway_token:"dB5y...PUJyN05", gateway_url:"http://172.31.0.8:3000/v1"}` | 2.03s | ✅ |
| **4_attune_setup** | `POST {attune}/api/v1/vault/setup` (skipped — vault 已 setup) | n/a | — | ✅ skipped |
| **5_unlock_api** | `POST {attune}/api/v1/vault/unlock` | `{status:ok, token:"…"}` | 0.68s | ✅ |
| **6_member_login** | `POST {attune}/api/v1/member/login-password` `{email,password, cloud_url:"http://172.31.0.7:8002"}` | `{status:ok, state:{kind:paid, account_id:20, license_id:16}, tier:pro}` | 0.36s | ✅ |
| **7_verify_settings** | `GET {attune}/api/v1/settings` | `{llm:{endpoint:"http://172.31.0.8:3000/v1", api_key:null, api_key_set:true, provider:"openai_compat", model:absent}}` | 0.00s | ✅ partially（model 未写——见 Finding-A） |
| **8_chat** | `POST {attune}/api/v1/chat` `{message:"...", session_id:...}` | **首次**: HTTP 500 + new-api 503 "system cpu overloaded"（上游 transient）<br>**重试**: HTTP 500 + new-api 400 "Model name not specified" | 7.67s | ❌（见 Finding-A） |

**总链路耗时**（不含 chat 调试）: signup 到 settings 注入 ≈ **3.7 秒**（端到端 6 个 HTTP round trip，含 2s polling provisioning）。

---

## 5. Findings — 必须解决才能 GA

### Finding-A (P0 / **GA blocker**) — paid 会员 chat 即装即用断在 model 字段

**症状**: 新 paid 用户 member login 后直接发 chat → new-api 返回 `400 "Model name not specified"`。

**根因链**:

1. `attune-core::llm_settings::merge_gateway_into_settings()` 只写 `provider`/`endpoint`/`api_key`，**不写 `model`**（[`llm_settings.rs:46`](../rust/crates/attune-core/src/llm_settings.rs)）。
2. `default_settings()` 给 laptop form_factor 设 `model: null`（等 wizard 填）。
3. `build_llm_from_settings()` 把 model `unwrap_or("")` → 发空 model name。
4. `attune-core::member_session::SettingsLocks::for_state(Paid)` 把整个 `llm` 字段标 `cloud_llm: Locked` → 用户**无法**自行 PATCH 改 model。
5. 测试场景跳过 wizard 直接 member login，model 永远是 null/空 → chat 永远 400。

**实测验证**: logout 把 lock 释放后，PATCH `{"llm":{"model":"gpt-4o-mini"}}` → 200。再次 chat → **成功返回 `"content":"PONG"`，cost_usd:0.000042, tokens_out:1**。完整链路其他都正常，只缺这一步。

**修复方向（任选一）**:
- (推荐) `merge_gateway_into_settings()` 增加 `default_model` 参数，从云端 `/me` 顺带下发一个推荐 model（gpt-4o-mini 即可）。让 paid 会员开箱即用。
- 或在 `OpenaiCompatProvider::new()` 内部 fallback: 空 model 时自动调 `/v1/models` 拿第一个可用（代码已有 `resolve_openai_compat_model` 函数，只是只在 `model=="auto"` 时触发，扩到 model.is_empty() 即可）。
- 或 settings_locks 给 paid 会员**只锁 endpoint+api_key，不锁 model 子字段**，让用户能在 UI 里改 model。

任一修复都能解 GA blocker；第 (2) 路径侵入最小（4-5 行 if 改一下 condition）。

### Finding-B (P1 / 部署文档补强) — `GATEWAY_PUBLIC_URL` 在 `accounts/docker-compose.yml` 缺占位

**症状**: 本地或自部署 cloud 后，`accounts` 容器 env 没 `GATEWAY_PUBLIC_URL` 占位（仅有 `LLM_GATEWAY_BASE_URL`，那是 accounts → newapi 内部 admin 调用），attune-server 拿到的 `gateway_url` 永远是生产硬编码 `https://gateway.engi-stack.com/v1`。

**影响**: 任何"我想在内网 / 本地跑全栈"的用户 chat 都会失败。

**修复**: `accounts/docker-compose.yml` env 列表加一行：
```yaml
- GATEWAY_PUBLIC_URL=${GATEWAY_PUBLIC_URL:-https://gateway.engi-stack.com/v1}
```
+ 在 `secrets/cloud.secrets.example.yaml` 或 `.env` 文档补一段"自部署时务必覆盖到 cluster 内可达 URL"。

### Finding-C (P2 / 观察) — new-api channel.status 的可观察性

`hiapi` channel 在测试期间一次 503 "system cpu overloaded"。这是 upstream（`hiapi.online`）暂时性故障，不在我方控制。但对 attune-server 是黑盒 — 用户只看到"chat 503"，不知道是 attune 自身还是上游。

**建议**: chat handler 把上游 status / error code 透传或加 hint（"上游服务繁忙，请稍后重试"），别只回 `{"error":"llm unavailable: openai HTTP 503"}`。

---

## 6. 安全 & 隐私 — 验证项

| 验证点 | 结果 |
|--------|------|
| `GET /api/v1/settings` 是否回传明文 api_key | ❌ no — `api_key:null + api_key_set:true`（按设计 redacted）✅ |
| vault locked 状态下 `GET /api/v1/settings` 是否泄漏 | 测试时 vault unlocked，已验证 redact；locked 时 403（先前测试观察过） |
| member login-password 是否把密码持久化 | 看 `routes/member.rs:88` 注释"密码只用于本次请求，不持久化"，已遵循 ✅ |
| newapi acct-N user 密码是否可重新推导（disaster recovery） | ✅ 由 `LICENSE_HMAC_KEY + username + account_id` HMAC 派生（`newapi_sync._derive_password`） |
| stripe_webhook dev-mode 跳验签是否危险 | dev only — `is_production(settings)` 为 true 时强制要求 webhook_secret（已在 `_verify_stripe_signature` 防住） ✅ |

---

## 7. Timing

| 段 | wall-clock |
|----|-----------|
| 注册到 me /plan=pro & token 有效 | ≈ 2.7s（含 background newapi sync） |
| member login-password 到 settings inject 完成 | 0.36s |
| chat 单 round trip（含 web_search + RAG + LLM）| 7-30s（受上游网络影响大） |

---

## 8. 跨仓视角 GA Go/No-Go

| 方面 | 结论 |
|------|------|
| accounts 仓本身 API 完备 | ✅ |
| cloud-llm-gateway (new-api) wiring | ✅（acct-N user provision、token mint、quota 扣账都对了） |
| attune 仓接收 cloud 凭据并自动写 vault | ✅（endpoint + api_key 注入完美） |
| attune 仓 chat 流末端 | ❌ **断在 model 字段** — 详见 Finding-A |
| 部署/运维（自部署用户路径） | ⚠️ Finding-B 需要补 docker-compose env 占位 |

**Cross-repo GA Go/No-Go**: ❌ **No-Go**，直到 Finding-A merged & verified。Finding-A 本身改动 < 10 行（推荐 path: `OpenaiCompatProvider` 空 model → 自动 resolve）。

---

## 9. 附录

- `/tmp/cross-repo-e2e/e2e.py` — 全程驱动脚本（urllib，无外部依赖）
- `/tmp/cross-repo-e2e/result.json` — 8-stage 结构化输出
- `/tmp/cross-repo-e2e/run.log` — 完整 stdout
- accounts `docker-compose.yml` 的 GATEWAY_PUBLIC_URL 临时改动已 revert，原文件状态恢复

---

**未触动的事**:
- 没修任何 source code（红线遵守）
- 没生产数据交互（本地 docker compose + test-only email `test-v10ga-e2e-*@example.com`）
- 没在远端 stripe/openai 做任何真实交易（dev mode webhook，自家 newapi quota 21 个 token 消耗）
