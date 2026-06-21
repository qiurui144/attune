//! doc_privacy — property tests (≥3, per §6.1 测试矩阵 / "Agent 验证铁律").
//!
//! Invariants verified across hundreds of randomized inputs — the
//! triple-verification class kvm relied on, plus attune's reversible-redaction
//! and no-panic guarantees:
//!
//! 1. **Zero-PII after redaction** — re-scanning redacted text finds no PII
//!    mapping (the core kvm invariant: redaction actually removed the value,
//!    not merely changed the bytes).
//! 2. **Reversible round-trip** — `restore()` of redacted text recovers the
//!    exact original for any input mixing PII and arbitrary text.
//! 3. **No panic on arbitrary bytes** — `redact_bytes` never panics on random
//!    UTF-8 / invalid-UTF-8 / empty input across all supported text extensions.
//! 4. **Classified is always fail-closed** — any text containing a confidential
//!    marker is graded Classified ⇒ redactor refuses ⇒ gate blocks.

use attune_core::doc_privacy::{
    classification_to_tier, gate_for_classification, Classification, DocPrivacyScanner,
    DocRedactor, GateDecision, RedactError, RedactMode,
};
use attune_core::outbound_gate::{OutboundError, OutboundGate, OutboundKind, OutboundPolicy};
use attune_core::pii::Redactor;
use attune_core::store::audit::PrivacyTier;
use proptest::prelude::*;

/// Generate a string that interleaves arbitrary text with real PII tokens so
/// the redactor has something to find.
fn pii_text() -> impl Strategy<Value = String> {
    let phones = prop::sample::select(vec![
        "13800138000",
        "13912345678",
        "15011112222",
    ]);
    let emails = prop::sample::select(vec!["alice@example.com", "bob@test.org"]);
    let filler = "[a-z 中文测试0-9]{0,40}";
    (filler, phones, filler, emails, filler).prop_map(|(a, p, b, e, c)| {
        format!("{a} 电话{p} {b} 邮箱{e} {c}")
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Invariant 1 + 2: redacted text has zero PII on re-scan AND restores exactly.
    #[test]
    fn redact_removes_pii_and_restores(input in pii_text()) {
        let r = Redactor::new();
        let dr = DocRedactor::new(&r);
        let out = dr
            .redact_bytes("txt", input.as_bytes(), Classification::Normal, RedactMode::Reversible)
            .expect("normal text redaction must succeed");
        let redacted = String::from_utf8(out.bytes.clone()).expect("utf-8 out");

        // (1) re-scan finds zero PII (the value really left, not just bytes changed)
        let rescan = r.redact(&redacted);
        prop_assert_eq!(rescan.mappings.len(), 0, "redacted text still has PII: {}", redacted);

        // (2) reversible round-trip recovers the original
        let restored = r.restore(&redacted, &out.mappings);
        prop_assert_eq!(restored, input);
    }

    /// Invariant 3: never panic on arbitrary input across text formats.
    #[test]
    fn never_panics_on_arbitrary_text(
        bytes in proptest::collection::vec(any::<u8>(), 0..200),
        ext in prop::sample::select(vec!["txt", "md", "csv", "json", "html"]),
    ) {
        let r = Redactor::new();
        let dr = DocRedactor::new(&r);
        // Must return a Result, never panic.
        let _ = dr.redact_bytes(ext, &bytes, Classification::Normal, RedactMode::Reversible);
        prop_assert!(true);
    }

    /// Invariant 4: a confidential marker always yields a fail-closed chain
    /// (Classified grade → redactor refuses → gate blocks).
    #[test]
    fn confidential_marker_is_always_fail_closed(
        prefix in "[a-z 中文0-9]{0,30}",
        marker in prop::sample::select(vec!["机密", "绝密", "confidential", "禁止外传"]),
        suffix in "[a-z 中文0-9]{0,30}",
    ) {
        let text = format!("{prefix}{marker}{suffix}");
        let r = Redactor::new();
        let scanner = DocPrivacyScanner::new(&r);
        let report = scanner.analyze_text(&text);
        prop_assert_eq!(report.classification, Classification::Classified);
        prop_assert!(report.blocked, "classified must be blocked");

        // redactor refuses
        let dr = DocRedactor::new(&r);
        let red = dr.redact_bytes("txt", text.as_bytes(), report.classification, RedactMode::Reversible);
        prop_assert!(matches!(red, Err(RedactError::Classified(_))));

        // gate decision is Block, tier is L0, and the real gate refuses cloud egress
        prop_assert_eq!(gate_for_classification(report.classification), GateDecision::Block);
        prop_assert_eq!(classification_to_tier(report.classification), PrivacyTier::L0);
        let policy = OutboundPolicy {
            kind: OutboundKind::Webdav,
            enabled: true,
            vault_unlocked: true,
            redactor: Some(&r),
            local_destination: false,
            contains_l0: true,
        };
        let verdict = OutboundGate::enforce(&policy, "file bytes");
        prop_assert!(matches!(verdict, Err(OutboundError::L0CloudBlocked)));
    }
}
