# Attune Feature Matrix (v0.6.1)

> **Status**: Living document — kept in sync with each release.
> **Audience**: Contributors, plugin developers, QA, new readers onboarding to attune.
> **Companion**: [`TESTING.md`](TESTING.md) (test pyramid + capability coverage), [`oss-pro-strategy.md`](oss-pro-strategy.md) (OSS × Pro boundary).
> **Bilingual**: [中文版](FEATURES.zh.md).

---

## 0. How to read this document

Each capability has a stable **ID** (`F-{nn}-{TOPIC}`) so commit messages, test cases, and PR descriptions can reference them unambiguously.

Each entry has 4 fixed fields:

- **Capability** — what users / clients see
- **Code** — primary module(s) where the logic lives (`crate::path` + key files)
- **Test Coverage** — which test files cover this capability, mapped to test pyramid layers (Unit / Integration / System / E2E / Smoke). See [`TESTING.md`](TESTING.md) for layer definitions.
- **Maturity** — ✅ **Active** (shipped, default-on) / 🟡 **Partial** (shipped, behind flag or partial coverage) / ❌ **Designed** (spec-only, not built)

OSS attune ships with **18 core capabilities** (this document). Industry verticals (law / patent / medical / academic / sales / engineering) live in `attune-pro` and are documented separately.

---

## 1. Capability Matrix (one-line summary)

| ID | Capability | Pillar | Maturity |
|----|-----------|--------|----------|
| **F-01-VAULT** | Three-factor encrypted vault + state machine + cross-device migration | 🔐 Sovereignty | ✅ |
| **F-02-RAG** | Hybrid retrieval (BM25 + vector + RRF) + J1 path-prefix chunker + two-stage hierarchical search | 📚 RAG Engine | ✅ |
| **F-03-CHAT** | RAG chat + B1 citation breadcrumb + session persistence + cross-session continuity | 💬 Conversation | ✅ |
| **F-04-READER** | Reader modal + 5 user annotation tags + 4-angle AI annotations + annotation-weighted RAG | 📖 Reading | ✅ |
| **F-05-COMPRESS** | Context compression pipeline + summary cache (70-85% cloud token savings) | 🗜️ Cost | ✅ |
| **F-06-WEBSEARCH** | Browser-automated web search (chromiumoxide / DuckDuckGo) + 30d encrypted cache | 🌐 Hybrid | ✅ |
| **F-07-EVOLUTION** | Episodic memory consolidation + SkillEvolver fail-signal expansion | 🧬 Active Evolution | ✅ |
| **F-08-BROWSEEXT** | Chrome extension G1/G2/G5: generic browse capture + auto bookmark + privacy panel | 📥 Capture | ✅ |
| **F-09-FORMFACTOR** | **★ v0.6.1**: FormFactor split (Laptop/K3Appliance/Server/Unknown) — LLM default path | 🪪 Hardware-aware | ✅ |
| **F-10-GOVERNOR** | H1 resource governor (3 profiles + per-task throttling + topbar pause) | ⚙️ System friendly | ✅ |
| **F-11-PLUGINS** | Plugin framework (`plugin.yaml` schema + `EntityExtractor` trait + marketplace toggle) | 🔌 Extensibility | ✅ |
| **F-12-PROJECT** | Project / Case generic layer + cross-evidence Project recommender | 🗂️ Organization | 🟡 |
| **F-13-WORKFLOW** | Workflow engine + Intent Router natural-language routing | 🔄 Automation | 🟡 |
| **F-14-ENTITIES** | Generic entity extractors (Person / Money / Date / Organization) | 🧩 NLP | ✅ |
| **F-15-MCP** | Python stdio shim wrapping REST for MCP clients | 🔧 Integration | ✅ |
| **F-16-DISTRIBUTION** | Tauri 2 desktop (Win MSI/NSIS, Linux deb/AppImage) + NAS HTTPS + hardware profile | 📦 Delivery | ✅ |
| **F-17-PRIVACY** | Phase A.5 three-tier privacy (L0 chunk-isolation / L1 PII placeholder / L3 v0.7+) + F-Pro cross-domain defense | 🔒 Privacy | 🟡 |
| **F-18-QUALITY** | K2 Parse golden set (CI gate) + RAGAS-style benchmark harness | 📊 Quality | ✅ |

---

## 2. Detailed Capabilities

### F-01-VAULT — Three-factor encrypted vault

**Capability**:
The user's master password + a device-bound 256-bit secret + Argon2id (64 MB / 3 rounds) derive a master key, which encrypts three Data Encryption Keys (DEK_db / DEK_idx / DEK_vec). Field-level AES-256-GCM encrypts content / tags / metadata in SQLite; tantivy index and usearch vectors hold their own DEKs. The vault has three states: **SEALED** (no password set) → **LOCKED** (password set, idle) → **UNLOCKED** (active session, 4h TTL via HMAC-SHA256 token). Lock zeroizes all keys from memory (`Zeroize` trait). Cross-device migration is via encrypted `.vault-profile` export/import — the device secret rolls forward, the master password stays in the user's head.

**Code**:
- `attune-core::vault` (`crates/attune-core/src/vault.rs:1-450`)
- `attune-core::crypto` (`crates/attune-core/src/crypto.rs`)
- `routes::vault` (`crates/attune-server/src/routes/vault.rs`) — REST endpoints (setup / unlock / lock / change-password / device-secret/export & import)

**Test Coverage**:
- Unit: `vault::tests` (16 tests covering setup-twice, dek-access-without-unlock, session-token-tampering, full-lifecycle CRUD), `crypto::tests` (3 tests for derive-master-key determinism)
- Integration: `tests/change_password_test.rs`, `tests/session_revoke_test.rs`, `tests/migration_roundtrip_test.rs`
- System: `tests/vault_setup_test.rs` (HTTP-level wizard setup → unlock → lock)
- Smoke: `scripts/smoke-test.sh` checks `/api/v1/vault/status` returns SEALED on fresh install
- E2E: 🟡 wizard step 1 (master password) covered by C.3 golden flow

**Maturity**: ✅ Active. v0.6.0 GA shipped; v0.6.1 unchanged.

---

### F-02-RAG — Hybrid retrieval engine

**Capability**:
Three-stage retrieval pipeline: (1) parallel candidate generation from BM25 (tantivy + jieba CJK tokenizer) + vector similarity (usearch HNSW with f16 quantization); (2) reciprocal rank fusion (RRF) merges both rankings with configurable weights (default 0.6 vector / 0.4 fulltext); (3) reranker (`bge-reranker-base` via ONNX Runtime) re-scores the top-K. The chunker (`chunker.rs::extract_sections`) does two-layer hierarchical chunking — section level (~1500 chars, Markdown headings / Rust `def|class` / paragraph fallback) and paragraph level (512 chars). At search time the engine returns parent-section context with the matched paragraph chunk, avoiding "context-free chunk in LLM" failure modes. **J1 path-prefix chunker** prepends `[Document > Section > Subsection]` to every chunk so the LLM knows which document it's reading. **J3 explicit min_score** lets the user trade recall for precision (default 0.0). **J5 strict prompt** disallows "I don't know" hedging and emits a 1-5 confidence marker the parser strips before user display.

**Code**:
- `attune-core::search` (`crates/attune-core/src/search.rs:1-600`) — `search_relevant()` is the hybrid entry point
- `attune-core::chunker` (`crates/attune-core/src/chunker.rs`) — `extract_sections()` + path prefix
- `attune-core::index` (`crates/attune-core/src/index.rs`) — tantivy wrapper
- `attune-core::vectors` (`crates/attune-core/src/vectors.rs`) — usearch wrapper
- `attune-core::infer::reranker` (`crates/attune-core/src/infer/reranker.rs`) — `bge-reranker-base` via `ort`
- `routes::search` (`crates/attune-server/src/routes/search.rs`)

**Test Coverage**:
- Unit: `search::tests`, `chunker::tests`, `index::tests`
- Integration: `tests/rag_w2_batch1_integration.rs`, `tests/rag_quality_benchmark.rs`
- System: `rust/tests/corpus_integration_test.rs` (real rust-book + cs-notes corpora)
- Quality: `rust/tests/golden/queries.json` precision@K regression
- Smoke: not covered (requires indexed corpus)

**Maturity**: ✅ Active.

---

### F-03-CHAT — RAG Chat + Citation + Sessions

**Capability**:
Chat is the primary interface. Each message goes through: query intent → hybrid retrieval (F-02) → context compression (F-05) → strict prompt → LLM call (local Ollama or remote OpenAI-compatible endpoint) → confidence parser → citation chip rendering. Each citation chip carries `source` (item id), `breadcrumb` (chapter path), `chunk_offset_start/end` (Reader deep-link target), and `confidence` (1-5). Sessions are persisted with AES-256-GCM (`store::conversations`), searchable by query, and **cross-session continuity** means a chat from 3 weeks ago can be resumed at the same context. No streaming — local 0.6B-3B models respond fast enough; remote APIs show a spinner during wait (per CLAUDE.md product decision).

**Code**:
- `attune-core::chat` (`crates/attune-core/src/chat.rs`) — `ChatEngine` + `parse_confidence`
- `attune-core::store::conversations` (`crates/attune-core/src/store/conversations.rs`)
- `routes::chat` + `routes::chat_sessions` (`crates/attune-server/src/routes/chat.rs`, `chat_sessions.rs`)

**Test Coverage**:
- Unit: `chat::tests` (parse_confidence / strip_confidence_marker / citation extraction edge cases)
- Integration: 🟡 partial — `routes::chat::tests` smoke
- System: F-CHAT-S1 (planned in B.3): full wizard → ingest → chat → citation → Reader jump
- E2E: 🟡 C.3 golden flow #3 covers citation → Reader jump

**Maturity**: ✅ Active.

---

### F-04-READER — Reader + Annotations + AI annotations

**Capability**:
Reader modal renders a stored item with chunk-level navigation. **User annotations**: select text → choose from 5 preset tags (⭐ Highlight / 📍 Deep-dive / 🤔 Question / ❓ Unclear / 🗑 Outdated) with 4 colors + free-text note. **AI annotations**: "🤖 AI analysis ▾" dropdown with 4 angles (⚠️ Risk / 🕰 Outdated / ⭐ Highlights / 🤔 Questions); a local LLM analyzes the chunk and emits annotations with precise text offsets. **Annotation-weighted RAG**: at search time, ⭐ Highlight / ⚠️ Risk → ×1.5 score boost; 🤔 Question → ×1.2; 🗑 / 🕰 Outdated → excluded. Annotation content is AES-256-GCM encrypted; cascades on item soft-delete (semantically: "forget the knowledge").

**Code**:
- `attune-core::store::annotations` (`crates/attune-core/src/store/annotations.rs`)
- `attune-core::ai_annotator` (`crates/attune-core/src/ai_annotator.rs`)
- `attune-core::annotation_weight` (`crates/attune-core/src/annotation_weight.rs`)
- `routes::annotations` (`crates/attune-server/src/routes/annotations.rs`)

**Test Coverage**:
- Unit: `annotation_weight::tests`, `ai_annotator::tests`
- Integration: `tests/rag_w3_batch_a_integration.rs` (annotation → weighted RAG round-trip)
- E2E: 🟡 C.3 golden flow #2 covers user annotation → RAG boost verification

**Maturity**: ✅ Active.

---

### F-05-COMPRESS — Context compression + Summary cache

**Capability**:
Retrieved chunks are compressed via a local LLM call into a **150-char summary** (economical mode, default) or **300-char head + summary** (accurate mode), before being concatenated into the chat LLM prompt. This reduces cloud token consumption by 70-85%. The summary is keyed by `sha256(chunk_text)`, persisted in `store::chunk_summaries` (encrypted), and reused forever — first-pass cost only. A "raw" mode skips compression entirely (local-only). The Token Chip UI estimates input tokens + cloud cost in real time, distinguishing 🟢 Local (free) from 💰 Cloud ($).

**Code**:
- `attune-core::context_compress` (`crates/attune-core/src/context_compress.rs`)
- `attune-core::store::chunk_summaries` (`crates/attune-core/src/store/chunk_summaries.rs`)

**Test Coverage**:
- Unit: `context_compress::tests`
- Integration: `tests/rag_w3_batch_b_integration.rs` (compression → cache → reuse)

**Maturity**: ✅ Active.

---

### F-06-WEBSEARCH — Browser-automated web search

**Capability**:
When the local vault has no high-confidence match, attune drives a system-installed Chromium-based browser (Chrome/Edge) headless via the CDP protocol (`chromiumoxide` crate) to scrape DuckDuckGo HTML results. **Zero API key, zero subscription**. Rate-limited to ≥2s between queries. Failure modes are explicit: no browser found → log warning + return empty results + chat appends "web search unavailable, install Chrome or Edge"; never silently downgrade to a paid API. Results cached for 30 days in `store::web_search_cache` (AES-256-GCM encrypted, keyed by `sha256(query)`).

**Code**:
- `attune-core::web_search` (trait), `web_search_browser` (impl)
- `attune-core::store::web_search_cache`
- `routes::web_search_cache` (`crates/attune-server/src/routes/web_search_cache.rs`)

**Test Coverage**:
- Unit: `web_search_browser::tests`, `store::web_search_cache::tests` (encryption-at-rest, overwrite semantics)
- Integration: 🟡 partial (mocked CDP)
- System: 🟡 manual (real Chrome instance) — covered in `tests/MANUAL_TEST_CHECKLIST.md`

**Maturity**: ✅ Active.

---

### F-07-EVOLUTION — Active learning loop

**Capability**:
Two cooperating loops: (1) **Episodic memory consolidation** (A1) — periodic background agent that reviews recent chats, condenses repeated threads into compact "episodes" recallable by intent. (2) **SkillEvolver** — silently records local-miss queries as "fail signals", and every 4 hours (or after 10 signals) sends them to an LLM that proposes synonym expansions, written silently into `learned_expansions` config. Three months in, the same query returns noticeably more relevant results — without any "retrain" UI.

**Code**:
- `attune-core::memory_consolidation` (`crates/attune-core/src/memory_consolidation.rs`)
- `attune-core::skill_evolution` (`crates/attune-core/src/skill_evolution.rs`)
- `attune-core::store::memories`, `store::signals`

**Test Coverage**:
- Unit: `skill_evolution::tests`, `memory_consolidation::tests`
- Integration: `tests/memory_consolidation_integration.rs`

**Maturity**: ✅ Active. K1 sleeptime agent upgrade is in M3+ roadmap (Letta-inspired).

---

### F-08-BROWSEEXT — Chrome extension generic browse capture

**Capability**:
The Chrome MV3 extension upgraded in v0.6 from "AI chat capture only" to **generic browse-state knowledge source**. It captures URL / title / time-on-page / scroll depth / copy-paste actions / dwell time / revisit frequency. **G1**: signals stream to attune backend over `/api/v1/browse_signals`. **G2**: pages with high engagement (≥3min dwell + >50% scroll + ≥1 copy-paste) auto-bookmark to a staging area for user review. **G5**: a privacy panel in the popup shows what was captured, lets the user clear all data with one click, edit per-domain whitelist, and a `HARD_BLACKLIST` (banks / healthcare / government login / password manager / incognito / pages with `password` field) cannot be enabled by user.

**Code**:
- `extension/` (TypeScript, Manifest V3 + Preact + Vite)
- `routes::browse_signals` (`crates/attune-server/src/routes/browse_signals.rs`)
- `routes::auto_bookmarks` (`crates/attune-server/src/routes/auto_bookmarks.rs`)
- `routes::privacy` (`crates/attune-server/src/routes/privacy.rs`)
- `attune-core::store::browse_signals`, `store::auto_bookmarks`

**Test Coverage**:
- Unit: `store::browse_signals::tests`, `store::auto_bookmarks::tests`
- Integration: `tests/projects_routes_test.rs` partial
- E2E: ❌ extension Playwright not yet in attune main repo (lives in extension submodule); covered by extension's own Playwright E2E (42 tests in Python prototype)

**Maturity**: ✅ Active.

---

### F-09-FORMFACTOR — Hardware form-factor split (★ v0.6.1)

**Capability**:
A new axis on `HardwareProfile`: `FormFactor` enum (`Laptop` / `K3Appliance` / `Server` / `Unknown`). Detection priority: (1) `ATTUNE_FORM_FACTOR=k3|laptop|server` env var (used by K3 image's systemd unit); (2) Linux DMI keyword in `/sys/class/dmi/id/product_name` containing "K3" or "Jetson"; (3) default `Laptop`. The form factor decides the LLM default path: **Laptop / Server / Unknown** → `llm.provider = "openai_compat"` (remote token, wizard guides the user to fill API key) — preserves v0.6.0 GA behavior. **K3Appliance** → `llm.provider = "ollama"` + `endpoint: "http://localhost:11434/v1"` + `model: "qwen2.5:3b"` for K3 image with pre-installed local LLM. The wizard Step 3 (`Step3LLM.tsx`) reads `prefers_local_llm` from `/status/diagnostics` and switches the recommended card (Ollama vs Cloud) with ★ Recommended marker + dashed border for the non-active recommendation.

**Code**:
- `attune-core::platform::FormFactor` + `detect_form_factor()` (`crates/attune-core/src/platform/mod.rs:69-130`)
- `routes::settings::default_settings()` (`crates/attune-server/src/routes/settings.rs:154-180`)
- `routes::status::diagnostics` exposes `form_factor` + `prefers_local_llm`
- `ui/src/wizard/Step3LLM.tsx`

**Test Coverage**:
- Unit: `platform::tests::form_factor_default_is_laptop`, `prefers_local_llm_only_for_k3`, `detect_form_factor_respects_env_override` (9 inputs), `form_factor_in_hardware_profile_detect`
- Unit (settings): `routes::settings::tests::laptop_form_factor_uses_remote_token`, `k3_form_factor_uses_local_ollama`, `server_and_unknown_fallback_to_remote_token`, `non_llm_settings_invariant_across_form_factors`
- Smoke: planned C.1 — `ATTUNE_FORM_FACTOR=k3 ./attune-server-headless` + curl `/api/v1/status/diagnostics` returns `form_factor: "k3"`

**Maturity**: ✅ Active (v0.6.1, 0 breaking change vs v0.6.0).

---

### F-10-GOVERNOR — H1 Resource governor

**Capability**:
Every long-running background task (embedding generation, OCR, ASR, SkillEvolver, vector index rebuild, browser capture, RPA crawlers) runs through a **task-level resource governor** with three default profiles: **Conservative** (battery / shared machine), **Balanced** (default desktop), **Aggressive** (idle workstation). Per-task throttling means critical-path queries (chat real-time retrieval) get green light while background batches (embedding queue / SkillEvolver) get red light. The top bar always carries a "Pause all background tasks" button. Auto-fallback: laptop on battery → Conservative; CPU > 80% sustained → throttle background tasks by 50%; full-screen game/presentation detection (OS focus) → all background tasks pause.

**Code**:
- `attune-core::resource_governor` (5 modules: `governor.rs`, `monitor.rs`, `profiles.rs`, `registry.rs`, `mod.rs`)
- Web UI top bar pause button

**Test Coverage**:
- Unit: `resource_governor::governor::tests`, `monitor::tests`, `profiles::tests`, `registry::tests`
- Integration: `tests/governor_integration.rs`

**Maturity**: ✅ Active.

---

### F-11-PLUGINS — Plugin framework

**Capability**:
Plugins are loaded at startup from `~/.local/share/attune/plugins/` (or `%LOCALAPPDATA%\attune\plugins\`). Each plugin = `plugin.yaml` (manifest with `id`, `name`, `type`, `category`, `version`, `requires.attune_core`, `capabilities[]`, optional `chat_trigger` for natural-language routing) + optional Rust crate or pure prompts. Signed `.attunepkg` distribution via Ed25519. **OSS attune ships zero industry plugins** (per `oss-pro-strategy.md` v2 Decision 2) — `assets/plugins/` is empty. Industry plugins (`law-pro`, `presales-pro`, `patent-pro`, `tech-pro`, `medical-pro`, `academic-pro`) live in the `attune-pro` private repo. The marketplace toggle UI (W4 E1) lets users enable/disable plugins per-vault.

**Code**:
- `attune-core::plugin_loader` (`crates/attune-core/src/plugin_loader.rs`)
- `attune-core::plugin_registry`
- `attune-core::plugin_sig` — Ed25519 signature verification
- `routes::plugins` (`crates/attune-server/src/routes/plugins.rs`)
- `routes::skills` (capability listing)

**Test Coverage**:
- Unit: `plugin_loader::tests`, `plugin_registry::tests`, `plugin_sig::tests`
- Integration: planned B.2 — `tests/persona_plugin_integration.rs` (Persona registration via plugin)

**Maturity**: ✅ Active. v0.7+ planned: `provides_role` schema for industry-Persona injection.

---

### F-12-PROJECT — Project / Case generic layer

**Capability**:
A Project is a user-defined grouping of items (files, conversations, notes) with optional metadata (case_no for legal Project subclass, application_no for patent, topic_keywords for research). The **ProjectRecommender** scans newly-ingested items for entity matches against existing Projects and recommends "is this part of Project X?" with a confidence score; if `chat_trigger.needs_confirm: true`, the UI shows a confirmation popup, else auto-files. Cross-evidence linking: at chat time, retrieval can be scoped to a single Project, and citations flag items from the same Project's evidence chain. Industry-specific Project subclasses (`legal_case` / `patent_application` / `research_topic`) are injected by attune-pro plugins via `extends_project_kind` (planned v0.7+).

**Code**:
- `attune-core::store::project` (`crates/attune-core/src/store/project.rs`)
- `attune-core::project_recommender`
- `routes::projects` (`crates/attune-server/src/routes/projects.rs`)

**Test Coverage**:
- Unit: `project_recommender::tests`, `store::project::tests`
- Integration: `tests/project_recommender_test.rs`, `tests/projects_routes_test.rs`

**Maturity**: 🟡 Partial — generic Project ✅; `extends_project_kind` plugin extension point ❌ (planned v0.7+).

---

### F-13-WORKFLOW — Workflow engine + Intent Router

**Capability**:
Two cooperating systems: **Workflow engine** runs declarative multi-step ops (`find_overlap`, `write_annotation`, `evidence_chain`) defined in plugin yaml files. Each step has explicit `needs_confirm` gates (user approval required for token-spending or RPA actions) and outputs go to `workflow.outputs[step_id]` for downstream steps to consume. **Intent Router** matches natural-language queries to skills via plugin `chat_trigger.patterns` (regex) and `chat_trigger.keywords` (BERT-style classifier); rule match → execute skill; ambiguous → fall back to RAG chat. The router is plugin-aggregated — OSS-only attune has empty trigger list (no industry triggers); attune-pro plugins populate via their own `chat_trigger`.

**Code**:
- `attune-core::workflow` (`crates/attune-core/src/workflow/mod.rs`)
- `attune-core::intent_router` (`crates/attune-core/src/intent_router.rs`)

**Test Coverage**:
- Unit: `workflow::tests`, `intent_router::tests`
- Integration: `tests/workflow_test.rs`

**Maturity**: 🟡 Partial — engine ✅; richer ops library and Intent Router third-tier (LLM fallback) ❌ planned.

---

### F-14-ENTITIES — Generic entity extractors

**Capability**:
Built-in extractors for `Person`, `Money`, `Date`, `Organization` — plus the trait-based `EntityExtractor` so plugins can register more. v0.6.0-rc.2 boundary trim: industry-specific extractors (`CaseNo` Chinese legal case number regex) moved to `attune-pro/plugins/law-pro/extractors/`; OSS contains only generic types. Entities feed Project recommender (F-12) and chat citations.

**Code**:
- `attune-core::entities` (`crates/attune-core/src/entities.rs`)
- `attune-core::taxonomy`

**Test Coverage**:
- Unit: `entities::tests`
- Integration: `tests/entities_test.rs`

**Maturity**: ✅ Active. Plugin-extensible `extends_entity_kinds` planned v0.7+.

---

### F-15-MCP — MCP integration via Python stdio shim

**Capability**:
`tools/attune_mcp_shim.py` is a stdio-based MCP server that wraps attune's REST API. MCP clients (Claude Desktop, Cline, Continue.dev) can register attune as a tool source — they get retrieval / item fetch / chat session listing without needing to write attune-aware code. The shim handles vault unlock state via a session token cached in `~/.cache/attune-mcp/`.

**Code**:
- `tools/attune_mcp_shim.py` (Python stdio bridge)
- Spec: `docs/mcp-integration.md` (bilingual)

**Test Coverage**:
- Manual: `tests/MANUAL_TEST_CHECKLIST.md` MCP section (live backend integration with Claude Desktop / Cursor / Cline)
- Protocol: `tests/mcp/test_mcp_shim.py` ✅ (7 tests, v0.6.3): subprocess +
  JSON-RPC 2.0 over stdio. Covers initialize handshake / notifications /
  tools/list / unknown tool error / unknown method error / malformed JSON
  resilience / dead backend structured error.
- Integration with live backend: 🟡 partial — protocol layer ✅ but actual
  tool execution (attune_search hits real index, attune_chat hits real LLM)
  remains manual.

**Maturity**: ✅ Active. Protocol-layer auto-test in v0.6.3.

---

### F-16-DISTRIBUTION — Tauri 2 desktop + NAS + hardware profile

**Capability**:
Desktop installer via Tauri 2 + tauri-plugin-updater (auto-updates). **Windows**: NSIS recommended (`Attune_0.6.1_x64-setup.exe` ~16 MB) + MSI for enterprise (~31 MB). **Linux**: deb (~27 MB) + AppImage (~94 MB). **NAS HTTPS mode**: `--host 0.0.0.0 --tls-cert ... --tls-key ...` exposes attune over HTTPS with Bearer token auth — designed for self-hosted deployment on a home NAS, accessible via mobile browser. **HardwareProfile** auto-detects at startup: CPU vendor/model, NVIDIA GPU (`/dev/nvidia0`), AMD GPU (`/dev/kfd` + gfx target like gfx1103 for Radeon 780M), AMD XDNA NPU (Ryzen AI), Intel NPU, total RAM, OS, FormFactor (F-09). The detected profile drives recommended summary model + ROCm `HSA_OVERRIDE_GFX_VERSION` env var injection.

**Code**:
- `apps/attune-desktop/src/` (Tauri shell)
- `attune-core::platform::HardwareProfile::detect()`
- `routes::status::diagnostics` exposes the profile

**Test Coverage**:
- Unit: `platform::tests` (15 tests covering OS detection, summary model recommendation, env var injection, FormFactor — see F-09)
- System: `tests/integration_test.rs`, `tests/server_test.rs`
- Smoke: `scripts/smoke-test.sh` validates binary spawn + `/api/v1/status/health` 200 + CORS

**Maturity**: ✅ Active on Windows (P0) + Linux (P1). macOS deferred (per CLAUDE.md platform priority).

#### Hardware utilization matrix (v0.6.3 audit)

attune detects hardware (HardwareProfile) but the actual utilization in each
inference backend depends on the build-time features. This matrix is the
single source of truth for **what gets accelerated where**:

| Backend | NVIDIA CUDA | AMD ROCm/HIP | AMD XDNA NPU | Intel NPU/iGPU | Apple Metal | CPU |
|---------|:-----------:|:------------:|:------------:|:--------------:|:-----------:|:---:|
| `embed::OllamaProvider` (bge-m3) | ✅ via Ollama runtime | ✅ via Ollama (ROCm) | ❌ Ollama 不支持 | ❌ Ollama 不支持 | ✅ via Ollama (Metal) | ✅ fallback |
| `infer::reranker` (ort) | ✅ CUDA EP | ❌ ROCm EP not compiled | ❌ DirectML EP not compiled | ❌ OpenVINO EP not compiled | ❌ CoreML EP not compiled | ✅ default |
| `infer::embedding` (ort, ONNX models) | ✅ CUDA EP | ❌ same as reranker | ❌ same | ❌ same | ❌ same | ✅ default |
| `asr` (whisper.cpp subprocess) | ✅ if user installs **GPU build** | ✅ if Vulkan/ROCm build | ❌ no XDNA support | ❌ no NPU support | ✅ if Metal build | ✅ default (CPU-only build) |
| `llm` (Ollama HTTP) | ✅ via Ollama runtime | ✅ via Ollama | ❌ same | ❌ same | ✅ via Ollama | ✅ fallback |
| `llm` (OpenAI-compat HTTP) | N/A (remote) | N/A | N/A | N/A | N/A | N/A |
| `ocr` (tesseract subprocess) | ❌ tesseract is CPU-only | ❌ | ❌ | ❌ | ❌ | ✅ |

**Gap analysis** (v0.6.3 → v0.7+):

1. **ort EP coverage**: Cargo.toml `ort = { features = ["std", "ndarray", "tracing", "tls-rustls"] }` only compiles **CUDA + CPU EPs**. To enable Intel NPU/iGPU we'd need `features += ["openvino"]`; AMD NPU needs `["directml"]` (Windows only); Apple Silicon needs `["coreml"]`. Each adds ~30-100MB to the binary. Status: 📋 v0.7+ (cross-platform CI matrix work).

2. **whisper.cpp GPU detection**: v0.6.3 added `AsrBackend.gpu_capable` field that probes `whisper-cli --help` for `--no-gpu`/`gpu-device` flags. Exposed via `/api/v1/ai_stack` so Settings UI warns the user when CPU-only build limits ASR to ~10x slower transcription.

3. **Ollama runtime GPU verification**: 🟡 partial — `/status/diagnostics` reports `ollama_status: "ready"` if HTTP probe succeeds, but doesn't confirm Ollama is using GPU vs falling back to CPU. v0.7+ enhancement: probe `ollama ps` to read VRAM usage.

4. **HardwareProfile → ort EP linkage**: `provider::build_session()` uses an isolated `detect_npu()` call (env var `NPU_VAULT_EP`) instead of reading the cached `state.hardware`. This is partly intentional (env var override is the documented escape hatch), but means `state.hardware.has_intel_npu = true` does not trigger OpenVINO automatically. v0.7+ work: thread `&HardwareProfile` through `build_session()` so detection drives EP selection by default.

---

### F-17-PRIVACY — Three-tier privacy model + cross-domain defense

**Capability**:
Two complementary systems. **Phase A.5 three-tier privacy**: **L0** per-file flag, chunks marked L0 never leave the device (forced local LLM); **L1 (default)** 12 PII classes (id-card with ISO 7064 checksum, phone, email, 8 API key vendors, etc.) detected by regex and replaced with reversible `[KIND_N]` placeholders before any cloud API call, with an outbound audit log (CSV exportable for compliance review); **L3 (v0.7 target)** LLM-based semantic redaction on Tier T3+/K3 hardware. **F-Pro cross-domain pollution defense**: items have a `corpus_domain` metadata field; chunks are prefixed `[领域: legal]` so retrieval can apply a cross-domain penalty (default 0.4) — keyword-based query intent detection (zero LLM call) determines target domain. This solves the shared-vault problem where a "反洗钱" query was pulling Java algorithm docs (real reported bug pre-v0.6.0-rc.5).

**Code**:
- `attune-core::pii` (mod.rs + patterns.rs)
- `routes::privacy` (audit log download)
- `routes::audit`
- `attune-core::store::items` `corpus_domain` field
- Cross-domain logic in `attune-core::search`

**Test Coverage**:
- Unit: `pii::patterns::tests` (regex coverage per PII class — 50 tests)
- Integration: `attune-core/tests/pii_chat_path_redact_test.rs` (4 tests, v0.6.2 ✅) — verifies
  ChatEngine wires Redactor: user_message redacted before LLM call, placeholders
  restored in response, multi-kind PII (phone+email+api_key) round-trips correctly,
  PII-free messages pass through unchanged.

**Maturity**: 🟢 **mostly Active — full chat path wired in v0.6.3**:
- L0 chunk-isolation: ✅ Active
- L1 PII module — **chat outbound path** ✅:
  - `Redactor::redact_batch` ✅ (v0.6.3): separator-based batch redact with
    globally unique `[KIND_N]` indices — same placeholder always points to
    same original value across user/history/knowledge segments
  - `ChatEngine::run_llm_once` ✅ (v0.6.3): redacts `[system_prompt,
    user_message, history[*]]` — system_prompt embeds knowledge so knowledge
    PII is covered transitively
  - `outbound_audit` log emitted via `log::info!` (v0.6.2) ✅
  - **Not yet wired**: `context_compress` LLM summary call, `ai_annotator`,
    `web_search_browser` query (separate v0.7+ patches — these are LLM call
    sites parallel to chat, each needs its own redact wrapper)
  - Audit log persistence to `store::audit_log` (currently log-only) — v0.7+
- F-Pro cross-domain defense: ✅ Active
- L3 LLM redaction: ❌ Designed (v0.7+)

---

### F-18-QUALITY — Quality gates

**Capability**:
Two complementary regression gates. **K2 Parse Golden Set** (W3 batch C, 2026-04-27): 5 baseline markdown fixtures in `crates/attune-core/tests/fixtures/parse_corpus/` with `manifest.yaml` describing expected `title_contains`, `min_text_chars`, `must_contain_phrases`, `section_count_min`, `section_paths_must_include`. Harness `parse_golden_set_regression.rs` enforces 100% pass rate for baseline (5 fixtures); when expanded to 200 fixtures, threshold drops to 95% per Readwise Reader methodology. **RAGAS-style benchmark harness**: `scripts/bench-orchestrator.sh` runs three-track retrieval benchmark (legal lawcontrol corpus / English rust-book / Chinese cs-notes) computing Hit@10, MRR, Recall@10. v0.6.0 GA achieved Hit@10 = 0.80/1.00/1.00 across the three tracks. Plus `tests/golden/queries.json` for precision@K regression.

**Code**:
- `crates/attune-core/tests/parse_golden_set_regression.rs`
- `crates/attune-core/tests/rag_quality_benchmark.rs`
- `scripts/bench-orchestrator.sh`, `scripts/run-benchmark-corpus.sh`, `scripts/run-final-eval.py`
- `rust/tests/golden/queries.json`

**Test Coverage**:
- Quality regression: itself is the test layer

**Maturity**: ✅ Active.

---

## 3. Cross-cutting Concerns

### 3.1 Security model

- All vault data field-level AES-256-GCM encrypted (DEK_db / DEK_idx / DEK_vec separated)
- Argon2id (64 MB / 3 rounds / 4 threads) — GPU/ASIC-resistant
- Session token: HMAC-SHA256(session_id + expires, MK), 4h TTL
- API key never returned in GET (`redact_api_key` in `routes::settings`)
- CORS allowlist: localhost + Chrome extension origins + user-configured origins
- TLS via `rustls` (pure Rust, no OpenSSL), `rustls-webpki` 0.103.13 (3 RUSTSEC CVEs fixed in v0.6.1)

### 3.2 Internationalization

- Bilingual public docs: every `<NAME>.md` ships with `<NAME>.zh.md`
- Web UI: i18n strings in `t()` calls (en-US + zh-CN)
- Tantivy CJK tokenization via `tantivy-jieba`
- Embedding via `bge-m3` (multilingual)

### 3.3 Error handling

- All `Result<T, VaultError>` typed errors; `VaultError` enum has `LlmUnavailable`, `Classification`, `IndexCorrupted`, `WrongPassword`, etc.
- HTTP responses: 4xx with structured `{"error": "...", "hint": "..."}` body; 5xx with generic message (no internal detail leak — see `routes::errors::tests::internal_error_response_is_generic`)
- Vault-locked endpoints return 403 with `{"error": "vault is locked", "hint": "POST /api/v1/vault/unlock"}`

### 3.4 Observability

- `tracing` crate, structured logs (JSON in production, pretty in dev)
- `/api/v1/status/diagnostics` exposes vault state, AI status, embedding/classifier readiness, ollama models, hardware profile, form_factor (F-09)
- Outbound audit log (F-17 L1) CSV-exportable for compliance review

---

## 4. Capability ↔ Test Layer Coverage Map

This is the inverse view of `TESTING.md`'s test pyramid. For each test layer, which capabilities does it currently cover?

| Capability | Unit | Integration | System | E2E | Smoke |
|-----------|:----:|:-----------:|:------:|:---:|:-----:|
| F-01-VAULT | ✅ | ✅ | ✅ | 🟡 | ✅ |
| F-02-RAG | ✅ | ✅ | ✅ corpus | ❌ | 🟡 |
| F-03-CHAT | ✅ | 🟡 | 🟡 (B.3 planned) | 🟡 (C.3 planned) | ❌ |
| F-04-READER | ✅ | ✅ | 🟡 | 🟡 (C.3 planned) | ❌ |
| F-05-COMPRESS | ✅ | ✅ | ❌ | ❌ | ❌ |
| F-06-WEBSEARCH | ✅ | 🟡 | 🟡 manual | ❌ | ❌ |
| F-07-EVOLUTION | ✅ | ✅ | ❌ | ❌ | ❌ |
| F-08-BROWSEEXT | ✅ | 🟡 | ❌ | ✅ (extension) | ❌ |
| F-09-FORMFACTOR | ✅ (8) | ❌ | ❌ | ❌ | 🟡 (C.1 planned) |
| F-10-GOVERNOR | ✅ | ✅ | ❌ | ❌ | ❌ |
| F-11-PLUGINS | ✅ | 🟡 (B.2 planned) | ❌ | ❌ | ❌ |
| F-12-PROJECT | ✅ | ✅ | ❌ | ❌ | ❌ |
| F-13-WORKFLOW | ✅ | ✅ | ❌ | ❌ | ❌ |
| F-14-ENTITIES | ✅ | ✅ | ❌ | ❌ | ❌ |
| F-15-MCP | ❌ | ✅ protocol (v0.6.3, 7 tests) | ❌ | ❌ | ❌ manual |
| F-16-DISTRIBUTION | ✅ | ✅ | ✅ | ❌ | ✅ |
| F-17-PRIVACY | ✅ | ✅ (full chat path, v0.6.3) | ❌ | ❌ | ❌ |
| F-18-QUALITY | ✅ | ✅ corpus | ✅ | ❌ | ❌ |

**Gaps** (drive B.1 / B.2 / B.3 / C.1 / C.3 task definitions):
1. F-03-CHAT lacks System / E2E coverage — B.3 (full wizard → chat flow) + C.3 #3 (citation jump)
2. F-09-FORMFACTOR Smoke not yet in `smoke-test.sh` — C.1
3. F-11-PLUGINS Integration not exercised end-to-end — B.2 (persona ↔ plugin)
4. F-17-PRIVACY chat-path leak prevention not tested — B.2 (PII chat integration)
5. F-15-MCP fully manual — automation needs cross-language harness (deferred to v0.7+)

---

## 5. Out-of-scope (NOT in OSS attune)

To make boundaries clear (per `oss-pro-strategy.md` v2):

| Capability | Where it lives | Why not OSS |
|-----------|----------------|-------------|
| Industry plugins (law-pro, patent-pro, etc.) | `attune-pro` private repo | Industry verticals = monetization layer |
| Industry Persona (Lawyer/Doctor/PatentAgent) | `attune-pro` plugin pack via `provides_role` | Industry binding violates OSS "any individual user" rule |
| Industry entities (CaseNo, expert opinion, patent number) | `attune-pro/plugins/<vertical>-pro/extractors/` | Same |
| Cloud sync, plugin registry, LLM proxy | `attune-pro` services layer | Centralized infra |
| Multi-tenant RBAC, case assignment, multi-user collab | `lawcontrol` (separate product) | B2B small-team scenarios |
| Mobile apps | Roadmap silent | Tauri 2.0 mobile not yet stable |

---

## 6. Capability Lifecycle

A new capability enters this document **only after** code is merged. "Designed but not built" specs live in `docs/superpowers/specs/` and are NOT listed here. When a capability:

- ships → entry created with Maturity ✅
- partially ships → 🟡 with explicit list of what's missing
- gets removed → entry deleted + `RELEASE.md` notes the removal
- moves to `attune-pro` → entry deleted from this doc, added to `attune-pro/docs/specs/`

This rule prevents "P0 approved ≠ code shipped" drift (see `feedback_decision_vs_implementation.md` in memory).

---

## Appendix: Maturity legend

- ✅ **Active** — shipped, default-on for all users
- 🟡 **Partial** — shipped but behind a flag, partial coverage, or planned extension point exists
- ❌ **Designed** — spec only, not in current binary; should not appear in this doc unless explicitly tracking a roadmap reservation

For a forward-looking roadmap, see `RELEASE.md` "What's next" + `oss-pro-strategy.md` §5 six-month roadmap.
