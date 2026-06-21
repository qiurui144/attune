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

/// Outcome of the one-call file-egress gate.
pub enum FileEgressOutcome {
    /// Egress refused — classified document must not leave the device.
    Blocked { reason: String },
    /// Egress allowed; `bytes` are the **redacted** file to put on the wire.
    Allowed(RedactionOutput),
}

/// Single correct entry-point for **any file-export / share / WebDAV egress**
/// (spec §3 "all egress points must flow through one gate"; closes the INT-2
/// "document export bypasses redaction" hole).
///
/// Pipeline: extract text upstream → scan/grade → classified ⇒ fail-closed
/// block → otherwise byte-level redact the file before it leaves.
///
/// `extracted_text` is the document's text layer (from the existing parser/OCR);
/// `ext` + `data` are the raw file. On `Classified` the function never produces
/// output bytes (defense in depth with the OutboundGate L0 check).
pub fn enforce_file_egress(
    redactor: &Redactor,
    extracted_text: &str,
    ext: &str,
    data: &[u8],
    mode: RedactMode,
) -> Result<FileEgressOutcome, RedactError> {
    let scanner = DocPrivacyScanner::new(redactor);
    let report = scanner.analyze_text(extracted_text);

    match gate_for_classification(report.classification) {
        GateDecision::Block => Ok(FileEgressOutcome::Blocked {
            reason: report
                .block_reason
                .unwrap_or_else(|| "classified document".to_string()),
        }),
        GateDecision::AllowRedacted { .. } => {
            let out = DocRedactor::new(redactor).redact_bytes(
                ext,
                data,
                report.classification,
                mode,
            )?;
            Ok(FileEgressOutcome::Allowed(out))
        }
    }
}

// ── Artifact-IR egress (the export / skill-runtime download point) ──────────

use crate::export::{Artifact, Block, Document, ExportFormat, Table};

/// PDF terminal decision (INT-2 §PDF, 务实/option-b): byte-level PDF redaction
/// stays **out of scope** (font/encoding-map rewriting risks emitting a
/// half-redacted / garbled file). Instead, when a PDF egress is refused (a
/// confidential PDF) or carries PII, callers offer a **redacted alternative
/// format** built from the same IR. This returns the recommended alt format for
/// a requested PDF export (`Md` — universally renderable + losslessly redactable;
/// callers can also offer `Docx`). `None` for any non-PDF format (those redact
/// in place and need no alternative).
///
/// Note: the *artifact* export path (IR → file) already redacts PDF safely
/// because it re-renders from the redacted IR, not the raw PDF bytes. This hint
/// is for the **confidential-block** case (fail-closed) and the raw-PDF-file
/// path ([`enforce_file_egress`], which fails closed on PDF) so the UI always has
/// an actionable "export as redacted docx/txt instead" path.
pub fn pdf_alt_format(requested: ExportFormat) -> Option<ExportFormat> {
    match requested {
        ExportFormat::Pdf => Some(ExportFormat::Md),
        _ => None,
    }
}

/// Outcome of the export/skill-runtime artifact egress gate.
pub enum ArtifactEgressOutcome {
    /// Egress refused — the artifact's text carries a confidential marker.
    Blocked { reason: String },
    /// Egress allowed; `artifact` is the **redacted** IR safe to render to a
    /// downloadable file. `mappings` lets the caller `restore()` later (reversible
    /// mode); `redacted` is the number of PII spans masked.
    Allowed {
        artifact: Artifact,
        mappings: Vec<crate::pii::PiiMatch>,
        redacted: usize,
        classification: Classification,
    },
}

/// Collect every user-visible text field of an [`Artifact`] in a stable order so
/// the egress gate can scan + redact it. Order is the render order; the same
/// order is used to write the redacted strings back in [`apply_redacted_strings`].
fn collect_artifact_strings(a: &Artifact) -> Vec<String> {
    let mut out = Vec::new();
    match a {
        Artifact::Table(t) => collect_table_strings(t, &mut out),
        Artifact::Document(d) => {
            if let Some(t) = &d.title {
                out.push(t.clone());
            }
            for b in &d.blocks {
                match b {
                    Block::Heading { text, .. } | Block::Paragraph { text } => out.push(text.clone()),
                    Block::List { items, .. } => out.extend(items.iter().cloned()),
                    Block::Table(t) => collect_table_strings(t, &mut out),
                }
            }
        }
    }
    out
}

fn collect_table_strings(t: &Table, out: &mut Vec<String>) {
    if let Some(title) = &t.title {
        out.push(title.clone());
    }
    out.extend(t.headers.iter().cloned());
    for row in &t.rows {
        out.extend(row.iter().cloned());
    }
}

/// Write the redacted strings back into a copy of the artifact, consuming
/// `redacted` in the exact same order [`collect_artifact_strings`] produced them.
fn apply_redacted_strings(a: &Artifact, redacted: &[String]) -> Artifact {
    let mut it = redacted.iter().cloned();
    let mut next = |slot: &mut String| {
        if let Some(v) = it.next() {
            *slot = v;
        }
    };
    match a {
        Artifact::Table(t) => {
            let mut t = t.clone();
            apply_table(&mut t, &mut next);
            Artifact::Table(t)
        }
        Artifact::Document(d) => {
            let mut d: Document = d.clone();
            if let Some(title) = &mut d.title {
                next(title);
            }
            for b in &mut d.blocks {
                match b {
                    Block::Heading { text, .. } | Block::Paragraph { text } => next(text),
                    Block::List { items, .. } => {
                        for item in items.iter_mut() {
                            next(item);
                        }
                    }
                    Block::Table(t) => apply_table(t, &mut next),
                }
            }
            Artifact::Document(d)
        }
    }
}

fn apply_table(t: &mut Table, next: &mut impl FnMut(&mut String)) {
    if let Some(title) = &mut t.title {
        next(title);
    }
    for h in t.headers.iter_mut() {
        next(h);
    }
    for row in t.rows.iter_mut() {
        for cell in row.iter_mut() {
            next(cell);
        }
    }
}

/// Single correct entry-point for the **export / skill-runtime file download**
/// egress (spec §3 "all egress points must flow through one gate"; closes the
/// INT-2 "document export bypasses redaction" hole for the rendered-artifact path).
///
/// A rendered office file (xlsx / docx / pdf / …) built from decrypted vault
/// content + LLM output is a genuine file egress: those bytes leave the device as
/// a download. This gate scans the artifact's text:
/// - confidential marker (`Classified`) ⇒ [`ArtifactEgressOutcome::Blocked`]
///   (fail-closed — the file is never rendered);
/// - otherwise the artifact's text fields are PII-redacted **in place** (reusing
///   the same engine as the LLM egress path) so the rendered file carries no
///   plaintext PII. Redaction is `Reversible` (`[KIND_N]` tokens) so the user can
///   `restore()` from `mappings`; pass `Irreversible` for untrusted sharing.
///
/// `extra_keywords` lets a pro plugin extend the confidential-keyword set
/// (industry markers); pass an empty slice for the generic OSS set.
pub fn enforce_artifact_egress(
    redactor: &Redactor,
    artifact: &Artifact,
    mode: RedactMode,
    extra_keywords: &[String],
) -> ArtifactEgressOutcome {
    let classifier = if extra_keywords.is_empty() {
        DocClassifier::new()
    } else {
        let combined = classifier::DEFAULT_CONFIDENTIAL_KEYWORDS
            .iter()
            .map(|s| s.to_string())
            .chain(extra_keywords.iter().cloned());
        DocClassifier::with_keywords(combined)
    };

    let strings = collect_artifact_strings(artifact);
    // Scan the concatenated text once for the confidentiality grade. We join with
    // a newline so multi-keyword phrases never form across field boundaries.
    let joined = strings.join("\n");
    let cls = classifier.classify(&joined, &[]);

    if let GateDecision::Block = gate_for_classification(cls.classification) {
        return ArtifactEgressOutcome::Blocked {
            reason: cls
                .block_reason
                .unwrap_or_else(|| "classified artifact".to_string()),
        };
    }

    // Redact every text field with globally-unique placeholders, then write back.
    let (redacted_segments, mut mappings) = redactor.redact_batch(&strings);
    let redacted_count = mappings.len();

    // Irreversible mode: collapse placeholders to a fixed mask, drop mappings.
    let (final_segments, final_mappings) = match mode {
        RedactMode::Reversible => (redacted_segments, mappings),
        RedactMode::Irreversible => {
            const MASK: &str = "\u{2588}\u{2588}\u{2588}\u{2588}";
            // Replace longest placeholders first to avoid [PHONE_1] vs [PHONE_10].
            mappings.sort_by_key(|m| std::cmp::Reverse(m.placeholder.len()));
            let masked: Vec<String> = redacted_segments
                .into_iter()
                .map(|mut s| {
                    for m in &mappings {
                        if s.contains(&m.placeholder) {
                            s = s.replace(&m.placeholder, MASK);
                        }
                    }
                    s
                })
                .collect();
            (masked, Vec::new())
        }
    };

    let redacted_artifact = apply_redacted_strings(artifact, &final_segments);
    ArtifactEgressOutcome::Allowed {
        artifact: redacted_artifact,
        mappings: final_mappings,
        redacted: redacted_count,
        classification: cls.classification,
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
    fn pdf_alt_format_suggests_md_for_pdf_only() {
        use crate::export::ExportFormat;
        assert_eq!(pdf_alt_format(ExportFormat::Pdf), Some(ExportFormat::Md));
        // Non-PDF formats redact in place → no alternative needed.
        assert_eq!(pdf_alt_format(ExportFormat::Docx), None);
        assert_eq!(pdf_alt_format(ExportFormat::Md), None);
        assert_eq!(pdf_alt_format(ExportFormat::Csv), None);
        assert_eq!(pdf_alt_format(ExportFormat::Xlsx), None);
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

    // ── enforce_file_egress: the INT-2 "export bypasses redaction" closure ────

    #[test]
    fn file_egress_blocks_classified_export() {
        let r = Redactor::new();
        let classified_text = "绝密：本文件禁止外传，含电话 13800138000";
        let out = enforce_file_egress(
            &r,
            classified_text,
            "txt",
            classified_text.as_bytes(),
            RedactMode::Reversible,
        )
        .unwrap();
        match out {
            FileEgressOutcome::Blocked { reason } => assert!(reason.contains("绝密")),
            FileEgressOutcome::Allowed(_) => panic!("classified file must be blocked from export"),
        }
    }

    #[test]
    fn file_egress_redacts_normal_export_no_plaintext_pii() {
        let r = Redactor::new();
        let text = "客户电话 13800138000 邮箱 c@d.com";
        let out = enforce_file_egress(&r, text, "txt", text.as_bytes(), RedactMode::Reversible)
            .unwrap();
        match out {
            FileEgressOutcome::Allowed(redaction) => {
                let s = String::from_utf8(redaction.bytes.clone()).unwrap();
                // The exported file carries NO plaintext PII (hole closed).
                assert!(!s.contains("13800138000"), "exported file must not carry phone");
                assert!(!s.contains("c@d.com"), "exported file must not carry email");
                // reversible: importer can restore
                let restored = r.restore(&s, &redaction.mappings);
                assert_eq!(restored, text);
            }
            FileEgressOutcome::Blocked { .. } => panic!("normal doc must be exportable (redacted)"),
        }
    }

    #[test]
    fn file_egress_unsupported_pdf_fails_closed() {
        let r = Redactor::new();
        let res = enforce_file_egress(&r, "phone 13800138000", "pdf", b"%PDF", RedactMode::Reversible);
        assert!(
            matches!(res, Err(RedactError::UnsupportedFormat(ref f)) if f == "pdf"),
            "PDF export must fail closed, never emit a half-redacted file"
        );
    }

    // ── enforce_artifact_egress: the export / skill-runtime download closure ──

    use crate::export::{Artifact, Block, Document, Table};

    fn doc_with(title: &str, paras: &[&str]) -> Artifact {
        Artifact::Document(Document {
            title: Some(title.to_string()),
            blocks: paras
                .iter()
                .map(|p| Block::Paragraph { text: p.to_string() })
                .collect(),
        })
    }

    /// Adversarial: a confidential artifact (classified marker in a paragraph) is
    /// fail-closed blocked — the file is never rendered.
    #[test]
    fn artifact_egress_blocks_classified_export() {
        let r = Redactor::new();
        let art = doc_with("交付物", &["绝密：本报告禁止外传，含电话 13800138000"]);
        match enforce_artifact_egress(&r, &art, RedactMode::Reversible, &[]) {
            ArtifactEgressOutcome::Blocked { reason } => assert!(reason.contains("绝密")),
            ArtifactEgressOutcome::Allowed { .. } => {
                panic!("classified artifact must be blocked from export")
            }
        }
    }

    /// A normal artifact carrying PII is exported **redacted** — the rendered text
    /// (and the IR fed to the renderer) carries no plaintext PII; reversible.
    #[test]
    fn artifact_egress_redacts_normal_export_no_plaintext_pii() {
        let r = Redactor::new();
        let art = doc_with(
            "客户清单",
            &["张三 电话 13800138000", "李四 邮箱 c@d.com"],
        );
        match enforce_artifact_egress(&r, &art, RedactMode::Reversible, &[]) {
            ArtifactEgressOutcome::Allowed { artifact, mappings, redacted, classification } => {
                assert_eq!(classification, Classification::Normal);
                assert!(redacted >= 2, "phone + email detected; got {redacted}");
                // Render to markdown and prove no plaintext PII leaves.
                let bytes = artifact.render(crate::export::ExportFormat::Md).unwrap();
                let md = String::from_utf8(bytes).unwrap();
                assert!(!md.contains("13800138000"), "rendered file must not carry phone");
                assert!(!md.contains("c@d.com"), "rendered file must not carry email");
                // Reversible: the placeholders restore to originals.
                let restored = r.restore(&md, &mappings);
                assert!(restored.contains("13800138000"));
                assert!(restored.contains("c@d.com"));
            }
            ArtifactEgressOutcome::Blocked { .. } => panic!("normal artifact must be exportable"),
        }
    }

    /// Table cells carrying PII are redacted across all rows.
    #[test]
    fn artifact_egress_redacts_table_cells() {
        let r = Redactor::new();
        let art = Artifact::Table(Table {
            title: Some("联系人".to_string()),
            headers: vec!["姓名".to_string(), "电话".to_string()],
            rows: vec![
                vec!["张三".to_string(), "13800138000".to_string()],
                vec!["李四".to_string(), "13900139000".to_string()],
            ],
            aligns: vec![],
        });
        match enforce_artifact_egress(&r, &art, RedactMode::Reversible, &[]) {
            ArtifactEgressOutcome::Allowed { artifact, redacted, .. } => {
                assert_eq!(redacted, 2, "two phones masked");
                let bytes = artifact.render(crate::export::ExportFormat::Csv).unwrap();
                let csv = String::from_utf8(bytes).unwrap();
                assert!(!csv.contains("13800138000"));
                assert!(!csv.contains("13900139000"));
                // structure preserved: header + 2 rows still render
                assert!(csv.contains("姓名") && csv.contains("张三") && csv.contains("李四"));
            }
            ArtifactEgressOutcome::Blocked { .. } => panic!("table must be exportable"),
        }
    }

    /// Irreversible mode masks PII with the block char and emits no restore map.
    #[test]
    fn artifact_egress_irreversible_has_no_mappings() {
        let r = Redactor::new();
        let art = doc_with("x", &["phone 13800138000"]);
        match enforce_artifact_egress(&r, &art, RedactMode::Irreversible, &[]) {
            ArtifactEgressOutcome::Allowed { artifact, mappings, .. } => {
                assert!(mappings.is_empty(), "irreversible mode emits no restore map");
                let bytes = artifact.render(crate::export::ExportFormat::Md).unwrap();
                let md = String::from_utf8(bytes).unwrap();
                assert!(!md.contains("13800138000"));
                assert!(md.contains('\u{2588}'), "irreversible mask expected");
            }
            ArtifactEgressOutcome::Blocked { .. } => panic!("must export"),
        }
    }

    /// A clean artifact (no PII, no markers) passes through unchanged.
    #[test]
    fn artifact_egress_clean_passes_through() {
        let r = Redactor::new();
        let art = doc_with("公开报告", &["这是一份公开的技术说明，无敏感信息。"]);
        match enforce_artifact_egress(&r, &art, RedactMode::Reversible, &[]) {
            ArtifactEgressOutcome::Allowed { redacted, classification, .. } => {
                assert_eq!(redacted, 0);
                assert_eq!(classification, Classification::Normal);
            }
            ArtifactEgressOutcome::Blocked { .. } => panic!("clean artifact must export"),
        }
    }

    /// Pro-plugin extra keywords extend the block set (industry marker).
    #[test]
    fn artifact_egress_extra_keywords_block() {
        let r = Redactor::new();
        let art = doc_with("案卷", &["本案卷密，禁止公开"]);
        // generic set does NOT contain 案卷密 → not blocked
        assert!(matches!(
            enforce_artifact_egress(&r, &art, RedactMode::Reversible, &[]),
            ArtifactEgressOutcome::Allowed { .. }
        ));
        // pro plugin injects 案卷密 → blocked
        match enforce_artifact_egress(&r, &art, RedactMode::Reversible, &["案卷密".to_string()]) {
            ArtifactEgressOutcome::Blocked { reason } => assert!(reason.contains("案卷密")),
            ArtifactEgressOutcome::Allowed { .. } => panic!("pro keyword must block"),
        }
    }

    /// Round-trip field-order invariant: redacted strings map back to the exact
    /// same slots (title, headings, list items, nested table) they came from.
    #[test]
    fn artifact_egress_preserves_field_order_with_nested_blocks() {
        let r = Redactor::new();
        let art = Artifact::Document(Document {
            title: Some("报告 13800138000".to_string()),
            blocks: vec![
                Block::Heading { level: 1, text: "概述 c@d.com".to_string() },
                Block::List { ordered: false, items: vec!["项目 13900139000".to_string()] },
                Block::Table(Table {
                    title: None,
                    headers: vec!["列".to_string()],
                    rows: vec![vec!["x@y.com".to_string()]],
                    aligns: vec![],
                }),
            ],
        });
        match enforce_artifact_egress(&r, &art, RedactMode::Reversible, &[]) {
            ArtifactEgressOutcome::Allowed { artifact, mappings, redacted, .. } => {
                assert_eq!(redacted, 4, "4 PII spans across title/heading/list/table");
                let bytes = artifact.render(crate::export::ExportFormat::Md).unwrap();
                let md = String::from_utf8(bytes).unwrap();
                for raw in ["13800138000", "c@d.com", "13900139000", "x@y.com"] {
                    assert!(!md.contains(raw), "raw {raw} must be redacted");
                }
                // restore recovers every original (proves correct slot mapping)
                let restored = r.restore(&md, &mappings);
                for raw in ["13800138000", "c@d.com", "13900139000", "x@y.com"] {
                    assert!(restored.contains(raw), "restore must recover {raw}");
                }
            }
            ArtifactEgressOutcome::Blocked { .. } => panic!("must export"),
        }
    }
}
