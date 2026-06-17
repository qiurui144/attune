//! I2 — per-kind accuracy metrics for the vision eval gate (spec 2026-06-16 §9.3).
//!
//! Each non-text kind is scored against a human-labeled ground truth with a kind-appropriate metric:
//! - chart series → value-set micro-F1 (a predicted (name, values) matches GT within relative error)
//! - formula LaTeX → token edit-distance similarity
//! - table / handwriting / stamp / caption text → token micro-F1 (set overlap)
//! - grounding → grounding precision (fraction of extracted values that carry a valid GroundingRef)
//!
//! All metrics are pure 🆓 logic (no model, no IO) so they are fully unit-testable and deterministic.
//! The harness aggregates them across seeds (mean ± std) and applies the floor gate (spec §9.2).

use std::collections::HashSet;

/// Micro-F1 of two token multisets, computed on their SET intersection (order-independent).
/// `predicted` / `truth` are already tokenized. Empty truth + empty prediction → 1.0 (vacuously
/// correct); empty truth + non-empty prediction → 0.0 (all spurious).
pub fn token_f1(predicted: &[String], truth: &[String]) -> f64 {
    let p: HashSet<&String> = predicted.iter().collect();
    let t: HashSet<&String> = truth.iter().collect();
    if p.is_empty() && t.is_empty() {
        return 1.0;
    }
    if p.is_empty() || t.is_empty() {
        return 0.0;
    }
    let tp = p.intersection(&t).count() as f64;
    let precision = tp / p.len() as f64;
    let recall = tp / t.len() as f64;
    if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    }
}

/// Whitespace tokenizer used by `token_f1` for free-text kinds (caption / handwriting / stamp).
pub fn tokenize(s: &str) -> Vec<String> {
    s.split_whitespace().map(|t| t.to_string()).collect()
}

/// Relative error between a predicted and a truth numeric value. `|p - t| / max(|t|, eps)`.
/// Used to decide whether a chart series data point "matches" GT within a tolerance.
pub fn relative_error(predicted: f64, truth: f64) -> f64 {
    let denom = truth.abs().max(1e-9);
    (predicted - truth).abs() / denom
}

/// Chart series micro-F1: a predicted series (name + values) is a true positive when a GT series
/// with the same name exists AND every value matches within `tol` relative error (same arity).
/// Returns F1 over the series sets. `tol` is golden-calibrated (spec §9.3).
pub fn chart_series_f1(
    predicted: &[(String, Vec<f64>)],
    truth: &[(String, Vec<f64>)],
    tol: f64,
) -> f64 {
    if predicted.is_empty() && truth.is_empty() {
        return 1.0;
    }
    if predicted.is_empty() || truth.is_empty() {
        return 0.0;
    }
    let series_matches = |p: &(String, Vec<f64>), t: &(String, Vec<f64>)| -> bool {
        p.0 == t.0
            && p.1.len() == t.1.len()
            && p.1.iter().zip(t.1.iter()).all(|(pv, tv)| relative_error(*pv, *tv) <= tol)
    };
    let tp = predicted
        .iter()
        .filter(|p| truth.iter().any(|t| series_matches(p, t)))
        .count() as f64;
    let precision = tp / predicted.len() as f64;
    let recall = tp / truth.len() as f64;
    if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    }
}

/// Normalized LaTeX similarity: `1 - editdistance(p, t) / max(len)`. Whitespace-insensitive (LaTeX
/// is reformatted freely by VLMs). 1.0 = identical, 0.0 = completely different.
pub fn latex_similarity(predicted: &str, truth: &str) -> f64 {
    let p: Vec<char> = predicted.chars().filter(|c| !c.is_whitespace()).collect();
    let t: Vec<char> = truth.chars().filter(|c| !c.is_whitespace()).collect();
    if p.is_empty() && t.is_empty() {
        return 1.0;
    }
    let max = p.len().max(t.len());
    if max == 0 {
        return 1.0;
    }
    let dist = levenshtein(&p, &t);
    1.0 - (dist as f64 / max as f64)
}

/// Classic Levenshtein edit distance on slices (rows of length |b|+1, O(|a|·|b|) time, O(|b|) space).
fn levenshtein<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Grounding precision: fraction of `total_values` that carry a VALID grounding ref
/// (`grounded_values`). 1.0 = every extracted value is locatable; 0.0 = none are. `total_values`
/// of 0 → 1.0 (no value can be ungrounded). Spec §9.3 grounding precision metric.
pub fn grounding_precision(grounded_values: usize, total_values: usize) -> f64 {
    if total_values == 0 {
        return 1.0;
    }
    grounded_values as f64 / total_values as f64
}

/// Mean and (population) standard deviation of a multi-seed score vector (spec §9.2: report mean ± std).
/// Empty → (0.0, 0.0).
pub fn mean_std(scores: &[f64]) -> (f64, f64) {
    if scores.is_empty() {
        return (0.0, 0.0);
    }
    let n = scores.len() as f64;
    let mean = scores.iter().sum::<f64>() / n;
    let var = scores.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n;
    (mean, var.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_f1_perfect_and_empty() {
        assert_eq!(token_f1(&[], &[]), 1.0);
        let a = tokenize("the quick brown fox");
        assert_eq!(token_f1(&a, &a), 1.0);
    }

    #[test]
    fn token_f1_empty_prediction_vs_nonempty_truth_is_zero() {
        assert_eq!(token_f1(&[], &tokenize("foo")), 0.0);
        assert_eq!(token_f1(&tokenize("foo"), &[]), 0.0);
    }

    #[test]
    fn token_f1_partial_overlap() {
        // predicted {a,b,c}, truth {b,c,d}: tp=2, p=2/3, r=2/3, f1=2/3.
        let f1 = token_f1(&tokenize("a b c"), &tokenize("b c d"));
        assert!((f1 - 2.0 / 3.0).abs() < 1e-9, "got {f1}");
    }

    #[test]
    fn token_f1_no_overlap_is_zero() {
        assert_eq!(token_f1(&tokenize("a b"), &tokenize("c d")), 0.0);
    }

    #[test]
    fn relative_error_basic_and_zero_truth() {
        assert!((relative_error(110.0, 100.0) - 0.1).abs() < 1e-9);
        assert!(relative_error(0.0, 0.0) < 1e-6); // both zero → ~0 error
    }

    #[test]
    fn chart_series_f1_exact_match() {
        let p = vec![("Q1".to_string(), vec![1.0, 2.0])];
        assert_eq!(chart_series_f1(&p, &p, 0.05), 1.0);
    }

    #[test]
    fn chart_series_f1_within_tolerance_matches() {
        let p = vec![("Q1".to_string(), vec![102.0])]; // 2% off
        let t = vec![("Q1".to_string(), vec![100.0])];
        assert_eq!(chart_series_f1(&p, &t, 0.05), 1.0); // within 5% tol
        assert_eq!(chart_series_f1(&p, &t, 0.01), 0.0); // outside 1% tol
    }

    #[test]
    fn chart_series_f1_wrong_name_or_arity_misses() {
        let t = vec![("Q1".to_string(), vec![100.0])];
        assert_eq!(chart_series_f1(&[("Q2".to_string(), vec![100.0])], &t, 0.05), 0.0);
        assert_eq!(chart_series_f1(&[("Q1".to_string(), vec![100.0, 1.0])], &t, 0.05), 0.0);
    }

    #[test]
    fn chart_series_f1_both_empty_is_one() {
        assert_eq!(chart_series_f1(&[], &[], 0.05), 1.0);
    }

    #[test]
    fn latex_similarity_identical_and_whitespace_insensitive() {
        assert_eq!(latex_similarity("E=mc^2", "E=mc^2"), 1.0);
        assert_eq!(latex_similarity("E = mc^2", "E=mc^2"), 1.0); // whitespace ignored
    }

    #[test]
    fn latex_similarity_one_char_diff() {
        // "E=mc^2" vs "E=mc^3": 1 substitution / 6 chars → similarity 1 - 1/6.
        let s = latex_similarity("E=mc^2", "E=mc^3");
        assert!((s - (1.0 - 1.0 / 6.0)).abs() < 1e-9, "got {s}");
    }

    #[test]
    fn latex_similarity_completely_different_is_low() {
        assert!(latex_similarity("abc", "xyzw") < 0.3);
    }

    #[test]
    fn grounding_precision_basic() {
        assert_eq!(grounding_precision(0, 0), 1.0);
        assert_eq!(grounding_precision(3, 3), 1.0);
        assert_eq!(grounding_precision(1, 4), 0.25);
        assert_eq!(grounding_precision(0, 5), 0.0);
    }

    #[test]
    fn mean_std_basic() {
        let (m, s) = mean_std(&[1.0, 1.0, 1.0]);
        assert_eq!(m, 1.0);
        assert_eq!(s, 0.0);
        let (m2, s2) = mean_std(&[0.0, 1.0]);
        assert!((m2 - 0.5).abs() < 1e-9);
        assert!((s2 - 0.5).abs() < 1e-9); // population std of {0,1}
    }

    #[test]
    fn mean_std_empty() {
        assert_eq!(mean_std(&[]), (0.0, 0.0));
    }

    #[test]
    fn levenshtein_known_distances() {
        assert_eq!(levenshtein(&tokenize("kitten"), &tokenize("kitten")), 0);
        let a: Vec<char> = "kitten".chars().collect();
        let b: Vec<char> = "sitting".chars().collect();
        assert_eq!(levenshtein(&a, &b), 3);
    }
}
