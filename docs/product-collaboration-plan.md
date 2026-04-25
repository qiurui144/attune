# Attune × lawcontrol 产品协同规划（已废弃）

> **DEPRECATED 2026-04-25** — 本文档描述的「共用 PluginHub / SSO / 商业捆绑 / 数据流互通」方案**已废弃**。
>
> 新方向：attune 是**独立应用**，不调用 lawcontrol 任何 API、不复用其代码。可参考其 plugin / RPA / Intent Router 设计模式，但实现完全独立。详见 [`CLAUDE.md`](../CLAUDE.md) 的「独立应用边界」章节。
>
> 本文件保留仅作历史参考，不再指导新开发。

---

**版本**：v1 draft · 2026-04-18 （已废弃 2026-04-25）
**范围**：两个产品的定位分工、共用云管平台、商业协同、数据流边界
**产品**：
- **lawcontrol** — 律所级 B2B，多人协作、案件管理、43+ 法律 skill 插件
- **Attune** — 个人级 B2C，私有知识库、批注、AI 对话、本地优先

## 1 · 核心协同原则

> **定位互补，平台共用，数据边界清晰**

- **定位互补**：两者不竞争，各自做自己最擅长的，相互引导客户
- **平台共用**：云管平台（PluginHub / 官网 / License / SSO / 监控）**一套**服务两个产品，降低 50%+ 运维成本
- **数据边界清晰**：律所数据 ↔ 个人数据永不混淆；跨流向需用户主动授权

## 2 · 能力矩阵（谁做什么）

| 能力 | lawcontrol | Attune | 策略 |
|------|-----------|--------|------|
| 多人协作 / RBAC / 案件卷宗 / 审计 | ✅ 独占 | ❌ 不做 | lawcontrol 独占 |
| 43+ 法律 skill（合同审查/诉讼/起草）| ✅ 独占 | ❌ 不做 | lawcontrol 独占 |
| Intent Router 法律意图路由 | ✅ 成熟 | ⚠️ 可学轻量版 | 两边各一份，schema 统一 |
| Plugin YAML + prompt.md + JSON schema | ✅ 成熟 | ⚠️ 可学 | **Attune 学习**（详见 §5）|
| PDF viewer + 批注 overlay | ✅ 成熟 | ⚠️ 纯文本 | lawcontrol 保持领先，Attune 走文本路径 |
| 个人加密知识库（Argon2+AES-GCM）| ❌ 无 | ✅ 核心 | Attune 独占（律所合规场景也可学）|
| 本地离线运行（Ollama + NPU）| ❌ 需在线 | ✅ 核心 | Attune 独占 |
| 硬件感知默认模型 | ❌ 无 | ✅ 有 | Attune 独占（lawcontrol 自身场景不需要）|
| 上下文压缩 + Token Chip 成本透明 | ❌ 无 | ✅ 核心 | **lawcontrol 可学**（律所计费强相关）|
| 批注加权 RAG (⭐×1.5 / 🗑 Drop) | ❌ 有批注不反哺 | ✅ 有 | **lawcontrol 可学** |
| 浏览器扩展无感注入 | ❌ 无 | ✅ 核心 | Attune 独占（Westlaw/知网场景）|
| Docker 多服务 (doc/audio/rerank worker) | ✅ 有 | ❌ 单进程 | lawcontrol 独占（Attune 单机足够）|
| WebDAV / NAS 远程目录 | ❌ 无 | ✅ 有 | Attune 独占 |

## 3 · 共用云管平台（**重点**）

### 3.1 PluginHub（复用 lawcontrol 现有的）

**现状**：lawcontrol 有生产级 `pluginhub/`（FastAPI + PostgreSQL + License heartbeat + 健康监控 + 事件上报）。

**方案**：**Attune 不建平行 pluginhub**，直接接入同一个。

扩展 License / Plugin 表加 `product: 'attune' | 'lawcontrol'` 字段：

```sql
ALTER TABLE plugins ADD COLUMN product_line TEXT NOT NULL DEFAULT 'lawcontrol';
ALTER TABLE licenses ADD COLUMN product_line TEXT NOT NULL DEFAULT 'lawcontrol';
-- 索引 tenant × product 组合
```

客户端（Attune or lawcontrol）发请求时：
```
GET /api/v1/index.json?product=attune
Authorization: Bearer {license_key}
```
PluginHub 根据 license 绑定的 product_line 过滤插件清单。

**好处**：
- 单套运维：一个 PG、一个 License 系统、一个告警
- 统一发版：内部工具（CLI / admin UI）一套
- 跨产品插件：某些通用插件（`web_search`、`skill_dispatch`）两边共用

### 3.2 官网

一套域名下两个产品线：

```
https://attune.example.com          → Attune 产品页 + 价格 + 下载
https://law.example.com              → lawcontrol 产品页 + 价格 + 演示
https://hub.example.com              → PluginHub 管理后台（内部）
https://docs.example.com             → 文档站（多产品）
https://account.example.com          → SSO + 订阅管理（两者共用）
https://status.example.com           → 状态页 (uptime)
```

**技术栈**：Astro / Next.js 静态站 + Vercel/Cloudflare 部署，monorepo 结构。

两个产品页顶部 nav **互相指引**：

```
[Attune]                             [lawcontrol]
  首页                                 首页
  价格                                 价格
  文档                                 文档
  ─────                               ─────
  🏛 我是律所管理员？                   👤 个人律师订阅？
     了解 lawcontrol →                    了解 Attune Pro →
```

### 3.3 SSO / Account 体系

**方案**：一个账号打通两个产品。推荐 **Keycloak**（开源，OIDC 标准，自托管避免依赖第三方）。

```
User 注册 account.example.com
    ↓
Keycloak 生成 user_id + session
    ↓
访问 attune.example.com → OIDC 跳转 → 拿 attune 订阅
访问 law.example.com    → OIDC 跳转 → 拿 lawcontrol 席位（律所管理员配置）
```

**订阅关系**：
- 个人用户买 Attune Pro → Keycloak user 加 `attune_pro` role
- 律所管理员买 lawcontrol 20 席 → 给 20 个 Keycloak user 加 `lawcontrol_seat` role，同时**每人自动赠送 Attune Pro**（商业捆绑，见 §7）

### 3.4 共用基础设施

| 组件 | 用途 | 推荐 |
|------|------|------|
| PluginHub | 插件分发 + License | ✅ 已有（lawcontrol/pluginhub）|
| Keycloak / Auth0 | SSO | Keycloak 自托管 |
| Sentry | 错误监控 | 自托管 Sentry 或 GlitchTip |
| Stripe / 支付宝 | 订阅支付 | Stripe 国际 + 支付宝国内 |
| 状态页 (Uptime Kuma / Gatus)| 服务可用性公开 | Gatus（已在 lawcontrol 用）|
| 邮件发送 (Postmark / 腾讯企业邮)| 交易邮件 | 国内用腾讯邮，国际 Postmark |
| Docs（Docusaurus / Mintlify）| 文档站 | Mintlify 免费版 |
| CDN + 对象存储 | 安装包分发 + 云备份 | Cloudflare R2 / 阿里云 OSS |

**规则**：**新组件都要求两产品通用**，不做产品专属基础设施（除非强场景需要）。

## 4 · 数据流边界

```
┌─── Attune（个人）─────────────┐       ┌─── lawcontrol（律所）──────────┐
│                                │       │                                 │
│  Vault (Argon2+AES 加密本地 DB)│       │  PostgreSQL + Redis（律所服务器）│
│    • 个人笔记                  │       │    • 案件卷宗                   │
│    • 读书批注                  │       │    • 合同、起诉状、证据          │
│    • Chat 历史                 │       │    • 客户档案（带 PII）         │
│    • AI 分析结果               │       │    • 律所成员权限                │
│                                │       │                                 │
└──────────────┬─────────────────┘       └─────────────┬──────────────────┘
               │                                        │
               │        ┌────── 数据流规则 ──────┐       │
               │        │                        │       │
               └──→ a. 仅用户主动 export ─────────────────┘
                        b. 仅 lawcontrol "我的资料夹" → Attune 只读同步
                        c. 个人批注/Chat 永不上行到 lawcontrol（防私密观点成证据）
                        d. 律所数据下行到 Attune 时带 "只读" + 审计水印
```

### 4.1 可互通的数据（用户主动）

| Attune → lawcontrol | lawcontrol → Attune |
|---------------------|---------------------|
| 合同审查个人笔记 → 案件卷宗"律师参考" | 律师个人分到的案件片段 → Attune 个人只读资料夹 |
| 个人收藏的判例 → 律所判例库（可选公开度）| 律所公开的判例库 → Attune 搜索源扩展 |

### 4.2 **不**互通（硬边界）

- ❌ Attune 个人 Chat 对话 → 永不同步到 lawcontrol（防律师私下吐槽客户被当证据）
- ❌ Attune 个人批注（🤔 存疑 / ❓ 不懂）→ 不同步（私人学习痕迹）
- ❌ lawcontrol 客户 PII（身份证号 / 联系方式）→ 绝不进 Attune 个人库（合规 / PIPL）
- ❌ 律所合规审计日志 → 不进 Attune（不归个人所有）

### 4.3 技术边界落地

**插件 schema 统一**：两边的 AI 输出共用一套 JSON schema（如 contract_review output），迁移时格式已对齐：

```yaml
# 双方共享的 schema 包（未来独立 npm/pip 包）
@attune-lawcontrol/schemas:
  - contract_review.output.schema.json    # 两边都用
  - litigation_strategy.output.schema.json  # lawcontrol 为主，Attune 可选
  - legal_research_memo.output.schema.json  # lawcontrol 为主，Attune 个人笔记可用
```

## 5 · 技术互学（具体 action items）

### Attune ← lawcontrol

1. **plugin.yaml + prompt.md + JSON schema 分离**
   - 现状：`ai_annotator.rs` 硬编码 prompt
   - 目标：`plugins/ai_angle_risk/{plugin.yaml, prompt.md, output.schema.json}`
   - 工作量：2-3 天
2. **Intent Router 轻量版**
   - 现状：chat 单一 prompt
   - 目标：引入"合同/判例/法条/一般"4 意图路由（律师插件专用）
   - 工作量：3 天
3. **律师插件 MVP schema 对齐**
   - 直接采用 lawcontrol `contract_review.output.schema.json` 结构
   - 工作量：零代码（仅 YAML 定义）

### lawcontrol ← Attune

4. **字段级加密** (Python 实现)
   - 需求：律所数据库里客户 PII 当前明文存 PostgreSQL → 合规风险
   - 方案：参照 Attune `crypto.rs` 的 Argon2+AES-GCM pattern，做 Python 侧 library
   - 工作量：5 天
5. **上下文压缩 + Token Chip**
   - 需求：律所律师按小时计费，客户问"这次 AI 多少钱"无答案
   - 方案：移植 Attune `context_compress.rs` 逻辑到 Python，UI 加 chip
   - 工作量：3-4 天
6. **批注加权 RAG**
   - 需求：律师标"已废止条款"后检索仍返回此条款
   - 方案：移植 Attune `annotation_weight::compute_adjust` → Python
   - 工作量：2 天

## 6 · 插件生态策略

### 6.1 三层插件分类

| 层级 | 谁发布 | 价格 | 示例 |
|------|-------|------|------|
| **核心免费**（开源） | 官方 | 免费 | `web_search`、`ocr`、`embedding_provider` |
| **行业会员插件**（商业） | 官方 | 会员专享 | `contract_review`、`pre_sales_bid`、`medical_icd10` |
| **社区插件**（开源/自由）| 社区 | 自定 | 用户自写的垂类 prompt |

### 6.2 PluginHub 发布流程

```
开发者写插件 → 本地 attune-cli package → 签 ed25519 → 上传 PluginHub
    ↓
PluginHub 管理员审核（人工 + 自动 schema 校验）
    ↓
批准 → 发布到 index.json，客户端拉取时校验签名
```

### 6.3 律师插件**跨产品一致**

Attune 的 "lawyer_contract_review" 和 lawcontrol 的 "contract_review" 应**共享同一份 output schema**：

- 好处：用户从 Attune 个人审查 → 升级到 lawcontrol 律所版时，历史数据格式已对齐，迁移无痛
- 做法：两边都从 `@attune-lawcontrol/schemas` 包引入 schema

## 7 · 商业协同

### 7.1 定价锚点

| 产品 | 价格 | 备注 |
|------|------|------|
| Attune Lite | 免费 | 开源 + 免费插件 |
| Attune Pro | ¥29/月 或 ¥199/年 | 2 选 1 行业插件 |
| Attune Pro+ | ¥49/月 或 ¥399/年 | 全部行业插件 |
| lawcontrol 小所版 | ¥399/席/月 × 5 席起 | 含协作 + 案件 + 40 skill |
| lawcontrol 中大所 | 定制 | 10+ 席，私有化 |

### 7.2 捆绑策略

1. **律所包 Attune Pro**：律所采购 lawcontrol 后，每个律师免费获 Attune Pro 账号
   - 逻辑：lawcontrol 收了律所的钱，单个律师通过 Attune 个人增粘性
   - 成本：Attune Pro 月均 ¥29，律所每席位 ¥399，赠送成本 7%
2. **硬件 + 3 年会员**：Mini PC / NPU 工作站预装 Attune + lawcontrol 节点（律所可选购）
   - 面向中小律所：一台 Mini PC ¥5000 硬件 + ¥3000 lawcontrol 3 年 3 席 = 总价 ¥8000 买断
3. **学生 5 折**：法学院学生 Attune Pro 年付 ¥99（培养未来律师习惯）

### 7.3 销售话术协同

官网话术**相互指引**：

- Attune 产品页 footer：_「你的律所需要多人协作、案件管理、合规审计？→ 了解 lawcontrol」_
- lawcontrol 产品页 footer：_「律所律师个人需要本地知识库、读书批注、AI 对话？→ 每席赠送 Attune Pro」_
- 双向导流，不竞争客户

## 8 · 路线图

### Phase 1（1-2 周）：基础设施对齐

- [ ] 本文档定稿 + review
- [ ] PluginHub 加 `product_line` 字段迁移（lawcontrol 侧 1 天）
- [ ] Attune CLI 加 `plugin install` 命令（对接 PluginHub API，3 天）
- [ ] 共享 schema 包建 repo（`attune-lawcontrol-schemas`）
- [ ] 官网 monorepo scaffold（attune + lawcontrol + hub + docs）

### Phase 2（3-4 周）：SSO + 账号打通

- [ ] Keycloak 部署到 account.example.com
- [ ] Attune / lawcontrol 分别接 OIDC
- [ ] 订阅系统（Stripe + 支付宝）对接 Keycloak roles
- [ ] 捆绑赠送逻辑（律所买 lawcontrol → 自动发 Attune Pro）

### Phase 3（5-6 周）：互学代码落地

- [ ] Attune 引入 plugin.yaml + prompt.md + JSON schema（§5.1）
- [ ] Attune 律师插件 MVP（contract_review 个人版，schema 对齐 lawcontrol）
- [ ] lawcontrol 引入字段级加密 library（§5.4）
- [ ] lawcontrol UI 加 Token Chip（§5.5）

### Phase 4（持续）：数据流互通

- [ ] Attune → lawcontrol 导出 API（笔记 / 批注 → 律师参考）
- [ ] lawcontrol → Attune 只读同步（个人资料夹）
- [ ] SSO 账号切换 UX 完善

## 9 · 风险与边界

### 9.1 法律风险

- **PII 隔离**：律所客户数据（身份证 / 联系方式）绝不进 Attune，违反 PIPL
- **合规审计**：lawcontrol 审计日志仅在律所服务器，不进 PluginHub 或 Attune
- **免责声明**：两边 AI 输出都带 "仅供参考，非专业意见" 水印
- **跨境数据**：CN 用户数据默认境内（阿里云 / 腾讯云），海外用户可选 AWS/Cloudflare

### 9.2 商业风险

- **律所买断即终结**：防止律所只买一次 lawcontrol 不续费 → 订阅而非买断
- **Attune Pro 被律所"代购"滥用**：一个律所账号给非本所律师用 → 限制每律所账号领取次数 × 席位
- **PluginHub 被滥用**：社区插件恶意传播 → 审核 + 签名 + 沙箱执行

### 9.3 技术风险

- **PluginHub 单点**：挂了两个产品都无法下载新插件 → 做 CDN 镜像 + 客户端本地 cache
- **SSO 单点**：Keycloak 挂了两边都登不上 → HA 部署 + 降级离线模式
- **schema 分歧**：两边插件 schema 漂移 → monorepo + CI 校验

## 10 · 待拍板的问题

1. **品牌**：Attune 中文名是否需要？（律师客户可能偏好中文品牌）
2. **云管平台部署地**：境内（阿里云 / 腾讯云）还是境外（AWS / Cloudflare）？中美合规策略？
3. **PluginHub 扩表**：谁先动手？lawcontrol 主维护，Attune 对接？
4. **律师执业证**：Pro 注册时验证，还是 lawcontrol 端统一验证？（建议 lawcontrol 统一，Attune 信任 lawcontrol 认证结果）
5. **Attune 价格锚点**：¥29/月 vs ¥25/月 vs ¥49/月？需要用户调研
6. **开源程度**：Attune 核心全开 Apache-2.0 ✓；lawcontrol 未来是否也要开核心？

---

## 附录 A · PluginHub 扩展建议（最小改动）

lawcontrol pluginhub 当前 schema 是 product-agnostic。**只需加一个字段**即可支持 Attune：

```python
# pluginhub/models.py
class Plugin(Base):
    __tablename__ = "plugins"
    id = Column(Integer, primary_key=True)
    slug = Column(String, unique=True, nullable=False)
    # ... 现有字段 ...
    product_line = Column(String, nullable=False, default="lawcontrol")  # 新增
    # 允许值: "lawcontrol" | "attune" | "both"

class License(Base):
    __tablename__ = "licenses"
    id = Column(Integer, primary_key=True)
    key = Column(String, unique=True, nullable=False)
    # ... 现有字段 ...
    product_lines = Column(JSON, nullable=False, default=list)  # 新增
    # 如 ["attune_pro"] 或 ["lawcontrol_seat", "attune_pro"]（律所捆绑）
```

客户端请求时带 product query：
```
GET /api/v1/index.json?product=attune
Authorization: Bearer {license_key}
```

PluginHub 过滤逻辑：
```python
def list_plugins(product: str = "lawcontrol", license_key: str = ...):
    lic = get_license(license_key)
    if product not in lic.product_lines:
        return 403
    return plugins.filter(product_line__in=[product, "both"])
```

## 附录 B · 文档更新清单

本协同规划需要更新：
- `lawcontrol/CLAUDE.md` — 加入 Attune 协同章节
- `attune/CLAUDE.md` — 加入 lawcontrol 协同章节（本 repo 已有"两条产品线"表述，需扩充）
- 官网 /pricing 页 — 展示捆绑价
- `attune/README.md` §License — 指出商业插件走 PluginHub
- `lawcontrol/README.md` §Roadmap — 加入 "Attune Pro 员工福利" 说明
