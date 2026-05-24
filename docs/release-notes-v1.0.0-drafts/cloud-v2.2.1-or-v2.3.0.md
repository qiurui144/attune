# Cloud Release Notes — v2.2.1 vs v2.3.0 Decision + Drafts

> **Decision**: The cloud commits since `cloud-v2.2.0-rc.1` split into two distinct release levels:
>
> - **`cloud-v2.2.1`** — Bug fixes only (GATEWAY_PUBLIC_URL + upstream chat error mapping). Small, safe, no migration needed.
> - **`cloud-v2.3.0`** — Architecture change (accounts/ + pluginhub/ symlink → git submodule). Breaking: requires `git submodule update --init --recursive` on existing deployments.
>
> Recommendation: ship `cloud-v2.2.1` first alongside attune v1.0.1, then `cloud-v2.3.0` when submodule migration is fully verified (planned ~2026-05-30).

---

---

# Cloud v2.2.1 — Bug Fixes (TBD, ~2026-05-27–31)

> Pairs with attune v1.0.1 / attune-pro v1.0.1.
> No breaking changes vs v2.2.0. Zero migration steps required.

## Bug Fixes

- **GATEWAY_PUBLIC_URL not respected** — The LLM gateway was building upstream request URLs from the internal Docker network hostname instead of `GATEWAY_PUBLIC_URL`. Clients calling the gateway from outside the Docker network (e.g., remote attune desktop clients) received TLS hostname mismatch errors or connection refused. `GATEWAY_PUBLIC_URL` env var is now read at startup and used for all outbound URL construction. (commit `4076720`)
- **LLM upstream error pass-through** — Upstream provider errors (429 / 503 / 4xx) were being swallowed by the gateway proxy and returned as generic 500s, hiding the real cause from attune clients. Now transparently forwarded with original status code. (commit `9c43bde` / aligned with attune-server `37e0d85`)
- **Proxy upload size limit** — Default Nginx `client_max_body_size` was 1 MB, blocking file uploads > 1 MB through the gateway proxy. Increased to 50 MB to match attune server's ingest limit. (commit `4076720`)

## Improvements

- **Container image tags pinned** — All `docker-compose.yml` service images now use explicit digest-pinned tags instead of `:latest` for reproducible, auditable deployments. (commit `a033e63`)
- **ghcr.io CI publish workflow** — `cloud-accounts`, `wiki`, and `pluginhub` images are now automatically published to `ghcr.io/qiurui144/` on each push to `master`. Enables air-gap deployments and version-pinned rollouts. (commit `29c5d50`)
- **wiki-portal rebrand** — `attune-wiki-web` submodule URL and pointer updated to the new `wiki-portal` repository name. No functional change. (commit `c936cbf`)

## Test Coverage (inherited from v2.2.0)

All v2.2.0 tests passing. No new test targets in this patch.

## Compatibility

| attune client | cloud version | Status |
|---------------|--------------|--------|
| attune v1.0.x | cloud v2.2.1 | ✅ Recommended |
| attune v0.7.x | cloud v2.2.1 | ⚠️ Best-effort (v1.0 LLM gateway features unavailable) |

---

---

# Cloud v2.3.0 — Repo Split + Submodule Architecture (planned ~2026-05-30)

> **Breaking change**: `accounts/` and `pluginhub/` are now git submodules.
> Existing deployments must run `git submodule update --init --recursive` before `docker compose pull`.

## Highlights

- **5-repo matrix clarified**: cloud (orchestration) / attune-accounts (PRIVATE, membership + billing Django) / attune-pluginhub (plugin marketplace) / wiki-portal (Docusaurus docs) / attune-official-web (WordPress)
- **Submodule pinning**: cloud repo commits now lock exact versions of each sub-service; deployments are deterministic and reproducible
- **Independent CI/release per service**: each sub-repo has its own CI pipeline and changelog; cloud repo is pure orchestration
- **ARCHITECTURE.md D7 ADR**: 5-repo split decision recorded with rationale

## Breaking Changes

- `git submodule update --init --recursive` required after `git pull` for existing deployments
- `accounts/` directory now empty in a fresh clone without `--recurse-submodules`

## Migration (v2.2.x → v2.3.0)

```bash
git pull origin master
git submodule update --init --recursive
docker compose pull
docker compose up -d
```

## What Changed

| Area | Change | Commit |
|------|--------|--------|
| `pluginhub/` | symlink → git submodule | `5d95269` (Phase 3) |
| `accounts/` | inline Django source → git submodule `attune-accounts` (PRIVATE) | `92517cc` (Phase 2) |
| `ARCHITECTURE.md` | §2 D7 ADR; §3 rewritten as 5-repo matrix | `9cc0d38` |
| `RELEASE.md` | v2.3.0 section added | `9cc0d38` |

## Known Limitations

- `attune-accounts` repo starts as PRIVATE (post-GA evaluation for open-source)
- `llm-gateway` / `monitor` / `proxy` remain inline (no independent release cycle per ARCHITECTURE.md §3.1)

## Compatibility

| attune client | cloud version | Status |
|---------------|--------------|--------|
| attune v1.0.x | cloud v2.3.0 | ✅ Recommended |
| attune v0.7.x | cloud v2.3.0 | ⚠️ Best-effort |
