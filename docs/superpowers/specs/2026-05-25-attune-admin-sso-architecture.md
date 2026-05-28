# attune-admin SSO 管理面板架构 spec

> 用户原话(2026-05-25 17:30):
> 「所有官方所需 admin 页面的管理,能否只支持 web admin 跳转管理,单点登录
>  (只有管理界面接入,其他普通用户页面的访问不影响)。
>  可以考虑制定独立单点登录的管理项目(类似于宝塔面板一样),负责所有官网的
>  更新管理以及后台文档、激活等等的管理(即管理员只能通过单点登录页面登录,
>  其他方式不再支持登录管理平台)」

## 目录

- [1. 目标定位](#1-目标定位)
- [2. 范围边界](#2-范围边界)
- [3. 架构数据流](#3-架构数据流)
- [4. 模块边界](#4-模块边界)
- [5. API 契约](#5-api-契约)
- [6. 扩展点](#6-扩展点)
- [7. 错误处理 + exit codes](#7-错误处理--exit-codes)
- [8. 成本契约](#8-成本契约)
- [9. 测试矩阵](#9-测试矩阵)
- [10. 向后兼容](#10-向后兼容)
- [11. 风险登记](#11-风险登记)

## 1. 目标定位

**用户痛点**:
- 现 5 个服务各自 admin 入口(WP `/wp-admin` / accounts `/admin` / pluginhub `/admin` / new-api `/admin` / gatus)— **5 套登录 + 5 套权限 + 5 套日志**
- 安全风险:任一 admin 入口被破即可侧入
- 运维心智重:user 切来切去多个 admin UI

**理想形态(宝塔面板风格)**:
- 一个统一 `admin.engi-stack.com` 入口
- 单点登录(SSO + 2FA 强制)
- 一个面板做完所有运维任务(官网内容更新 / 文档 publish / 激活管理 / 用户审核 / plugin 审核 / LLM gateway / monitor)
- 其他 admin 入口**全部关闭**(IP whitelist 仅 admin server)

**产品 positioning 对齐**:
- 隐私 / 本地优先:admin 数据(audit log / admin 账号)独立 DB,不混入 user DB
- 简洁:user 端 0 影响(www / wiki / accounts 用户登录正常)
- 自主可控:**自研 JWT-based SSO**(避免 Keycloak 重型依赖)

## 2. 范围边界

**做**:
- 新建 `cloud/admin/`(submodule:qiurui144/attune-admin)
- 子域 `admin.engi-stack.com`
- SSO 认证:username + password + **TOTP 2FA 强制**
- 5 服务管理 panel(WP / wiki / accounts / pluginhub / gateway)
- 激活管理(plugin 审核 / user 升级 / refund 处理)
- 内容更新 workflow trigger(sync-content.sh 集成)
- Audit log(append-only 30 天 retention)
- 角色:super-admin / admin / read-only

**不做**(v1.0.6 范围)推 v1.0.7+:
- 多 tenant admin(per org 隔离)
- 完整 RBAC(细颗粒权限)
- LDAP / SAML / OIDC 接入(自研 SSO 即可)
- mobile responsive(桌面端足够)

**绝不做**:
- 重型 SSO(Keycloak / Authentik 等)
- 第三方 IdP 依赖(全自主)

## 3. 架构数据流

### 3.1 admin 登录流程

```
[Admin Browser]
   ↓ https://admin.engi-stack.com
[nginx-proxy](TLS termination)
   ↓
[attune-admin FastAPI](独立容器)
   ↓ Login: username + password + TOTP
[admin_db (PostgreSQL,独立)](验证 + audit log)
   ↓ JWT signed
[Admin Browser](store JWT in httpOnly cookie)
   ↓ subsequent requests carry JWT
[attune-admin](verify JWT + RBAC)
   ↓ proxy to backend service
[WP REST API / accounts admin / pluginhub admin / new-api admin / gatus]
```

### 3.2 非 admin user 访问(不变)

```
[普通 User Browser]
   ↓ https://www.engi-stack.com / wiki / accounts(user login)
[nginx-proxy]
   ↓
[官网 / wiki / accounts user login(无影响)]
```

### 3.3 admin → backend service 调用

```
[attune-admin] (内部容器,docker network)
   ↓ internal-only token(per service)
[wordpress-admin-api]  — WP JWT plugin
[accounts /admin/api]  — internal admin token(per A3 已有)
[pluginhub /admin/api] — internal admin token
[new-api /api/admin]   — root token(install-wizard 自动生成,per #182)
[gatus configuration]  — file watch(no admin API,改文件 + reload)
```

### 3.4 IP whitelist(关键)

- WP `/wp-admin` nginx 配 `allow <attune-admin-internal-ip>; deny all;`
- accounts `/admin` 同
- pluginhub `/admin` 同
- new-api 不公开 admin endpoint(only internal docker network)

**普通用户访问 `/wp-admin` 返 403**(除非 admin)。

## 4. 模块边界

| 仓 / 目录 | 角色 | 新建? |
|----------|------|------|
| **cloud/admin/**(新 submodule `qiurui144/attune-admin`)| SSO + UI + proxy | ✅ NEW |
| cloud/docker-compose.yml | add admin service | EDIT |
| cloud/proxy/nginx-config/ | admin.* subdomain + IP whitelist 其他 admin path | EDIT |
| cloud/accounts/admin/ | restrict to internal token | EDIT(per A3 已部分有) |
| cloud/pluginhub/admin/ | restrict to internal token | EDIT |
| cloud/llm-gateway/ | new-api 已 native | NO CHANGE |
| cloud/official-web/(WP)| 加 JWT plugin + IP whitelist `/wp-admin` | CONFIG |
| cloud/secrets/cloud.enc.yaml | add admin master credentials | EDIT |

## 5. API 契约

### 5.1 attune-admin endpoints

```
POST /api/v1/auth/login
  body: { username, password, totp }
  return: { jwt, refresh_token, expires_at }

POST /api/v1/auth/refresh
  body: { refresh_token }
  return: { jwt, expires_at }

POST /api/v1/auth/logout
  invalidate refresh_token

GET  /api/v1/admin/users
  list users (paginated)

POST /api/v1/admin/users/<id>/upgrade
  trigger user tier upgrade

POST /api/v1/admin/plugins/<id>/approve
  approve plugin submission

POST /api/v1/admin/plugins/<id>/reject
  reject + reason

POST /api/v1/admin/wp/posts
  proxy to WP REST POST /wp-json/wp/v2/posts

GET  /api/v1/admin/audit
  audit log(paginated)
```

### 5.2 第一个 super-admin 创建

- install-wizard 时(per #182 已规划 admin_email + 临时密码 + TOTP secret)
- 凭证报告含 admin master credentials
- user 必须立即改密码 + 绑定 TOTP

## 6. 扩展点

- 加新管理 panel:`cloud/admin/src/panels/<name>.tsx` + backend route
- 加新 backend service proxy:cloud/admin/src/routes/<service>.py
- 角色扩(per RBAC 推 v1.0.7):`admin_db.roles` 表 + middleware decorator

## 7. 错误处理 + exit codes

| 场景 | HTTP | 行为 |
|------|------|------|
| 密码错 | 401 | rate limit count + 1(累计 10 lockout 30min)|
| TOTP 错 | 401 | 同上 |
| JWT 过期 | 401 | refresh path |
| 权限不足 | 403 | audit log + redirect |
| 后端服务挂 | 503 | retry + status banner |
| audit log 写失败 | **block action**(append-only critical) | user 重试 |

## 8. 成本契约

- 部署:1 docker container(FastAPI ~50MB)+ 1 PostgreSQL DB(共享 cloud PG 或独立)
- LLM 依赖:0(纯运维,不调 LLM)
- 维护:小 — 一个 FastAPI 服务

## 9. 测试矩阵(per § 测试方案规范 8 场景)

| 场景 | 范围 |
|------|------|
| happy | admin login → list users → audit log |
| edge | TOTP 6 位 / 8 位 / backup code |
| error | rate limit 触发 / lockout / refresh expired |
| adversarial | brute force / JWT forge / TOTP replay / SQL inject / XSS |
| 多并发 | 同时 5 admin online,audit log race |
| 资源耗尽 | audit log 写满 / refresh token DB 大 |
| 国际化 | admin UI 中英双语 |
| 降级 | backend service 挂时 admin UI 仍可访问 audit log + 其他 panel |

## 10. 向后兼容

- v1.0 GA 时无 admin 项目 — 各服务自己 admin
- v1.0.6 上 admin 后:
  - WP `/wp-admin` IP whitelist 后,**所有现 admin 必须通过 attune-admin**
  - install-wizard 必须新增 admin_username + admin_password(临时 + 必改)字段
  - migration script:从现 admin user 迁出(WP / accounts / pluginhub super-admin → attune-admin)

**v1.0.6 user action**(部署):
1. cloud.sh upgrade → 新 admin container 起
2. 凭证报告含 admin_master 凭证
3. user 登录 admin → 改密码 + 绑 TOTP
4. WP / accounts / pluginhub IP whitelist 启用

## 11. 风险登记

| R | 描述 | 缓解 |
|---|------|------|
| R1 | 第一个 admin 账号管理:install-wizard 出错 → 无法登录 | 应急 path:cloud.sh `--reset-admin` 子命令 重置 admin 密码 |
| R2 | TOTP secret 丢 → 锁死 | backup code 10 个生成时显示一次(凭证报告含)|
| R3 | JWT 私钥泄露 → 全 admin 账号可伪造 | 私钥进 sops(per L2 自动生成)+ rotation 6 月 |
| R4 | audit log 写失败但 action 继续 | append-only critical:写失败 → action block,**绝不静默继续** |
| R5 | admin.engi-stack.com 被 DDoS | rate limit + Cloudflare(若选)+ IP whitelist option |
| R6 | WP `/wp-admin` IP whitelist 后 admin 出差 IP 变 → 锁死 | attune-admin 永远可访问(无 IP whitelist),所有 admin action 走 attune-admin |
| R7 | new-api root token 泄露 | new-api 仅内部 docker network,user 无法访问;rotation 6 月 |
| R8 | admin → backend service 调用断 → admin UI 卡 | 每 service status 显示 + offline mode(仅 audit log + 本地 admin user)|

---

## 实施 plan(推 v1.0.6,6/25 - 7/02 1 周 sprint)

详 plan 待 spec 评审通过后单独写:`docs/superpowers/plans/2026-06-25-attune-admin-impl.md`

预估 commit 拆解(15 commit / 7 day):

| Day | Commit | 范围 |
|-----|--------|------|
| D1 | C1 cloud/admin/ 仓 init + Dockerfile + docker-compose | infra |
| D1 | C2 FastAPI skeleton + JWT auth + TOTP | backend |
| D2 | C3 PostgreSQL schema(admins / sessions / audit_log) + alembic | DB |
| D2 | C4 React UI scaffold + login page + 2FA setup wizard | UI |
| D3 | C5 user management panel(list / search / upgrade / suspend) | feature |
| D3 | C6 plugin marketplace 审核 panel(approve / reject) | feature |
| D4 | C7 WP content panel(post create / edit / publish via REST API)| feature |
| D4 | C8 wiki content workflow(trigger sync-content + docusaurus rebuild) | feature |
| D5 | C9 LLM gateway panel(channel list / add / quota query)| feature |
| D5 | C10 monitor / health / audit log panel | feature |
| D6 | C11 IP whitelist 配置(nginx-proxy + WP + accounts + pluginhub) | security |
| D6 | C12 cloud.sh `--reset-admin` 应急 path | ops |
| D7 | C13 docs/ADMIN-RUNBOOK.md user guide | docs |
| D7 | C14 integration test(login + 5 panel + audit) | test |
| D7 | C15 cloud README + RELEASE.md v1.0.6 note | docs |

## user 决策点

1. **后端栈** FastAPI(推荐,与 pluginhub 一致)vs Django(与 accounts 一致)?
2. **前端栈** React + Vite(与 attune-server UI 一致 design tokens)vs HTMX server-render(更简)?
3. **DB** 独立 PostgreSQL DB(隔离)vs 共享 cloud PG(节省)?
4. **IP whitelist 全启用** vs **选择性启用**(某些 admin endpoint 保留 公网 access)?
5. **TOTP 强制**(无 disable option)vs **可选**(user 设置)?
6. **审计 log retention** 30 天 / 90 天 / 365 天?
7. **角色简化** super-admin / admin / read-only 三角色 vs 更细 RBAC(推 v1.0.7)?
