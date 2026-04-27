# Attune

[English](README.md) · [中文](README.zh.md)

**Private AI Knowledge Companion** — Local-first, globally augmented, increasingly attuned to your expertise.

Attune is a personal AI knowledge base designed for knowledge-intensive professionals (lawyers, patent agents, researchers, consultants). Your professional domain becomes clearer the more you use it; local knowledge answers first, and the system reaches out to the web only when needed. All data is encrypted on your own device — portable across machines, portable across jobs.

---

## Two product lines

This repository contains two parallel product lines sharing the Chrome extension protocol (`/api/v1/*`):

| Line | Path | Purpose |
|------|------|---------|
| **Python prototype** | `src/npu_webhook/` | Fast iteration for algorithms and experimental features. FastAPI + ChromaDB + SQLite FTS5 |
| **Rust production** | [`rust/`](rust/README.md) | Production-grade Private AI Knowledge Companion. Axum + rusqlite + tantivy + usearch + Preact UI |

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

| User | Primary value |
|------|--------------|
| **Lawyers / Patent agents** | Accumulate cases, precedents, and technical disclosures privately; law/patent industry plugins; bring your vault when you change firms |
| **Researchers / Academics** | Conversational retrieval across topics, citations traceable to source paragraphs |
| **Independent consultants / Analysts** | Industry plugins + local + web hybrid retrieval, reuse methodologies across projects |
| **AI power users / Prosumers** | A private version of AI memory: local encryption + pluggable LLM + self-hosted |

---

## Quick start

### 5 steps from download to first use

1. **Download** the binary from the [Releases](../../releases) page (or `cargo build --release` from source — see below).
2. **Run** `./attune-server --host 127.0.0.1 --port 18900` (Linux) or double-click `attune-server.exe` (Windows). The first launch creates `~/.local/share/attune/` (or `%LOCALAPPDATA%\attune\`).
3. **Open** `http://localhost:18900/` in your browser. The first-run wizard appears automatically.
4. **Set Master Password** + pick an LLM backend on step 3 (see "AI model platforms" table below for `base_url` / model / pricing). API key is stored encrypted with your master password.
5. **Bind data** in the wizard's last step: drop a file, point at a folder, or skip and use the Items / Reader UI later.

That's it. The Cmd+K palette jumps between Chat, Items, Reader, Sessions, and Settings. Lock the vault from the top bar at any time.

### Rust production line (build from source)

```bash
cd rust
cargo build --release
./target/release/attune-server --host 127.0.0.1 --port 18900
```

Full documentation: [`rust/README.md`](rust/README.md).

### Python prototype

```bash
python -m venv .venv && source .venv/bin/activate
pip install -e ".[dev]"
uvicorn npu_webhook.main:app --reload --port 18900
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
- `src/npu_webhook/` (Python prototype)
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

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) (TBD) and [NOTICE](NOTICE).

## Documentation

- [Product positioning design](docs/superpowers/specs/2026-04-17-product-positioning-design.md)
- [Frontend redesign spec](docs/superpowers/specs/2026-04-19-frontend-redesign-design.md)
- [UX quality infrastructure](docs/superpowers/specs/2026-04-19-ux-quality-design.md)
- [Data infrastructure](docs/superpowers/specs/2026-04-19-data-infrastructure-design.md)
- [Distribution & compliance](docs/superpowers/specs/2026-04-19-distribution-compliance-design.md)
