# Attune

[中文](README.zh.md) · [English](README.md) · [Wiki](https://wiki.your-company.com/attune/) · [Pricing](https://wiki.your-company.com/plans/attune-pricing/)

> 🇨🇳 **中文用户请优先阅读** [README.zh.md](README.zh.md) — 项目文档以中文为主，本英文版为辅。

**Private AI Knowledge Companion** — Local-first, globally augmented, increasingly attuned to your expertise.

Attune is a generic personal AI knowledge base for **any individual knowledge worker** — students, independent developers, researchers, writers, AI power users. Your interests become clearer the more you use it; local knowledge answers first, and the system reaches out to the web only when needed. All data is encrypted on your own device — portable across machines, portable across jobs.

**Industry-vertical users** (lawyers / patent agents / doctors / scholars / sales engineers) install Attune (this OSS) and add the matching `attune-pro/<vertical>-pro` plugin pack — see the three-product matrix below.

## 📥 Download

Latest stable: **server v0.7.0** · **desktop v0.7.0**

### Desktop app (Web UI + system tray) — [desktop-v0.7.0 Release](https://github.com/qiurui144/attune/releases/tag/desktop-v0.7.0)

| Platform | File | Size | Notes |
|----------|------|------|-------|
| Windows | [`Attune_0.7.0_x64-setup.exe`](https://github.com/qiurui144/attune/releases/download/desktop-v0.7.0/Attune_0.7.0_x64-setup.exe) | 17 MB | NSIS installer (recommended) |
| Windows | [`Attune_0.7.0_x64_en-US.msi`](https://github.com/qiurui144/attune/releases/download/desktop-v0.7.0/Attune_0.7.0_x64_en-US.msi) | 33 MB | MSI for enterprise |
| Linux deb | [`Attune_0.7.0_amd64.deb`](https://github.com/qiurui144/attune/releases/download/desktop-v0.7.0/Attune_0.7.0_amd64.deb) | 29 MB | Debian/Ubuntu |
| Linux rpm | [`Attune-0.7.0-1.x86_64.rpm`](https://github.com/qiurui144/attune/releases/download/desktop-v0.7.0/Attune-0.7.0-1.x86_64.rpm) | 29 MB | RHEL/Fedora |
| Linux AppImage | [`Attune_0.7.0_amd64.AppImage`](https://github.com/qiurui144/attune/releases/download/desktop-v0.7.0/Attune_0.7.0_amd64.AppImage) | 97 MB | Generic Linux |

### Server / CLI binaries (headless / NAS / server) — [v0.7.0 Release](https://github.com/qiurui144/attune/releases/tag/v0.7.0)

| Platform | File |
|----------|------|
| Linux x86_64 | [`attune-linux-x86_64.tar.gz`](https://github.com/qiurui144/attune/releases/download/v0.7.0/attune-linux-x86_64.tar.gz) |
| Linux ARM64 | [`attune-linux-aarch64.tar.gz`](https://github.com/qiurui144/attune/releases/download/v0.7.0/attune-linux-aarch64.tar.gz) |
| macOS Apple Silicon | [`attune-macos-aarch64.tar.gz`](https://github.com/qiurui144/attune/releases/download/v0.7.0/attune-macos-aarch64.tar.gz) |
| Windows x86_64 | [`attune-windows-x86_64.zip`](https://github.com/qiurui144/attune/releases/download/v0.7.0/attune-windows-x86_64.zip) |

> macOS Intel: build from source with `cargo build --release` (Apple Silicon already covers modern Mac users). SHA256 checksum file ships with each archive.

## v0.7 sprint highlights (2026-05-15) — Memory Moat Phase A+B

> **"优势不在于模型，而在于以安全有效的记忆"** — 同样的 LLM，挂上 attune 比单跑模型答得更准、更敢用。

### Phase A — 文档编辑嵌入功能完全有效（修 3 个 release-blocker）

之前 `update_item` 仅刷 SQL → 搜索永远返回旧内容；同名重传重复 embed；delete 不清向量。本 sprint 用 **`attune-core::reindex` 协调模块**收敛三资源（DB / VectorIndex / FulltextIndex / embed_queue）事务式 cleanup，同步增加：

- `items.content_hash` 列（SHA-256 hex）+ migration → update / upload 短路省 ~3s/100KB embedding
- `reindex_queue` 表 + `AppState::start_reindex_worker`（3s 轮询）→ 解锁 `scanner` / `scanner_webdav` 等无法直接持锁的后台 worker
- `routes/items.rs::update_item` 返回 `UpdateOutcome` 三态（existed / content_changed / backfilled_hash），仅真改才触发 reindex

### Phase B — 自学习闭环 3 hook

`skill_signals` 加 `kind` + `ref_id` 列，5 类信号汇入：

| Hook | kind | 写入位点 |
|---|---|---|
| 1 | `doc_create` / `doc_update` / `doc_delete` | upload / items.update / items.delete / scanner |
| 2 | `citation_hit` | chat.rs 取 top-5 引用 chunk 喂入 |
| 3 | `annotation_marker` | annotations.rs::create_annotation |

skill_evolution 从"失败驱动"升级为"全谱信号驱动" — 可按 kind 设阈值 / 调权。

### Phase C spec

`docs/specs/memory-moat-v07.md` — 文档版本化 / 编辑触发重标注 / 失败信号反推 project / 衰减曲线 / embed_model_version 迁移工具链，RICE 排序后留 sprint 2+。

📊 **5 agents 并行交付的 v0.7 缺口模块**（commit 71d82ee）：cost / tools / demo / query_rewrite / entity_graph / skill_eval / report / reader / capture(email+telegram) / sync(webdav) / vlm + 4 个 server 路由（audit log + log.csv + demo load + chat stream）

🧪 **验证**：workspace lib tests **910 passed / 0 failed / 1 ignored**；MANUAL_TEST_CHECKLIST 新增 8 条 Memory Moat 验收。

---

## v0.6.0-rc.5 highlights (2026-04-28)

🎯 **Three-track PRO benchmark** — verified end-to-end RAG quality on legal + general English + Chinese fundamentals:

| Scenario | Hit@10 | MRR | Verdict |
|----------|--------|-----|---------|
| 法律 / lawcontrol corpus | **0.80** | 0.50 | ✅ PRO |
| Rust / rust-book | **1.00** | **1.00** | ✅ PRO 满分 |
| 中文八股 / cs-notes | **1.00** | **1.00** | ✅ PRO 满分 |
| **5-dim answer quality (lawcontrol golden_qa)** | **25.00/25** (100%) | 10/10 excellent | ✅ +39% vs baseline |

🔒 **Phase A.5 — Three-layer privacy model**:
- **L0 🔒**: per-file flag, chunk never leaves device (forced local LLM)
- **L1 default**: 12 PII classes (id-card with ISO 7064 / phone / email / 8 API key vendors / etc.) with reversible `[KIND_N]` placeholders + outbound audit log (CSV exportable for compliance)
- **L3** (v0.7): LLM-based semantic redaction on Tier T3+/K3 hardware

🌐 **F-Pro — Cross-domain pollution defense**:
- `items.corpus_domain` metadata + `[领域: legal]` chunk prefix + cross-domain penalty (0.4) + keyword query intent detection (zero LLM call)
- Logical domain isolation on shared vault — no more "反洗钱" pulling Java algorithm docs

📋 **Evidence flow end-to-end**: chat citations now include real `breadcrumb` (chapter path), `chunk_offset_start/end` (Reader deep-link target), and `confidence` (1-5, parsed from LLM strict-prompt marker).

Reproduce: `bash scripts/bench-orchestrator.sh all && python3 scripts/run-final-eval.py`. Full benchmark methodology in [`docs/benchmarks/dual-track-baseline.md`](docs/benchmarks/dual-track-baseline.md), release notes in [`docs/v0.6-release-notes.md`](docs/v0.6-release-notes.md).

---

## Two product lines

This repository contains two parallel product lines sharing the Chrome extension protocol (`/api/v1/*`):

| Line | Path | Purpose |
|------|------|---------|
| **Python prototype** | `python/src/attune_python/` | Fast iteration for algorithms and experimental features. FastAPI + ChromaDB + SQLite FTS5 |
| **Rust production** | [`rust/`](rust/README.md) | Production-grade generic personal knowledge base. Axum + rusqlite + tantivy + usearch + Preact UI |

Validated Python features get promoted to the Rust line. See [`rust/README.md`](rust/README.md) for the full Rust documentation.

---

## Three-product matrix (where Attune fits)

> Decisive positioning (2026-04-27): Attune (this repo, OSS) is a **generic personal knowledge base**, with **zero industry binding**. Industry depth (law / medical / academic / sales / engineering / patent) is delivered as commercial plugin packs in `attune-pro`. Small-team B2B law-firm scenarios are handled by a separate product, `lawcontrol`.

| Product | License | Form | User group |
|---------|---------|------|------------|
| **`attune`** (this repo) | Apache-2.0 | Tauri desktop / Chrome extension | **Personal generic users** — universal RAG, encrypted vault, browser capture, MCP outlet |
| **`attune-pro`** (private) | Proprietary | Plugin packs (.attunepkg signed) loaded into `attune` | **Personal industry users** — law / presales / patent / tech / medical / academic vertical packs |
| **`lawcontrol`** (separate product) | Proprietary | Django + Vue B2B SaaS | **Law-firm small teams** — multi-tenant RBAC + case assignment + multi-user collaboration |

**Equation:**
- Personal generic user = `attune (OSS)`
- Personal industry user = `attune (OSS)` + `attune-pro/<vertical>-pro` plugin pack
- Industry small team = `lawcontrol`

The three products are technically independent (no cross-product runtime dependency) and strategically complementary (same team, distinct user segments). Full strategy + admission rules: [`docs/oss-pro-strategy.md`](docs/oss-pro-strategy.md) (bilingual).

---

## Three pillars (Rust line)

### Active Evolution
It learns from every query without configuration. Local misses become signals; a background `SkillEvolver` periodically asks an LLM to generate synonym expansions that silently improve recall over time. After three months, the same query returns noticeably more relevant results — without any "retrain" button.

### Conversational Companion
RAG Chat is the primary interface. Every answer carries clickable citation chips that open the original source in a side drawer. Sessions persist and search across time — discussions from three weeks ago continue right where they left off.

### Hybrid Intelligence
Local knowledge first. When the local vault has no match, a headless Chrome (or Edge) automatically browses the web — **no API keys, no subscription**. Every answer explicitly labels its origin: local or web. Your professional data stays encrypted at home; public information is fetched live.

---

## Sovereignty & transparency

- **Zero-lock pricing**: you pay only for the software itself + your own LLM tokens (if you choose cloud models). No middleman, no search-API subscription, no hidden fees.
- Argon2id(64MB/3r) + AES-256-GCM field-level encryption + Device Secret multi-factor, all data held locally.
- Single ~30 MB static Rust binary — zero runtime dependencies.
- Cross-device migration via encrypted `.vault-profile` export/import.

---

## Who it's for

**OSS attune** is for **any individual knowledge worker**, regardless of field:

| User | Primary value |
|------|--------------|
| **Students / Independent developers** | Personal RAG over reading notes, code repos, blog drafts; sessions persist across topics |
| **Researchers / Writers** | Conversational retrieval across topics, citations traceable to source paragraphs |
| **AI power users / Prosumers** | A private version of AI memory: local encryption + pluggable LLM + self-hosted |
| **Knowledge workers in any domain** | Universal vault + browse capture + auto-bookmark + cross-session continuity |

**Industry users** — install OSS attune **and** the matching `attune-pro/<vertical>-pro` plugin pack:

| Vertical | Adds (via attune-pro) |
|----------|----------------------|
| **Lawyers** | `law-pro`: contract review / risk matrix / drafting / OA reply / clause lookup + CaseNo extractor + Case dossier workflow |
| **Patent agents** | `patent-pro` (M3+): FTO search workflow + USPTO/EPO automation + patent-number extractor |
| **Sales / Presales** | `presales-pro`: competitor analysis / BANT scoring / quotes / demo scripts |
| **Doctors / Scholars** | `medical-pro` / `academic-pro` (planned): medical terminology / case templates; citation graphs / paper-writing assistant |

---

## Quick start

### 5 steps from download to first use

1. **Download** the binary from the [Releases](../../releases) page (or `cargo build --release` from source — see below).
2. **Run** `./attune-server-headless --host 127.0.0.1 --port 18900` (Linux) or double-click `attune-server-headless.exe` (Windows). The first launch creates `~/.local/share/attune/` (or `%LOCALAPPDATA%\attune\`).
3. **Open** `http://localhost:18900/` in your browser. The first-run wizard appears automatically.
4. **Set Master Password** + pick an LLM backend on step 3 (see "AI model platforms" table below for `base_url` / model / pricing). API key is stored encrypted with your master password.
5. **Bind data** in the wizard's last step: drop a file, point at a folder, or skip and use the Items / Reader UI later.

That's it. The Cmd+K palette jumps between Chat, Items, Reader, Sessions, and Settings. Lock the vault from the top bar at any time.

### Rust production line (build from source)

```bash
cd rust
cargo build --release
./target/release/attune-server-headless --host 127.0.0.1 --port 18900
```

Full documentation: [`rust/README.md`](rust/README.md).

### Python prototype

```bash
cd python
python -m venv .venv && source .venv/bin/activate
pip install -e ".[dev]"
uvicorn attune_python.main:app --reload --port 18900
```

---

## AI model platforms

Attune speaks the **OpenAI-compatible chat protocol**, so you can plug in any provider that exposes `/v1/chat/completions`. The Settings → AI tab includes a "Quick preset" dropdown that pre-fills `endpoint` + `model` for the providers below — you only paste the API key.

| Provider | base_url | Recommended model | Price (input)* | Get a key |
|----------|----------|-------------------|----------------|-----------|
| **DeepSeek** | `https://api.deepseek.com/v1` | `deepseek-chat` | ¥1 / M tok | [platform.deepseek.com](https://platform.deepseek.com/api_keys) |
| **Aliyun Qwen** (DashScope) | `https://dashscope.aliyuncs.com/compatible-mode/v1` | `qwen-plus` | ¥4 / M tok | [bailian.console.aliyun.com](https://bailian.console.aliyun.com/?apiKey=1) |
| **Zhipu GLM** | `https://open.bigmodel.cn/api/paas/v4` | `glm-4-plus` | ¥50 / M tok | [open.bigmodel.cn](https://open.bigmodel.cn/usercenter/apikeys) |
| **Moonshot Kimi** | `https://api.moonshot.cn/v1` | `moonshot-v1-8k` | ¥12 / M tok | [platform.moonshot.cn](https://platform.moonshot.cn/console/api-keys) |
| **Baichuan** | `https://api.baichuan-ai.com/v1` | `Baichuan4-Turbo` | ¥15 / M tok | [platform.baichuan-ai.com](https://platform.baichuan-ai.com/console/apikey) |
| **Ollama (local)** | `http://localhost:11434/v1` | `qwen2.5:7b` | free / local | `curl -fsSL https://ollama.com/install.sh \| sh && ollama pull qwen2.5:7b` |
| **OpenAI** | `https://api.openai.com/v1` | `gpt-4o-mini` | ~¥3 / M tok | [platform.openai.com](https://platform.openai.com/api-keys) |

*Pricing is the input-token rate at the time of writing. Check each provider's pricing page for current rates and output-token rates.

**Recommendation**: DeepSeek for daily use (cheapest non-local), Ollama if you have a 16 GB+ GPU, OpenAI when you need maximum quality.

---

## Skill development (free + Pro, same mechanism)

A *skill* is a small YAML + prompt bundle that Attune auto-suggests when your chat message matches its keywords or regex. Both the free build and Pro share the same loader — Pro just preinstalls more skills. **You never edit YAML through the UI; you write or download skills, drop them in the plugins folder, then toggle them in Settings → Skills.**

**1. Create the directory**

```
~/.local/share/attune/plugins/<plugin-id>/
```

(Windows: `%APPDATA%\attune\plugins\<plugin-id>\`)

**2. Write `plugin.yaml`**

```yaml
id: my-plugin/contract-quick-review
name: 快速合同审查
type: skill
version: "0.1.0"
description: 30-second triage of contract risks

chat_trigger:
  enabled: true            # plugin author can ship disabled-by-default
  needs_confirm: true      # show user a confirm prompt before running
  priority: 5              # higher wins when multiple skills match
  patterns:
    - '帮我.*审查.*合同'      # any matching pattern fires
  keywords: ['审查合同', '合同风险', 'contract review']
  min_keyword_match: 1     # how many keyword hits required
  exclude_patterns: ['起草']  # vetoes the match if hit
  requires_document: true  # only fire when chat has a pending file
```

**3. Write `prompt.md`** — the actual LLM prompt loaded when the skill runs.

**4. Restart Attune** so the plugin registry rescans the folder.

**5. Open Settings → Skills tab.** Your skill appears with its keywords; toggle it on/off without touching YAML again.

Distributing skills to others: zip the folder as `<plugin-id>.attunepkg` — recipients drop it into the same plugins folder. Pro skills (legal / sales / academic packs) ship through the same path; the only difference is they come pre-installed.

---

## Features at a glance (Rust line)

- **First-run wizard**: Welcome · Master Password · LLM backend (local Ollama / cloud API / demo) · Hardware detection with model recommendations · First data binding
- **Chat**: RAG with citation chips · session history · typing-stream rendering · Token Chip cost estimator (local free / cloud $ live)
- **Reader + Annotations**: full-text reading with 5 preset tags × 4 colors, plus AI 4-angle analysis (risk / outdated / highlights / questions)
- **Items**: search, source-type filter, delete · drawer-based reading
- **Remote directories**: bind local folders or WebDAV with credentials
- **Settings**: theme (light / dark / auto) · language (zh / en) · LLM config · export `.vault-profile`
- **Cmd+K global palette**: jump between views, sessions, and items
- **Stability**: connection state machine · retry matrix · WebSocket auto-reconnect
- **Plugin architecture**: Ed25519-signed YAML plugins (community + commercial tracks)

---

## 用户形态与默认底座

> 面向**非专业用户**（非应用开发者），默认开箱即用，不需要任何技术配置。唯一暴露给用户的"配置"是 plugin（开源标准 MCP / skill / agents）。
> 产品 = **Tauri 桌面应用窗口**（Windows / Linux / 未来 macOS）。Web UI 仅用于服务器端 API 调试，**不是产品 UI**。

### 默认底座（随二进制打包，hidden 不暴露）

| 能力 | 默认引擎 | 用户可改？ |
|------|---------|---------|
| Embedding | bge-m3 | ❌ |
| Reranker | bge-reranker-v2-m3 | ❌ |
| OCR | PP-OCRv5 | ❌（但可选**场景预设**，不暴露引擎） |
| ASR | whisper-large-v3-turbo（中文 WER 5-7%） | ❌ |
| 数据目录 | `~/.local/share/attune`（Linux）/ `%APPDATA%\attune`（Win） | ❌ |

### LLM 大模型 — 主云端 + 统一 OpenAI 兼容协议

**所有 LLM 调用统一走 OpenAI 兼容协议**（`POST /v1/chat/completions`），不论后端是云端 OpenAI / DeepSeek / 智谱 / 通义 / Anthropic 兼容代理 / Ollama 本地。attune 不为每个 provider 写独立 SDK — 一个 OpenAI client 走天下。

**默认不打包本地 LLM**：
- 普通免费用户：自己配云端大模型 API key（在应用窗口设置面板）
- 付费用户：云端 gateway 自动下发（用户不持 raw key）
- 本地 LLM（可选）：Ollama 自行装（`docs/local-llm-setup.md`），同样走 OpenAI 兼容 endpoint `http://127.0.0.1:11434/v1`

**多模态支持**（per OpenAI Vision API）：
- 文件（PDF / DOCX / TXT / 代码）：attune 自动 OCR/解析 → 拼到 user message 文本
- 图片（PNG / JPG / WEBP）：走 OpenAI vision `content array`，支持 base64 data URI 或 https URL，需模型支持 vision（gpt-4o / claude-3.5-sonnet / qwen-vl-max / ...）；非 vision 模型自动 drop + log warning

### 用户形态

| 形态 | 标识 | 网络要求 | LLM 来源 |
|------|------|---------|---------|
| **离线 self-host** | LoggedOut | 永不联网；仅 RAG / 搜索可用；LLM Chat 需配自己的云端 API 或自装本地 LLM (Ollama) | 自配（云 / 本地） |
| **免费会员** | Free（云端账号） | 注册/登录 + Chat 时联网 | **自己配云端大模型 API key**（默认） |
| **付费会员** | Paid（云端 license） | Chat 时联网；30 天 license 离线缓存 | **云端 gateway 自动**（Pro 高级模型，用户不持 raw key） |

### 用户可配置项（应用窗口暴露的仅 6 项）

| 项 | 离线 | 免费 | 付费 |
|----|:----:|:----:|:----:|
| **vault 主密码**（改密码） | ✏️ | ✏️ | ✏️ |
| **本地知识库目录关联**（隐私自管） | ✏️ | ✏️ | ✏️ |
| **云端大模型**（普通用户自己 API key） | ✏️ | ✏️ | 🔒（云端 gateway 下发） |
| **plugin 装载**（社区 / 开发者本地） | ✏️ | ✏️ | 🔒（云端按 license 自动 sync） |
| **plugin 卸载** | ✏️ | ✏️ | 🔒（防误删 pro plugin） |
| **OCR 场景预设**（合同/票据/截图...） | ✏️ | ✏️ | ✏️ |

**OCR 场景预设**：4 个内置 `contract`（合同/法律 300dpi）/ `receipt`（票据 200dpi）/ `screenshot`（截图 200dpi）/ `ancient`（古籍 600dpi）；用户可自建任意数量自定义 profile。CLI: `attune ocr-profile-{list,show,create,delete}`。

### 形态切换

- **离线 → 免费会员**：应用窗口 → 我的账号 → 登录/注册，写 `~/.config/npu-vault/license.json`（free license code），设备绑定 1:2 自动生效
- **免费 → 付费**：应用窗口 → 我的账号 → 升级 → 跳到 accounts.attune.ai 付款；付款后云端自动 sync pro plugins + 云端大模型自动接入

### 一键安装

| 平台 | 包格式 | 内含 |
|------|-------|------|
| Linux | AppImage（单文件）+ deb（apt 包） | attune binary + 底座模型（embedding/reranker/OCR/ASR）+ poppler-utils |
| Windows | MSI installer | 同上 + Windows runtime |
| macOS（未来） | dmg + brew tap | 同上 |

**不包含**：Ollama 本地 LLM（用户需用时自行装，见 `docs/local-llm-setup.md`）。默认 attune 走云端大模型，无需 Ollama 也能完整使用。

---

## 代码模块视角（开发者用）

> 一份完整的代码功能清单，用于代码 review / 文档审计 / 测试覆盖核查。每条 feature 含 ID / 模块 / 测试覆盖。

### attune-core 核心模块

| ID | 模块 | 主要 API | 测试 |
|----|------|---------|------|
| **C-VAULT** | `vault.rs` | setup / unlock / lock / dek_db / change_password / 设备 secret | unit |
| **C-CRYPTO** | `crypto.rs` | Argon2id 派生 / AES-256-GCM / 字段加密 / zeroize | unit |
| **C-STORE** | `store.rs` | rusqlite + 字段级加密 / item CRUD / FTS5 队列 | unit + integration |
| **C-CHUNKER** | `chunker.rs` | 滑窗分块 + 章节切割 | unit |
| **C-PARSER** | `parser.rs` | PDF/DOCX/MD/code 解析 + bytes 入口 + `parse_file_with_profile` / `parse_bytes_with_profile`（传 OCR profile_id） | unit + integration |
| **C-EMBED** | `embed.rs` | Ollama / ONNX / openai_compat embedding provider | unit + ignored e2e |
| **C-LLM** | `llm.rs` | LlmProvider trait（chat / chat_with_history / **chat_multimodal**）+ OpenAI compat + Ollama + Attachment (Image/TextFile) | unit + 3 multimodal + ignored e2e |
| **C-CHAT** | `chat.rs` | ChatEngine / Citation / confidence parse | unit + integration |
| **C-CLUSTER** | `clusterer.rs` | HDBSCAN 聚类 | unit |
| **C-CLASSIFIER** | `classifier.rs` | LLM 文档分类 | unit |
| **C-INDEX** | `index.rs` | tantivy + usearch | unit + integration |
| **C-OCR** | `ocr/` | PP-OCRv5 + pdftoppm + extract_text_from_pdf + `_with_dpi` | unit + ignored e2e |
| **C-OCR-PROFILE** | `ocr/profile.rs` + `profile_registry.rs` | OcrProfile + 4 builtin + 持久化 CRUD + `dpi_for_profile` | 17 unit |
| **C-ASR** | `asr.rs` | whisper.cpp subprocess | unit |
| **C-WORKFLOW** | `workflow.rs` | YAML workflow + 事件触发 | unit + integration |

### Plugin 协议层 (v2)

| ID | 模块 | 功能 | 测试 |
|----|------|------|------|
| **P-LOADER** | `plugin_loader.rs` | PluginManifest v2（pricing/resources/registers_case_kinds/skills/agents/mcps/ui） | unit + integration |
| **P-LOADER-ENC** | `plugin_loader::from_dir_with_key` | 自动识别 plugin.yaml.enc 解密装载 | integration |
| **P-REGISTRY** | `plugin_registry.rs` | scan + 5 查询 API（skills/agents/mcps/case_kind/chat_trigger） | 19 unit + 10 generic_plugins_test |
| **P-REG-CHAT** | `plugin_registry::match_chat_trigger` | regex/keywords 匹配 + priority + exclude_patterns | 5 unit |
| **P-SIG** | `plugin_sig.rs` | Ed25519 keygen / sign / verify_loose / verify_strict / verify_with_key | 14 unit |
| **P-ENC** | `plugin_encryption.rs` | Argon2id + AES-GCM yaml 加密 + trust↔pricing 联动校验 | 7 unit |
| **P-DISPATCH** | `capability_dispatch.rs` | subprocess + timeout + exit_code (0/2/-1) | 8 unit |
| **P-RUNNER** | `agent_runner.rs` | run_agent_subprocess + format_for_chat | 5 unit |
| **P-SYNC** | `plugin_sync.rs` | 拉云端 entitled_plugins → download → verify → install | 7 unit |

### Skill / Agent / MCP 三角色

| ID | 模块 | 功能 | 测试 |
|----|------|------|------|
| **S-DATE** | `skills/parse_chinese_date.rs` | 中文日期 → ISO 8601（含中文数字大写） | 13 unit |
| **S-ENTITY** | `skills/extract_entities.rs` | 人名 / 日期 / 金额 / 地点 / 组织（纯规则） | 11 unit |
| **S-CLASS** | `skills/classify_chunk_kind.rs` | 8 类 chunk 分类 | 10 unit |
| **S-SUM** | `skills/summarize_text.rs` | LLM 摘要 + summarize_document_set | 6 unit |
| **A-CLASS** | `agents/document_classifier.rs` | 编排 3 skill → ClassifiedEvidence | 6 unit + e2e |
| **A-TRAIT** | `agents/mod.rs::Agent` | trait + AgentOutput<T>（computation/audit_trail/red_lines/missing/followups/confidence） | unit |
| **MCP-CLIENT** | `mcp_client.rs` | stdio JSON-RPC + 心跳 + 重启 + id 路由 + transaction lock | 7 unit |

### 案件库 / 设备 / 会员 / License

| ID | 模块 | 功能 | 测试 |
|----|------|------|------|
| **CASE-META** | `case_metadata.rs` | CaseMetadata + Party + classified_evidence 持久化 | 4 unit |
| **DEV-BIND** | `device_binding.rs` | DeviceFingerprint + License + 1:2 状态机 | 5 unit |
| **DEV-CLIENT** | `accounts_client.rs` | HTTP client → cloud accounts | 3 unit |
| **CLOUD-CLIENT** | `cloud_client.rs` | login/signup/me/list_licenses（FastAPI）+ cookie 自动管理 | 4 unit |
| **LICENSE** | `license.rs` | LicenseClaims + Ed25519 签名 + base64 code + 离线校验 | 9 unit |
| **MEMBER** | `member_session.rs` | MemberState 3 档（LoggedOut/Free/Paid）+ SettingsLocks 6 字段 | 6 unit |
| **LIC-CACHE** | `license_cache.rs` | ~/.config/npu-vault/license.json 持久化 (chmod 600) | 5 unit |

### attune-server 路由 / attune-cli 子命令

参见 `DEVELOP.md` 「路由清单」与 `attune-cli --help`。完整子命令包括 vault setup/unlock/lock/status、plugin-{keygen,sign,verify,encrypt,decrypt,install,list,uninstall}、login、sync-plugins、link-folder、ocr、ocr-profile-{list,show,create,delete}、deploy。

### Tauri 桌面 app（apps/attune-desktop）

| ID | 模块 | 功能 |
|----|------|------|
| **TAURI-EMBED** | `main.rs::spawn_embedded_server()` | 子进程启动 `attune-server-headless --port 18900`，主窗口打开 `http://127.0.0.1:18900` |
| **TAURI-TRAY** | `main.rs` | 系统托盘图标 + 菜单（Show / Hide / Quit），单实例检测 |
| **TAURI-DROP** | `main.rs` | FileDrop 事件 → emit `attune-file-drop` 到前端 WebView |
| **TAURI-UPLOAD** | `main.rs::upload_dropped_paths` | Tauri command：读取本地文件路径 → multipart POST `/api/v1/upload`（reqwest 0.12 rustls-tls） |

### 测试金字塔与日志栈

```
        E2E (Playwright + 真集成)
       ─────────────────────────
      Integration (跨模块, ~30 tests)
     ─────────────────────────────────
    Unit (单模块, ~734 tests in attune-core)
   ──────────────────────────────────────
  Smoke (CLI 冒烟 7, Server 冒烟 N)
```

技术栈基础库（开源高可用）：`tracing` + `tracing-subscriber`（结构化日志）/ `thiserror`（类型化错误）/ `axum` + `tower-http`（HTTP）/ `argon2` + `aes-gcm` + `ed25519-dalek`（audited cryptography）/ `rusqlite` bundled（跨平台 SQLite）/ `tantivy` + `tantivy-jieba`（全文搜索）/ `usearch`（HNSW 向量）。

### 已知约束

- attune (OSS) **不内置任何行业 agent** — `civil_loan_agent` 等在 attune-pro
- paid plugin yaml 加密载入需要 `ATTUNE_PLUGIN_KEY` env（设备 license token）
- OCR / LLM 走 subprocess / HTTP，不直接 link C++
- Web UI vite bundle 不在本仓 build，dist/ checked in
- 跨平台：Linux/Win/macOS — aarch64（K3 一体机）交叉编译

---

## Hardware support

Automatic chip-level detection for recommending the best local model:

| RAM / Accelerator | Recommended summary model |
|-------------------|---------------------------|
| ≥32 GB + dGPU/NPU | `qwen2.5:7b` (~1 s/chunk) |
| 16–32 GB + iGPU/NPU | `qwen2.5:3b` (~2 s/chunk) |
| 8–16 GB + iGPU | `qwen2.5:1.5b` (~3 s/chunk) |
| <8 GB, CPU only | `llama3.2:1b` (~5 s/chunk) |

Intel Meteor/Lunar/Arrow Lake NPU, AMD Phoenix/Hawk Point/Strix Point NPU, and NVIDIA/AMD GPUs are auto-detected; Ollama is the default inference backend with ROCm / CUDA / Metal / CPU support.

---

## License

### Open-source core

**Apache License 2.0** — see [LICENSE](LICENSE). Covers:
- `rust/crates/*` (attune-core / attune-server / attune-cli)
- `extension/` (Chrome extension)
- `rust/crates/attune-server/ui/` (Preact UI)
- `python/src/attune_python/` (Python prototype)
- `plugins/free/*` (free community plugins: tech, patent, presales baseline)

Free to fork, modify, and use commercially. Apache-2.0 includes a patent grant (§3).

### Commercial plugins & services (proprietary)

Not in this repository. Available via Attune Pro subscription:
- Law plugin (contract review / clause library / drafting assistant)
- Presales Pro (competitive comparison / BANT / quotes)
- Cloud backup / multi-device sync
- Official plugin registry with signing keys
- Hosted LLM proxy

See [NOTICE](NOTICE) for details.

### AI output disclaimer

LLM-generated content may be inaccurate, incomplete, or misleading. Attune and its contributors **make no warranty on AI correctness**. Legal, medical, financial, or safety decisions must be independently verified by qualified professionals. See LICENSE §7–§8.

---

## 致谢 / Acknowledgements

Attune is built on the shoulders of outstanding open-source projects. We are grateful to their authors and contributors.

**后端框架 / Backend**

- [Axum](https://github.com/tokio-rs/axum) — ergonomic async web framework built on Tokio (MIT)
- [Tokio](https://github.com/tokio-rs/tokio) — the async runtime powering the entire server (MIT)
- [tower-http](https://github.com/tower-rs/tower-http) — HTTP middleware utilities (CORS, tracing) (MIT)
- [axum-server](https://github.com/programatik29/axum-server) — TLS integration for Axum via rustls (MIT)
- [FastAPI](https://github.com/fastapi/fastapi) + [Uvicorn](https://github.com/encode/uvicorn) — Python prototype line HTTP layer (MIT)

**数据库与搜索 / Storage & Search**

- [rusqlite](https://github.com/rusqlite/rusqlite) — SQLite bindings with bundled SQLite for zero-dependency deployment (MIT)
- [tantivy](https://github.com/quickwit-oss/tantivy) + [tantivy-jieba](https://github.com/meilisearch/tantivy-jieba) — full-text search engine with Chinese word segmentation (MIT)
- [usearch](https://github.com/unum-cloud/usearch) — high-performance HNSW vector index (Apache-2.0)
- [hdbscan](https://github.com/genbio-ai/hdbscan) — density-based clustering for automatic topic grouping (MIT)
- [ChromaDB](https://github.com/chroma-core/chroma) — vector store used in the Python prototype (Apache-2.0)
- [SQLAlchemy](https://github.com/sqlalchemy/sqlalchemy) + [Alembic](https://github.com/sqlalchemy/alembic) — ORM and migrations for the pluginhub backend (MIT)

**加密与安全 / Cryptography**

- [argon2](https://github.com/RustCrypto/password-hashes/tree/master/argon2) — Argon2id key derivation for master password hashing (MIT / Apache-2.0)
- [aes-gcm](https://github.com/RustCrypto/AEADs/tree/master/aes-gcm) — AES-256-GCM authenticated encryption for vault fields (MIT / Apache-2.0)
- [zeroize](https://github.com/RustCrypto/utils/tree/master/zeroize) — secure zeroing of secrets from memory (MIT / Apache-2.0)
- [rustls](https://github.com/rustls/rustls) — pure-Rust TLS stack; zero system OpenSSL dependency (MIT / Apache-2.0 / ISC)
- [ed25519-dalek](https://github.com/dalek-cryptography/curve25519-dalek/tree/main/ed25519-dalek) — Ed25519 signatures for plugin package verification (MIT / Apache-2.0)

**本地 AI 底座 / Local AI**

- [Ollama](https://github.com/ollama/ollama) — local LLM runtime (embedding, chat, rerank); recommended backend (MIT)
- [ONNX Runtime](https://github.com/microsoft/onnxruntime) (`ort`) — cross-platform inference engine for OCR and embedding models (MIT)
- [kreuzberg-paddle-ocr](https://github.com/Goldziher/kreuzberg) — PP-OCRv5 bindings via ONNX Runtime for in-process document OCR (MIT)
- [whisper.cpp](https://github.com/ggerganov/whisper.cpp) — fast on-device ASR; bundled binary in desktop packages (MIT)
- [HuggingFace Hub](https://github.com/huggingface/hf-hub) (`hf-hub`) — model weight fetching from the HF registry (Apache-2.0)

**文档解析 / Document Parsing**

- [PyMuPDF](https://github.com/pymupdf/PyMuPDF) — PDF rendering and text extraction for the Python line (AGPL-3.0 / commercial)
- [pdf-extract](https://github.com/jrmuizel/pdf-extract) — pure-Rust PDF text extraction (MIT)
- [python-docx](https://github.com/python-openxml/python-docx) — .docx reading in the Python prototype (MIT)
- [calamine](https://github.com/tafia/calamine) — Excel / ODS spreadsheet parsing (MIT / Apache-2.0)

**网络与协议 / Networking**

- [reqwest](https://github.com/seanmonstar/reqwest) — HTTP client (Ollama API, web fetch, WebDAV) (MIT / Apache-2.0)
- [reqwest_dav](https://github.com/niuhuan/reqwest_dav) — WebDAV client built on reqwest (MIT)
- [async-imap](https://github.com/async-email/async-imap) — async IMAP email ingestion (Apache-2.0 / MIT)

**前端 / Frontend**

- [Preact](https://github.com/preactjs/preact) — lightweight React-compatible UI library powering the Chrome extension (MIT)
- [Vite](https://github.com/vitejs/vite) — fast frontend build tooling (MIT)

**打包与工具 / Packaging & Tooling**

- [serde](https://github.com/serde-rs/serde) + [serde_json](https://github.com/serde-rs/json) — ubiquitous Rust serialization framework (MIT / Apache-2.0)
- [clap](https://github.com/clap-rs/clap) — CLI argument parsing (MIT / Apache-2.0)
- [tracing](https://github.com/tokio-rs/tracing) — structured application-level logging (MIT)
- [jieba](https://github.com/fxsjy/jieba) — Chinese tokenizer used in FTS5 pipeline (MIT)

## Contributing

Contribution guidelines are still being drafted. For now, see [DEVELOP.md](DEVELOP.md) for branch model + build commands, and [NOTICE](NOTICE) for third-party attribution.

## Documentation

- [Memory Moat v0.7 spec](docs/specs/memory-moat-v07.md)
- [v0.7 gap analysis](docs/v07-gap-analysis.md)
- [Product positioning design](docs/superpowers/specs/2026-04-17-product-positioning-design.md)
- [Frontend redesign spec](docs/superpowers/specs/2026-04-19-frontend-redesign-design.md)
- [UX quality infrastructure](docs/superpowers/specs/2026-04-19-ux-quality-design.md)
- [Data infrastructure](docs/superpowers/specs/2026-04-19-data-infrastructure-design.md)
- [Distribution & compliance](docs/superpowers/specs/2026-04-19-distribution-compliance-design.md)
