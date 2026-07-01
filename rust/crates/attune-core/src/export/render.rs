//! Format renderers. Every renderer takes the validated [`Artifact`] IR and
//! returns raw file bytes. Dispatch lives in [`render`]; each backend is a
//! submodule kept self-contained for isolated testing.

use super::{Align, Artifact, Block, Document, ExportError, ExportFormat, Table};
use crate::export::sanitize::escape_cell;

mod docx_render;
mod pdf_render;
mod xlsx_render;

/// Render `artifact` to `format`, returning the file bytes. Assumes the artifact
/// has already passed [`Artifact::validate`].
pub fn render(artifact: &Artifact, format: ExportFormat) -> Result<Vec<u8>, ExportError> {
    match format {
        ExportFormat::Md => Ok(render_md(artifact).into_bytes()),
        ExportFormat::Csv => render_csv(artifact),
        ExportFormat::Xlsx => xlsx_render::render(artifact),
        ExportFormat::Docx => docx_render::render(artifact),
        ExportFormat::Pdf => pdf_render::render(artifact),
    }
}

// ─────────────────────────────── Markdown ───────────────────────────────────

/// Render an artifact to GitHub-flavoured Markdown (internal, dependency-free).
pub fn render_md(artifact: &Artifact) -> String {
    match artifact {
        Artifact::Table(t) => md_table(t),
        Artifact::Document(d) => md_document(d),
    }
}

fn md_escape(s: &str) -> String {
    // Escape pipe and backslash so a cell value can't break the table grid.
    s.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\n', "<br>")
}

fn md_align_marker(a: Align) -> &'static str {
    match a {
        Align::Left => ":---",
        Align::Center => ":---:",
        Align::Right => "---:",
    }
}

fn md_table(t: &Table) -> String {
    let mut out = String::new();
    if let Some(title) = &t.title {
        out.push_str("### ");
        out.push_str(title);
        out.push_str("\n\n");
    }
    out.push_str("| ");
    out.push_str(
        &t.headers
            .iter()
            .map(|h| md_escape(h))
            .collect::<Vec<_>>()
            .join(" | "),
    );
    out.push_str(" |\n| ");
    out.push_str(
        &(0..t.ncols())
            .map(|i| md_align_marker(t.align_for(i)).to_string())
            .collect::<Vec<_>>()
            .join(" | "),
    );
    out.push_str(" |\n");
    for row in &t.rows {
        out.push_str("| ");
        out.push_str(
            &row.iter()
                .map(|c| md_escape(c))
                .collect::<Vec<_>>()
                .join(" | "),
        );
        out.push_str(" |\n");
    }
    out
}

fn md_document(d: &Document) -> String {
    let mut out = String::new();
    if let Some(title) = &d.title {
        out.push_str("# ");
        out.push_str(title);
        out.push_str("\n\n");
    }
    for block in &d.blocks {
        match block {
            Block::Heading { level, text } => {
                let lvl = (*level).clamp(1, 6) as usize;
                out.push_str(&"#".repeat(lvl));
                out.push(' ');
                out.push_str(text);
                out.push_str("\n\n");
            }
            Block::Paragraph { text } => {
                out.push_str(text);
                out.push_str("\n\n");
            }
            Block::List { ordered, items } => {
                for (i, item) in items.iter().enumerate() {
                    if *ordered {
                        out.push_str(&format!("{}. {}\n", i + 1, item));
                    } else {
                        out.push_str(&format!("- {item}\n"));
                    }
                }
                out.push('\n');
            }
            Block::Table(t) => {
                out.push_str(&md_table(t));
                out.push('\n');
            }
        }
    }
    // Trim the trailing blank line for a clean file.
    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');
    out
}

// ───────────────────────────────── CSV ──────────────────────────────────────

/// Render a table (or a single-table document) to RFC-4180 CSV with formula-
/// injection escaping on every cell.
pub fn render_csv(artifact: &Artifact) -> Result<Vec<u8>, ExportError> {
    let table = csv_table(artifact)?;
    let mut wtr = csv::WriterBuilder::new().from_writer(Vec::new());
    // header — escape_cell returns Cow<str>; collect to owned strings (&str: AsRef<[u8]>)
    let header: Vec<String> = table
        .headers
        .iter()
        .map(|h| escape_cell(h).into_owned())
        .collect();
    wtr.write_record(&header)
        .map_err(|e| ExportError::RenderFailed(format!("csv header: {e}")))?;
    for row in &table.rows {
        let rec: Vec<String> = row.iter().map(|c| escape_cell(c).into_owned()).collect();
        wtr.write_record(&rec)
            .map_err(|e| ExportError::RenderFailed(format!("csv row: {e}")))?;
    }
    wtr.flush()
        .map_err(|e| ExportError::RenderFailed(format!("csv flush: {e}")))?;
    wtr.into_inner()
        .map_err(|e| ExportError::RenderFailed(format!("csv finish: {e}")))
}

/// Resolve the single table a tabular format (csv) should use.
/// A `Document` is acceptable only if it contains exactly one table.
pub(crate) fn csv_table(artifact: &Artifact) -> Result<&Table, ExportError> {
    match artifact {
        Artifact::Table(t) => Ok(t),
        Artifact::Document(d) => {
            let tables = d.tables();
            match tables.len() {
                1 => Ok(tables[0]),
                0 => Err(ExportError::UnsupportedArtifact(
                    "csv requires a table; document has none".into(),
                )),
                n => Err(ExportError::UnsupportedArtifact(format!(
                    "csv requires a single table; document has {n} (use xlsx/docx/pdf)"
                ))),
            }
        }
    }
}
