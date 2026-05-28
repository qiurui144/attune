# attune cloud v2.2.0 — attune v1.0.0 GA Companion (2026-05-26)

> Cloud SaaS backend powering Attune Pro Membership, the LLM Gateway, and the Plugin Hub.
> Self-hostable reference implementation included in the OSS `attune-core` workspace.
> Client compatibility: **attune >= 1.0.0** (desktop / server / CLI all three forms).

---

## Highlights

- **Accounts service** — device-bound 1:2 license model + Stripe subscription lifecycle (subscribe / cancel / renew) + member email notifications; `entitled_plugins` field auto-feeds plugin auto-sync
- **LLM Gateway** — OpenAI-compatible unified protocol; upstream API key never exposed to client; token quota management; hot-reload into running attune-server after login
- **Plugin Hub** — encrypted plugin pack distribution + Ed25519 license verification + heartbeat + atomic install; compatible with attune-pro v1.0.0 law-pro pack
- **Full cross-repo E2E verified** — Playwright fullstack-e2e + payment-e2e green; Gatus endpoint health monitoring live
- **Reference implementation in OSS** — `attune-accounts` Rust crate + `attune-core::license` / `cloud_client` / `plugin_sync` / `llm` — self-host alternative, Apache-2.0

---

## Services in This Release

| Service | Stack | Function |
|---------|-------|---------|
| **accounts** | FastAPI + PostgreSQL + Alembic | User auth, device binding (1:2), license CRUD, Stripe webhooks, `entitled_plugins` sync |
| **pluginhub** | (soft-linked from lawcontrol hub) | Encrypted `.attunepkg` distribution, license validation, heartbeat events |
| **llm-gateway** | FastAPI | OpenAI-compatible proxy; upstream key vault; token quota; routes member requests to OpenAI / Anthropic / Gemini |
| **proxy** | Nginx | TLS termination, unified entrypoint, health checks, rate limiting |
| **monitor** | Gatus | Endpoint heartbeat dashboard; Prometheus + Grafana (in progress) |

---

## What's New (cloud-v2.1.x → cloud-v2.2.0)

### Accounts

- `entitled_plugins` JSON column (Alembic migration `0002_entitled_plugins`) — stores list of plugin IDs the member's subscription entitles them to
- Schema aligned with `attune_core::cloud_client::License::entitled_plugins` — login response populates the list; client's `attune sync-plugins` uses it to auto-install entitled packs
- `attune sync-plugins` flow: `GET /licenses/me` → `entitled_plugins` → download `.attunepkg` from pluginhub → Ed25519 verify → atomic install
- LLM auto-configure: login response includes `gateway_token` + `gateway_url`; attune-server's `apply_cloud_llm_if_needed` hot-reloads without restart
- Stripe subscription lifecycle webhook: subscribe / cancel / renew events update `is_active` + send member email (verified via mailpit)

### Plugin Hub

- Enforces `attune_min_version: 1.0.0` — packs uploaded for attune v1.0.0+ are blocked from download by older clients (old v0.x packs unaffected on serve)
- Whitelist ID validation + staging + rename atomic replace in `attune_core::plugin_sync::install_plugin_package`
- Compatible with attune-pro `v1.0.0` law-pro pack signing format

### LLM Gateway

- Vision content array support (multimodal prompts)
- Unit test: verified upstream API key is never forwarded to client (client only receives `gateway_token`)
- `attune_core::llm::LlmProvider` uses same OpenAI-compatible protocol to call this gateway

### OSS Reference Implementation (in attune-core workspace)

| Crate / Module | What it does |
|----------------|-------------|
| `attune-accounts` (Rust crate) | Reference accounts server — 10 unit tests; drop-in alternative for self-hosters |
| `attune-core::license` | `LicenseClaims` + Ed25519 offline verification (9 unit tests) |
| `attune-core::license_cache` | Persists license to `~/.config/npu-vault/license.json` (chmod 600) |
| `attune-core::cloud_client` | HTTP client → cloud accounts FastAPI (login / me / list_licenses / logout / sync-plugins) |
| `attune-core::plugin_sync` | Pull entitled plugins → download → verify_sig → atomic install |
| `attune-core::llm` | OpenAI-compatible unified protocol + `chat_multimodal` |

---

## End-to-End Flow (v1.0 GA Day Verification)

```bash
# --- Admin side ---
bash /data/company/cloud/cloud.sh en        # deploy 4 services
curl -X POST /admin/llm/configure \
     -d '{"provider":"openai","api_key":"sk-...","model":"gpt-4o-mini"}'
curl -X POST /admin/licenses/generate \
     -d '{"email":"user@example.com","plan":"pro"}'
# → returns license_code

# --- User side ---
attune login user@example.com \
     --cloud-url https://accounts.engi-stack.com
# enter license_code when prompted
attune sync-plugins                          # auto-installs entitled pro plugins
attune-server-headless                       # start server on :18900
# open http://127.0.0.1:18900 — plugins visible in Marketplace tab
```

Cross-repo E2E status: Playwright fullstack-e2e ✅ + payment-e2e ✅ (screenshots archived in attune repo at `docs/screenshots/payment-e2e/`).

---

## Breaking Changes

- `pluginhub` now rejects upload of plugin packs with `attune_min_version < 1.0.0` — only affects pack publishers, not end users downloading packs
- `accounts` API: `GET /licenses/me` response now includes `entitled_plugins` (array of plugin IDs) — old clients ignore the field; new clients (attune >= 1.0.0) use it for `sync-plugins`

---

## Migration

### cloud-v2.1.x → cloud-v2.2.0

```bash
# 1. Pull latest cloud repo
cd /data/company/cloud
git pull

# 2. Run database migration (adds entitled_plugins column)
cd accounts
alembic upgrade head
# applies: 0002_entitled_plugins

# 3. Restart services
bash cloud.sh restart
# or: docker compose restart accounts pluginhub llm-gateway proxy
```

Pluginhub soft-link path unchanged — zero migration on pluginhub side. Existing attune v0.7.x clients will continue to work (they ignore `entitled_plugins`).

---

## Known Limitations (cloud-v2.2.1 / cloud-v2.3 roadmap)

- **Monitoring / Alerting**: Prometheus metrics collection configured but Grafana dashboard not yet wired — cloud-v2.2.1
- **Multi-region deployment** (EN / CN / JP): single-region for v1.0 GA; multi-region design deferred to cloud-v2.3
- **Stripe production sandbox**: webhook flow verified with mailpit (dev); production Stripe sandbox end-to-end test deferred to cloud-v2.2.1
- OCI container images: 6 images published to `ghcr.io/qiurui144/` on first GA tag push; verify in GitHub Packages tab after tagging

---

## Version Pairing

| Product | Version | Notes |
|---------|---------|-------|
| attune (OSS) | v1.0.0 | client — desktop / server / CLI |
| attune-pro | v1.0.0 | plugin pack — law-pro |
| cloud | **cloud-v2.2.0** | this release — supports attune >= 1.0.0 |
| wiki-web / official-web | submodule (v1.0 content bumped) | deployed alongside cloud-v2.2.0 |

---

## Self-Hosting

Cloud services can be self-hosted using the Docker Compose setup in the `cloud` repository:

```bash
git clone https://github.com/qiurui144/attune-cloud  # private repo
bash cloud.sh up
```

Alternatively, use the `attune-accounts` Rust reference implementation (Apache-2.0) from the OSS `attune` repository for a minimal accounts server.

---

## Documentation

- Admin deploy guide: https://wiki.engi-stack.com/cloud/deploy
- Self-host accounts: https://wiki.engi-stack.com/cloud/self-host
- API reference: https://wiki.engi-stack.com/cloud/api
- Source (OSS reference impl): https://github.com/qiurui144/attune (see `rust/crates/attune-accounts/`)
