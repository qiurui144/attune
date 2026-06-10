# Attune

[中文](README.zh.md) · [English](README.md) · [Wiki](https://wiki.your-company.com/attune/) · [Pricing](https://wiki.your-company.com/plans/attune-pricing/)

> 🇨🇳 **中文用户请优先阅读** [README.zh.md](README.zh.md) — 项目文档以中文为主，本英文版为辅。

**Private AI Knowledge Companion** — Local-first, globally augmented, increasingly attuned to your expertise.

Attune is a generic personal AI knowledge base for **any individual knowledge worker** — students, independent developers, researchers, writers, AI power users. Your interests become clearer the more you use it; local knowledge answers first, and the system reaches out to the web only when needed. All data is encrypted on your own device — portable across machines, portable across jobs.

**Industry-vertical users** (lawyers / patent agents / doctors / scholars / sales engineers) install Attune (this OSS) and add the matching `attune-pro/<vertical>-pro` plugin pack — see the three-product matrix below.

## 📥 Download

Latest stable: **server v1.2.0** · **desktop v1.2.0** — released 2026-06-01: GitConnector + WASM cross-platform agent distribution + one-click dependency deploy. Build on top of **v1.1.0 (Agent Control Plane / ACP)** and **v1.0 GA**.
See [`rust/RELEASE.md`](rust/RELEASE.md) for the full changelog (version SSOT).

Grab the binaries from the [Releases page](https://github.com/qiurui144/attune/releases) — pick the latest `vX.Y.Z` (server/CLI tarballs) and `desktop-vX.Y.Z` (Tauri installers) tag. Each archive ships with a SHA256 checksum.

| Track | Tag prefix | Artifacts |
|-------|-----------|-----------|
| **Desktop app** (Web UI + system tray) | `desktop-vX.Y.Z` | Windows NSIS `.exe` / MSI · Linux `.deb` / `.rpm` / `.AppImage` |
| **Server / CLI** (headless / NAS / server) | `vX.Y.Z` | `attune-linux-x86_64.tar.gz` · `attune-linux-aarch64.tar.gz` · `attune-macos-aarch64.tar.gz` · `attune-windows-x86_64.zip` |

> macOS Intel: build from source with `cargo build --release` (Apple Silicon already covers modern Mac users).

### Package managers (recommended for v1.0+)

From **v1.0.0** onwards, Attune ships through native package managers with auto-update wired in:

```bash
# Windows
winget install qiurui144.Attune

# Debian / Ubuntu
curl -fsSL https://qiurui144.github.io/attune/attune-archive-keyring.gpg | sudo tee /usr/share/keyrings/attune-archive-keyring.gpg > /dev/null
echo "deb [signed-by=/usr/share/keyrings/attune-archive-keyring.gpg] https://qiurui144.github.io/attune/apt stable main" | sudo tee /etc/apt/sources.list.d/attune.list
sudo apt update && sudo apt install attune

# RHEL / Fedora / openSUSE
sudo curl -fsSL -o /etc/yum.repos.d/attune.repo https://qiurui144.github.io/attune/rpm/attune.repo
sudo dnf install attune
```

The Tauri desktop app also has a **built-in auto-updater** — once installed, new versions arrive in-app without you touching a CLI. Full install guide and troubleshooting: [`docs/INSTALL.md`](docs/INSTALL.md).

## What's new since v1.0 GA (current: v1.2.0)

The v1.0→v1.2 line layers production-grade governance and cross-platform reach onto the GA core. Full notes in [`rust/RELEASE.md`](rust/RELEASE.md); the per-module × tech-stack map lives in [`rust/DEVELOP.md` → 能力矩阵 × 技术栈选型](rust/DEVELOP.md#能力矩阵--技术栈选型).

- **Agent Control Plane (ACP, v1.1.0)** — central agent registry + typed handoffs, declarative DAG flow executor, per-`agent×model` failure telemetry, cost-aware scheduler, and a workspace-level quality gate (ratchet-only thresholds). Governs the whole agent ecosystem as one engineering org.
- **Cross-platform agent distribution (WASM, v1.2.0)** — deterministic agents/skills compile to `wasm32-wasip1` and run via an embedded `wasmtime`; one `.attunepkg` with one `.wasm` runs on Windows / Linux / riscv64. The WASM-safe `attune-agent-sdk` leaf crate keeps the `Agent` trait free of native deps.
- **GitConnector (v1.2.0)** — import a knowledge base directly from a Git repo (GitHub / GitLab / Gitea / Bitbucket / Codeberg / sr.ht over HTTPS): clone → glob filter → ingest → follow upstream commits. Local-only, zero-LLM import path, with SSRF protection.
- **Privacy OutboundGate + `PrivacyTier::L0` "never leaves device"** — every network egress (LLM / Cloud / WebDAV / Web Search / Telemetry) is funneled through one gate that consults settings + PII redaction; L0-tagged content refuses any cloud LLM call.
- **One-click dependency deploy** — Ollama readiness detection + in-app install/pull, base-model auto-ensure, and LM Studio endpoint auto-detect, so non-technical users never touch a terminal.

> **v1.0 GA (2026-05-25)** delivered the Office Helper (OCR scenes + card/ID checksums + whisper.cpp transcription), 4 OSS deterministic/heuristic agents, a real-LLM verification gate, and the Agent 验证铁律 6-category floor. Per-version notes — including v0.7 Memory Moat and the v0.6 RAG-quality benchmarks — live in [`rust/RELEASE.md`](rust/RELEASE.md) (version SSOT); the benchmark methodology is in [`docs/benchmarks/dual-track-baseline.md`](docs/benchmarks/dual-track-baseline.md).

## Two product lines

This repository contains two parallel product lines sharing the Chrome extension protocol (`/api/v1/*`):

| Line | Path | Purpose |
|------|------|---------|
| **Python prototype** | `python/src/attune_python/` | Fast iteration for algorithms and experimental features. FastAPI + ChromaDB + SQLite FTS5 |
| **Rust production** | [`rust/`](rust/README.md) | Production-grade generic personal knowledge base. Axum + rusqlite + tantivy + usearch + Preact UI |

Validated Python features get promoted to the Rust line. See [`rust/README.md`](rust/README.md) for the full Rust documentation.

---

## Three-product matrix (where Attune fits)

> Decisive positioning (2026-04-27): Attune (this repo, OSS) is a **generic personal knowledge base**, with **zero industry binding**. Industry depth (law / medical / academic / sales / engineering / patent) is delivered as commercial plugin packs in `attune-pro`. Small-team B2B law-firm scenarios are handled by a separate product, `attune-enterprise`.

| Product | License | Form | User group |
|---------|---------|------|------------|
| **`attune`** (this repo) | Apache-2.0 | Tauri desktop / Chrome extension | **Personal generic users** — universal RAG, encrypted vault, browser capture, MCP outlet |
| **`attune-pro`** (private) | Proprietary | Plugin packs (.attunepkg signed) loaded into `attune` | **Personal industry users** — law / presales / patent / tech / medical / academic vertical packs |
| **`attune-enterprise`** (separate product) | Proprietary | Django + Vue B2B SaaS | **Law-firm small teams** — multi-tenant RBAC + case assignment + multi-user collaboration |

**Equation:**
- Personal generic user = `attune (OSS)`
- Personal industry user = `attune (OSS)` + `attune-pro/<vertical>-pro` plugin pack
- Industry small team = `attune-enterprise`

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
- **免费 → 付费**：应用窗口 → 我的账号 → 升级 → 跳到 accounts.engi-stack.com 付款；付款后云端自动 sync pro plugins + 云端大模型自动接入

### 一键安装

| 平台 | 包格式 | 内含 |
|------|-------|------|
| Linux | AppImage（单文件）+ deb（apt 包） | attune binary + 底座模型（embedding/reranker/OCR/ASR）+ poppler-utils |
| Windows | MSI installer | 同上 + Windows runtime |
| macOS（未来） | dmg + brew tap | 同上 |

**不包含**：Ollama 本地 LLM（用户需用时自行装，见 `docs/local-llm-setup.md`）。默认 attune 走云端大模型，无需 Ollama 也能完整使用。

---

## 代码模块视角（开发者用）

> 面向开发者的 **能力 × 实现模块 × 技术栈 × 选型理由** 完整矩阵已收敛到
> [`rust/DEVELOP.md` → 能力矩阵 × 技术栈选型](rust/DEVELOP.md#能力矩阵--技术栈选型)（基于
> develop 实时代码：5 crate / 60+ `attune-core` 模块 / 40+ route 文件），并在那里持续维护，
> 避免 README 与代码漂移。crate 布局、路由清单、启动序列、加密/搜索/采集深挖节也都在
> `rust/DEVELOP.md`。
>
> 边界提醒：attune (OSS) **不内置任何行业 agent**（`civil_loan_agent` 等在 attune-pro）；
> OCR / LLM / ASR 走 subprocess / HTTP，核心逻辑不直接 link C++；Web UI vite bundle 的
> `dist/` checked in（不在本仓 build）。

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

- [Install guide](docs/INSTALL.md) · [Testing guide](docs/TESTING.md) · [Deployment guide](docs/DEPLOY.md)
- [Developer guide & capability × tech-stack matrix](rust/DEVELOP.md)
- [OSS × Pro strategy](docs/oss-pro-strategy.md) (bilingual)
- [Memory Moat v0.7 spec](docs/superpowers/specs/2026-05-19-memory-moat-v07.md)
- [Product positioning design](docs/superpowers/specs/2026-04-17-product-positioning-design.md)
- [Frontend redesign spec](docs/superpowers/specs/2026-04-19-frontend-redesign-design.md)
- [UX quality infrastructure](docs/superpowers/specs/2026-04-19-ux-quality-design.md)
- [Data infrastructure](docs/superpowers/specs/2026-04-19-data-infrastructure-design.md)
- [Distribution & compliance](docs/superpowers/specs/2026-04-19-distribution-compliance-design.md)
