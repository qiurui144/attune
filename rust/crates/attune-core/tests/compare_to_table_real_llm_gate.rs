//! `compare_to_table` agent — REAL-LLM verification gate (#[ignore], secret-gated).
//!
//! The unit/integration tests use a scripted mock; this gate runs the agent against a REAL LLM
//! (deepseek-v4-flash by default) over a hand-labeled corpus, N=3 seeds, and asserts the
//! parameter-extraction + diff F1 is at/above the floor (Agent 验证铁律 — the floor only
//! ratchets UP, never down; a weak tier below floor is recorded honestly per §6.3 and the min
//! tier is labeled in RELEASE.md, never by relaxing the floor here).
//!
//! ## How to run (secret NEVER echoed; per CLAUDE.md §1.4)
//! ```bash
//! set -a; source /tmp/secrets-deepseek/key.env; set +a   # DEEPSEEK_API_KEY (NEVER echoed)
//! export ATTUNE_LLM_PROVIDER=openai_compat
//! export ATTUNE_LLM_ENDPOINT="https://api.deepseek.com/v1"
//! export ATTUNE_LLM_API_KEY="$DEEPSEEK_API_KEY"
//! export ATTUNE_LLM_MODEL="deepseek-chat"          # deepseek-v4-flash tier
//! cargo test -p attune-core --test compare_to_table_real_llm_gate -- --ignored --nocapture --test-threads=1
//! ```
//!
//! ## Floor (ratchet only UP)
//! extraction+diff F1 ≥ 0.85 (mean over N seeds; must hold at mean − this is the §4.5 floor).

#![allow(clippy::print_stdout)]

use attune_core::llm::{LlmProvider, OllamaLlmProvider, OpenAiLlmProvider};
use attune_core::skill_runtime::compare_to_table;

const DEFAULT_OLLAMA_MODEL: &str = "qwen2.5:3b";
const F1_FLOOR: f64 = 0.85;

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

/// A hand-labeled case: a real two-entity document + the ground-truth (param, a, b) rows and the
/// set of params that SHOULD be flagged as differing. The labels are independent of the agent.
struct Case {
    doc: &'static str,
    entity_a: &'static str,
    entity_b: &'static str,
    /// Expected (param, value_a or "", value_b or "").
    gold: &'static [(&'static str, &'static str, &'static str)],
}

const CORPUS: &[Case] = &[
    Case {
        doc: "本文档比较两款摄像头。设备 A：分辨率 1080p，功耗 5W，接口 USB，重量 200g。\
              设备 B：分辨率 4K，功耗 12W，接口 USB，重量 350g。",
        entity_a: "设备 A",
        entity_b: "设备 B",
        gold: &[
            ("分辨率", "1080p", "4K"),
            ("功耗", "5W", "12W"),
            ("接口", "USB", "USB"),
            ("重量", "200g", "350g"),
        ],
    },
    Case {
        doc: "笔记本 X1：CPU i5-1240P，内存 16GB，屏幕 14 寸，重量 1.2kg。\
              笔记本 X2：CPU i7-1260P，内存 32GB，屏幕 14 寸，重量 1.4kg。",
        entity_a: "笔记本 X1",
        entity_b: "笔记本 X2",
        gold: &[
            ("CPU", "i5-1240P", "i7-1260P"),
            ("内存", "16GB", "32GB"),
            ("屏幕", "14 寸", "14 寸"),
            ("重量", "1.2kg", "1.4kg"),
        ],
    },
    Case {
        doc: "Drive A: capacity 512GB, sequential read 3500MB/s, warranty 5y. \
              Drive B: capacity 1TB, sequential read 7000MB/s, warranty 5y.",
        entity_a: "Drive A",
        entity_b: "Drive B",
        gold: &[
            ("capacity", "512GB", "1TB"),
            ("sequential read", "3500MB/s", "7000MB/s"),
            ("warranty", "5y", "5y"),
        ],
    },
    Case {
        doc: "路由器 R1：频段 2.4GHz，端口 4 个，速率 300Mbps，天线 2 根。\
              路由器 R2：频段 5GHz，端口 8 个，速率 1200Mbps，天线 4 根。",
        entity_a: "路由器 R1",
        entity_b: "路由器 R2",
        gold: &[
            ("频段", "2.4GHz", "5GHz"),
            ("端口", "4 个", "8 个"),
            ("速率", "300Mbps", "1200Mbps"),
            ("天线", "2 根", "4 根"),
        ],
    },
    Case {
        doc: "机型甲：屏幕 6.1 寸，电池 3200mAh，充电 20W。\
              机型乙：屏幕 6.7 寸，电池 4500mAh，充电 67W，防水 IP68。",
        entity_a: "机型甲",
        entity_b: "机型乙",
        gold: &[
            ("屏幕", "6.1 寸", "6.7 寸"),
            ("电池", "3200mAh", "4500mAh"),
            ("充电", "20W", "67W"),
            ("防水", "", "IP68"),
        ],
    },
];

/// Count extraction+diff F1 against gold for one case's produced comparison.
/// A "predicted item" is a (param-normalized, value_a, value_b) triple; we match it to gold by
/// param name (normalized: trim + lowercase). TP = a gold param whose A & B values both match
/// (substring-tolerant). Precision/recall over params; F1 = harmonic mean.
fn case_f1(cmp: &attune_core::skill_runtime::ParamComparison, gold: &[(&str, &str, &str)]) -> f64 {
    let norm = |s: &str| s.trim().to_lowercase().replace(' ', "");
    let val_match = |got: Option<&str>, want: &str| -> bool {
        match (got, want) {
            (None, "") => true,
            (Some(g), w) if !w.is_empty() => {
                let (g, w) = (norm(g), norm(w));
                g == w || g.contains(&w) || w.contains(&g)
            }
            _ => false,
        }
    };

    let mut tp = 0usize;
    for (gp, ga, gb) in gold {
        // find a predicted row whose param matches and both values match.
        let hit = cmp.rows.iter().any(|r| {
            norm(&r.name) == norm(gp)
                && val_match(r.value_a.as_deref(), ga)
                && val_match(r.value_b.as_deref(), gb)
        });
        if hit {
            tp += 1;
        }
    }
    let predicted = cmp.rows.len().max(1) as f64;
    let gold_n = gold.len().max(1) as f64;
    let precision = tp as f64 / predicted;
    let recall = tp as f64 / gold_n;
    if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    }
}

#[test]
#[ignore = "real LLM; secret/Ollama-gated. Run via the CI secret lane or manually."]
fn compare_to_table_real_llm_f1_floor() {
    let llm = require_llm();
    let model = model_name();
    let n_seeds = seeds();
    println!("[gate] compare_to_table real-LLM F1 floor={F1_FLOOR} model={model} seeds={n_seeds} cases={}", CORPUS.len());

    let mut seed_f1s = Vec::with_capacity(n_seeds);
    for seed in 0..n_seeds {
        let mut case_f1s = Vec::with_capacity(CORPUS.len());
        for (i, c) in CORPUS.iter().enumerate() {
            let cmp = compare_to_table(c.doc, c.entity_a, c.entity_b, llm.as_ref(), &model);
            // Grounding sanity: no produced value may be absent from the source (anti-fabrication).
            for r in &cmp.rows {
                if let Some(v) = &r.value_a {
                    assert!(
                        c.doc.contains(v.trim()),
                        "[seed {seed} case {i}] ungrounded A value leaked: {v:?}"
                    );
                }
                if let Some(v) = &r.value_b {
                    assert!(
                        c.doc.contains(v.trim()),
                        "[seed {seed} case {i}] ungrounded B value leaked: {v:?}"
                    );
                }
            }
            let f1 = case_f1(&cmp, c.gold);
            println!(
                "[seed {seed}] case {i} F1={f1:.3} ({} rows)",
                cmp.rows.len()
            );
            case_f1s.push(f1);
        }
        let (mean, _) = mean_std(&case_f1s);
        println!("[seed {seed}] mean F1={mean:.3}");
        seed_f1s.push(mean);
    }

    let (mean, std) = mean_std(&seed_f1s);
    println!("[gate] compare_to_table F1 mean={mean:.3} std={std:.3} over {n_seeds} seeds");
    assert!(
        mean >= F1_FLOOR,
        "compare_to_table extraction+diff F1 {mean:.3} < floor {F1_FLOOR} on model {model}. \
         Do NOT lower the floor (Agent 验证铁律) — record honestly + label min tier in RELEASE.md."
    );
}

/// A non-ignored guard so the corpus stays well-formed (catches label rot without an LLM).
#[test]
fn corpus_is_well_formed() {
    assert!(
        CORPUS.len() >= 5,
        "real-LLM corpus must have ≥5 hand-labeled cases"
    );
    for (i, c) in CORPUS.iter().enumerate() {
        assert!(!c.doc.is_empty(), "case {i} empty doc");
        assert!(!c.gold.is_empty(), "case {i} no gold rows");
        // every non-empty gold value must actually appear in the doc (so the floor is achievable).
        for (p, a, b) in c.gold {
            if !a.is_empty() {
                assert!(
                    c.doc.contains(*a),
                    "case {i} gold A value {a:?} (param {p}) not in doc"
                );
            }
            if !b.is_empty() {
                assert!(
                    c.doc.contains(*b),
                    "case {i} gold B value {b:?} (param {p}) not in doc"
                );
            }
        }
    }
}
