//! pdf renderer (typst via `typst-as-lib` + `typst-pdf`).
//!
//! **CJK correctness (spec R1, the top export risk):** a subset of WenQuanYi
//! Micro Hei is embedded with `include_bytes!` and registered as the only font,
//! and the generated typst markup sets `#set text(font: "WenQuanYi Micro Hei")`.
//! This guarantees Chinese text renders on any host without system fonts and that
//! a `pdf-extract` round-trip reads the text back **un-garbled**.
//!
//! The IR is lowered to typst *markup* (not eval'd typst code from the user), and
//! every user string is escaped, so a malicious cell cannot inject typst directives.

use super::super::{Align, Artifact, Block, Document, ExportError, Table};
use typst_as_lib::TypstEngine;
use typst_pdf::PdfOptions;

/// Subset CJK font embedded into the binary (spec R1). ~3 MB; covers the common
/// CJK Unified Ideographs block + ASCII/Latin-1/CJK punctuation.
static CJK_FONT: &[u8] = include_bytes!("../../../assets/fonts/AttuneCJK-subset.ttf");

/// The family name inside [`CJK_FONT`] (verified via fontTools).
const FONT_FAMILY: &str = "WenQuanYi Micro Hei";

/// Escape a string for safe inclusion in typst **markup** (content mode).
/// Neutralises the markup-significant characters so arbitrary text — including a
/// hostile cell value — renders literally and cannot inject typst syntax.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '#' => out.push_str("\\#"),
            '$' => out.push_str("\\$"),
            '*' => out.push_str("\\*"),
            '_' => out.push_str("\\_"),
            '`' => out.push_str("\\`"),
            '<' => out.push_str("\\<"),
            '>' => out.push_str("\\>"),
            '@' => out.push_str("\\@"),
            '=' => out.push_str("\\="),
            '-' => out.push_str("\\-"),
            '+' => out.push_str("\\+"),
            '/' => out.push_str("\\/"),
            '[' => out.push_str("\\["),
            ']' => out.push_str("\\]"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str(" \\ "), // hard line break in markup
            _ => out.push(c),
        }
    }
    out
}

fn align_kw(a: Align) -> &'static str {
    match a {
        Align::Left => "left",
        Align::Center => "center",
        Align::Right => "right",
    }
}

/// Emit a typst `#table(...)` for an IR table. Header cells are bolded.
fn table_markup(t: &Table) -> String {
    let mut m = String::new();
    if let Some(title) = &t.title {
        m.push_str(&format!("=== {}\n\n", esc(title)));
    }
    let cols = t.ncols();
    let aligns = (0..cols)
        .map(|i| align_kw(t.align_for(i)))
        .collect::<Vec<_>>()
        .join(", ");
    m.push_str(&format!(
        "#table(\n  columns: {cols},\n  align: ({aligns}),\n  stroke: 0.5pt,\n"
    ));
    // header
    m.push_str("  table.header(");
    m.push_str(
        &t.headers
            .iter()
            .map(|h| format!("[*{}*]", esc(h)))
            .collect::<Vec<_>>()
            .join(", "),
    );
    m.push_str("),\n");
    // rows
    for row in &t.rows {
        m.push_str("  ");
        m.push_str(
            &row.iter()
                .map(|c| format!("[{}]", esc(c)))
                .collect::<Vec<_>>()
                .join(", "),
        );
        m.push_str(",\n");
    }
    m.push_str(")\n\n");
    m
}

fn document_markup(d: &Document) -> String {
    let mut m = String::new();
    if let Some(title) = &d.title {
        m.push_str(&format!("= {}\n\n", esc(title)));
    }
    for block in &d.blocks {
        match block {
            Block::Heading { level, text } => {
                let lvl = (*level).clamp(1, 6) as usize;
                m.push_str(&"=".repeat(lvl));
                m.push(' ');
                m.push_str(&esc(text));
                m.push_str("\n\n");
            }
            Block::Paragraph { text } => {
                m.push_str(&esc(text));
                m.push_str("\n\n");
            }
            Block::List { ordered, items } => {
                for (i, item) in items.iter().enumerate() {
                    if *ordered {
                        m.push_str(&format!("+ {}\n", esc(item)));
                        let _ = i;
                    } else {
                        m.push_str(&format!("- {}\n", esc(item)));
                    }
                }
                m.push('\n');
            }
            Block::Table(t) => {
                m.push_str(&table_markup(t));
            }
        }
    }
    m
}

/// Build the full typst source: a preamble that pins the embedded CJK font, then
/// the lowered IR markup.
fn build_source(artifact: &Artifact) -> String {
    let body = match artifact {
        Artifact::Table(t) => table_markup(t),
        Artifact::Document(d) => document_markup(d),
    };
    format!(
        "#set text(font: \"{FONT_FAMILY}\", size: 11pt)\n\
         #set page(margin: 2cm)\n\
         #set par(justify: false)\n\n\
         {body}"
    )
}

pub fn render(artifact: &Artifact) -> Result<Vec<u8>, ExportError> {
    let source = build_source(artifact);

    let engine = TypstEngine::builder()
        .main_file(source)
        .fonts([CJK_FONT])
        .build();

    // typst 0.14: PagedDocument lives in typst::layout (re-export of typst_library).
    let doc: typst::layout::PagedDocument = engine
        .compile()
        .output
        .map_err(|e| ExportError::RenderFailed(format!("typst compile: {e:?}")))?;

    typst_pdf::pdf(&doc, &PdfOptions::default())
        .map_err(|e| ExportError::RenderFailed(format!("typst pdf: {e:?}")))
}
