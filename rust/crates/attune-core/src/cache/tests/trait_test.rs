//! Unit tests for the cache trait surface — keys, scope, value.

use crate::cache::{cache_key, CacheScope, CachedValue};

#[test]
fn cache_key_is_blake3_32_hex_lowercase() {
    let k = cache_key("gpt-4o-mini", "hello world");
    assert_eq!(k.len(), 32, "32-hex prefix of blake3");
    assert!(
        k.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "expected all lowercase ascii hex, got: {k}"
    );
}

#[test]
fn cache_key_changes_with_model() {
    let a = cache_key("gpt-4o", "x");
    let b = cache_key("gpt-4o-mini", "x");
    assert_ne!(a, b, "different models must produce different keys");
}

#[test]
fn cache_key_changes_with_prompt() {
    let a = cache_key("m", "hello");
    let b = cache_key("m", "world");
    assert_ne!(a, b, "different prompts must produce different keys");
}

#[test]
fn cache_key_is_stable() {
    let a = cache_key("gpt-4o", "hello");
    let b = cache_key("gpt-4o", "hello");
    assert_eq!(a, b, "same inputs must produce same key");
}

#[test]
fn cache_key_no_concatenation_ambiguity() {
    // ("ab", "c") and ("a", "bc") must differ thanks to the 0xFF separator
    // in blake3 hashing — otherwise prefix-ambiguity would create false hits.
    let ambig_a = cache_key("ab", "c");
    let ambig_b = cache_key("a", "bc");
    assert_ne!(
        ambig_a, ambig_b,
        "model/prompt boundary must be unambiguous"
    );
}

#[test]
fn cached_value_holds_tokens_metadata() {
    let v = CachedValue {
        bytes: b"hello".to_vec(),
        tokens_in: 10,
        tokens_out: 5,
        model: "gpt-4o-mini".into(),
    };
    assert_eq!(v.bytes, b"hello");
    assert_eq!(v.tokens_in, 10);
    assert_eq!(v.tokens_out, 5);
    assert_eq!(v.model, "gpt-4o-mini");
}

#[test]
fn cache_scope_serializes_lowercase() {
    assert_eq!(serde_json::to_string(&CacheScope::Llm).unwrap(), r#""llm""#);
    assert_eq!(
        serde_json::to_string(&CacheScope::Embed).unwrap(),
        r#""embed""#
    );
    assert_eq!(
        serde_json::to_string(&CacheScope::Search).unwrap(),
        r#""search""#
    );
    assert_eq!(serde_json::to_string(&CacheScope::All).unwrap(), r#""all""#);
}

#[test]
fn cache_scope_round_trip() {
    for s in [
        CacheScope::Llm,
        CacheScope::Embed,
        CacheScope::Search,
        CacheScope::All,
    ] {
        let j = serde_json::to_string(&s).unwrap();
        let back: CacheScope = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back);
    }
}
