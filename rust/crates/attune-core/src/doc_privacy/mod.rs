//! Document file-level privacy (spec `2026-06-20-privacy-layer-enhancement.md`).
//!
//! Closes the INT-2 gap: attune historically only redacted **text prompts** at
//! the LLM egress point. A user's real asset is a **file** (contract PDF, client
//! roster XLSX). On export / WebDAV sync / share those file bytes left the device
//! **bypassing** the text redactor. This module adds:
//!
//! - **Entity-level detection** inside a document's extracted text (reusing the
//!   existing [`crate::pii::Redactor`] — 12 PII classes + checksums).
//! - **Document grading** (`normal | sensitive_partial | classified`) via
//!   [`classifier::DocClassifier`] (confidential-keyword fail-closed).
//! - **Confidential fail-closed block (G4, highest value)**: a `Classified`
//!   document is refused at both the redactor and the OutboundGate.
//! - **Reversible byte-level redaction** of text / docx / xlsx (attune `[KIND_N]`
//!   tokens — improves on kvm's irreversible black blocks; restorable on import).
//! - **Classification → OutboundGate decision (G5)**: [`gate_for_classification`]
//!   maps a doc grade onto the existing egress contract so file export points
//!   flow through the same gate as every other egress.
//!
//! ## Provenance / license
//!
//! Design borrowed from `qiurui144/kvm-info-privacy` (no LICENSE file → design
//! reuse only, **no verbatim code copy**) and the MIT-licensed
//! `qiurui144/kvm-privacy-gateway` (mode/grade/audit schema design). The MITM
//! transparent-proxy path of the gateway is deliberately **not** brought over
//! (attune product decision: built-in Chat, no browser injection, no CA install).
//!
//! ## Cost contract (CLAUDE.md §成本契约)
//!
//! Scan = 🆓 zero-cost (regex + dictionary, CPU ms). No LLM at any point in this
//! module (v1.x); LLM-assisted grading is explicitly deferred to v.next.

pub mod classifier;
pub mod models;
pub mod redactor;

pub use classifier::{ClassificationResult, DocClassifier};
pub use models::{Classification, DetectionReport, DocEntity};
pub use redactor::{DocRedactor, RedactError, RedactMode, RedactionOutput};

use crate::pii::Redactor;
use crate::store::audit::PrivacyTier;

/// Scans extracted document text for PII + a confidentiality grade.
///
/// Pairs an existing [`Redactor`] (PII detection, plugin/dictionary aware) with a
/// [`DocClassifier`] (keyword grading). The scanner produces a privacy-first
/// [`DetectionReport`] that carries **no captured PII values** — safe to persist.
pub struct DocPrivacyScanner<'a> {
    redactor: &'a Redactor,
    classifier: DocClassifier,
}

impl<'a> DocPrivacyScanner<'a> {
    pub fn new(redactor: &'a Redactor) -> Self {
        Self { redactor, classifier: DocClassifier::new() }
    }

    /// Use a custom classifier (e.g. pro-plugin-extended confidential keywords).
    pub fn with_classifier(redactor: &'a Redactor, classifier: DocClassifier) -> Self {
        Self { redactor, classifier }
    }

    /// Analyze already-extracted document text (parsing/OCR happens upstream).
    ///
    /// `page_of` maps a byte offset to a page index; pass `None` for flat text
    /// (everything reported as page 0).
    pub fn analyze_text(&self, text: &str) -> DetectionReport {
        if text.is_empty() {
            return DetectionReport::clean();
        }

        // 1) PII detection (reuse Redactor — same engine as the LLM egress path).
        let detection = self.redactor.redact(text);
        let entities: Vec<DocEntity> = detection
            .mappings
            .iter()
            .map(|m| DocEntity {
                kind: m.kind.clone(),
                page: 0, // flat text; page mapping is a caller concern
                layer: "text".to_string(),
                byte_start: Some(m.byte_start),
                byte_end: Some(m.byte_end),
            })
            .collect();

        // 2) Confidentiality grade (keyword fail-closed first, then PII density).
        let cls = self.classifier.classify(text, &entities);

        let summary = DetectionReport::build_summary(&entities);
        DetectionReport {
            classification: cls.classification,
            blocked: cls.blocked,
            block_reason: cls.block_reason,
            warning: cls.warning,
            entities,
            summary,
        }
    }
}

/// The egress verdict for a document grade (G5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    /// `Classified` → never leaves the device (fail-closed block).
    Block,
    /// `SensitivePartial` / `Normal` → egress allowed but content must be
    /// redacted first. The bool carries whether redaction is *mandatory*
    /// (always true here — attune redacts on every egress).
    AllowRedacted { mandatory_redact: bool },
}

/// Map a document classification onto the OutboundGate decision (spec §3 / S4).
///
/// - `Classified`        → [`GateDecision::Block`] (treated like L0 永不出网).
/// - `SensitivePartial`  → allow, redaction mandatory.
/// - `Normal`            → allow, redaction mandatory (attune always redacts).
pub fn gate_for_classification(c: Classification) -> GateDecision {
    match c {
        Classification::Classified => GateDecision::Block,
        Classification::SensitivePartial | Classification::Normal => {
            GateDecision::AllowRedacted { mandatory_redact: true }
        }
    }
}

/// Map a document classification onto the existing [`PrivacyTier`] vocabulary so
/// file egress reuses the same tier semantics as chunk content:
/// `Classified → L0` ("永不出网", drives `contains_l0` on the OutboundPolicy),
/// everything else `→ L1` (regex-redacted cloud-ok).
pub fn classification_to_tier(c: Classification) -> PrivacyTier {
    match c {
        Classification::Classified => PrivacyTier::L0,
        Classification::SensitivePartial | Classification::Normal => PrivacyTier::L1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanner_with(r: &Redactor) -> DocPrivacyScanner<'_> {
        DocPrivacyScanner::new(r)
    }

    #[test]
    fn scan_detects_pii_entities_without_values() {
        let r = Redactor::new();
        let s = scanner_with(&r);
        let report = s.analyze_text("电话 13800138000 邮箱 a@b.com");
        assert!(report.entities.len() >= 2, "phone + email detected; got {:?}", report.summary);
        // privacy-first: serialized report carries no PII value
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("13800138000"), "report must not leak phone value");
        assert!(!json.contains("a@b.com"), "report must not leak email value");
    }

    #[test]
    fn scan_grades_classified_doc_blocked() {
        let r = Redactor::new();
        let s = scanner_with(&r);
        let report = s.analyze_text("机密文件：客户电话 13800138000");
        assert_eq!(report.classification, Classification::Classified);
        assert!(report.blocked);
    }

    #[test]
    fn scan_empty_doc_is_clean() {
        let r = Redactor::new();
        let s = scanner_with(&r);
        let report = s.analyze_text("");
        assert_eq!(report.classification, Classification::Normal);
        assert!(report.entities.is_empty());
    }

    #[test]
    fn gate_blocks_classified() {
        assert_eq!(gate_for_classification(Classification::Classified), GateDecision::Block);
    }

    #[test]
    fn gate_allows_normal_and_sensitive_with_mandatory_redact() {
        assert_eq!(
            gate_for_classification(Classification::Normal),
            GateDecision::AllowRedacted { mandatory_redact: true }
        );
        assert_eq!(
            gate_for_classification(Classification::SensitivePartial),
            GateDecision::AllowRedacted { mandatory_redact: true }
        );
    }

    #[test]
    fn classification_maps_classified_to_l0() {
        assert_eq!(classification_to_tier(Classification::Classified), PrivacyTier::L0);
        assert_eq!(classification_to_tier(Classification::Normal), PrivacyTier::L1);
        assert_eq!(classification_to_tier(Classification::SensitivePartial), PrivacyTier::L1);
    }

    /// End-to-end adversarial: a classified doc, mapped to L0, fed to the real
    /// OutboundGate with a cloud destination, is blocked — proving the wiring
    /// closes the "file export bypasses gate" hole.
    #[test]
    fn classified_doc_blocked_at_real_outbound_gate() {
        use crate::outbound_gate::{OutboundGate, OutboundKind, OutboundError, OutboundPolicy};
        let r = Redactor::new();
        let s = scanner_with(&r);
        let report = s.analyze_text("绝密：导出此文件");
        assert_eq!(report.classification, Classification::Classified);

        let tier = classification_to_tier(report.classification);
        let policy = OutboundPolicy {
            kind: OutboundKind::Webdav, // file sync egress point
            enabled: true,
            vault_unlocked: true,
            redactor: Some(&r),
            local_destination: false,
            contains_l0: tier == PrivacyTier::L0,
        };
        let verdict = OutboundGate::enforce(&policy, "file bytes here");
        assert!(
            matches!(verdict, Err(OutboundError::L0CloudBlocked)),
            "classified doc must be blocked at the real gate; got {verdict:?}"
        );
    }
}
