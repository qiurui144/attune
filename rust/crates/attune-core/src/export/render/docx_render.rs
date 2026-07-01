//! docx renderer (`docx-rs`). Documents map blocks → Word paragraphs / tables;
//! a bare table maps to a single Word table. Headings are bold + sized; lists use
//! a literal bullet/number prefix (robust, no numbering.xml dependency).
//!
//! Round-trip (spec §9.1) re-reads `word/document.xml` from the produced zip and
//! asserts the text — including Chinese — survives verbatim.

use super::super::{Align, Artifact, Block, Document, ExportError, Table};
use docx_rs::{
    AlignmentType, Docx, Paragraph, Run, Table as DxTable, TableCell, TableRow, WidthType,
};
use std::io::Cursor;

fn run(text: &str) -> Run {
    Run::new().add_text(text)
}

fn heading_para(level: u8, text: &str) -> Paragraph {
    // Word default body is 22 half-points (11pt). Scale headings down by level.
    let size = match level.clamp(1, 6) {
        1 => 36,
        2 => 32,
        3 => 28,
        4 => 26,
        5 => 24,
        _ => 22,
    };
    Paragraph::new().add_run(Run::new().add_text(text).bold().size(size))
}

fn dx_align(a: Align) -> AlignmentType {
    match a {
        Align::Left => AlignmentType::Left,
        Align::Center => AlignmentType::Center,
        Align::Right => AlignmentType::Right,
    }
}

/// Build a Word table from the IR table. Header row cells are bold.
fn build_table(t: &Table) -> DxTable {
    let mut rows: Vec<TableRow> = Vec::with_capacity(t.rows.len() + 1);

    // header
    let header_cells: Vec<TableCell> = t
        .headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            TableCell::new().add_paragraph(
                Paragraph::new()
                    .add_run(Run::new().add_text(h).bold())
                    .align(dx_align(t.align_for(i))),
            )
        })
        .collect();
    rows.push(TableRow::new(header_cells));

    // data
    for row in &t.rows {
        let cells: Vec<TableCell> = row
            .iter()
            .enumerate()
            .map(|(i, c)| {
                TableCell::new().add_paragraph(
                    Paragraph::new()
                        .add_run(run(c))
                        .align(dx_align(t.align_for(i))),
                )
            })
            .collect();
        rows.push(TableRow::new(cells));
    }

    DxTable::new(rows)
        .set_grid(vec![])
        .width(9000, WidthType::Dxa)
}

fn apply_blocks(mut docx: Docx, blocks: &[Block]) -> Docx {
    for block in blocks {
        docx = match block {
            Block::Heading { level, text } => docx.add_paragraph(heading_para(*level, text)),
            Block::Paragraph { text } => docx.add_paragraph(Paragraph::new().add_run(run(text))),
            Block::List { ordered, items } => {
                for (i, item) in items.iter().enumerate() {
                    let prefix = if *ordered {
                        format!("{}. ", i + 1)
                    } else {
                        "• ".to_string()
                    };
                    docx = docx
                        .add_paragraph(Paragraph::new().add_run(run(&format!("{prefix}{item}"))));
                }
                docx
            }
            Block::Table(t) => docx.add_table(build_table(t)),
        };
    }
    docx
}

pub fn render(artifact: &Artifact) -> Result<Vec<u8>, ExportError> {
    let mut docx = Docx::new();

    match artifact {
        Artifact::Table(t) => {
            if let Some(title) = &t.title {
                docx = docx.add_paragraph(heading_para(1, title));
            }
            docx = docx.add_table(build_table(t));
        }
        Artifact::Document(d) => {
            docx = apply_document(docx, d);
        }
    }

    let mut buf = Cursor::new(Vec::new());
    docx.build()
        .pack(&mut buf)
        .map_err(|e| ExportError::RenderFailed(format!("docx pack: {e}")))?;
    Ok(buf.into_inner())
}

fn apply_document(mut docx: Docx, d: &Document) -> Docx {
    if let Some(title) = &d.title {
        docx = docx.add_paragraph(heading_para(1, title));
    }
    apply_blocks(docx, &d.blocks)
}
