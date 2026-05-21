//! D5.5 — Office helper 六类下限 structural gate
//!
//! 类比 attune-pro 的 `agent_golden_gate.rs::six_category_floor_check` (Phase 2)，
//! 但术语换 office 维度：
//!
//!   ┌────────────────┬─────────────────────────────────────────────────┬─────────┐
//!   │ 类别            │ 来源                                              │ 下限     │
//!   ├────────────────┼─────────────────────────────────────────────────┼─────────┤
//!   │ Golden case    │ tests/golden/office/ocr/<scene>/*.expected.yaml │ ≥ 5/scene │
//!   │ Error case     │ tests/office_error_contract.rs `#[tokio::test]`  │ ≥ 3      │
//!   │ Prop test      │ tests/office_prop_tests.rs proptest! block       │ ≥ 3      │
//!   │ Boundary       │ scene_*.rs `#[cfg(test)] mod tests` `#[test]`   │ ≥ 5/scene │
//!   │ Integration    │ tests/office_happy_path.rs + golden gates        │ ≥ 1/scene │
//!   │ ASR            │ tests/office_asr_golden_gate.rs `#[test]`        │ ≥ 5      │
//!   └────────────────┴─────────────────────────────────────────────────┴─────────┘
//!
//! 5 OCR scene (`document` / `receipt` / `table` / `card` / `id_card`) + 1 ASR
//! 维度。`id_card` 含 3 subtype 拆分为 3 个 golden bucket 但 boundary 共享
//! `scene_id_card.rs`，按 plan §4.6 仍计为 1 scene。
//!
//! **环境变量控制**:
//!   `ATTUNE_ENFORCE_OFFICE_FLOOR=1` → 缺口 panic (CI block)
//!   未设置                            → 只打印警告, test pass (兼容 backfill 期)
//!
//! 默认 off：real sample backfill (D3.5) 还未完成；synthetic + ENGINEERING_FIXTURE
//! 算入 golden count，但 real_count 单独追踪。Sprint 完成后切到 default-on。
//!
//! per CLAUDE.md「Agent 验证铁律」§2 (6 类测试覆盖下限) + Office helper plan §4.6.

use std::path::{Path, PathBuf};

// ─── scene 定义 ──────────────────────────────────────────────────────────────

/// OCR scene + boundary 维度。
///
/// `golden_buckets`: 计 golden YAML 的目录名 (相对 `tests/golden/office/ocr/`)。
///   `id_card` 拆分为 3 个 subtype 目录。
/// `boundary_src`:    boundary `#[test]` 所在的 src/ 文件 (相对 attune-core crate root)。
/// `gate_file`:       integration 覆盖该 scene 的 gate test 文件。
struct OcrSceneMeta {
    name: &'static str,
    golden_buckets: &'static [&'static str],
    boundary_src: &'static str,
    gate_file: &'static str,
}

const OCR_SCENES: &[OcrSceneMeta] = &[
    OcrSceneMeta {
        name: "document",
        golden_buckets: &["document"],
        boundary_src: "src/ocr/structured/scene_document.rs",
        gate_file: "tests/office_ocr_golden_gate.rs",
    },
    OcrSceneMeta {
        name: "receipt",
        golden_buckets: &["receipt"],
        boundary_src: "src/ocr/structured/scene_receipt.rs",
        gate_file: "tests/office_ocr_golden_gate.rs",
    },
    OcrSceneMeta {
        name: "table",
        golden_buckets: &["table"],
        boundary_src: "src/ocr/structured/scene_table.rs",
        gate_file: "tests/office_ocr_golden_gate.rs",
    },
    OcrSceneMeta {
        name: "card",
        golden_buckets: &["card"],
        boundary_src: "src/ocr/structured/scene_card.rs",
        gate_file: "tests/office_ocr_golden_gate.rs",
    },
    OcrSceneMeta {
        // id_card 含 3 subtype: id_card_cn / bank_card / business_license
        name: "id_card",
        golden_buckets: &["id_card_cn", "bank_card", "business_license"],
        boundary_src: "src/ocr/structured/scene_id_card.rs",
        gate_file: "tests/office_ocr_golden_gate.rs",
    },
];

// ─── path helpers ────────────────────────────────────────────────────────────

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `rust/crates/attune-server/tests/golden/office/ocr/`
fn ocr_golden_root() -> PathBuf {
    manifest_dir().join("tests").join("golden").join("office").join("ocr")
}

/// `rust/crates/attune-core/`
fn attune_core_root() -> PathBuf {
    manifest_dir().parent().unwrap().join("attune-core")
}

// ─── counter primitives ──────────────────────────────────────────────────────

/// 数指定目录下 `.yaml` 文件，区分 (real_or_engineered, synthetic)。
///
/// `SYNTHETIC_*` reviewer name → synthetic bucket；其他 (含 REAL / ENGINEERING_FIXTURE)
/// → real-or-engineered bucket。Plan §4.6 允许 synthetic 计 floor 但单独追踪。
fn count_golden_in_bucket(bucket_dir: &Path) -> (usize, usize) {
    let mut real = 0;
    let mut synth = 0;
    let entries = match std::fs::read_dir(bucket_dir) {
        Ok(e) => e,
        Err(_) => return (0, 0),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|x| x != "yaml").unwrap_or(true) {
            continue;
        }
        let raw = match std::fs::read_to_string(&path) {
            Ok(r) => r,
            Err(_) => continue,
        };
        // approved=true 是 gate 入门，不 approved 直接跳过 (与 office_ocr_golden_gate 同源)
        if !raw.contains("approved: true") {
            continue;
        }
        if raw.contains("SYNTHETIC") {
            synth += 1;
        } else {
            real += 1;
        }
    }
    (real, synth)
}

/// 数文件内 `#[cfg(test)] mod tests` 块里的 `#[test]` 数 (boundary unit tests)。
///
/// 简化模型：遇到 `#[cfg(test)]` 进入测试模块，之后所有 `#[test]` 计为 boundary。
/// 文件没 `#[cfg(test)]` → 返回 0。
fn count_boundary_tests_in_src(src_path: &Path) -> usize {
    let raw = match std::fs::read_to_string(src_path) {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let mut count = 0;
    let mut in_test_mod = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#[cfg(test)]") {
            in_test_mod = true;
            continue;
        }
        if in_test_mod && trimmed == "#[test]" {
            count += 1;
        }
    }
    count
}

/// 数文件内 top-level `#[tokio::test]` / `#[test]` attribute 总数 (integration tests)。
///
/// 不区分 module nesting (office_*.rs 文件 flat 结构)。
fn count_tests_in_file(test_path: &Path) -> usize {
    let raw = match std::fs::read_to_string(test_path) {
        Ok(r) => r,
        Err(_) => return 0,
    };
    // Match: `#[test]` (exact), `#[tokio::test]` (exact), `#[tokio::test(...)]` (any args)
    raw.lines()
        .map(str::trim)
        .filter(|l| {
            *l == "#[test]"
                || *l == "#[tokio::test]"
                || (l.starts_with("#[tokio::test(") && l.ends_with(")]"))
        })
        .count()
}

/// 数 office_prop_tests.rs 内 `proptest! { #[test] ... }` 块的 `#[test]` 数。
///
/// 与 boundary 区分：boundary 在 src/ 内，prop 在 prop_tests.rs 顶层 proptest! 块。
/// 这里简化为统计文件内所有 `#[test]` (因为 prop_tests.rs 全文件就是 proptest 用)。
fn count_prop_tests_in_file(test_path: &Path) -> usize {
    count_tests_in_file(test_path)
}

// ─── 报告结构 ────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct OcrSceneReport {
    name: String,
    real_golden: usize,
    synth_golden: usize,
    boundary_count: usize,
    has_integration: bool,
}

impl OcrSceneReport {
    fn total_golden(&self) -> usize {
        self.real_golden + self.synth_golden
    }

    fn violations(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.total_golden() < 5 {
            let need = 5 - self.total_golden();
            v.push(format!(
                "[{}] golden count = {} (real={} synth={}) < 5 (need {need} more); \
                 add YAML under tests/golden/office/ocr/<bucket>/",
                self.name, self.total_golden(), self.real_golden, self.synth_golden,
            ));
        }
        if self.boundary_count < 5 {
            let need = 5 - self.boundary_count;
            v.push(format!(
                "[{}] boundary #[test] count = {} < 5 (need {need} more); \
                 add #[cfg(test)] tests to scene_*.rs",
                self.name, self.boundary_count,
            ));
        }
        if !self.has_integration {
            v.push(format!(
                "[{}] no integration gate test found; \
                 add scene gate to office_ocr_golden_gate.rs",
                self.name,
            ));
        }
        v
    }
}

#[derive(Debug)]
struct GlobalReport {
    error_count: usize,
    prop_count: usize,
    asr_count: usize,
}

impl GlobalReport {
    fn violations(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.error_count < 3 {
            let need = 3 - self.error_count;
            v.push(format!(
                "[global] error contract test count = {} < 3 (need {need} more); \
                 add #[tokio::test] to office_error_contract.rs",
                self.error_count,
            ));
        }
        if self.prop_count < 3 {
            let need = 3 - self.prop_count;
            v.push(format!(
                "[global] proptest count = {} < 3 (need {need} more); \
                 add #[test] inside proptest! {{ ... }} to office_prop_tests.rs",
                self.prop_count,
            ));
        }
        if self.asr_count < 5 {
            let need = 5 - self.asr_count;
            v.push(format!(
                "[asr] asr gate test count = {} < 5 (need {need} more); \
                 add #[tokio::test] to office_asr_golden_gate.rs",
                self.asr_count,
            ));
        }
        v
    }
}

// ─── gate 主入口 ─────────────────────────────────────────────────────────────

fn build_ocr_scene_report(meta: &OcrSceneMeta) -> OcrSceneReport {
    let root = ocr_golden_root();
    let (mut real, mut synth) = (0, 0);
    for bucket in meta.golden_buckets {
        let (r, s) = count_golden_in_bucket(&root.join(bucket));
        real += r;
        synth += s;
    }

    let boundary_count = count_boundary_tests_in_src(&attune_core_root().join(meta.boundary_src));
    let has_integration = manifest_dir().join(meta.gate_file).exists();

    OcrSceneReport {
        name: meta.name.to_string(),
        real_golden: real,
        synth_golden: synth,
        boundary_count,
        has_integration,
    }
}

fn build_global_report() -> GlobalReport {
    let tests_dir = manifest_dir().join("tests");
    GlobalReport {
        error_count: count_tests_in_file(&tests_dir.join("office_error_contract.rs")),
        prop_count: count_prop_tests_in_file(&tests_dir.join("office_prop_tests.rs")),
        asr_count: count_tests_in_file(&tests_dir.join("office_asr_golden_gate.rs")),
    }
}

#[test]
fn office_six_category_floor_check() {
    let enforce = std::env::var("ATTUNE_ENFORCE_OFFICE_FLOOR")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    println!("\n=== Office helper — 6 类测试下限 check ===");
    println!(
        "强制模式: {} (ATTUNE_ENFORCE_OFFICE_FLOOR={})",
        if enforce { "ON" } else { "OFF (warning-only)" },
        std::env::var("ATTUNE_ENFORCE_OFFICE_FLOOR").unwrap_or_else(|_| "<unset>".into()),
    );

    let mut all_violations: Vec<String> = Vec::new();

    // ─── OCR scene 维度 ──────────────────────────────────────────────────
    for meta in OCR_SCENES {
        let report = build_ocr_scene_report(meta);
        println!(
            "  [ocr/{}] golden={} (real={}+synth={}) boundary={} integration={}",
            report.name,
            report.total_golden(),
            report.real_golden,
            report.synth_golden,
            report.boundary_count,
            report.has_integration,
        );
        all_violations.extend(report.violations());
    }

    // ─── 全局维度 (error / prop / asr) ───────────────────────────────────
    let global = build_global_report();
    println!(
        "  [global] error={} prop={} asr={}",
        global.error_count, global.prop_count, global.asr_count,
    );
    all_violations.extend(global.violations());

    // ─── 总结 ────────────────────────────────────────────────────────────
    if all_violations.is_empty() {
        println!("\n✅ 全部 6 类下限达标 (5 OCR scene × 3 metric + 3 global metric = 18 check)");
        return;
    }

    println!("\n⚠️  发现 {} 项缺口:", all_violations.len());
    for v in &all_violations {
        println!("    - {v}");
    }

    if enforce {
        panic!(
            "Office helper 六类下限未达标 ({} 项缺口) — 见 CLAUDE.md「Agent 验证铁律」§2 \
             + Office plan §4.6。\n\
             ENFORCE 模式 (ATTUNE_ENFORCE_OFFICE_FLOOR=1) 下缺口必须修复才可 merge。",
            all_violations.len()
        );
    } else {
        println!(
            "\nINFO: 当前 OFF 模式 (兼容 D3.5 real-sample backfill 期). \
             设 ATTUNE_ENFORCE_OFFICE_FLOOR=1 切到强制模式。"
        );
    }
}

// ─── 单元测试 (六类 gate 内部 counter 自检) ──────────────────────────────────

#[cfg(test)]
mod inner_tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn count_golden_picks_approved_and_distinguishes_synth() {
        let tmp = TempDir::new().unwrap();
        let bucket = tmp.path();

        std::fs::write(
            bucket.join("real-1.expected.yaml"),
            "approved: true\nreviewer:\n  name: REAL_PHOTO_ANONYMIZED\n",
        )
        .unwrap();
        std::fs::write(
            bucket.join("synth-1.expected.yaml"),
            "approved: true\nreviewer:\n  name: SYNTHETIC_GENERATED\n",
        )
        .unwrap();
        std::fs::write(
            bucket.join("unapproved.expected.yaml"),
            "approved: false\nreviewer:\n  name: REAL_PHOTO\n",
        )
        .unwrap();
        // non-yaml ignored
        std::fs::write(bucket.join("README.md"), "ignore").unwrap();

        let (real, synth) = count_golden_in_bucket(bucket);
        assert_eq!(real, 1, "real_count = REAL_* with approved=true");
        assert_eq!(synth, 1, "synth_count = SYNTHETIC_* with approved=true");
    }

    #[test]
    fn count_golden_empty_dir_returns_zero() {
        let tmp = TempDir::new().unwrap();
        let (real, synth) = count_golden_in_bucket(tmp.path());
        assert_eq!((real, synth), (0, 0));
    }

    #[test]
    fn count_golden_missing_dir_returns_zero() {
        let (real, synth) = count_golden_in_bucket(Path::new("/nonexistent/path/foo/bar"));
        assert_eq!((real, synth), (0, 0));
    }

    #[test]
    fn count_boundary_tests_counts_test_attrs_in_cfg_test_mod() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("scene_foo.rs");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "pub fn extract() {{}}\n\
             #[cfg(test)]\n\
             mod tests {{\n\
                 #[test]\n\
                 fn t1() {{}}\n\
                 #[test]\n\
                 fn t2() {{}}\n\
             }}\n"
        )
        .unwrap();
        assert_eq!(count_boundary_tests_in_src(&path), 2);
    }

    #[test]
    fn count_boundary_tests_ignores_test_attr_outside_cfg_test() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("scene_bar.rs");
        let mut f = std::fs::File::create(&path).unwrap();
        // `#[test]` before `#[cfg(test)]` should NOT be counted
        writeln!(
            f,
            "#[test]\nfn outside() {{}}\n\
             #[cfg(test)]\n\
             mod tests {{\n\
                 #[test]\n\
                 fn t1() {{}}\n\
             }}\n"
        )
        .unwrap();
        assert_eq!(count_boundary_tests_in_src(&path), 1);
    }

    #[test]
    fn count_boundary_tests_missing_file_returns_zero() {
        assert_eq!(
            count_boundary_tests_in_src(Path::new("/nonexistent/x.rs")),
            0
        );
    }

    #[test]
    fn count_tests_in_file_counts_tokio_and_plain_test() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("office_x.rs");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "#[tokio::test]\nasync fn t1() {{}}\n\
             #[test]\nfn t2() {{}}\n\
             #[tokio::test(flavor = \"multi_thread\", worker_threads = 2)]\nasync fn t3() {{}}\n\
             // commented out: #[test]\n"
        )
        .unwrap();
        assert_eq!(count_tests_in_file(&path), 3);
    }

    #[test]
    fn report_no_violations_when_all_floors_met() {
        let report = OcrSceneReport {
            name: "document".into(),
            real_golden: 6,
            synth_golden: 0,
            boundary_count: 10,
            has_integration: true,
        };
        assert!(report.violations().is_empty());
    }

    #[test]
    fn report_violations_when_golden_short() {
        let report = OcrSceneReport {
            name: "document".into(),
            real_golden: 2,
            synth_golden: 1, // total=3 < 5
            boundary_count: 10,
            has_integration: true,
        };
        let v = report.violations();
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("golden count = 3"));
    }

    #[test]
    fn report_violations_when_boundary_short() {
        let report = OcrSceneReport {
            name: "table".into(),
            real_golden: 5,
            synth_golden: 0,
            boundary_count: 3, // < 5
            has_integration: true,
        };
        let v = report.violations();
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("boundary #[test] count = 3"));
    }

    #[test]
    fn global_violations_when_metrics_short() {
        let global = GlobalReport {
            error_count: 1,
            prop_count: 2,
            asr_count: 4,
        };
        let v = global.violations();
        assert_eq!(v.len(), 3);
    }

    #[test]
    fn global_no_violations_when_all_met() {
        let global = GlobalReport {
            error_count: 12,
            prop_count: 5,
            asr_count: 8,
        };
        assert!(global.violations().is_empty());
    }
}
