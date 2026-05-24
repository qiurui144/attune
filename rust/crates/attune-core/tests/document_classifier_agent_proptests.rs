//! document_classifier_agent — property tests (≥3 invariants).
//!
//! 6-class coverage: this file covers the "Property tests ≥ 3" floor.
//!
//! Invariants asserted:
//!   P1. classified_count == inputs.len()        (no doc lost / duplicated)
//!   P2. kind_summary values sum == classified.len()  (partition invariant)
//!   P3. confidence ∈ [0.0, 1.0] always (no NaN / no inf / no negative)
//!   P4. empty input → empty classified + confidence == 0.0
//!   P5. doc order in output preserves input order (file names match positionally)

use attune_core::agents::document_classifier::{run, DocumentInput};
use proptest::prelude::*;

fn arb_text() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[a-zA-Z0-9 借条买卖合同流水微信收据判决书出借人借款人本金月利率\u{4e00}-\u{9fff}]{0,200}")
        .unwrap()
}

fn arb_file() -> impl Strategy<Value = String> {
    proptest::string::string_regex("file[0-9]{1,4}\\.(pdf|txt|png)").unwrap()
}

fn arb_docs() -> impl Strategy<Value = Vec<(String, String)>> {
    proptest::collection::vec((arb_file(), arb_text()), 0..15)
}

proptest! {
    /// P1 + P2 + P3 combined per run for efficiency.
    #[test]
    fn prop_invariants_count_partition_confidence_bounds(docs in arb_docs()) {
        let inputs: Vec<DocumentInput<'_>> = docs
            .iter()
            .map(|(f, t)| DocumentInput { file: f.as_str(), text: t.as_str() })
            .collect();
        let out = run(&inputs);

        // P1: count preservation
        prop_assert_eq!(out.computation.classified.len(), docs.len());

        // P2: kind_summary partition invariant
        let sum: usize = out.computation.kind_summary.values().sum();
        prop_assert_eq!(sum, out.computation.classified.len());

        // P3: confidence bounds
        prop_assert!(out.confidence.is_finite(), "overall confidence must be finite");
        prop_assert!(out.confidence >= 0.0 && out.confidence <= 1.0,
            "overall confidence ∈ [0,1], got {}", out.confidence);
        for c in &out.computation.classified {
            prop_assert!(c.confidence.is_finite());
            prop_assert!(c.confidence >= 0.0 && c.confidence <= 1.0,
                "per-doc confidence ∈ [0,1], got {} for {}", c.confidence, c.file);
        }

        // 红线 always empty for document_classifier (per impl)
        prop_assert!(out.red_lines_violated.is_empty());
    }

    /// P5: order preservation
    #[test]
    fn prop_order_preserved(docs in arb_docs()) {
        let inputs: Vec<DocumentInput<'_>> = docs
            .iter()
            .map(|(f, t)| DocumentInput { file: f.as_str(), text: t.as_str() })
            .collect();
        let out = run(&inputs);
        prop_assert_eq!(out.computation.classified.len(), docs.len());
        for (i, c) in out.computation.classified.iter().enumerate() {
            prop_assert_eq!(&c.file, &docs[i].0, "file order at index {}", i);
        }
    }

    /// P4: empty input idempotent
    #[test]
    fn prop_empty_input_invariant(_seed in 0u32..10u32) {
        let out = run(&[]);
        prop_assert!(out.computation.classified.is_empty());
        prop_assert_eq!(out.confidence, 0.0);
        prop_assert!(out.computation.kind_summary.is_empty());
        prop_assert!(out.red_lines_violated.is_empty());
        prop_assert!(out.missing_evidence.is_empty());
        prop_assert!(out.followups.is_empty());
    }
}
