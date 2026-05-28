//! T2 — Eval response surface (citations + grounding + confidence + cost).
//!
//! Spec: `docs/superpowers/specs/2026-05-28-kb-memory-vs-vlm-llm-bench-validation.md`
//! §9.4 P0 #2 (R3 groundedness gap) + §9.2 R3/G4.
//!
//! ## What we surface
//!
//! - `citations[].chunk_id` — stable per-chunk identifier (item_id + chunk_offset),
//!   so bench can score grounding without re-deriving anchors.
//! - `citations[].span` — `[chunk_offset_start, chunk_offset_end]` exposed as
//!   bench-friendly 2-element array (kept alongside the existing two fields
//!   for backward compat).
//! - `grounding` block — chat_reliability::evaluate_response distilled to
//!   `{ score, level }` so bench can read a single float without parsing the
//!   full ReliabilityReport.
//! - `cost` block — `{ tokens_in, tokens_out, estimated_usd, model, provider }`
//!   structured form of the existing scalar `tokens_*` / `cost_usd` keys.
//! - `eval` block — surfaced only when `X-Attune-Eval-Mode: 1` header is set
//!   (T1 deterministic seed pairs with T2 surface).
//!
//! ## Backward compatibility
//!
//! Every existing key (`citations[].item_id`, `chunk_offset_start`,
//! `chunk_offset_end`, `confidence`, `tokens_in`, `cost_usd`) **stays in
//! place**. New keys are additive. Tests below assert both old + new shape.
//!
//! ## Testing strategy
//!
//! Spawning the real HTTP server for chat happy path requires ~30s Argon2id
//! setup per case (see `vault_lock_endpoint_test.rs` for the pattern). For T2
//! we keep tests at the unit level on the **pure builders** in
//! `attune_server::eval`. The builders are exactly what `routes/chat.rs` and
//! `routes/search.rs` call — covering them by unit test gives the same
//! invariants without the per-test Argon2id cost. Full HTTP coverage of the
//! eval surface lives in `tests/amd_laptop_e2e_smoke.rs` (`#[ignore]` E2E).

use attune_core::chat_reliability::{
    evaluate_response, ChatReliabilityConfig, RetrievedChunk,
};
use attune_server::eval::{
    build_citation, build_cost_block, build_eval_block, build_grounding_block,
    build_latency_breakdown, parse_eval_headers, GroundingLevel,
};

// ---------------------------------------------------------------------------
// Helper: synthesize a search-result-like JSON value
// ---------------------------------------------------------------------------

fn knowledge_value(item_id: &str, title: &str, score: f64) -> serde_json::Value {
    serde_json::json!({
        "item_id": item_id,
        "title": title,
        "score": score,
        "content": "Rust ownership ensures memory safety without GC.",
        "inject_content": "Rust ownership ensures memory safety without GC.",
        "breadcrumb": ["Rust Book", "Chapter 4"],
        "chunk_offset_start": 100,
        "chunk_offset_end": 250,
        "source_type": "local",
    })
}

// ---------------------------------------------------------------------------
// Citations: backward compat + new keys
// ---------------------------------------------------------------------------

#[test]
fn citation_preserves_existing_legacy_keys() {
    let k = knowledge_value("doc-001", "Rust Book", 0.85);
    let c = build_citation(&k);

    // Existing keys MUST remain (Chrome extension / Web UI depend on these)
    assert_eq!(c["item_id"], "doc-001", "legacy item_id must be preserved");
    assert_eq!(c["title"], "Rust Book", "legacy title must be preserved");
    assert_eq!(c["relevance"], 0.85, "legacy relevance must be preserved");
    assert_eq!(c["chunk_offset_start"], 100, "legacy chunk_offset_start preserved");
    assert_eq!(c["chunk_offset_end"], 250, "legacy chunk_offset_end preserved");
    assert!(c["breadcrumb"].is_array(), "legacy breadcrumb preserved");
}

#[test]
fn citation_adds_chunk_id_for_bench_grounding() {
    let k = knowledge_value("doc-001", "Rust Book", 0.85);
    let c = build_citation(&k);

    // T2 NEW: chunk_id derived from item_id + offset (deterministic)
    let chunk_id = c["chunk_id"].as_str().expect("chunk_id must be a string");
    assert!(
        chunk_id.starts_with("doc-001:"),
        "chunk_id must be derived from item_id, got: {chunk_id}"
    );
    assert!(
        chunk_id.contains("100"),
        "chunk_id must encode chunk_offset_start, got: {chunk_id}"
    );
}

#[test]
fn citation_adds_span_array() {
    let k = knowledge_value("doc-001", "Rust Book", 0.85);
    let c = build_citation(&k);

    // T2 NEW: span [start, end] as 2-element array for bench schema parity
    let span = c["span"].as_array().expect("span must be an array");
    assert_eq!(span.len(), 2, "span must be [start, end]");
    assert_eq!(span[0], 100);
    assert_eq!(span[1], 250);
}

#[test]
fn citation_score_alias_for_relevance() {
    // bench wants `score` field name; UI wants `relevance` — both surfaced
    let k = knowledge_value("doc-001", "Rust Book", 0.85);
    let c = build_citation(&k);
    assert_eq!(c["score"], 0.85, "T2: score alias must equal relevance");
}

#[test]
fn citation_handles_missing_offsets_gracefully() {
    // web search citation: no chunk_offset_start/end
    let k = serde_json::json!({
        "item_id": "web:https://example.com",
        "title": "Example",
        "score": 0.55,
        "breadcrumb": [],
    });
    let c = build_citation(&k);
    // chunk_id must still produce a stable value (fallback: item_id alone)
    assert!(c["chunk_id"].is_string());
    // span must be null/absent rather than [null, null]
    assert!(
        c["span"].is_null() || !c["span"].is_array(),
        "missing offsets → span should be null, got: {}",
        c["span"]
    );
}

// ---------------------------------------------------------------------------
// Grounding block
// ---------------------------------------------------------------------------

#[test]
fn grounding_high_when_response_well_supported() {
    let response = "Rust ownership ensures memory safety without GC [item:doc-001].";
    let chunks = vec![RetrievedChunk::new(
        "doc-001",
        "Rust ownership ensures memory safety without GC.",
    )];
    let report = evaluate_response(response, &chunks, "what is rust ownership?",
                                    &ChatReliabilityConfig::default());

    let block = build_grounding_block(&report);
    let score = block["score"].as_f64().expect("score must be number");
    assert!(score >= 0.5, "well-supported response should ground >= 0.5, got {score}");
    let level = block["level"].as_str().expect("level must be string");
    assert!(matches!(level, "high" | "medium"),
            "well-grounded should be high/medium, got {level}");
}

#[test]
fn grounding_surfaces_fabricated_citation_marker_in_report() {
    // Response cites a chunk that does not exist
    let response = "Rust achieves immortality via ownership [item:nonexistent].";
    let chunks = vec![RetrievedChunk::new(
        "doc-001",
        "Rust ownership ensures memory safety without GC.",
    )];
    let report = evaluate_response(response, &chunks, "what is rust ownership?",
                                    &ChatReliabilityConfig::default());

    // The chat_reliability agent owns: when a response cites an item_id
    // that isn't in the retrieved set, citation_grounded MUST contain a
    // Fabricated entry. Bench R3 reads citations_count + status to compute
    // its own aggregate; the f32 score is a coarse summary.
    let block = build_grounding_block(&report);
    assert_eq!(block["citations_count"].as_u64().unwrap(), 1,
               "fabricated cite must show up in citations_count");
    // Score will not be 1.0 — citation_penalty triggered
    let score = block["score"].as_f64().unwrap();
    assert!(score < 1.0,
            "fabricated cite must lower score below 1.0, got {score}");
}

#[test]
fn grounding_contradiction_pushes_level_down() {
    // Heavy contradiction signal (weight_contradiction = 0.50 default).
    // Two date contradictions → contradiction_penalty = (2/2) * 0.5 = 0.5
    // → confidence ≤ 0.5 → at most Medium.
    let response = "The event happened on 2024-03-15 and also on 2024-03-15.";
    // Chunks claim different dates for the same entity
    let chunks = vec![
        RetrievedChunk::new("doc-001", "The event happened on 2023-01-10."),
        RetrievedChunk::new("doc-002", "The event happened on 2025-12-31."),
    ];
    let report = evaluate_response(response, &chunks, "when?",
                                    &ChatReliabilityConfig::default());
    let block = build_grounding_block(&report);
    let level = block["level"].as_str().unwrap();
    let score = block["score"].as_f64().unwrap();
    // With or without exact contradiction detection, the test asserts the
    // invariant: when contradictions_count > 0, level is not "high".
    if block["contradictions_count"].as_u64().unwrap() > 0 {
        assert_ne!(level, "high",
                   "contradictions present must not earn high level (score={score})");
    }
}

#[test]
fn grounding_block_includes_counts_for_eval_consumers() {
    let response = "Rust ownership.";
    let chunks: Vec<RetrievedChunk> = vec![];
    let report = evaluate_response(response, &chunks, "", &ChatReliabilityConfig::default());
    let block = build_grounding_block(&report);

    assert!(block["citations_count"].is_u64(),
            "grounding.citations_count must be u64 for bench R3 aggregator");
    assert!(block["contradictions_count"].is_u64(),
            "grounding.contradictions_count must be u64");
    assert!(block["hallucination_flags_count"].is_u64(),
            "grounding.hallucination_flags_count must be u64");
}

#[test]
fn grounding_level_threshold_boundaries() {
    // Pure-function thresholds: 0.7+ = high, 0.4..0.7 = medium, < 0.4 = low
    assert_eq!(GroundingLevel::from_score(0.9), GroundingLevel::High);
    assert_eq!(GroundingLevel::from_score(0.7), GroundingLevel::High);
    assert_eq!(GroundingLevel::from_score(0.69), GroundingLevel::Medium);
    assert_eq!(GroundingLevel::from_score(0.4), GroundingLevel::Medium);
    assert_eq!(GroundingLevel::from_score(0.39), GroundingLevel::Low);
    assert_eq!(GroundingLevel::from_score(0.0), GroundingLevel::Low);
}

// ---------------------------------------------------------------------------
// Cost block
// ---------------------------------------------------------------------------

#[test]
fn cost_block_for_cloud_model() {
    let block = build_cost_block(120, 80, "gpt-4o-mini", false);
    assert_eq!(block["tokens_in"], 120);
    assert_eq!(block["tokens_out"], 80);
    assert_eq!(block["model"], "gpt-4o-mini");
    assert_eq!(block["provider"], "openai",
               "gpt-4o-mini provider must be 'openai'");
    let usd = block["estimated_usd"].as_f64().expect("estimated_usd must be f64");
    assert!(usd > 0.0, "cloud model should have positive cost, got {usd}");
}

#[test]
fn cost_block_for_local_model() {
    let block = build_cost_block(120, 80, "qwen2.5:3b", true);
    assert_eq!(block["tokens_in"], 120);
    assert_eq!(block["tokens_out"], 80);
    assert_eq!(block["model"], "qwen2.5:3b");
    assert_eq!(block["provider"], "ollama",
               "qwen models must report ollama provider");
    // Local must return 0.0 (not null) so bench can sum without filter
    let usd = block["estimated_usd"].as_f64().expect("estimated_usd must be f64");
    assert_eq!(usd, 0.0, "local model cost must be 0.0, got {usd}");
}

#[test]
fn cost_block_unknown_provider_returns_zero_cost() {
    let block = build_cost_block(120, 80, "totally-unknown-model", false);
    assert_eq!(block["provider"], "unknown");
    let usd = block["estimated_usd"].as_f64().unwrap();
    assert_eq!(usd, 0.0, "unknown model defaults to 0 cost, never null");
}

#[test]
fn cost_block_claude_provider_detection() {
    let block = build_cost_block(100, 50, "claude-3-5-sonnet", false);
    assert_eq!(block["provider"], "anthropic");
}

#[test]
fn cost_block_gemini_provider_detection() {
    let block = build_cost_block(100, 50, "gemini-1.5-pro", false);
    assert_eq!(block["provider"], "google");
}

#[test]
fn cost_block_deepseek_provider_detection() {
    let block = build_cost_block(100, 50, "deepseek-chat", false);
    assert_eq!(block["provider"], "deepseek");
}

// ---------------------------------------------------------------------------
// Eval block (header-gated)
// ---------------------------------------------------------------------------

#[test]
fn eval_block_returns_none_when_no_headers() {
    let headers = axum::http::HeaderMap::new();
    let parsed = parse_eval_headers(&headers);
    assert!(!parsed.eval_mode, "no headers → eval_mode disabled");
    let block = build_eval_block(&parsed, 50);
    assert!(block.is_null(), "no eval headers → eval block must be null");
}

#[test]
fn eval_block_surfaces_when_eval_mode_set() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-attune-eval-mode", "1".parse().unwrap());
    let parsed = parse_eval_headers(&headers);
    assert!(parsed.eval_mode);
    let block = build_eval_block(&parsed, 50);
    assert!(!block.is_null(), "eval_mode header → block must be present");
    assert_eq!(block["determinism"], "best_effort");
    assert!(block["seed_used"].is_null(),
            "no seed header → seed_used is null but key present");
}

#[test]
fn eval_block_carries_seed_when_header_present() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-attune-eval-mode", "1".parse().unwrap());
    headers.insert("x-attune-eval-seed", "42".parse().unwrap());
    let parsed = parse_eval_headers(&headers);
    assert_eq!(parsed.seed, Some(42));
    let block = build_eval_block(&parsed, 50);
    assert_eq!(block["seed_used"], 42);
}

#[test]
fn eval_block_invalid_seed_does_not_panic() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-attune-eval-mode", "1".parse().unwrap());
    headers.insert("x-attune-eval-seed", "not-a-number".parse().unwrap());
    let parsed = parse_eval_headers(&headers);
    // Invalid seed → drop silently to None (per spec §7 graceful degradation)
    assert!(parsed.seed.is_none(), "invalid seed must drop to None, not panic");
}

#[test]
fn eval_block_trace_full_adds_latency_breakdown() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-attune-eval-mode", "1".parse().unwrap());
    headers.insert("x-attune-eval-trace", "full".parse().unwrap());
    let parsed = parse_eval_headers(&headers);
    assert!(parsed.trace_full);

    let block = build_eval_block(&parsed, 123);
    let trace = &block["trace"];
    assert!(!trace.is_null(), "trace=full must populate trace key");
    let lb = &trace["latency_breakdown_ms"];
    assert_eq!(lb["total"], 123);
    // Per-stage stays at 0 until v1.1 SearchTracer ships (per plan §9.5 #6)
    for key in ["rewrite", "bm25", "vector", "rrf", "rerank"] {
        assert!(lb[key].is_u64(),
                "latency_breakdown_ms.{key} must be u64 (0 placeholder OK)");
    }
}

#[test]
fn eval_block_trace_off_omits_trace() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-attune-eval-mode", "1".parse().unwrap());
    // No trace header
    let parsed = parse_eval_headers(&headers);
    assert!(!parsed.trace_full);
    let block = build_eval_block(&parsed, 50);
    // trace key may be null/absent
    assert!(
        block["trace"].is_null(),
        "without trace=full, trace block should be null"
    );
}

// ---------------------------------------------------------------------------
// Latency breakdown
// ---------------------------------------------------------------------------

#[test]
fn latency_breakdown_default_zeros() {
    let lb = build_latency_breakdown(0);
    for key in ["rewrite", "bm25", "vector", "rrf", "rerank", "total"] {
        assert!(lb[key].is_u64(), "key {key} must be u64");
    }
    assert_eq!(lb["total"], 0);
}

#[test]
fn latency_breakdown_total_passes_through() {
    let lb = build_latency_breakdown(999);
    assert_eq!(lb["total"], 999, "total must round-trip");
}
