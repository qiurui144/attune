//! Telemetry queue — default-off, opt-in only.
//!
//! v1.0.6 ships the queue + default-false persistence only; **actual HTTP
//! send is not implemented** and is gated behind a future v1.1 toggle AND
//! `settings.privacy.telemetry == true`. Today, [`Telemetry::send`] returns
//! [`SendOutcome::SkippedDisabled`] when the user hasn't opted in, and
//! [`SendOutcome::SkippedNotImplemented`] when they have (no HTTP backend yet).
//!
//! per spec `docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md` §4.2
//! #⑤: telemetry is **never** auto-opt-in; first-launch must not surface a
//! "share telemetry" prompt; crash dumps stay local until the user explicitly
//! flips `privacy.telemetry=true` AND we ship a HTTP send path.
//!
//! **Task 5 of v1.0.6 Privacy Logic Implementation Plan.**

use crate::outbound_gate::{OutboundGate, OutboundKind, OutboundPolicy};

/// One queued telemetry event. Payloads are redacted-metadata only — never
/// chat prompts, never response text, never API keys.
#[derive(Debug, Clone)]
pub struct TelemetryEvent {
    /// ISO-8601 timestamp at event creation.
    pub ts_iso: String,
    /// Stable kebab-case kind tag: `vault_lock` | `outbound_call` |
    /// `dsar_export` | `settings_changed`.
    pub kind: String,
    /// Already-redacted metadata as JSON. Caller is responsible for not
    /// embedding raw PII / secrets here.
    pub redacted_meta: serde_json::Value,
}

/// Outcome of attempting to send a [`TelemetryEvent`].
#[derive(Debug, PartialEq, Eq)]
pub enum SendOutcome {
    /// Sent successfully (v1.0.6 never returns this — no HTTP path yet).
    Sent,
    /// User has not opted into telemetry; event dropped.
    SkippedDisabled,
    /// User has opted in, but v1.0.6 ships no HTTP backend; event dropped.
    SkippedNotImplemented,
    /// OutboundGate refused the call (e.g. payload check failed).
    SkippedGate,
}

/// Telemetry sink. Constructed with the current `privacy.telemetry` flag.
///
/// Default is **disabled**; constructor must be passed `false` unless the
/// user has explicitly flipped the toggle through the Privacy dashboard.
pub struct Telemetry {
    pub enabled: bool,
}

impl Telemetry {
    /// Constructor takes the value loaded from `settings.privacy.telemetry`.
    /// **Default**: callers without a settings snapshot should pass `false`.
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    /// Disabled-by-default convenience constructor — matches "no settings
    /// loaded yet" semantics. Always returns a Telemetry with `enabled=false`.
    pub fn disabled() -> Self {
        Self { enabled: false }
    }

    /// Always returns [`SendOutcome::SkippedDisabled`] when not enabled.
    /// Returns [`SendOutcome::SkippedNotImplemented`] when enabled — v1.0.6
    /// ships **no actual HTTP send**, by design.
    ///
    /// Even when enabled, the call still routes through [`OutboundGate`] so
    /// the audit script's grep guard sees `OutboundGate::enforce` here.
    pub fn send(&self, _event: &TelemetryEvent) -> SendOutcome {
        if !self.enabled {
            return SendOutcome::SkippedDisabled;
        }

        // v1.0.6: defensively route through the gate for audit-script visibility.
        // Telemetry payload is empty here — we don't have an HTTP backend, so the
        // gate's redactor isn't needed.
        // Telemetry is exempt from vault-locked (no vault data) and never
        // carries item content (no L0 tier). `cloud()` defaults both privacy-
        // tier fields safely; vault_unlocked=false is harmless because the gate
        // skips the vault check for OutboundKind::Telemetry.
        let policy = OutboundPolicy::cloud(
            OutboundKind::Telemetry,
            self.enabled,
            false,
            None,
        );
        match OutboundGate::enforce(&policy, "") {
            Ok(_) => SendOutcome::SkippedNotImplemented,
            Err(_) => SendOutcome::SkippedGate,
        }
    }
}

impl Default for Telemetry {
    /// Default is **disabled** — never auto-opt-in (per spec §4.2 #⑤).
    fn default() -> Self {
        Self::disabled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev() -> TelemetryEvent {
        TelemetryEvent {
            ts_iso: "2026-05-28T00:00:00Z".into(),
            kind: "vault_lock".into(),
            redacted_meta: serde_json::json!({}),
        }
    }

    /// Default Telemetry is **disabled** — never auto-opt-in.
    #[test]
    fn default_is_disabled() {
        let t = Telemetry::default();
        assert!(!t.enabled);
        assert_eq!(t.send(&ev()), SendOutcome::SkippedDisabled);
    }

    /// `Telemetry::disabled()` constructor matches default.
    #[test]
    fn disabled_constructor_matches_default() {
        let t = Telemetry::disabled();
        assert!(!t.enabled);
        assert_eq!(t.send(&ev()), SendOutcome::SkippedDisabled);
    }

    /// `Telemetry::new(false)` is disabled.
    #[test]
    fn new_false_is_disabled() {
        let t = Telemetry::new(false);
        assert_eq!(t.send(&ev()), SendOutcome::SkippedDisabled);
    }

    /// `Telemetry::new(true)` returns `SkippedNotImplemented` — opt-in is
    /// honored but no HTTP send happens in v1.0.6.
    #[test]
    fn new_true_returns_skipped_not_implemented_in_v1_0_6() {
        let t = Telemetry::new(true);
        // Even with consent, v1.0.6 has no HTTP backend — must not pretend to send.
        assert_eq!(t.send(&ev()), SendOutcome::SkippedNotImplemented);
    }

    /// Telemetry must never return `Sent` in v1.0.6.
    #[test]
    fn never_returns_sent_in_v1_0_6() {
        for enabled in [false, true] {
            let t = Telemetry::new(enabled);
            assert_ne!(
                t.send(&ev()),
                SendOutcome::Sent,
                "v1.0.6 must never return Sent (no HTTP backend yet)"
            );
        }
    }
}
