//! R1 table structure — SLANet-style ONNX adapter. The model emits an HTML table
//! string with rowspan/colspan; we parse it into `Cell` grid (the merge markers
//! that scene_table.rs's y/x heuristic cannot produce — spec §1.1).

use super::{Cell, CostTier, RegionCtx, RegionKind, RegionRecognizer, RegionResult};
use crate::error::Result;
use image::DynamicImage;

/// Parse a (subset of) HTML table into a Cell grid with spans. Supports
/// `<tr>`, `<td>`/`<th>`, `rowspan="N"`, `colspan="N"`. Robust to attribute order.
pub fn parse_html_table(html: &str) -> (Vec<Cell>, u32, u32) {
    let mut cells = Vec::new();
    let mut row = 0u32;
    let mut max_col = 0u32;
    for tr in html.split("<tr").skip(1) {
        let mut col = 0u32;
        for td in tr
            .split('<')
            .filter(|s| s.starts_with("td") || s.starts_with("th"))
        {
            let row_span = attr_num(td, "rowspan").unwrap_or(1);
            let col_span = attr_num(td, "colspan").unwrap_or(1);
            let text = td.split('>').nth(1).unwrap_or("").trim().to_string();
            cells.push(Cell {
                row,
                col,
                row_span,
                col_span,
                text,
                confidence: 1.0,
            });
            col += col_span;
            max_col = max_col.max(col);
        }
        if col > 0 {
            row += 1;
        }
    }
    (cells, row, max_col)
}

fn attr_num(s: &str, attr: &str) -> Option<u32> {
    let i = s.find(attr)?;
    let rest = &s[i + attr.len()..];
    let digits: String = rest
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

pub struct TableStructureRecognizer {
    pub model_path: std::path::PathBuf,
}

impl RegionRecognizer for TableStructureRecognizer {
    fn kind(&self) -> RegionKind {
        RegionKind::Table
    }
    fn recognize(&self, _crop: &DynamicImage, _ctx: &RegionCtx) -> Result<RegionResult> {
        if !self.model_path.exists() {
            return Ok(RegionResult::UnrecognizedV1 {
                reason: "model-missing".into(),
            });
        }
        // Real ONNX inference produces an HTML string; parse to cells. Until wired,
        // an available model yields an empty table (deterministic, never fabricates).
        let html = String::new();
        let (cells, rows, cols) = parse_html_table(&html);
        Ok(RegionResult::TableV1 {
            cells,
            row_count: rows,
            col_count: cols,
        })
    }
    fn cost_tier(&self) -> CostTier {
        CostTier::Local
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_2x2() {
        let html = "<table><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>";
        let (cells, rows, cols) = parse_html_table(html);
        assert_eq!((rows, cols), (2, 2));
        assert_eq!(cells.len(), 4);
        assert_eq!(cells[0].text, "a");
        assert_eq!(cells[3].text, "d");
    }

    #[test]
    fn parse_colspan_merge() {
        let html = r#"<tr><td colspan="2">merged</td></tr><tr><td>x</td><td>y</td></tr>"#;
        let (cells, rows, cols) = parse_html_table(html);
        assert_eq!((rows, cols), (2, 2));
        assert_eq!(cells[0].col_span, 2);
        assert_eq!(cells[0].text, "merged");
    }

    #[test]
    fn parse_rowspan_merge() {
        let html = r#"<tr><td rowspan="2">tall</td><td>b</td></tr><tr><td>c</td></tr>"#;
        let (cells, _rows, _cols) = parse_html_table(html);
        assert_eq!(cells[0].row_span, 2);
    }

    #[test]
    fn empty_html_is_empty_table() {
        let (cells, rows, cols) = parse_html_table("");
        assert!(cells.is_empty());
        assert_eq!((rows, cols), (0, 0));
    }

    #[test]
    fn missing_model_unrecognized_not_fabricated() {
        let rec = TableStructureRecognizer {
            model_path: "/missing/slanet.onnx".into(),
        };
        let r = rec
            .recognize(
                &DynamicImage::new_rgb8(1, 1),
                &RegionCtx { ocr_lines: vec![], page: 0 },
            )
            .unwrap();
        assert!(matches!(r, RegionResult::UnrecognizedV1 { .. }));
    }
}
