# attune Desktop W1 allowlist + SPKI cert-pinning (cloud slice8 §5.6.1)

**Date**: 2026-06-10
**Worktree branch**: `worktree-agent-a0c5bf063af908ac9` (base develop @ `9201676`)
**Scope**: attune-core only. No cloud / attune-pro touched. No push, no tag (slice8 is HELD).

## 1. Contract found — and it is unambiguous

The attune-side contract lives in the **cloud** repo (slice8 is cloud-owned, contract delegated to attune):

- `/data/company/cloud/docs/superpowers/specs/2026-06-03-cloud-security-终态-mtls-certpin.md`
  - **§5.6.1** desktop W1-A/B trust-anchor allowlist contract (the exact Rust shape)
  - **§3.2** SPKI cert-pin data flow + `ACCOUNTS_SPKI_PINS` constant + pin format
  - **§5.5** pin management (≤3 pins, rotation)
  - **§7.1** error code table (`pin-mismatch`, `anchor-not-pinned`)
  - **§10.3 / §11 R1** dual-pin rotation + the "换 CA 全量断线" catastrophe to avoid
  - **§12** già-landed inventory (W1-A/B desktop = attune-side, this task)
- W1-C SSOT anchor value confirmed in `cloud/accounts/accounts/config.py:110` →
  `8866ae9b8f0026aaa99902a34fa06223b5e88d5a8f933c7f084342cb9953bcac` (law-pro publisher key).

Every security-critical parameter is pinned by the spec: **SPKI pin format** =
`base64(sha256(DER(SubjectPublicKeyInfo)))`; **host** = `accounts.engi-stack.com`;
**allowlist semantics** = W1-B cross-check `signing_pubkey_hex ∈ OFFICIAL_PLUGIN_ANCHORS`
*before* `verify_with_key`; **anchor gate = fail-closed**; **SPKI pin = provisioned at
release** (empty in source → falls back to std webpki, §10.3 — not fail-open since chain
validation still runs). → **Implemented, not escalated.**

## 2. Implemented

| File | Change |
|------|--------|
| `crates/attune-core/src/plugin_anchor.rs` (new) | W1-A `OFFICIAL_PLUGIN_ANCHORS` const (read-only segment, NOT vault) + `is_official_anchor()` (case-fold, fail-closed). 7 tests. |
| `crates/attune-core/src/cert_pin.rs` (new) | `ACCOUNTS_SPKI_PINS` const (empty as-shipped, ≤3) + `spki_pin_of_cert_der()` (x509-cert) + `SpkiPinVerifier` (wraps std `WebPkiServerVerifier` — chain validation runs FIRST, pin is an *additional* constraint, never a downgrade) + `pinned_client_config()`. 8 tests. |
| `crates/attune-core/src/cloud_client.rs` | `CloudClient::new` now builds the blocking client with `use_preconfigured_tls(pinned_client_config())` — every accounts call is pinned. |
| `crates/attune-core/src/plugin_sync.rs` | W1-B: `verify_plugin_anchor(ep)` called in `install_one_plugin` **before** `verify_with_key`; off-allowlist key → refuse install, surfaced via `SyncReport.failed`. 5 new tests. |
| `crates/attune-core/src/error.rs` | `VaultError::AnchorNotPinned(String)` + `PinMismatch(String)` (kebab Display → §7.1 codes). |
| `crates/attune-core/Cargo.toml` | `x509-cert 0.2` (pure-Rust RustCrypto, der/spki/const-oid already resolved; WASM-safe; lighter than x509-parser). |
| `src/testdata/cert_pin/{leaf_a,leaf_b}.der` + `PINS.txt` | Two real self-signed P-256 leaf certs (no key kept) with openssl-computed SPKI pins. |

### Key design decisions (security-critical)

1. **Pin is additive, never a downgrade.** `SpkiPinVerifier` delegates to rustls'
   `WebPkiServerVerifier` for full chain + hostname + validity first; only then enforces
   `leaf SPKI ∈ pins`. A pin-only verifier (skipping chain validation) would be *weaker*
   than normal TLS — explicitly avoided.
2. **SPKI empty-set ⇒ pinning disabled, deferring to webpki (§10.3 fail-safe).** The
   production pin is provisioned at *release time* (`openssl s_client …`, CI pin-verify
   step in `desktop-release.yml`), not in source — shipping a placeholder pin would match
   nothing and brick every client (§11 R1). This fall-back is the *as-shipped* state and
   is no weaker than today's unpinned client. **This applies ONLY to the SPKI pin.**
3. **W1 anchor gate is fail-closed and always non-empty** (the law-pro anchor is baked in).
   Empty/blank/off-allowlist key → reject. This is the only control that defends against a
   *compromised accounts server* (a valid TLS endpoint cert-pin can't catch).
4. **Cross-check ordering** — anchor (trust-root) check runs *before* signature math, so a
   forged package signed by an attacker key never reaches `verify_with_key` (which would
   otherwise "verify" successfully against the attacker's own key).

## 3. Test evidence (cargo test, attune-core)

```
cert_pin::      8 passed; 0 failed   (incl. rust_spki_extraction_matches_openssl_pin ★)
plugin_anchor:: 7 passed; 0 failed
plugin_sync::  18 passed; 0 failed   (13 orig + 5 new W1-B)
cloud_client:: 22 passed; 0 failed
error::        11 passed; 0 failed
```

★ The load-bearing test: our pure-Rust SPKI extraction equals the `openssl`-computed pin
byte-for-byte for both fixtures — so a pin baked from the CI openssl command will match the
live extraction (otherwise all clients would brick).

**Full lib suite**: `1518 passed; 1 failed`. The single failure is
`scanner::tests::create_watcher_works` — **environmental, not a regression**: it fails with
`"OS file watch limit reached"` (host inotify watch budget exhausted by other processes; the
user holds 152 inotify instances and `max_user_watches` is saturated). It fails identically
in isolation, touches `scanner.rs` (untouched by this task), and is independent of every file
changed here.

**Clippy**: `cargo clippy -p attune-core --all-targets -- -D warnings` → clean (Finished, 0
warnings).

**Build**: `cargo build -p attune-core` → GREEN (`x509-cert v0.2.5` resolved).

## 4. Left for the controller / release

- **SPKI pin provisioning** is a *release-time* step, not source: extract the live
  `accounts.engi-stack.com` pin and add it to `ACCOUNTS_SPKI_PINS`, plus the CI pin-verify
  step in `desktop-release.yml` (§5.5). Until then pinning is inert (std webpki), by design.
- Merge this worktree branch into develop (do NOT push from here — develop is another
  agent's). No tag (slice8 HELD).
