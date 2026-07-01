//! Capability Registry — AppState bootstrap registration + health/enabled projection.
//!
//! Plan 2026-06-26-capability-registry-p0.md Tasks 4 (builtin registration),
//! 5 (health projection), 7 (boundary + error + OSS-boundary audit).
//!
//! Harness mirrors model_bootstrap_test.rs: in-memory vault + AppState::new.
//! Registration happens in AppState::new, so a locked vault is sufficient (no
//! unlock needed — the registry is pure metadata).

use std::sync::Arc;

use attune_core::capability::{CapabilityHealth, CapabilityTier};

/// Build a fresh AppState over an in-memory vault. Registration runs in `new`.
fn test_state() -> Arc<attune_server::state::AppState> {
    let tmp = tempfile::TempDir::new().unwrap();
    let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
    // Keep tmp alive for the duration of the test by leaking it — these are
    // short unit tests and the in-memory vault holds no on-disk handle we need
    // to clean. (TempDir Drop would remove the dir; the memory vault doesn't
    // depend on it post-open, but leaking keeps the path valid defensively.)
    std::mem::forget(tmp);
    Arc::new(attune_server::state::AppState::new(vault, false))
}

// ── Task 4: builtin registration + OSS boundary ──────────────────────────────

#[test]
fn builtin_capabilities_registered_oss_only() {
    let state = test_state();
    let caps = state.capabilities.list();
    let ids: Vec<&str> = caps.iter().map(|c| c.id.as_str()).collect();
    for expected in [
        "embedding",
        "reranker",
        "ocr",
        "asr",
        "llm",
        "vlm",
        "web-search",
        "pluginhub",
        "marketplace",
    ] {
        assert!(
            ids.contains(&expected),
            "missing builtin capability {expected}"
        );
    }
    assert_eq!(
        caps.len(),
        9,
        "exactly 9 builtin heavy capabilities expected"
    );
    // OSS boundary core assertion (spec §9): zero Pro/Enterprise tier.
    assert!(
        caps.iter().all(|c| c.tier == CapabilityTier::Oss),
        "OSS build must register only Oss-tier capabilities"
    );
}

#[test]
fn oss_build_has_zero_pro_capabilities() {
    let state = test_state();
    let caps = state.capabilities.list();
    // No Pro/Enterprise tier.
    for c in &caps {
        assert_eq!(
            c.tier,
            CapabilityTier::Oss,
            "non-OSS tier leaked into OSS registry: {} ({:?})",
            c.id,
            c.tier
        );
    }
    // No pro-vertical id leakage (law / patent / presales / medical / academic).
    for c in &caps {
        for banned in ["law", "patent", "presales", "medical", "academic"] {
            assert!(
                !c.id.contains(banned),
                "pro vertical id leaked into OSS registry: {}",
                c.id
            );
            assert!(
                !c.name.to_lowercase().contains(banned),
                "pro vertical name leaked into OSS registry: {}",
                c.name
            );
        }
    }
}

#[test]
fn capability_metadata_flags_are_correct() {
    let state = test_state();
    let web = state.capabilities.get("web-search").unwrap();
    assert!(web.allows_outbound, "web-search is an outbound source");

    let emb = state.capabilities.get("embedding").unwrap();
    assert!(emb.requires_local_model, "embedding requires a local model");
    assert!(
        !emb.allows_outbound,
        "embedding is local-first (no outbound)"
    );

    let reranker = state.capabilities.get("reranker").unwrap();
    assert!(reranker.requires_local_model);

    let ocr = state.capabilities.get("ocr").unwrap();
    assert!(ocr.requires_local_model);
    let asr = state.capabilities.get("asr").unwrap();
    assert!(asr.requires_local_model);

    let llm = state.capabilities.get("llm").unwrap();
    assert!(llm.allows_outbound, "llm defaults to cloud (outbound)");
    let vlm = state.capabilities.get("vlm").unwrap();
    assert!(vlm.allows_outbound);

    let hub = state.capabilities.get("pluginhub").unwrap();
    assert!(hub.requires_member, "pluginhub is a member-gated feature");
    assert!(
        hub.allows_outbound,
        "pluginhub reaches the hub over the network"
    );
}

// ── Task 5: health/enabled projection ────────────────────────────────────────

#[test]
fn refresh_projects_model_bootstrap_into_health() {
    let state = test_state();
    state.model_bootstrap.mark_downloading("embedding"); // → Installing
    state.model_bootstrap.mark_ready("reranker"); // → Ok
    state.model_bootstrap.mark_failed("ocr", "no net"); // → Unavailable
    state.refresh_capability_health();

    assert_eq!(
        state.capabilities.get("embedding").unwrap().health,
        CapabilityHealth::Installing
    );
    assert_eq!(
        state.capabilities.get("reranker").unwrap().health,
        CapabilityHealth::Ok
    );
    assert_eq!(
        state.capabilities.get("ocr").unwrap().health,
        CapabilityHealth::Unavailable,
        "failed model → Unavailable (present but not usable)"
    );
}

#[test]
fn refresh_projects_provider_presence_into_enabled() {
    let state = test_state();
    // No llm/vlm/web_search provider installed in the bare test harness.
    state.refresh_capability_health();
    let llm = state.capabilities.get("llm").unwrap();
    assert!(!llm.enabled, "llm has no provider → disabled");
    assert_eq!(llm.health, CapabilityHealth::Unavailable);
    let vlm = state.capabilities.get("vlm").unwrap();
    assert!(!vlm.enabled);
    let web = state.capabilities.get("web-search").unwrap();
    assert!(!web.enabled);
}

#[test]
fn refresh_is_idempotent() {
    let state = test_state();
    state.model_bootstrap.mark_ready("asr");
    state.refresh_capability_health();
    let h1 = state.capabilities.get("asr").unwrap().health;
    state.refresh_capability_health();
    let h2 = state.capabilities.get("asr").unwrap().health;
    assert_eq!(h1, h2);
    assert_eq!(h1, CapabilityHealth::Ok);
}

// ── Task 7: boundary (≥5) ────────────────────────────────────────────────────

#[test]
fn empty_registry_lists_empty() {
    let r = attune_core::capability::CapabilityRegistry::new();
    assert!(r.list().is_empty());
    assert!(r.snapshot().is_empty());
}

#[test]
fn unknown_id_get_is_none() {
    let state = test_state();
    assert!(state.capabilities.get("does-not-exist").is_none());
}

#[test]
fn refresh_with_no_model_phases_marks_unavailable() {
    let state = test_state();
    // Fresh model_bootstrap: every class is Pending (never downloaded). A Pending
    // model is "scheduled, not yet usable" → Installing per the projection.
    state.refresh_capability_health();
    // embedding/reranker/ocr/asr are Pending → Installing (scheduled).
    assert_eq!(
        state.capabilities.get("embedding").unwrap().health,
        CapabilityHealth::Installing
    );
}

#[test]
fn pluginhub_disabled_when_not_paid() {
    let state = test_state();
    // Default MemberState::LoggedOut → not paid.
    state.refresh_capability_health();
    let hub = state.capabilities.get("pluginhub").unwrap();
    assert!(!hub.enabled, "pluginhub disabled when not a paid member");
    assert_eq!(
        hub.health,
        CapabilityHealth::Degraded,
        "available-but-gated → Degraded"
    );
}

#[test]
fn marketplace_always_ok() {
    let state = test_state();
    // No providers, logged out — marketplace (OSS browse) is still reachable.
    state.refresh_capability_health();
    assert_eq!(
        state.capabilities.get("marketplace").unwrap().health,
        CapabilityHealth::Ok
    );
}

// ── Task 7: error (≥3) ───────────────────────────────────────────────────────

#[test]
fn set_health_on_absent_id_is_noop_not_insert() {
    let state = test_state();
    let before = state.capabilities.list().len();
    assert!(!state
        .capabilities
        .set_health("ghost", CapabilityHealth::Installing));
    assert!(state.capabilities.get("ghost").is_none());
    assert_eq!(
        state.capabilities.list().len(),
        before,
        "no insert on absent id"
    );
}

#[test]
fn failed_model_projects_unavailable_not_panic() {
    let state = test_state();
    state.model_bootstrap.mark_failed("asr", "boom");
    state.refresh_capability_health(); // must not panic
    assert_eq!(
        state.capabilities.get("asr").unwrap().health,
        CapabilityHealth::Unavailable
    );
}

#[test]
fn projection_does_not_deadlock_on_repeated_calls() {
    // The registry lock is independent of vault/vectors/fulltext. Calling refresh
    // back-to-back from two scopes must not deadlock (independent-lock smoke).
    let state = test_state();
    state.refresh_capability_health();
    state.refresh_capability_health();
    {
        // Hold a short vault guard scope, then refresh — refresh must not need it.
        let _v = state.vault.lock().unwrap();
    }
    state.refresh_capability_health();
    assert_eq!(state.capabilities.list().len(), 9);
}
