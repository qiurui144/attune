//! Outbound call gate. Every network egress (LLM / Cloud SaaS / WebDAV /
//! Web Search / Telemetry) MUST be wrapped by [`OutboundGate::enforce`] so
//! settings and PII redactor are consulted in **one** place.
//!
//! v1.0.6 Privacy Logic Strategy (per
//! `docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md` §3.1).
//!
//! ## Contract
//!
//! 1. **Disabled outbound** (`policy.enabled == false`) → returns
//!    [`OutboundError::Disabled`]. Caller MUST NOT make the network request.
//! 2. **Locked vault** → returns [`OutboundError::VaultLocked`]. (Telemetry is
//!    exempt because telemetry payloads contain no vault data; see field
//!    `policy.requires_vault`.)
//! 3. **Payload contains PII but no redactor installed** → returns
//!    [`OutboundError::RedactorRequired`]. We **fail closed** rather than send
//!    PII unredacted.
//! 4. **All checks pass** → returns the **redacted** payload, never the
//!    original. Caller passes the returned string to the wire.
//!
//! ## Why a struct gate and not a free function?
//!
//! Today we have a single `enforce()` static method, but the struct gives us a
//! stable type-name for future audit hooks (e.g. emit a `record_outbound`
//! audit event from inside the gate so every wrap point is auto-logged).

use crate::pii::Redactor;

/// Discriminator over the 5 outbound kinds tracked by Privacy Logic Strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutboundKind {
    /// Cloud LLM endpoints (`chat.rs` / Cloud LLM gateway / BYOK provider).
    Llm,
    /// Attune Cloud SaaS (`cloud_client.rs` — accounts / billing / DSAR).
    CloudSaas,
    /// User-configured WebDAV remote sync (`sync/webdav.rs`).
    Webdav,
    /// Browser-driven web search (`web_search_browser.rs`).
    WebSearch,
    /// Diagnostic telemetry (default-off; `telemetry.rs`).
    Telemetry,
}

impl OutboundKind {
    /// Stable kebab-case label used in audit log + UI strings.
    pub fn as_str(self) -> &'static str {
        match self {
            OutboundKind::Llm => "llm",
            OutboundKind::CloudSaas => "cloud_saas",
            OutboundKind::Webdav => "webdav",
            OutboundKind::WebSearch => "web_search",
            OutboundKind::Telemetry => "telemetry",
        }
    }
}

/// Reasons the gate may refuse an outbound call.
#[derive(Debug, thiserror::Error)]
pub enum OutboundError {
    /// User has disabled this outbound point in privacy settings.
    #[error("outbound-disabled: user has disabled {0:?} in privacy settings")]
    Disabled(OutboundKind),
    /// Vault is locked / sealed; non-telemetry outbound requires unlocked vault.
    #[error("vault-locked: outbound requires unlocked vault")]
    VaultLocked,
    /// Payload contained PII but no redactor was installed → fail closed.
    #[error("redactor-required: payload contained PII and redactor is unavailable")]
    RedactorRequired,
}

/// Per-call policy snapshot. Build once at the callsite, pass to
/// [`OutboundGate::enforce`].
pub struct OutboundPolicy<'a> {
    pub kind: OutboundKind,
    /// Whether the user has enabled this outbound point in privacy settings.
    pub enabled: bool,
    /// Whether the vault is currently unlocked. Telemetry ignores this
    /// (payload has no vault data) — see [`OutboundGate::enforce`].
    pub vault_unlocked: bool,
    /// Optional redactor. If `None` and `payload` contains PII, the gate
    /// fails closed with [`OutboundError::RedactorRequired`].
    pub redactor: Option<&'a Redactor>,
}

/// The single outbound enforcement entry-point.
pub struct OutboundGate;

impl OutboundGate {
    /// Enforce all four contract clauses (disabled / vault / redactor / redact).
    ///
    /// Returns the **redacted** payload to be sent to the wire — never the
    /// original. Caller is responsible for using the returned string.
    pub fn enforce(policy: &OutboundPolicy<'_>, payload: &str) -> Result<String, OutboundError> {
        // 1) Disabled by user → refuse.
        if !policy.enabled {
            return Err(OutboundError::Disabled(policy.kind));
        }

        // 2) Vault-locked → refuse (except for telemetry, which doesn't carry
        //    vault data and never needs vault unlocked).
        if policy.kind != OutboundKind::Telemetry && !policy.vault_unlocked {
            return Err(OutboundError::VaultLocked);
        }

        // 3) Redact PII (fail closed if payload appears to contain PII but no
        //    redactor is installed).
        let Some(redactor) = policy.redactor else {
            // No redactor: only allow if payload has zero PII signal.
            // Heuristic: short whitelist — empty payload always OK, otherwise
            // require explicit redactor. This is intentionally strict.
            if payload.is_empty() {
                return Ok(String::new());
            }
            // If we can't redact, fail closed. PII detection here would be a
            // duplicate of Redactor's own logic; safer to require the redactor
            // even when payload happens to be PII-free.
            return Err(OutboundError::RedactorRequired);
        };

        let result = redactor.redact(payload);
        Ok(result.redacted_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pol(kind: OutboundKind, enabled: bool, vault_unlocked: bool, with_redactor: bool) -> OutboundPolicy<'static> {
        // Leak a Redactor so we can return a 'static reference (test only).
        let redactor: Option<&'static Redactor> = if with_redactor {
            Some(Box::leak(Box::new(Redactor::new())))
        } else {
            None
        };
        OutboundPolicy {
            kind,
            enabled,
            vault_unlocked,
            redactor,
        }
    }

    #[test]
    fn disabled_outbound_is_refused() {
        let p = pol(OutboundKind::Llm, false, true, true);
        match OutboundGate::enforce(&p, "hello") {
            Err(OutboundError::Disabled(OutboundKind::Llm)) => {}
            other => panic!("expected Disabled(Llm), got {other:?}"),
        }
    }

    #[test]
    fn vault_locked_blocks_llm() {
        let p = pol(OutboundKind::Llm, true, false, true);
        assert!(
            matches!(OutboundGate::enforce(&p, "hello"), Err(OutboundError::VaultLocked)),
            "vault-locked LLM call must be refused"
        );
    }

    #[test]
    fn vault_locked_does_not_block_telemetry() {
        // Telemetry carries no vault data → vault-locked must NOT refuse it.
        let p = pol(OutboundKind::Telemetry, true, false, true);
        // Empty payload is fine; just verify it's not VaultLocked.
        let out = OutboundGate::enforce(&p, "");
        assert!(out.is_ok(), "telemetry must not be vault-gated; got {out:?}");
    }

    #[test]
    fn missing_redactor_for_non_empty_payload_returns_err() {
        let p = pol(OutboundKind::Llm, true, true, false);
        assert!(
            matches!(
                OutboundGate::enforce(&p, "phone 13800138000"),
                Err(OutboundError::RedactorRequired)
            ),
            "must fail closed when payload non-empty + redactor absent"
        );
    }

    #[test]
    fn missing_redactor_for_empty_payload_is_ok() {
        let p = pol(OutboundKind::Llm, true, true, false);
        assert_eq!(OutboundGate::enforce(&p, "").unwrap(), "");
    }

    #[test]
    fn redactor_replaces_phone_before_leaving() {
        let p = pol(OutboundKind::Llm, true, true, true);
        let out = OutboundGate::enforce(&p, "联系电话 13800138000 请回拨").unwrap();
        assert!(
            !out.contains("13800138000"),
            "phone must be redacted; got: {out}"
        );
        assert!(
            out.contains("[PHONE_") || out.contains("PHONE_"),
            "placeholder expected; got: {out}"
        );
    }

    #[test]
    fn enabled_with_redactor_passes_clean_text_unchanged() {
        let p = pol(OutboundKind::Llm, true, true, true);
        let out = OutboundGate::enforce(&p, "hello world").unwrap();
        assert_eq!(out, "hello world");
    }

    #[test]
    fn kind_as_str_stable_kebab() {
        // Audit log / UI rely on these labels being stable.
        assert_eq!(OutboundKind::Llm.as_str(), "llm");
        assert_eq!(OutboundKind::CloudSaas.as_str(), "cloud_saas");
        assert_eq!(OutboundKind::Webdav.as_str(), "webdav");
        assert_eq!(OutboundKind::WebSearch.as_str(), "web_search");
        assert_eq!(OutboundKind::Telemetry.as_str(), "telemetry");
    }

    #[test]
    fn each_kind_observes_disabled() {
        // Every OutboundKind must respect the disabled bit — not just Llm.
        for k in [
            OutboundKind::Llm,
            OutboundKind::CloudSaas,
            OutboundKind::Webdav,
            OutboundKind::WebSearch,
            OutboundKind::Telemetry,
        ] {
            let p = pol(k, false, true, true);
            assert!(
                matches!(OutboundGate::enforce(&p, "data"), Err(OutboundError::Disabled(_))),
                "{k:?} did not refuse when disabled"
            );
        }
    }
}
