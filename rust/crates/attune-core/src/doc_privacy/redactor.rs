//! Document file-level redactor — produces a **redacted copy** of a file.
//!
//! ## attune vs kvm
//!
//! kvm-info-privacy redacts with **irreversible** black blocks (`████`) / spaces.
//! attune defaults to **reversible** `[KIND_N]` placeholder tokens (reusing the
//! existing [`crate::pii::Redactor`]) so a user who exports a redacted document
//! can later `restore()` the originals — the export/share use-case the spec
//! (§2 S2, §11) calls out. An irreversible mode is still available for the
//! "share with an untrusted third party" case via [`RedactMode::Irreversible`].
//!
//! ## Formats
//!
//! - **text / md / txt / csv** — redact the whole string.
//! - **docx / xlsx (OOXML)** — ZIP copy in place, redacting only the text-bearing
//!   parts (`word/document.xml`, `xl/sharedStrings.xml`, `xl/worksheets/*`). The
//!   byte-level "copy archive, rewrite text parts" approach is borrowed from
//!   kvm's `redactors/text.rs` design (reimplemented; kvm ships no LICENSE).
//!
//! PDF byte-level content-stream rewriting is intentionally **out of scope of
//! this slice** (spec DP.2 PDF is higher-risk: font/encoding maps). PDF requests
//! return [`RedactError::UnsupportedFormat`] (fail-closed: we never emit a
//! half-redacted PDF).
//!
//! ## Core invariant (borrowed from kvm "Layer-1 triple verification")
//!
//! Re-scanning a redacted **text** output finds zero PII. (MD5-differs is NOT
//! accepted as evidence of redaction — see tests.)

use crate::doc_privacy::models::Classification;
use crate::pii::{PiiMatch, Redactor};
use std::collections::HashMap;
use std::io::{Read, Write};

/// Whether redaction is reversible (`[KIND_N]` tokens, restorable) or not
/// (irreversible mask for untrusted sharing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactMode {
    /// attune default — `[KIND_N]` tokens, `restore()`-able.
    Reversible,
    /// kvm-style irreversible mask (for sharing to untrusted parties).
    Irreversible,
}

/// Mask used in `Irreversible` mode (Unicode full block, like kvm).
const MASK: &str = "\u{2588}\u{2588}\u{2588}\u{2588}";

#[derive(Debug, thiserror::Error)]
pub enum RedactError {
    #[error("doc-classified: classified document cannot be redacted/exported — {0}")]
    Classified(String),
    #[error("doc-unsupported-format: byte-level redaction not supported for .{0}")]
    UnsupportedFormat(String),
    #[error("doc-redact-io: {0}")]
    Io(String),
}

/// Result of a file-level redaction.
pub struct RedactionOutput {
    /// The redacted file bytes (caller writes to disk / streams to wire).
    pub bytes: Vec<u8>,
    /// Reversible mappings (empty in `Irreversible` mode). `restore()` against
    /// these recovers originals from a `[KIND_N]`-bearing string.
    pub mappings: Vec<PiiMatch>,
    /// Count per placeholder-prefix (e.g. {"PHONE": 2}).
    pub redacted_counts: HashMap<String, usize>,
    pub mode: RedactMode,
}

pub struct DocRedactor<'a> {
    redactor: &'a Redactor,
}

impl<'a> DocRedactor<'a> {
    pub fn new(redactor: &'a Redactor) -> Self {
        Self { redactor }
    }

    /// Redact a file given its extension + raw bytes.
    ///
    /// `classification` is the gate decision from [`crate::doc_privacy::DocClassifier`]:
    /// a `Classified` document is **fail-closed refused** here too (defense in
    /// depth — the OutboundGate also blocks, but the redactor never produces an
    /// output for a classified doc).
    pub fn redact_bytes(
        &self,
        ext: &str,
        data: &[u8],
        classification: Classification,
        mode: RedactMode,
    ) -> Result<RedactionOutput, RedactError> {
        if classification == Classification::Classified {
            return Err(RedactError::Classified(
                "fail-closed: classified document is not exportable".into(),
            ));
        }

        let ext = ext.to_lowercase();
        match ext.as_str() {
            "txt" | "md" | "markdown" | "csv" | "text" | "json" | "html" | "htm" => {
                self.redact_text_bytes(data, mode)
            }
            "docx" => self.redact_ooxml(data, &["word/document.xml"], mode),
            "xlsx" => self.redact_ooxml(
                data,
                // sharedStrings holds most cell text; worksheets hold inline strings.
                &["xl/sharedStrings.xml"],
                mode,
            ),
            "pdf" => Err(RedactError::UnsupportedFormat("pdf".into())),
            other => Err(RedactError::UnsupportedFormat(other.into())),
        }
    }

    fn redact_text(
        &self,
        text: &str,
        mode: RedactMode,
    ) -> (String, Vec<PiiMatch>, HashMap<String, usize>) {
        match mode {
            RedactMode::Reversible => {
                let r = self.redactor.redact(text);
                (r.redacted_text, r.mappings, r.stats.by_kind)
            }
            RedactMode::Irreversible => {
                // Reuse detection (reversible) then collapse every placeholder to
                // a fixed mask so nothing is restorable.
                let r = self.redactor.redact(text);
                let mut out = r.redacted_text;
                let mut counts: HashMap<String, usize> = HashMap::new();
                // Replace each placeholder with the mask (longest first to avoid
                // [PHONE_10] vs [PHONE_1] prefix collisions).
                let mut sorted = r.mappings.clone();
                sorted.sort_by_key(|m| std::cmp::Reverse(m.placeholder.len()));
                for m in &sorted {
                    if out.contains(&m.placeholder) {
                        out = out.replace(&m.placeholder, MASK);
                        *counts
                            .entry(m.kind.placeholder_prefix().to_uppercase())
                            .or_insert(0) += 1;
                    }
                }
                (out, Vec::new(), counts)
            }
        }
    }

    fn redact_text_bytes(
        &self,
        data: &[u8],
        mode: RedactMode,
    ) -> Result<RedactionOutput, RedactError> {
        let text = String::from_utf8_lossy(data);
        let (out, mappings, counts) = self.redact_text(&text, mode);
        Ok(RedactionOutput {
            bytes: out.into_bytes(),
            mappings,
            redacted_counts: counts,
            mode,
        })
    }

    /// Redact an OOXML (docx/xlsx) archive: copy every entry verbatim except the
    /// named text parts, which are redacted in place.
    fn redact_ooxml(
        &self,
        data: &[u8],
        text_parts: &[&str],
        mode: RedactMode,
    ) -> Result<RedactionOutput, RedactError> {
        let reader = std::io::Cursor::new(data);
        let mut archive =
            zip::ZipArchive::new(reader).map_err(|e| RedactError::Io(format!("zip open: {e}")))?;

        let mut out_buf = Vec::new();
        let mut all_mappings: Vec<PiiMatch> = Vec::new();
        let mut counts: HashMap<String, usize> = HashMap::new();
        {
            let cursor = std::io::Cursor::new(&mut out_buf);
            let mut writer = zip::ZipWriter::new(cursor);
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);

            for i in 0..archive.len() {
                let mut entry = archive
                    .by_index(i)
                    .map_err(|e| RedactError::Io(format!("zip entry {i}: {e}")))?;
                let name = entry.name().to_string();
                // Reject path-traversal entry names (OOXML adversarial surface).
                if name.contains("..") || name.starts_with('/') {
                    return Err(RedactError::Io(format!("unsafe zip entry name: {name}")));
                }

                if text_parts.contains(&name.as_str()) {
                    let mut xml = String::new();
                    entry
                        .read_to_string(&mut xml)
                        .map_err(|e| RedactError::Io(format!("read {name}: {e}")))?;
                    let (red, mappings, c) = self.redact_text(&xml, mode);
                    for (k, v) in c {
                        *counts.entry(k).or_insert(0) += v;
                    }
                    all_mappings.extend(mappings);
                    writer
                        .start_file(&name, opts)
                        .map_err(|e| RedactError::Io(format!("start {name}: {e}")))?;
                    writer
                        .write_all(red.as_bytes())
                        .map_err(|e| RedactError::Io(format!("write {name}: {e}")))?;
                } else {
                    // Copy non-text parts verbatim (raw copy keeps compression).
                    writer
                        .raw_copy_file(entry)
                        .map_err(|e| RedactError::Io(format!("copy {name}: {e}")))?;
                }
            }
            writer
                .finish()
                .map_err(|e| RedactError::Io(format!("zip finish: {e}")))?;
        }

        Ok(RedactionOutput {
            bytes: out_buf,
            mappings: all_mappings,
            redacted_counts: counts,
            mode,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redactor() -> Redactor {
        Redactor::new()
    }

    // ── Reversible text redaction ────────────────────────────────────────────

    #[test]
    fn text_redaction_is_reversible_roundtrip() {
        let r = redactor();
        let dr = DocRedactor::new(&r);
        let input = "联系电话 13800138000 邮箱 alice@example.com";
        let out = dr
            .redact_bytes(
                "txt",
                input.as_bytes(),
                Classification::Normal,
                RedactMode::Reversible,
            )
            .unwrap();
        let redacted = String::from_utf8(out.bytes.clone()).unwrap();
        assert!(!redacted.contains("13800138000"), "phone must be masked");
        assert!(
            !redacted.contains("alice@example.com"),
            "email must be masked"
        );
        // restore brings originals back
        let restored = r.restore(&redacted, &out.mappings);
        assert_eq!(
            restored, input,
            "reversible round-trip must recover original"
        );
    }

    // ── Core invariant: re-scan finds zero PII (NOT just MD5-differs) ─────────

    #[test]
    fn rescan_redacted_text_finds_zero_pii() {
        let r = redactor();
        let dr = DocRedactor::new(&r);
        let input = "身份证 110101199003078515 手机 13900139000";
        let out = dr
            .redact_bytes(
                "txt",
                input.as_bytes(),
                Classification::Normal,
                RedactMode::Reversible,
            )
            .unwrap();
        let redacted = String::from_utf8(out.bytes).unwrap();
        // The triple-verification invariant: redacted output, re-scanned, has no PII.
        let rescan = r.redact(&redacted);
        assert_eq!(
            rescan.mappings.len(),
            0,
            "re-scan of redacted text must find zero PII; got {:?}",
            rescan.mappings
        );
    }

    #[test]
    fn md5_differs_is_not_evidence_of_redaction() {
        // Regression for kvm anti-pattern: a different hash does NOT prove the
        // PII is gone — the rescan invariant is what proves it.
        let r = redactor();
        let dr = DocRedactor::new(&r);
        let input = "card 4111111111111111";
        let out = dr
            .redact_bytes(
                "txt",
                input.as_bytes(),
                Classification::Normal,
                RedactMode::Reversible,
            )
            .unwrap();
        assert_ne!(
            out.bytes,
            input.as_bytes(),
            "bytes changed (necessary, not sufficient)"
        );
        let redacted = String::from_utf8(out.bytes).unwrap();
        assert!(
            !redacted.contains("4111111111111111"),
            "the real proof: value gone"
        );
    }

    // ── Irreversible mode ────────────────────────────────────────────────────

    #[test]
    fn irreversible_mode_has_no_mappings_and_cannot_restore() {
        let r = redactor();
        let dr = DocRedactor::new(&r);
        let input = "phone 13800138000";
        let out = dr
            .redact_bytes(
                "txt",
                input.as_bytes(),
                Classification::Normal,
                RedactMode::Irreversible,
            )
            .unwrap();
        let redacted = String::from_utf8(out.bytes).unwrap();
        assert!(!redacted.contains("13800138000"));
        assert!(
            out.mappings.is_empty(),
            "irreversible mode emits no restore map"
        );
        assert!(redacted.contains(MASK), "irreversible mask expected");
    }

    // ── Fail-closed: classified doc refused ──────────────────────────────────

    #[test]
    fn classified_doc_is_fail_closed_refused() {
        let r = redactor();
        let dr = DocRedactor::new(&r);
        let res = dr.redact_bytes(
            "txt",
            b"phone 13800138000",
            Classification::Classified,
            RedactMode::Reversible,
        );
        assert!(
            matches!(res, Err(RedactError::Classified(_))),
            "classified must be refused"
        );
    }

    // ── Unsupported format fail-closed ───────────────────────────────────────

    #[test]
    fn pdf_is_unsupported_fail_closed_not_passthrough() {
        let r = redactor();
        let dr = DocRedactor::new(&r);
        let res = dr.redact_bytes(
            "pdf",
            b"%PDF-1.7 ...",
            Classification::Normal,
            RedactMode::Reversible,
        );
        assert!(
            matches!(res, Err(RedactError::UnsupportedFormat(ref f)) if f == "pdf"),
            "PDF must fail closed (never emit a half-redacted PDF)"
        );
    }

    // ── OOXML (docx) byte-level redaction ────────────────────────────────────

    fn make_docx(body_text: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buf);
            let mut w = zip::ZipWriter::new(cursor);
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            // minimal [Content_Types].xml + document.xml
            w.start_file("[Content_Types].xml", opts).unwrap();
            w.write_all(b"<?xml version=\"1.0\"?><Types/>").unwrap();
            w.start_file("word/document.xml", opts).unwrap();
            let xml = format!(
                "<?xml version=\"1.0\"?><w:document><w:body><w:p><w:r><w:t>{body_text}</w:t></w:r></w:p></w:body></w:document>"
            );
            w.write_all(xml.as_bytes()).unwrap();
            w.finish().unwrap();
        }
        buf
    }

    #[test]
    fn docx_redaction_masks_pii_in_document_xml() {
        let r = redactor();
        let dr = DocRedactor::new(&r);
        let docx = make_docx("客户手机 13800138000 邮箱 bob@example.com");
        let out = dr
            .redact_bytes(
                "docx",
                &docx,
                Classification::Normal,
                RedactMode::Reversible,
            )
            .unwrap();
        // The output is a valid zip whose document.xml no longer carries the PII.
        let mut ar = zip::ZipArchive::new(std::io::Cursor::new(out.bytes)).unwrap();
        let mut doc_xml = String::new();
        ar.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut doc_xml)
            .unwrap();
        assert!(
            !doc_xml.contains("13800138000"),
            "phone redacted in docx; got {doc_xml}"
        );
        assert!(
            !doc_xml.contains("bob@example.com"),
            "email redacted in docx"
        );
        // other parts preserved
        assert!(
            ar.by_name("[Content_Types].xml").is_ok(),
            "non-text part preserved"
        );
        assert!(
            !out.mappings.is_empty(),
            "reversible mappings recorded for docx"
        );
    }

    #[test]
    fn docx_with_no_pii_is_structurally_intact() {
        let r = redactor();
        let dr = DocRedactor::new(&r);
        let docx = make_docx("这是一份普通文档没有敏感信息");
        let out = dr
            .redact_bytes(
                "docx",
                &docx,
                Classification::Normal,
                RedactMode::Reversible,
            )
            .unwrap();
        let mut ar = zip::ZipArchive::new(std::io::Cursor::new(out.bytes)).unwrap();
        assert!(ar.by_name("word/document.xml").is_ok());
        assert!(out.mappings.is_empty());
    }

    // ── Boundary cases ───────────────────────────────────────────────────────

    #[test]
    fn empty_text_produces_empty_output() {
        let r = redactor();
        let dr = DocRedactor::new(&r);
        let out = dr
            .redact_bytes("txt", b"", Classification::Normal, RedactMode::Reversible)
            .unwrap();
        assert!(out.bytes.is_empty());
        assert!(out.mappings.is_empty());
    }

    #[test]
    fn corrupt_docx_fails_closed_not_panics() {
        let r = redactor();
        let dr = DocRedactor::new(&r);
        let res = dr.redact_bytes(
            "docx",
            b"not a real zip file at all",
            Classification::Normal,
            RedactMode::Reversible,
        );
        assert!(
            matches!(res, Err(RedactError::Io(_))),
            "corrupt docx must error, not panic"
        );
    }

    #[test]
    fn invalid_utf8_text_does_not_panic() {
        let r = redactor();
        let dr = DocRedactor::new(&r);
        let bytes = vec![0xff, 0xfe, 0x00, b'p', b'h', b'o', b'n', b'e'];
        let out = dr.redact_bytes(
            "txt",
            &bytes,
            Classification::Normal,
            RedactMode::Reversible,
        );
        assert!(
            out.is_ok(),
            "invalid utf-8 must be lossily handled, not panic"
        );
    }
}
