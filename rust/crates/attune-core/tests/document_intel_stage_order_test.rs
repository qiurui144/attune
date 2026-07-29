use attune_core::crypto::Key32;
use attune_core::document_intelligence::deep_summary::{
    summarize, DeepSummaryConfig, StageLlms, SummaryLevel,
};
use attune_core::document_intelligence::model_routing::ModelRouter;
use attune_core::llm::LlmProvider;
use attune_core::store::Store;
use attune_core::TokenUsage;
use serde_json::json;
use std::sync::{Arc, Mutex};

struct OrderedLlm {
    model: String,
    label: String,
    sequence: Arc<Mutex<Vec<String>>>,
}

impl OrderedLlm {
    fn new(model: &str, label: &str, sequence: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            model: model.to_string(),
            label: label.to_string(),
            sequence,
        }
    }
}

impl LlmProvider for OrderedLlm {
    fn chat(&self, _system: &str, _user: &str) -> attune_core::error::Result<(String, TokenUsage)> {
        self.sequence.lock().unwrap().push(self.label.clone());
        Ok((
            format!("{} summary", self.label),
            TokenUsage::empty("ordered-test", &self.model),
        ))
    }

    fn is_available(&self) -> bool {
        true
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

fn router() -> ModelRouter {
    ModelRouter::from_settings(&json!({
        "model_routing": {
            "cheap": "gpt-4o-mini",
            "reasoning": "gpt-4o",
            "vision": "qwen-vl:7b"
        }
    }))
}

fn long_doc() -> String {
    let paragraph = "This paragraph is intentionally long enough to force the deep summary map stage. It contains repeated engineering details, resource constraints, scheduling notes, and provenance requirements. ".repeat(40);
    format!("# Alpha\n\n{paragraph}\n\n# Beta\n\n{paragraph}\n")
}

#[test]
fn deep_summary_runs_all_cheap_map_calls_before_reasoning_reduce() {
    let sequence = Arc::new(Mutex::new(Vec::new()));
    let cheap = OrderedLlm::new("gpt-4o-mini", "cheap-map", sequence.clone());
    let reasoning = OrderedLlm::new("gpt-4o", "reasoning-reduce", sequence.clone());
    let llms = StageLlms {
        cheap: &cheap,
        reasoning: &reasoning,
    };
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let cfg = DeepSummaryConfig {
        min_tokens_for_pipeline: 0,
        short_block_tokens: 1,
        chunk_size: 260,
        chunk_overlap: 0,
        reduce_fanin: 10_000,
        ..DeepSummaryConfig::default()
    };

    let (_summary, bill) = summarize(
        &long_doc(),
        SummaryLevel::Detailed,
        "stage-order",
        &router(),
        &llms,
        &store,
        &dek,
        &cfg,
    )
    .unwrap();

    let sequence = sequence.lock().unwrap().clone();
    let first_reduce = sequence
        .iter()
        .position(|s| s == "reasoning-reduce")
        .expect("reduce call exists");
    let map_count = sequence.iter().filter(|s| *s == "cheap-map").count();
    let reduce_count = sequence.iter().filter(|s| *s == "reasoning-reduce").count();

    assert!(
        map_count > 1,
        "fixture should exercise multiple cheap map calls, got {sequence:?}"
    );
    assert_eq!(
        reduce_count, 1,
        "with reduce_fanin above chunk count there should be exactly one reduce call"
    );
    assert!(
        sequence[..first_reduce].iter().all(|s| s == "cheap-map"),
        "all map calls must finish before reduce starts: {sequence:?}"
    );
    assert!(
        sequence[first_reduce + 1..]
            .iter()
            .all(|s| s == "reasoning-reduce"),
        "no late map call may run after reduce starts: {sequence:?}"
    );
    assert_eq!(bill.map_llm_tokens.model, "gpt-4o-mini");
    assert_eq!(bill.reduce_llm_tokens.model, "gpt-4o");
}
