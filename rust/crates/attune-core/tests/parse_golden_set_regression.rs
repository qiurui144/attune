// K2 Parse Golden Set 回归测试 (W3 batch C, 2026-04-27)
//
// per spec docs/superpowers/specs/2026-04-27-w3-batch-c-design.md
// 来源参照：Readwise Reader 200 篇 parsing benchmark + CI 95% 阈值方法论
//
// 5 fixture markdown 文件 + manifest.yaml 描述 expected → 跑 chunker
// extract_sections_with_path 并对照断言。任一 fixture fail → CI 红。
//
// 扩到 200 篇时只需追加 fixture + manifest 条目，不必改 harness 代码。

use std::path::PathBuf;

use attune_core::chunker::{extract_sections_with_path, SectionWithPath};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Manifest {
    fixtures: Vec<FixtureSpec>,
    regression: RegressionConfig,
}

#[derive(Debug, Deserialize)]
struct FixtureSpec {
    id: String,
    file: String,
    #[allow(dead_code)]
    source: String,
    #[allow(dead_code)]
    pinned_version: String,
    #[allow(dead_code)]
    license: String,
    expected: Expected,
}

#[derive(Debug, Deserialize)]
struct Expected {
    title_contains: Vec<String>,
    min_text_chars: usize,
    must_contain_phrases: Vec<String>,
    section_count_min: usize,
    section_paths_must_include: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct RegressionConfig {
    min_pass_rate: f32,
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("parse_corpus")
}

fn load_manifest() -> Manifest {
    let path = fixtures_dir().join("manifest.yaml");
    let yaml =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read manifest {path:?}: {e}"));
    serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("parse manifest yaml: {e}"))
}

fn load_fixture_content(file: &str) -> String {
    let path = fixtures_dir().join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {path:?}: {e}"))
}

/// 验证一个 fixture：返回 Ok(()) = 通过，Err(reasons) = 失败原因列表
fn check_fixture(
    spec: &FixtureSpec,
    sections: &[SectionWithPath],
    full_text: &str,
) -> Result<(), Vec<String>> {
    let mut failures = Vec::new();

    // 1. min_text_chars
    let char_count = full_text.chars().count();
    if char_count < spec.expected.min_text_chars {
        failures.push(format!(
            "min_text_chars: expected ≥{}, got {}",
            spec.expected.min_text_chars, char_count
        ));
    }

    // 2. title_contains（用第一个 section 的 path 第一项 + 文档第一行综合判断）
    let first_line = full_text.lines().next().unwrap_or("");
    let first_section_title = sections
        .first()
        .and_then(|s| s.path.first())
        .cloned()
        .unwrap_or_default();
    let title_text = format!("{first_line} {first_section_title}");
    for needed in &spec.expected.title_contains {
        if !title_text.contains(needed) {
            failures.push(format!(
                "title_contains: '{needed}' missing from first line / first section"
            ));
        }
    }

    // 3. must_contain_phrases（全文搜索）
    for phrase in &spec.expected.must_contain_phrases {
        if !full_text.contains(phrase) {
            failures.push(format!("must_contain_phrases: '{phrase}' missing"));
        }
    }

    // 4. section_count_min
    if sections.len() < spec.expected.section_count_min {
        failures.push(format!(
            "section_count_min: expected ≥{}, got {}",
            spec.expected.section_count_min,
            sections.len()
        ));
    }

    // 5. section_paths_must_include（每个 expected path 必须有匹配的 section）
    for expected_path in &spec.expected.section_paths_must_include {
        let found = sections.iter().any(|s| s.path == *expected_path);
        if !found {
            failures.push(format!(
                "section_paths_must_include: path {:?} missing",
                expected_path
            ));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}

#[test]
fn k2_baseline_corpus_passes_min_rate() {
    let manifest = load_manifest();
    let mut total = 0usize;
    let mut passed = 0usize;
    let mut all_failures: Vec<(String, Vec<String>)> = Vec::new();

    for spec in &manifest.fixtures {
        total += 1;
        let content = load_fixture_content(&spec.file);
        let sections = extract_sections_with_path(&content);
        match check_fixture(spec, &sections, &content) {
            Ok(()) => passed += 1,
            Err(failures) => all_failures.push((spec.id.clone(), failures)),
        }
    }

    let pass_rate = passed as f32 / total.max(1) as f32;
    println!(
        "K2 Parse Golden Set: {passed}/{total} passed (rate {:.2}%, threshold {:.2}%)",
        pass_rate * 100.0,
        manifest.regression.min_pass_rate * 100.0
    );
    if !all_failures.is_empty() {
        for (id, fs) in &all_failures {
            eprintln!("  FAIL {id}:");
            for f in fs {
                eprintln!("    - {f}");
            }
        }
    }
    assert!(
        pass_rate >= manifest.regression.min_pass_rate,
        "K2 pass rate {:.2}% < threshold {:.2}%",
        pass_rate * 100.0,
        manifest.regression.min_pass_rate * 100.0
    );
}

#[test]
fn k2_manifest_loads() {
    let m = load_manifest();
    assert_eq!(m.fixtures.len(), 5, "baseline 应有 5 fixtures");
    assert!(
        (m.regression.min_pass_rate - 1.0).abs() < 0.001,
        "baseline 阈值锁 100%"
    );
}

#[test]
fn k2_all_fixtures_exist() {
    let m = load_manifest();
    for spec in &m.fixtures {
        let path = fixtures_dir().join(&spec.file);
        assert!(path.exists(), "fixture file missing: {path:?}");
    }
}

#[test]
fn k2_001_rust_ownership_passes() {
    run_single_fixture("001-rust-ownership");
}

#[test]
fn k2_002_china_civil_code_passes() {
    run_single_fixture("002-china-civil-code");
}

#[test]
fn k2_003_tech_blog_passes() {
    run_single_fixture("003-tech-blog");
}

#[test]
fn k2_004_news_article_passes() {
    run_single_fixture("004-news-article");
}

#[test]
fn k2_005_academic_paper_passes() {
    run_single_fixture("005-academic-paper");
}

fn run_single_fixture(id: &str) {
    let m = load_manifest();
    let spec = m
        .fixtures
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("fixture {id} not in manifest"));
    let content = load_fixture_content(&spec.file);
    let sections = extract_sections_with_path(&content);
    if let Err(failures) = check_fixture(spec, &sections, &content) {
        for f in &failures {
            eprintln!("  - {f}");
        }
        panic!("fixture {id} failed {} expectations", failures.len());
    }
}
