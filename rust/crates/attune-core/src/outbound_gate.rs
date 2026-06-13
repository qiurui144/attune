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
//! 3. **L0-tagged content to a cloud destination** → returns
//!    [`OutboundError::L0CloudBlocked`]. `PrivacyTier::L0` ("🔒 永不出网,
//!    强制本地 LLM") content must NEVER reach a non-local endpoint. Checked
//!    before redaction because redaction does not make L0 content cloud-safe —
//!    L0 means *no egress at all*, even redacted.
//! 4. **Payload contains PII but no redactor installed** → returns
//!    [`OutboundError::RedactorRequired`]. We **fail closed** rather than send
//!    PII unredacted.
//! 5. **All checks pass** → returns the **redacted** payload, never the
//!    original. Caller passes the returned string to the wire.
//!
//! ## Why a struct gate and not a free function?
//!
//! Today we have a single `enforce()` static method, but the struct gives us a
//! stable type-name for future audit hooks (e.g. emit a `record_outbound`
//! audit event from inside the gate so every wrap point is auto-logged).

use crate::pii::Redactor;

/// Discriminator over the 6 outbound kinds tracked by Privacy Logic Strategy.
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
    /// Embedding providers (OllamaProvider / OpenAiEmbeddingProvider).
    /// Local Ollama (localhost/127.x) is always permitted even for L0 content;
    /// cloud embedding endpoints are subject to the full L0 + PII gate.
    /// (#82 P0 privacy fix — embed was the only egress point missing gate wiring.)
    Embedding,
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
            OutboundKind::Embedding => "embedding",
        }
    }

    /// Returns true if this kind is Embedding (used to select Telemetry-style
    /// vault-lock exemption — embedding of memory summaries runs on a background
    /// timer that may fire before the vault has been unlocked by the UI; we
    /// short-circuit at the caller level instead).
    pub fn is_embedding(self) -> bool {
        matches!(self, OutboundKind::Embedding)
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
    /// Payload carries `PrivacyTier::L0` ("永不出网") content but the destination
    /// is a cloud (non-local) endpoint → fail closed. L0 content may only go to
    /// a local LLM; even redaction does not relax this.
    #[error("l0-cloud-blocked: L0-tagged content cannot leave the device to a cloud destination")]
    L0CloudBlocked,
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
    /// Whether the destination endpoint is a **local** LLM/service (Ollama /
    /// localhost). When `false` (cloud), any `contains_l0` payload is refused
    /// with [`OutboundError::L0CloudBlocked`]. For non-LLM egress points
    /// (WebDAV / WebSearch / CloudSaas) the destination is always cloud, so
    /// callers pass `local_destination: false`.
    pub local_destination: bool,
    /// Whether `payload` (or the context being assembled for it) includes any
    /// content tagged `PrivacyTier::L0`. When `true` AND
    /// `local_destination == false`, the gate refuses. Callers that never carry
    /// item content (telemetry / search query / webdav path) pass `false`.
    pub contains_l0: bool,
}

impl<'a> OutboundPolicy<'a> {
    /// Convenience constructor for the common case: a cloud destination that
    /// carries no L0 content (the historical 4-field policy). Keeps existing
    /// call sites terse while the two privacy-tier fields default safely.
    pub fn cloud(
        kind: OutboundKind,
        enabled: bool,
        vault_unlocked: bool,
        redactor: Option<&'a Redactor>,
    ) -> Self {
        OutboundPolicy {
            kind,
            enabled,
            vault_unlocked,
            redactor,
            local_destination: false,
            contains_l0: false,
        }
    }
}

/// The single outbound enforcement entry-point.
pub struct OutboundGate;

impl OutboundGate {
    /// Enforce all five contract clauses
    /// (disabled / vault / L0-cloud / redactor / redact).
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

        // 3) L0 "永不出网" → an L0-tagged payload may only reach a local LLM.
        //    Checked BEFORE redaction: L0 means no egress at all, redaction does
        //    not make it cloud-safe. Local destinations are exempt.
        if policy.contains_l0 && !policy.local_destination {
            return Err(OutboundError::L0CloudBlocked);
        }

        // 4) Redact PII (fail closed if payload appears to contain PII but no
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
            local_destination: false,
            contains_l0: false,
        }
    }

    /// Build an LLM policy carrying L0 content, parameterized on destination.
    fn l0_pol(local_destination: bool, with_redactor: bool) -> OutboundPolicy<'static> {
        let redactor: Option<&'static Redactor> = if with_redactor {
            Some(Box::leak(Box::new(Redactor::new())))
        } else {
            None
        };
        OutboundPolicy {
            kind: OutboundKind::Llm,
            enabled: true,
            vault_unlocked: true,
            redactor,
            local_destination,
            contains_l0: true,
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
        assert_eq!(OutboundKind::Embedding.as_str(), "embedding");
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
            OutboundKind::Embedding,
        ] {
            let p = pol(k, false, true, true);
            assert!(
                matches!(OutboundGate::enforce(&p, "data"), Err(OutboundError::Disabled(_))),
                "{k:?} did not refuse when disabled"
            );
        }
    }

    // ── #82 P0: Embedding OutboundGate ───────────────────────────────────────

    /// L0 chunk to a cloud embedding endpoint must be blocked.
    /// This is the adversarial test: construct a real L0 policy and verify the
    /// gate refuses before any HTTP is sent.
    #[test]
    fn embedding_l0_cloud_blocked() {
        let p = OutboundPolicy {
            kind: OutboundKind::Embedding,
            enabled: true,
            vault_unlocked: true,
            redactor: Some(Box::leak(Box::new(Redactor::new()))),
            local_destination: false, // cloud endpoint, e.g. api.openai.com
            contains_l0: true,
        };
        assert!(
            matches!(
                OutboundGate::enforce(&p, "敏感医疗记录 L0 内容"),
                Err(OutboundError::L0CloudBlocked)
            ),
            "L0 chunk to cloud embedding endpoint must be blocked"
        );
    }

    /// L0 chunk to localhost Ollama must be allowed (forced-local is the point).
    #[test]
    fn embedding_l0_local_allowed() {
        let p = OutboundPolicy {
            kind: OutboundKind::Embedding,
            enabled: true,
            vault_unlocked: true,
            redactor: Some(Box::leak(Box::new(Redactor::new()))),
            local_destination: true, // localhost ollama
            contains_l0: true,
        };
        let out = OutboundGate::enforce(&p, "敏感内容 L0 本地允许");
        assert!(out.is_ok(), "L0 to local embedding must be allowed; got {out:?}");
    }

    /// Cloud embedding disabled → gate refuses.
    #[test]
    fn embedding_disabled_refuses() {
        let p = OutboundPolicy {
            kind: OutboundKind::Embedding,
            enabled: false, // user disabled cloud embedding
            vault_unlocked: true,
            redactor: Some(Box::leak(Box::new(Redactor::new()))),
            local_destination: false,
            contains_l0: false,
        };
        assert!(
            matches!(
                OutboundGate::enforce(&p, "normal content"),
                Err(OutboundError::Disabled(OutboundKind::Embedding))
            ),
            "disabled embedding must be refused"
        );
    }

    /// Non-L0 chunk to cloud embedding, enabled → allowed (normal path regression).
    #[test]
    fn embedding_non_l0_cloud_allowed() {
        let p = OutboundPolicy {
            kind: OutboundKind::Embedding,
            enabled: true,
            vault_unlocked: true,
            redactor: Some(Box::leak(Box::new(Redactor::new()))),
            local_destination: false,
            contains_l0: false,
        };
        let out = OutboundGate::enforce(&p, "public knowledge document");
        assert!(out.is_ok(), "non-L0 cloud embedding must be allowed; got {out:?}");
    }

    // ── G3: L0 "永不出网" enforcement ────────────────────────────────────

    #[test]
    fn l0_content_to_cloud_is_blocked() {
        // L0-tagged content + cloud destination → MUST refuse, even with a
        // redactor present (redaction does not relax L0).
        let p = l0_pol(/* local_destination */ false, /* redactor */ true);
        assert!(
            matches!(
                OutboundGate::enforce(&p, "敏感证据 phone 13800138000"),
                Err(OutboundError::L0CloudBlocked)
            ),
            "L0 content to cloud must be refused"
        );
    }

    #[test]
    fn l0_content_to_local_is_allowed() {
        // L0 content + local destination → allowed (forced-local is the point).
        let p = l0_pol(/* local_destination */ true, /* redactor */ true);
        let out = OutboundGate::enforce(&p, "敏感证据 phone 13800138000")
            .expect("L0 to local LLM must be allowed");
        // Redaction still applies even on local.
        assert!(!out.contains("13800138000"), "phone still redacted; got: {out}");
    }

    #[test]
    fn l0_blocked_takes_precedence_over_redactor_required() {
        // L0 + cloud + NO redactor → L0CloudBlocked (the stronger refusal),
        // not RedactorRequired. The order proves L0 is checked first.
        let p = l0_pol(/* local_destination */ false, /* redactor */ false);
        assert!(
            matches!(
                OutboundGate::enforce(&p, "敏感证据"),
                Err(OutboundError::L0CloudBlocked)
            ),
            "L0-cloud must refuse before the redactor check"
        );
    }

    #[test]
    fn non_l0_cloud_unaffected_by_l0_gate() {
        // Default cloud policy (contains_l0=false) still passes through normally.
        let p = OutboundPolicy::cloud(
            OutboundKind::Llm,
            true,
            true,
            Some(Box::leak(Box::new(Redactor::new()))),
        );
        assert_eq!(OutboundGate::enforce(&p, "hello").unwrap(), "hello");
    }
}
