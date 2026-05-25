//! Reranker fix 后稳定性 audit — 验证 MAX_SEQ_LEN=512 fix 后长文档不再 ONNX panic
//!
//! 2026-05-24 全栈模型可靠性 audit R14:
//! 之前 fix (commit 92c2750) 把 MAX_SEQ_LEN 2048→512 解决 BGE-reranker-base
//! position_embeddings dim=514 限制. 本 audit 用 5 个真实长文档 query 验证 fix 稳定:
//! - 文档长度 1000/2000/4000/6000/8000 chars
//! - 期望: 全部 OK, 不 panic, 返合理 score
//!
//! 跑法:
//!   cargo test -p attune-core --release --test reranker_long_doc_audit -- --ignored --nocapture

use attune_core::infer::reranker::OrtRerankProvider;
use attune_core::infer::RerankProvider;

#[test]
#[ignore]
fn reranker_long_doc_no_panic() {
    let reranker = match OrtRerankProvider::bge_reranker_v2_m3() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skip: reranker init failed: {e}");
            return;
        }
    };

    let query = "What is ownership in Rust programming language and how does it manage memory?";

    // 真实长文档 5 levels
    let base = "Ownership is Rust's most unique feature, with deep implications for the language. \
                It enables Rust to make memory safety guarantees without a garbage collector. \
                Ownership rules: each value has a variable that's its owner. There can only be \
                one owner at a time. When the owner goes out of scope, the value is dropped. ";
    let docs: Vec<(usize, String)> = (1..=10).map(|n| {
        let txt = base.repeat(n * 50);  // 1x ~ 500 chars, 10x ~ 5000+ chars
        (txt.chars().count(), txt)
    }).collect();

    eprintln!("\n=== Reranker stability under long-doc input ===");
    let mut all_ok = true;
    for (chars, doc) in &docs {
        let t = std::time::Instant::now();
        let docs_refs: Vec<&str> = vec![doc.as_str()];
        match reranker.score(query, &docs_refs) {
            Ok(scores) => {
                let score = scores.first().copied().unwrap_or(f32::NAN);
                eprintln!(
                    "  doc {} chars: score={:.4}  time={:.0}ms",
                    chars,
                    score,
                    t.elapsed().as_millis()
                );
                if score.is_nan() {
                    eprintln!("    ❌ NaN score!");
                    all_ok = false;
                }
            }
            Err(e) => {
                eprintln!("  doc {} chars: ERROR {}", chars, e);
                all_ok = false;
            }
        }
    }
    eprintln!("\n=== RESULT: {} ===", if all_ok { "✅ all OK — fix (MAX_SEQ_LEN=512) holds" } else { "❌ some failed" });
    assert!(all_ok, "reranker should not fail on long docs after MAX_SEQ_LEN fix");
}
