# 行业版 Attune 软件设计（独立应用 · 律师 vertical 第一刀）

**版本**：v1 · 2026-04-25
**作者**：qiurui144
**关联决策**：CLAUDE.md「独立应用边界」+「产品决策记录 2026-04-25」5 条
**前置 spec**：[2026-04-17-product-positioning-design.md](2026-04-17-product-positioning-design.md)（三大支柱定位：主动进化 / 对话式 / 混合智能）

---

## 0. 摘要

把 Attune 从"通用私有 AI 知识伙伴"升级为**会员制行业 AI 应用**，第一个 vertical 切律师（个人版 attune-law-personal）。

**与 lawcontrol 关系**：完全独立。不调 lawcontrol API、不复用其代码，可参考其 plugin / RPA / Intent Router 设计模式（七类插件分法 + AI 边界严守），实现完全自研。

**双形态**：
- **B 形态**（主路径）：本地笔电算力 + 远端 LLM token，单一 Win MSI / Linux deb 安装包
- **A 形态**（二期）：K3 一体机（SpacemiT X100，192.168.100.209），底座推理由 K3 :8080 提供，可选装本地 LLM

**核心价值**：律师丢一张借条照片，attune 5 秒内告诉他"这是王某诉李某案 · 第 3 份证据 · 与已有借款合同金额一致 · 与微信记录时间冲突 · 建议补充资金到账银行流水"。

---

## 1. 三层架构

```
┌──────────────────────────────────────────────────────────────┐
│  接入层  Web UI  ·  Chrome 扩展  ·  IM channel（v1.0+）       │
├──────────────────────────────────────────────────────────────┤
│  AI 层   skill (单步)  +  workflow (多步)  +  intent router  │  → 远端 token（默认）
│          plugin.yaml 契约：output schema · needs_confirm     │     K3 形态可走本地
│          chat_trigger.patterns / keywords 自然语言路由       │
├──────────────────────────────────────────────────────────────┤
│  数据层  RPA · 全文 · 向量 · Project 卷宗 · 个人知识库        │  ← 本地（笔电盘）
│          严禁碰 AI · 必须确定性 + 合规                         │     或 K3 SSD
└──────────────────────────────────────────────────────────────┘
```

**AI 边界硬约束**：数据层（RPA / crawler / 检索）禁用任何 AI 调用 — 这是商业可信的底座。AI 只在 AI 层（skill / workflow）里出现。借鉴自 lawcontrol，attune 自研实现。

---

## 2. 数据模型 — Project / Case 卷宗

### 2.1 Project 通用层（attune-core）

```sql
CREATE TABLE project (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    kind TEXT NOT NULL,                    -- 'case' / 'deal' / 'topic' / 'generic'
    metadata_encrypted BLOB,               -- 行业特化字段（律师 = 案件信息），AES-256-GCM
    created_at INTEGER, updated_at INTEGER,
    archived INTEGER DEFAULT 0
);

CREATE TABLE project_file (                -- 多对多：一个文件可属多 Project
    project_id TEXT, file_id TEXT,
    role TEXT,                             -- 行业特化（律师 = 'evidence'/'pleading'/'reference'）
    added_at INTEGER,
    PRIMARY KEY (project_id, file_id)
);

CREATE TABLE project_timeline (            -- 跨证据链推理的时间线
    project_id TEXT, ts INTEGER,
    event_type TEXT,                       -- 'fact' / 'evidence_added' / 'rpa_call' / 'ai_inference'
    payload_encrypted BLOB
);
```

### 2.2 Case 行业层（attune-law plugin）

`metadata_encrypted` 在 attune-law 渲染时反序列化为：

```yaml
case_no: "(2024)京02民终1234号"
court: "北京市第二中级人民法院"
parties:
  - role: plaintiff
    name: "王某"
    type: natural_person
  - role: defendant
    name: "李某"
    type: natural_person
case_type: "民间借贷纠纷"
status: "一审进行中"
filing_date: "2024-03-15"
hearing_dates: ["2024-05-20", "2024-07-08"]
```

attune-core 只看到一个 opaque blob，行业插件解码渲染。

### 2.3 创建时机（Q-A 答案：b 推荐式）

- **第一份文件上传时不强制选 Project**（不打断零散使用）
- AI 在以下任一条件触发后浮出 "建议归档到 Project" 气泡：
  - 用户已上传 ≥ 3 份文件且**实体重叠度** > 0.6（同人名 / 同案号 / 同公司）
  - 用户在 chat 里提到"案件 / 客户 / 项目"等关键词
  - 用户上传新文件时检测到 ≥ 2 个已有文件实体重叠
- 用户三选一：**[新建 Project] / [加入 ${existing}] / [跳过，永远视为零散]**
- 已有文件支持事后批量归类（"案件管理" tab 拖拽分组）

---

## 3. AI 层

### 3.1 plugin.yaml 升级（在 attune-pro 现有基础上加 chat_trigger）

```yaml
# plugins/law-pro/capabilities/contract_review/plugin.yaml
id: law-pro/contract_review
type: skill
name: 合同风险审查
version: "0.1.0"

requires:
  attune_core: ">=0.6.0"

constraints:
  output_format: json
  temperature: 0.2

output:
  schema: { ... 已存在 ... }

# —— 新增：自然语言路由 ——
chat_trigger:
  enabled: true
  needs_confirm: true              # AI 处理前必须用户确认
  priority: 5
  patterns:
    - '(帮我|请).*(审查|审核|review).*(合同|协议|条款)'
  keywords: ['审查合同', '合同风险', '看一下这份合同']
  min_keyword_match: 1
  exclude_patterns: ['起草', '生成']
  requires_document: true          # 必须有上传的文件
  description: "AI 审查合同条款风险"

# —— 新增：跨 Project 上下文要求 ——
context_strategy:
  scope: project                   # 'project' | 'global' | 'file_only'
  inject_top_k_related: 5          # 自动注入同 Project 内最相关的 K 个 chunk
```

### 3.2 Intent Router（attune-core 新增 ~300 行）

```rust
// crates/attune-core/src/intent_router.rs
pub struct IntentRouter {
    skills: Vec<SkillManifest>,    // 启动时扫描 plugins/* 加载
}

impl IntentRouter {
    pub fn route(&self, user_message: &str, context: &ChatContext) -> Vec<IntentMatch> {
        // 1. 正则 patterns 匹配
        // 2. keywords 计数 ≥ min_keyword_match
        // 3. exclude_patterns 否决
        // 4. requires_document 检查 context.has_pending_file
        // 5. 多个匹配按 priority 排序，返回 top-N
    }
}

pub struct IntentMatch {
    pub skill_id: String,
    pub confidence: f32,
    pub needs_confirm: bool,
    pub args: serde_json::Value,
}
```

UI 层：用户敲完一句话 → router 返回匹配 → 如果 confidence > 阈值且 needs_confirm，浮出 chip "AI 检测到你想审查合同，使用 contract_review skill？[确定] [换个问法]"。

### 3.3 跨证据链推理 workflow（核心价值）

新 workflow type，不是 skill。三段式：

```yaml
# plugins/law-pro/workflows/evidence_chain_inference/workflow.yaml
id: law-pro/evidence_chain_inference
type: workflow
trigger:
  on: file_added                   # 文件上传后自动跑（被动触发，但只在 Project scope 内）
  scope: project

steps:
  - id: extract_entities
    type: skill
    skill: law-pro/entity_extraction
    input: { file_id: $event.file_id }
    output: entities                # 人名 / 金额 / 日期 / 案号 / 地点

  - id: cross_reference
    type: deterministic            # 不调 AI，纯 SQL 查询
    operation: find_overlap
    input:
      entities: $extract_entities.entities
      project_id: $event.project_id
    output: related_files

  - id: inference
    type: skill
    skill: law-pro/evidence_chain_skill
    input:
      new_file: $event.file_id
      related: $related_files
      project_metadata: $event.project_metadata
    output:
      - location: "证据归属哪条事实链"
      - relations: "与哪些证据呼应/矛盾"
      - gaps: "证据链还缺什么"

  - id: render
    type: deterministic
    operation: write_annotation
    input: $inference
```

输出落到批注侧栏 + Project timeline 节点，律师打开时一目了然。

---

## 4. 数据层 — 自研 RPA

### 4.1 七类插件分法（参考 lawcontrol）

| type | AI 允许？ | 例子 |
|:---|:---|:---|
| rpa | ❌ 严禁 | npc_law / 公众号 / 裁判文书（v0.7） |
| crawler | ❌ 严禁 | RSS / 法律出版社官网 |
| search | ❌ 严禁 | 已有的 web_search（DuckDuckGo / Bing） |
| **skill** | ✅ | contract_review / lawyer_letter / ... |
| **workflow** | ✅（步骤间编排） | evidence_chain_inference |
| channel | ❌ | 微信群（v1.0+） / Outlook（v1.0+） |
| industry | — | 聚合声明（attune-law）|

### 4.2 RPA 适配器（自研，复用 chromiumoxide）

底层基础已在 `attune-core/src/web_search_browser.rs`（chromiumoxide 驱动 system Chrome）。新增 `attune-core/src/rpa/` 模块：

```rust
// crates/attune-core/src/rpa/mod.rs
#[async_trait]
pub trait RpaAdapter: Send + Sync {
    fn id(&self) -> &str;
    fn manifest(&self) -> &RpaManifest;       // 来自 plugin.yaml
    async fn invoke(&self, op: &str, args: serde_json::Value, ctx: &RpaContext) -> RpaResult;
    async fn health_check(&self) -> AdapterHealth;
}

pub struct RpaContext {
    pub user_id: String,
    pub project_id: Option<String>,
    pub task_id: String,                       // 给前端 follow 的 ID
    pub progress_tx: mpsc::Sender<Progress>,   // 异步增量推进度
    pub browser_pool: Arc<BrowserPool>,        // 共享 chromiumoxide 实例
}
```

### 4.3 v0.6 GA 第一批 RPA（只做免登录）

| adapter | 站点 | 操作 | 工作量 |
|:---|:---|:---|:---|
| `flk_npc` | flk.npc.gov.cn | search_law / get_article（按法条号） | 1 天 |
| `wechat_article` | mp.weixin.qq.com | extract（用户分享 URL，提取正文 + 元信息） | 1 天 |

需账号的（裁判文书 / pkulaw / qichacha）作 v0.7 升级卖点。

### 4.4 RPA 工作流四维（Q-C 答案：b 列清单律师勾选）

**1. 触发模式**

| 模式 | 默认 | 例子 |
|:---|:---|:---|
| 主动 | ✅ ON | chat: "查《劳动合同法》第 39 条" |
| 被动（文件触发） | 🔘 抽实体后**列清单律师勾选** | 上传起诉状 → AI 抽出"被告: 某某有限公司" → 浮气泡："要查工商信息吗？[查] [跳过]" |
| 定时 | 🔘 单条 opt-in | 暂缓到 v0.7 |

**2. 执行模式 — 异步后台 + 顶栏进度面板**

```
chat 输入"查王某 vs 李某 裁判文书"
  → IntentRouter 路由到 wenshu RPA（v0.7）/ flk_npc RPA（v0.6）
  → 立即返回 task_id（< 200ms）
  → 顶栏 chip "后台任务 (1)" 闪现
  → 用户继续聊天 / 浏览
  → 完成（~10s）→ 浏览器内通知 + chat 自动 follow-up
     "查到 5 条结果，归档到 ${project_name}：[#1 ...] [#2 ...]"
  → Project timeline 添加 'rpa_call' 节点
```

**3. 错误恢复**

| 故障 | 处理 |
|:---|:---|
| 账号失效 | 弹窗 "${站点}账号失效，开 headed 浏览器重新登录" → cookie 持久化到 vault |
| 验证码 | 切 headed 模式让用户手动过 |
| 限速 | 自动 backoff（指数退避，最多 3 次）+ 任务面板显示"等待中" |
| 数据缺失 | 返回结构化"未找到" + Suggested rewrite |

**4. 审批门 + 成本可见**

- 每次 RPA 调用前弹气泡（除非 Project 设置了"永远自动通过"）：
  > "即将调用 ${adapter} · 预计 1 配额（剩 99）· ~12s · 远端 LLM 解析 ~200 tok（¥0.0006）。[继续] [跳过] [此 Project 自动通过]"
- 顶栏"后台任务" chip 点开 = 当日 RPA 配额 / Token / 估算费用面板
- 每次调用记录到 Project timeline（合规审计）

---

## 5. 接入层 — Chrome 扩展行业化

### 5.1 现状 vs 目标

现状：扩展只捕 ChatGPT/Claude/Gemini 对话 + 注入个人知识 + 文件上传
目标：**自动捕行业相关浏览习惯**

### 5.2 行业模板

`attune-law plugin` 自带白名单：

```yaml
browser_capture_templates:
  - domain: flk.npc.gov.cn
    label: 国家法律法规库
    selector: { content: ".law-content", title: "h1.law-title" }
    auto_extract_fields: [law_no, effective_date, articles]

  - domain: wenshu.court.gov.cn
    label: 裁判文书网
    requires_user_account: true
    selector: { content: ".PDF_pox", title: ".labelBox" }
    auto_extract_fields: [case_no, court, parties, judgment_date, key_points]

  - domain: mp.weixin.qq.com
    label: 公众号文章（律法相关）
    keyword_filter: ['法律', '判例', '律师', '合同', '合规']  # 标题/作者关键词命中才识别
    selector: { content: ".rich_media_content", title: "#activity-name" }

  - domain: mail.qq.com
    label: 邮箱（标题含案件关键词）
    keyword_filter: ['案号', '诉讼', '律师函', '合同']
    selector: { content: ".body-content", title: ".subject" }
```

### 5.3 自动浮窗 + 三档默认（Q-B 答案：c）

进入白名单页面时：
- 内容抽取（在扩展端 readability + selector）
- **三档默认行为**（首次安装时强制选）：
  - **激进**：5 秒倒计时浮窗，不点就归档到 Suggested Project（1Password Watchtower 风格）
  - **平衡**（推荐 ★）：永远显示气泡，需点击"归档/跳过/永远忽略"
  - **保守**：默认完全不显示，需用户点扩展工具栏图标才归档

### 5.4 检索行为捕获

用户在 pkulaw / 裁判文书网搜索 → 扩展捕获**检索词 + 命中前 5 条标题** → 自动入 Project research log（不入正文，只记元数据）。

### 5.5 浏览习惯画像

每周一早上扩展 sidebar 推送：
> "本周你在 pkulaw 检索 18 次，最关注'劳动合同 解除'。建议关注：《最高法关于审理劳动争议案件适用法律解释（二）》（已自动归档）"

---

## 6. 本地 AI 底座

### 6.1 模块成熟度（盘点 2026-04-25）

| 模块 | 笔电（B 形态） | K3（A 形态） |
|:---|:---|:---|
| Embedding | ORT bge-base / Ollama bge-m3 ✅ | K3 :8080 /v1/embeddings ✅ |
| Rerank | ORT bge-reranker ✅ | K3 :8080 /v1/rerank ✅ |
| ASR | **❌ 缺**（whisper.cpp 待集成） | K3 :8080 /v1/transcribe ✅ |
| OCR | tesseract + poppler ✅ | K3 :8080 /v1/ocr ✅ |
| LLM Chat | Ollama（用户自装）/ 远端 API（默认） | 远端 API 默认 / K3 可选装 |

### 6.2 ASR 集成方案（M3）

- **whisper.cpp binary + Rust subprocess**（与 K3 一致路径）
- 默认 model：**whisper-small Q8**（中文 WER 15-20% 业务可用，~500 MB）
- 安装包捆绑 whisper-cli.exe（Win）/ whisper-cli（Linux）+ ggml-small-q8.bin
- 用户硬件 < 8GB RAM 时降级到 whisper-tiny + UI 提示"精度有限，建议用 K3 一体机"
- 中文 WER 实测加入 `tests/golden/asr_*.json` 做 quality regression

### 6.3 模型分发（M2）

**捆绑**（笔电安装包，~150-200 MB）：
- bge-small ONNX（~90 MB，dim 512，所有硬件 fallback）
- bge-base ONNX（~280 MB，dim 768，≥ 16GB RAM 默认）— 可选下载
- whisper-small Q8（~500 MB）
- tesseract chi_sim 训练数据（~50 MB）

**不捆绑**：
- LLM 模型（用户走远端 token 默认；想本地装的提示运行 `ollama pull qwen2.5:7b`）
- bge-m3（~1.2 GB，"Settings → 升级模型"按需下载）

### 6.4 远端 LLM 默认配置

启动后用户 Settings 里有：
- Endpoint：默认 `https://api.attune.ai/v1`（attune 自营 gateway，含支付宝 / 微信扫码充值）
- 也可填 OpenAI / Anthropic / DeepSeek / 月之暗面 / 智谱 / 云端 Ollama 任意 OpenAI 兼容 endpoint
- API key 加密存到 vault

---

## 7. 跨平台分发（M1 + 平台优先级）

### 7.1 阶段 0：跨平台编译卫生（0.5 周）

```toml
# Cargo.toml
[features]
default = []
cuda = ["ort/cuda"]                  # Linux NVIDIA
directml = ["ort/directml"]          # Windows 核显/独显
# coreml feature 保留但 v0.6/v0.7 不验证
```

### 7.2 Windows 安装包（P0）

- WiX MSI installer
- 捆绑 Ollama runtime（OllamaSetup.exe 内嵌，post-install 自动安装到默认位置 + 设置 systray autostart）
- 捆绑 whisper-cli.exe + tesseract.exe + 模型
- EV Code Signing（生产前必须）+ Defender SmartScreen 注册（首次发版会报误，需要 7-14 天信誉积累）
- v0.6 GA 目标：从下载 .msi 到 Web UI 出现 ≤ 60 秒（含 Ollama 注册服务）

### 7.3 Linux 安装包（P1）

- AppImage（自包含，所有发行版可用）
- .deb（Ubuntu 22.04+ / Debian 12+，systemd user unit）
- .rpm（Fedora 40+）
- Ollama / tesseract 走系统包 dependency（不重复捆绑）
- aarch64 build 给 K3 一体机

### 7.4 macOS（暂不做）

不投入资源至 v1.0；保留 cfg 抽象不破坏未来兼容。

---

## 8. 会员体系

### 8.1 三档定价

| | 笔电软件订阅 | K3 一体机捆绑 |
|:---|:---|:---|
| **个人版** | ¥99/月（含 50 万 tok/月 远端 LLM + flk_npc/wechat 免费 RPA） | ¥3999 硬件 + ¥99/月（同 quota） |
| **专业版** | ¥299/月（含 200 万 tok/月 + 所有 RPA + skill 优先级） | ¥6999 硬件 + ¥299/月 |
| **行业插件包** | 单买 ¥199/月/包（attune-law / attune-presales / ...） | 同 |

### 8.2 License Key（沿用 attune-pro/docs/license-key-design.md）

- HMAC-SHA256 离线校验
- payload：`{ key_id, plan, seats, features, device_fp, issued_at, expires_at, grace_days, customer_id }`
- 失效后 **grace 7 天**全功能 → 7-30 天只读 → 30 天后只能 export
- 撤销列表：CRL 走 attune.ai/api/v1/license/crl，每 24h 拉一次（离线时旧规则生效）

---

## 9. Sprint 节奏（10 周到 attune-law-personal v0.1）

| Sprint | 周 | 交付 | 依赖 |
|:---|:---|:---|:---|
| **0** | 0.5 | 跨平台编译卫生（ort feature 拆 / cfg 补全 / Windows MSVC build 通） | — |
| **1** | 1.5 | Project / Case 数据模型 + AI 推荐归类 + 跨证据链 workflow | S0 |
| **2** | 2 | Intent Router + 9 个 attune-pro skill 加 chat_trigger（5 law + 4 presales）| S1 |
| **3** | 2 | RPA 自研：flk_npc + wechat_article + 异步后台框架 + 顶栏进度面板 | S2 |
| **4** | 1 | 扩展行业化：白名单 + 浮窗 + 三档默认 + 检索捕获 | S2 |
| **5** | 1 | ASR 集成（whisper.cpp） + 中文 WER golden test | S0 |
| **6** | 1 | Win MSI + Linux deb 打包 + Ollama 捆绑 + 安装 smoke test | 全部 |
| **7** | 1 | License key 联调 + 会员配额扣减 + Playwright 全链路 E2E | S6 |

总周期：**~10 周**到可发的 v0.1。

---

## 10. 测试策略

沿用 `docs/TESTING.md` 六层金字塔，新增律师 corpus：

```bash
tests/corpora/law/
├── 真实劳动合同样本-2024.zip      # 公开样本（脱敏）
├── 公开判决书-2023-2024.zip      # 中国裁判文书网公开判决（commit 固化）
├── 民法典全文-2020.md            # 全国人大公开
└── golden/
    ├── contract_review_evidence_chain.json  # 跨证据链推理 golden
    ├── chat_trigger_router.json             # intent router precision
    └── asr_chinese_wer.json                 # 中文 ASR 准确率回归
```

### E2E 关键路径

Playwright 走完一遍**核心场景**：
1. 安装 → unlock vault → 检测到首次启动 → 三档默认选择
2. 上传起诉状 PDF → AI 抽实体 → 浮气泡"要建 Project 吗" → 用户确认
3. 上传借条照片 → OCR → 跨证据链推理 → 批注侧栏出现"与第 1 份合同金额一致"
4. chat 输入"帮我审查这份合同" → IntentRouter 命中 contract_review → 弹 confirm → AI 输出风险清单
5. 文件上传后浮气泡"要查被告工商信息吗" → 用户勾选 → RPA 调 gsxt（v0.7）或 flk_npc（v0.6 demo）
6. 浏览 flk.npc.gov.cn 一篇法条 → 扩展浮窗 → 归档到当前 Project
7. 用户敲入 ¥99/月会员 license key → 配额展示

---

## 11. 风险与未决问题

### 已识别风险

| 风险 | 缓解 |
|:---|:---|
| Windows EV Code Signing 周期长（首次 7-14 天 SmartScreen 信誉冷启动） | v0.6 走 alpha 内测渠道；正式版必须留 2 周 buffer |
| RPA 站点反爬变化（pkulaw / 裁判文书网随时改 selector） | 抽 selector 到 plugin.yaml，热更新；建立 RPA 健康监控 + 自动报警 |
| 中文 WER 小模型不达标 | 实测后再决定默认；不达标的硬件提示"建议上 K3 一体机" |
| 首批律师试用反馈差（Project AI 推荐归类不准） | v0.6 alpha 只发 ≤ 20 个律师；准确率 < 70% 不进 GA |

### 未决问题（v0.7 之前要拍）

- **远端 LLM gateway 谁建？**自营（attune.ai 走 OpenRouter 风格代理 + 国内支付）vs 用户自带 API key（让用户自己上 DeepSeek 等）
- **K3 一体机销售渠道**：直营 / 京东 / 找硬件代工厂？
- **是否需要 lawcontrol 互通的 export/import 格式？**（用户主动触发型）

---

## 12. 与既有 attune-pro 商用仓的关系

attune-pro 现有 9 capabilities（5 law + 4 presales）按本 spec 升级：
- 加 `chat_trigger` 字段 → Intent Router 可路由
- 加 `context_strategy.scope: project` → 跨证据链联想自动注入
- 加 `needs_confirm: true`（关键 skill 用 LLM 前确认）
- 加 attune-law plugin 把 Project 渲染为 Case

**不重写**，只加配置 + 升级。预计 attune-pro 这部分工作量 1-2 天。

---

## 13. 验收清单（v0.1 GA Definition of Done）

- [ ] Win MSI + Linux deb 双包装可用，从下载到 Web UI 出现 ≤ 60 秒
- [ ] 安装包不含 LLM 模型，但含 Ollama runtime + bge-small + whisper-small + tesseract chi_sim
- [ ] Project / Case 数据模型 + AI 推荐归类（准确率 ≥ 70% 在 20 律师样本上）
- [ ] Intent Router 路由 9 个 attune-pro skill 准确率 ≥ 85%
- [ ] flk_npc + wechat_article 两个 RPA 走完异步后台 + 顶栏进度面板 + 错误恢复
- [ ] 跨证据链推理 workflow 在律师 corpus golden test 上 precision@3 ≥ 0.6
- [ ] 中文 ASR WER ≤ 20%（whisper-small Q8）
- [ ] 扩展行业化模板（≥ 5 个白名单域名）+ 三档默认
- [ ] License key 离线校验 + 配额扣减 + grace period
- [ ] Playwright 7 步关键路径全过

---

## 14. 实施前提

- 本 spec 通过用户 review（待）
- 调用 `superpowers:writing-plans` 出每个 Sprint 的实现 plan
- 每个 Sprint 用 `superpowers:subagent-driven-development` 执行
- 用 `superpowers:using-git-worktrees` 隔离开发分支

— end of spec —
