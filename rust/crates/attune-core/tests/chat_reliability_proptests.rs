//! chat_reliability — property tests (≥3, per "Agent 验证铁律").
//!
//! Invariants verified across hundreds of randomized inputs. Property tests
//! catch entire classes of bugs that the golden set cannot enumerate — e.g.
//! "confidence ever leaves [0, 1]" or "re-running the agent yields a
//! different report".
//!
//! Invariants:
//!
//! 1. **Confidence bounded** — `overall_confidence ∈ [0, 1]` for any
//!    response + chunk shape (including pathological large inputs and
//!    UTF-8 boundary stress).
//! 2. **Determinism / idempotence** — calling `evaluate_response` twice on
//!    the same `(response, chunks, query, config)` returns the *identical*
//!    `ChatReliabilityReport`. Encodes the "no LLM / no clock / no RNG"
//!    cost contract.
//! 3. **Monotone in hallucination signal** — adding tokens that are not in
//!    any chunk can never *increase* confidence. Encodes the intuition
//!    that more unsupported claims → lower confidence (never higher).
//! 4. **Empty-chunks lower bound** — when `chunks = []` and the response
//!    contains zero entities, the agent finds no signal and confidence
//!    is exactly 1.0.

use attune_core::chat_reliability::{
    evaluate_response, ChatReliabilityConfig, RetrievedChunk,
};
use proptest::prelude::*;

// ── Strategy helpers ─────────────────────────────────────────────────────────

/// Arbitrary plain-text response — restricted to ASCII letters / digits /
/// spaces / a handful of CJK chars + punctuation. Avoids generating
/// content that would itself violate UTF-8 boundary contracts; the
/// boundary stress lives in dedicated `#[test]` cases.
fn arb_response() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            // ASCII word
            "[a-z]{2,8}".prop_map(|s| s + " "),
            // ASCII number-like
            "[0-9]{2,6}".prop_map(|s| s + " "),
            // Short CJK run
            Just("公司 ".to_string()),
            Just("评估 ".to_string()),
            Just("时间 ".to_string()),
            Just("内容 ".to_string()),
            // Cite marker (mostly fabricated so we exercise the negative path)
            "[a-z0-9\\-]{4,16}".prop_map(|id| format!("[item:{id}] ")),
        ],
        1..30,
    )
    .prop_map(|parts| parts.concat())
}

fn arb_chunk() -> impl Strategy<Value = RetrievedChunk> {
    (
        "[a-z0-9\\-]{4,16}",
        proptest::collection::vec(
            prop_oneof![
                "[a-z]{2,8}".prop_map(|s| s + " "),
                Just("公司 ".to_string()),
                Just("评估 ".to_string()),
                Just("时间 ".to_string()),
            ],
            1..20,
        )
        .prop_map(|parts| parts.concat()),
    )
        .prop_map(|(id, text)| RetrievedChunk::new(id, text))
}

fn arb_chunks() -> impl Strategy<Value = Vec<RetrievedChunk>> {
    proptest::collection::vec(arb_chunk(), 0..10)
}

fn cfg() -> ChatReliabilityConfig {
    ChatReliabilityConfig::default()
}

// ── Invariants ───────────────────────────────────────────────────────────────

proptest! {
    /// Invariant 1: confidence stays in [0, 1] for any input.
    #[test]
    fn prop_confidence_bounded_in_unit_interval(
        response in arb_response(),
        chunks in arb_chunks(),
    ) {
        let r = evaluate_response(&response, &chunks, "q", &cfg());
        prop_assert!(
            r.overall_confidence >= 0.0 && r.overall_confidence <= 1.0,
            "confidence out of range: {} (response_len={}, chunks={})",
            r.overall_confidence,
            response.len(),
            chunks.len(),
        );
    }

    /// Invariant 2: idempotent / deterministic — same input always
    /// produces the identical report. This encodes the zero-LLM / zero-
    /// RNG / zero-clock cost contract; if a future refactor introduces
    /// any non-determinism this test catches it.
    #[test]
    fn prop_deterministic_repeat_yields_identical_report(
        response in arb_response(),
        chunks in arb_chunks(),
    ) {
        let r1 = evaluate_response(&response, &chunks, "q", &cfg());
        let r2 = evaluate_response(&response, &chunks, "q", &cfg());
        prop_assert_eq!(r1, r2);
    }

    /// Invariant 3: monotone in unsupported claims — appending a new
    /// concrete claim ("¥99999") that's not in any chunk can *only*
    /// decrease (or leave unchanged) the overall confidence; it can
    /// never raise it. This locks in the directionality of the
    /// hallucination signal.
    #[test]
    fn prop_appending_unsupported_claim_never_raises_confidence(
        response in arb_response(),
        chunks in arb_chunks(),
    ) {
        let before = evaluate_response(&response, &chunks, "q", &cfg());
        // Inject a money token that no chunk contains (¥99999_NEVER_IN_CHUNKS).
        let injected = format!("{response} 金额 ¥99999。");
        let after = evaluate_response(&injected, &chunks, "q", &cfg());
        prop_assert!(
            after.overall_confidence <= before.overall_confidence + 1e-6,
            "appending unsupported claim raised confidence: \
             before={} after={} (response_len={})",
            before.overall_confidence,
            after.overall_confidence,
            response.len(),
        );
    }

    /// Invariant 4: when there are no signals possible (chunks empty AND
    /// response has no extractable entities AND no cite markers), the
    /// agent reports 1.0 confidence. Encodes the "innocent until proven
    /// guilty" default — absent any evidence of unreliability, the
    /// response is presumed fine.
    #[test]
    fn prop_no_signal_inputs_score_one(
        words in proptest::collection::vec("[a-z]{2,8}", 1..20),
    ) {
        // Pure-ASCII response with no cite markers, no numbers, no orgs.
        let response = words.join(" ");
        let r = evaluate_response(&response, &[], "q", &cfg());
        prop_assert!(
            (r.overall_confidence - 1.0).abs() < 1e-6,
            "no-signal input scored != 1.0: {} (response={:?})",
            r.overall_confidence,
            response,
        );
        prop_assert!(r.citation_grounded.is_empty());
        prop_assert!(r.contradictions.is_empty());
        prop_assert!(r.hallucination_flags.is_empty());
    }
}
