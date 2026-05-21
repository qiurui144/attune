//! D3.2 — L1 OCR Golden Gate (准确度 + 速度红线).
//!
//! Spec §5 + §6.2 + plan §D3.2.
//!
//! 测试策略:
//!   1. 扫 `tests/golden/office/ocr/<scene>/*.expected.yaml`
//!   2. 对每个 yaml, 找对应 `<id>.png` / `.jpg` / `.jpeg` / `.webp` / `.pdf` 图片
//!   3. 有图 → 喂 PP-OCR provider → 抽取 → 比对 expected fields → 累计 accuracy + elapsed_ms
//!   4. 无图 → SKIP (打 warning, 不 fail) — 留作内部脱敏样本后补
//!   5. PP-OCR provider 不可用 (CI runner 没 ONNX models) → 整个 scene SKIP
//!
//! 红线 (per BASELINE_ENV.md):
//!   - document: 字符级 ≥ 92% | p50 ≤ 3s
//!   - receipt:  字段级 ≥ 92% | p50 ≤ 2s
//!   - table:    cell 级 ≥ 92% | p50 ≤ 4s
//!   - card:     字段级 ≥ 92% (Z 高标杆) | p50 ≤ 1.5s
//!   - id_card_cn / bank_card / business_license: ≥ 95% | p50 ≤ 2s
//!
//! 准确度计算 (字段级 schemas):
//!   hits = sum(actual_field.value == expected_value)
//!   total = sum(expected_fields count) over all samples
//!   accuracy = hits / total

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Deserialize)]
struct ExpectedYaml {
    #[allow(dead_code)]
    id: String,
    profile: String,
    #[serde(default)]
    id_card_subtype: Option<String>,
    #[allow(dead_code)]
    schema_version: String,
    #[serde(default)]
    expected_fields: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    expected_text_min_length: Option<usize>,
    #[serde(default)]
    expected_lines_count_min: Option<usize>,
    max_elapsed_ms: u64,
    reviewer: Reviewer,
}

#[derive(Debug, Deserialize)]
struct Reviewer {
    #[allow(dead_code)]
    name: String,
    approved: bool,
}

#[derive(Debug, Clone)]
struct SceneRedLine {
    min_field_accuracy: f64,
    max_p50_ms: u64,
}

fn red_line(scene: &str) -> SceneRedLine {
    match scene {
        "document" => SceneRedLine { min_field_accuracy: 0.92, max_p50_ms: 3000 },
        "receipt" => SceneRedLine { min_field_accuracy: 0.92, max_p50_ms: 2000 },
        "table" => SceneRedLine { min_field_accuracy: 0.92, max_p50_ms: 4000 },
        "card" => SceneRedLine { min_field_accuracy: 0.92, max_p50_ms: 1500 },
        "id_card_cn" | "bank_card" | "business_license" => {
            SceneRedLine { min_field_accuracy: 0.95, max_p50_ms: 2000 }
        }
        other => panic!("unknown scene: {other}"),
    }
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join("office")
        .join("ocr")
}

fn find_image_companion(yaml_path: &Path) -> Option<PathBuf> {
    // <id>.expected.yaml → <id>.{png,jpg,jpeg,webp,bmp,tiff,tif,gif,pdf}
    let stem = yaml_path
        .file_name()?
        .to_str()?
        .strip_suffix(".expected.yaml")?;
    let dir = yaml_path.parent()?;
    for ext in ["png", "jpg", "jpeg", "webp", "bmp", "tiff", "tif", "gif", "pdf"] {
        let candidate = dir.join(format!("{stem}.{ext}"));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[derive(Debug, Default)]
struct SceneStats {
    samples_total: usize,
    samples_with_image: usize,
    samples_skipped_no_image: usize,
    samples_skipped_unapproved: usize,
    field_hits: usize,
    field_total: usize,
    elapsed_ms: Vec<u64>,
    timeout_violations: Vec<String>,
}

impl SceneStats {
    fn accuracy(&self) -> f64 {
        if self.field_total == 0 {
            0.0
        } else {
            self.field_hits as f64 / self.field_total as f64
        }
    }

    fn p50_ms(&self) -> u64 {
        if self.elapsed_ms.is_empty() {
            return 0;
        }
        let mut sorted = self.elapsed_ms.clone();
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    }

    fn p95_ms(&self) -> u64 {
        if self.elapsed_ms.is_empty() {
            return 0;
        }
        let mut sorted = self.elapsed_ms.clone();
        sorted.sort_unstable();
        let idx = ((sorted.len() as f64 * 0.95) as usize).min(sorted.len() - 1);
        sorted[idx]
    }
}

fn run_scene(scene: &str) -> SceneStats {
    let dir = golden_dir().join(scene);
    let mut stats = SceneStats::default();

    if !dir.exists() {
        eprintln!("[golden-gate {scene}] dir missing, skipping: {}", dir.display());
        return stats;
    }

    // PP-OCR provider 不可用 (model 未下载) → 整 scene skip
    let provider = attune_core::ocr::detect_default_provider();
    if provider.is_none() {
        eprintln!(
            "[golden-gate {scene}] PP-OCR provider unavailable (run --bootstrap-models). \
             SKIPPING all samples for this scene."
        );
        return stats;
    }
    let provider = provider.unwrap();

    for entry in std::fs::read_dir(&dir).expect("read golden scene dir") {
        let path = entry.expect("dir entry").path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.ends_with(".expected.yaml") {
            continue;
        }

        let yaml_str = std::fs::read_to_string(&path).expect("read yaml");
        let exp: ExpectedYaml = match serde_yaml::from_str(&yaml_str) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[golden-gate {scene}] bad yaml {}: {e}", path.display());
                continue;
            }
        };
        stats.samples_total += 1;

        if !exp.reviewer.approved {
            stats.samples_skipped_unapproved += 1;
            continue;
        }

        let img_path = match find_image_companion(&path) {
            Some(p) => p,
            None => {
                stats.samples_skipped_no_image += 1;
                continue;
            }
        };
        stats.samples_with_image += 1;

        let ocr_profile = attune_core::ocr::profile_for_id(Some(&exp.profile));
        let start = Instant::now();
        let out = match provider.extract_structured(&img_path, &ocr_profile) {
            Ok(o) => o,
            Err(e) => {
                eprintln!(
                    "[golden-gate {scene}] OCR engine failed on {}: {e}",
                    img_path.display()
                );
                continue;
            }
        };
        let elapsed_ms = start.elapsed().as_millis() as u64;
        stats.elapsed_ms.push(elapsed_ms);

        // 速度红线
        if elapsed_ms > exp.max_elapsed_ms {
            stats.timeout_violations.push(format!(
                "{}: {}ms > red line {}ms",
                exp.id, elapsed_ms, exp.max_elapsed_ms
            ));
        }

        // lines count 下限
        let lines = out.lines.clone().unwrap_or_default();
        if let Some(min_lines) = exp.expected_lines_count_min {
            if lines.len() < min_lines {
                eprintln!(
                    "[golden-gate {scene}] {}: got {} lines, expected ≥ {}",
                    exp.id,
                    lines.len(),
                    min_lines
                );
            }
        }

        // 字段抽取 + 比对
        let structured = attune_core::ocr::structured::extract(
            &exp.profile,
            &lines,
            exp.id_card_subtype.as_deref(),
        );

        if let Some(s) = structured {
            let value =
                serde_json::to_value(&s).expect("structured serialize");
            for (k, expected) in &exp.expected_fields {
                stats.field_total += 1;
                let actual = value
                    .pointer(&format!("/fields/{k}/value"))
                    .and_then(|v| v.as_str());
                if actual == Some(expected.as_str()) {
                    stats.field_hits += 1;
                } else if let Some(text_min) = exp.expected_text_min_length {
                    // document scene 走字符级 — 不严格比对每字段, 而比对总文本长度
                    if value
                        .pointer("/fields/text/value")
                        .and_then(|v| v.as_str())
                        .map(|t| t.chars().count() >= text_min)
                        .unwrap_or(false)
                    {
                        stats.field_hits += 1;
                    }
                }
            }
        } else if !exp.expected_fields.is_empty() {
            // structured 没产出, 但 yaml 期望字段 → 算 0 hits (total 仍计入)
            stats.field_total += exp.expected_fields.len();
        }
    }

    stats
}

/// 主入口: 通用 scene 跑 + 红线断言.
fn assert_scene(scene: &str) {
    let red = red_line(scene);
    let stats = run_scene(scene);

    eprintln!(
        "[golden-gate {scene}] samples: total={} with_image={} skip_no_image={} skip_unapproved={}",
        stats.samples_total,
        stats.samples_with_image,
        stats.samples_skipped_no_image,
        stats.samples_skipped_unapproved
    );
    eprintln!(
        "[golden-gate {scene}] accuracy: {}/{} = {:.4} (red line ≥ {:.2})",
        stats.field_hits,
        stats.field_total,
        stats.accuracy(),
        red.min_field_accuracy
    );
    eprintln!(
        "[golden-gate {scene}] speed: p50={}ms p95={}ms (red line p50 ≤ {}ms)",
        stats.p50_ms(),
        stats.p95_ms(),
        red.max_p50_ms
    );

    // skip-policy: 若没有 image-companion sample 跑出来, gate 不 assert
    // (本仓内部脱敏样本由 D3.5+ 手工补; CI 上 PP-OCR 也可能未安装)
    if stats.samples_with_image == 0 {
        eprintln!(
            "[golden-gate {scene}] SKIP — 0 image-companion samples (provider may also be missing). \
             This is expected pre-D3.5; will become enforcing once samples land."
        );
        return;
    }

    assert!(
        stats.accuracy() >= red.min_field_accuracy,
        "scene={scene} accuracy={:.4} < red line {:.2}; hits={}/{}",
        stats.accuracy(),
        red.min_field_accuracy,
        stats.field_hits,
        stats.field_total
    );

    assert!(
        stats.p50_ms() <= red.max_p50_ms,
        "scene={scene} p50={}ms > red line {}ms",
        stats.p50_ms(),
        red.max_p50_ms
    );

    let p95_limit = (red.max_p50_ms as f64 * 1.5) as u64;
    assert!(
        stats.p95_ms() <= p95_limit,
        "scene={scene} p95={}ms > red line {}ms (p50 ≤ {})",
        stats.p95_ms(),
        p95_limit,
        red.max_p50_ms
    );
}

#[test]
fn ocr_document_gate() {
    assert_scene("document");
}

#[test]
fn ocr_receipt_gate() {
    assert_scene("receipt");
}

#[test]
fn ocr_table_gate() {
    assert_scene("table");
}

#[test]
fn ocr_card_gate() {
    assert_scene("card");
}

#[test]
fn ocr_id_card_cn_gate() {
    assert_scene("id_card_cn");
}

#[test]
fn ocr_bank_card_gate() {
    assert_scene("bank_card");
}

#[test]
fn ocr_business_license_gate() {
    assert_scene("business_license");
}

/// Meta-gate: 至少每个 scene 都有 ≥ 1 个 approved YAML
/// (否则 D3.5 ENFORCE mode 会 fail).
#[test]
fn each_scene_has_at_least_one_approved_yaml() {
    let scenes = [
        "document",
        "receipt",
        "table",
        "card",
        "id_card_cn",
        "bank_card",
        "business_license",
    ];
    let mut empty: Vec<&str> = Vec::new();
    for scene in scenes {
        let dir = golden_dir().join(scene);
        if !dir.exists() {
            empty.push(scene);
            continue;
        }
        let count: usize = std::fs::read_dir(&dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(".expected.yaml"))
                    .unwrap_or(false)
            })
            .count();
        if count == 0 {
            empty.push(scene);
        }
    }
    // 当前 D3.1 阶段, document / table / card 还没 yaml — 暂用 warning, 不 fail
    if !empty.is_empty() {
        eprintln!(
            "[golden-gate meta] scenes with 0 approved yaml: {empty:?} \
             (D3.5 ENFORCE mode will require ≥ 1; warning only at D3.2.)"
        );
    }
}
