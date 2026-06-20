//! Document-skill REAL-LLM verification gates for `reference_generate` and `research_synthesis`
//! (用户例 1/2/4), `#[ignore]` + secret-gated. The unit/integration tests use scripted mocks;
//! these gates run the agents against a REAL LLM (deepseek v4-flash by default), N=3 seeds, and
//! assert the grounding / fact-consistency floors (Agent 验证铁律: floor only ratchets UP).
//!
//! ## How to run (secret NEVER echoed; per CLAUDE.md §1.4)
//! ```bash
//! set -a; source /tmp/secrets-deepseek/key.env; set +a   # DEEPSEEK_API_KEY (NEVER echoed)
//! export ATTUNE_LLM_PROVIDER=openai_compat
//! export ATTUNE_LLM_ENDPOINT="https://api.deepseek.com/v1"
//! export ATTUNE_LLM_API_KEY="$DEEPSEEK_API_KEY"
//! export ATTUNE_LLM_MODEL="deepseek-chat"          # deepseek-v4-flash tier
//! cargo test -p attune-core --test doc_skills_real_llm_gate -- --ignored --nocapture --test-threads=1
//! ```
//!
//! ## Floors (ratchet only UP)
//! - reference_generate: 内容保真 + grounding ≥ 0.85 (fraction of sections grounded, mean/N).
//! - research_synthesis: grounding ≥ 0.90 (fraction of body sentences grounded after judge) AND
//!   fact-consistency = 1.0 (NO produced fact contradicts / fabricates beyond the sources — checked
//!   by asserting every grounded sentence's claim re-links to a source; an ungrounded sentence is
//!   allowed only if flagged [需核实], never silently shipped as fact).

#![allow(clippy::print_stdout)]

use attune_core::llm::{LlmProvider, OllamaLlmProvider, OpenAiLlmProvider};
use attune_core::skill_runtime::{reference_generate, research_synthesis};
use attune_core::writing::{SourceMaterial, SynthesisStructure};

const DEFAULT_OLLAMA_MODEL: &str = "qwen2.5:3b";
const REF_FLOOR: f64 = 0.85;
const SYNTH_GROUND_FLOOR: f64 = 0.90;

fn require_llm() -> Box<dyn LlmProvider> {
    let kind = std::env::var("ATTUNE_LLM_PROVIDER").unwrap_or_else(|_| "ollama".into());
    match kind.as_str() {
        "openai_compat" | "openai" => {
            let endpoint = std::env::var("ATTUNE_LLM_ENDPOINT")
                .expect("ATTUNE_LLM_ENDPOINT required (e.g. https://api.deepseek.com/v1)");
            let api_key = std::env::var("ATTUNE_LLM_API_KEY").expect("ATTUNE_LLM_API_KEY required");
            let model = std::env::var("ATTUNE_LLM_MODEL").expect("ATTUNE_LLM_MODEL required");
            let host = endpoint.split("//").nth(1).unwrap_or(&endpoint);
            println!("[provider] openai_compat host={host} model={model}");
            Box::new(OpenAiLlmProvider::new(&endpoint, &api_key, &model))
        }
        _ => {
            let model =
                std::env::var("ATTUNE_LLM_MODEL").unwrap_or_else(|_| DEFAULT_OLLAMA_MODEL.into());
            let p = OllamaLlmProvider::with_model(&model);
            assert!(
                p.is_available(),
                "Ollama not reachable on :11434 — start `ollama serve` + `ollama pull {model}`. \
                 A missing model must NOT silently pass this gate (Agent 验证铁律)."
            );
            println!("[provider] ollama model={model}");
            Box::new(p)
        }
    }
}

fn model_name() -> String {
    std::env::var("ATTUNE_LLM_MODEL").unwrap_or_else(|_| DEFAULT_OLLAMA_MODEL.into())
}

fn seeds() -> usize {
    std::env::var("ATTUNE_REAL_LLM_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(3)
}

fn mean_std(xs: &[f64]) -> (f64, f64) {
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    let std = (xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt();
    (mean, std)
}

// ───────────────────────── research_synthesis corpus ─────────────────────────

/// A hand-labeled multi-source synthesis case: per-source material + facts that MUST appear
/// (content fidelity) + facts that MUST NOT appear (fabrication guard — none of these are in any
/// source, so a faithful synthesis never asserts them as grounded fact).
struct SynthCase {
    sources: &'static [(&'static str, &'static str)],
    must_contain: &'static [&'static str],
    must_not_assert: &'static [&'static str],
}

const SYNTH_CORPUS: &[SynthCase] = &[
    SynthCase {
        sources: &[
            ("algo", "靶点 EGFR 在非小细胞肺癌中过表达，与肿瘤增殖密切相关。"),
            ("tcm", "中药黄芪具有调节免疫、抗疲劳的作用。"),
        ],
        must_contain: &["EGFR", "黄芪"],
        must_not_assert: &["靶点 EGFR 可被黄芪直接抑制", "已完成三期临床"],
    },
    SynthCase {
        sources: &[
            ("arch", "新一代架构采用并行计算单元，显著提升吞吐量。"),
            ("bench", "评测体系应覆盖吞吐量、延迟、能效三个维度。"),
        ],
        must_contain: &["吞吐量", "评测"],
        must_not_assert: &["延迟降低了 90%", "能效全球第一"],
    },
    SynthCase {
        sources: &[
            ("a", "RAG 通过检索相关文档增强生成的事实性。"),
            ("b", "向量检索使用近似最近邻加速。"),
        ],
        must_contain: &["RAG", "检索"],
        must_not_assert: &["RAG 完全消除幻觉", "检索延迟为零"],
    },
];

/// Grounding score for a synthesis result: fraction of *body* sentences that are grounded
/// (verified). Heading-only segments are excluded by the agent already.
fn synth_grounding_score(r: &attune_core::writing::WritingResult) -> f64 {
    let body: Vec<&attune_core::writing::Segment> = r
        .segments
        .iter()
        .filter(|s| s.text.trim().chars().count() > 8) // body sentence, not a heading
        .collect();
    if body.is_empty() {
        return 0.0;
    }
    let grounded = body.iter().filter(|s| s.verified).count();
    grounded as f64 / body.len() as f64
}

#[test]
#[ignore = "real LLM; secret/Ollama-gated. Run via the CI secret lane or manually."]
fn research_synthesis_real_llm_grounding_floor() {
    let llm = require_llm();
    let model = model_name();
    let n_seeds = seeds();
    println!(
        "[gate] research_synthesis grounding floor={SYNTH_GROUND_FLOOR} fact-consistency=1.0 model={model} seeds={n_seeds} cases={}",
        SYNTH_CORPUS.len()
    );

    let mut seed_scores = Vec::with_capacity(n_seeds);
    for seed in 0..n_seeds {
        let mut case_scores = Vec::with_capacity(SYNTH_CORPUS.len());
        for (i, c) in SYNTH_CORPUS.iter().enumerate() {
            let sources: Vec<SourceMaterial> = c
                .sources
                .iter()
                .map(|(id, t)| SourceMaterial::new(*id, *t))
                .collect();
            let r = research_synthesis(&sources, SynthesisStructure::Thematic, 0, llm.as_ref())
                .expect("synthesis produced a result");

            // Content fidelity: every must_contain fact appears somewhere in the synthesis.
            for needle in c.must_contain {
                assert!(
                    r.content.contains(needle),
                    "[seed {seed} case {i}] synthesis dropped required fact {needle:?}"
                );
            }
            // Fact-consistency = 1.0: a fabricated fact must NOT appear as a *grounded* sentence.
            // (If the model writes one, it must be unverified/[需核实], never shipped as fact.)
            for bogus in c.must_not_assert {
                for s in &r.segments {
                    if s.verified && s.text.contains(bogus) {
                        panic!("[seed {seed} case {i}] fabricated fact shipped as grounded: {bogus:?}");
                    }
                }
            }

            let g = synth_grounding_score(&r);
            println!("[seed {seed}] case {i} grounding={g:.3} ({} segs)", r.segments.len());
            case_scores.push(g);
        }
        let (mean, _) = mean_std(&case_scores);
        println!("[seed {seed}] mean grounding={mean:.3}");
        seed_scores.push(mean);
    }
    let (mean, std) = mean_std(&seed_scores);
    println!("[gate] research_synthesis grounding mean={mean:.3} std={std:.3} over {n_seeds} seeds");
    assert!(
        mean >= SYNTH_GROUND_FLOOR,
        "research_synthesis grounding {mean:.3} < floor {SYNTH_GROUND_FLOOR} on {model}. \
         Do NOT lower the floor (Agent 验证铁律) — record honestly + label min tier in RELEASE.md."
    );
}

// ───────────────────────── reference_generate corpus ─────────────────────────

struct RefCase {
    reference: &'static str,
    source_data: &'static [(&'static str, &'static str)],
    /// facts from the source that a faithful new doc should carry.
    must_contain: &'static [&'static str],
    /// reference skeleton headings that must be reused.
    headings: &'static [&'static str],
}

const REF_CORPUS: &[RefCase] = &[
    RefCase {
        reference: "# 项目背景\n背景示例。\n\n# 设备参数\n参数示例。\n\n# 质保\n质保示例。",
        source_data: &[("dev", "设备分辨率 4K，功耗 12W，接口 USB，质保 5 年。")],
        must_contain: &["4K", "USB"],
        headings: &["项目背景", "设备参数", "质保"],
    },
    RefCase {
        reference: "# Overview\nintro.\n\n# Specifications\nspec.\n\n# Warranty\nwarranty.",
        source_data: &[("dev", "Resolution 4K, power 12W, interface USB, warranty 5 years.")],
        must_contain: &["4K", "USB"],
        headings: &["Overview", "Specifications", "Warranty"],
    },
    RefCase {
        reference: "# 概述\n略。\n\n# 技术指标\n略。",
        source_data: &[("sys", "系统支持 1000 并发，响应延迟 50ms，可用性 99.9%。")],
        must_contain: &["1000", "50ms"],
        headings: &["概述", "技术指标"],
    },
];

/// reference_generate score: fraction of sections that are grounded (not flagged unverified) AND
/// carry non-empty content — combined content-fidelity + grounding (the ref floor).
fn ref_score(doc: &attune_core::skill_runtime::ReferenceDoc) -> f64 {
    if doc.sections.is_empty() {
        return 0.0;
    }
    let n = doc.sections.len();
    let grounded = (0..n)
        .filter(|i| !doc.unverified_sections.contains(i) && !doc.sections[*i].1.trim().is_empty())
        .count();
    grounded as f64 / n as f64
}

#[test]
#[ignore = "real LLM; secret/Ollama-gated. Run via the CI secret lane or manually."]
fn reference_generate_real_llm_fidelity_floor() {
    let llm = require_llm();
    let model = model_name();
    let n_seeds = seeds();
    println!(
        "[gate] reference_generate fidelity+grounding floor={REF_FLOOR} model={model} seeds={n_seeds} cases={}",
        REF_CORPUS.len()
    );

    let mut seed_scores = Vec::with_capacity(n_seeds);
    for seed in 0..n_seeds {
        let mut case_scores = Vec::with_capacity(REF_CORPUS.len());
        for (i, c) in REF_CORPUS.iter().enumerate() {
            let sources: Vec<SourceMaterial> = c
                .source_data
                .iter()
                .map(|(id, t)| SourceMaterial::new(*id, *t))
                .collect();
            let doc = reference_generate(c.reference, &sources, "新文档", llm.as_ref())
                .expect("reference_generate produced a doc");

            // Skeleton reuse: every reference heading is present.
            for h in c.headings {
                assert!(
                    doc.sections.iter().any(|(head, _)| head.contains(h)),
                    "[seed {seed} case {i}] reference heading {h:?} not reused"
                );
            }
            // Content fidelity: required source facts appear somewhere in the generated bodies.
            let all_body: String = doc.sections.iter().map(|(_, b)| b.as_str()).collect();
            for needle in c.must_contain {
                assert!(
                    all_body.contains(needle),
                    "[seed {seed} case {i}] generated doc dropped source fact {needle:?}"
                );
            }

            let s = ref_score(&doc);
            println!("[seed {seed}] case {i} score={s:.3} ({} sections, {} unverified)", doc.sections.len(), doc.unverified_sections.len());
            case_scores.push(s);
        }
        let (mean, _) = mean_std(&case_scores);
        println!("[seed {seed}] mean score={mean:.3}");
        seed_scores.push(mean);
    }
    let (mean, std) = mean_std(&seed_scores);
    println!("[gate] reference_generate score mean={mean:.3} std={std:.3} over {n_seeds} seeds");
    assert!(
        mean >= REF_FLOOR,
        "reference_generate fidelity+grounding {mean:.3} < floor {REF_FLOOR} on {model}. \
         Do NOT lower the floor (Agent 验证铁律) — record honestly + label min tier in RELEASE.md."
    );
}

// ───────────────────────── non-ignored corpus guards ─────────────────────────

#[test]
fn corpora_well_formed() {
    assert!(SYNTH_CORPUS.len() >= 3, "synthesis corpus ≥3 cases");
    assert!(REF_CORPUS.len() >= 3, "reference corpus ≥3 cases");
    for (i, c) in SYNTH_CORPUS.iter().enumerate() {
        assert!(c.sources.len() >= 2, "synth case {i} must be multi-source");
        // must_contain facts must actually be present in some source (so the floor is achievable).
        for needle in c.must_contain {
            assert!(
                c.sources.iter().any(|(_, t)| t.contains(needle)),
                "synth case {i} must_contain {needle:?} not in any source"
            );
        }
        // must_not_assert facts must NOT be in any source (so they're genuine fabrication probes).
        for bogus in c.must_not_assert {
            assert!(
                !c.sources.iter().any(|(_, t)| t.contains(bogus)),
                "synth case {i} must_not_assert {bogus:?} unexpectedly in a source"
            );
        }
    }
    for (i, c) in REF_CORPUS.iter().enumerate() {
        for h in c.headings {
            assert!(c.reference.contains(h), "ref case {i} heading {h:?} not in reference");
        }
        for needle in c.must_contain {
            assert!(
                c.source_data.iter().any(|(_, t)| t.contains(needle)),
                "ref case {i} must_contain {needle:?} not in source data"
            );
        }
    }
}
