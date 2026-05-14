//! PP-OCR mobile 中文表格识别独立 smoke 测试 (任其坤工商银行 ICBC 流水案件)
//!
//! 2026-05-09 用户拍板触发. 之前 bank_aggregator 链路输出 0 笔交易,
//! 但无法确认是 (a) PP-OCR 中文识别失败 / (b) parse_icbc_bank_text 启发式不匹配 /
//! (c) "任其坤" 在流水里以别字段出现.
//!
//! 此测试**独立验证 PP-OCR**: 只跑前 5 页 (~ 1 min), dump 文本前 5000 字到 stderr,
//! 让人类直接审 OCR 质量.
//!
//! 跑法:
//!   ATTUNE_TEST_PPOCR_PDF=/path/to/工商银行流水清单.pdf \
//!   cargo test -p attune-core --release ppocr_icbc -- --ignored --nocapture
//!
//! 前置: PP-OCR 模型已下载 (HF_ENDPOINT=https://hf-mirror.com cargo run --bin attune-server-headless -- --bootstrap-only).

use std::path::Path;
use std::process::Command;

#[test]
#[ignore]
fn ppocr_icbc_dump_first_5_pages() {
    let pdf_path = match std::env::var("ATTUNE_TEST_PPOCR_PDF") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("skip: set ATTUNE_TEST_PPOCR_PDF env var");
            return;
        }
    };

    let pdf = Path::new(&pdf_path);
    assert!(pdf.exists(), "PDF not found: {pdf_path}");

    let provider = match attune_core::ocr::ppocr::PpOcrProvider::new() {
        Some(p) => p,
        None => panic!(
            "PP-OCR models not present. Run: HF_ENDPOINT=https://hf-mirror.com \
             cargo run --bin attune-server-headless -- --bootstrap-only"
        ),
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let prefix = tmp.path().join("page");

    // 只切前 5 页 (-f 1 -l 5), 节省 OCR 时间
    eprintln!("[smoke] pdftoppm 切前 5 页 (300 DPI)...");
    let t0 = std::time::Instant::now();
    let status = Command::new("pdftoppm")
        .args(["-r", "300", "-png", "-f", "1", "-l", "5"])
        .arg(pdf)
        .arg(&prefix)
        .status()
        .expect("pdftoppm");
    assert!(status.success(), "pdftoppm exit {:?}", status.code());
    eprintln!("[smoke] pdftoppm done in {:.1}s", t0.elapsed().as_secs_f64());

    // 收集 PNG
    let mut pages: Vec<_> = std::fs::read_dir(tmp.path())
        .expect("readdir")
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("png"))
        .collect();
    pages.sort();
    eprintln!("[smoke] PNG count: {}", pages.len());

    use attune_core::ocr::OcrProvider;

    let mut all_text = String::new();
    for (i, png) in pages.iter().enumerate() {
        let t = std::time::Instant::now();
        match provider.extract_text_from_image(png) {
            Ok(text) => {
                eprintln!(
                    "[smoke] page {} OCR: {} chars, {:.1}s",
                    i + 1,
                    text.len(),
                    t.elapsed().as_secs_f64()
                );
                all_text.push_str("=== PAGE ");
                all_text.push_str(&format!("{}", i + 1));
                all_text.push_str(" ===\n");
                all_text.push_str(text.trim());
                all_text.push_str("\n\n");
            }
            Err(e) => eprintln!("[smoke] page {} ERR: {}", i + 1, e),
        }
    }

    eprintln!("\n========== OCR DUMP (first 5 pages, total {} chars) ==========", all_text.len());
    let dump: String = all_text.chars().take(5000).collect();
    eprintln!("{dump}");
    eprintln!("========== END DUMP ==========\n");

    // 诊断断言 (不 fail 测试, 只输出诊断)
    eprintln!("=== Diagnostics ===");
    eprintln!("contains '任其坤': {}", all_text.contains("任其坤"));
    eprintln!("contains '工商银行': {}", all_text.contains("工商银行"));
    eprintln!("contains '中国工商': {}", all_text.contains("中国工商"));
    eprintln!("contains '余额': {}", all_text.contains("余额"));
    eprintln!("contains '收入': {}", all_text.contains("收入"));
    eprintln!("contains '交易': {}", all_text.contains("交易"));
    eprintln!("contains '梁素燕': {}", all_text.contains("梁素燕"));

    // ISO 日期数量
    let iso_dates = all_text.matches(char::is_numeric).count();
    eprintln!("numeric chars: {iso_dates}");

    let amount_pattern = regex::Regex::new(r"\d{1,3}(?:,\d{3})*\.\d{2}").unwrap();
    let amounts: Vec<_> = amount_pattern.find_iter(&all_text).map(|m| m.as_str()).collect();
    eprintln!("amount-like patterns: {} (sample: {:?})", amounts.len(), amounts.iter().take(5).collect::<Vec<_>>());

    let date_pattern = regex::Regex::new(r"\d{4}[-/年]\d{1,2}[-/月]\d{1,2}").unwrap();
    let dates: Vec<_> = date_pattern.find_iter(&all_text).map(|m| m.as_str()).collect();
    eprintln!("date patterns: {} (sample: {:?})", dates.len(), dates.iter().take(5).collect::<Vec<_>>());

    // 不 fail 即诊断成功 — 业务可用性靠人审 dump 决定
    eprintln!("=== Smoke OK (manual review required) ===");
}
