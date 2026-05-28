# DSAR — 数据主体权利请求

> Data Subject Access Request — 用户对自己数据的法定权利。
> GDPR Article 15/17/20 + 中国 PIPL §44-50 法律强约束。

## 你有哪些权利

| 权利 | GDPR 条款 | PIPL 条款 | 在 attune 怎么用 |
|------|----------|----------|----------------|
| **查看 / 复制权** | Art.15 / Art.20 | §44 / §45 | 导出 cloud 账户所有数据 (JSON 下载) |
| **删除权 (被遗忘权)** | Art.17 | §47 | 删除 cloud 账户 (30 天 grace 期可撤销) |
| **更正权** | Art.16 | §46 | Settings 页直接改 profile (即时生效) |
| **可携权** | Art.20 | §45 | 导出 = 标准 JSON, 可迁移到任何兼容系统 |

## 数据范围：本地 vs Cloud

attune 的数据天然分两块：

### 本地 Vault (BYOK / self-host 用户)

你的所有 vault 数据 (文档 / 批注 / chat / Project) **永远在你本地或你自管的 K3 一体机**。
导出/删除走 attune CLI:
```bash
# 导出整个 vault (encrypted .tar 文件)
attune-cli vault export ./my-vault-backup.tar

# 删除 vault (彻底, 不可逆)
attune-cli vault destroy --confirm
```

本地 vault 不归属 cloud accounts 管, 不需要 DSAR endpoint.

### Cloud Account (cloud member 用户)

只有 cloud member (Attune Pro Membership Gateway) 用户在 cloud 留有数据:
- profile (email, plan, 注册时间)
- licenses (购买的设备授权 license)
- billing events (Stripe 退订 / 支付历史)
- pluginhub 安装的插件记录
- LLM gateway 用量统计

这部分由 cloud accounts 服务管, 走 DSAR endpoint.

## 操作步骤

### 1. 导出我的 Cloud 数据

attune Desktop UI → Settings → Account → "导出我的数据"

或经 CLI:
```bash
curl -X POST http://localhost:18900/api/v1/dsar/export \
  -H "Content-Type: application/json" \
  -d '{"email":"you@example.com","password":"your-password"}' \
  > my-attune-export.json
```

返回 JSON 含:
- `profile` — 你的 cloud 账户信息
- `licenses` — 已购 license + 设备
- `billing_events` — 支付历史 (Stripe 内部 payload 不含)
- `cross_service.pluginhub` — 插件安装记录
- `cross_service.llm_gateway` — LLM token 用量
- `legal_notice` — GDPR/PIPL 条款引用

**注**: gateway_token (明文 LLM key) **不在导出**中 — 经 Settings 页 reveal 机制更安全; password hash 不导出 (对你无价值, 安全敏感字段).

### 2. 删除我的 Cloud 账户

attune Desktop UI → Settings → Account → "删除账户" (双重确认)

或经 CLI:
```bash
curl -X POST http://localhost:18900/api/v1/dsar/delete \
  -H "Content-Type: application/json" \
  -d '{"email":"you@example.com","password":"your-password"}'
```

**生效**:
- 立即: cloud account 标记 inactive, 你无法登录
- 30 天 grace 期: 你可调 cancel-deletion 撤销
- 31 天: 物理删除, 不可逆
  - Stripe customer 删除
  - pluginhub user 数据清空
  - LLM gateway user disabled
  - accounts DB user row 删除
  - billing_events.user_id 置 NULL (保留审计 trail 但匿名化)

**注意**: 删除 cloud 账户**不影响本地 vault** — vault 是你完全本地的数据.

### 3. 撤销删除 (30 天 grace 期内)

如果你误触发了删除:
```bash
curl -X POST http://localhost:18900/api/v1/dsar/cancel-deletion \
  -H "Content-Type: application/json" \
  -d '{"email":"you@example.com","password":"your-password"}'
```

**限制**:
- 必须在 grace 期内 (deletion 时间戳 + 30 天) 操作
- v1.0 实现: 撤销必须在**同一会话**内 (cloud login 会拒已删除用户)
- v1.1 改进: 邮件确认链接路径, 不依赖会话

## 隐私 / 安全说明

- **密码**: 经 DSAR endpoint 时密码仅本次 HTTP 请求使用, server 不持久化, 不写日志
- **下载**: 导出 JSON 经 Content-Disposition 头让浏览器下载到本地, server 不留副本
- **跨服务**: pluginhub / gateway 拉数据是 best-effort, 如对方服务挂, 不阻塞 accounts 端数据导出
- **审计 trail**: billing_events 在 hard delete 后保留 (user_id=NULL), 满足金融审计要求, 但失去你的 PII 关联

## 法律联系

如有 DSAR 相关投诉或问题:
- Email: privacy@engi-stack.com
- 数据控制者 (Data Controller): Attune (engi-stack.com)
- 监管投诉:
  - EU 用户: 你所在国的 Data Protection Authority
  - 中国大陆用户: 国家网信办 (CAC) 或地方网信部门
