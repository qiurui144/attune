# Attune (Rust production line)

[English](README.md) · [中文](README.zh.md)

**Private AI Knowledge Companion** — Local-first, globally augmented, increasingly attuned to your expertise.

Attune is a generic personal AI knowledge base for **any individual knowledge worker** (OSS Apache-2.0, zero industry binding). Industry users (lawyers / doctors / patent agents / scholars) load the corresponding `attune-pro/<vertical>-pro` plugin pack for vertical capabilities — see [`docs/oss-pro-strategy.md`](../docs/oss-pro-strategy.md).

This directory is the Rust production build — a single ~47 MB stripped binary (59 MB unstripped, x86_64-linux release) containing the encrypted vault, RAG engine, HTTP server with TLS, and embedded Preact UI. No runtime dependencies.

---

## What's in this directory

This is the Rust production source. The three product pillars (**Active Evolution** / **Conversational Companion** / **Hybrid Intelligence**) are introduced at a high level in the [top-level README](../README.md#three-pillars-rust-line); their concrete implementation details live below in the [Core capabilities](#core-capabilities) section, followed by the security model, API endpoints, build instructions, and testing matrix.

For the OSS × attune-pro feature boundary, see [`docs/oss-pro-strategy.md`](../docs/oss-pro-strategy.md).

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

### Multi-Layer Memory (token-efficient context)
- Tiered memory — L0 raw chunks · L1 chunk summaries · L2 episodic memory · L3 semantic (topic) memory
- Tier-aware context assembler routes each query to the cheapest tier that answers it:
  recall/overview questions answer from compact L2/L3 summaries, precise lookups stay on L0
- Coverage gate guarantees no regression — weak memory hits fall back to the raw-chunk path
- Rolling history compaction — overflowed conversation turns roll into one cached summary
- Measured ~79% median reduction in injected-knowledge tokens on recall/overview queries

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
When `pdf_extract` returns empty or too-little text, Attune automatically runs OCR via **PP-OCRv5 mobile** (DBNet+CRNN+CLS, ~21 MB ONNX, Chinese accuracy 94–96%) + `pdftoppm`. Tesseract removed since v0.6.x (replaced by PP-OCR as single engine). One-shot install: `scripts/install-ocr-deps.sh` (apt/dnf/pacman/brew).

### Office Helper (v0.7.1 — OCR structured extraction + ASR transcription)

Standalone `/api/v1/office/*` endpoints expose OCR/ASR as **office-assistant tools** (results are NOT auto-saved to vault — user explicitly chooses to save).

Five **structured OCR scenes** (zero-LLM, regex anchors + bbox proximity + GB/Luhn checksums):

| Scene | Fields | Accuracy red line | Speed (p50) |
|-------|--------|-------------------|-------------|
| `document` | title + reading-order blocks (title/paragraph/list/figure_caption/footer) | char-level ≥ 92% | A4 ≤ 3s |
| `receipt` | invoice_no, issue_date, seller, buyer, amount_total, tax_amount, amount_chinese | field-level ≥ 92% | ≤ 2s |
| `table` | headers + 2D cells + row_count + column_count | cell ≥ 92% | A4 ≤ 4s |
| `card` | name, company, job_title, phone, email, address | field-level ≥ 92% | ≤ 1.5s |
| `id_card` (3 subtypes: `id_card_cn` / `bank_card` / `business_license`) | per-subtype schema with GB 11643 / Luhn / GB 32100-2015 check digits | field-level ≥ 95% | ≤ 2s |

**Async ASR transcription** (`whisper.cpp small Q8` default, RTF ≤ 0.5 on CPU): submit returns `{job_id, ws_url}`, WebSocket pushes progress every 500 ms, supports speaker diarization (pyannote.audio subprocess). WER red lines: Chinese ≤ 15%, English ≤ 10%, mixed ≤ 18%, DER ≤ 25%.

Endpoints:
```
POST   /api/v1/office/ocr            sync, multipart, schema-tagged response
POST   /api/v1/office/transcribe     async, returns job_id
GET    /api/v1/office/jobs/{job_id}  poll job state + result
DELETE /api/v1/office/jobs/{job_id}  cancel (409 if done/failed/cancelled)
WS     /api/v1/office/jobs/ws        progress push every 500ms
```

CLI:
```
attune ocr <image> [--profile receipt] [--id-card-subtype id_card_cn] [--json]
attune transcribe <audio> [--diarization] [--json]
```

Spec: [`docs/superpowers/specs/2026-05-20-office-helper-design.md`](../docs/superpowers/specs/2026-05-20-office-helper-design.md). Golden dataset + L1/L2 gate testing: [`crates/attune-server/tests/golden/office/README.md`](crates/attune-server/tests/golden/office/README.md).

### Multi-format ingestion

Attune parses 20+ file types natively. Unknown binary formats (video, archives, executables) are rejected with 422 rather than being silently treated as text. Remote ingest sources are also supported: **WebDAV** remote directories (ETag-based incremental sync) and **Email IMAP** (email bodies + document attachments auto-indexed, UID-based incremental sync).

| Category | Formats |
|----------|---------|
| Document | `.md` `.txt` `.pdf` `.docx` `.epub` `.rtf` |
| Web / Presentation | `.html` `.htm` `.pptx` |
| Spreadsheet / Data | `.xlsx` `.xls` `.csv` |
| Image → OCR | `.png` `.jpg` `.jpeg` `.webp` `.bmp` `.tiff` `.tif` `.gif` |
| Audio → ASR | `.mp3` `.wav` `.m4a` `.flac` `.ogg` `.aac` `.opus` `.wma` |
| Code / Config | `.py` `.js` `.ts` `.rs` `.go` `.java` `.c` `.cpp` `.h` `.rb` `.php` `.swift` `.kt` `.sh` `.toml` `.yaml` `.json` `.xml` and more |

OCR uses 7 built-in scene profiles (contract / receipt / screenshot / ancient / table / form / card). Upload images or PDFs to `/api/v1/upload?ocr_profile=receipt` to activate a specific profile.

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
# target/release/attune-server-headless  (HTTP server, ~47 MB stripped)
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
./target/release/attune-server-headless --port 18900
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
./target/release/attune-server-headless \
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
| POST | `/vault/setup` | First-time password setup — response includes `recovery_key` (one-time, save offline) |
| POST | `/vault/unlock` | Unlock vault, returns session token |
| POST | `/vault/lock` | Manual lock (zero in-memory keys) |
| POST | `/vault/change-password` | Change master password |
| POST | `/vault/reset-with-recovery-key` | Reset password using recovery key — data preserved |
| POST | `/vault/forgot-password-reset` | Last-resort destructive reset (requires `"confirm":"RESET"`) |
| GET | `/vault/device-secret/export` | Export device secret (for migration) |
| POST | `/vault/device-secret/import` | Import device secret (new device) |

### Knowledge

| Method | Path | Description |
|--------|------|-------------|
| POST | `/ingest` | Text ingest (plain JSON) |
| POST | `/upload` | File upload (multipart) |
| GET | `/search?q=&top_k=` | Hybrid search (BM25 + vector + RRF) |
| GET/PATCH/DELETE | `/items[/{id}]` | Item CRUD |

### Projects (Sprint 1, see spec §2.1)

| Method | Path | Description |
|--------|------|-------------|
| GET/POST | `/projects` | List / create Project (Case dossier) |
| GET/PATCH/DELETE | `/projects/{id}` | Project CRUD |
| GET/POST/DELETE | `/projects/{id}/files` | File assignment to Project |
| GET | `/projects/{id}/timeline` | Case timeline (events + evidence) |

### Workflow Engine (Sprint 1 Phase C, see spec §3.3)

- Built-in `law-pro/evidence_chain_inference` workflow runs automatically when a file is uploaded **and** assigned to a Project (after the user accepts the recommender's suggestion from Phase B).
- 4 steps: extract entities (skill, mocked) → cross_reference (deterministic, SQL) → inference (skill, mocked) → write_annotation (stub, Sprint 2 wires vault DEK).
- WS push: `{"type": "workflow_complete", "workflow_id": "...", "file_id": "...", "project_id": "..."}` after run.
- Sprint 2 will wire real LLM via Intent Router and externalize the workflow yaml to attune-law plugin.

### Plugin Loader (Sprint 2 Phase A)

attune-server scans `~/.local/share/attune/plugins/` at startup. Each subdirectory:
- `plugin.yaml` declares the plugin (id / name / type / version)
- `workflows/*.yaml` declares workflows (parsed via attune-core schema)
- `capabilities/<cap_id>/plugin.yaml` declares nested skills

Trigger registry: file_added events match plugin workflows where `trigger.on == 'file_added'`,
each spawn-and-run with workflow_complete pushed via WebSocket.

attune-pro `.attunepkg` bundles unpack into this dir. Empty plugin dir = no-op (no workflows fire).

### UI Notifications (Sprint 1 Phase D)

WebSocket `/ws/scan-progress` now multiplexes three message types:
- `progress` — embedding queue / classifier counts (existing)
- `project_recommendation` — file_uploaded triggers candidate list with overlap score; chat keyword triggers hint suggestion
- `workflow_complete` — Sprint 2 plugin-registered workflows complete; banner toast top-right

The frontend renders a bottom-right RecommendationOverlay (accept/dismiss) and reuses Toast for workflow completion.

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
| `attune-server-headless` | ~47 MB stripped | HTTP API server (TLS + Preact UI + search engine) |

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
    │       │   ├── hooks/        # useChat / useItems / useAnnotations / useRemote / useSettings / useOcrProfiles / useMember / useFolderLinks / useShortcut
    │       │   ├── store/        # signals / api (get/post/patch/put/delete) / connection / ws
    │       │   └── i18n/         # core + zh + en
    │       └── dist/index.html   # single-file bundle (committed, referenced by include_str!)
    └── attune-cli/               # bin: command-line tool
```

---

## Desktop Distribution

Attune ships in two forms (same Rust backend code):

| Form | Binary | Use Case |
|------|--------|----------|
| **Attune Desktop** | `apps/attune-desktop` (Tauri 2 shell) | Laptop users — double-click MSI/deb, native window + tray + drag-drop |
| **Attune Server** (headless) | `crates/attune-server/bin/headless.rs` (`attune-server-headless`) | K3 appliance / NAS / server — `attune-server-headless --host 0.0.0.0 ...` |

### Build (local)

```bash
# Linux
cd apps/attune-desktop
cargo install --locked tauri-cli --version "^2.0"
(cd ../../rust/crates/attune-server/ui && npm ci && npm run build)
cargo tauri build --bundles deb,appimage

# Windows (run on Windows host)
cargo tauri build --bundles nsis,msi
```

Output: `target/release/bundle/{deb,appimage,nsis,msi}/`.

### Auto-update

Desktop checks `https://updates.engi-stack.com/desktop/{target}/{version}/latest.json`
30 seconds after launch. Updates are minisign-signed; pubkey embedded in binary.
See `docs/superpowers/specs/2026-04-25-industry-attune-design.md` §6.6 for full design.

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
