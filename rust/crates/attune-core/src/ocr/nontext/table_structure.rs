//! R1 table structure — SLANet-style ONNX adapter. The model emits an HTML table
//! string with rowspan/colspan; we parse it into `Cell` grid (the merge markers
//! that scene_table.rs's y/x heuristic cannot produce — spec §1.1).

use super::{Cell, CostTier, RegionCtx, RegionKind, RegionRecognizer, RegionResult};
use crate::error::{Result, VaultError};
use image::{DynamicImage, GenericImage};

const SLANET_INPUT: usize = 488;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// PaddleOCR PP-Structure SLANet Chinese structure dictionary.
///
/// Output tensors normally contain two extra classes (sos/eos) before these 48 entries.
/// A sibling `table_structure_dict*.txt` can override this when a different SLANet export is used.
const DEFAULT_STRUCTURE_DICT: [&str; 48] = [
    "<thead>",
    "</thead>",
    "<tbody>",
    "</tbody>",
    "<tr>",
    "</tr>",
    "<td>",
    "<td",
    ">",
    "</td>",
    " colspan=\"2\"",
    " colspan=\"3\"",
    " colspan=\"4\"",
    " colspan=\"5\"",
    " colspan=\"6\"",
    " colspan=\"7\"",
    " colspan=\"8\"",
    " colspan=\"9\"",
    " colspan=\"10\"",
    " colspan=\"11\"",
    " colspan=\"12\"",
    " colspan=\"13\"",
    " colspan=\"14\"",
    " colspan=\"15\"",
    " colspan=\"16\"",
    " colspan=\"17\"",
    " colspan=\"18\"",
    " colspan=\"19\"",
    " colspan=\"20\"",
    " rowspan=\"2\"",
    " rowspan=\"3\"",
    " rowspan=\"4\"",
    " rowspan=\"5\"",
    " rowspan=\"6\"",
    " rowspan=\"7\"",
    " rowspan=\"8\"",
    " rowspan=\"9\"",
    " rowspan=\"10\"",
    " rowspan=\"11\"",
    " rowspan=\"12\"",
    " rowspan=\"13\"",
    " rowspan=\"14\"",
    " rowspan=\"15\"",
    " rowspan=\"16\"",
    " rowspan=\"17\"",
    " rowspan=\"18\"",
    " rowspan=\"19\"",
    " rowspan=\"20\"",
];

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
    fn recognize(&self, crop: &DynamicImage, _ctx: &RegionCtx) -> Result<RegionResult> {
        if !self.model_path.exists() {
            return Ok(RegionResult::UnrecognizedV1 {
                reason: "model-missing".into(),
            });
        }
        let html = run_slanet(&self.model_path, crop)?;
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

fn run_slanet(model_path: &std::path::Path, crop: &DynamicImage) -> Result<String> {
    let input = preprocess_slanet(crop);
    let mut session = ort::session::Session::builder()
        .and_then(|mut b| b.commit_from_file(model_path))
        .map_err(|e| VaultError::ModelLoad(format!("table_structure session build: {e}")))?;
    let input_name = session
        .inputs()
        .first()
        .map(|i| i.name().to_string())
        .unwrap_or_else(|| "x".to_string());
    let input_tensor =
        ort::value::Tensor::<f32>::from_array((vec![1usize, 3, SLANET_INPUT, SLANET_INPUT], input))
            .map_err(|e| VaultError::ModelLoad(format!("table_structure input tensor: {e}")))?;
    let outputs = session
        .run(ort::inputs! { input_name.as_str() => input_tensor })
        .map_err(|e| VaultError::ModelLoad(format!("table_structure ort run: {e}")))?;

    let mut best: Option<(Vec<usize>, Vec<f32>)> = None;
    for (_name, value) in outputs.iter() {
        let (shape, flat) = value
            .try_extract_tensor::<f32>()
            .map_err(|e| VaultError::ModelLoad(format!("table_structure extract output: {e}")))?;
        let shape_usize: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
        if shape_usize.len() == 3 {
            let last = *shape_usize.last().unwrap_or(&0);
            // SLANet structure logits are [1, 501, 50] for the selected export. Keep this
            // tolerant so compatible exports with the same 48-token dictionary still work.
            if last >= DEFAULT_STRUCTURE_DICT.len() && last <= DEFAULT_STRUCTURE_DICT.len() + 8 {
                best = Some((shape_usize, flat.to_vec()));
                break;
            }
        }
    }
    let (shape, logits) = best
        .ok_or_else(|| VaultError::ModelLoad("table_structure logits output not found".into()))?;
    let dict = load_structure_dict(model_path);
    Ok(decode_structure_logits(&shape, &logits, &dict))
}

fn preprocess_slanet(img: &DynamicImage) -> Vec<f32> {
    let mut canvas = DynamicImage::new_rgb8(SLANET_INPUT as u32, SLANET_INPUT as u32).to_rgb8();
    for px in canvas.pixels_mut() {
        *px = image::Rgb([255, 255, 255]);
    }

    let (w, h) = (img.width().max(1), img.height().max(1));
    let scale = (SLANET_INPUT as f32 / w as f32).min(SLANET_INPUT as f32 / h as f32);
    let nw = ((w as f32 * scale).round() as u32).clamp(1, SLANET_INPUT as u32);
    let nh = ((h as f32 * scale).round() as u32).clamp(1, SLANET_INPUT as u32);
    let resized = img
        .resize_exact(nw, nh, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let _ = canvas.copy_from(&resized, 0, 0);

    let mut out = vec![0f32; 3 * SLANET_INPUT * SLANET_INPUT];
    let plane = SLANET_INPUT * SLANET_INPUT;
    for (x, y, px) in canvas.enumerate_pixels() {
        let idx = y as usize * SLANET_INPUT + x as usize;
        for c in 0..3 {
            let v = px[c] as f32 / 255.0;
            out[c * plane + idx] = (v - MEAN[c]) / STD[c];
        }
    }
    out
}

fn load_structure_dict(model_path: &std::path::Path) -> Vec<String> {
    let dir = model_path.parent();
    let candidates = [
        "table_structure_dict_ch.txt",
        "table_structure_dict.txt",
        "slanet_dict.txt",
    ];
    if let Some(dir) = dir {
        for name in candidates {
            let path = dir.join(name);
            if let Ok(raw) = std::fs::read_to_string(&path) {
                let tokens: Vec<String> = raw
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
                    .collect();
                if !tokens.is_empty() {
                    return tokens;
                }
            }
        }
    }
    DEFAULT_STRUCTURE_DICT
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

fn decode_structure_logits(shape: &[usize], logits: &[f32], dict: &[String]) -> String {
    if shape.len() != 3 || shape[0] == 0 || shape[1] == 0 || shape[2] == 0 {
        return String::new();
    }
    let seq = shape[1];
    let classes = shape[2];
    let class_offset = classes.saturating_sub(dict.len());
    let eos = if class_offset >= 2 {
        Some(1usize)
    } else {
        None
    };
    let mut html = String::new();
    for step in 0..seq {
        let base = step * classes;
        if base + classes > logits.len() {
            break;
        }
        let mut best_idx = 0usize;
        let mut best_val = f32::NEG_INFINITY;
        for cls in 0..classes {
            let val = logits[base + cls];
            if val > best_val {
                best_val = val;
                best_idx = cls;
            }
        }
        if Some(best_idx) == eos {
            break;
        }
        if best_idx < class_offset {
            continue;
        }
        if let Some(tok) = dict.get(best_idx - class_offset) {
            html.push_str(tok);
        }
    }
    normalize_decoded_html(&html)
}

fn normalize_decoded_html(raw: &str) -> String {
    raw.to_string()
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
                &RegionCtx {
                    ocr_lines: vec![],
                    page: 0,
                },
            )
            .unwrap();
        assert!(matches!(r, RegionResult::UnrecognizedV1 { .. }));
    }

    #[test]
    fn preprocess_slanet_has_expected_nchw_len() {
        let v = preprocess_slanet(&DynamicImage::new_rgb8(20, 10));
        assert_eq!(v.len(), 3 * SLANET_INPUT * SLANET_INPUT);
    }

    #[test]
    fn decode_structure_logits_argmaxes_with_sos_eos_offset() {
        let dict = DEFAULT_STRUCTURE_DICT
            .iter()
            .map(|s| (*s).to_string())
            .collect::<Vec<_>>();
        let classes = dict.len() + 2;
        let toks = [
            2 + 4, // <tr>
            2 + 6, // <td>
            2 + 9, // </td>
            2 + 6, // <td>
            2 + 9, // </td>
            2 + 5, // </tr>
            1,     // eos
        ];
        let mut logits = vec![0.0f32; toks.len() * classes];
        for (step, cls) in toks.iter().enumerate() {
            logits[step * classes + cls] = 1.0;
        }
        let html = decode_structure_logits(&[1, toks.len(), classes], &logits, &dict);
        let (cells, rows, cols) = parse_html_table(&html);
        assert_eq!((rows, cols), (1, 2));
        assert_eq!(cells.len(), 2);
    }

    #[test]
    fn decode_structure_logits_handles_colspan_tokens() {
        let dict = DEFAULT_STRUCTURE_DICT
            .iter()
            .map(|s| (*s).to_string())
            .collect::<Vec<_>>();
        let classes = dict.len() + 2;
        let toks = [
            2 + 4,  // <tr>
            2 + 7,  // <td
            2 + 10, // colspan="2"
            2 + 8,  // >
            2 + 9,  // </td>
            2 + 5,  // </tr>
            1,
        ];
        let mut logits = vec![0.0f32; toks.len() * classes];
        for (step, cls) in toks.iter().enumerate() {
            logits[step * classes + cls] = 1.0;
        }
        let html = decode_structure_logits(&[1, toks.len(), classes], &logits, &dict);
        let (cells, rows, cols) = parse_html_table(&html);
        assert_eq!((rows, cols), (1, 2));
        assert_eq!(cells[0].col_span, 2);
    }
}
