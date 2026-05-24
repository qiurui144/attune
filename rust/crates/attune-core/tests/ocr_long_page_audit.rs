//! 一次性 audit test — OCR 超长页 silent fail 反证
//!
//! 2026-05-24 全栈模型可靠性 audit 新发现 bug:
//! PP-OCRv5 mobile 对 height>8000px image silent fail (返 0 chars 不报错)
//!
//! Verify: 1632×21050px image 切成 4 段 ~5500px each → 各 OCR → 应有真实文字
//!
//! 跑法:
//!   /data/company/project/attune/tmp/full-stack-audit-2026-05-24/ocr_long_page_bug_repro.sh
//!   cargo test -p attune-core --release --test ocr_long_page_audit -- --ignored --nocapture

use std::path::Path;

#[test]
#[ignore]
fn ocr_long_page_tiles_vs_full() {
    let provider = match attune_core::ocr::ppocr::PpOcrProvider::new() {
        Some(p) => p,
        None => {
            eprintln!("skip: PP-OCR models not present");
            return;
        }
    };

    use attune_core::ocr::OcrProvider;

    let tile_dir = "/data/company/project/attune/tmp/full-stack-audit-2026-05-24/ocr_long_page";
    let original = "/tmp/ppocr_audit/p4-5-04.png";

    // 1. full image OCR (reproduce bug)
    eprintln!("\n=== Test A: full image 1632×21050px ===");
    let full_path = Path::new(original);
    if full_path.exists() {
        let t = std::time::Instant::now();
        match provider.extract_text_from_image(full_path) {
            Ok(text) => {
                eprintln!("full OCR: {} chars in {:.1}s", text.len(), t.elapsed().as_secs_f64());
                if text.is_empty() {
                    eprintln!("  ★★★ CONFIRMED BUG: silent 0 chars on long page ★★★");
                } else {
                    eprintln!("  preview: {}", text.chars().take(200).collect::<String>());
                }
            }
            Err(e) => eprintln!("full OCR ERROR: {e}"),
        }
    } else {
        eprintln!("full image missing, skip A");
    }

    // 2. each tile (reproduce fix proposal)
    eprintln!("\n=== Test B: 4 tiles ~5500px each ===");
    let out_dir = "/data/company/project/attune/tmp/full-stack-audit-2026-05-24/ocr_long_page";
    let mut total_tile_chars = 0;
    for i in 1..=4 {
        let path = format!("{}/tile-{}.png", tile_dir, i);
        let p = Path::new(&path);
        if !p.exists() {
            eprintln!("  tile {} missing", i);
            continue;
        }
        let t = std::time::Instant::now();
        match provider.extract_text_from_image(p) {
            Ok(text) => {
                let chars = text.chars().count();
                total_tile_chars += chars;
                eprintln!("  tile {}: {} chars in {:.1}s", i, chars, t.elapsed().as_secs_f64());
                // save raw OCR text per tile for manual CER comparison
                let out = format!("{}/tile-{}.txt", out_dir, i);
                std::fs::write(&out, &text).ok();
            }
            Err(e) => eprintln!("  tile {} ERROR: {e}", i),
        }
    }
    eprintln!("\n=== SUMMARY ===");
    eprintln!("total chars from 4 tiles: {}", total_tile_chars);
    eprintln!("vs original full image: 0 (bug)");
    eprintln!("→ if total_tile_chars > 0, fix proposal validated: auto-tile by height threshold");
}
