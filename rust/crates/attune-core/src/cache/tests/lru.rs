//! L1 MemoryLruCache backend tests.

use crate::cache::memory::MemoryLruCache;
use crate::cache::{CacheBackend, CacheScope, CachedValue};

fn mkval(b: &[u8]) -> CachedValue {
    CachedValue {
        bytes: b.to_vec(),
        tokens_in: 10,
        tokens_out: 5,
        model: "m".into(),
    }
}

#[tokio::test]
async fn put_then_get_returns_value() {
    let c = MemoryLruCache::new(10);
    c.put(CacheScope::Llm, "k1", mkval(b"v1"), None).await;
    let v = c.get(CacheScope::Llm, "k1").await.expect("hit");
    assert_eq!(v.bytes, b"v1");
    assert_eq!(v.tokens_in, 10);
}

#[tokio::test]
async fn miss_returns_none() {
    let c = MemoryLruCache::new(10);
    assert!(c.get(CacheScope::Llm, "nope").await.is_none());
}

#[tokio::test]
async fn lru_eviction_when_over_cap() {
    let c = MemoryLruCache::new(2);
    c.put(CacheScope::Llm, "a", mkval(b"a"), None).await;
    c.put(CacheScope::Llm, "b", mkval(b"b"), None).await;
    c.put(CacheScope::Llm, "c", mkval(b"c"), None).await; // evicts "a" (LRU)
    assert!(c.get(CacheScope::Llm, "a").await.is_none(), "a should be evicted");
    assert!(c.get(CacheScope::Llm, "b").await.is_some());
    assert!(c.get(CacheScope::Llm, "c").await.is_some());
}

#[tokio::test]
async fn recent_access_protects_from_eviction() {
    let c = MemoryLruCache::new(2);
    c.put(CacheScope::Llm, "a", mkval(b"a"), None).await;
    c.put(CacheScope::Llm, "b", mkval(b"b"), None).await;
    // Touch "a" so it becomes most recent; "b" is now LRU.
    let _ = c.get(CacheScope::Llm, "a").await;
    c.put(CacheScope::Llm, "c", mkval(b"c"), None).await; // should evict "b"
    assert!(c.get(CacheScope::Llm, "a").await.is_some(), "a should survive — was touched");
    assert!(c.get(CacheScope::Llm, "b").await.is_none(), "b should be evicted — LRU");
    assert!(c.get(CacheScope::Llm, "c").await.is_some());
}

#[tokio::test]
async fn count_returns_scope_size() {
    let c = MemoryLruCache::new(10);
    c.put(CacheScope::Llm, "a", mkval(b"x"), None).await;
    c.put(CacheScope::Embed, "b", mkval(b"y"), None).await;
    assert_eq!(c.count(CacheScope::Llm).await, 1);
    assert_eq!(c.count(CacheScope::Embed).await, 1);
    assert_eq!(c.count(CacheScope::Search).await, 0);
    assert_eq!(c.count(CacheScope::All).await, 2);
}

#[tokio::test]
async fn clear_scope_only_removes_that_scope() {
    let c = MemoryLruCache::new(10);
    c.put(CacheScope::Llm, "a", mkval(b"x"), None).await;
    c.put(CacheScope::Llm, "b", mkval(b"x"), None).await;
    c.put(CacheScope::Embed, "e", mkval(b"y"), None).await;
    let n = c.clear(CacheScope::Llm).await;
    assert_eq!(n, 2, "two llm entries removed");
    assert_eq!(c.count(CacheScope::Llm).await, 0);
    assert!(
        c.get(CacheScope::Embed, "e").await.is_some(),
        "embed scope untouched"
    );
}

#[tokio::test]
async fn clear_all_drains_every_scope() {
    let c = MemoryLruCache::new(10);
    c.put(CacheScope::Llm, "a", mkval(b"x"), None).await;
    c.put(CacheScope::Embed, "b", mkval(b"y"), None).await;
    c.put(CacheScope::Search, "c", mkval(b"z"), None).await;
    let n = c.clear(CacheScope::All).await;
    assert_eq!(n, 3);
    assert_eq!(c.count(CacheScope::All).await, 0);
}

#[tokio::test]
async fn put_replaces_existing_key() {
    let c = MemoryLruCache::new(10);
    c.put(CacheScope::Llm, "k", mkval(b"old"), None).await;
    c.put(CacheScope::Llm, "k", mkval(b"new"), None).await;
    let v = c.get(CacheScope::Llm, "k").await.unwrap();
    assert_eq!(v.bytes, b"new");
    assert_eq!(c.count(CacheScope::Llm).await, 1, "no duplicate row");
}

#[tokio::test]
async fn scopes_do_not_share_namespace() {
    let c = MemoryLruCache::new(10);
    c.put(CacheScope::Llm, "shared-key", mkval(b"llm-value"), None).await;
    c.put(CacheScope::Embed, "shared-key", mkval(b"embed-value"), None).await;
    let llm_v = c.get(CacheScope::Llm, "shared-key").await.unwrap();
    let embed_v = c.get(CacheScope::Embed, "shared-key").await.unwrap();
    assert_eq!(llm_v.bytes, b"llm-value");
    assert_eq!(embed_v.bytes, b"embed-value");
}
