//! xlsx renderer (`rust_xlsxwriter`). Tables → one sheet; a multi-table document
//! → one sheet per table. Header row is bold; column alignment is honoured.
//! Every cell is formula-injection-escaped before writing.

use super::super::{Align, Artifact, ExportError, Table};
use crate::export::sanitize::escape_cell;
use rust_xlsxwriter::{Format, FormatAlign, Workbook};

/// Excel limits sheet names to 31 chars and forbids `: \ / ? * [ ]`.
fn safe_sheet_name(raw: &str, fallback_idx: usize) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']'))
        .take(31)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        format!("Sheet{}", fallback_idx + 1)
    } else {
        cleaned
    }
}

fn xlsx_align(a: Align) -> FormatAlign {
    match a {
        Align::Left => FormatAlign::Left,
        Align::Center => FormatAlign::Center,
        Align::Right => FormatAlign::Right,
    }
}

pub fn render(artifact: &Artifact) -> Result<Vec<u8>, ExportError> {
    let tables: Vec<&Table> = match artifact {
        Artifact::Table(t) => vec![t],
        Artifact::Document(d) => {
            let ts = d.tables();
            if ts.is_empty() {
                return Err(ExportError::UnsupportedArtifact(
                    "xlsx requires at least one table; document has none".into(),
                ));
            }
            ts
        }
    };

    let mut workbook = Workbook::new();
    let header_fmt = Format::new().set_bold();

    for (idx, table) in tables.iter().enumerate() {
        let sheet = workbook.add_worksheet();
        let name = safe_sheet_name(table.title.as_deref().unwrap_or(""), idx);
        sheet
            .set_name(&name)
            .map_err(|e| ExportError::RenderFailed(format!("xlsx sheet name {name:?}: {e}")))?;

        // Header row (bold).
        for (col, h) in table.headers.iter().enumerate() {
            sheet
                .write_string_with_format(0, col as u16, escape_cell(h).as_ref(), &header_fmt)
                .map_err(|e| ExportError::RenderFailed(format!("xlsx header: {e}")))?;
        }

        // Per-column alignment formats (reused across the column's data cells).
        let col_fmts: Vec<Format> = (0..table.ncols())
            .map(|i| Format::new().set_align(xlsx_align(table.align_for(i))))
            .collect();

        for (r, row) in table.rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                sheet
                    .write_string_with_format(
                        (r + 1) as u32,
                        c as u16,
                        escape_cell(cell).as_ref(),
                        &col_fmts[c],
                    )
                    .map_err(|e| ExportError::RenderFailed(format!("xlsx cell: {e}")))?;
            }
        }
    }

    workbook
        .save_to_buffer()
        .map_err(|e| ExportError::RenderFailed(format!("xlsx save: {e}")))
}
