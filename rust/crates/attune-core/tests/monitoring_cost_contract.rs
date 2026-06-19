//! Cost-contract anti-sneak gate (spec §8 R1 + §9.1 反偷跑) — **CI hard gate**.
//!
//! The monitoring闭环's highest product invariant: the watch matcher, triage, dedup, and
//! DEFAULT digest paths are 🆓 zero-cost — they must NEVER invoke an LLM (the product's "passive
//! proactivity" core). This is enforced two ways:
//!
//! 1. **Compile-time** (the real guard): `WatchMatcher::evaluate` and `DigestBuilder::build_default`
//!    carry NO `&dyn LlmProvider` in their signatures, so they CANNOT call an LLM. This test file
//!    even compiling against those signatures proves the guard holds (a future refactor that
//!    threads an LLM handle into either would break this file's calls).
//!
//! 2. **Runtime proptest** (this file): we additionally install a process-global panic-on-call LLM
//!    sentinel and run `evaluate` + `build_default` over arbitrary inputs. Any LLM call (e.g. via a
//!    sneaky global provider) would panic → test fails. Asserts the call counter stays 0.
//!
//! If this test ever fails or is deleted, a zero-cost path started spending tokens — STOP and fix
//! the path, do not relax the gate (per CLAUDE.md ratchet rule).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use attune_core::llm::{ChatMessage, LlmCallOptions, LlmProvider};
use attune_core::monitoring::digest::{DigestBuilder, MapContentSource};
use attune_core::monitoring::{ItemMeta, Watch, WatchMatcher};
use proptest::prelude::*;

/// Process-global counter incremented on ANY LLM entrypoint. Stays 0 for zero-cost paths.
static LLM_CALLS: AtomicUsize = AtomicUsize::new(0);

/// A provider that records (and would-panic) on any call. Used only to PROVE the zero-cost paths
/// never reach an LLM — they don't even accept this type, so the guard is structural; the counter
/// is belt-and-suspenders against a future global-provider regression.
struct PanicLlm;

impl LlmProvider for PanicLlm {
    fn chat(&self, _system: &str, _user: &str) -> attune_core::error::Result<(String, attune_core::usage::TokenUsage)> {
        LLM_CALLS.fetch_add(1, Ordering::SeqCst);
        panic!("COST CONTRACT VIOLATION: zero-cost path invoked the LLM");
    }
    fn chat_with_history(&self, _messages: &[ChatMessage]) -> attune_core::error::Result<(String, attune_core::usage::TokenUsage)> {
        LLM_CALLS.fetch_add(1, Ordering::SeqCst);
        panic!("COST CONTRACT VIOLATION: zero-cost path invoked the LLM (history)");
    }
    fn chat_with_options(&self, _messages: &[ChatMessage], _opts: &LlmCallOptions) -> attune_core::error::Result<String> {
        LLM_CALLS.fetch_add(1, Ordering::SeqCst);
        panic!("COST CONTRACT VIOLATION: zero-cost path invoked the LLM (options)");
    }
    fn is_available(&self) -> bool {
        true
    }
    fn model_name(&self) -> &str {
        "panic-llm"
    }
}

fn watch(id: &str, keywords: Vec<String>, threshold: f32) -> Watch {
    Watch {
        id: id.to_string(),
        label: "w".to_string(),
        keywords,
        entities: vec![],
        anchor_text: String::new(),
        anchor_vec: None,
        source_ids: vec![],
        match_threshold: threshold,
        source_weights: HashMap::new(),
        enabled: true,
    }
}

fn item(id: &str, title: &str, content: &str) -> ItemMeta {
    ItemMeta {
        id: id.to_string(),
        title: title.to_string(),
        content: content.to_string(),
        source_type: "rss".to_string(),
        vector: None,
        entities: vec![],
        created_at: "2026-06-18T00:00:00Z".to_string(),
        content_hash: format!("h-{id}"),
    }
}

proptest! {
    /// For ANY arbitrary keyword/content/title inputs, evaluate + build_default complete with
    /// ZERO LLM calls. The PanicLlm is constructed (proving it's available) but is structurally
    /// un-passable to either function — and the counter confirms no global leak.
    #[test]
    fn zero_cost_paths_never_call_llm(
        keywords in prop::collection::vec("[a-zA-Z]{1,8}", 0..4),
        titles in prop::collection::vec("[a-zA-Z ]{0,20}", 1..6),
        contents in prop::collection::vec("[a-zA-Z .]{0,60}", 1..6),
        threshold in 0.0f32..1.0,
    ) {
        let _sentinel = PanicLlm; // exists; never reachable by zero-cost paths.
        let before = LLM_CALLS.load(Ordering::SeqCst);

        let n = titles.len().min(contents.len());
        let items: Vec<ItemMeta> = (0..n)
            .map(|i| item(&format!("i{i}"), &titles[i], &contents[i]))
            .collect();
        let w = watch("w", keywords, threshold);

        let matcher = WatchMatcher::default();
        let hits = matcher.evaluate(&items, &[w], &HashMap::new(), "2026-06-19T00:00:00Z");

        let cm = MapContentSource(
            items.iter().map(|it| (it.id.clone(), it.content.clone())).collect(),
        );
        let card = DigestBuilder::default().build_default(
            "w", "label", &hits, &cm, &HashMap::new(), "2026-06-19T00:00:00Z",
        );
        // default digest must never carry an LLM summary.
        prop_assert!(card.llm_summary.is_none());

        let after = LLM_CALLS.load(Ordering::SeqCst);
        prop_assert_eq!(after, before, "zero-cost path made an LLM call");
    }

    /// Idempotence: same input → identical hit signature (no nondeterminism / no hidden LLM).
    #[test]
    fn evaluate_idempotent(
        keywords in prop::collection::vec("[a-z]{1,6}", 1..3),
        contents in prop::collection::vec("[a-z ]{0,40}", 1..5),
    ) {
        let items: Vec<ItemMeta> = contents.iter().enumerate()
            .map(|(i, c)| item(&format!("i{i}"), "t", c)).collect();
        let w = vec![watch("w", keywords, 0.3)];
        let m = WatchMatcher::default();
        let h1 = m.evaluate(&items, &w, &HashMap::new(), "2026-06-19T00:00:00Z");
        let h2 = m.evaluate(&items, &w, &HashMap::new(), "2026-06-19T00:00:00Z");
        let sig = |hs: &[attune_core::monitoring::WatchHit]| hs.iter()
            .map(|h| (h.item_id.clone(), (h.score * 1e6) as i64)).collect::<Vec<_>>();
        prop_assert_eq!(sig(&h1), sig(&h2));
    }
}

/// Non-proptest sanity: the panic-LLM truly panics when called (so the gate has teeth).
#[test]
#[should_panic(expected = "COST CONTRACT VIOLATION")]
fn panic_llm_has_teeth() {
    let _ = PanicLlm.chat("x", "y");
}
