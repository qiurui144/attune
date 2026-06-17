//! organizer engine — deterministic golden gate + property tests (CI-blocking).
//!
//! per `attune/CLAUDE.md` §「Agent 验证铁律」/ §6.1 测试矩阵:
//!   - deterministic path (GenericStrategy, no LLM) → required pass rate **1.00**
//!   - ground truth **independent** of the engine: every case's `expected.*` is
//!     hand-curated (vector geometry → expected min group count), not derived by
//!     running `analyze_items` and recording whatever it produced.
//!   - 6-class coverage:
//!       * Golden case ≥ 10 ......... `golden/organize/01..12` (12 files)
//!       * Property tests ≥ 3 ....... `mod proptests` below (3)
//!       * Boundary `#[test]` ....... organizer lib unit tests (mod.rs / grouping.rs)
//!       * Error case ............... `11-empty-scope-error.yaml`
//!       * Integration E2E .......... `attune-server/tests/organize_e2e_test.rs`
//!       * Regression fixture ....... `12-extractive-label-from-titles.yaml`
//!
//! Vectors are synthesized from a `lobe` axis index so the gate is fully
//! deterministic and offline (no embedding model, no LLM): items sharing a lobe
//! sit on the same basis axis (one tight cluster); different lobes are
//! orthogonal in 8-D Euclidean space, which HDBSCAN resolves into distinct
//! clusters once each lobe has ≥ min_cluster_size points.

use std::path::{Path, PathBuf};

use attune_core::organizer::{
    analyze_items,
    strategy::StrategyRegistry,
    types::ItemView,
    OrganizeError,
};
use serde::Deserialize;

const DIM: usize = 8;

// ── YAML schema ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GoldenCase {
    id: String,
    #[serde(default)]
    #[allow(dead_code)]
    description: String,
    min_cluster_size: usize,
    #[serde(default)]
    items: Vec<GoldenItem>,
    expected: GoldenExpected,
}

#[derive(Debug, Deserialize)]
struct GoldenItem {
    id: String,
    #[serde(default)]
    lobe: usize,
    #[serde(default)]
    dir: String,
    title: String,
    /// item has no vector at all (still embedding) → must land in noise.
    #[serde(default)]
    no_vector: bool,
    /// override the synthesized vector's dimension (simulate an old-model vector).
    #[serde(default)]
    dim_override: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
struct GoldenExpected {
    #[serde(default)]
    min_groups: usize,
    #[serde(default)]
    all_items_covered: bool,
    #[serde(default)]
    labels_non_empty: bool,
    #[serde(default)]
    tier: Option<u8>,
    #[serde(default)]
    dimension_mismatch: Option<usize>,
    /// if set, at least one group label must contain one of these substrings.
    #[serde(default)]
    label_any_contains: Vec<String>,
    /// if set, analyze_items must return this error code (no proposal).
    #[serde(default)]
    error: Option<String>,
}

/// Synthesize a vector for an item: lobe → basis axis; tiny per-index jitter.
fn synth_vector(lobe: usize, idx: usize, dim: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; dim];
    let axis = lobe.min(dim - 2);
    v[axis] = 1.0;
    v[axis + 1] = 0.01 * (idx as f32 + 1.0);
    v
}

fn item_views(case: &GoldenCase) -> Vec<ItemView> {
    case.items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let embedding = if it.no_vector {
                None
            } else {
                let dim = it.dim_override.unwrap_or(DIM);
                Some(synth_vector(it.lobe, i, dim))
            };
            ItemView {
                item_id: it.id.clone(),
                title: it.title.clone(),
                content_snippet: String::new(),
                dir: it.dir.clone(),
                embedding,
            }
        })
        .collect()
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/organize")
}

fn load_cases() -> Vec<GoldenCase> {
    let dir = golden_dir();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read golden dir {dir:?}: {e}"))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "yaml").unwrap_or(false))
        .collect();
    files.sort();
    files
        .iter()
        .map(|p| {
            let txt = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {p:?}: {e}"));
            serde_yaml::from_str::<GoldenCase>(&txt)
                .unwrap_or_else(|e| panic!("parse {p:?}: {e}"))
        })
        .collect()
}

/// Deterministic golden gate: every case must satisfy its hand-curated
/// expectations under GenericStrategy with NO LLM (tier-2 extractive).
#[test]
fn organize_golden_gate_all_pass() {
    let cases = load_cases();
    assert!(
        cases.len() >= 10,
        "golden floor is ≥10 cases, found {}",
        cases.len()
    );

    let mut failures: Vec<String> = Vec::new();
    for case in &cases {
        let reg = StrategyRegistry::new();
        let strat = reg.resolve(None); // GenericStrategy
        let items = item_views(case);
        let want = &case.expected;

        let result = analyze_items(
            format!("golden-{}", case.id),
            None,
            items,
            strat.as_ref(),
            None, // no LLM → deterministic tier-2
            case.min_cluster_size,
        );

        // Error-case branch.
        if let Some(err_code) = &want.error {
            match (&result, err_code.as_str()) {
                (Err(OrganizeError::EmptyScope), "empty-scope") => {}
                other => failures.push(format!(
                    "[{}] expected error {err_code}, got {other:?}",
                    case.id
                )),
            }
            continue;
        }

        let proposal = match result {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("[{}] unexpected error: {e}", case.id));
                continue;
            }
        };

        if proposal.groups.len() < want.min_groups {
            failures.push(format!(
                "[{}] min_groups {} but got {}",
                case.id,
                want.min_groups,
                proposal.groups.len()
            ));
        }

        if want.all_items_covered {
            let covered = proposal.all_item_ids().len();
            let input = case.items.len();
            if covered != input {
                failures.push(format!(
                    "[{}] coverage {covered} != input {input} (lost/dup items)",
                    case.id
                ));
            }
        }

        if want.labels_non_empty {
            for g in &proposal.groups {
                if g.label.trim().is_empty() {
                    failures.push(format!("[{}] group {} has empty label", case.id, g.group_id));
                }
            }
        }

        if let Some(tier) = want.tier {
            if proposal.cost.tier != tier {
                failures.push(format!(
                    "[{}] tier {} but got {}",
                    case.id, tier, proposal.cost.tier
                ));
            }
        }

        if let Some(mismatch) = want.dimension_mismatch {
            if proposal.dimension_mismatch_count != mismatch {
                failures.push(format!(
                    "[{}] dimension_mismatch {} but got {}",
                    case.id, mismatch, proposal.dimension_mismatch_count
                ));
            }
        }

        if !want.label_any_contains.is_empty() {
            let hit = proposal.groups.iter().any(|g| {
                let lower = g.label.to_lowercase();
                want.label_any_contains
                    .iter()
                    .any(|sub| lower.contains(&sub.to_lowercase()))
            });
            if !hit {
                let labels: Vec<&str> = proposal.groups.iter().map(|g| g.label.as_str()).collect();
                failures.push(format!(
                    "[{}] no group label contains any of {:?}; labels were {:?}",
                    case.id, want.label_any_contains, labels
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "organize golden gate failures ({}/{} cases had issues):\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

/// Multi-model tier matrix for the tier-3 LLM naming path. `#[ignore]` because
/// it needs a real model stack (Ollama / cloud) — never auto-run in CI, per
/// §1.3 compute authorization. Driven by `scripts/run-organize-tier-matrix.sh`,
/// which sets `ATTUNE_MATRIX_MODEL` per tier. Prints the LLM-produced cluster
/// names for each golden fixture so a human can judge naming quality and set the
/// minimum supported tier in RELEASE.md.
///
/// Run manually: ORGANIZE only a local Ollama model is wired here directly:
///   ATTUNE_MATRIX_MODEL=qwen2.5:3b cargo test -p attune-core \
///     --test organize_golden_gate -- --ignored organize_tier_matrix_naming
/// Cloud tiers (gemini/deepseek) require the OpenAI-compat provider to be wired
/// via env; left as PENDING-VERIFY until run against a real endpoint.
#[test]
#[ignore = "needs a real LLM stack (Ollama/cloud) + §1.3 compute approval; driven by scripts/run-organize-tier-matrix.sh, never in CI"]
fn organize_tier_matrix_naming() {
    use attune_core::llm::{LlmProvider, OllamaLlmProvider};
    use attune_core::organizer::strategy::{ClusterLabel, GenericStrategy, LabelCtx, OrganizationStrategy};
    use attune_core::organizer::types::ClusterView;

    let model = std::env::var("ATTUNE_MATRIX_MODEL").unwrap_or_else(|_| "qwen2.5:3b".to_string());
    eprintln!("== organize tier-matrix naming :: model = {model} ==");

    // Only local Ollama models are wired directly; cloud needs env provider.
    let llm: Box<dyn LlmProvider> = Box::new(OllamaLlmProvider::with_model(&model));
    let strat = GenericStrategy;

    for case in load_cases() {
        if case.expected.error.is_some() || case.items.is_empty() {
            continue;
        }
        let items = item_views(&case);
        // Build one ClusterView over the first lobe's items (a representative
        // group) and ask the LLM to name it — measures the naming path only.
        let members: Vec<ItemView> = items.into_iter().take(6).collect();
        let view = ClusterView { group_id: 0, items: &members };
        match strat.label_cluster(&view, &LabelCtx { llm: Some(llm.as_ref()) }) {
            Ok(ClusterLabel { name, summary, source, .. }) => {
                eprintln!("[{}] name={name:?} summary={summary:?} source={source:?}", case.id);
            }
            Err(e) => eprintln!("[{}] LLM naming error: {e}", case.id),
        }
    }
    eprintln!("== tier-matrix done for {model}: judge names above; set min tier in RELEASE.md ==");
}

// ── Property tests (≥3) — the core invariant: nothing lost, nothing duplicated.

mod proptests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::HashSet;

    /// Build ItemViews from a vector of (lobe, has_vector) flags, distinct ids.
    fn build_items(flags: &[(usize, bool)]) -> Vec<ItemView> {
        flags
            .iter()
            .enumerate()
            .map(|(i, (lobe, has_vec))| ItemView {
                item_id: format!("item-{i}"),
                title: format!("title {i}"),
                content_snippet: String::new(),
                dir: format!("/d{}", lobe % 3),
                embedding: if *has_vec {
                    Some(synth_vector(*lobe, i, DIM))
                } else {
                    None
                },
            })
            .collect()
    }

    proptest! {
        /// INVARIANT 1: groups ∪ noise == inputs (no loss, no dup), for any mix
        /// of lobes / missing vectors / cluster sizes.
        #[test]
        fn union_equals_inputs(
            flags in proptest::collection::vec((0usize..4, any::<bool>()), 1..30),
            min in 2usize..8,
        ) {
            let reg = StrategyRegistry::new();
            let strat = reg.resolve(None);
            let items = build_items(&flags);
            let input_ids: HashSet<String> =
                items.iter().map(|i| i.item_id.clone()).collect();

            let p = analyze_items("p".into(), None, items, strat.as_ref(), None, min)
                .expect("non-empty scope must yield a proposal");

            let out_ids: Vec<String> = p.all_item_ids();
            let out_set: HashSet<String> = out_ids.iter().cloned().collect();
            // no duplicates
            prop_assert_eq!(out_ids.len(), out_set.len(), "duplicate item in output");
            // exact set equality
            prop_assert_eq!(out_set, input_ids, "groups∪noise must equal inputs");
        }

        /// INVARIANT 2: tier is 2 whenever no LLM is supplied (cost contract:
        /// the no-LLM path never claims tier-3 spend).
        #[test]
        fn no_llm_always_tier_two(
            flags in proptest::collection::vec((0usize..4, any::<bool>()), 1..25),
            min in 2usize..8,
        ) {
            let reg = StrategyRegistry::new();
            let strat = reg.resolve(None);
            let items = build_items(&flags);
            let p = analyze_items("p".into(), None, items, strat.as_ref(), None, min)
                .expect("proposal");
            prop_assert_eq!(p.cost.tier, 2, "no-LLM path must be tier-2");
            prop_assert_eq!(p.cost.est_tokens, 0, "no-LLM path spends 0 tokens");
        }

        /// INVARIANT 3: every non-noise group is non-empty AND carries a
        /// non-empty extractive label (GenericStrategy never emits a blank
        /// group). Guards the labeling path under arbitrary inputs.
        #[test]
        fn groups_are_named_and_nonempty(
            flags in proptest::collection::vec((0usize..3, any::<bool>()), 1..25),
            min in 2usize..8,
        ) {
            let reg = StrategyRegistry::new();
            let strat = reg.resolve(None);
            let items = build_items(&flags);
            let p = analyze_items("p".into(), None, items, strat.as_ref(), None, min)
                .expect("proposal");
            for g in &p.groups {
                prop_assert!(!g.items.is_empty(), "named group must have ≥1 item");
                prop_assert!(!g.label.trim().is_empty(), "named group must have a label");
            }
        }
    }
}
