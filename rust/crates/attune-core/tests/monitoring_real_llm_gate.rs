//! 信息监控闭环 — REAL-LLM verification gate (DeepSeek-class, #[ignore]).
//!
//! Per Agent 验证铁律 (attune/CLAUDE.md §9.2): mock-only is a false sense of security. This
//! exercises the two LLM-driven monitoring paths against a REAL OpenAI-compatible model, N≥3
//! seeds, with hard floors:
//!   - **digest LLM summary**: keypoint-recall ≥ 0.80 (seeded watch terms surviving the summary)
//!     AND grounding-precision = 1.00 (every keypoint carries an in-range [n] source ref — the
//!     §4.5.B validator guarantee; an ungrounded/hallucinated-ref response is rejected, never shown).
//!   - **deep-research synthesis grounding**: every [n] ref in the report is in-range (no fabricated
//!     source), AND keypoint-recall ≥ 0.75 over the seeded materials.
//!
//! ## How to run (§4.5-D matrix, provider-agnostic via env)
//!
//! Tier 2 — weak cloud (deepseek-v4-flash):
//! ```bash
//! set -a; source /tmp/secrets-deepseek/key.env; set +a
//! export ATTUNE_LLM_PROVIDER=openai_compat
//! export ATTUNE_LLM_ENDPOINT="$DEEPSEEK_BASE_URL"   # https://api.deepseek.com/v1
//! export ATTUNE_LLM_API_KEY="$DEEPSEEK_API_KEY"     # NEVER echoed / logged / committed
//! export ATTUNE_LLM_MODEL=deepseek-chat ATTUNE_REAL_LLM_SEEDS=3
//! cargo test -p attune-core --test monitoring_real_llm_gate -- --ignored --nocapture --test-threads=1
//! ```
//! Tier 1 default (no env) = local Ollama qwen2.5:3b. The harness NEVER prints the api key.
//! Raw per-seed numbers go to stdout → reports/runs/<ts>_monitoring-<tier>/.

#![allow(clippy::print_stdout)]

use attune_core::llm::{LlmProvider, OllamaLlmProvider, OpenAiLlmProvider};
use attune_core::monitoring::deep_research::{DeepResearch, ResearchDoc, ResearchOpts, SourceKind};
use attune_core::monitoring::digest::{DigestBuilder, MapContentSource};
use attune_core::monitoring::WatchHit;

const DEFAULT_OLLAMA_MODEL: &str = "qwen2.5:3b";

fn require_llm() -> Box<dyn LlmProvider> {
    let kind = std::env::var("ATTUNE_LLM_PROVIDER").unwrap_or_else(|_| "ollama".into());
    match kind.as_str() {
        "openai_compat" | "openai" => {
            let endpoint = std::env::var("ATTUNE_LLM_ENDPOINT").expect("ATTUNE_LLM_ENDPOINT required");
            let api_key = std::env::var("ATTUNE_LLM_API_KEY").expect("ATTUNE_LLM_API_KEY required");
            let model = std::env::var("ATTUNE_LLM_MODEL").expect("ATTUNE_LLM_MODEL required");
            let host = endpoint.split("//").nth(1).unwrap_or(&endpoint);
            println!("[provider] openai_compat host={host} model={model}");
            Box::new(OpenAiLlmProvider::new(&endpoint, &api_key, &model))
        }
        _ => {
            let model = std::env::var("ATTUNE_LLM_MODEL").unwrap_or_else(|_| DEFAULT_OLLAMA_MODEL.into());
            let p = OllamaLlmProvider::with_model(&model);
            assert!(p.is_available(), "Ollama not reachable on :11434");
            println!("[provider] ollama model={model}");
            Box::new(p)
        }
    }
}

fn model_name() -> String {
    std::env::var("ATTUNE_LLM_MODEL").unwrap_or_else(|_| DEFAULT_OLLAMA_MODEL.into())
}
fn seeds() -> usize {
    std::env::var("ATTUNE_REAL_LLM_SEEDS").ok().and_then(|s| s.parse().ok()).filter(|n| *n >= 1).unwrap_or(3)
}
fn mean_std(xs: &[f64]) -> (f64, f64) {
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    let std = (xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt();
    (mean, std)
}

fn hit(item_id: &str, title: &str, score: f32) -> WatchHit {
    WatchHit {
        watch_id: "w".into(),
        item_id: item_id.into(),
        title: title.into(),
        score,
        reasons: vec![],
        dedup_group: None,
        created_at: "2026-06-18T00:00:00Z".into(),
    }
}

// ── Gate A: digest LLM summary — keypoint-recall + grounding ────────────────

#[test]
#[ignore = "requires real LLM — openai_compat (DeepSeek) via env, or Ollama"]
fn digest_summary_real_llm() {
    let llm = require_llm();
    let n_seeds = seeds();
    println!("\n=== GATE A: digest LLM summary — {} ({n_seeds} seeds) ===", model_name());

    // Watch "RISC-V toolchain": 4 items each anchored on a distinct seeded term.
    let hits = vec![
        hit("i1", "RVV 1.0 ratified", 0.9),
        hit("i2", "GCC autovectorization gains", 0.8),
        hit("i3", "LLVM RISC-V backend update", 0.7),
        hit("i4", "RVA23 profile finalized", 0.6),
    ];
    let cm = MapContentSource(
        [
            ("i1", "The RVV 1.0 vector extension specification was ratified this week."),
            ("i2", "GCC 14 brings major autovectorization improvements for RISC-V."),
            ("i3", "The LLVM RISC-V backend gained scalable vector support."),
            ("i4", "The RVA23 application profile was finalized by RVI."),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect(),
    );
    // seeded key terms that a faithful summary should preserve.
    let seeded = ["RVV", "GCC", "LLVM", "RVA23"];
    let builder = DigestBuilder::default();

    let mut recall_per_seed = Vec::new();
    let mut all_grounded = true;
    for seed in 0..n_seeds {
        let card = builder.build_llm_summary(
            "w", "RISC-V toolchain", &hits, &cm, &std::collections::HashMap::new(),
            "2026-06-19T00:00:00Z", llm.as_ref(),
        );
        let summary = card.llm_summary.clone().unwrap_or_default();
        // grounding: a non-empty summary means the validator passed (every keypoint had an
        // in-range ref); an empty summary = fell back to extractive = ungrounded LLM output.
        let grounded = !summary.is_empty();
        all_grounded &= grounded;
        let hits_terms = seeded.iter().filter(|t| summary.contains(**t)).count();
        let recall = hits_terms as f64 / seeded.len() as f64;
        recall_per_seed.push(recall);
        println!("  seed {seed}: grounded={grounded} keypoint-recall={recall:.2}");
    }
    let (mean, std) = mean_std(&recall_per_seed);
    println!("  keypoint-recall mean={mean:.3} std={std:.3}");
    assert!(all_grounded, "every seed must yield a grounded summary (validator passed)");
    assert!(mean >= 0.80, "digest keypoint-recall floor 0.80, got {mean:.3} (label tier in RELEASE, do NOT relax)");
}

// ── Gate B: deep-research synthesis — grounded refs + recall ────────────────

#[test]
#[ignore = "requires real LLM — openai_compat (DeepSeek) via env, or Ollama"]
fn deep_research_synthesis_real_llm() {
    let llm = require_llm();
    let n_seeds = seeds();
    println!("\n=== GATE B: deep-research synthesis — {} ({n_seeds} seeds) ===", model_name());

    let docs = vec![
        ResearchDoc { kind: SourceKind::Vault, reference: "item-1".into(), title: "RVV ratified".into(),
            snippet: "The RVV 1.0 vector extension was ratified.".into() },
        ResearchDoc { kind: SourceKind::Web, reference: "https://lwn.net/x".into(), title: "Toolchain support".into(),
            snippet: "GCC and LLVM now support RVV codegen.".into() },
        ResearchDoc { kind: SourceKind::Vault, reference: "item-2".into(), title: "RVA23 profile".into(),
            snippet: "RVA23 mandates the vector extension.".into() },
    ];
    let n_docs = docs.len();
    let seeded = ["RVV", "RVA23"];

    let mut recall_per_seed = Vec::new();
    let mut all_refs_in_range = true;
    for seed in 0..n_seeds {
        let report = DeepResearch.run("RISC-V vector ecosystem", &docs, &ResearchOpts::default(), Some(llm.as_ref()));
        let md = &report.report_markdown;
        // grounding-precision: every [n] in the report must be in 1..=n_docs (no fabricated source).
        let in_range = refs_in_range(md, n_docs);
        all_refs_in_range &= in_range;
        let recall = seeded.iter().filter(|t| md.contains(**t)).count() as f64 / seeded.len() as f64;
        recall_per_seed.push(recall);
        println!("  seed {seed}: refs_in_range={in_range} recall={recall:.2}");
    }
    let (mean, std) = mean_std(&recall_per_seed);
    println!("  synthesis recall mean={mean:.3} std={std:.3}");
    assert!(all_refs_in_range, "no fabricated source refs (grounding-precision = 1.00)");
    assert!(mean >= 0.75, "deep-research recall floor 0.75, got {mean:.3} (label tier in RELEASE, do NOT relax)");
}

// ── Gate C: cross-source verification — recall + grounding + conflict detect ─

#[test]
#[ignore = "requires real LLM — openai_compat (DeepSeek) via env, or Ollama"]
fn cross_source_verification_real_llm() {
    use attune_core::monitoring::deep_research::Verification;

    let llm = require_llm();
    let n_seeds = seeds();
    println!("\n=== GATE C: cross-source verification — {} ({n_seeds} seeds) ===", model_name());

    // Two distinct sources state the SAME fact in DIFFERENT words (must cluster → confirmed),
    // plus one independent single-source fact (must stay single). This is exactly what the
    // deterministic exact-title path CANNOT do, so it isolates the LLM semantic step.
    let docs = vec![
        ResearchDoc { kind: SourceKind::Vault, reference: "item-1".into(),
            title: "RVV ratified".into(),
            snippet: "The RVV 1.0 vector extension was ratified this week.".into() },
        ResearchDoc { kind: SourceKind::Web, reference: "https://lwn.net/x".into(),
            title: "RISC-V finalizes vectors".into(),
            snippet: "RISC-V International has finalized the 1.0 vector extension specification.".into() },
        ResearchDoc { kind: SourceKind::Vault, reference: "item-2".into(),
            title: "GCC 14 autovec".into(),
            snippet: "GCC 14 adds RISC-V autovectorization, unrelated to ratification.".into() },
    ];
    let n_docs = docs.len();
    let opts = ResearchOpts::default();

    // recall = (found a confirmed multi-source claim) ; grounding = every claim's sources are real.
    let real_refs: std::collections::HashSet<&str> =
        docs.iter().map(|d| d.reference.as_str()).collect();

    let mut recall_per_seed = Vec::new();
    let mut all_grounded = true;
    let mut conflict_false_positive = false;
    for seed in 0..n_seeds {
        let report = DeepResearch.run("RISC-V vector ratification", &docs, &opts, Some(llm.as_ref()));
        // grounding: every source reference in every claim must be a real doc reference.
        let grounded = report.claims.iter().all(|c| {
            !c.sources.is_empty() && c.sources.iter().all(|s| real_refs.contains(s.reference.as_str()))
        });
        all_grounded &= grounded;
        // recall: did it confirm the cross-worded ratification fact as multi-source?
        let confirmed_multi = report.claims.iter().any(|c| {
            c.verification == Verification::MultiSourceConfirmed && c.sources.len() >= 2
        });
        // these sources genuinely agree → a 'conflicting' verdict here is a false positive.
        if report.claims.iter().any(|c| c.verification == Verification::Conflicting) {
            conflict_false_positive = true;
        }
        let recall = if confirmed_multi { 1.0 } else { 0.0 };
        recall_per_seed.push(recall);
        // grounding-floor on doc-index range is structural (validator), so refs are always in-range;
        // n_docs printed for context.
        println!("  seed {seed}: grounded={grounded} confirmed_multi={confirmed_multi} (n_docs={n_docs})");
    }
    let (mean, std) = mean_std(&recall_per_seed);
    println!("  cross-source confirm-recall mean={mean:.3} std={std:.3}");
    assert!(all_grounded, "every claim's sources must trace to a real doc (grounding-precision = 1.00)");
    assert!(!conflict_false_positive, "agreeing sources must not be labeled conflicting (false-positive guard)");
    assert!(mean >= 0.80, "cross-source confirm-recall floor 0.80, got {mean:.3} (label tier in RELEASE, do NOT relax)");
}

/// Extract every `[n]` integer ref from markdown; true if all are in 1..=max.
fn refs_in_range(md: &str, max: usize) -> bool {
    let mut ok = true;
    let bytes = md.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let mut j = i + 1;
            let mut num = String::new();
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                num.push(bytes[j] as char);
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b']' && !num.is_empty() {
                if let Ok(n) = num.parse::<usize>() {
                    if n < 1 || n > max {
                        ok = false;
                    }
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    ok
}
