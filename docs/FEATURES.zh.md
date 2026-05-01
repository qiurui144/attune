# Attune 功能矩阵 (v0.6.1)

> **状态**：活文档 — 每次 release 同步更新。
> **受众**：贡献者、插件开发者、QA、新读者 onboarding。
> **配套**：[`TESTING.md`](TESTING.md)（测试金字塔 + 能力覆盖映射）、[`oss-pro-strategy.zh.md`](oss-pro-strategy.zh.md)（OSS × Pro 边界）。
> **双语**：[English](FEATURES.md)。

---

## 0. 阅读说明

每条能力有稳定的 **ID**（`F-{nn}-{TOPIC}`），让 commit message、测试 case、PR 描述都能明确引用。

每条能力包含 4 个固定字段：

- **能力** — 用户/客户端看到的内容
- **代码** — 主要逻辑所在的模块（`crate::path` + 关键文件）
- **测试覆盖** — 哪些测试文件覆盖此能力，按测试金字塔层（Unit / Integration / System / E2E / Smoke）映射。层级定义见 [`TESTING.md`](TESTING.md)
- **成熟度** — ✅ **Active**（已发布、默认开启）/ 🟡 **Partial**（已发布但有 flag、覆盖部分）/ ❌ **Designed**（仅设计、未实装）

OSS attune 包含 **18 条核心能力**（本文档）。行业纵向（律师 / 专利 / 医疗 / 学术 / 售前 / 工程师）在 `attune-pro` 中独立文档化。

---

## 1. 能力矩阵（一行总结）

| ID | 能力 | 支柱 | 成熟度 |
|----|------|------|--------|
| **F-01-VAULT** | 三因子加密 vault + 状态机 + 跨设备迁移 | 🔐 主权 | ✅ |
| **F-02-RAG** | 混合检索（BM25 + 向量 + RRF）+ J1 路径前缀 chunker + 两阶段层级检索 | 📚 RAG 引擎 | ✅ |
| **F-03-CHAT** | RAG Chat + B1 引用 breadcrumb + 会话持久化 + 跨会话延续 | 💬 对话 | ✅ |
| **F-04-READER** | Reader 模态 + 5 类用户批注 + 4 角度 AI 批注 + 批注加权 RAG | 📖 阅读 | ✅ |
| **F-05-COMPRESS** | 上下文压缩流水线 + 摘要缓存（70-85% 云端 token 节省） | 🗜️ 成本 | ✅ |
| **F-06-WEBSEARCH** | 浏览器自动化网络搜索（chromiumoxide / DuckDuckGo）+ 30 天加密缓存 | 🌐 混合智能 | ✅ |
| **F-07-EVOLUTION** | Episodic memory consolidation + SkillEvolver 失败信号扩展词 | 🧬 主动进化 | ✅ |
| **F-08-BROWSEEXT** | Chrome 扩展 G1/G2/G5：通用浏览捕获 + 自动 bookmark + 隐私面板 | 📥 捕获 | ✅ |
| **F-09-FORMFACTOR** | **★ v0.6.1**：FormFactor 形态分裂（Laptop/K3Appliance/Server/Unknown）— LLM 默认路径 | 🪪 形态感知 | ✅ |
| **F-10-GOVERNOR** | H1 资源治理（3 档 + per-task 限流 + 顶栏 pause） | ⚙️ 系统友好 | ✅ |
| **F-11-PLUGINS** | 插件框架（`plugin.yaml` schema + `EntityExtractor` trait + marketplace toggle） | 🔌 可扩展 | ✅ |
| **F-12-PROJECT** | Project / Case 通用层 + 跨证据链 Project recommender | 🗂️ 组织 | 🟡 |
| **F-13-WORKFLOW** | Workflow 引擎 + Intent Router 自然语言路由 | 🔄 自动化 | 🟡 |
| **F-14-ENTITIES** | 通用 entity extractor（Person / Money / Date / Organization） | 🧩 NLP | ✅ |
| **F-15-MCP** | Python stdio shim 包装 REST，对接 MCP 客户端 | 🔧 集成 | ✅ |
| **F-16-DISTRIBUTION** | Tauri 2 桌面（Win MSI/NSIS、Linux deb/AppImage）+ NAS HTTPS + 硬件画像 | 📦 分发 | ✅ |
| **F-17-PRIVACY** | Phase A.5 三层隐私（L0 chunk 隔离 / L1 PII 占位符 / L3 v0.7+）+ F-Pro 跨域防御 | 🔒 隐私 | 🟡 |
| **F-18-QUALITY** | K2 Parse golden set（CI 门控）+ RAGAS 风格 benchmark harness | 📊 质量 | ✅ |

---

## 2. 详细能力

### F-01-VAULT — 三因子加密 vault

**能力**：
用户的主密码 + 设备绑定的 256-bit 密钥 + Argon2id（64MB / 3 轮）派生主密钥，主密钥再加密三把数据加密密钥（DEK_db / DEK_idx / DEK_vec）。SQLite 中 content / tags / metadata 走字段级 AES-256-GCM；tantivy 索引和 usearch 向量各自持有 DEK。Vault 三态：**SEALED**（未设密码）→ **LOCKED**（已设密码、闲置）→ **UNLOCKED**（活跃会话，4h TTL via HMAC-SHA256 token）。锁定时所有密钥从内存 zeroize（`Zeroize` trait）。跨设备迁移：加密 `.vault-profile` 导出/导入 — 设备密钥滚动迁移，主密码保留在用户脑袋里。

**代码**：
- `attune-core::vault`（`crates/attune-core/src/vault.rs:1-450`）
- `attune-core::crypto`（`crates/attune-core/src/crypto.rs`）
- `routes::vault`（`crates/attune-server/src/routes/vault.rs`）— REST 端点（setup / unlock / lock / change-password / device-secret/export & import）

**测试覆盖**：
- Unit：`vault::tests`（16 个测试覆盖 setup-twice、dek-access-without-unlock、session-token-tampering、full-lifecycle CRUD）、`crypto::tests`（3 个 derive-master-key 确定性）
- Integration：`tests/change_password_test.rs`、`tests/session_revoke_test.rs`、`tests/migration_roundtrip_test.rs`
- System：`tests/vault_setup_test.rs`（HTTP 层 wizard setup → unlock → lock）
- Smoke：`scripts/smoke-test.sh` 检查 `/api/v1/vault/status` 在新装时返回 SEALED
- E2E：🟡 wizard step 1（master password）由 C.3 golden flow 覆盖

**成熟度**：✅ Active。v0.6.0 GA 已发，v0.6.1 不变。

---

### F-02-RAG — 混合检索引擎

**能力**：
三阶段检索流水线：(1) 并行候选生成 — BM25（tantivy + jieba 中文分词）+ 向量相似度（usearch HNSW，f16 量化）；(2) RRF 融合，可配置权重（默认 0.6 向量 / 0.4 全文）；(3) reranker（`bge-reranker-base` via ONNX Runtime）re-score top-K。Chunker（`chunker.rs::extract_sections`）做两层层级分块 — 章节级（~1500 字，Markdown 标题 / Rust `def|class` / 段落 fallback）+ 段落级（512 字）。检索时返回父章节上下文 + 命中段落 chunk，避免"无上下文 chunk 喂给 LLM"的失败模式。**J1 路径前缀 chunker** 给每个 chunk 头部加 `[Document > Section > Subsection]` 让 LLM 知道在读哪份文档。**J3 显式 min_score** 让用户在 recall 和 precision 之间权衡（默认 0.0）。**J5 强约束 prompt** 禁止"我不知道"等模糊用语，并发出 1-5 置信度 marker，parser 在显示给用户前剥离。

**代码**：
- `attune-core::search`（`crates/attune-core/src/search.rs:1-600`）— `search_relevant()` 是混合检索入口
- `attune-core::chunker`（`crates/attune-core/src/chunker.rs`）— `extract_sections()` + 路径前缀
- `attune-core::index`（`crates/attune-core/src/index.rs`）— tantivy 包装
- `attune-core::vectors`（`crates/attune-core/src/vectors.rs`）— usearch 包装
- `attune-core::infer::reranker`（`crates/attune-core/src/infer/reranker.rs`）— `bge-reranker-base` via `ort`
- `routes::search`（`crates/attune-server/src/routes/search.rs`）

**测试覆盖**：
- Unit：`search::tests`、`chunker::tests`、`index::tests`
- Integration：`tests/rag_w2_batch1_integration.rs`、`tests/rag_quality_benchmark.rs`
- System：`rust/tests/corpus_integration_test.rs`（真实 rust-book + cs-notes 语料）
- Quality：`rust/tests/golden/queries.json` precision@K 回归
- Smoke：未覆盖（需要已索引语料）

**成熟度**：✅ Active。

---

### F-03-CHAT — RAG Chat + 引用 + 会话

**能力**：
Chat 是主交互界面。每条消息流经：query intent → 混合检索（F-02）→ 上下文压缩（F-05）→ 强约束 prompt → LLM 调用（本地 Ollama 或远端 OpenAI 兼容端点）→ 置信度 parser → citation chip 渲染。每个 citation chip 携带 `source`（item id）、`breadcrumb`（章节路径）、`chunk_offset_start/end`（Reader 跳转锚点）、`confidence`（1-5）。会话用 AES-256-GCM 持久化（`store::conversations`）、可按 query 搜索，**跨会话延续**意味着 3 周前的对话能从断点接着聊。**不实现流式输出** — 本地 0.6B-3B 模型响应足够快；远端 API 等待时显示 spinner（per CLAUDE.md 产品决策）。

**代码**：
- `attune-core::chat`（`crates/attune-core/src/chat.rs`）— `ChatEngine` + `parse_confidence`
- `attune-core::store::conversations`（`crates/attune-core/src/store/conversations.rs`）
- `routes::chat` + `routes::chat_sessions`（`crates/attune-server/src/routes/chat.rs`、`chat_sessions.rs`）

**测试覆盖**：
- Unit：`chat::tests`（parse_confidence / strip_confidence_marker / citation 提取边界 case）
- Integration：🟡 部分 — `routes::chat::tests` smoke
- System：F-CHAT-S1（B.3 计划）：完整 wizard → ingest → chat → citation → Reader 跳转
- E2E：🟡 C.3 golden flow #3 覆盖 citation → Reader 跳转

**成熟度**：✅ Active。

---

### F-04-READER — Reader + 批注 + AI 批注

**能力**：
Reader 模态以 chunk 级导航渲染存储的 item。**用户批注**：选中文字 → 5 个预设标签（⭐ 重点 / 📍 待深入 / 🤔 存疑 / ❓ 不懂 / 🗑 过时）+ 4 色高亮 + 自由附注。**AI 批注**："🤖 AI 分析 ▾"下拉选 4 个角度（⚠️ 风险 / 🕰 过时 / ⭐ 要点 / 🤔 疑点）；本地 LLM 分析 chunk 并发出带精确 offset 的批注。**批注加权 RAG**：检索时 ⭐ 要点 / ⚠️ 风险 → ×1.5 boost；🤔 疑点 → ×1.2；🗑 / 🕰 过时 → 直接剔除。批注 content 字段 AES-256-GCM 加密；item 软删除时级联（语义："忘记知识"）。

**代码**：
- `attune-core::store::annotations`（`crates/attune-core/src/store/annotations.rs`）
- `attune-core::ai_annotator`（`crates/attune-core/src/ai_annotator.rs`）
- `attune-core::annotation_weight`（`crates/attune-core/src/annotation_weight.rs`）
- `routes::annotations`（`crates/attune-server/src/routes/annotations.rs`）

**测试覆盖**：
- Unit：`annotation_weight::tests`、`ai_annotator::tests`
- Integration：`tests/rag_w3_batch_a_integration.rs`（批注 → 加权 RAG 端到端）
- E2E：🟡 C.3 golden flow #2 覆盖用户批注 → RAG boost 验证

**成熟度**：✅ Active。

---

### F-05-COMPRESS — 上下文压缩 + 摘要缓存

**能力**：
检索命中的 chunk 先过本地 LLM 调用压缩成 **150 字摘要**（economical 模式，默认）或 **300 字头 + 摘要**（accurate 模式），然后再拼到 chat LLM prompt。这把云端 token 消耗减少 70-85%。摘要按 `sha256(chunk_text)` 索引，在 `store::chunk_summaries` 加密持久化，永久复用 — 一次成本，永久受益。"raw" 模式跳过压缩（仅本地）。Token Chip UI 实时估算 input token + 云端价格，区分 🟢 Local（免费）和 💰 Cloud（带 $）。

**代码**：
- `attune-core::context_compress`（`crates/attune-core/src/context_compress.rs`）
- `attune-core::store::chunk_summaries`（`crates/attune-core/src/store/chunk_summaries.rs`）

**测试覆盖**：
- Unit：`context_compress::tests`
- Integration：`tests/rag_w3_batch_b_integration.rs`（压缩 → 缓存 → 复用）

**成熟度**：✅ Active。

---

### F-06-WEBSEARCH — 浏览器自动化网络搜索

**能力**：
当本地 vault 无高置信度匹配时，attune 通过 CDP 协议（`chromiumoxide` crate）后台驱动用户系统已装的 Chromium 内核浏览器（Chrome/Edge）抓取 DuckDuckGo HTML 结果。**零 API key、零订阅费**。最低间隔 2s 限流。失败模式明确：未找到浏览器 → 日志 warning + 返回空结果 + Chat 追加"网络搜索不可用，请安装 Chrome 或 Edge"；**绝不**静默降级到付费 API。结果在 `store::web_search_cache` 加密缓存 30 天（AES-256-GCM，`sha256(query)` 索引）。

**代码**：
- `attune-core::web_search`（trait）、`web_search_browser`（impl）
- `attune-core::store::web_search_cache`
- `routes::web_search_cache`（`crates/attune-server/src/routes/web_search_cache.rs`）

**测试覆盖**：
- Unit：`web_search_browser::tests`、`store::web_search_cache::tests`（at-rest 加密、覆盖语义）
- Integration：🟡 部分（mock CDP）
- System：🟡 人工（真实 Chrome 实例）— 由 `tests/MANUAL_TEST_CHECKLIST.md` 覆盖

**成熟度**：✅ Active。

---

### F-07-EVOLUTION — 主动学习闭环

**能力**：
两个协作循环：(1) **Episodic memory consolidation**（A1）— 周期后台 agent 审视近期对话，把重复的 thread 浓缩成可按 intent 召回的紧凑"episode"。(2) **SkillEvolver** — 静默记录本地无命中查询为"失败信号"，每 4 小时（或累积 10 条）发给 LLM 提议同义词扩展，静默写入 `learned_expansions` 配置。三个月后同样的 query 返回明显更相关的结果 — 没有任何"重新训练" UI。

**代码**：
- `attune-core::memory_consolidation`（`crates/attune-core/src/memory_consolidation.rs`）
- `attune-core::skill_evolution`（`crates/attune-core/src/skill_evolution.rs`）
- `attune-core::store::memories`、`store::signals`

**测试覆盖**：
- Unit：`skill_evolution::tests`、`memory_consolidation::tests`
- Integration：`tests/memory_consolidation_integration.rs`

**成熟度**：✅ Active。K1 sleeptime agent 升级在 M3+ roadmap（Letta 启发）。

---

### F-08-BROWSEEXT — Chrome 扩展通用浏览捕获

**能力**：
Chrome MV3 扩展 v0.6 起从"AI 对话捕获专用"升级为**通用浏览状态知识源**。捕获 URL / title / time-on-page / scroll depth / copy-paste actions / dwell time / 回访频次。**G1**：信号通过 `/api/v1/browse_signals` 流到 attune 后端。**G2**：高 engagement 页面（≥3min 停留 + >50% 滚动 + ≥1 copy-paste）自动 bookmark 到 staging 区供用户审查。**G5**：popup 中的隐私面板显示捕获了什么、一键清除全部数据、编辑 per-domain 白名单，`HARD_BLACKLIST`（银行 / 医疗 / 政府登录 / 密码管理器 / incognito / 含 `password` 字段的页面）用户无法启用。

**代码**：
- `extension/`（TypeScript，Manifest V3 + Preact + Vite）
- `routes::browse_signals`（`crates/attune-server/src/routes/browse_signals.rs`）
- `routes::auto_bookmarks`（`crates/attune-server/src/routes/auto_bookmarks.rs`）
- `routes::privacy`（`crates/attune-server/src/routes/privacy.rs`）
- `attune-core::store::browse_signals`、`store::auto_bookmarks`

**测试覆盖**：
- Unit：`store::browse_signals::tests`、`store::auto_bookmarks::tests`
- Integration：`tests/projects_routes_test.rs` 部分
- E2E：❌ 扩展 Playwright 还未在 attune 主仓（在 extension 子模块）；由 extension 自己的 Playwright E2E 覆盖（Python 原型线 42 测试）

**成熟度**：✅ Active。

---

### F-09-FORMFACTOR — 硬件形态分裂（★ v0.6.1）

**能力**：
`HardwareProfile` 上的新轴：`FormFactor` enum（`Laptop` / `K3Appliance` / `Server` / `Unknown`）。检测优先级：(1) `ATTUNE_FORM_FACTOR=k3|laptop|server` env var（K3 镜像 systemd unit 用）；(2) Linux DMI `/sys/class/dmi/id/product_name` 含 "K3" 或 "Jetson" 关键字；(3) 默认 `Laptop`。形态决定 LLM 默认路径：**Laptop / Server / Unknown** → `llm.provider = "openai_compat"`（远端 token，wizard 引导用户填 API key）— 保持 v0.6.0 GA 行为。**K3Appliance** → `llm.provider = "ollama"` + `endpoint: "http://localhost:11434/v1"` + `model: "qwen2.5:3b"`，配合 K3 镜像预装本地 LLM。Wizard Step 3（`Step3LLM.tsx`）从 `/status/diagnostics` 读 `prefers_local_llm`，切换推荐卡（Ollama vs Cloud）+ ★ Recommended marker + 非推荐卡 dashed border。

**代码**：
- `attune-core::platform::FormFactor` + `detect_form_factor()`（`crates/attune-core/src/platform/mod.rs:69-130`）
- `routes::settings::default_settings()`（`crates/attune-server/src/routes/settings.rs:154-180`）
- `routes::status::diagnostics` 暴露 `form_factor` + `prefers_local_llm`
- `ui/src/wizard/Step3LLM.tsx`

**测试覆盖**：
- Unit：`platform::tests::form_factor_default_is_laptop`、`prefers_local_llm_only_for_k3`、`detect_form_factor_respects_env_override`（9 输入）、`form_factor_in_hardware_profile_detect`
- Unit（settings）：`routes::settings::tests::laptop_form_factor_uses_remote_token`、`k3_form_factor_uses_local_ollama`、`server_and_unknown_fallback_to_remote_token`、`non_llm_settings_invariant_across_form_factors`
- Smoke：C.1 计划 — `ATTUNE_FORM_FACTOR=k3 ./attune-server-headless` + curl `/api/v1/status/diagnostics` 返回 `form_factor: "k3"`

**成熟度**：✅ Active（v0.6.1，0 breaking change vs v0.6.0）。

---

### F-10-GOVERNOR — H1 资源治理

**能力**：
每个长跑后台任务（embedding 生成、OCR、ASR、SkillEvolver、向量索引重建、浏览器捕获、RPA 爬虫）通过**任务级资源治理器**运行，三个默认档位：**Conservative**（电池 / 共享机器）、**Balanced**（默认桌面）、**Aggressive**（闲置工作站）。Per-task 限流意味着关键路径查询（chat 实时检索）开绿灯，后台批处理（embedding 队列 / SkillEvolver）开红灯。顶栏永远挂"暂停所有后台任务"按钮。自动 fallback：笔记本电池 → Conservative；CPU > 80% 持续 → 后台任务限速 50%；全屏游戏/演示检测（OS focus）→ 所有后台任务暂停。

**代码**：
- `attune-core::resource_governor`（5 模块：`governor.rs`、`monitor.rs`、`profiles.rs`、`registry.rs`、`mod.rs`）
- Web UI 顶栏 pause 按钮

**测试覆盖**：
- Unit：`resource_governor::governor::tests`、`monitor::tests`、`profiles::tests`、`registry::tests`
- Integration：`tests/governor_integration.rs`

**成熟度**：✅ Active。

---

### F-11-PLUGINS — 插件框架

**能力**：
启动时从 `~/.local/share/attune/plugins/`（或 `%LOCALAPPDATA%\attune\plugins\`）加载插件。每个插件 = `plugin.yaml`（manifest 含 `id`、`name`、`type`、`category`、`version`、`requires.attune_core`、`capabilities[]`、可选 `chat_trigger` 自然语言路由）+ 可选 Rust crate 或纯 prompt。签名 `.attunepkg` 分发（Ed25519）。**OSS attune 不含任何行业插件**（per `oss-pro-strategy.md` v2 决策 2）— `assets/plugins/` 是空的。行业插件（`law-pro`、`presales-pro`、`patent-pro`、`tech-pro`、`medical-pro`、`academic-pro`）住在 `attune-pro` 私有仓。Marketplace toggle UI（W4 E1）让用户 per-vault 启用/禁用插件。

**代码**：
- `attune-core::plugin_loader`（`crates/attune-core/src/plugin_loader.rs`）
- `attune-core::plugin_registry`
- `attune-core::plugin_sig` — Ed25519 签名校验
- `routes::plugins`（`crates/attune-server/src/routes/plugins.rs`）
- `routes::skills`（capability 列表）

**测试覆盖**：
- Unit：`plugin_loader::tests`、`plugin_registry::tests`、`plugin_sig::tests`
- Integration：B.2 计划 — `tests/persona_plugin_integration.rs`（插件注入 Persona）

**成熟度**：✅ Active。v0.7+ 计划：`provides_role` schema 用于行业 Persona 注入。

---

### F-12-PROJECT — Project / Case 通用层

**能力**：
Project 是用户定义的 item 分组（文件、对话、笔记），含可选 metadata（legal Project 子类的 case_no、专利的 application_no、研究课题的 topic_keywords）。**ProjectRecommender** 扫新入库 item，对比已有 Project 的实体匹配，给出"是否属于 Project X？"+ 置信度评分；如果 `chat_trigger.needs_confirm: true`，UI 弹确认浮窗，否则自动归档。跨证据链联想：chat 时检索可 scope 到单 Project，citation 标注同 Project 证据链 item。行业 Project 子类（`legal_case` / `patent_application` / `research_topic`）通过 attune-pro 插件 `extends_project_kind` 注入（v0.7+ 计划）。

**代码**：
- `attune-core::store::project`（`crates/attune-core/src/store/project.rs`）
- `attune-core::project_recommender`
- `routes::projects`（`crates/attune-server/src/routes/projects.rs`）

**测试覆盖**：
- Unit：`project_recommender::tests`、`store::project::tests`
- Integration：`tests/project_recommender_test.rs`、`tests/projects_routes_test.rs`

**成熟度**：🟡 部分 — 通用 Project ✅；`extends_project_kind` 插件扩展点 ❌（v0.7+ 计划）。

---

### F-13-WORKFLOW — Workflow 引擎 + Intent Router

**能力**：
两个协作系统：**Workflow 引擎**运行声明式多步 ops（`find_overlap`、`write_annotation`、`evidence_chain`），定义在插件 yaml 文件中。每步有显式 `needs_confirm` 门（用户批准花 token 或 RPA 动作），输出送到 `workflow.outputs[step_id]` 供下游步骤消费。**Intent Router** 通过插件 `chat_trigger.patterns`（regex）和 `chat_trigger.keywords`（BERT 分类器）匹配自然语言查询到 skill；规则匹配 → 执行 skill；模糊 → fallback 到 RAG chat。Router 是 plugin-aggregated — OSS-only attune 触发列表为空（无行业触发）；attune-pro 插件通过自己的 `chat_trigger` 填充。

**代码**：
- `attune-core::workflow`（`crates/attune-core/src/workflow/mod.rs`）
- `attune-core::intent_router`（`crates/attune-core/src/intent_router.rs`）

**测试覆盖**：
- Unit：`workflow::tests`、`intent_router::tests`
- Integration：`tests/workflow_test.rs`

**成熟度**：🟡 部分 — 引擎 ✅；更丰富的 ops 库和 Intent Router 第三层（LLM fallback）❌ 计划中。

---

### F-14-ENTITIES — 通用 entity extractor

**能力**：
内置 `Person`、`Money`、`Date`、`Organization` extractor — 加上基于 trait 的 `EntityExtractor`，让插件能注册更多。v0.6.0-rc.2 边界瘦身：行业专属 extractor（`CaseNo` 中文法律案号正则）迁到 `attune-pro/plugins/law-pro/extractors/`；OSS 仅含通用类型。Entity 喂 Project recommender（F-12）和 chat citation。

**代码**：
- `attune-core::entities`（`crates/attune-core/src/entities.rs`）
- `attune-core::taxonomy`

**测试覆盖**：
- Unit：`entities::tests`
- Integration：`tests/entities_test.rs`

**成熟度**：✅ Active。Plugin-extensible `extends_entity_kinds` v0.7+ 计划。

---

### F-15-MCP — Python stdio shim MCP 集成

**能力**：
`tools/attune_mcp_shim.py` 是基于 stdio 的 MCP server，包装 attune REST API。MCP 客户端（Claude Desktop、Cline、Continue.dev）可注册 attune 为工具源 — 它们获得检索 / item 取出 / chat session 列表能力，无需写 attune-aware 代码。Shim 通过 `~/.cache/attune-mcp/` 缓存 session token 处理 vault unlock 状态。

**代码**：
- `tools/attune_mcp_shim.py`（Python stdio bridge）
- 规范：`docs/mcp-integration.md`（双语）

**测试覆盖**：
- 人工：`tests/MANUAL_TEST_CHECKLIST.md` MCP 段
- Integration：🟡 未自动化（需要跨语言 harness）

**成熟度**：✅ Active。

---

### F-16-DISTRIBUTION — Tauri 2 桌面 + NAS + 硬件画像

**能力**：
桌面安装器走 Tauri 2 + tauri-plugin-updater（自动更新）。**Windows**：NSIS 推荐（`Attune_0.6.1_x64-setup.exe` ~16 MB）+ MSI 企业部署（~31 MB）。**Linux**：deb（~27 MB）+ AppImage（~94 MB）。**NAS HTTPS 模式**：`--host 0.0.0.0 --tls-cert ... --tls-key ...` 通过 HTTPS + Bearer token auth 暴露 attune — 设计给家用 NAS 自托管，手机浏览器可访问。**HardwareProfile** 启动时自检：CPU vendor/model、NVIDIA GPU（`/dev/nvidia0`）、AMD GPU（`/dev/kfd` + gfx target 如 Radeon 780M 的 gfx1103）、AMD XDNA NPU（Ryzen AI）、Intel NPU、总 RAM、OS、FormFactor（F-09）。检测的 profile 驱动推荐摘要模型 + ROCm `HSA_OVERRIDE_GFX_VERSION` env var 注入。

**代码**：
- `apps/attune-desktop/src/`（Tauri shell）
- `attune-core::platform::HardwareProfile::detect()`
- `routes::status::diagnostics` 暴露 profile

**测试覆盖**：
- Unit：`platform::tests`（15 个测试覆盖 OS 检测、推荐摘要模型、env var 注入、FormFactor — 见 F-09）
- System：`tests/integration_test.rs`、`tests/server_test.rs`
- Smoke：`scripts/smoke-test.sh` 验证二进制 spawn + `/api/v1/status/health` 200 + CORS

**成熟度**：✅ Active on Windows（P0）+ Linux（P1）。macOS 推迟（per CLAUDE.md 平台优先级）。

---

### F-17-PRIVACY — 三层隐私模型 + 跨域防御

**能力**：
两个互补系统。**Phase A.5 三层隐私**：**L0** per-file flag，标记 L0 的 chunk 永不出网（强制本地 LLM）；**L1（默认）** 12 类 PII（身份证 ISO 7064 校验、手机、邮箱、8 家 API key 等）通过正则识别并替换为可逆 `[KIND_N]` placeholder 然后才允许任何云 API 调用，附带出网审计 CSV（合规审查可下载）；**L3（v0.7 目标）** LLM 语义脱敏，Tier T3+/K3 硬件自动启用。**F-Pro 跨域污染防御**：item 有 `corpus_domain` metadata 字段；chunk 前缀 `[领域: legal]` 让检索可施加跨域 penalty（默认 0.4）— 关键词 query intent 检测（零 LLM 调用）决定目标域。这解决共享 vault 问题，"反洗钱" query 不再拉出 Java 算法内容（v0.6.0-rc.5 前的真实 bug）。

**代码**：
- `attune-core::pii`（mod.rs + patterns.rs）
- `routes::privacy`（隐私审计日志下载）
- `routes::audit`
- `attune-core::store::items` `corpus_domain` 字段
- 跨域逻辑在 `attune-core::search`

**测试覆盖**：
- Unit：`pii::patterns::tests`（每 PII 类的正则覆盖 — 50 个测试）
- Integration：`attune-core/tests/pii_chat_path_redact_test.rs`（4 测试，v0.6.2 ✅）—
  验证 ChatEngine 接入 Redactor：user_message 在 LLM 调用前 redact、placeholder 在响应里 restore、
  多种 PII（phone+email+api_key）独立 round-trip、无 PII 消息原样穿过。

**成熟度**：🟡 部分 — **v0.6.2 接入主链路**（cb5baa3 + 本 commit）：
- L0 chunk 隔离：✅ Active
- L1 PII 模块：🟡 **部分接入** —
  - `ChatEngine::run_llm_once` ✅ 在 LLM 调用前 redact `user_message`，
    响应里 restore placeholder（v0.6.2）
  - `outbound_audit` 日志通过 `log::info!` target 输出 ✅
  - **暂未接入**: `history.content`, `knowledge.inject_content/content`
    （需要跨 redact 调用全局 mappings counter 合并 — v0.7+）
  - **暂未接入**: `context_compress` LLM 摘要调用、`ai_annotator`、
    `web_search` query（v0.7+ 单独 patch）
  - 审计日志持久化到 `store::audit_log`（当前仅 log）— v0.7+
- F-Pro 跨域防御：✅ Active
- L3 LLM 脱敏：❌ Designed（v0.7+）

---

### F-18-QUALITY — 质量门

**能力**：
两个互补回归门。**K2 Parse Golden Set**（W3 batch C，2026-04-27）：5 篇 baseline markdown fixture 在 `crates/attune-core/tests/fixtures/parse_corpus/`，`manifest.yaml` 描述每篇的 expected `title_contains`、`min_text_chars`、`must_contain_phrases`、`section_count_min`、`section_paths_must_include`。Harness `parse_golden_set_regression.rs` 强制 baseline 100% 通过率（5 fixture）；扩 200 fixture 时门槛降到 95%（per Readwise Reader 方法论）。**RAGAS 风格 benchmark harness**：`scripts/bench-orchestrator.sh` 跑三赛道检索 benchmark（法律 lawcontrol 语料 / 英文 rust-book / 中文 cs-notes）计算 Hit@10、MRR、Recall@10。v0.6.0 GA 实现三赛道 Hit@10 = 0.80/1.00/1.00。加上 `tests/golden/queries.json` 做 precision@K 回归。

**代码**：
- `crates/attune-core/tests/parse_golden_set_regression.rs`
- `crates/attune-core/tests/rag_quality_benchmark.rs`
- `scripts/bench-orchestrator.sh`、`scripts/run-benchmark-corpus.sh`、`scripts/run-final-eval.py`
- `rust/tests/golden/queries.json`

**测试覆盖**：
- Quality regression：本身就是测试层

**成熟度**：✅ Active。

---

## 3. 跨切关注点

### 3.1 安全模型

- 所有 vault 数据字段级 AES-256-GCM 加密（DEK_db / DEK_idx / DEK_vec 分开）
- Argon2id（64 MB / 3 轮 / 4 线程）— 抗 GPU/ASIC
- Session token：HMAC-SHA256(session_id + expires, MK)，4h TTL
- API key 在 GET 中永不返回（`routes::settings::redact_api_key`）
- CORS allowlist：localhost + Chrome 扩展 origin + 用户配置 origin
- TLS via `rustls`（纯 Rust，无 OpenSSL），`rustls-webpki` 0.103.13（v0.6.1 修了 3 个 RUSTSEC CVE）

### 3.2 国际化

- 双语公开文档：每个 `<NAME>.md` 配 `<NAME>.zh.md`
- Web UI：i18n 字符串走 `t()` 调用（en-US + zh-CN）
- Tantivy CJK 分词 via `tantivy-jieba`
- Embedding via `bge-m3`（多语言）

### 3.3 错误处理

- 所有 `Result<T, VaultError>` 类型化错误；`VaultError` enum 含 `LlmUnavailable`、`Classification`、`IndexCorrupted`、`WrongPassword` 等
- HTTP 响应：4xx 含结构化 `{"error": "...", "hint": "..."}` body；5xx 含通用消息（不泄露内部细节 — 见 `routes::errors::tests::internal_error_response_is_generic`）
- vault-locked 端点返回 403 含 `{"error": "vault is locked", "hint": "POST /api/v1/vault/unlock"}`

### 3.4 可观测性

- `tracing` crate，结构化日志（生产 JSON、开发 pretty）
- `/api/v1/status/diagnostics` 暴露 vault 状态、AI 状态、embedding/classifier readiness、ollama models、硬件画像、form_factor（F-09）
- 出网审计日志（F-17 L1）CSV 可导出供合规审查

---

## 4. 能力 ↔ 测试层覆盖映射

这是 `TESTING.md` 测试金字塔的**反向视图**。每个测试层目前覆盖了哪些能力？

| 能力 | Unit | Integration | System | E2E | Smoke |
|------|:----:|:-----------:|:------:|:---:|:-----:|
| F-01-VAULT | ✅ | ✅ | ✅ | 🟡 | ✅ |
| F-02-RAG | ✅ | ✅ | ✅ corpus | ❌ | 🟡 |
| F-03-CHAT | ✅ | 🟡 | 🟡（B.3 计划） | 🟡（C.3 计划） | ❌ |
| F-04-READER | ✅ | ✅ | 🟡 | 🟡（C.3 计划） | ❌ |
| F-05-COMPRESS | ✅ | ✅ | ❌ | ❌ | ❌ |
| F-06-WEBSEARCH | ✅ | 🟡 | 🟡 人工 | ❌ | ❌ |
| F-07-EVOLUTION | ✅ | ✅ | ❌ | ❌ | ❌ |
| F-08-BROWSEEXT | ✅ | 🟡 | ❌ | ✅（扩展） | ❌ |
| F-09-FORMFACTOR | ✅（8） | ❌ | ❌ | ❌ | 🟡（C.1 计划） |
| F-10-GOVERNOR | ✅ | ✅ | ❌ | ❌ | ❌ |
| F-11-PLUGINS | ✅ | 🟡（B.2 计划） | ❌ | ❌ | ❌ |
| F-12-PROJECT | ✅ | ✅ | ❌ | ❌ | ❌ |
| F-13-WORKFLOW | ✅ | ✅ | ❌ | ❌ | ❌ |
| F-14-ENTITIES | ✅ | ✅ | ❌ | ❌ | ❌ |
| F-15-MCP | ❌ | ❌ | ❌ | ❌ | ❌ 人工 |
| F-16-DISTRIBUTION | ✅ | ✅ | ✅ | ❌ | ✅ |
| F-17-PRIVACY | ✅ | ✅（chat 路径，v0.6.2） | ❌ | ❌ | ❌ |
| F-18-QUALITY | ✅ | ✅ corpus | ✅ | ❌ | ❌ |

**缺口**（驱动 B.1 / B.2 / B.3 / C.1 / C.3 任务定义）：
1. F-03-CHAT 缺 System / E2E 覆盖 — B.3（完整 wizard → chat 流程）+ C.3 #3（citation 跳转）
2. F-09-FORMFACTOR Smoke 还未在 `smoke-test.sh` — C.1
3. F-11-PLUGINS Integration 未端到端跑通 — B.2（persona ↔ plugin）
4. F-17-PRIVACY chat 路径泄露防御未测 — B.2（PII chat 集成）
5. F-15-MCP 全人工 — 自动化需跨语言 harness（推迟 v0.7+）

---

## 5. 不在 OSS attune 中

明确边界（per `oss-pro-strategy.md` v2）：

| 能力 | 住在哪 | 为什么不在 OSS |
|------|--------|--------------|
| 行业插件（law-pro、patent-pro 等） | `attune-pro` 私有仓 | 行业纵向 = 变现层 |
| 行业 Persona（Lawyer/Doctor/PatentAgent） | `attune-pro` 插件包 via `provides_role` | 行业绑定违反 OSS "any individual user" 规则 |
| 行业 entity（CaseNo、鉴定意见、专利号） | `attune-pro/plugins/<vertical>-pro/extractors/` | 同上 |
| Cloud sync、plugin registry、LLM proxy | `attune-pro` 服务层 | 中心化基础设施 |
| 多租户 RBAC、案件分配、多人协作 | `lawcontrol`（独立产品） | B2B 小团队场景 |
| 移动端 app | Roadmap silent | Tauri 2.0 mobile 还未稳定 |

---

## 6. 能力生命周期

新能力**只在代码合入后**才进入本文档。"已设计未实装"spec 住在 `docs/superpowers/specs/`，**不**列在这里。能力：

- ship → 创建 entry，Maturity ✅
- 部分 ship → 🟡 + 显式列出缺什么
- 移除 → entry 删除 + `RELEASE.md` 记录移除
- 迁到 `attune-pro` → 从本文档删除，加到 `attune-pro/docs/specs/`

这条规则避免"P0 已批准 ≠ 代码已实装"漂移（见 memory `feedback_decision_vs_implementation.md`）。

---

## 附录：成熟度图例

- ✅ **Active** — 已发布、所有用户默认开启
- 🟡 **Partial** — 已发布但有 flag、覆盖部分、计划扩展点存在
- ❌ **Designed** — 仅 spec、不在当前二进制；除非显式追踪 roadmap reservation，否则不应出现在本文档

前瞻性 roadmap 见 `RELEASE.md` "What's next" + `oss-pro-strategy.md` §5 六个月路线图。
