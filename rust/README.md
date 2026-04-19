# Attune (Rust production line)

[English](README.md) · [中文](README.zh.md)

**Private AI Knowledge Companion** — Local-first, globally augmented, increasingly attuned to your expertise.

Attune is the Rust production build — a single ~30 MB static binary containing the encrypted knowledge vault, RAG engine, HTTP server with TLS, and embedded Preact UI. No runtime dependencies.

---

## Three pillars

### Active Evolution
Every local miss is silently recorded as a signal. A background `SkillEvolver` periodically sends the accumulated signals to an LLM to generate synonym expansions, which improve future recall without any user intervention.

### Conversational Companion
RAG chat is the primary interaction. Each assistant response carries citation chips that open the original source in a side drawer. Sessions are persisted and searchable — past discussions can be resumed seamlessly.

### Hybrid Intelligence
Local vault first. When the vault has no match, a headless Chrome (or Edge) is driven to search public web content — **no external API keys, no subscription fees**. Every answer carries an explicit "from local" or "from web" label.

---

## Sovereignty & transparency

- **Zero-lock pricing** — you pay only for the software itself + your own LLM tokens (if using cloud providers).
- Argon2id (64 MB, 3 rounds) + AES-256-GCM field-level encryption + Device Secret multi-factor.
- All data held locally; single binary distribution with no runtime dependencies.
- Cross-device migration via encrypted `.vault-profile` export/import.

---

## Core capabilities

### Active Evolution
- Failure signals auto-accumulate
- Background `SkillEvolver` generates synonym expansions every 4 hours (or after 10 signals)
- `learned_expansions` applied silently at search time

### Conversational Companion
- RAG chat with citation chips (local documents / web results)
- Three-stage retrieval: vector (usearch HNSW) + BM25 (tantivy + jieba CJK tokenizer) → rerank → top-k
- Session persistence with cross-session continuity
- HDBSCAN topic clustering

### Hybrid Intelligence
- Hybrid full-text + vector search
- Browser-automated web search (driving a local Chrome/Edge, zero API cost)
- Pluggable embeddings (Ollama / ONNX) and LLMs (Ollama / OpenAI-compatible endpoints)
- Industry plugins (patent / law / tech / presales + runtime user-defined YAML)
- Real-time USPTO patent search (`POST /api/v1/patent/search`)

### Data sovereignty
- Encrypted local storage (Argon2id + AES-256-GCM + Device Secret)
- Single static binary distribution
- NAS mode (`--host 0.0.0.0` + rustls TLS + Bearer token auth)
- Encrypted `.vault-profile` export/import for cross-device migration
- Chrome extension compatible (13 extension endpoints + 40+ REST/WebSocket for UI/CLI)
- **Embedded Preact UI** — single HTML file compiled into the binary via `include_str!`, responsive mobile-friendly

### Annotations & deep reading (Batch A)
- **User annotations**: select text in the Reader → choose from 5 preset tags (⭐ Important / 📍 Deep-dive / 🤔 Question / ❓ Unclear / 🗑 Outdated) with 4 colors + note
- **AI annotations**: "🤖 AI analysis ▾" dropdown with 4 angles (⚠️ Risk / 🕰 Outdated / ⭐ Highlights / 🤔 Questions); local LLM analysis ~15 s per angle, returning precise-offset annotations
- **Annotations influence RAG**: ⭐ Highlights / ⚠️ Risk → ×1.5 boost · 🤔 Questions → ×1.2 · 🗑 / 🕰 Outdated → excluded
- Annotation content is AES-256-GCM encrypted; cascades on item soft-delete

### Context compression & cost transparency (Batch B)
- **Context compression**: retrieved chunks are compressed via local LLM to ~150-char summaries (economical, default) or 300-char+head (accurate) before being sent to the chat LLM — 70–85% cloud token reduction
- **Summary cache**: keyed by `sha256(chunk_text)`, persisted + encrypted — one-time cost, reused forever
- **Token Chip**: live input-token + cloud-$ estimator next to the chat input (green "Local" / amber "$ Cloud"); click to expand the detailed breakdown (candidates / injected / boost / dropped / strategy / cache hits / original chars)

### Cost & trigger contract (key design principle)

| Tier | Cost | Trigger policy |
|------|------|---------------|
| 🆓 **Free** (CPU, ms) | Parse, tokenize, BM25/tantivy, OCR, annotation weighting, summary cache hits | Always on |
| ⚡ **Local compute** (GPU/NPU, seconds) | Embedding generation, basic classify, first-time summary | Background during ingest, pausable from top bar |
| 💰 **Time & money** (LLM, s-min) | Chat, AI annotation analysis, deep analysis, cloud APIs | **User-triggered only**, never background |

### Hardware-aware defaults (Batch 1)

At startup, Attune detects CPU / RAM / GPU / NPU and recommends a local summary model:

| RAM / accelerator | Default summary model |
|-------------------|-----------------------|
| ≥32 GB + accelerator | `qwen2.5:7b` |
| 16–32 GB | `qwen2.5:3b` |
| 8–16 GB | `qwen2.5:1.5b` |
| <8 GB | `llama3.2:1b` |

### Scanned PDF OCR fallback (Batch 1)
When `pdf_extract` returns empty or too-little text, Attune automatically runs OCR via `tesseract` + `pdftoppm` (Chinese + English). One-shot install: `scripts/install-ocr-deps.sh` (apt/dnf/pacman/brew).

### Front-end UX (Batch 1–2, rewritten in Preact)
- Two-column chat-first layout (ChatGPT-like), sidebar collapsible to 64 px icon bar
- Global top bar: logo + 🔒 lock + 👤 account menu (settings / export profile / export device secret / lock)
- Settings modal in ChatGPT/Gemini style: tabs for General / AI / Data / Privacy / About
- Model chip at chat header (🟢 Local / 🔵 Cloud) — tap to switch or jump to settings
- `⌘K` global command palette (views + sessions + items)

---

## Target users

| User | Primary value |
|------|--------------|
| **Lawyers / Patent agents** | Accumulate cases, precedents, and technical disclosures privately; law/patent plugins; portable when changing firms |
| **Researchers / Academics** | Conversational cross-topic retrieval, citations traceable to source paragraphs |
| **Independent consultants / Analysts** | Industry plugins + local + web hybrid retrieval, reuse methodologies across projects |
| **AI power users / Prosumers** | Private version of AI memory: local encryption + pluggable LLMs + self-hosted |

See the [product positioning design](../docs/superpowers/specs/2026-04-17-product-positioning-design.md) for detailed scenarios.

---

## Quick start

### 1. Build

```bash
cd rust
cargo build --release
# Artifacts:
# target/release/attune         (CLI, 4.2 MB)
# target/release/attune-server  (HTTP server, ~30 MB)
```

### 2. Start Ollama (optional, for semantic search)

```bash
curl -fsSL https://ollama.com/install.sh | sh
ollama pull bge-m3
ollama pull qwen2.5:3b
```

Without Ollama, Attune falls back to tantivy BM25 full-text search.

### 3. CLI mode

```bash
./target/release/attune setup              # First run: set master password
./target/release/attune unlock             # Unlock vault
./target/release/attune insert -t "Title" -c "Content"
./target/release/attune list -l 20
./target/release/attune status             # JSON status
./target/release/attune lock
```

### 4. HTTP server mode

```bash
./target/release/attune-server --port 18900
# Browser: http://localhost:18900/
# The first-time wizard will walk you through setup
```

### 5. NAS mode (remote HTTPS + auth)

```bash
# Self-signed cert
openssl req -x509 -newkey rsa:2048 \
  -keyout key.pem -out cert.pem \
  -days 365 -nodes -subj "/CN=your-nas.local"

# Start HTTPS + Bearer auth
./target/release/attune-server \
  --host 0.0.0.0 \
  --port 18900 \
  --tls-cert cert.pem \
  --tls-key key.pem
```

---

## Security model

### Key hierarchy

```
Master Password (user memorized)  +  Device Secret (256-bit random, on disk)
                │                       │
                └───────────┬───────────┘
                            ↓
                Argon2id(m=64MB, t=3, p=4)
                → 32-byte Master Key (MK)
                            │
                    ┌───────┼────────┐
                    ↓       ↓        ↓
                  DEK_db  DEK_idx  DEK_vec
```

- **Master Password** — user-memorized, never persisted
- **Device Secret** — 256-bit random, generated at setup in `{config_dir}/device.key` (permissions 0600); exportable for multi-device use
- **Argon2id parameters** — 64 MB memory, 3 iterations, 4 threads, resisting GPU/ASIC brute force
- **Three DEKs** — separately encrypting SQLite data, tantivy full-text index, and usearch vectors. Password change re-encrypts only the 96-byte DEKs, not the underlying business data.

### Field-level encryption

| Field | Encryption | Rationale |
|-------|-----------|-----------|
| `id`, `created_at`, `source_type`, `url`, `domain` | Plaintext | Filtering does not require unlock |
| `title` | Plaintext | Allow item listing in LOCKED state |
| `content`, `tags`, `metadata` | AES-256-GCM (DEK_db) | Core sensitive data |
| tantivy index | In-memory (DEK_idx reserved) | Full-text index ≈ plaintext |
| usearch vectors | File-level encryption (DEK_vec) | Vectors can reverse the source |

Every encrypted field uses an independent 96-bit random nonce, stored as `nonce(12B) ‖ ciphertext ‖ tag(16B)`.

---

## API endpoints

All endpoints are prefixed with `/api/v1/`. Localhost access is auth-free; remote requires Bearer token. Disable with `--no-auth`.

### Vault management

| Method | Path | Description |
|--------|------|-------------|
| GET | `/vault/status` | Vault state (sealed/locked/unlocked) + item count |
| POST | `/vault/setup` | First-time password setup |
| POST | `/vault/unlock` | Unlock vault, returns session token |
| POST | `/vault/lock` | Manual lock (zero in-memory keys) |
| POST | `/vault/change-password` | Change master password |
| GET | `/vault/device-secret/export` | Export device secret (for migration) |
| POST | `/vault/device-secret/import` | Import device secret (new device) |

### Knowledge

| Method | Path | Description |
|--------|------|-------------|
| POST | `/ingest` | Text ingest (plain JSON) |
| POST | `/upload` | File upload (multipart) |
| GET | `/search?q=&top_k=` | Hybrid search (BM25 + vector + RRF) |
| GET/PATCH/DELETE | `/items[/{id}]` | Item CRUD |

### Chat & sessions

| Method | Path | Description |
|--------|------|-------------|
| POST | `/chat` | RAG conversation |
| GET | `/chat/sessions` | List sessions |
| GET/DELETE | `/chat/sessions/{id}` | Session history / delete |

### Annotations

| Method | Path | Description |
|--------|------|-------------|
| POST | `/annotations` | Create user annotation |
| GET | `/annotations?item_id=` | List annotations for an item |
| PATCH/DELETE | `/annotations/{id}` | Edit / delete annotation |
| POST | `/annotations/ai` | AI analysis (angle: risk / outdated / highlights / questions) |

### System

| Method | Path | Description |
|--------|------|-------------|
| GET | `/status/health` | Health check (no auth) |
| GET | `/status/diagnostics` | Full diagnostics incl. hardware + Ollama status |
| GET/PATCH | `/settings` | Get / update settings |
| POST | `/llm/test` | Test a cloud LLM endpoint |
| POST | `/models/pull` | Background Ollama model pull |

### Web UI

| Path | Description |
|------|-------------|
| GET `/` | Embedded Preact SPA (single HTML with inline JS + CSS) |

---

## Data storage

| Data | Linux | Windows |
|------|-------|---------|
| Encrypted database | `~/.local/share/attune/vault.db` | `%LOCALAPPDATA%\attune\vault.db` |
| Device Secret | `~/.config/attune/device.key` | `%APPDATA%\attune\device.key` |
| Config | `~/.config/attune/` | `%APPDATA%\attune\` |

**Migrating to a new device:**
1. Backup `vault.db` (encrypted, copy as-is)
2. Export `device.key` via API or direct copy
3. On the new device, install the binary + import `device.key` + `unlock` with the original password

---

## Binaries

| Binary | Size | Purpose |
|--------|------|---------|
| `attune` | 4.2 MB | CLI tool (7 subcommands) |
| `attune-server` | ~30 MB | HTTP API server (TLS + Preact UI + search engine) |

Size breakdown: rustls crypto stack + tantivy full-text + usearch C++ bindings + Tokio + Axum + Preact UI.

---

## Testing

```bash
cargo test --workspace    # 376+ tests
```

---

## Project structure

```
rust/
├── Cargo.toml                    # workspace root
└── crates/
    ├── attune-core/              # lib: crypto / storage / search / scan (24+ modules)
    ├── attune-server/            # bin: Axum HTTP API + embedded Preact UI
    │   └── ui/                   # Preact + Vite frontend (50+ modules)
    │       ├── src/
    │       │   ├── wizard/       # 5-step first-run wizard
    │       │   ├── layout/       # Sidebar + MainShell + DrawerHost
    │       │   ├── views/        # Chat / Items / Remote / Knowledge / Settings
    │       │   ├── components/   # Button / Input / Modal / Drawer / ChatMessage / Reader / CommandPalette ...
    │       │   ├── hooks/        # useChat / useItems / useAnnotations / useRemote / useSettings / useShortcut
    │       │   ├── store/        # signals / api / connection / ws
    │       │   └── i18n/         # core + zh + en
    │       └── dist/index.html   # single-file bundle (committed, referenced by include_str!)
    └── attune-cli/               # bin: command-line tool
```

---

## License

### Open-source core (Apache-2.0)

See repo root [`LICENSE`](../LICENSE). Covers all code in this directory tree.

### Commercial plugins & services (proprietary)

Not in this repository. Available via Attune Pro subscription. See [`NOTICE`](../NOTICE).

### Trademark

"Attune" is a trademark of the Attune Contributors. Forks must remove the "Attune" name from the user-visible UI or obtain a separate trademark license (Apache-2.0 §6 does not grant trademark rights).

### AI output disclaimer

LLM-generated content may be inaccurate, incomplete, or misleading. Attune and its contributors **make no warranty on AI correctness**. Legal, medical, financial, or safety decisions must be independently verified by qualified professionals. See LICENSE §7–§8.
