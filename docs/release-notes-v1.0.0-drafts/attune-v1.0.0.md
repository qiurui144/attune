# Attune v1.0.0 — Private AI Knowledge Companion (2026-05-25 GA)

> **Your private AI knowledge companion.** All knowledge encrypted on your device.
> Local inference first, cloud LLM on demand. Wrong password = sealed vault, data unreadable.

---

## Highlights

- **4-layer Memory Architecture (L0–L3)** — episodic / rolling summary / semantic topic layers; chat context token usage reduced by 78.7% (median) vs v0.6
- **Office Helper: Structured OCR + Async ASR** — 5 OCR scene profiles (receipt / table / card / id-card / document) + 3 card subtypes (ID / bank card / biz license); whisper.cpp async meeting transcription + WebSocket progress push; CLI `attune ocr` / `attune transcribe`
- **4 OSS Intelligence Agents** — `memory_consolidation` (L2→L3 promotion) / `internal_knowledge_linker` (cross-item entity graph) / `chat_reliability` (citation / contradiction / hallucination post-hoc eval) / `self_evolving_skill` (heuristic + LLM expansion with CJK Trad↔Simp normalization)
- **5 Built-in Ingest Connectors** — Local folder watch / Email IMAP / WebDAV (NAS) / RSS feed / Telegram (scaffold, v1.1)
- **Cloud Member Gateway** — one-click login + LLM auto-configure + law-pro plugin auto-sync; Attune Pro Membership or BYOK
- **49 E2E / 1145+ lib tests** — 0 failures; `cargo clippy -D warnings` clean

---

## Downloads

| Platform | File | Notes |
|----------|------|-------|
| **Windows** (installer, recommended) | `Attune_1.0.0_x64-setup.exe` | NSIS, supports auto-updater |
| **Windows** (MSI, enterprise) | `Attune_1.0.0_x64.msi` | MSI, manual update |
| **Linux** (.deb, Ubuntu/Debian) | `attune_1.0.0_amd64.deb` | systemd-compatible |
| **Linux** (.rpm, Fedora/RHEL) | `attune_1.0.0_x86_64.rpm` | |
| **Linux** (AppImage, portable) | `Attune_1.0.0_amd64.AppImage` | no install needed |

> **Server / CLI tarballs** (headless NAS / K3 appliance):
>
> | Platform | File |
> |----------|------|
> | Linux x86_64 | `attune-v1.0.0-linux-x86_64.tar.gz` |
> | Linux aarch64 | `attune-v1.0.0-linux-aarch64.tar.gz` |
> | Windows x86_64 | `attune-v1.0.0-windows-x86_64.zip` |
> | macOS Apple Silicon | `attune-v1.0.0-macos-aarch64.tar.gz` |

SHA256 checksums: see `checksums.txt` attached to this release.

---

## What's New (v0.7.0 → v1.0.0)

### Memory & Intelligence

- `memory_consolidation_agent` — deterministic L2 → L3 promotion algorithm + Store helpers; 11 real golden cases + 3 error fixtures + ENFORCE gate
- `internal_knowledge_linker_agent` — activates entity_graph dead code; cross-item concept linking with 6-class reliability gate
- `chat_reliability_agent` — post-hoc citation / contradiction / hallucination evaluation; 4 proptest invariant suites (256 cases)
- `self_evolving_skill_agent` — per-query expansion learner (heuristic path + LLM expansion); CJK Traditional ↔ Simplified normalization + dedupe; real-LLM verification gate 4/4 PASS

### Office Helper (v0.7.1 merged into GA)

- **OCR**: receipt (7 fields) / table (cell reconstruction) / card (6 fields) / id_card (3 subtypes: id_card_cn / bank_card / business_license, with Luhn / GB 11643 / GB 32100 checksum validation) / document (paragraph clustering + 2-column reorder + title detection)
- **ASR**: whisper.cpp subprocess + async job queue + WebSocket `ws://…/api/v1/office/transcribe/ws` progress push
- **REST / WS**: `POST /api/v1/office/ocr` / `POST /api/v1/office/transcribe` / `GET /api/v1/office/jobs/{id}`
- **Web UI**: `OfficeView` tab — OCR panel + ASR transcription panel
- **CLI**: `attune ocr <image> [--profile receipt|table|card|id_card|document] [--json]` / `attune transcribe <audio>`

### Cloud & LLM Integration

- Cloud member login: `attune login <email>` → token stored at `~/.config/npu-vault/license.json` (chmod 600)
- Auto-configure LLM gateway: member `gateway_token` + `gateway_url` hot-reload into running server (`apply_cloud_llm_if_needed`)
- Plugin auto-sync: `attune sync-plugins` pulls entitled pro plugins from pluginhub → verify Ed25519 sig → atomic install
- Fix P0: empty model field now triggers `/v1/models` probe; paid users no longer get 400 on first chat

### Ingest Connectors

- **Email IMAP** (`attune-core/src/ingest/email.rs`): full production — periodic incremental fetch + dedup + vault index
- **RSS** (`attune-core/src/ingest/rss.rs`): full production — feed polling + entry dedup + auto-index
- **WebDAV** refactor: periodic incremental sync worker + encrypted remote config persistence

### Quality & Reliability

- Agent ENFORCE gate: 6-class floor (≥10 golden / ≥3 proptest / ≥5 boundary / ≥3 error fixture / ≥1 E2E subprocess / regression fixture) — 0 violations across all 4 OSS agents + office helper
- E2E suite: `tests/e2e/run_all.sh` 49 PASS / 3 WARN / 0 FAIL
- Frontend E2E: `tests/e2e/playwright/run_ui_all.sh` 45 PASS / 0 FAIL
- CLI smoke gate: all 29 subcommands verified end-to-end
- Real OCR + ASR verification: 5+5 real samples end-to-end (desensitized, in `tests/golden/office/`)
- Tauri auto-updater CI infra: AppImage (Linux) and NSIS (Windows) bundles signed with `TAURI_SIGNING_PRIVATE_KEY`

### Bug Fixes

- `fix(cli)`: `vault-import` false "vault.db exists" error (#61)
- `fix(ocr)`: gender label/value serialization + `amount_total` thousands separator (#62)
- `fix(core)`: expose `parse_llm_terms` as pub, remove drifted local copy (#77)
- `fix(chat)`: LLM upstream error status mapping (5xx → 503, 4xx → correct forwarding)
- `fix(skill-evolution)`: CJK script normalization + dedupe in LLM expansion

---

## Breaking Changes (v0.7.x → v1.0.0)

- **attune-pro plugins now require `attune >= 1.0.0`** — install or upgrade attune to v1.0.0 before loading pro plugins
- **Pepper / key derivation**: if you migrated vault during v0.6 beta using a non-standard pepper, see `docs/migration-pepper.md` for the one-time re-derive step (affects fewer than 100 early beta users)
- No other API or schema breaking changes from v0.7.x

---

## Migration (v0.7.x → v1.0.0)

```bash
# 1. Backup your vault (always recommended before upgrade)
attune export --output ~/attune-backup-$(date +%Y%m%d).zip

# 2. Uninstall old desktop (optional — installer will overwrite)
#    Linux deb: sudo apt remove attune
#    Windows:   Control Panel → Programs → Uninstall Attune

# 3. Install new package (see Downloads above)
#    Linux: sudo dpkg -i attune_1.0.0_amd64.deb
#    Windows: run Attune_1.0.0_x64-setup.exe

# 4. Launch and unlock vault with your existing password
#    The vault schema auto-migrates on first unlock (< 2 seconds, non-destructive)

# 5. (Optional) Log in to Attune Cloud for Pro membership / LLM gateway
attune login your@email.com
attune sync-plugins   # if you have a law-pro or other pro entitlement
```

Server / CLI headless upgrade:

```bash
tar -xzf attune-v1.0.0-linux-x86_64.tar.gz
sudo mv attune-server /usr/local/bin/attune-server
sudo mv attune /usr/local/bin/attune
sudo systemctl restart attune  # if using systemd service
```

---

## Known Limitations

- `law-pro::defamation_extractor` LLM F1 = 0.72 (target ≥ 0.75) — prompt tuning + golden set expansion in v1.0.1
- Weak model matrix (#68) — gemma:2b / phi3:mini holdout validation deferred to v1.0.1
- **macOS Intel (`x86_64-apple-darwin`)** not in release artifacts — developers can `cargo build --release --target x86_64-apple-darwin`
- **Linux ARM64 desktop** `.deb` not in desktop-v1.0.0 artifacts — server/CLI tarballs cover ARM64 (e.g., K3 appliance, Raspberry Pi)
- `doc-audit.sh` 1 ERR + 3 WARN pending cleanup — v1.0.1 docs sprint
- Nightly real-LLM CI workflow not yet created — v1.0.1 CI improvement
- Telegram ingest connector: scaffold only (capture code present, frontend entry disabled) — v1.1.0

---

## Acknowledgments

Built with Rust, Axum, tantivy, usearch, hdbscan, whisper.cpp, and the broader Rust ecosystem. Thanks to all early testers and the open-source community.

---

## Documentation

- Wiki: https://wiki.engi-stack.com/attune
- Install guide: https://wiki.engi-stack.com/attune/install
- Source: https://github.com/qiurui144/attune
- Issues: https://github.com/qiurui144/attune/issues
- Changelog (full): see `rust/RELEASE.md` in the repository
