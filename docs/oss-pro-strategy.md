# OSS × Pro 战略框架 — Attune

> 状态：**v2，2026-04-27**（同日边界审计后修订）。活文档 — 每季度或重大决策变化时复审。
>
> **v2 vs v1 变化**：决策性产品定位 — *attune (OSS) = 通用个人知识库；
> attune-pro = 个人用户的行业增强；lawcontrol = 律所小团队 B2B 方案。* v0.6.0-rc.2
> 删除 OSS 4 个 builtin 行业插件（law / presales / patent / tech），全部下放到
> `attune-pro` plugin packs。详见 §3 决策 2 理由。
>
> 受众：Attune Contributors（决策者）、Pro 插件开发者、评估商业接入的合作方。
>
> 配套文档：`docs/v0.6-release-readiness.md`（发版决策）·
> `docs/superpowers/specs/2026-04-25-industry-attune-design.md`（行业纵向设计）·
> `attune-pro/docs/license-key-design.md`（license 后端）·
> `attune-pro/docs/versioning.md`（跨仓版本策略）。

---

## 1. 为什么需要这份文档

Attune 是两个一起发版的仓库：

- **`attune`**（本仓，Apache-2.0）— 通用 RAG 引擎、加密 vault、插件框架、Chrome 扩展、桌面应用。
- **`attune-pro`**（私有，Proprietary）— 行业纵向插件（law-pro、presales-pro 等）+ 商业服务（cloud-sync、plugin-registry、llm-proxy）。

分离的工程基建已经就位（Cargo git-tag 依赖、Ed25519 插件签名、`.attunepkg` 包格式、5 档 license key）。在这份文档之前**缺**的是清晰的政策回答 *"什么进哪里、为什么、什么时候"* — 这样贡献者不会误把商业代码 backport 到开源仓，付费用户也能看到一致的价值阶梯。

这份文档就是这个政策。

---

## 2. 现状基线

### 2.1 仓库分割

| 仓库 | License | 可见性 | 用途 |
|------|---------|--------|------|
| `attune` | Apache-2.0 | 公开 | 核心引擎 + 4 内置 "basic" 插件 (tech / law / presales / patent) + 桌面 + Chrome 扩展 |
| `attune-pro` | Proprietary | 私有 | 行业纵向 pro 插件 (law-pro、presales-pro，更多计划中) + 商业服务 |

### 2.2 跨仓绑定

`attune-pro` workspace 锁定 `attune-core = { git = "...", tag = "v0.X.Y" }`。每次公开仓发版后，Pro 仓发兼容性 PR（按 `attune-pro/docs/versioning.md`）。**永远不要把商业代码 backport 到公开仓。** 如果某个 Pro 功能未来要开源，在 `attune` 重写干净版本。

### 2.3 v0.6.0 哪些是开源

W3+W4 全部交付都是开源：混合 RAG、J1 路径前缀 chunker、J3 显式 min_score、J5 强约束 prompt + 置信度 + 二次检索、B1 引用 breadcrumb、F2 sidecar 透传、C1 web 搜索缓存、G1 浏览捕获 + G5 隐私面板、G2 自动 bookmark staging、A1 memory consolidation、H1 资源治理、K2 parse golden set、MCP outlet shim、RAGAS benchmark harness、plugin marketplace toggle、profile topic distribution。**没有任何基础功能被付费墙挡住。**

---

## 3. 三个核心决策

### 决策 1 — Feature gate 哲学：**Thick OSS-core**

| 模式 | 代表 | 不选的原因 |
|------|------|-----------|
| Open-core "thin" | GitLab CE/EE、Sentry self-hosted | 故意残缺 OSS 驱动付费与 Attune "private-first" 形象冲突；社区敌意风险高 |
| Open-source + Cloud SaaS | Plausible、Cal.com、Supabase | `CLAUDE.md` 已否决跑 SaaS 镜像（"不做 SaaS 镜像"）— 集中精力在开源 + 插件生态 |
| **Thick OSS-core** ✅ | Bitwarden、Standard Notes、Plex | OSS 是个人通用 fully-featured；Pro 货币化路径是行业纵向（律师/售前/医疗）+ 企业服务（同步、registry、LLM 网关、硬件） |

**操作准则：**

> 单个个人用户希望从私人知识伙伴里得到的所有功能都保持开源。Pro 通过 (a) 深度行业专属工具 + (b) 只对团队有意义或需要运营成本的服务来增加价值。

这是承重原则。每个未来的功能决策都过这一条。

### 决策 2 — OSS attune 不含**任何**行业插件（v2 更新，2026-04-27）

> **从 v1 修订**：早期方案保留 4 个内置插件 (`tech` / `law` / `presales` / `patent`)
> 在 OSS 作为 "basic 升级路径"。审计 + 产品定位澄清后，v2 把**全部**行业插件迁到
> `attune-pro`。OSS 改为纯通用知识库 — 零行业绑定。

**v2 规则（决策性）**：OSS attune 是**纯通用知识库**。行业 taxonomy
（律师 / 专利 / 售前 / 技术 / 医疗 / 学术）**只**在 `attune-pro`。

| 行业 | OSS 范围 | Pro plugin pack | Pro 深度 capabilities |
|------|---------|-----------------|---------------------|
| 法律 | _OSS 无_ | `law-pro` ✅ active | 5 capabilities：合同审查 · 风险矩阵 · 起草 · OA 回复 · 条款查询；CaseNo extractor |
| 销售 / 售前 | _OSS 无_ | `presales-pro` ✅ active | 4 capabilities：竞品分析 · BANT 评估 · 报价 · 演示话术 |
| 专利 | _OSS 无_ | `patent-pro` (M3+) | 专利数据库直连 · 侵权检测 · 申请书草稿 |
| 软件 / 技术 | _OSS 无_ | `tech-pro` (M3+) | 代码库扫描 · GitHub PR 自动审查 · IDE 集成 |
| 医疗 | _OSS 无_ | `medical-pro` (计划中) | 医学术语 · 病历模板 · 文献追踪 |
| 学术 | _OSS 无_ | `academic-pro` (计划中) | 引用网络 · 论文写作助手 · 阅读清单管理 |

**为什么 OSS 不含任何行业插件？** 三个原因：
1. **战略定位**（per `2026-04-27 决策性定位`）：OSS attune = 通用个人知识伙伴。
   行业纵向是**变现层** — 全部在 Pro
2. **没有"留作 demo"的灰色地带**：即使 OSS 留 `tech` 作 demo，也会让 OSS 倾斜成
   "IT 工程师的工具" — 那仍是行业。真通用 = 零行业
3. **干净的升级路径**：用户先装 OSS attune → 用通用 vault / RAG / 浏览捕获 →
   发现某个行业痛点 → 装对应 Pro plugin pack

**v0.6.0-rc.2 删除清单**：
- `assets/plugins/{tech,law,presales,patent}.yaml` — 4 个 builtin yaml 删除
- `entities.rs::EntityKind::CaseNo` + `extract_case_no` — 移到 `attune-pro/plugins/law-pro/extractors/case_no.rs`
- `project_recommender.rs::CHAT_TRIGGER_KEYWORDS` const — 改为 plugin 聚合列表（OSS 默认空 → 永不触发；装 Pro 后由对应 plugin.yaml 提供）

### 决策 2.5 — 三产品矩阵（v2 新增）

> attune (OSS) × attune-pro × lawcontrol = **三角矩阵**

| 产品 | License | 形态 | 用户群 | 内容 |
|------|---------|------|--------|------|
| **attune (OSS)** | Apache-2.0 | Tauri 桌面 / Chrome 扩展，单机 vault | **个人通用用户** | 纯通用知识库 — RAG / 加密 / 浏览捕获 / 自动 bookmark / MCP / benchmark — **零行业绑定** |
| **attune-pro** | Proprietary | Plugin pack (.attunepkg signed)，由 attune 装载 | **个人行业用户**（律师 / 医生 / 学者 / 售前 / 工程师 / 专利代理）| 6 vertical packs：law-pro / presales-pro / medical-pro / academic-pro / patent-pro / tech-pro |
| **lawcontrol** | Proprietary | Django + Vue + 19 容器 B2B SaaS | **律所 / 小团队**（RBAC / 多租户 / 多渠道）| 行业小规模团队方案 |

**等式**：
- 个人通用用户 = `attune (OSS)`
- 个人行业用户 = `attune (OSS)` + `attune-pro/<vertical>-pro`
- 行业小规模团队 = `lawcontrol`

**准入规则**（决策性 — 每个新功能都过这一遍）：
- 一个功能进 **OSS attune** 当且仅当它对**任何**领域的个人通用用户都有价值（笔记 / 文档 / 浏览 / 跨设备 / 加密 / 检索）
- 一个功能进 **attune-pro** 当且仅当它对**某个特定行业**的个人用户有价值（律师合同审查 / 医生病历 / 工程师代码扫描 / 售前 BANT）
- 一个功能进 **lawcontrol** 当且仅当它**只在律所 B2B 团队场景**下有价值（多租户 / RBAC / 案件分配 / 多人协作）

三者技术上独立运行（无跨产品运行时依赖）。共用的"行业知识"层（law prompts /
case schema）M3+ 商业化时可能放 git submodule (`legal-prompts-pack`) — 与任何
单一产品的仓分离。

### 决策 3 — 货币化：5 档订阅 + 硬件

对齐 `attune-pro/docs/license-key-design.md`（5 plan 已在 license key payload 设计中：`lite` / `pro` / `pro_plus` / `team` / `enterprise`）。

| 档位 | 价格 | 包含 | 目标用户 |
|------|------|------|---------|
| **Lite** | ¥0 (OSS) | `attune` 全部 (RAG / vault / 浏览捕获 / MCP / benchmark — 仅通用能力，**零行业插件** per Decision 2 v2)、浏览器扩展 | 个人用户、开发者、评估期 |
| **Pro** | ¥99 / 年 | Lite + **一个**纵向插件包 (如 law-pro)，单设备 | 单独执业律师、个人售前 |
| **Pro+** | ¥299 / 年 | Lite + **全部**纵向插件包 + cloud-sync，3 设备 | 跨学科自由职业者、深度用户 |
| **Team** | ¥999 / 月起，按席位 | Pro+ + plugin-registry (内部插件) + audit log + 团队协作 | 中小律所、售前团队 (5–50 人) |
| **Enterprise** | 定制 (年签) | Team + SSO + on-prem 部署 + SLA + 行业咨询 | 大律所、医院、高校 (50+ 人) |
| **K3 一体机** | ¥6,999 起 (硬件 + Pro+ 一年) | 设备 + 捆绑本地 LLM + 上门安装 + 远程支持 | 不愿装软件的传统行业用户（小诊所、传统律所）|

**定价锚点说明：**
- 律师 ¥99/年 Pro = 每周省 ~1 小时合同审查 ⇒ 5 倍 ROI（律师时薪 ¥500-2000）
- ¥6,999 K3 = 一台办公电脑同价 ⇒ 新建律所装备级支出能接受
- ¥999/月 Team 5 席起步 = ¥200/席/月 ⇒ 落在 SMB 专业 SaaS 工具正常范围
- Lite 永久免费 — 没有定时炸弹试用，没有 nag screen。Lite 用户是漏斗 + 长尾社区

---

## 4. Feature gate 边界（唯一真源）

不确定新功能归哪边时，本表是答案。决策变化时更新；所有人引用这个。

### 4.1 OSS 范围 (`attune` 仓，Apache-2.0)

| 维度 | 功能 | OSS? |
|------|------|------|
| 存储 | DEK + AES-256-GCM vault、Argon2id KDF、sidecar 表模式 | ✅ |
| 索引 | 混合 BM25 + 向量 + RRF、J1 路径前缀 chunker、J3 显式 min_score、K2 parse golden | ✅ |
| 生成 | RAG chat、J5 强约束 prompt + 置信度 + 二次检索 | ✅ |
| 记忆 | A1 episodic memory consolidation | ✅ |
| 资源 | H1 governor 三档 + 顶栏 pause + 任务级限流 | ✅ |
| 引用 | B1 citation deep-link、F2 breadcrumb sidecar 加密落盘 | ✅ |
| 浏览 | G1 通用浏览捕获 + 默认 opt-out + HARD_BLACKLIST + G5 隐私面板 + G2 自动 bookmark staging | ✅ |
| Web | C1 web 搜索缓存 + DELETE/GET routes (W4-002) | ✅ |
| 插件框架 | plugin.yaml schema、维度 schema、plugin loader、EntityExtractor trait、marketplace toggle (W4 E1) | ✅ |
| 画像 | Topic distribution API (W4 F1)、import/export | ✅ |
| Builtin 行业插件 | **无** (v0.6.0-rc.2 起 — 全部迁到 attune-pro per Decision 2 v2) | ❌ |
| 通用 Entity extractors | Person / Money / Date / Organization (无行业专属) | ✅ |
| Persona 系统 | 4 个通用角色（Developer / Researcher / Writer / Custom）+ 插件可扩展角色注册（`provides_role` schema）— **无行业角色** | 🟡 v0.7+ |
| 分发 | Tauri 桌面 (Linux deb/AppImage、Windows MSI/NSIS)、Chrome 扩展 | ✅ |
| MCP 集成 | Python stdio shim (`tools/attune_mcp_shim.py`) 包装 REST | ✅ |
| 质量 | RAGAS-style benchmark harness + 双语方法学文档 | ✅ |
| 文档 | README / DEVELOP / RELEASE / TESTING / ACKNOWLEDGMENTS — 双语 EN + zh | ✅ |
| 全双语 | 所有公开文档配 `<NAME>.md` + `<NAME>.zh.md` | ✅ |

### 4.2 Pro 范围 (`attune-pro` 仓，Proprietary)

| 维度 | 功能 | 需要档位 |
|------|------|---------|
| 纵向插件 | `law-pro` (active)：builtin/dimensions.yaml + 5 capabilities (合同审查 / 风险矩阵 / 起草 / OA / 条款查询) + CaseNo extractor | Pro |
| 纵向插件 | `presales-pro` (active)：builtin/dimensions.yaml + 4 capabilities (竞品 / BANT / 报价 / 演示话术) | Pro |
| 纵向插件 | `patent-pro` (v0.6.0-rc.2 scaffold)：builtin/dimensions.yaml + capabilities (M3+) | Pro |
| 纵向插件 | `tech-pro` (v0.6.0-rc.2 scaffold)：builtin/dimensions.yaml + capabilities (M3+) | Pro |
| 纵向插件 | `medical-pro`、`academic-pro` (M3+ 计划) | Pro |
| 行业 Persona | Lawyer / Doctor / PatentAgent / Scholar / SalesEngineer — 由 vertical plugin pack 通过 `provides_role: <name>` 注入 + 自带行业域名白名单 yaml（如 Lawyer = 最高法 / 律协 / Westlaw / pkulaw / ...） | Pro |
| 多纵向 | 全部纵向包打包 | Pro+ |
| 同步服务 | `cloud-sync` — DEK 永不离机，仅同步加密 blob | Pro+ |
| 插件市场 | `plugin-registry` — 签名第三方插件分发 + 私有内部插件 | Team |
| LLM 网关 | `llm-proxy` — 托管网关 (Anthropic / OpenAI / Qwen) 含团队用量上限 + 审计 | Team |
| 合规 | Audit log (每次 vault 访问记录用户/时间/范围) | Team |
| 身份 | SSO (SAML / OIDC) | Enterprise |
| 部署 | On-prem 部署，私有安装包 + 隔离网支持 | Enterprise |
| 支持 | 行业咨询、定制 prompt 调优、专属 CSM | Enterprise |
| 硬件 | K3 一体机 OS image 捆绑 Qwen 1.5B + 上门安装 + 远程支持 | K3 SKU |

### 4.3 新功能归类决策规则（v2 — 三产品矩阵）

贡献者提新功能时按这顺序问：

1. **是否专属律所 B2B 团队场景**（多租户 / RBAC / 案件分配 / 多人协作）？→ **lawcontrol**（独立产品）
2. **是否专属某一个行业**（律师 / 医生 / 学者 / 售前 / 工程师 / 专利代理）？→ **attune-pro** 纵向 plugin pack
3. **是否需要中心化基础设施**（托管服务、计费、多租户协调、签名插件分发）？→ **attune-pro** services 层
4. **是否对任何领域的个人通用用户都有价值（与行业无关）？** → **OSS attune**（默认）

**示例**（决策性 — 这些过去引发过混淆）：

| 功能提议 | 判定 |
|---------|------|
| CaseNo extractor (中文法律案号正则) | ❌ OSS — v0.6.0-rc.2 移到 attune-pro/law-pro/extractors/ |
| Project recommender 关键词 "案件/诉讼" 硬编码 | ❌ OSS — v0.6.0-rc.2 改为 plugin 聚合列表 |
| 行业分类维度 (律师/专利/售前/技术 taxonomy) | ❌ OSS — 4 个 builtin yaml v0.6.0-rc.2 全删，迁 attune-pro/<vertical>-pro/builtin/ |
| 通用 Project / Timeline / Annotation CRUD | ✅ OSS — 每个个人用户都需要项目组织 |
| Workflow 引擎 + deterministic ops (find_overlap / write_annotation) | ✅ OSS — 通用引擎；具体行业 workflow 是 Pro plugin yaml 内容 |
| MCP outlet shim | ✅ OSS — 每个有 MCP 客户端的个人用户都受益 |
| RAGAS benchmark harness | ✅ OSS — 每个个人用户都受益于质量验证 |
| 多 vault 同步、audit log、SSO | ❌ OSS — Pro+ / Team / Enterprise (中心化基建) |
| 共享案件 / 多人协作 / RBAC | ❌ OSS、❌ Pro — 这些是 **lawcontrol** 范围 (B2B 团队场景) |

---

## 5. 6 个月路线

| 里程碑 | 周 | 目标 | OSS 侧 | Pro 侧 |
|--------|----|------|--------|--------|
| **M1** | 现在 → +2 | OSS v0.6.0 GA | rc.1 (今天) → soak 7 天 → GA | bump cargo dep tag = v0.6.0；law-pro 烟测新 attune-core |
| **M2** | +3 → +4 | law-pro 跑通新 attune | 维护为主 (W4 followups #1-#5) | 全部 5 个 law-pro capabilities 接 J5 confidence + breadcrumb sidecar；plugin-build pipeline 自动签名 `.attunepkg` |
| **M3** | +5 → +8 | 商业化 v1 上线 | 维护 + W5 K1 sleeptime / A2 conflict detection 起步 | License key 后端 (Ed25519 + 离线校验) ；订阅页 (Lite ¥0 / Pro ¥99 / Pro+ ¥299) 上线；10–30 律师种子用户 |
| **M4** | +9 → +16 | K3 一体机 v1 | 维护 + W7-8 plugin SDK 双语 + CRDT 准备 | K3 OS image 捆绑 attune + Qwen 1.5B；售前流程 + 上门安装 SOP；首批 10 台硬件用户 |
| **M5** | +17 → +24 | cloud-sync + plugin registry | 维护 + W9-10 K3 items keys (per Standard Notes 004 spec) | 加密同步后端 (DEK 永不离机)；内部 plugin marketplace beta |

**耦合规则：** Pro 发版滞后 OSS 发版。永远不发依赖未发布 OSS API 的 Pro 功能。`attune-pro/docs/versioning.md` 的跨仓版本矩阵就是契约。

---

## 6. 风险与对策

| 风险 | 严重度 | 对策 |
|------|--------|------|
| OSS 太强 — 抢走 Pro 收入 | 中 | OSS 是通用个人版，Pro 是行业纵向 + 服务。一个律师可以装 OSS 自用 *也可以*订 law-pro 做合同审查。两者不冲突 |
| Pro 插件与新 OSS API 不兼容 | 中 | `versioning.md` 强制 Pro 锁 OSS tag；OSS API 变化先触发 Pro 兼容性 PR 再发版 |
| Apache-2.0 vs AGPL 派系争议 | 低 | 暂保持 Apache-2.0。若出现规模化 free-rider 商业 fork，再评估 dual-license (Apache-2.0 + Commercial) — 但不预设限制 |
| Pro 价值不够 — 用户不付费 | **高** | law-pro 必须证明 3 倍 ROI。W4 J6 公开 RAG 数字是武器：不只 "law-pro 比 law-basic 强"，而是 "law-pro vs 同语料 competitor baseline 的公开数字" |
| 中外双市场 | 中 | 双语文档已就位。中国优先纵向：律师 / 售前 (现有 RPA + 中文法律语料)。国际优先：academic-pro / medical-pro (英文语料更丰富) |
| K3 一体机售后成本失控 | 中 | M4 前定 SLA + 远程支持工具链。初期限 10 台/月，控制运营压力 |
| License key 盗用 / 共享 | 中 | License key 含设备指纹 (per `license-key-design.md`)；公开吊销列表；M5 上线 cloud-sync 用量异常检测 |
| 商业代码意外 backport 到开源仓 | 高 | M2 计划：CI 规则阻止 `attune` 与 `attune-pro` 之间出现 verbatim 复制（除测试外）。Reviewer 按规则审查 |
| OSS 贡献者 burnout (没明确变现回路) | 中 | 维护者补贴来自 Pro 收入；M3 起开 OSS 贡献 bounty 计划，资金来自 Pro 利润 |

---

## 7. License 演进策略

**现在 (v0.6 → v1.0)：** `attune` Apache-2.0，`attune-pro` Proprietary。简洁、清晰，匹配当前策略。

**未来变更 OSS license 的可能触发：**

| 触发 | 可能反应 |
|------|---------|
| 规模化 free-rider 商业 fork (如 Amazon-style "managed Attune") | Dual-license：社区 Apache-2.0 + 商业 SaaS Commercial |
| 需要强制贡献回流 (如大公司资助 fork) | 切 AGPL — 但仅对绿地新代码，不溯及社区已有贡献 |
| 转向更强网络效应特性 (cloud-sync、plugin registry 自然增长) | 保持 Apache-2.0；靠 Pro 服务护城河，而非 license 限制 |

**明确不会做：**
- 切 BUSL / SSPL / Elastic License 这类 "源码可见但非 OSS" license。这些毒害社区信任，Attune 整个定位都靠这个信任
- 追溯重新 license 社区已有贡献
- 在 Apache-2.0 之外加 "additional restrictions" 条款

---

## 8. Plugin SDK 契约（给第三方开发者）

第三方插件开发者需要知道的：

- 基于 `attune-core` 公开 API 的某个 tag 编译（从 v0.6.0 起步）
- 插件清单 = `plugin.yaml` + 可选 `prompt.md` + Rust crate (或纯 prompt)
- 分发：签名 `.attunepkg` artifact (Ed25519)。允许自分发；Pro `plugin-registry` 是可选分发渠道之一，不是唯一
- License：自己选。MIT/Apache/GPL 插件都可以。需要付费 license 的插件可以用 Attune license key 系统 (M5+) 或自己实现
- 收入分成 (仅 Pro `plugin-registry`，M5+)：作者 70%，Attune 30% (托管 + 签名 + 支付)。发布前调整
- Contributor License Agreement (CLA) *不要求* `attune` 的 OSS 贡献 — 仅商业插件分发到 `attune-pro` 时要求

---

## 9. 待定问题（按需重审）

| 问题 | 暂缓原因 | 重审时间 |
|------|---------|---------|
| 是否接受 VC 投资加速 K3 硬件？ | 早期 — 先 bootstrap M1-M3 学清楚单位经济 | M4 (首批 10 台 K3 销售后) |
| `cloud-sync` 是否独立 `attune-cloud` 仓？ | 当前规模仓库开销 > 收益 | 当 `attune-pro/services/` 超 5 个服务 |
| 是否发布 "Pro 等价" 社区插件作为社会公益？ | 影响收入；削弱 Pro 升级路径 | 仅当 Pro 达 ¥10M ARR 且有余力回馈时 |
| Lite 用户是否给*某种*同步 (如 1 设备免费、3 设备 Pro+)？ | 同步基建成本 > 当前规模 Lite 获客价值 | Lite MAU 达 100k 时重审 |
| 移动端 (iOS / Android) | 路线图未提 — Tauri 2.0 移动端不成熟 | 当 Tauri 移动端 stable + first-party 存储原语 |

---

## 10. 责任人

| 维度 | 责任人 | 节奏 |
|------|--------|------|
| OSS 发版节奏 | Attune Contributors 维护者 | 每发版 (semver) |
| Pro 插件发版 | Pro 插件作者 | 每插件独立 semver |
| License key 后端 | Pro 基础设施团队 | M3 后持续部署 |
| 价格变化 | Attune Contributors 核心团队 | 季度复审；提前 30 天公示 |
| 战略框架 (本文档) | Attune Contributors 核心团队 | 季度复审；重大修订标顶部 |

---

## 11. 决策日志

| 日期 | 决策 | 状态 |
|------|------|------|
| 2026-04-25 | 行业纵向第一刀切：律师 | Active (CLAUDE.md, industry-attune-design.md) |
| 2026-04-25 | LLM 不捆绑安装包；远端 token 默认；K3 可捆绑本地 LLM | Active (CLAUDE.md cost & trigger contract) |
| 2026-04-25 | 平台优先级：Windows P0 → Linux P1 → macOS 暂不做 | Active (CLAUDE.md) |
| 2026-04-27 | Chrome 扩展 = 通用浏览状态知识源 (不止 AI 对话) | Active (W3 batch B 已发) |
| 2026-04-27 | 资源治理基线：每个后台任务限流 (H1) | Active (W3 W1 已发) |
| 2026-04-27 | 双语文档强制要求所有公开材料 | Active |
| 2026-04-27 (v1) | OSS-Pro 分割 = Thick OSS-core；定价 ¥99 / ¥299 / ¥999/月 / 定制 + ¥6,999 K3 | **被 v2 取代**（定位审计发现 OSS 在行业方向上太厚）|
| **2026-04-27 (v2)** | **三产品矩阵：attune (OSS, 通用) × attune-pro (个人行业增强) × lawcontrol (B2B 小团队)。OSS 不含任何行业插件。** | **Active** |
| **2026-04-27 (v2)** | **v0.6.0-rc.2 边界瘦身：删 4 个 builtin yaml + CaseNo extractor + CHAT_TRIGGER_KEYWORDS const；全部迁 attune-pro plugin packs** | **Active** |
| **2026-04-27 (v2)** | **定价：保留 v1 数字，详细分级策略推迟到 M3 商业化（per "暂时没有任何用户，都可以转身" 用户授权）** | **Active** |

---

## 快速链接

- `attune` 仓：https://github.com/qiurui144/attune (Apache-2.0)
- `attune-pro` 仓：私有 (申请权限)
- 本文档英文：`docs/oss-pro-strategy.md`
- 发版决策：`docs/v0.6-release-readiness.md`
- 行业设计：`docs/superpowers/specs/2026-04-25-industry-attune-design.md`
- License key 设计：`attune-pro/docs/license-key-design.md`
- 跨仓版本策略：`attune-pro/docs/versioning.md`
