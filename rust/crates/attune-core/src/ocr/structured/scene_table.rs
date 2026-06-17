//! `table_v1` — 通用表格还原 (cells 2D + 表头检测).
//!
//! Spec §4.2: headers / rows / row_count / column_count.
//!
//! 算法:
//!   1. y 聚类成"逻辑行" (y 重叠 ≥ 50% 视为同行)
//!   2. 每行内 cells 按 x 排序
//!   3. 列对齐: 所有 cell x-center 1D one-pass clustering
//!   4. headers = 第一行 (启发式: 全非数字)
//!
//! 限制: PP-OCRv5 mobile 不输出 cell 合并标记, 合并 cell 展开/留空, confidence 标低.

use super::{FieldValue, StructuredFields};
use crate::ocr::RawLine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TableFields {
    /// JSON array string: r#"["col1","col2",...]"# or empty.
    pub headers: FieldValue,
    /// JSON 2D array string: r#"[["a","b"],["c","d"]]"#.
    pub rows: FieldValue,
    /// integer as string.
    pub row_count: FieldValue,
    /// integer as string.
    pub column_count: FieldValue,
}

/// 主入口.
pub fn extract(lines: &[RawLine]) -> StructuredFields {
    if lines.is_empty() {
        return StructuredFields::TableV1 {
            fields: TableFields::default(),
            unrecognized_fields: vec!["table_structure".into()],
            validation_warnings: vec![],
        };
    }

    let logical_rows = cluster_into_rows(lines, 0.5);
    if logical_rows.is_empty() {
        return StructuredFields::TableV1 {
            fields: TableFields::default(),
            unrecognized_fields: vec!["table_structure".into()],
            validation_warnings: vec![],
        };
    }

    let column_centers = compute_column_centers(lines, &logical_rows);
    let col_count = column_centers.len();

    let cells: Vec<Vec<String>> = logical_rows
        .iter()
        .map(|row_indices| {
            let mut row = vec![String::new(); col_count];
            for &idx in row_indices {
                let line = &lines[idx];
                let x_center = line.bbox.x as f32 + line.bbox.w as f32 / 2.0;
                let col = nearest_column(&column_centers, x_center);
                if row[col].is_empty() {
                    row[col] = line.text.clone();
                } else {
                    row[col].push(' ');
                    row[col].push_str(&line.text);
                }
            }
            row
        })
        .collect();

    let row_count_actual = cells.len();
    let (headers_opt, body_rows) = detect_headers(cells);
    let confidence_overall = compute_table_confidence(lines, &logical_rows);

    let mut fields = TableFields::default();
    let mut unrecognized: Vec<&'static str> = Vec::new();

    if let Some(h) = headers_opt {
        fields.headers = FieldValue {
            value: Some(serde_json::to_string(&h).unwrap_or_default()),
            confidence: confidence_overall,
            bbox: None,
            source_line_idx: None,
        };
    } else {
        unrecognized.push("headers");
    }

    fields.rows = FieldValue {
        value: Some(serde_json::to_string(&body_rows).unwrap_or_default()),
        confidence: confidence_overall,
        bbox: None,
        source_line_idx: None,
    };
    fields.row_count = FieldValue {
        value: Some(body_rows.len().to_string()),
        confidence: confidence_overall,
        bbox: None,
        source_line_idx: None,
    };
    fields.column_count = FieldValue {
        value: Some(col_count.to_string()),
        confidence: confidence_overall,
        bbox: None,
        source_line_idx: None,
    };

    let warnings: Vec<String> = if row_count_actual == 0 || col_count == 0 {
        vec!["empty table structure".into()]
    } else {
        vec![]
    };

    StructuredFields::TableV1 {
        fields,
        unrecognized_fields: unrecognized.iter().map(|s| s.to_string()).collect(),
        validation_warnings: warnings,
    }
}

/// Build `TableFields` from pre-merged cells produced by the nontext table-structure
/// recognizer (which DOES carry rowspan/colspan markers, unlike the y/x heuristic in
/// `extract`). Callers prefer this when the nontext pass ran. Output schema is identical
/// to `extract` (StructuredFields::TableV1) so downstream consumers are unaffected.
///
/// row_count / col_count account for spans (a cell at row r spanning row_span rows reaches
/// r + row_span). Headers = the row-0 cell texts.
#[cfg(feature = "nontext")]
pub fn extract_from_cells(cells: &[crate::ocr::nontext::Cell]) -> StructuredFields {
    if cells.is_empty() {
        return StructuredFields::TableV1 {
            fields: TableFields::default(),
            unrecognized_fields: vec!["table_structure".into()],
            validation_warnings: vec![],
        };
    }
    let row_count = cells.iter().map(|c| c.row + c.row_span.max(1)).max().unwrap_or(0);
    let col_count = cells.iter().map(|c| c.col + c.col_span.max(1)).max().unwrap_or(0);
    let header_texts: Vec<String> = cells
        .iter()
        .filter(|c| c.row == 0)
        .map(|c| c.text.clone())
        .collect();
    let headers = serde_json::to_string(&header_texts).unwrap_or_default();

    let cell = |v: String| FieldValue {
        value: Some(v),
        confidence: 1.0,
        bbox: None,
        source_line_idx: None,
    };
    StructuredFields::TableV1 {
        fields: TableFields {
            headers: cell(headers),
            rows: cell(String::new()),
            row_count: cell(row_count.to_string()),
            column_count: cell(col_count.to_string()),
        },
        unrecognized_fields: vec![],
        validation_warnings: vec![],
    }
}

/// y 聚类成逻辑行.
fn cluster_into_rows(lines: &[RawLine], y_overlap_threshold: f32) -> Vec<Vec<usize>> {
    if lines.is_empty() {
        return vec![];
    }
    let mut sorted: Vec<usize> = (0..lines.len()).collect();
    sorted.sort_by_key(|&i| lines[i].bbox.y);

    let mut rows: Vec<Vec<usize>> = Vec::new();
    for &i in &sorted {
        let cur_y0 = lines[i].bbox.y as f32;
        let cur_y1 = cur_y0 + lines[i].bbox.h as f32;
        let mut placed = false;
        for row in rows.iter_mut() {
            let rep = row[0];
            let r_y0 = lines[rep].bbox.y as f32;
            let r_y1 = r_y0 + lines[rep].bbox.h as f32;
            let overlap = (cur_y1.min(r_y1) - cur_y0.max(r_y0)).max(0.0);
            let min_h = (cur_y1 - cur_y0).min(r_y1 - r_y0).max(1.0);
            if overlap / min_h >= y_overlap_threshold {
                row.push(i);
                placed = true;
                break;
            }
        }
        if !placed {
            rows.push(vec![i]);
        }
    }

    for row in rows.iter_mut() {
        row.sort_by_key(|&i| lines[i].bbox.x);
    }
    rows.sort_by_key(|row| lines[row[0]].bbox.y);
    rows
}

/// 1D one-pass clustering 列中心.
fn compute_column_centers(lines: &[RawLine], rows: &[Vec<usize>]) -> Vec<f32> {
    let mut centers: Vec<f32> = Vec::new();
    for row in rows {
        for &i in row {
            let cx = lines[i].bbox.x as f32 + lines[i].bbox.w as f32 / 2.0;
            centers.push(cx);
        }
    }
    if centers.is_empty() {
        return vec![];
    }
    centers.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let widths: Vec<f32> = lines.iter().map(|l| l.bbox.w as f32).collect();
    let mut sorted_w = widths.clone();
    sorted_w.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median_w = if sorted_w.is_empty() {
        50.0
    } else {
        sorted_w[sorted_w.len() / 2]
    };
    let merge_thr = (median_w * 0.5).max(20.0);

    let mut clusters: Vec<Vec<f32>> = Vec::new();
    for c in centers {
        match clusters.last_mut() {
            Some(last) if (c - last.last().copied().unwrap_or(c)).abs() < merge_thr => {
                last.push(c);
            }
            _ => clusters.push(vec![c]),
        }
    }
    clusters
        .iter()
        .map(|cl| cl.iter().sum::<f32>() / cl.len() as f32)
        .collect()
}

fn nearest_column(centers: &[f32], x: f32) -> usize {
    let mut best = 0usize;
    let mut best_d = f32::MAX;
    for (i, &c) in centers.iter().enumerate() {
        let d = (c - x).abs();
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best
}

/// 首行全非数字 + ≥2 行 → 视作 headers.
fn detect_headers(mut cells: Vec<Vec<String>>) -> (Option<Vec<String>>, Vec<Vec<String>>) {
    if cells.is_empty() {
        return (None, vec![]);
    }
    let first = &cells[0];
    let non_empty: Vec<&String> = first.iter().filter(|c| !c.is_empty()).collect();
    if non_empty.is_empty() {
        return (None, cells);
    }
    let all_non_numeric = non_empty.iter().all(|c| {
        let t = c.trim();
        !t.chars().all(|ch| ch.is_ascii_digit() || ch == '.' || ch == ',' || ch == '-')
    });
    if all_non_numeric && cells.len() > 1 {
        let h = cells.remove(0);
        (Some(h), cells)
    } else {
        (None, cells)
    }
}

fn compute_table_confidence(lines: &[RawLine], rows: &[Vec<usize>]) -> f32 {
    if lines.is_empty() {
        return 0.0;
    }
    let mut sum = 0.0f32;
    let mut count = 0usize;
    for row in rows {
        for &i in row {
            sum += lines[i].confidence;
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        (sum / count as f32).min(0.92)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::BBox;

    fn rl(text: &str, x: u32, y: u32, w: u32) -> RawLine {
        RawLine {
            text: text.into(),
            bbox: BBox { x, y, w, h: 30 },
            confidence: 0.95,
        }
    }

    #[test]
    fn empty_input_returns_unrecognized_table() {
        let StructuredFields::TableV1 { unrecognized_fields, .. } = extract(&[]) else {
            unreachable!()
        };
        assert!(unrecognized_fields.contains(&"table_structure".to_string()));
    }

    #[test]
    fn simple_2x2_table_parses() {
        let lines = vec![
            rl("Name", 10, 10, 60),
            rl("Age", 100, 10, 40),
            rl("Alice", 10, 60, 60),
            rl("30", 100, 60, 40),
        ];
        let StructuredFields::TableV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert_eq!(fields.column_count.value.as_deref(), Some("2"));
        assert_eq!(fields.row_count.value.as_deref(), Some("1"));
        let headers_json = fields.headers.value.as_deref().unwrap();
        let headers: Vec<String> = serde_json::from_str(headers_json).unwrap();
        assert_eq!(headers, vec!["Name".to_string(), "Age".to_string()]);
        let rows_json = fields.rows.value.as_deref().unwrap();
        let rows: Vec<Vec<String>> = serde_json::from_str(rows_json).unwrap();
        assert_eq!(rows, vec![vec!["Alice".to_string(), "30".to_string()]]);
    }

    #[test]
    fn all_numeric_first_row_not_headers() {
        let lines = vec![
            rl("100", 10, 10, 40),
            rl("200", 100, 10, 40),
            rl("300", 10, 60, 40),
            rl("400", 100, 60, 40),
        ];
        let StructuredFields::TableV1 { fields, unrecognized_fields, .. } = extract(&lines) else {
            unreachable!()
        };
        assert!(unrecognized_fields.contains(&"headers".to_string()));
        assert_eq!(fields.row_count.value.as_deref(), Some("2"));
    }

    #[test]
    fn y_clustering_handles_slight_misalignment() {
        let lines = vec![
            rl("A", 10, 10, 40),
            rl("B", 100, 12, 40),
            rl("C", 200, 8, 40),
        ];
        let rows = cluster_into_rows(&lines, 0.5);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 3);
    }

    #[test]
    fn x_ordering_within_row() {
        let lines = vec![
            rl("third", 200, 10, 40),
            rl("first", 10, 10, 40),
            rl("second", 100, 10, 40),
        ];
        let rows = cluster_into_rows(&lines, 0.5);
        assert_eq!(rows.len(), 1);
        let texts: Vec<&str> = rows[0].iter().map(|&i| lines[i].text.as_str()).collect();
        assert_eq!(texts, vec!["first", "second", "third"]);
    }

    #[test]
    fn three_column_table() {
        let lines = vec![
            rl("ID", 10, 10, 40),
            rl("Name", 80, 10, 60),
            rl("Score", 200, 10, 60),
            rl("1", 10, 60, 40),
            rl("Alice", 80, 60, 60),
            rl("95", 200, 60, 60),
            rl("2", 10, 110, 40),
            rl("Bob", 80, 110, 60),
            rl("88", 200, 110, 60),
        ];
        let StructuredFields::TableV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert_eq!(fields.column_count.value.as_deref(), Some("3"));
        assert_eq!(fields.row_count.value.as_deref(), Some("2"));
        let rows_json = fields.rows.value.as_deref().unwrap();
        let rows: Vec<Vec<String>> = serde_json::from_str(rows_json).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["1".to_string(), "Alice".into(), "95".into()]);
        assert_eq!(rows[1], vec!["2".to_string(), "Bob".into(), "88".into()]);
    }

    #[cfg(feature = "nontext")]
    #[test]
    fn extract_from_cells_2x2_counts_rows_and_cols() {
        use crate::ocr::nontext::Cell;
        let mk = |row, col, text: &str| Cell {
            row,
            col,
            row_span: 1,
            col_span: 1,
            text: text.into(),
            confidence: 1.0,
        };
        let cells = vec![
            mk(0, 0, "H1"),
            mk(0, 1, "H2"),
            mk(1, 0, "a"),
            mk(1, 1, "b"),
        ];
        let StructuredFields::TableV1 { fields, .. } = extract_from_cells(&cells) else {
            unreachable!()
        };
        assert_eq!(fields.row_count.value.as_deref(), Some("2"));
        assert_eq!(fields.column_count.value.as_deref(), Some("2"));
        let headers: Vec<String> =
            serde_json::from_str(fields.headers.value.as_deref().unwrap()).unwrap();
        assert_eq!(headers, vec!["H1".to_string(), "H2".into()]);
    }

    #[cfg(feature = "nontext")]
    #[test]
    fn extract_from_cells_respects_spans() {
        use crate::ocr::nontext::Cell;
        // A single cell at (0,0) spanning 2 rows x 3 cols → 2 rows, 3 cols.
        let cells = vec![Cell {
            row: 0,
            col: 0,
            row_span: 2,
            col_span: 3,
            text: "merged".into(),
            confidence: 1.0,
        }];
        let StructuredFields::TableV1 { fields, .. } = extract_from_cells(&cells) else {
            unreachable!()
        };
        assert_eq!(fields.row_count.value.as_deref(), Some("2"));
        assert_eq!(fields.column_count.value.as_deref(), Some("3"));
    }
}
