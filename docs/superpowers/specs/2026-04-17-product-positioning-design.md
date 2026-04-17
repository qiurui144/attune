# npu-vault 产品定位重设计

**日期**：2026-04-17
**作者**：产品定位 brainstorming 流程产出
**状态**：待实施
**范围**：Rust 商用线 npu-vault（不涉及 Python 原型线）

---

## 0. 背景与动机

### 为什么要重设定位

过去的定位核心词是 "**1Password 式加密 + 本地优先知识库引擎**"。这在产品早期（只有加密存储 + 基础搜索）成立，但随着近几个里程碑上线（专利插件、网络搜索 fallback、SkillClaw 风格自动技能进化），老定位无法覆盖：

1. **产品不再是被动仓库** —— SkillEvolver 会从失败查询中自学，主动优化检索
2. **产品不再是纯工具** —— RAG Chat + 引用 + 会话持久化已经是主交互，跟"存取"心智不同
3. **产品不再纯本地** —— 网络搜索 fallback 已落地，"本地优先"不准确，应该是"本地决定、全网增强"
4. **1Password 类比误导** —— 它是凭据保险箱（被动/静态），npu-vault 是知识伙伴（主动/进化）

### 本次重设的目标

- 给出覆盖当前能力的一句话定位、三大支柱、目标用户、典型场景
- 明确"零锁定付费"原则作为底层承诺
- 产出 README / 仓库文档的具体改动清单
- 同步规划：把 `WebSearchProvider` 的默认实现从付费 API 改为浏览器自动化

---

## 1. 核心承诺

### 产品名
**npu-vault**（暂不改名，后续可再议）

### 一句话定位
> **私有 AI 知识伙伴** — 本地决定，全网增强，越用越懂你的专业。
>
> *Your private AI knowledge companion — locally owned, globally augmented, increasingly attuned to your expertise.*

### 产品承诺（约 60 字）
> npu-vault 是为知识密集型专业人士打造的私有 AI 知识伙伴。你的专业领域它会越用越懂；本地知识够用时在本地决定，不够用时主动上网补全；所有数据加密存在你自己的设备上，换设备、换工作都能带走。

### 情感钩子（按重要性）

1. **伙伴关系** — 不是每次打开都陌生的工具，而是记得你研究过什么、习惯怎么问问题、领域常识越积越厚的长期搭档
2. **主权** — 你的知识是你的资产，加密存在你设备上，不被平台锁定
3. **进化** — 不需要你配置，产品自己从每次交互中学习

---

## 2. 三大支柱

### 支柱 1：主动进化（Active Evolution）
> 「它从每次查询中学习，不需要你配置。」

- **能做什么**：本地知识库没命中的查询会自动沉淀为"失败信号"；后台 `SkillEvolver` 每 4 小时或累积 10 条信号时触发，用 LLM 分析主题并生成同义词扩展，静默写入配置
- **用户感知**：用三个月后搜同一个词，结果比第一天准得多；整个过程没有任何"训练"按钮或提示弹窗
- **底层支撑**：SkillClaw 启发的 Summarize → Aggregate → Execute 三阶段流水线；`learned_expansions` 静默生效

### 支柱 2：对话伙伴（Conversational Companion）
> 「跟它讨论，不是跟它搜索。」

- **能做什么**：RAG Chat 为主界面，每条回答都带可点击的引用源（本地文档或网络结果）；会话持久化并可搜索，三周前的讨论能接着往下问
- **用户感知**：像跟一个长期配合的同事讨论 —— 它记得你上次查过什么、用过什么术语、关注哪些领域
- **底层支撑**：三阶段检索（vector + BM25 → rerank → top-k）+ 动态注入预算 + 加密会话存储

### 支柱 3：混合智能（Hybrid Intelligence）
> 「本地决定，全网增强。」

- **能做什么**：优先在本地知识库检索；本地无结果时通过**后台浏览器自动化**进行网络搜索；Chat 回答明确标注"来自本地"或"来自网络"
- **用户感知**：你的专业积累和私事在本地、加密；公开信息现查现用；每个答案都看得到出处
- **底层支撑**：`BrowserSearchProvider` 唯一实现；后台驱动**用户系统已装的 Chrome / Edge**（Chromium 内核）；无任何 API 依赖，无任何隐藏付费；网络搜索失败时明确提示而非隐式降级到付费服务

---

## 3. 底座：主权与透明

不作为支柱（差异化不靠这个，是底线），但写在文档显要位置作为承诺。

### 数据主权

- Argon2id(64MB/3 轮) + AES-256-GCM 字段级加密 + Device Secret 多因子
- 所有数据本地持有，单二进制分发，零运行时依赖
- 换设备通过加密导出/导入无损迁移
- 云端 LLM 可选但非必须（本地 Ollama 全能力）

### 零锁定付费原则

用户只需要支付两样东西：

1. **软件本身** —— npu-vault 的授权费（明码标价，一次性或订阅）
2. **LLM token**（可选） —— 如果你用云端 LLM（OpenAI / Anthropic / 其他），token 是你和模型厂商的事，我们不抽成、不加价、不代收

除此之外，**无任何隐藏费用**：

- 网络搜索默认走浏览器自动化，**不需要 Brave / Tavily 这类搜索 API 密钥**
- Embedding 和 Chat 默认走本地 Ollama，**无云端依赖**
- 存储在你自己的设备上，**没有云端订阅费**
- 所有数据加密可导出，**不锁定**

---

## 4. 网络搜索架构：唯一方案，无降级

### 核心原则

**网络搜索只有一种实现方式：后台驱动用户系统已装的浏览器。**

不降级、不做 fallback 链、不保留 API 方案 —— 因为：

- **API 一定会有隐藏付费**（Brave / Tavily 超额收费），违反"零锁定付费"原则
- **SearXNG 自托管**增加用户部署负担，不符合"开箱即用"
- **多方案并存**会让 settings 变复杂、心智变模糊
- **单一实现**让维护负担最小、用户选择最简

### 需要删除的代码

当前 `web_search.rs` 中的以下 provider **全部删除**：

- `BraveSearchProvider` —— 删除
- `TavilySearchProvider` —— 删除
- `SearxngSearchProvider` —— 删除

`WebSearchProvider` trait 保留（作为未来扩展点）。

### 唯一实现：BrowserSearchProvider

```
BrowserSearchProvider
├─ 使用 chromiumoxide（async CDP 客户端，成熟库）
├─ 后台（headless）启动用户系统已装的 Chrome / Edge
│    自动检测 Chromium 内核浏览器路径：
│      Linux:   /usr/bin/google-chrome, /usr/bin/chromium, /usr/bin/microsoft-edge
│      macOS:   /Applications/Google Chrome.app/Contents/MacOS/Google Chrome,
│               /Applications/Microsoft Edge.app/...
│      Windows: Program Files\Google\Chrome\Application\chrome.exe,
│               Program Files (x86)\Microsoft\Edge\Application\msedge.exe
├─ 默认搜索引擎：DuckDuckGo HTML 端点（对爬虫友好）
├─ 可插拔 SearchEngineStrategy：每个引擎一个抓取模块
├─ 速率限制：最低间隔 2s
└─ 失败处理：
    - 浏览器未安装 → 启动时 log warning，运行时返回空结果 + 明确错误提示
    - 搜索执行失败 → 返回空结果 + 记录错误
    - 绝不调用付费 API 兜底
```

### 失败时的用户体验

当系统未安装 Chromium 内核浏览器或浏览器启动失败时：

1. 日志明确记录："web search unavailable: no Chromium-based browser found"
2. Chat 端返回 `web_search_used: false` + 追加一句说明："本地知识库无结果；网络搜索不可用，请安装 Chrome 或 Edge 后重试"
3. **不会**静默降级到任何付费 API，**不会**静默失败

### from_settings 默认值变化

**Before**
```json
"web_search": {
  "enabled": false,
  "provider": "brave",
  "api_key": "",
  "base_url": ""
}
```

**After**
```json
"web_search": {
  "enabled": true,
  "engine": "duckduckgo",
  "browser_path": null,
  "min_interval_ms": 2000
}
```

- 默认即开启（`enabled: true`），零配置可用
- 去掉 `provider` 字段（只有一种实现）
- `browser_path: null` 表示自动检测；用户可显式指定路径
- 完全删除 `api_key` / `base_url` 字段

---

## 5. 目标用户与典型场景

### 四类用户（按权重）

| 用户 | 日常痛点 | npu-vault 的价值 |
|------|---------|----------------|
| **律师 / 专利代理**（核心） | 每个案件涉及的法条、判例、客户技术交底散落在邮件、Word、网盘；换律所时所有沉淀一夜归零 | 加密本地积累 + 专利/法律插件 + 可携带迁移 |
| **研究员 / 学者** | 读过的论文记不清在哪看过、笔记跟阅读行为脱节、跨课题调研重复劳动 | 对话式检索 + 引用可追溯 + 跨课题知识联动 |
| **独立顾问 / 分析师** | 每个项目客户、行业、方法论都不同，但 60% 的底层知识可复用 | 行业插件 + 主动扩展学习 + 本地 + 网络融合 |
| **AI 重度用户 / 技术 Prosumer** | ChatGPT Memory 不可控、不可导出、不懂领域；想要"私有版 AI 记忆" | 本地加密 + 可插拔 LLM + 自托管 + 数据主权 |

**共通画像**：每天处理大量非结构化信息、知识沉淀是核心资产、对隐私有真实诉求（不是口头的）、愿意接受一点点技术门槛换取长期主权。

### 四个典型场景

**场景 1 — 专利代理做一次 FTO 检索**
上午客户发来技术交底书。代理把文件拖进 npu-vault → 自动分类（patent 插件识别为"技术交底书 / 申请阶段"）→ Chat 提问"这个技术方案在 USPTO 有类似先行技术吗" → 本地历史案卷先匹配 → 没有匹配时自动跳到 USPTO 实时检索 → 回答带完整引用链。**三大支柱全部在场。**

**场景 2 — 律师跨三年持续跟进一类案件**
每次办完一个案件把卷宗导入。半年后再办同类案时，直接 Chat 问"我之前处理过的类似条款争议案里，最后哪种抗辩策略成功率最高" → 本地多年积累 + 引用指向每个具体案卷。**对话伙伴 + 主动进化在场。**

**场景 3 — 研究员深入一篇论文**
下载 PDF 丢进来，问"这篇和我六个月前读的 X 论文在方法论上有什么差异" → 两篇都在本地 → RAG 给出对比 + 原文段落高亮。同时 SkillEvolver 在后台发现该研究员最近几周反复查询"扩散模型采样加速"相关词，自动把 "DPM-Solver / consistency models / rectified flow" 这类术语加入扩展词典。**主动进化 + 对话伙伴在场。**

**场景 4 — 顾问切换项目**
结项后把项目文档归档，不删。三个月后新客户行业不同，但某个"定价策略框架"可复用。提问时 npu-vault 跨项目检索 → 自动识别方法论迁移性 → 给出老项目的引用 + 新领域需补充的信息（自动 web 搜索补齐）。**混合智能 + 对话伙伴在场。**

---

## 6. 前端设计理念

产品定位从"加密仓库"变成"主动进化的对话伙伴"，前端必须跟上这个心智切换。下面六条设计原则约束所有未来的 UI 改动。

### 6.1 对话优先（Chat-First）
Chat 是默认着陆 tab，不是 9 个 tab 里的一个。其他功能（搜索、录入、条目、设置）是为对话提供支撑的辅助能力，不跟对话竞争注意力。打开 npu-vault 第一眼看到的是光标在输入框里闪，不是仪表盘。

### 6.2 零隐式入口（No Hidden Entrances）
后台工作不出现在 UI 上。SkillEvolver 进化、embedding queue 消费、定时重扫、查询扩展 —— 这些全部在后台静默完成，UI 不弹"建议"、不加"优化中"进度条、不暴露"技能"页。

新能力加入 ≠ 加一个 tab / 按钮 / 开关。能自动化的事情永远自动化；实在需要用户决策的才出 UI。用户感知到的应该是"它越用越好"，而不是"又有新功能要学"。

### 6.3 出处必现（Provenance by Default）
每条 AI 回答自动、强制显示引用源：本地文档（含标题）/ 网络结果（含 URL）/ 多来源（分组）。这不是 settings 可选项，是默认行为。专业人士不会信任"黑盒回答"，看得见出处才能把结果用到真实工作里。

### 6.4 零构建流水线（Zero Build Pipeline）
继续单文件 HTML + vanilla JS + `include_str!()`。不引入 React / Vue / Svelte。理由：
- 跟 Rust 单二进制"一次编译到处运行"哲学一致
- 升级 npu-vault = 换个二进制，没有前端构建、缓存失效、版本漂移问题
- 知识密集型专业人士心智成本容忍度很低，多一个 npm 步骤就是失败

代价是不能用现代组件库。接受。

### 6.5 中性专业感（Calm Professionalism）
暗色但不冷漠；留白代替装饰；字体工整但不严肃；没有赛博朋克、没有花哨动画、没有"AI 未来感"视觉梗。设计气质应该让律师/研究员愿意把它留在工作屏 8 小时，而不是打开十分钟就觉得累。

参考坐标：Linear / Anthropic 官网的克制感，不是 ChatGPT 的 splash effect。

### 6.6 响应式优先（Mobile as First-Class）
NAS 模式 + 手机远程 HTTPS 访问是第一类场景，不是降级适配。tab 在移动端切换到底栏、Chat 输入框永远可见、长列表下拉加载 —— 这些是默认形态，不是后补。

### 6.7 Tab 减法（Consequences of 6.1 + 6.2）

当前 9 tabs（对话 / 搜索 / 录入 / 条目 / 分类 / 聚类 / 远程 / 历史 / 设置）在新定位下有冗余。建议在后续迭代中合并到 **5 tabs**：

| 保留 | 合并到 |
|------|--------|
| 对话（默认着陆） | — |
| 搜索 | — |
| 知识（= 条目 + 分类 + 聚类） | 以文档为主视图，tag 和聚类作为筛选/视图切换，不作为独立 tab |
| 归档（= 录入 + 远程 + 历史） | 统一为"数据进出口"：上传、绑定远程目录、查看对话历史 |
| 设置 | — |

该合并本次 spec 不强制执行，作为后续 UI 重构的方向指引。

---

## 7. 文档改动清单

| 文档 | 改动类型 | 核心内容 |
|------|---------|---------|
| `npu-vault/README.md` | **重写头部 + 重排功能列表** | 新 tagline、加入三大支柱章节、按支柱重排功能、新增"目标用户与场景"小节、新增"主权与透明"小节 |
| `README.md`（仓库顶层） | **更新头部** | tagline 同步、删除"1Password 式加密"表述、同步"两条产品线"描述 |
| `CLAUDE.md`（仓库顶层） | **更新产品描述段落** | 删除"1Password 式加密"旧表述、双产品线段落同步 |
| `npu-vault/DEVELOP.md` | **极小改动** | 只在开头项目定位段同步 tagline，技术章节不动 |
| `npu-vault/RELEASE.md` | **不动** | 历史记录保留；下次发版时用新定位词汇写 changelog |

### npu-vault/README.md 头部改写示例

**Before**
```markdown
# npu-vault

**本地优先、端到端加密的个人知识库引擎。** 跨 Linux / Windows / NAS（HTTPS 远程），通过 Chrome 扩展、本地文件扫描、文件上传自动积累知识，让云端 AI 更懂你。

单一静态 Rust 二进制，零运行时依赖，28 MB 含完整 Web UI、TLS 和加密搜索引擎。

## 功能

- **1Password 式加密** — Master Password + Device Secret → ...
```

**After**
```markdown
# npu-vault

**私有 AI 知识伙伴** — 本地决定，全网增强，越用越懂你的专业。

npu-vault 是为知识密集型专业人士打造的本地 AI 知识伙伴。你的专业领域它会越用越懂；本地知识够用时在本地决定，不够用时主动上网补全；所有数据加密存在你自己的设备上，换设备、换工作都能带走。

单一静态 Rust 二进制约 28 MB，含完整 Web UI、TLS 和加密搜索引擎。

## 三大支柱

### 主动进化
它从每次查询中学习，不需要你配置。本地无命中的查询自动沉淀为信号，后台定期让 LLM 分析并生成同义词扩展，静默生效 —— 三个月后搜同一个词结果明显更准。

### 对话伙伴
RAG Chat 为主界面，每条回答带可追溯的引用源；会话持久化并可搜索，跨时间、跨项目的知识能顺着对话接上。

### 混合智能
本地知识库优先；本地无结果时自动通过浏览器网络搜索补充（零 API 费用）；回答明确标注来源。专业积累留在本地、加密；公开信息现查现用。

## 主权与透明

- Argon2id + AES-256-GCM 字段级加密 + Device Secret，所有数据本地持有
- 单二进制分发，零运行时依赖
- 换设备通过加密导出/导入无损迁移
- **你只付两样钱**：软件本身 + 你自己的 LLM token（如果你用云端 LLM）。无中间商、无搜索 API 订阅、无隐藏费用
```

### 功能列表重排原则

当前的扁平功能列表改为按支柱分组：

```markdown
## 核心能力

### 主动进化
- 失败信号自动沉淀 + SkillEvolver 后台进化（4h / 10 条信号触发）
- 查询词自动扩展（learned_expansions 静默生效）

### 对话伙伴
- RAG Chat + 引用源追溯
- 三阶段检索（vector + BM25 → rerank → top-k）
- 会话持久化 + 跨会话知识联动
- HDBSCAN 聚类"回忆"

### 混合智能
- 本地全文 + 向量混合检索
- 浏览器自动化网络搜索（后台驱动系统已装 Chrome / Edge，零 API 成本）
- 可插拔 Embedding（Ollama / ONNX）和 LLM（Ollama / OpenAI 兼容端点）
- 领域插件（patent / law / tech / presales + 运行时加载）

### 数据主权与透明
- 加密本地存储（Argon2id + AES-256-GCM + Device Secret）
- 单二进制分发，零运行时依赖
- NAS 模式（HTTPS + Bearer auth）
- 加密导出/导入跨设备迁移
- Chrome 扩展兼容 18 个 API 端点
```

---

## 8. 代码改动清单（伴随文档改动）

### 必须改动

- **重构** `npu-vault/crates/vault-core/src/web_search.rs` 拆为模块
  ```
  web_search/
  ├─ mod.rs            WebSearchProvider trait 定义 + from_settings
  ├─ browser.rs        BrowserSearchProvider 实现
  └─ engines/
      ├─ mod.rs        SearchEngineStrategy trait
      └─ duckduckgo.rs 默认引擎抓取策略
  ```

- **删除** 当前 `web_search.rs` 中的：
  - `BraveSearchProvider` 结构体和 impl
  - `TavilySearchProvider` 结构体和 impl
  - `SearxngSearchProvider` 结构体和 impl
  - 对应的 settings 字段（api_key / base_url / provider 枚举）
  - 相关测试（from_settings_brave_creates_provider 等）

- **Cargo.toml** 新增依赖：
  ```toml
  chromiumoxide = { version = "0.7", features = ["tokio-runtime"] }
  ```
  （版本号以实施时最新稳定版为准）

- **routes/settings.rs** 更新 `default_settings()` 里 `web_search` 块结构

- **routes/chat.rs** 更新失败时的用户提示文案（明确说明"安装 Chrome 或 Edge"）

### 新增测试

- BrowserSearchProvider 单元测试
  - 浏览器路径检测（mock 文件系统）
  - 搜索引擎策略解析（本地静态 HTML fixture 模拟 DuckDuckGo 结果页）
- 无浏览器场景：`search()` 返回明确错误而非静默空结果

### 不改动

- `WebSearchProvider` trait 定义（保留作为扩展点）
- chat 路由中的"本地空 → 触发网络搜索"条件逻辑（已经对）
- `skill_evolution.rs`（失败信号记录与网络搜索独立）

---

## 9. 实施顺序

1. **Phase A（本 spec 通过后）** — 产出 writing-plans 的实施计划
2. **Phase B** — 按计划 subagent-driven 或 inline 执行
3. **Phase C** — 最后统一做 Playwright E2E 验证（README 改动不会破坏 Web UI，主要跑 chat + web search 回归）

---

## 10. 开放问题

### 10.1 产品改名（候选讨论中）

`npu-vault` 对外偏技术向，易被误认为"NPU 开发者工具"，且"vault"偏向静态仓库心智，与新定位"主动进化的伙伴"不符。

建议改名。以下是三个候选方向，附简短理由：

| 候选 | 方向 | 为什么候选 | 风险 |
|------|-----|----------|------|
| **Attune** | 动词感，意为"调谐、越来越契合" | 精准对应"越用越懂你的专业"；读音简单（英/中都好读，中文可音译为"雅同"或保留原文）；域名相对可得 | 单词在英语中偏文艺，可能让专业用户觉得不够硬核 |
| **Kin** | 名词，意为"亲属 / 至亲伙伴" | 极短、情感温度高、直接传达"伙伴"关系；易记易打 | 过于通用，SEO 和搜索区分度差 |
| **Lore** | 名词，意为"传承的知识 / 掌故" | 传达"长期积累的领域知识"的气质，对研究员和律师心智契合；短、有品质感 | 与游戏界的"lore"一词重合，可能冲淡专业定位 |

**暂不决定**。建议在本次实施完成、产品定位文档对外之前再做一轮命名 brainstorming（可能需要加入品牌/营销视角）。本次 spec 先用 `npu-vault` 过渡。

### 10.2 定价制度（保留议题）

本 spec 只声明了"零锁定付费"原则（用户只付两样：软件费 + 可选 LLM token）。具体金额、订阅 vs 买断、是否提供免费版等细节，不在定位文档里解决。后续单独议题。

### 10.3 多语言文档

目前所有定位文字以中文为主、Tagline 附英文。未来面向海外用户时需要独立的英文文档线。本次不动。
