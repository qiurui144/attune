# ICP 备案决策矩阵 — engi-stack.com 域名 + cloud 上架路径

> **状态**：决策报告（user 决策）。**本文不实施任何备案 / 服务器迁移操作**。
>
> **触发**：5/26 上架前 Legal P0（per `docs/superpowers/specs/2026-05-25-software-engineering-gap-audit.md` §10.2）。
>
> **范围**：仅涵盖 `engi-stack.com` 域名 + cloud 6 个子域（accounts / gateway / hub / wiki / status / 主域）的中国大陆访问合规路径。OSS 桌面与 GitHub Releases 分发不受影响（不依赖 engi-stack.com DNS）。

## 目录

- [1. 现状](#1-现状)
- [2. 法定要求](#2-法定要求)
- [3. 决策矩阵](#3-决策矩阵)
- [4. 建议](#4-建议)
- [5. 5/26 上架影响](#5-526-上架影响)
- [6. 行动项](#6-行动项)

---

## 1. 现状

### 1.1 域名 WHOIS

- **域名**：`engi-stack.com`
- **顶级域**：`.ai`（Anguilla 安圭拉 TLD，加勒比海英属海外领地）
- **WHOIS 查询**：本地 `whois engi-stack.com` 返回为空（`.ai` registry 限制公开 WHOIS；用户已掌握 registrar 后台信息）
- **持有方**：engi-stack（per 内部记录）

> **重要观察**：`.ai` **不是 .cn 域名**。中国 ICP 备案系统**仅强制 .cn 域名 + 大陆服务器**组合；`.ai` 域名 + 大陆服务器组合在实务上仍需备案（工信部 2018 起加强非 .cn 域名备案监管，参见 [工信部 32 号令](https://www.miit.gov.cn/jgsj/xgj/gzdt/art/2020/art_2fd1cb1a52a14f7ea5f0d6d2d33fc6e8.html)），但执行口径有弹性。

### 1.2 当前 DNS 解析

```
$ dig engi-stack.com +short
198.18.1.194
```

> `198.18.0.0/15` 是 IANA 保留段（RFC 2544 网络性能测试用），**不是公网 IP**。这意味着 engi-stack.com **当前未在公网 DNS 配置真实 A 记录**，或本地解析被路由器 / Pi-hole 拦截。这是开发期占位状态，**5/26 上架前必须配置真实公网 A 记录**。

### 1.3 服务器位置（计划）

per `cloud/docs/PRODUCTION_DEPLOY.md`：
- 6 子域全部解析到**同一台服务器公网 IP**
- 服务器具体物理位置**未在仓库公开记录**（per CLAUDE.md secrets 规则不应硬编码）
- 候选选项（待 user 拍板）：
  - **A. 阿里云北京 / 腾讯云广州**（大陆机房）
  - **B. 阿里云香港 / Cloudflare（Hong Kong PoP）**（境外但低延迟）
  - **C. 阿里云硅谷 / AWS US-West / DigitalOcean US**（境外远距）

### 1.4 部署组件清单

| 子域 | 组件 | 是否含中国用户敏感数据 |
|------|------|---------------------|
| `engi-stack.com`（official-web）| WordPress 营销官网 | 否（纯营销） |
| `accounts.engi-stack.com` | Django 会员中心 | **是**（用户邮箱 / Argon2id 密码 / Stripe 订阅）|
| `gateway.engi-stack.com` | new-api LLM 网关 | 部分（脱敏 prompt 元数据 30 天） |
| `hub.engi-stack.com` | pluginhub 插件市场 | 否（公开 plugin 元数据）|
| `wiki.engi-stack.com` | wiki-web 文档站 | 否（纯文档）|
| `status.engi-stack.com` | gatus 监控 | 否（仅服务可用性数据）|

---

## 2. 法定要求

### 2.1 中国法规依据

| 法规 | 关键要求 |
|------|---------|
| [《互联网信息服务管理办法》(2000 第 292 号令)](https://www.gov.cn/gongbao/content/2011/content_1860864.htm) §4 | 提供互联网信息服务的，需取得增值电信业务经营许可证或办理备案 |
| [《非经营性互联网信息服务备案管理办法》(信部 33 号令)](https://www.miit.gov.cn/zwgk/zcwj/wjfb/tz/art/2020/art_b27cc6fcd31a47fea4dabaad1c8bb98c.html) | 非经营性 = ICP 备案；经营性 = 经营许可证 + 备案 |
| [《工业和信息化部关于规范使用域名提供互联网信息服务的通知》(2017 工信微函 [2017]105 号)](https://www.miit.gov.cn/) | **非 .cn 域名同样需备案**，未备案不得在大陆境内提供互联网信息服务 |
| **网信办《关于做好互联网信息服务转移备案工作的通知》** | 服务器位置变更需更新备案 |

### 2.2 必备情形（**必须**备案）

| 服务器位置 | 域名 | 备案要求 |
|-----------|------|---------|
| 大陆机房 | 任意（含 .ai / .com）| **必须** ICP 备案 |
| 大陆机房 | .cn / .中国 | **必须** ICP 备案 |
| 境外机房 | 任意 | 法律层面**不强制**，但实务影响见 §2.3 |

### 2.3 不备案的实务影响（境外服务器）

- **大陆访问延迟**：从大陆访问境外服务器经海底光缆，延迟 100-300ms（vs 大陆机房 10-50ms）；高峰期可能跳变 500ms+
- **GFW / 防火墙拦截**：境外 IP 不定期被随机阻断；HTTPS SNI 字段可能触发关键词拦截
- **ICP 备案号合规**：大陆 SaaS 商业宣传通常预期在 footer 看到「京 ICP 证 XXX 号」；缺失影响信任度
- **支付通道**：境内支付通道（支付宝 / 微信支付）签约通常要求 ICP；如仅用 Stripe / PayPal 可绕过
- **App Store**：iOS / Android 国区上架要求 ICP 备案

### 2.4 ICP 备案流程时长

- **企业备案**（engi-stack 主体）：阿里云 / 腾讯云代办 **7-20 个工作日**
- 需要材料：营业执照 + 法定代表人身份证 + 域名证书 + 服务器接入资质 + 网站内容承诺
- 流程：阿里云后台提交 → 当地通管局审核 → 备案号下发 → 工信部公示

### 2.5 GDPR / 国际合规并行

不论选择大陆 OR 境外服务器，国际用户访问场景仍需：
- GDPR：DPA + SCC 2021（per `cloud/official-web/content/legal/dpa.md`）
- CCPA：「Do Not Sell My Info」按钮 (v1.1)
- DPF / 充分性决定：跨境 EU 出境到 US 子处理者（Stripe / OpenAI）已通过 EU-US Data Privacy Framework

---

## 3. 决策矩阵

### A. ICP 备案 + 大陆服务器

| 项 | 详情 |
|----|------|
| **时长** | 7-20 工作日审批；5/26 上架时**赶不上**，大陆访问延后到 6 月中下旬 |
| **成本** | 备案免费；服务器：阿里云 ECS 2c8g ~¥400/月 + 流量；代办费用 0-¥500（云服务商免费包含）|
| **资质** | engi-stack 工商登记 + 法定代表人身份证 + 域名证书（.ai 域名需提交所有者认证）|
| **优势** | 大陆访问低延迟、稳定；后续支付宝 / 微信支付 / 国区 App Store / 大陆 SEO 通畅；合规标签清晰 |
| **劣势** | 备案审核期内大陆访问中断；备案后内容受工信部 / 网信办监管，敏感关键词需自审；服务器变更需更新备案 |
| **5/26 上架可行性** | ❌ 上架时备案未下发，大陆暂无法访问 |

### B. 海外服务器 + 不备案（HK / 港 / 美）

| 项 | 详情 |
|----|------|
| **时长** | 即时（5/26 可上架）|
| **成本** | 阿里云香港 ECS ~$70/月；CloudFlare Pro $20/月 |
| **资质** | 无（域名 + 服务器境外，无需备案）|
| **优势** | 5/26 准时上架；国际用户访问快；无内容审查；快速迭代不受备案变更约束 |
| **劣势** | 大陆访问延迟高（150-250ms）+ 不稳定（峰值时可能丢包 / 中断）；无法对接境内支付通道；国区 App Store 受限；footer 无 ICP 备案号对大陆企业用户信任度低 |
| **5/26 上架可行性** | ✅ 立即上架，但**大陆访问体验差** |

### C. 混合架构（CDN 分流，推荐）

| 项 | 详情 |
|----|------|
| **架构** | 主服务器 HK / US（B 方案）；engi-stack.com 公网解析走 Cloudflare CDN；大陆访问通过 Cloudflare 中国合作节点 (JD Cloud)（部分付费层级支持） |
| **时长** | 即时（5/26 可上架）；后台可并行启动 ICP 备案 |
| **成本** | 主服务器 ~$70/月 + Cloudflare Enterprise ~$200/月（CN 节点访问）|
| **优势** | 5/26 立即上架；大陆访问通过 CDN 加速到 50-100ms；后续可平滑迁移到 ICP + 大陆机房 |
| **劣势** | Cloudflare CN 节点严格要求境内备案才能启用（迁回 A 路径循环）；不备案场景下 Cloudflare 走境外 PoP（落 B 方案）|
| **5/26 上架可行性** | ✅ 上架可行，长期路径清晰 |

### D. 海外 + 国区不主推（推荐路径变体）

| 项 | 详情 |
|----|------|
| **策略** | B 方案 + 主动声明「**v1.0 GA 仅面向国际市场 + 海外华人开发者**」；中国大陆用户使用桌面 OSS 版（不依赖 cloud）|
| **5/26 可上架范围** | 海外用户 + 中国大陆 OSS 桌面用户（GitHub Releases 直接下载，不经 engi-stack.com）|
| **后续路径** | v1.1 / v1.2 启动 ICP 备案，6-7 月份发布「Attune 中国大陆专版」|
| **优势** | 上架最简单；产品定位清晰（先国际后大陆）；不影响 OSS 桌面市场（GitHub 直接发布不经过 engi-stack.com）|
| **劣势** | 短期内丢失大陆云端 SaaS 用户群体；attune-enterprise 律所市场（中国主市场）延后 |

---

## 4. 建议

基于 attune 用户群（个人 OSS / B2C 桌面 + 律所 B2B + 海外华人开发者）以及 5/26 deadline 硬约束：

### 推荐 **方案 D**（海外 + 国区不主推 v1.0）作为 5/26 上架方案

**理由**：
1. **5/26 上架准时**：无需等待 ICP 备案（7-20 工作日）
2. **OSS 桌面市场零影响**：attune OSS 桌面通过 GitHub Releases 分发，不依赖 engi-stack.com；中国大陆开发者照常可下载（GitHub 在大陆可访问，虽偶有抖动但不依赖备案）
3. **海外华人开发者市场**：直接覆盖 SF / NY / London / Tokyo 华人开发者，避开备案审查门槛
4. **v1.0.1 / v1.0.2 sprint 期内启动 ICP**：5/26 上架后立即启动备案，6 月底 / 7 月初推出「中国大陆版」（accounts.cn.engi-stack.com / 大陆 ICP 备案 / 大陆机房）
5. **attune-enterprise 律所市场**：以「本地部署」(self-hosted) 形式优先推广，绕开 SaaS 备案约束；律所对自托管接受度高

**5/26 上架时执行清单**：
- engi-stack.com 主域 + 5 子域 DNS 指向 HK / US 服务器（推荐阿里云香港或 Cloudflare）
- 主页 footer 显示「Serving international and overseas Chinese-speaking developers. Mainland China edition coming v1.0.2 (target Jun 2026).」
- privacy policy 中跨境传输章节准备就绪（per `legal/privacy.md` §7）
- 5/26 同日启动 ICP 备案流程（并行）

### 退而求其次 **方案 C**（混合架构）

如果 user 强烈希望 5/26 上架日大陆访问也通畅：
- 5/22-25 紧急部署 Cloudflare CDN（境外 PoP 兜底，不依赖 ICP）
- 大陆访问延迟 100-200ms（vs 直连 200-300ms），可接受但非最优
- 同步启动 ICP，备案后切到 CN PoP，延迟降至 50-100ms

### 不推荐 **方案 A** 单独使用

如不结合 CDN，仅大陆服务器 + 等备案 → **5/26 完全无法上架**。除非接受推迟上架日 7-20 工作日（new deadline = 6/12-6/20）。

---

## 5. 5/26 上架影响

| 决策 | 5/26 上架可行 | 大陆访问 5/26 当日 | 备案完成日 |
|------|--------------|------------------|-----------|
| A 备案 + 大陆服务器 | ❌ | N/A（推迟） | 6/12-6/20 |
| B 海外不备案 | ✅ | ⚠️ 慢 / 不稳 | N/A |
| C 混合 CDN | ✅ | ✅（通过 CDN）| 后台并行 6/12-6/20 |
| **D 海外 + 国区不主推** ⭐ | ✅ | ⚠️ 主动声明先国际 | v1.0.2 启动 |

### 上架不阻断要素（无论选哪个）

- ✅ OSS 桌面（GitHub Releases）不依赖 engi-stack.com DNS，全球可下载
- ✅ Attune Pro 商业版（如已上架）不依赖 engi-stack.com 主域，可走 `gateway.<region>.engi-stack.com` 子域分布式部署
- ✅ wiki / docs 可通过 GitHub Pages 兜底（github.com/qiurui144/attune/wiki）

### 上架阻断要素（决策影响）

- ⚠️ official-web（engi-stack.com）大陆访问：受方案影响（A 慢 → ICP 后好；B 慢；C 中等；D 主动放弃大陆短期）
- ⚠️ accounts.engi-stack.com 大陆注册流程：同上
- ⚠️ Stripe 付款 → 大陆用户卡 + 备案问题；建议大陆用户 v1.0.x 使用 Apple Pay / Google Pay / 海外卡

---

## 6. 行动项

### user 决策点（5/25 当日）

- [ ] **决策**：选定方案（A / B / C / **D**）
- [ ] 若选 D：批准 footer 「先国际后大陆」声明文案
- [ ] 若选 C：批准 Cloudflare Enterprise 额外 $200/月预算
- [ ] 若选 A：接受推迟上架到 6/12-6/20

### 决策后 24h 内（5/25-5/26）

- [ ] 5/25 晚：engi-stack.com DNS A 记录指向选定服务器公网 IP
- [ ] 5/26 上架前：验证 `dig engi-stack.com +short` 返回真实公网 IP（不再是 198.18.x.x 占位）
- [ ] 5/26 上架前：6 个子域全部 DNS 配置完毕
- [ ] 5/26 上架前：legal/* 三个文档同步到 WordPress 落地为 `/legal/tos /privacy /dpa` 路由
- [ ] 5/26 上架前：footer 显示
  - 选 D：「Serving international users. CN mainland edition v1.0.2 (Jun 2026)」
  - 选 A/C 备案完成后：「engi-stack © 2026 — 京 ICP 备 XXX 号」
- [ ] 5/26 同日（若 D）：启动 ICP 备案

### 5/26 后跟进

- [ ] v1.0.1 sprint（5/27-6/2）：监控大陆访问真实数据（gatus + 用户反馈）
- [ ] v1.0.2 sprint（6 月中）：ICP 备案完成后切换 CDN 至 CN PoP / 大陆机房
- [ ] v1.0.x：若选 D，发布「Attune 中国大陆版」公告 + accounts.cn.engi-stack.com 子域上线

---

## 附录 A：参考资料

- [《非经营性互联网信息服务备案管理办法》](https://www.miit.gov.cn/zwgk/zcwj/wjfb/tz/art/2020/art_b27cc6fcd31a47fea4dabaad1c8bb98c.html)
- [阿里云 ICP 备案首页](https://beian.aliyun.com/)
- [腾讯云备案文档](https://cloud.tencent.com/document/product/243)
- [Cloudflare CN Network 文档](https://www.cloudflare.com/products/china-network/)
- 同期 gap audit：`docs/superpowers/specs/2026-05-25-software-engineering-gap-audit.md` §10.2

---

> **下一步**：等待 user 选定方案 → 启动相应执行清单（§6）。
