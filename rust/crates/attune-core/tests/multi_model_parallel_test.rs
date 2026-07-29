use attune_core::cache::memory::MemoryLruCache;
use attune_core::cache::{CacheBackend, CacheScope, CachedValue};
use attune_core::document_intelligence::model_routing::{ModelRole, ModelRouter};
use attune_core::llm::LlmProvider;
use attune_core::{cache_key, TokenUsage};
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

#[derive(Debug, Clone)]
struct CallRecord {
    model: String,
    system: String,
    user: String,
}

#[derive(Clone)]
struct ParallelProbeLlm {
    model: String,
    barrier: Arc<Barrier>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    calls: Arc<Mutex<Vec<CallRecord>>>,
}

impl ParallelProbeLlm {
    fn new(
        model: &str,
        barrier: Arc<Barrier>,
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
        calls: Arc<Mutex<Vec<CallRecord>>>,
    ) -> Self {
        Self {
            model: model.to_string(),
            barrier,
            active,
            max_active,
            calls,
        }
    }
}

impl LlmProvider for ParallelProbeLlm {
    fn chat(&self, system: &str, user: &str) -> attune_core::error::Result<(String, TokenUsage)> {
        let now_active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(now_active, Ordering::SeqCst);
        self.barrier.wait();
        std::thread::sleep(Duration::from_millis(25));
        self.calls.lock().unwrap().push(CallRecord {
            model: self.model.clone(),
            system: system.to_string(),
            user: user.to_string(),
        });
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok((
            format!("response-from-{}", self.model),
            TokenUsage {
                tokens_in: 11,
                tokens_out: 7,
                cached_in: 0,
                model: self.model.clone(),
                provider: "parallel-probe".into(),
            },
        ))
    }

    fn is_available(&self) -> bool {
        true
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn is_local(&self) -> bool {
        self.model.contains("qwen") || self.model.contains("llama")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_roles_run_in_parallel_without_usage_or_cache_collision() {
    let router = ModelRouter::from_settings(&json!({
        "model_routing": {
            "cheap": "gpt-4o-mini",
            "reasoning": "gpt-4o",
            "vision": "qwen-vl:7b"
        }
    }));
    router.validate().unwrap();

    let barrier = Arc::new(Barrier::new(3));
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let prompt = "same prompt must not share model state".to_string();

    let handles: Vec<_> = ModelRole::all()
        .into_iter()
        .map(|role| {
            let llm = ParallelProbeLlm::new(
                router.pick(role),
                barrier.clone(),
                active.clone(),
                max_active.clone(),
                calls.clone(),
            );
            let prompt = prompt.clone();
            std::thread::spawn(move || llm.chat("sys", &prompt).unwrap().1)
        })
        .collect();

    let usages: Vec<TokenUsage> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(
        max_active.load(Ordering::SeqCst),
        3,
        "all three logical model roles should be able to overlap on one machine"
    );

    let usage_models: BTreeSet<_> = usages.iter().map(|u| u.model.clone()).collect();
    assert_eq!(
        usage_models,
        BTreeSet::from([
            "gpt-4o-mini".to_string(),
            "gpt-4o".to_string(),
            "qwen-vl:7b".to_string()
        ]),
        "usage attribution must carry the concrete model selected for each role"
    );

    let cache: Arc<dyn CacheBackend> = Arc::new(MemoryLruCache::new(16));
    for usage in &usages {
        cache
            .put(
                CacheScope::Llm,
                &cache_key(&usage.model, &prompt),
                CachedValue {
                    bytes: usage.model.as_bytes().to_vec(),
                    tokens_in: usage.tokens_in,
                    tokens_out: usage.tokens_out,
                    model: usage.model.clone(),
                },
                None,
            )
            .await;
    }

    assert_eq!(
        cache.count(CacheScope::Llm).await,
        3,
        "same prompt routed to three models must occupy three cache entries"
    );
    for usage in &usages {
        let key = cache_key(&usage.model, &prompt);
        let hit = cache
            .get(CacheScope::Llm, &key)
            .await
            .expect("model-specific cache hit");
        assert_eq!(hit.model, usage.model);
        assert_eq!(hit.bytes, usage.model.as_bytes());
    }

    let call_models: BTreeSet<_> = calls
        .lock()
        .unwrap()
        .iter()
        .map(|c| c.model.clone())
        .collect();
    assert_eq!(call_models, usage_models);
    assert!(calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| c.system == "sys" && c.user == prompt));
}

#[test]
fn byok_single_model_fallback_is_valid_but_not_fully_configured() {
    let router = ModelRouter::all_same("llama3.2:3b");
    router.validate().unwrap();
    assert!(!router.is_fully_configured());
    assert_eq!(router.pick(ModelRole::Cheap), "llama3.2:3b");
    assert_eq!(router.pick(ModelRole::Reasoning), "llama3.2:3b");
    assert_eq!(router.pick(ModelRole::Vision), "llama3.2:3b");
}
