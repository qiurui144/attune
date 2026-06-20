//! Document render — map a writing-engine [`WritingResult`] into a [`Document`] Artifact.
//!
//! The bridge between "what `reference_generate` / `research_synthesis` produced" (a grounded
//! narrative with heading/body sections) and "what the export engine writes" (a multi-block
//! [`Document`] → docx / pdf / md). **Zero-cost** (pure CPU, no LLM).
//!
//! ## Why this and not `writing::WritingResult` directly?
//! The writing engine emits a flat `content: String` + `segments` with UTF-16 offsets +
//! `unverified_spans`. The export `Document` IR wants structured blocks (heading / paragraph).
//! This module reconstructs the heading/body structure (the synthesis/reference content is built
//! as `"{heading}\n{body}"` per section, joined by blank lines) into [`Block`]s, and — crucially —
//! it surfaces the **grounding verdict**: an unverified (ungrounded) segment is annotated with a
//! `[需核实]` marker so a fabricated/unsupported sentence is visible in the downloaded file rather
//! than silently shipped as fact (spec §11 risk A/D, the hallucination red line).

use crate::export::{Artifact, Block, Document};
use crate::writing::WritingResult;

/// The marker appended to an unverified (ungrounded) sentence in the rendered document so the
/// reader sees which claims could not be traced back to a source (spec §6 / §11 A).
pub const UNVERIFIED_MARKER: &str = "［需核实］";

/// Build a [`Document`] Artifact from a writing-engine [`WritingResult`].
///
/// `title` becomes the document H1. The content is split back into `heading\nbody` sections
/// (the synthesis/reference assembler joins sections with a blank line and puts the heading on
/// its own first line). Unverified spans are marked inline with [`UNVERIFIED_MARKER`].
pub fn writing_to_document(result: &WritingResult, title: &str) -> Artifact {
    let marked = annotate_unverified(result);
    let mut blocks = Vec::new();
    for section in split_sections(&marked) {
        match section {
            Section::Heading(h) => blocks.push(Block::Heading { level: 2, text: h }),
            Section::Body(b) => {
                // A body may contain multiple paragraphs (blank-line separated within a section
                // is unusual, but split defensively so each becomes its own paragraph block).
                for para in b.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
                    blocks.push(Block::Paragraph { text: para.to_string() });
                }
            }
        }
    }
    // Never emit a structurally-empty document: if the model produced nothing parseable, keep at
    // least one paragraph carrying the (possibly empty) content so the export still yields a file.
    if blocks.is_empty() && !marked.trim().is_empty() {
        blocks.push(Block::Paragraph { text: marked.trim().to_string() });
    }
    Artifact::document(Document {
        title: Some(title.to_string()),
        blocks,
    })
}

/// Build a [`Document`] from explicit (heading, body) sections — used by `reference_generate`,
/// which assembles its output section-by-section against a reference skeleton. `unverified`
/// section indices get the [`UNVERIFIED_MARKER`] on their body.
pub fn sections_to_document(
    title: &str,
    sections: &[(String, String)],
    unverified_idx: &[usize],
) -> Artifact {
    let mut blocks = Vec::new();
    for (i, (heading, body)) in sections.iter().enumerate() {
        if !heading.trim().is_empty() {
            blocks.push(Block::Heading { level: 2, text: heading.trim().to_string() });
        }
        let mut body = body.trim().to_string();
        if unverified_idx.contains(&i) && !body.is_empty() {
            body.push_str(UNVERIFIED_MARKER);
        }
        for para in body.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
            blocks.push(Block::Paragraph { text: para.to_string() });
        }
    }
    Artifact::document(Document {
        title: Some(title.to_string()),
        blocks,
    })
}

/// One reconstructed section of a writing result.
enum Section {
    Heading(String),
    Body(String),
}

/// Split the assembled `content` back into heading/body sections. The synthesis assembler writes
/// `"{heading}\n{body}"` per section, joined by `"\n\n"`. We treat the first line of each
/// double-newline-separated block as the heading and the rest as the body.
fn split_sections(content: &str) -> Vec<Section> {
    let mut out = Vec::new();
    for block in content.split("\n\n") {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }
        let mut lines = block.splitn(2, '\n');
        let heading = lines.next().unwrap_or("").trim();
        let body = lines.next().map(str::trim).unwrap_or("");
        if !heading.is_empty() {
            out.push(Section::Heading(heading.to_string()));
        }
        if !body.is_empty() {
            out.push(Section::Body(body.to_string()));
        } else if heading.is_empty() {
            // a single-line block with no heading split → treat the whole as body
            out.push(Section::Body(block.to_string()));
        }
    }
    out
}

/// Insert the [`UNVERIFIED_MARKER`] at the end of every unverified segment's text in `content`,
/// working from the back so earlier offsets stay valid. Offsets are UTF-16 code units.
fn annotate_unverified(result: &WritingResult) -> String {
    if result.unverified_spans.is_empty() {
        return result.content.clone();
    }
    // Map each unverified span end (UTF-16) to a char-boundary insertion point.
    let mut ends: Vec<u32> = result.unverified_spans.iter().map(|s| s[1]).collect();
    ends.sort_unstable();
    ends.dedup();

    let chars: Vec<char> = result.content.chars().collect();
    // Precompute the UTF-16 prefix length at each char boundary.
    let mut u16_at = Vec::with_capacity(chars.len() + 1);
    let mut acc = 0u32;
    u16_at.push(0u32);
    for ch in &chars {
        acc += ch.len_utf16() as u32;
        u16_at.push(acc);
    }
    let mut out = String::with_capacity(result.content.len() + ends.len() * UNVERIFIED_MARKER.len());
    let mut next_end = 0usize; // index into `ends`
    for (i, ch) in chars.iter().enumerate() {
        out.push(*ch);
        let boundary_u16 = u16_at[i + 1];
        while next_end < ends.len() && ends[next_end] == boundary_u16 {
            out.push_str(UNVERIFIED_MARKER);
            next_end += 1;
        }
    }
    // Any span ending past the content end (defensive) → append once.
    if next_end < ends.len() {
        out.push_str(UNVERIFIED_MARKER);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_intelligence::token_bill::TokenBill;
    use crate::writing::{Segment, WritingMode, WritingResult, WRITING_SCHEMA_VERSION};

    fn result(content: &str, unverified: Vec<[u32; 2]>) -> WritingResult {
        WritingResult {
            schema_version: WRITING_SCHEMA_VERSION,
            mode: WritingMode::Synthesis,
            content: content.to_string(),
            segments: vec![Segment {
                text: content.to_string(),
                offset: [0, content.chars().map(|c| c.len_utf16() as u32).sum()],
                grounding: vec![],
                verified: unverified.is_empty(),
            }],
            annotations: vec![],
            unverified_spans: unverified,
            token_bill: TokenBill::default(),
        }
    }

    #[test]
    fn sections_become_heading_and_paragraph_blocks() {
        let content = "背景\n这是背景内容。\n\n方案\n这是方案内容。";
        let art = writing_to_document(&result(content, vec![]), "研究综合");
        let Artifact::Document(d) = art else { panic!("expected document") };
        assert_eq!(d.title.as_deref(), Some("研究综合"));
        // 2 headings + 2 paragraphs
        assert_eq!(d.blocks.len(), 4);
        assert!(matches!(&d.blocks[0], Block::Heading { level: 2, text } if text == "背景"));
        assert!(matches!(&d.blocks[1], Block::Paragraph { text } if text == "这是背景内容。"));
        assert!(matches!(&d.blocks[2], Block::Heading { level: 2, text } if text == "方案"));
        assert!(d.validate().is_ok());
    }

    #[test]
    fn unverified_span_gets_marker() {
        // The whole single-segment content is unverified → marker appended at its end.
        let content = "标题\n一段未核实的断言。";
        let total: u32 = content.chars().map(|c| c.len_utf16() as u32).sum();
        let art = writing_to_document(&result(content, vec![[0, total]]), "doc");
        let Artifact::Document(d) = art else { panic!() };
        // body paragraph must carry the marker
        let has_marker = d.blocks.iter().any(|b| matches!(b, Block::Paragraph { text } if text.contains(UNVERIFIED_MARKER)));
        assert!(has_marker, "unverified body must be marked: {:?}", d.blocks);
    }

    #[test]
    fn sections_to_document_marks_listed_unverified() {
        let secs = vec![
            ("背景".to_string(), "已核实背景。".to_string()),
            ("依据".to_string(), "未核实依据。".to_string()),
        ];
        let art = sections_to_document("标书", &secs, &[1]);
        let Artifact::Document(d) = art else { panic!() };
        // section 1 (依据) body must carry the marker; section 0 must not.
        let para_texts: Vec<&String> = d.blocks.iter().filter_map(|b| match b {
            Block::Paragraph { text } => Some(text),
            _ => None,
        }).collect();
        assert!(para_texts.iter().any(|t| t.contains("未核实依据") && t.contains(UNVERIFIED_MARKER)));
        assert!(para_texts.iter().any(|t| t.contains("已核实背景") && !t.contains(UNVERIFIED_MARKER)));
    }

    #[test]
    fn empty_content_yields_valid_empty_document() {
        let art = writing_to_document(&result("", vec![]), "空");
        let Artifact::Document(d) = art else { panic!() };
        assert!(d.blocks.is_empty());
        assert!(d.validate().is_ok(), "headerless empty doc is still valid");
    }

    #[test]
    fn marker_offsets_with_cjk_and_emoji() {
        // content with an emoji (2 UTF-16 units) before the unverified span boundary.
        let content = "标题\nhi 😀 未核实。";
        let total: u32 = content.chars().map(|c| c.len_utf16() as u32).sum();
        let art = writing_to_document(&result(content, vec![[0, total]]), "doc");
        let Artifact::Document(d) = art else { panic!() };
        // No panic + marker present somewhere.
        let s = serde_json::to_string(&d).unwrap();
        assert!(s.contains(UNVERIFIED_MARKER));
    }
}
