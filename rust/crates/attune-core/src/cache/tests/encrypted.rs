//! L2 SqliteEncryptedCache backend tests.

use std::sync::{Arc, Mutex};

use tempfile::TempDir;

use crate::cache::sqlite_encrypted::SqliteEncryptedCache;
use crate::cache::{CacheBackend, CacheScope, CachedValue};
use crate::crypto::Key32;
use crate::store::Store;

fn mkval(b: &[u8]) -> CachedValue {
    CachedValue {
        bytes: b.to_vec(),
        tokens_in: 42,
        tokens_out: 7,
        model: "gpt-4o-mini".into(),
    }
}

fn make_store_in(dir: &TempDir) -> Arc<Mutex<Store>> {
    let store = Store::open(&dir.path().join("v.db")).unwrap();
    Arc::new(Mutex::new(store))
}

#[tokio::test]
async fn put_then_get_returns_plaintext() {
    let dir = TempDir::new().unwrap();
    let store = make_store_in(&dir);
    let dek = Key32::generate();
    let c = SqliteEncryptedCache::new(store, dek);

    c.put(CacheScope::Llm, "k", mkval(b"secret response"), None)
        .await;
    let v = c.get(CacheScope::Llm, "k").await.expect("hit");
    assert_eq!(v.bytes, b"secret response");
    assert_eq!(v.tokens_in, 42);
    assert_eq!(v.tokens_out, 7);
    assert_eq!(v.model, "gpt-4o-mini");
}

#[tokio::test]
async fn raw_blob_is_ciphertext_not_plaintext() {
    let dir = TempDir::new().unwrap();
    let store = make_store_in(&dir);
    let dek = Key32::generate();
    let c = SqliteEncryptedCache::new(store.clone(), dek);

    let marker = b"PLAINTEXT-MARKER-XYZZY";
    c.put(CacheScope::Llm, "k", mkval(marker), None).await;

    // Inspect raw SQLite directly to confirm encryption-at-rest.
    let g = store.lock().unwrap();
    let raw: Vec<u8> = g
        .raw_connection_for_test()
        .query_row(
            "SELECT response FROM llm_cache WHERE key='k'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    drop(g);

    assert!(
        !raw.windows(marker.len()).any(|w| w == marker),
        "L2 blob must be encrypted at rest; found plaintext marker. Raw: {:?}",
        raw
    );
    // AES-256-GCM output = 12-byte nonce + ciphertext + 16-byte tag, so the
    // ciphertext is always longer than the plaintext.
    assert!(
        raw.len() > marker.len(),
        "ciphertext should include nonce + tag overhead"
    );
}

#[tokio::test]
async fn get_miss_returns_none() {
    let dir = TempDir::new().unwrap();
    let store = make_store_in(&dir);
    let dek = Key32::generate();
    let c = SqliteEncryptedCache::new(store, dek);

    assert!(c.get(CacheScope::Llm, "never-put").await.is_none());
}

#[tokio::test]
async fn embed_scope_is_bypassed_by_this_backend() {
    let dir = TempDir::new().unwrap();
    let store = make_store_in(&dir);
    let dek = Key32::generate();
    let c = SqliteEncryptedCache::new(store, dek);

    c.put(CacheScope::Embed, "k", mkval(b"vec"), None).await;
    // No put happened — embed scope is intentionally bypassed.
    assert!(c.get(CacheScope::Embed, "k").await.is_none());
}

#[tokio::test]
async fn wrong_dek_treats_existing_entry_as_miss() {
    let dir = TempDir::new().unwrap();
    let store = make_store_in(&dir);

    // Write with DEK_A.
    let dek_a = Key32::generate();
    let writer = SqliteEncryptedCache::new(store.clone(), dek_a);
    writer
        .put(CacheScope::Llm, "k", mkval(b"data-under-dek-a"), None)
        .await;

    // Read with DEK_B — decrypt must fail and be reported as miss (spec §7.3
    // graceful degradation: corrupt blob must not crash the call path).
    let dek_b = Key32::generate();
    let reader = SqliteEncryptedCache::new(store, dek_b);
    assert!(reader.get(CacheScope::Llm, "k").await.is_none());
}

#[tokio::test]
async fn count_and_clear_delegate_to_store() {
    let dir = TempDir::new().unwrap();
    let store = make_store_in(&dir);
    let dek = Key32::generate();
    let c = SqliteEncryptedCache::new(store, dek);

    c.put(CacheScope::Llm, "a", mkval(b"x"), None).await;
    c.put(CacheScope::Llm, "b", mkval(b"y"), None).await;
    assert_eq!(c.count(CacheScope::Llm).await, 2);

    let removed = c.clear(CacheScope::Llm).await;
    assert_eq!(removed, 2);
    assert_eq!(c.count(CacheScope::Llm).await, 0);
}
