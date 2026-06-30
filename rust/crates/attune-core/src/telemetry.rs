//! Telemetry queue — default-off, opt-in only.
//!
//! v1.1 ships an actual HTTP send path, but it remains default-false and
//! opt-in only. [`Telemetry::send`] returns [`SendOutcome::SkippedDisabled`]
//! when the user hasn't opted in and [`SendOutcome::SkippedNoEndpoint`] when
//! no endpoint is configured.
//!
//! per spec `docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md` §4.2
//! #⑤: telemetry is **never** auto-opt-in; first-launch must not surface a
//! "share telemetry" prompt; crash dumps stay local until the user explicitly
//! flips `privacy.telemetry=true` AND an endpoint is configured.
//!
//! **Task 5 of v1.0.6 Privacy Logic Implementation Plan.**

use crate::outbound_gate::{OutboundGate, OutboundKind, OutboundPolicy};
use crate::pii::Redactor;
use serde::Serialize;
use std::time::Duration;

/// One queued telemetry event. Payloads are redacted-metadata only — never
/// chat prompts, never response text, never API keys.
#[derive(Debug, Clone, Serialize)]
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
    /// Sent successfully.
    Sent,
    /// User has not opted into telemetry; event dropped.
    SkippedDisabled,
    /// User has opted in, but no endpoint is configured; event dropped.
    SkippedNoEndpoint,
    /// OutboundGate refused the call (e.g. payload check failed).
    SkippedGate,
    /// HTTP/backend failure; event not persisted remotely.
    Failed,
}

/// Telemetry sink. Constructed with the current `privacy.telemetry` flag.
///
/// Default is **disabled**; constructor must be passed `false` unless the
/// user has explicitly flipped the toggle through the Privacy dashboard.
pub struct Telemetry {
    pub enabled: bool,
    pub endpoint: Option<String>,
    pub install_id: Option<String>,
}

impl Telemetry {
    /// Constructor takes the value loaded from `settings.privacy.telemetry`.
    /// **Default**: callers without a settings snapshot should pass `false`.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            endpoint: std::env::var("ATTUNE_TELEMETRY_ENDPOINT").ok(),
            install_id: None,
        }
    }

    /// Constructor used by settings/cloud-session aware callers.
    pub fn with_endpoint(
        enabled: bool,
        endpoint: Option<String>,
        install_id: Option<String>,
    ) -> Self {
        Self {
            enabled,
            endpoint,
            install_id,
        }
    }

    /// Disabled-by-default convenience constructor — matches "no settings
    /// loaded yet" semantics. Always returns a Telemetry with `enabled=false`.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            endpoint: None,
            install_id: None,
        }
    }

    /// Always returns [`SendOutcome::SkippedDisabled`] when not enabled. When enabled,
    /// routes through [`OutboundGate`] and POSTs a redacted metadata envelope.
    pub fn send(&self, event: &TelemetryEvent) -> SendOutcome {
        if !self.enabled {
            return SendOutcome::SkippedDisabled;
        }
        // Telemetry is exempt from vault-locked (no vault data) and never
        // carries item content (no L0 tier). `cloud()` defaults both privacy-
        // tier fields safely; vault_unlocked=false is harmless because the gate
        // skips the vault check for OutboundKind::Telemetry.
        let redactor = Redactor::new();
        let policy = OutboundPolicy::cloud(
            OutboundKind::Telemetry,
            self.enabled,
            false,
            Some(&redactor),
        );
        let endpoint = match self
            .endpoint
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(v) => v,
            None => return SendOutcome::SkippedNoEndpoint,
        };
        let payload = TelemetryEnvelope {
            schema_version: 1,
            install_id: self.install_id.as_deref(),
            event,
        };
        let body = match serde_json::to_string(&payload) {
            Ok(v) => v,
            Err(_) => return SendOutcome::Failed,
        };
        let body = match OutboundGate::enforce(&policy, &body) {
            Ok(redacted) => redacted,
            Err(_) => return SendOutcome::SkippedGate,
        };
        let client = match reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
        {
            Ok(c) => c,
            Err(_) => return SendOutcome::Failed,
        };
        match client
            .post(endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
        {
            Ok(resp) if resp.status().is_success() => SendOutcome::Sent,
            _ => SendOutcome::Failed,
        }
    }
}

#[derive(Serialize)]
struct TelemetryEnvelope<'a> {
    schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    install_id: Option<&'a str>,
    event: &'a TelemetryEvent,
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    fn ev() -> TelemetryEvent {
        TelemetryEvent {
            ts_iso: "2026-05-28T00:00:00Z".into(),
            kind: "vault_lock".into(),
            redacted_meta: serde_json::json!({}),
        }
    }

    fn ev_with_pii() -> TelemetryEvent {
        TelemetryEvent {
            ts_iso: "2026-05-28T00:00:00Z".into(),
            kind: "settings_changed".into(),
            redacted_meta: serde_json::json!({
                "field": "phone",
                "sample": "13800138000",
            }),
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

    /// `Telemetry::new(true)` without an endpoint honors opt-in but does not fabricate send.
    #[test]
    fn new_true_without_endpoint_returns_skipped_no_endpoint() {
        let t = Telemetry::with_endpoint(true, None, None);
        assert_eq!(t.send(&ev()), SendOutcome::SkippedNoEndpoint);
    }

    #[test]
    fn envelope_serializes_without_prompt_fields() {
        let event = ev();
        let body = serde_json::to_string(&TelemetryEnvelope {
            schema_version: 1,
            install_id: Some("install-1"),
            event: &event,
        })
        .unwrap();
        assert!(body.contains("vault_lock"));
        assert!(body.contains("install-1"));
        assert!(!body.contains("prompt"));
    }

    #[test]
    fn enabled_with_endpoint_posts_redacted_envelope() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local telemetry fixture");
        let addr = listener.local_addr().expect("fixture addr");
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept telemetry request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set timeout");
            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            loop {
                let n = stream.read(&mut tmp).expect("read request");
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if let Some(header_end) = find_header_end(&buf) {
                    let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
                    let content_len = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            if name.eq_ignore_ascii_case("content-length") {
                                value.trim().parse::<usize>().ok()
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0);
                    let expected = header_end + 4 + content_len;
                    while buf.len() < expected {
                        let n = stream.read(&mut tmp).expect("read request body");
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    break;
                }
            }
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .expect("write response");
            tx.send(String::from_utf8_lossy(&buf).to_string())
                .expect("send captured request");
        });

        let telemetry = Telemetry::with_endpoint(
            true,
            Some(format!("http://{addr}/v1/events")),
            Some("install-1".into()),
        );
        assert_eq!(telemetry.send(&ev_with_pii()), SendOutcome::Sent);
        let request = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("captured telemetry request");
        handle.join().expect("fixture thread");
        assert!(request.starts_with("POST /v1/events HTTP/1.1"));
        assert!(request.contains("\"schema_version\":1"));
        assert!(request.contains("\"install_id\":\"install-1\""));
        assert!(request.contains("\"kind\":\"settings_changed\""));
        assert!(request.contains("[PHONE_1]"));
        assert!(!request.contains("13800138000"));
        assert!(!request.contains("prompt"));
    }

    fn find_header_end(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n")
    }
}
