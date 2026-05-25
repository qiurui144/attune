# Security Policy

## Reporting a Vulnerability

Please **do not** open a public GitHub issue for security vulnerabilities.

Report via one of these private channels:

- **Email**: happyqiuqiu9604@gmail.com (subject: `[attune-security] <summary>`)
- **GitHub Security Advisory**: [Report a vulnerability](https://github.com/qiurui144/attune/security/advisories/new)

We will acknowledge within **7 days** and target a fix / disclosure within **60 days**
of the initial report (following responsible disclosure best practices).

## Automated Dependency Scanning

All pushes and pull requests to `main` / `develop` run:

- **`cargo audit`** (via `rustsec/audit-check`) — checks against the [RustSec Advisory Database](https://rustsec.org/)
- **`cargo deny`** — enforces license policy, bans `openssl-sys`, and warns on duplicate crate versions

Configuration: `rust/deny.toml`

## Cryptographic Stack

attune's Rust backend uses **pure-Rust cryptography only** — no system OpenSSL:

| Primitive | Crate | Notes |
|-----------|-------|-------|
| Password hashing | `argon2` | Argon2id, memory-hard KDF |
| Symmetric encryption | `aes-gcm` | AES-256-GCM AEAD |
| Secret zeroing | `zeroize` | All secrets implement `Zeroize` |
| TLS | `rustls` | Pure-Rust TLS 1.2/1.3 |
| HMAC / SHA | `hmac` + `sha2` | Used for TOTP and integrity checks |

The `openssl` and `openssl-sys` crates are explicitly banned in `deny.toml`.

## Known Exemptions

No active exemptions. Any future `[[advisories.ignore]]` entries in `deny.toml`
must be documented here with:

- RUSTSEC advisory ID
- Affected crate + version range
- Reason patch is not yet applied
- Target resolution date
