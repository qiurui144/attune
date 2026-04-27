# OSS × Pro Strategy Framework — Attune

> Status: **v1, 2026-04-27** (W4 deliverable). Living document — review every quarter or
> when a major decision changes.
>
> Audience: Attune Contributors (decision-makers), Pro plugin developers, partners
> evaluating commercial integration.
>
> Companion docs: `docs/v0.6-release-readiness.md` (release planning) ·
> `docs/superpowers/specs/2026-04-25-industry-attune-design.md` (industry-vertical design) ·
> `attune-pro/docs/license-key-design.md` (license backend) ·
> `attune-pro/docs/versioning.md` (cross-repo version policy).

---

## 1. Why this document exists

Attune is two repos that ship together:

- **`attune`** (this repo, Apache-2.0) — universal RAG engine, encrypted vault, plugin
  framework, Chrome extension, desktop app.
- **`attune-pro`** (private, Proprietary) — vertical industry plugins (law-pro,
  presales-pro, …) and commercial services (cloud-sync, plugin-registry, llm-proxy).

The infrastructure for separation is in place (Cargo git-tag dependencies, Ed25519
plugin signing, `.attunepkg` package format, 5-tier license keys). What was
**missing** until this document: a clear policy answering *"what goes where, why, and
when"* — so contributors don't accidentally backport commercial code, and so paying
users see a coherent value ladder.

This is that policy.

---

## 2. Current state (baseline)

### 2.1 Repo split

| Repo | License | Visibility | Purpose |
|------|---------|------------|---------|
| `attune` | Apache-2.0 | public | Core engine + 4 builtin "basic" plugins (tech / law / presales / patent) + desktop + Chrome extension |
| `attune-pro` | Proprietary | private | Vertical pro plugins (law-pro, presales-pro, more planned) + commercial services |

### 2.2 Cross-repo binding

`attune-pro` workspace pins `attune-core = { git = "...", tag = "v0.X.Y" }`. Each public
release tag is followed by a Pro-side compatibility PR (per `attune-pro/docs/versioning.md`).
**Never backport commercial code into the public repo.** If a Pro feature ever becomes
open source, it is rewritten cleanly in `attune`.

### 2.3 What's open in v0.6.0

Everything shipped through W3+W4 is open source: hybrid RAG, J1 path-prefix chunker,
J3 explicit min_score, J5 strict prompt + confidence + secondary retrieval, B1 citation
breadcrumb, F2 sidecar transparency, C1 web-search cache, G1 browse capture + G5 privacy
panel, G2 auto-bookmark staging, A1 memory consolidation, H1 resource governor, K2 parse
golden set, MCP outlet shim, RAGAS benchmark harness, plugin marketplace toggle, profile
topic distribution. **No basic functionality is gated behind a paywall.**

---

## 3. The three core decisions

### Decision 1 — Feature-gate philosophy: **Thick OSS-core**

| Model | Examples | Why we don't pick it |
|-------|----------|----------------------|
| Open-core "thin" | GitLab CE/EE, Sentry self-hosted | Deliberately crippling OSS to drive sales conflicts with Attune's "private-first" promise; community resentment risk is high |
| Open-source + Cloud SaaS | Plausible, Cal.com, Supabase | `CLAUDE.md` already vetoed running a SaaS mirror ("不做 SaaS 镜像") — staying focused on the open + plugin ecosystem instead |
| **Thick OSS-core** ✅ | Bitwarden, Standard Notes, Plex | OSS is fully-featured for individuals; Pro monetizes verticals (law/presales/medical) and enterprise services (sync, registry, LLM proxy, hardware) |

**Operational rule:**

> Anything a single individual user would want from a personal knowledge companion stays
> open. Pro adds value through (a) deep industry-specific tooling and (b) services that
> only make sense for teams or that require infrastructure operating cost.

This is the load-bearing principle. Every future feature decision runs through it.

### Decision 2 — Builtin "basic" plugins stay open as upgrade path

Today `attune` ships 4 builtin plugins (`tech` / `law` / `presales` / `patent`) — light
classification + dimension prompts. `attune-pro` has `law-pro` and `presales-pro` with
deep capabilities (5 capabilities for law-pro alone: contract review, clause lookup,
drafting, OA reply, risk matrix).

**Rule:** keep the basic plugins in `attune`; rename them in user-facing copy to
"basic" so the Pro version is positioned as an *upgrade path*, not a replacement.

| Builtin (OSS) | Pro counterpart | Pro differentiation |
|---------------|-----------------|---------------------|
| `tech` (basic classification + code chunker) | `tech-pro` (planned) | Repo scanning, GitHub PR auto-review, IDE integration |
| `law` (legal taxonomy + dictionary) | `law-pro` ✅ | 5 capabilities: contract review · risk matrix · drafting assistant · OA reply · clause lookup |
| `presales` (sales taxonomy) | `presales-pro` ✅ | 4 capabilities: competitor analysis · BANT scoring · quotes · demo scripts |
| `patent` (patent taxonomy + simple search) | `patent-pro` (planned) | Direct patent DB integration · infringement detection · application drafting |
| _(none yet)_ | `medical-pro` (planned) | Medical terminology · case templates · literature tracking |
| _(none yet)_ | `academic-pro` (planned) | Citation graphs · paper-writing assistant · reading-list curation |

**Why not delete the basic plugins from OSS?** Two reasons:
1. The OSS user gets a *complete* experience out of the box — no "your dimension list is
   empty, please subscribe" feeling.
2. Industry classification quality on basic plugins demonstrates the engine's depth and
   becomes the credibility ladder to Pro: *"if the basic plugin is already this good,
   imagine the Pro one."*

### Decision 3 — Monetization: 5 subscription tiers + hardware

Aligns with `attune-pro/docs/license-key-design.md` (5 plans already designed in license
key payload: `lite` / `pro` / `pro_plus` / `team` / `enterprise`).

| Plan | Price | Includes | Target user |
|------|-------|----------|-------------|
| **Lite** | ¥0 (OSS) | All of `attune`, all 4 builtin basic plugins, MCP outlet, browser extension | Individual users, developers, evaluators |
| **Pro** | ¥99 / year | Lite + **one** vertical pack (e.g. law-pro), single device | Solo lawyer, solo presales engineer |
| **Pro+** | ¥299 / year | Lite + **all** vertical packs + cloud-sync, 3 devices | Cross-discipline freelancers, power users |
| **Team** | ¥999 / month, per-seat | Pro+ + plugin-registry (private plugins) + audit log + team collaboration | Small-to-mid law firms, presales teams (5–50 seats) |
| **Enterprise** | Custom (annual) | Team + SSO + on-prem deployment + SLA + industry consulting | Large firms, hospitals, universities (50+ seats) |
| **K3 appliance** | ¥6,999+ (hardware + 1y Pro+) | Hardware + bundled local LLM + on-site setup + remote support | Industries that won't install software (small clinics, traditional law firms) |

**Pricing-anchor rationale:**
- ¥99/year Pro for a lawyer = ~1 hour/week of contract-review savings ⇒ 5× ROI
  (lawyer hourly rate ¥500–2000)
- ¥6,999 K3 = price of an office laptop ⇒ approachable as new-firm capital outlay
- ¥999/month Team starting at 5 seats = ¥200/seat/month ⇒ within SaaS norms for SMB
  professional tools
- Lite stays free forever — no time-bombed trial, no nag screens. Lite users are the
  funnel and the long-tail community.

---

## 4. Feature-gate boundary (single source of truth)

When unsure where a new feature belongs, this table is the answer. Update it when a
decision changes; everyone references it.

### 4.1 OSS scope (`attune` repo, Apache-2.0)

| Domain | Feature | In OSS? |
|--------|---------|---------|
| Storage | DEK + AES-256-GCM vault, Argon2id KDF, sidecar table pattern | ✅ |
| Indexing | Hybrid BM25 + vector + RRF, J1 path-prefix chunker, J3 explicit min_score, K2 parse golden | ✅ |
| Generation | RAG chat, J5 strict prompt + confidence + secondary retrieval | ✅ |
| Memory | A1 episodic memory consolidation | ✅ |
| Resource | H1 governor with 3 profiles + topbar pause + per-task throttle | ✅ |
| Citation | B1 citation deep-link, F2 breadcrumb sidecar with at-rest encryption | ✅ |
| Browser | G1 generic browse capture + opt-out + HARD_BLACKLIST + G5 privacy panel + G2 auto-bookmark staging | ✅ |
| Web | C1 web-search cache + DELETE/GET routes (W4-002) | ✅ |
| Plugin framework | plugin.yaml, dimension schema, builtin loader, marketplace toggle (W4 E1) | ✅ |
| Profile | Topic distribution API (W4 F1), import/export | ✅ |
| Builtin plugins | tech / law / presales / patent — basic dimensions + light classification | ✅ |
| Distribution | Tauri desktop (Linux deb/AppImage, Windows MSI/NSIS), Chrome extension | ✅ |
| MCP integration | Python stdio shim (`tools/attune_mcp_shim.py`) wrapping REST | ✅ |
| Quality | RAGAS-style benchmark harness + bilingual methodology doc | ✅ |
| Documentation | README / DEVELOP / RELEASE / TESTING / ACKNOWLEDGMENTS — bilingual EN + zh | ✅ |
| Bilingual everything | All public docs ship `<NAME>.md` + `<NAME>.zh.md` | ✅ |

### 4.2 Pro scope (`attune-pro` repo, Proprietary)

| Domain | Feature | Tier required |
|--------|---------|---------------|
| Vertical plugins | `law-pro` (5 capabilities: contract review / risk matrix / drafting / OA / clause lookup) | Pro |
| Vertical plugins | `presales-pro` (4 capabilities: competitor / BANT / quote / demo script) | Pro |
| Vertical plugins | `medical-pro`, `academic-pro`, `patent-pro`, `tech-pro` (planned) | Pro |
| Multi-vertical | All vertical packs in one license | Pro+ |
| Sync service | `cloud-sync` — DEK never leaves device, only encrypted blobs synced | Pro+ |
| Plugin marketplace | `plugin-registry` — signed third-party plugin distribution + private internal plugins | Team |
| LLM gateway | `llm-proxy` — hosted gateway (Anthropic / OpenAI / Qwen) with team usage cap & audit | Team |
| Compliance | Audit log (every vault access logged with user/time/scope) | Team |
| Identity | SSO (SAML / OIDC) | Enterprise |
| Deployment | On-prem deployment with private installer + air-gap support | Enterprise |
| Support | Industry consulting, custom prompt tuning, dedicated CSM | Enterprise |
| Hardware | K3 appliance OS image with bundled Qwen 1.5B + on-site setup + remote support | K3 SKU |

### 4.3 Decision rules for new features

When a contributor proposes a feature, ask in this order:

1. **Does it benefit a solo individual user?** If yes, candidate for OSS.
2. **Does it require centralized infrastructure (hosted service, billing, multi-tenant)?**
   If yes, it goes Pro regardless of who could use it.
3. **Is it a vertical-specific deep capability (industry RPA, compliance template,
   domain-specific taxonomy)?** If yes, it's a Pro plugin.
4. **Is it a single-org operational concern (SSO, audit, on-prem)?** Pro Team or Enterprise.
5. **None of the above?** Default to OSS — be biased toward openness.

---

## 5. Six-month roadmap

| Milestone | Weeks | Goal | OSS side | Pro side |
|-----------|-------|------|----------|----------|
| **M1** | now → +2 | OSS v0.6.0 GA | rc.1 (today) → soak 7 days → GA | bump cargo dep tag = v0.6.0; smoke-test law-pro against new attune-core |
| **M2** | +3 → +4 | law-pro on new attune | maintenance only (W4 followups #1-#5) | All 5 law-pro capabilities consume J5 confidence + breadcrumb sidecar; plugin-build pipeline auto-signs `.attunepkg` |
| **M3** | +5 → +8 | Commercial v1 launch | maintenance + W5 K1 sleeptime / A2 conflict detection start | License key backend up (Ed25519 + offline verify); subscription page (Lite ¥0 / Pro ¥99 / Pro+ ¥299) live; 10–30 lawyer seed users |
| **M4** | +9 → +16 | K3 appliance v1 | maintenance + W7-8 plugin SDK bilingual + CRDT prep | K3 OS image bundles attune + Qwen 1.5B; presales workflow + on-site setup SOP; first batch of 10 hardware customers |
| **M5** | +17 → +24 | cloud-sync + plugin registry | maintenance + W9-10 K3 items keys (per Standard Notes 004 spec) | Encrypted sync backend (DEK never leaves user device); internal plugin marketplace beta |

**Coupling rule:** Pro releases lag OSS releases. Never ship a Pro feature that
requires an unreleased OSS API. The cross-repo version matrix in
`attune-pro/docs/versioning.md` is the contract.

---

## 6. Risks and mitigations

| Risk | Severity | Mitigation |
|------|----------|------------|
| OSS too good — cannibalizes Pro revenue | Medium | OSS is universal personal use; Pro is industry-vertical + services. A lawyer can install OSS for personal notes *and* subscribe to law-pro for contract review. They don't compete. |
| Pro plugin breaks against new OSS API | Medium | `versioning.md` enforces Pro pinning to OSS tag; OSS API changes trigger a Pro compatibility PR before the public OSS release ships. |
| Apache-2.0 vs AGPL faction debate | Low | Keep Apache-2.0 for now. If a free-rider commercial fork emerges, evaluate dual-license (Apache-2.0 + Commercial) — but don't pre-emptively constrain. |
| Pro value prop too weak — users don't pay | **High** | law-pro must demonstrate 3× ROI. W4 J6 published RAG numbers are the weapon: not just "law-pro better than law-basic", but "law-pro published vs same-corpus competitor baseline." |
| China + Western dual market | Medium | Bilingual docs already in place. China-first verticals: lawyer / presales (existing RPA + Chinese legal corpus). Western-first: academic-pro / medical-pro (richer English corpora). |
| K3 appliance support cost spirals | Medium | Define SLA + remote support tooling before M4 first shipment. Cap to 10 units/month initially to keep ops manageable. |
| License key piracy / sharing | Medium | License key has device fingerprinting (per `license-key-design.md`); public revocation list; cloud-sync usage anomaly detection in M5. |
| Backporting commercial code accidentally | High | CI rule (planned for M2): block any `git diff` between `attune` and `attune-pro` that copies non-test files verbatim. Code reviewers check against this rule. |
| OSS contributor burnout (no clear monetization path back) | Medium | Maintainer compensation comes from Pro revenue; bounty program for OSS contributions funded by Pro margin starting M3. |

---

## 7. License evolution policy

**Now (v0.6 → v1.0):** Apache-2.0 for `attune`, Proprietary for `attune-pro`. Simple,
clean, works for the current strategy.

**Future triggers that would justify changing OSS license:**

| Trigger | Possible response |
|---------|-------------------|
| Free-rider commercial fork at scale (e.g. Amazon-style "managed Attune") | Dual-license: Apache-2.0 for community + Commercial for revenue-generating SaaS use |
| Need to enforce contributions back (e.g. major corp-funded fork) | Switch to AGPL — but only for greenfield code, never re-license existing community contributions |
| Move toward stronger network-effect features (cloud-sync, plugin registry growing organically) | Keep Apache-2.0; lean on Pro service moats instead of license restrictions |

**What we explicitly will not do:**
- Switch to BUSL / SSPL / Elastic License style "source-available but not OSS" licenses.
  These poison community trust and Attune's whole positioning depends on that trust.
- Re-license existing community contributions retroactively.
- Add an "additional restrictions" clause beyond Apache-2.0.

---

## 8. Plugin SDK contract (for third-party developers)

This is what a third-party plugin developer needs to know:

- Build against `attune-core`'s public API at a specific tag (start with v0.6.0).
- Plugin manifest = `plugin.yaml` + optional `prompt.md` + Rust crate (or pure prompt).
- Distribution: signed `.attunepkg` artifacts (Ed25519). Self-distribution allowed; the
  Pro `plugin-registry` is one optional distribution channel, not the only one.
- License: your choice. Plugins under MIT/Apache/GPL are fine. Plugins requiring a
  paid license can use the Attune license key system (M5+) or roll their own.
- Revenue share (Pro `plugin-registry` only, M5+): 70% to plugin author, 30% to
  Attune for hosting + signing + payment processing. (Subject to change before launch.)
- Contributor License Agreement (CLA) is *not* required for OSS contributions to
  `attune` — only for commercial plugins distributed via `attune-pro`.

---

## 9. Open questions (defer until needed)

| Question | Defer because | Revisit by |
|----------|---------------|------------|
| Should we accept VC funding to accelerate K3 hardware? | Premature — bootstrap M1-M3 first to learn unit economics | M4 (after first 10 K3 sales) |
| Should `cloud-sync` be a separate `attune-cloud` repo? | Adds repo overhead without benefit at current scale | When `attune-pro/services/` exceeds 5 services |
| Should we publish a "Pro-equivalent" community plugin set as social good? | Hurts revenue; deflates Pro upgrade path | Only if Pro hits ¥10M ARR and we can afford giving back |
| Should Lite users get *some* sync (e.g. 1 device free, 3 device Pro+)? | Sync infra cost > Lite acquisition value at current scale | Revisit at 100k Lite MAU |
| Mobile apps (iOS / Android) | Roadmap silent — Tauri 2.0 mobile is immature | When Tauri mobile reaches stable + first-party storage primitives |

---

## 10. Owners

| Area | Owner | Cadence |
|------|-------|---------|
| OSS release cadence | Attune Contributors maintainers | Per release (semver) |
| Pro plugin release | Pro plugin author | Independent semver per plugin |
| License key backend | Pro infrastructure team | Continuous deployment after M3 |
| Pricing changes | Attune Contributors core team | Reviewed quarterly; published 30 days ahead |
| Strategy framework (this doc) | Attune Contributors core team | Reviewed quarterly; major revisions noted at top |

---

## 11. Decision log

| Date | Decision | Status |
|------|----------|--------|
| 2026-04-25 | Industry-vertical first cut: lawyer | Active (CLAUDE.md, industry-attune-design.md) |
| 2026-04-25 | LLM not bundled in installer; remote token default; K3 may bundle local LLM | Active (CLAUDE.md cost & trigger contract) |
| 2026-04-25 | Platform priority: Windows P0 → Linux P1 → macOS deferred | Active (CLAUDE.md) |
| 2026-04-27 | Browser extension = generic browse-state knowledge source (not just AI chat) | Active (W3 batch B shipped) |
| 2026-04-27 | Resource governance baseline: every background task throttled (H1) | Active (W3 W1 shipped) |
| 2026-04-27 | Bilingual docs mandatory for all public-facing material | Active |
| **2026-04-27** | **OSS-Pro split = Thick OSS-core (this document) ratifies prior architecture** | **Active (this doc v1)** |
| **2026-04-27** | **Pricing: ¥99 / ¥299 / ¥999/mo / Custom + ¥6,999 K3** | **Proposed (this doc v1)** |

---

## Quick links

- `attune` repo: https://github.com/qiurui144/attune (Apache-2.0)
- `attune-pro` repo: private (request access)
- This document (zh): `docs/oss-pro-strategy.zh.md`
- Release planning: `docs/v0.6-release-readiness.md`
- Industry design: `docs/superpowers/specs/2026-04-25-industry-attune-design.md`
- License key design: `attune-pro/docs/license-key-design.md`
- Cross-repo version policy: `attune-pro/docs/versioning.md`
