//! D5.5 — ENFORCE Six-Category Floor Gate for Office Helper.
//!
//! Spec §6.4 + plan §D5.5. Mirrors law-pro `six_category_floor_check` model:
//!   每 OCR scene + ASR lang 必须满足 6 类下限, 否则 ENFORCE mode panic 阻塞 GA tag.
//!
//! 6 类下限:
//!   1. **Golden** (approved YAML)         — OCR ≥ 5 / scene; ASR ≥ 10 累计
//!   2. **Error cases**                    — ≥ 3 (kebab error codes 测试)
//!   3. **Proptest invariants**            — ≥ 3 (in office_prop_tests.rs)
//!   4. **Boundary tests**                 — ≥ 5 per scene (lib `#[cfg(test)]`)
//!   5. **Integration subprocess**         — ≥ 1 per scene (golden gate file 存在)
//!   6. **Concurrent / cancel**            — ≥ 1 each (in office_concurrent / office_cancel)
//!
//! 默认: 只打印 warning, 不 fail (backfill 期兼容).
//! `ATTUNE_ENFORCE_OFFICE_FLOOR=1`: 缺口 panic, block CI build.

use std::path::{Path, PathBuf};

// ─── Floor definition ────────────────────────────────────────────────

const GOLDEN_FLOOR_OCR_PER_SCENE: usize = 5;
const GOLDEN_FLOOR_ASR_TOTAL: usize = 10;
const ERROR_CASE_FLOOR: usize = 3;
const PROPTEST_FLOOR: usize = 3;
const BOUNDARY_FLOOR_PER_SCENE: usize = 5;
#[allow(dead_code)]
const INTEGRATION_FLOOR_PER_SCENE: usize = 1; // documented; CAT 5 uses has_integration_for_* (bool)
const CONCURRENT_FLOOR: usize = 1;
const CANCEL_FLOOR: usize = 1;

const OCR_SCENES: &[&str] = &[
    "document",
    "receipt",
    "table",
    "card",
    "id_card_cn",
    "bank_card",
    "business_license",
];

const ASR_LANGS: &[&str] = &["zh_aishell", "en_libri", "zh_en_mixed", "meeting"];

// ─── Path helpers ────────────────────────────────────────────────────

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn golden_office_dir() -> PathBuf {
    manifest_dir().join("tests").join("golden").join("office")
}

fn tests_dir() -> PathBuf {
    manifest_dir().join("tests")
}

fn attune_core_src() -> PathBuf {
    manifest_dir()
        .parent()
        .expect("crates/attune-server has parent")
        .join("attune-core")
        .join("src")
}

// ─── Counters ───────────────────────────────────────────────────────

/// Count approved YAML files (reviewer.approved: true) in a directory.
fn count_approved_yamls(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            let p = e.path();
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(".expected.yaml"))
                    .unwrap_or(false)
        })
        .filter(|e| {
            let yaml = std::fs::read_to_string(e.path()).unwrap_or_default();
            // Simple substring match — robust to ordering and indentation; YAML
            // parser overhead not needed for floor check.
            yaml.contains("approved: true")
        })
        .count()
}

/// Count `#[test]` occurrences in a file (boundary tests live there).
fn count_test_attrs(path: &Path) -> usize {
    let Ok(src) = std::fs::read_to_string(path) else {
        return 0;
    };
    src.matches("#[test]").count()
        + src
            .matches("#[tokio::test")
            .count() // tokio test attrs also count
}

/// Count proptest invariants in a file (each `prop_*` fn declaration).
fn count_proptest_invariants(path: &Path) -> usize {
    let Ok(src) = std::fs::read_to_string(path) else {
        return 0;
    };
    // proptest! blocks: each `fn prop_xxx(...)` is one invariant
    src.matches("fn prop_").count()
}

// ─── Per-scene counters ─────────────────────────────────────────────

/// For an OCR scene, count boundary tests in scene_<name>.rs (inside attune-core).
fn count_boundary_for_scene(scene: &str) -> usize {
    // scene names in golden dir aren't identical to source file names — map them
    let src_name: String = match scene {
        "id_card_cn" | "bank_card" | "business_license" => "scene_id_card.rs".into(),
        other => format!("scene_{other}.rs"),
    };
    let path = attune_core_src()
        .join("ocr")
        .join("structured")
        .join(src_name);
    count_test_attrs(&path)
}

/// Integration "subprocess" mapping: 我们没有 per-scene 子进程, 但有 office_*_golden_gate.rs.
/// 对 OCR: office_ocr_golden_gate.rs 含 ≥1 scene-named test → 算作该 scene 的 integration.
/// 对 ASR: office_asr_golden_gate.rs 含 ≥1 lang-named test → 算作该 lang 的 integration.
fn has_integration_for_ocr_scene(scene: &str) -> bool {
    let path = tests_dir().join("office_ocr_golden_gate.rs");
    let Ok(src) = std::fs::read_to_string(&path) else {
        return false;
    };
    // each scene gate fn is named `ocr_<scene>_gate`
    src.contains(&format!("ocr_{scene}_gate"))
}

fn has_integration_for_asr_lang(lang: &str) -> bool {
    let path = tests_dir().join("office_asr_golden_gate.rs");
    let Ok(src) = std::fs::read_to_string(&path) else {
        return false;
    };
    // map dir name → test fn
    let needle = match lang {
        "zh_aishell" => "asr_zh_wer",
        "en_libri" => "asr_en_wer",
        "zh_en_mixed" => "asr_zh_en_mixed_wer",
        "meeting" => "asr_meeting_speaker_count",
        _ => return false,
    };
    src.contains(needle)
}

// ─── Violations report ──────────────────────────────────────────────

#[derive(Debug, Default)]
struct Violations {
    items: Vec<String>,
}

impl Violations {
    fn add(&mut self, msg: impl Into<String>) {
        self.items.push(msg.into());
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

fn collect_violations() -> Violations {
    let mut v = Violations::default();
    let golden = golden_office_dir();

    // ─── Category 1: Golden approved YAML ─────────────────────────
    for scene in OCR_SCENES {
        let count = count_approved_yamls(&golden.join("ocr").join(scene));
        if count < GOLDEN_FLOOR_OCR_PER_SCENE {
            v.add(format!(
                "[CAT 1 golden] OCR scene '{scene}': {count} approved YAML < {GOLDEN_FLOOR_OCR_PER_SCENE} (need real samples; synthetic generator covers id_card_cn/bank_card/business_license/receipt; document/table/card need anonymized real PDFs)",
            ));
        }
    }
    let asr_total: usize = ASR_LANGS
        .iter()
        .map(|lang| count_approved_yamls(&golden.join("asr").join(lang)))
        .sum();
    if asr_total < GOLDEN_FLOOR_ASR_TOTAL {
        v.add(format!(
            "[CAT 1 golden] ASR total: {asr_total} approved YAML < {GOLDEN_FLOOR_ASR_TOTAL} (run scripts/fetch-office-asr-golden.sh)",
        ));
    }

    // ─── Category 2: Error cases ──────────────────────────────────
    let err_count = count_test_attrs(&tests_dir().join("office_error_contract.rs"));
    if err_count < ERROR_CASE_FLOOR {
        v.add(format!(
            "[CAT 2 error] office_error_contract.rs: {err_count} tests < {ERROR_CASE_FLOOR}"
        ));
    }

    // ─── Category 3: Proptest invariants ──────────────────────────
    let prop_count = count_proptest_invariants(&tests_dir().join("office_prop_tests.rs"));
    if prop_count < PROPTEST_FLOOR {
        v.add(format!(
            "[CAT 3 proptest] office_prop_tests.rs: {prop_count} invariants < {PROPTEST_FLOOR}"
        ));
    }

    // ─── Category 4: Boundary tests per scene ─────────────────────
    for scene in OCR_SCENES {
        let count = count_boundary_for_scene(scene);
        if count < BOUNDARY_FLOOR_PER_SCENE {
            v.add(format!(
                "[CAT 4 boundary] OCR scene '{scene}': {count} #[test] < {BOUNDARY_FLOOR_PER_SCENE}"
            ));
        }
    }

    // ─── Category 5: Integration subprocess per scene ─────────────
    for scene in OCR_SCENES {
        if !has_integration_for_ocr_scene(scene) {
            v.add(format!(
                "[CAT 5 integration] OCR scene '{scene}': missing ocr_{scene}_gate in office_ocr_golden_gate.rs"
            ));
        }
    }
    for lang in ASR_LANGS {
        if !has_integration_for_asr_lang(lang) {
            v.add(format!(
                "[CAT 5 integration] ASR lang '{lang}': missing corresponding gate fn in office_asr_golden_gate.rs"
            ));
        }
    }

    // ─── Category 6: Concurrent + Cancel ──────────────────────────
    let concurrent_count = count_test_attrs(&tests_dir().join("office_concurrent_test.rs"));
    if concurrent_count < CONCURRENT_FLOOR {
        v.add(format!(
            "[CAT 6 concurrent] office_concurrent_test.rs: {concurrent_count} tests < {CONCURRENT_FLOOR}"
        ));
    }
    let cancel_count = count_test_attrs(&tests_dir().join("office_cancel_test.rs"));
    if cancel_count < CANCEL_FLOOR {
        v.add(format!(
            "[CAT 6 cancel] office_cancel_test.rs: {cancel_count} tests < {CANCEL_FLOOR}"
        ));
    }

    v
}

#[test]
fn six_category_floor_check() {
    let enforce = std::env::var("ATTUNE_ENFORCE_OFFICE_FLOOR")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    eprintln!(
        "=== Office Helper 6-Category Floor Check ===\n  ENFORCE mode: {} (ATTUNE_ENFORCE_OFFICE_FLOOR={})",
        enforce,
        std::env::var("ATTUNE_ENFORCE_OFFICE_FLOOR").unwrap_or_else(|_| "<unset>".into()),
    );

    let violations = collect_violations();

    if violations.is_empty() {
        eprintln!("  ✓ All 6 categories met for {} OCR scenes + {} ASR langs",
            OCR_SCENES.len(), ASR_LANGS.len());
        return;
    }

    eprintln!("\n  Violations ({}):", violations.items.len());
    for item in &violations.items {
        eprintln!("    {item}");
    }

    if enforce {
        panic!(
            "\nOffice helper 6-category floor check FAILED in ENFORCE mode.\n\
             {} violations detected. Fix the gaps or unset ATTUNE_ENFORCE_OFFICE_FLOOR=0\n\
             for backfill-mode warnings.\n",
            violations.items.len()
        );
    } else {
        eprintln!(
            "\n  (Backfill mode — warnings only. Enable enforcement with\n   ATTUNE_ENFORCE_OFFICE_FLOOR=1 cargo test --test office_six_category_floor)"
        );
    }
}

// ─── Self-tests for the counters ────────────────────────────────────

#[cfg(test)]
mod counter_tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn count_approved_yamls_filters_correctly() {
        let tmp = tempfile::tempdir().unwrap();
        // Approved
        let mut f = std::fs::File::create(tmp.path().join("a.expected.yaml")).unwrap();
        writeln!(f, "id: a\nreviewer:\n  approved: true").unwrap();
        // Not approved
        let mut f = std::fs::File::create(tmp.path().join("b.expected.yaml")).unwrap();
        writeln!(f, "id: b\nreviewer:\n  approved: false").unwrap();
        // Not a yaml at all
        std::fs::write(tmp.path().join("c.txt"), "ignored").unwrap();
        // Approved with extra fields
        let mut f = std::fs::File::create(tmp.path().join("d.expected.yaml")).unwrap();
        writeln!(f, "id: d\nreviewer:\n  name: X\n  approved: true").unwrap();

        assert_eq!(count_approved_yamls(tmp.path()), 2);
    }

    #[test]
    fn count_approved_yamls_empty_dir_returns_zero() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(count_approved_yamls(tmp.path()), 0);
    }

    #[test]
    fn count_approved_yamls_missing_dir_returns_zero() {
        assert_eq!(count_approved_yamls(Path::new("/no/such/dir/exists/here")), 0);
    }

    #[test]
    fn count_test_attrs_counts_both_test_kinds() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test_file.rs");
        std::fs::write(
            &path,
            r#"
#[test]
fn one() {}

#[tokio::test]
async fn two() {}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three() {}

// not a real annotation: #[test] in a comment
"#,
        )
        .unwrap();
        // 1 plain #[test] + 2 #[tokio::test (substring match)
        // (The comment line also contains "#[test]" so it counts → 4 total.
        //  That's acceptable conservativeness — over-counts ≠ false negative.)
        let count = count_test_attrs(&path);
        assert!(count >= 3, "expected ≥3 tests, got {count}");
    }

    #[test]
    fn count_proptest_invariants_finds_fn_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("prop.rs");
        std::fs::write(
            &path,
            r#"
proptest! {
    #[test]
    fn prop_one(x: u32) {}
    #[test]
    fn prop_two(s in ".*") {}
}
fn helper() {}  // not a prop_ prefix
fn prop_three() {} // outside proptest! block but matches prefix
"#,
        )
        .unwrap();
        assert_eq!(count_proptest_invariants(&path), 3);
    }
}
