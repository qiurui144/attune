//! Data models for document-file-level privacy (spec §3 / §5).
//!
//! Borrows the *design* of `kvm-info-privacy::models` (Classification grades,
//! DetectionReport shape) but is an attune-native reimplementation:
//! - PII kind reuses attune's existing [`crate::pii::PiiKind`] (12 classes),
//!   **not** kvm's 8-class `EntityType` (no verbatim copy — kvm-info-privacy
//!   ships no LICENSE, so only the design is reused).
//! - `DetectionReport` stores **no PII values** (privacy-first / §1.4): an
//!   entity records only kind + page + layer, never the captured string. This
//!   is what gets persisted to `doc_privacy_meta`.
//!
//! The `Classification` enum keeps kvm's snake_case wire values
//! (`normal | sensitive_partial | classified`) so its design rationale and
//! fixture semantics carry over.

use crate::pii::PiiKind;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Document-wide sensitivity grade. Drives the OutboundGate egress decision
/// (see [`crate::doc_privacy::classify_to_tier`]):
/// - `Normal`           → ordinary egress (still PII-redacted).
/// - `SensitivePartial` → high density of PII; egress allowed but must redact.
/// - `Classified`       → confidential marker found; **fail-closed block**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    Normal,
    SensitivePartial,
    Classified,
}

impl Classification {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::SensitivePartial => "sensitive_partial",
            Self::Classified => "classified",
        }
    }
}

/// One detected PII span inside a document. **No value is stored** — only its
/// kind and location, so persisting a report never re-introduces the secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocEntity {
    /// Reuses attune's PII taxonomy (id_card / phone / email / …).
    pub kind: PiiKind,
    /// 0-based page index (text-layer docs only; 0 for flat text).
    pub page: u32,
    /// "text" | "image" — which layer this entity came from.
    pub layer: String,
    /// Byte offset of the span within the extracted text (for redaction).
    /// `None` for image-layer entities (located by bbox, not text offset).
    pub byte_start: Option<usize>,
    pub byte_end: Option<usize>,
}

/// Result of a document privacy scan (returned by `/doc-privacy/scan`, cached in
/// `doc_privacy_meta`). Privacy-first: contains **no** captured PII values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionReport {
    pub classification: Classification,
    /// `true` ⇔ `classification == Classified` ⇔ egress is fail-closed blocked.
    pub blocked: bool,
    /// Human-readable reason when blocked (e.g. which keyword on which page).
    /// Never contains a PII value.
    pub block_reason: Option<String>,
    /// Advisory note (e.g. "12 high-risk PII entities — full redaction advised").
    pub warning: Option<String>,
    pub entities: Vec<DocEntity>,
    /// Count per PII-kind placeholder prefix (e.g. {"PHONE": 2, "EMAIL": 1}).
    pub summary: HashMap<String, usize>,
}

impl DetectionReport {
    /// Build the per-kind count summary from a slice of entities.
    pub fn build_summary(entities: &[DocEntity]) -> HashMap<String, usize> {
        let mut m = HashMap::new();
        for e in entities {
            *m.entry(e.kind.placeholder_prefix().to_uppercase())
                .or_insert(0) += 1;
        }
        m
    }

    /// A clean (Normal, no entities) report — used for empty / PII-free docs.
    pub fn clean() -> Self {
        Self {
            classification: Classification::Normal,
            blocked: false,
            block_reason: None,
            warning: None,
            entities: Vec::new(),
            summary: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_wire_values_match_kvm_snake_case() {
        assert_eq!(Classification::Normal.as_str(), "normal");
        assert_eq!(
            Classification::SensitivePartial.as_str(),
            "sensitive_partial"
        );
        assert_eq!(Classification::Classified.as_str(), "classified");
    }

    #[test]
    fn classification_serde_roundtrip() {
        for c in [
            Classification::Normal,
            Classification::SensitivePartial,
            Classification::Classified,
        ] {
            let j = serde_json::to_string(&c).unwrap();
            let back: Classification = serde_json::from_str(&j).unwrap();
            assert_eq!(c, back);
        }
        // wire shape is the bare snake_case string
        assert_eq!(
            serde_json::to_string(&Classification::SensitivePartial).unwrap(),
            "\"sensitive_partial\""
        );
    }

    #[test]
    fn summary_counts_by_kind_prefix() {
        let ents = vec![
            DocEntity {
                kind: PiiKind::Phone,
                page: 0,
                layer: "text".into(),
                byte_start: Some(0),
                byte_end: Some(11),
            },
            DocEntity {
                kind: PiiKind::Phone,
                page: 1,
                layer: "text".into(),
                byte_start: Some(20),
                byte_end: Some(31),
            },
            DocEntity {
                kind: PiiKind::Email,
                page: 0,
                layer: "text".into(),
                byte_start: Some(40),
                byte_end: Some(55),
            },
        ];
        let s = DetectionReport::build_summary(&ents);
        assert_eq!(s.get("PHONE"), Some(&2));
        assert_eq!(s.get("EMAIL"), Some(&1));
        assert_eq!(s.get("ID"), None);
    }

    #[test]
    fn clean_report_is_normal_unblocked() {
        let r = DetectionReport::clean();
        assert_eq!(r.classification, Classification::Normal);
        assert!(!r.blocked);
        assert!(r.entities.is_empty());
    }

    #[test]
    fn report_with_no_values_does_not_leak_pii() {
        // A persisted report must never carry the captured PII string.
        let ents = vec![DocEntity {
            kind: PiiKind::IdCard,
            page: 0,
            layer: "text".into(),
            byte_start: Some(0),
            byte_end: Some(18),
        }];
        let report = DetectionReport {
            classification: Classification::SensitivePartial,
            blocked: false,
            block_reason: None,
            warning: Some("1 high-risk entity".into()),
            summary: DetectionReport::build_summary(&ents),
            entities: ents,
        };
        let json = serde_json::to_string(&report).unwrap();
        // The struct has no `value`/`original` field, so the secret can't appear.
        assert!(
            !json.contains("11010119900307"),
            "report JSON must not carry PII value"
        );
    }
}
