//! Unit tests for usage::types — JSON wire format + serde round-trip.

use crate::usage::types::{
    CacheOutcome, CallOutcome, ErrorKind, TokenUsage, UsageEvent, UsageKind,
};

#[test]
fn token_usage_serializes_camelcase_for_ui() {
    let t = TokenUsage {
        tokens_in: 100,
        tokens_out: 50,
        cached_in: 20,
        model: "gemini-1.5-flash".into(),
        provider: "cloud_gateway".into(),
    };
    let j = serde_json::to_string(&t).unwrap();
    assert!(
        j.contains(r#""tokensIn":100"#),
        "wire format must be camelCase, got: {j}"
    );
    assert!(j.contains(r#""cachedIn":20"#));
}

#[test]
fn cache_outcome_serializes_lowercase_string() {
    assert_eq!(
        serde_json::to_string(&CacheOutcome::Hit).unwrap(),
        r#""hit""#
    );
    assert_eq!(
        serde_json::to_string(&CacheOutcome::Miss).unwrap(),
        r#""miss""#
    );
    assert_eq!(
        serde_json::to_string(&CacheOutcome::Bypass).unwrap(),
        r#""bypass""#
    );
}

#[test]
fn call_outcome_carries_retry_attempt() {
    let r = CallOutcome::Retry { attempt: 2 };
    let j = serde_json::to_string(&r).unwrap();
    assert!(
        j.contains(r#""attempt":2"#),
        "Retry must serialize attempt field, got: {j}"
    );
}

#[test]
fn call_outcome_fail_carries_error_kind() {
    let f = CallOutcome::Fail {
        error_kind: ErrorKind::Timeout,
    };
    let j = serde_json::to_string(&f).unwrap();
    assert!(j.contains("timeout"), "Fail must serialize error_kind, got: {j}");
}

#[test]
fn usage_event_round_trip() {
    let e = UsageEvent {
        ts_ms: 1_717_000_000_000,
        kind: UsageKind::LlmChat,
        usage: TokenUsage {
            tokens_in: 200,
            tokens_out: 80,
            cached_in: 0,
            model: "qwen2.5:3b".into(),
            provider: "ollama".into(),
        },
        cost_usd: Some(0.0),
        cache: CacheOutcome::Miss,
        outcome: CallOutcome::Ok,
        latency_ms: 320,
        agent_id: None,
        query_hash: None,
    };
    let j = serde_json::to_string(&e).unwrap();
    let back: UsageEvent = serde_json::from_str(&j).unwrap();
    assert_eq!(back.ts_ms, 1_717_000_000_000);
    assert!(matches!(back.outcome, CallOutcome::Ok));
    assert_eq!(back.usage.tokens_in, 200);
    assert_eq!(back.usage.provider, "ollama");
}
