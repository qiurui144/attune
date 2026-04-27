# 行业版扩展框架（通用 plugin SDK + Intent Router + Tauri 双形态）

> **版本**: v1 (slim) · 2026-04-25 · trimmed 2026-04-27
>
> **本文档定位**: 把 Attune 从"通用私有 AI 知识伙伴"扩展为支持任意行业 vertical 的应用框架。
> 通用框架部分（plugin SDK / chat_trigger / Intent Router / Project 通用数据模型 /
> 通用 RPA 框架 / Tauri 双形态 / 跨平台分发）保留在 attune 公开仓。
>
> **商业部分已迁移**: 第一个落地的具体行业 vertical (律师) 的实现细节、定价、收入预估、
> 9 capabilities 详细、Teacher Engine 全套机制、License Key 后端、Sprint 节奏 — 全部迁移
> 到 [attune-pro](https://github.com/qiurui144/attune-pro) 私有仓 (Pro 订阅 + maintainer 可见)。
>
> 见 [docs/oss-pro-strategy.md](../../oss-pro-strategy.md) — OSS×Pro 完整框架 v1。

**关联决策**：CLAUDE.md「独立应用边界」+「产品决策记录 2026-04-25」5 条
**前置 spec**：[2026-04-17-product-positioning-design.md](2026-04-17-product-positioning-design.md)（三大支柱定位：主动进化 / 对话式 / 混合智能）

---

## 0. 摘要

把 Attune 从"通用私有 AI 知识伙伴"升级为支持任意行业 vertical 的应用框架。

**通用框架（本文档覆盖）**：
- 通用 plugin SDK（plugin.yaml + chat_trigger schema + Intent Router）
- 通用数据模型（Project / Case 卷宗）
- 通用 RPA 框架（七类插件分法 + RpaAdapter trait + chromiumoxide 适配器）
- Tauri 双形态（B = 笔电 Tauri 桌面 + 远端 LLM token；A = K3 一体机 headless）
- 同一份 Rust 后端代码（attune-server crate），双形态共享

**具体行业 vertical 实现** (例：律师 attune-law): 定价 / 收入 / 9 capabilities / 行业 RPA / Teacher Engine — 见 attune-pro 私有仓。

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

**AI 边界硬约束**：数据层（RPA / crawler / 检索）禁用任何 AI 调用 — 这是商业可信的底座。AI 只在 AI 层（skill / workflow）里出现。

---

## 2. 数据模型 — Project 通用卷宗

### 2.1 Project 通用层（attune-core）

```sql
CREATE TABLE project (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    kind TEXT NOT NULL,                    -- 'case' / 'deal' / 'topic' / 'generic'
    metadata_encrypted BLOB,               -- 行业特化字段，AES-256-GCM
    created_at INTEGER, updated_at INTEGER,
    archived INTEGER DEFAULT 0
);

CREATE TABLE project_file (                -- 多对多：一个文件可属多 Project
    project_id TEXT, file_id TEXT,
    role TEXT,                             -- 行业特化（角色字段）
    added_at INTEGER,
    PRIMARY KEY (project_id, file_id)
);

CREATE TABLE project_timeline (            -- 跨证据/事件链推理的时间线
    project_id TEXT, ts INTEGER,
    event_type TEXT,                       -- 'fact' / 'evidence_added' / 'rpa_call' / 'ai_inference'
    payload_encrypted BLOB
);
```

attune-core 只看到 opaque encrypted blob — **行业特化字段（律师案号 / 当事方 / 案件类型 / 售前客户 / 学术领域 ...）由对应 vertical 插件解码渲染**。具体字段 schema 见各 vertical 的 plugin.yaml + attune-pro 仓相关文档。

### 2.2 行业 vertical 渲染层

行业 vertical 插件定义 metadata schema + 渲染规则。具体律师 vertical 的 Case schema (案号 / 法院 / 当事方 / 案件类型 / 状态) 见 attune-pro 仓。

attune-core **永不硬编码**任何行业字段 — 全部由 plugin 提供。

### 2.3 创建时机（推荐式而非强制）

- **第一份文件上传时不强制选 Project**（不打断零散使用）
- AI 在以下任一条件触发后浮出 "建议归档到 Project" 气泡：
  - 用户已上传 ≥ 3 份文件且**实体重叠度** > 0.6（同人名 / 同案号 / 同公司）
  - 用户在 chat 里提到行业触发关键词（"案件 / 客户 / 项目 / 案号"等）
  - 用户上传新文件时检测到 ≥ 2 个已有文件实体重叠
- 用户三选一：**[新建 Project] / [加入 ${existing}] / [跳过，永远视为零散]**
- 已有文件支持事后批量归类（"案件管理" tab 拖拽分组）

---

## 3. AI 层

### 3.1 plugin.yaml 升级（chat_trigger 通用 schema）

通用 plugin schema 加 `chat_trigger` 字段让插件可被自然语言触发：

```yaml
# 例：通用 plugin 模板
id: examples/skill_demo
type: skill
name: 示例技能
version: "0.1.0"

requires:
  attune_core: ">=0.6.0"

constraints:
  output_format: json
  temperature: 0.2

output:
  schema: { ... 已存在 ... }

# —— 自然语言路由 ——
chat_trigger:
  enabled: true
  needs_confirm: true              # AI 处理前必须用户确认
  priority: 5
  patterns:
    - '(帮我|请).*(关键词).*(对象)'
  keywords: ['关键词1', '关键词2']
  min_keyword_match: 1
  exclude_patterns: ['排除词']
  requires_document: true          # 必须有上传的文件
  description: "调用本 skill 的描述"

# —— 跨 Project 上下文要求 ——
context_strategy:
  scope: project                   # 'project' | 'global' | 'file_only'
  inject_top_k_related: 5
```

完整 plugin.yaml schema 见 `attune_core::plugin_loader::PluginManifest`（attune 公开仓）。
具体的行业 capability （如律师合同审查的具体 prompts / output schema）见各 vertical 仓 (attune-pro)。

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

UI 层：用户敲完一句话 → router 返回匹配 → 如果 confidence > 阈值且 needs_confirm，浮出 chip "AI 检测到你想 ${行为}，使用 ${skill_id}？[确定] [换个问法]"。

### 3.3 跨实体推理 workflow（通用 workflow 引擎）

新 workflow type，不是 skill。三段式：

```yaml
# 通用 workflow 模板
id: examples/cross_entity_inference
type: workflow
trigger:
  on: file_added                   # 文件上传后自动跑（被动触发，仅在 Project scope 内）
  scope: project

steps:
  - id: extract_entities
    type: skill
    skill: examples/entity_extraction
    input:
      file_id: $event.file_id
    output: entities

  - id: cross_reference
    type: deterministic
    operation: find_overlap
    input:
      entities: $extract_entities.entities
      project_id: $event.project_id
    output: related_files

  - id: render
    type: deterministic
    operation: write_annotation
    input:
      project_id: $event.project_id
      file_id: $event.file_id
      summary: $cross_reference.summary
```

attune-core 提供 workflow 解析 + runner + deterministic ops 库（`extract / lookup / find_overlap / write_annotation`）。具体的行业 workflow（律师跨证据链 / 售前 BANT / 医疗病历关联）由各 vertical 插件用此 schema 实现，不内置在 attune-core。

---

## 4. 数据层 — 自研 RPA 框架

### 4.1 七类插件分法（参考 lawcontrol 设计）

| type | AI 允许？ | 例子 |
|:---|:---|:---|
| rpa | ❌ 严禁 | 政府公开站点查询 / 公众号抓取 / 行业数据库 |
| crawler | ❌ 严禁 | RSS / 行业出版社 |
| search | ❌ 严禁 | 已有的 web_search（DuckDuckGo / Bing） |
| **skill** | ✅ | 单步 LLM 调用，含 prompt + output schema |
| **workflow** | ✅（步骤间编排） | 多步任务编排 |
| channel | ❌ | 微信 / Outlook / Slack |
| industry | — | 行业级聚合声明（attune-law / attune-medical） |

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

### 4.3 行业 RPA 列表

各 vertical 的具体 RPA 实现（律师的法规库 / 售前的客户工商查询 / 医疗的文献库等）见对应 attune-pro plugin pack。attune-core 仅提供框架，不内置任何具体 adapter。

### 4.4 RPA 工作流四维（通用 UX 框架）

**1. 触发模式**

| 模式 | 默认 | 例子 |
|:---|:---|:---|
| 主动 | ✅ ON | chat: "查 ${特定信息}" |
| 被动（文件触发） | 🔘 抽实体后**列清单用户勾选** | 上传文件 → AI 抽实体 → 浮气泡："要查 ${实体相关数据} 吗？[查] [跳过]" |
| 定时 | 🔘 单条 opt-in | 暂缓到 v0.7 |

**2. 执行模式 — 异步后台 + 顶栏进度面板**

```
chat 输入触发 → IntentRouter 路由到对应 RPA adapter
  → 立即返回 task_id（< 200ms）
  → 顶栏 chip "后台任务 (1)" 闪现
  → 用户继续聊天 / 浏览
  → 完成（~10s）→ 浏览器内通知 + chat 自动 follow-up
  → Project timeline 添加 'rpa_call' 节点
```

**3. 错误恢复**

| 故障 | 处理 |
|:---|:---|
| 账号失效 | 弹窗 → 开 headed 浏览器重新登录 → cookie 持久化到 vault |
| 验证码 | 切 headed 模式让用户手动过 |
| 限速 | 自动 backoff（指数退避，最多 3 次） |
| 数据缺失 | 返回结构化"未找到" + Suggested rewrite |

**4. 审批门 + 成本可见**

- 每次 RPA 调用前弹气泡（除非 Project 设置了"永远自动通过"）
- 顶栏"后台任务" chip 点开 = 当日 RPA 配额 / Token / 估算费用面板
- 每次调用记录到 Project timeline（合规审计）

---

## 5. 接入层 — Chrome 扩展行业化框架

### 5.1 现状 vs 目标

现状：扩展只捕 ChatGPT/Claude/Gemini 对话 + 注入个人知识 + 文件上传
目标：**自动捕行业相关浏览习惯**（W3 batch B G1 已实现通用浏览捕获 + G5 隐私面板）

### 5.2 行业模板机制

每个 vertical 插件可声明 `browser_capture_templates` 列表（白名单域名 + selector + 提取字段）。具体的行业模板（如律师的法规库 / 裁判文书网模板）见对应 attune-pro plugin。attune-core / Chrome 扩展提供模板加载 + 应用机制，不内置具体行业模板。

### 5.3 自动浮窗 + 三档默认（W3 G5 已实现）

- 三档：`automatic` / `whitelist_only` / `manual`
- 默认 `whitelist_only`（保护用户隐私）
- 全局 hard blacklist（银行 / 医疗 / 政府敏感页面）双层正则强阻断

### 5.4 检索行为捕获

用户在白名单站点的搜索行为（query + 点击的结果）入 `browse_signals` (G1 已实现) → SkillEvolver (W5+) 反哺。

### 5.5 浏览习惯画像

跨 session topic cluster (G4 W7-8 计划) → SkillClaw 失败信号源之一。

---

## 6. 本地 AI 底座

### 6.1 模块成熟度（盘点 2026-04-25）

| 模块 | 当前状态 | M1 目标 |
|:---|:---|:---|
| Embedding (bge-m3) | ✅ 已捆绑 (~1.2 GB) | 保持 |
| Rerank | 🔘 用 Ollama 走 LLM rerank | M3 加 bge-reranker-base |
| ASR (whisper.cpp) | ❌ 未集成 | M3 集成 + WER golden test |
| OCR (tesseract) | ✅ subprocess 调用 | 保持 |
| LLM | 🚫 不捆绑（远端 token 默认） | 不变 |

### 6.2 ASR 集成方案（M3）

whisper.cpp binary + Rust subprocess（与 K3 推理服务一致路径），中文 WER 必须 < 20%（whisper-small Q8 实测满足）才能选默认模型；whisper-tiny WER 35-40% 不可用。

### 6.3 模型分发（M2）

**捆绑安装包**：
- whisper.cpp binary (~10 MB)
- tesseract Chinese language data (~30 MB)
- Ollama runtime (~50 MB)

**不捆绑**：
- LLM 模型（用户走远端 token 默认；想本地装的提示运行 `ollama pull qwen2.5:7b`）
- bge-m3（~1.2 GB，"Settings → 升级模型"按需下载）

### 6.4 LLM provider 配置（通用）

启动后用户 Settings 里有：
- Endpoint：默认 OpenAI 兼容 (用户自填或选 attune 自营 gateway)
- 也可填 Anthropic / DeepSeek / 月之暗面 / 智谱 / 云端 Ollama 任意 OpenAI 兼容 endpoint
- API key 加密存到 vault
- Settings 全部本地存储；不强制走 attune 自营 gateway

attune 自营 gateway 详细定价、quota、付费充值流程见 attune-pro 仓的 LLM proxy 设计文档。

---

## 6.5 桌面壳（Tauri 2）

### 6.5.1 核心决策

- **入口形态 = 双轨发版** — Tauri 桌面包给笔电用户，纯 attune-server 包给 K3 / NAS / 服务器。同一份 Rust 后端代码
- **WebView 加载方式 = HTTP 加载 :18900** — 前端零改动，Tauri 是浏览器壳。后期可增量把高频 API 改走 Tauri IPC
- **attune-server 启动方式 = Tauri 主进程内嵌 axum runtime** — 单一二进制 Attune.exe / .AppImage，启动时同时拉起 axum + WebView

### 6.5.2 架构

```
Attune Desktop（笔电用户）            Attune Server（K3 / NAS / 服务器）
┌──────────────────────────┐         ┌──────────────────────────┐
│ apps/attune-desktop      │         │ rust/crates/attune-server│
│ (Tauri 2, Rust)          │         │ (axum :18900 only)       │
│                          │         │                          │
│ ├─ Tauri WebView         │         │  ← 浏览器 / Chrome 扩展  │
│ │  → 加载 :18900         │         │     远程访问             │
│ ├─ 系统托盘 + 单实例     │         │                          │
│ ├─ 原生通知 / 文件关联   │         │                          │
│ ├─ 自动更新（见 §6.6）   │         │                          │
│ └─ 内嵌 attune-server    │         │                          │
│    （axum 跑在同进程）   │         │                          │
└──────────────────────────┘         └──────────────────────────┘
        Win MSI / Linux deb                Linux deb / aarch64
```

### 6.5.3 Cargo workspace 改动

```toml
# rust/Cargo.toml
[workspace]
members = [
    "crates/attune-core",
    "crates/attune-server",       # 改为 lib + 旧 bin headless 入口保留
    "apps/attune-desktop",        # 新增 — Tauri shell + 内嵌 attune-server
]
```

`attune-server` 从单一 binary crate 改为 **library + 一个 headless bin**：
- `lib.rs` 暴露 `pub fn run_in_runtime(handle: tokio::runtime::Handle, config: ServerConfig) -> ServerHandle`
- `bin/attune-server-headless.rs` 是原 main，保留给 K3 / NAS 部署
- `apps/attune-desktop/src/main.rs` 调用 `attune_server::run_in_runtime()` 把 axum 跑在 Tauri tokio runtime 上

### 6.5.4 桌面壳必备特性

| 特性 | 实现 | 优先级 |
|:---|:---|:---|
| 系统托盘（关闭最小化） | `tauri::tray::TrayIconBuilder` | P0 |
| 单实例锁（重复双击只激活已有窗口） | `tauri-plugin-single-instance` | P0 |
| 启动 splash + axum 健康检查 | 自定义 webview pre-init | P0 |
| 文件关联（双击 .pdf 用 attune 打开） | tauri.conf.json `fileAssociations` | P1 |
| 拖拽文件到窗口 → 自动上传 | webview drop event → invoke('upload') | P0 |
| 原生通知 | `tauri::notification::Notification` | P1 |
| 自动启动（开机自启） | `tauri-plugin-autostart` | P2，opt-in |
| 系统暗色模式跟随 | `tauri::WebviewWindow::theme()` | P2 |

---

## 6.6 安装更新策略

### 6.6.1 设计目标

- **零摩擦**：用户不需要主动检查更新；后台静默检查，重大版本弹窗提示
- **可信任**：所有更新包 Ed25519 签名，离线验签
- **可控**：用户可选 stable / beta 通道；可关闭自动检查
- **回滚**：若新版崩溃，下次启动时检测到 panic 自动建议回滚（保留上一版二进制）
- **双轨形态各自适配**：Tauri Desktop 走 in-app updater；attune-server-only 走系统包管理

### 6.6.2 通道与版本策略

- `stable`: GA 版本（默认）
- `beta`: rc 版本（用户 opt-in）
- `nightly`: develop 分支构建（仅开发者）

### 6.6.3 数据迁移

vault schema 升级走 `migrate_*` fn 模式（per W3 batch A 的 `migrate_breadcrumbs_encrypt` 经验）。详见 [attune/docs/migration-pepper.md](../../migration-pepper.md)。

---

## 7. 跨平台分发（M1 + 平台优先级）

### 7.1 平台优先级（per CLAUDE.md）

- **Windows P0** （MSI / NSIS）
- **Linux P1** （deb / AppImage）
- **macOS 暂不做** （aarch64 留作 K3 一体机）

### 7.2 Attune Desktop（笔电用户，Tauri bundler 打包）

```bash
cd apps/attune-desktop
cargo tauri build --bundles deb,appimage    # Linux
cargo tauri build --bundles nsis,msi         # Windows
```

详见 `.github/workflows/desktop-release.yml`。

### 7.3 Attune Server（K3 / NAS / 服务器，纯 attune-server）

```bash
cd rust && cargo build --release -p attune-server -p attune-cli
```

详见 `.github/workflows/rust-release.yml`。

### 7.4 跨平台编译卫生

per CLAUDE.md 跨平台规范：
- 文件路径用 PathBuf；`#[cfg(unix)]` 保护 Unix 特有调用
- C/C++ 依赖（rusqlite bundled / usearch）需 CI 矩阵验证
- 测试隔离：tempfile::TempDir + Mock providers，不依赖 GPU/Ollama

---

## 8. 会员体系

通用 license key 校验 (Ed25519 + offline verify) 由 attune-core 提供 — 这是商业插件 gating 机制，OSS 用户走 lite 档（无 license key 即 free 全功能）。完整定价 / Plan tiers / License Key 后端详见 [docs/oss-pro-strategy.md §3](../../oss-pro-strategy.md) 和 attune-pro 仓的 `docs/license-key-design.md`。

---

## 9. 行业 vertical 落地节奏

通用 attune-core 的 W3+W4 路线见 [docs/v0.6-release-readiness.md](../../v0.6-release-readiness.md)。

具体行业 vertical 的 11 周 Sprint 计划（包含 license 联调、配额扣减、Playwright E2E）见 attune-pro 仓的 industry-vertical-design.md §9。

---

## 10. 测试策略

沿用 [docs/TESTING.md](../../TESTING.md) 六层金字塔：

```bash
tests/corpora/
├── ${vertical}/                  # 各 vertical 用真实公开 corpus
│   ├── samples-${year}.zip      # 公开样本（脱敏）
│   ├── reference-corpus.md      # 行业参考全文
│   └── golden/
│       ├── ${capability}.json   # 各 capability 的 golden test
│       └── ${chain}.json        # 跨实体推理 golden
└── ...
```

行业 vertical 的具体 corpus（律师 corpus / 售前 corpus / 医疗 corpus）由 attune-pro 仓维护，不进 attune 公开仓。

### E2E 关键路径（通用）

Playwright 走完一遍**通用核心场景**：
1. 安装 → unlock vault → 检测到首次启动 → 三档默认选择
2. 上传文档 PDF → AI 抽实体 → 浮气泡"要建 Project 吗" → 用户确认
3. 上传相关文件 → OCR → 跨实体推理 → 批注侧栏出现关联

具体行业 E2E 路径（律师跨证据链 / 售前 BANT 流程 / 医疗病历关联）见 attune-pro 仓。

---

## 11. 风险与未决问题

### 已识别风险（通用框架层）

- **跨平台编译矩阵**: Win MSVC build 历史多次因依赖（usearch C++ / cross-env）失败 → CI 必须三平台验证
- **Tauri 2 升级风险**: webview 加载 :18900 在某些 Win 版本不稳 → 备选方案：Tauri IPC 走 attune-core 直调
- **plugin loader 加载顺序**: 多 vertical 同时 enable 时 chat_trigger 优先级冲突 → Intent Router 必须按 priority + needs_confirm 严格排序

### 未决问题（v0.7 之前要拍）

- 是否支持单机多 vault（一台机器跑两个独立用户）？
- workflow 内 deterministic ops 是否需要 sandboxing（防 plugin 调危险 ops）？
- Project 跨 vault 共享（团队场景）的 schema 怎么设计？

商业化未决问题（定价 / SLA / 客户分级）见 attune-pro 仓。

---

## 12. 与 attune-pro 商用仓的关系

详细见 [docs/oss-pro-strategy.md](../../oss-pro-strategy.md)。

- attune-core 是**通用底座**，行业功能（律师 / 售前 / 学术 / 医疗等）全部在 attune-pro
- 任何包含定价 / 收入 / 客户具体信息 / 行业 RPA 实现细节的内容，必须在 attune-pro 仓
- 任何通用框架（plugin SDK / Intent Router / RPA 框架 / 数据模型 / Tauri 架构）在 attune 公开仓
- 跨仓接口契约见 [attune-pro/INTEGRATION.md](https://github.com/qiurui144/attune-pro)

### 行业耦合修订（2026-04-25 Phase D-0 cleanup + 2026-04-27 边界审计）

**2026-04-25 Phase D-0**: 原 Phase A+B+C 错误把 `law-pro/evidence_chain_inference` workflow 编入 attune-core builtins.rs，Phase D-0 移除：
- 通用 workflow 引擎留 attune-core
- 律师 evidence_chain workflow 由 attune-pro 在 plugin.yaml 注册（Sprint 2 plugin loader）
- attune-pro 仓的 plugin.yaml 注册 workflow → attune-core 加载 → file_added 时按 trigger 匹配

**2026-04-27 边界审计**: 通用 plugin SDK 文档原本暴露 attune-pro 9 capabilities 详细 + 定价 + Teacher Engine — 全部迁移到 attune-pro 仓 industry-vertical-design.md。本文档保留通用框架部分。

---

## 13. 验收清单（v0.6 GA Definition of Done — 通用部分）

### 桌面体验（Tauri Desktop）

- [ ] Win MSI 双击安装，启动后自动打开 Tauri 窗口 + 系统托盘图标
- [ ] Linux deb / AppImage 同上
- [ ] 单实例锁：第二次双击只激活已有窗口
- [ ] 拖拽 PDF 到窗口 → 自动上传到当前 vault
- [ ] 关闭窗口时最小化到托盘，托盘菜单 Quit / Show

### 安装与更新

- [ ] 首次启动检查更新（默认 stable 通道）
- [ ] Ed25519 验签失败时拒绝安装
- [ ] 通道切换 stable / beta 在 Settings 内
- [ ] 一次性同意（隐私 + 数据收集）见 attune-pro Teacher Engine 章节（订阅版本特有）

### Headless 兼容（双轨）

- [ ] 纯 attune-server binary（不含 Tauri webview）能在 Linux x86_64 / aarch64 跑
- [ ] 浏览器访问 :18900 与 Tauri 内嵌 webview 体验一致
- [ ] Chrome 扩展同时可访问本地 :18900 + 远程 attune-server :18900

### 通用插件加载

- [ ] plugin.yaml schema 解析通过（参考 `attune_core::plugin_loader::PluginManifest`）
- [ ] chat_trigger 字段 patterns / keywords 匹配测试覆盖
- [ ] Intent Router 多匹配按 priority 排序

商业化与合规验收（License key / Pro 订阅 / 行业 RPA 完整性）见 attune-pro 仓。

---

## 14. 实施前提

完成 v0.6.0 GA 后才考虑行业 vertical 的具体落地：

- ✅ W3+W4 全部完成（plugin SDK + chat_trigger schema + Intent Router 设计准备完成）
- ✅ Tauri 2 桌面壳 v0.6.0-alpha.3 已 build 成功
- ⏳ v0.6.0-rc.1 → soak 7 天 → GA
- ⏳ attune-pro 仓 bump cargo dep tag → v0.6.0
- ⏳ 行业 vertical (律师 attune-law) 的 Sprint 2-7 见 attune-pro 仓

---

## 快速链接

- OSS×Pro Strategy v1: [docs/oss-pro-strategy.md](../../oss-pro-strategy.md) (双语)
- v0.6 release planning: [docs/v0.6-release-readiness.md](../../v0.6-release-readiness.md)
- Migration pepper: [docs/migration-pepper.md](../../migration-pepper.md)
- 行业 vertical 完整版（含商业内容）: attune-pro 仓 `docs/industry-vertical-design.md`
- 跨仓接口契约: [attune-pro/INTEGRATION.md](https://github.com/qiurui144/attune-pro)
