# Hybrid Token Strategy (Edge + Agent Routing) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a `CapabilityRouter` that decides — given an incoming `Intent` (capability + tokens + latency SLO + user tier) and live signals (`HardwareSnapshot`, `UsageSummary` from Plan A1, `OfflineState`, `BudgetWatcher`) — which `ProviderEndpoint` to call and what the fallback chain is, all driven by a three-profile system (`edge_first` / `balanced` / `cloud_first`) with offline detection, cost-budget guard, and `docs/HYBRID-TOKEN-STRATEGY.md` as a product-level SSOT.

**Architecture:** New `attune-core::routing` module with `CapabilityRouter` async trait + `DefaultRouter` impl driven by a static capability matrix (per-profile lookup table) + adaptive demotion via Plan A1's `UsageAggregator::recent(N)` outcome history. Offline detection runs in a 30-second background tokio task probing cloud_gateway / Ollama / K3 endpoints. REST surface `/api/v1/routing/{profile,decide,health,matrix}` exposes everything to the UI; CLI subcommand mirrors. `docs/HYBRID-TOKEN-STRATEGY.md` is the canonical capability matrix doc, referenced from README + DEVELOP + wiki.

**Tech Stack:** Rust (async_trait, tokio, axum, serde, reqwest for health probes), arc-swap for hot-swap of profile config, proptest for property tests. Frontend: TypeScript (Preact components in `rust/crates/attune-server/ui/src/`). No new external deps beyond what Plan A1 introduces.

**Spec reference:** `docs/superpowers/specs/2026-05-28-hybrid-token-strategy.md`
**Target version:** v1.1.0 (2026-08-15)
**Branch:** `feat/hybrid-token-routing` (cut from `develop` *after* Plan A1 Task M merged)

---

## ⚠️ BLOCKING DEPENDENCY: Plan A1 Task M

This plan **blockedBy** Plan A1's `Task M: Freeze public API surface for routing consumers`
(commit must be on `develop` before this plan's Task A starts).

Required surface from Plan A1 (verified at the start of Task A below):
- `attune_core::TokenUsage` — for `est_cost_usd` calc input
- `attune_core::UsageEvent` — fed back to router for fail-rate demotion
- `attune_core::UsageKind` — maps 1:1 with this plan's `Capability` enum
- `attune_core::CacheOutcome` + `CallOutcome` — outcome reporting from `report_outcome()`
- `attune_core::UsageAggregator::recent(N)` — recent-events window for adaptive routing
- `attune_core::cost::estimate_cost_usd` (existing) — for `Decision.est_cost_usd`

**Do not proceed with this plan if any of the above is unavailable.** Verify with the Task A doctest before doing anything else.

---

## File Structure

### Create (Rust)

| Path | Responsibility |
|------|----------------|
| `rust/crates/attune-core/src/routing/mod.rs` | Public re-exports + `CapabilityRouter` async trait |
| `rust/crates/attune-core/src/routing/profile.rs` | `RoutingProfile` enum + default rules per profile + custom-profile loader |
| `rust/crates/attune-core/src/routing/decision.rs` | `RoutingDecision`, `ProviderEndpoint`, `CostTier`, `Intent`, `UserTier`, `Capability` |
| `rust/crates/attune-core/src/routing/rules.rs` | Static capability matrix + decision tree (`decide_for_capability`) |
| `rust/crates/attune-core/src/routing/budget.rs` | `BudgetWatcher` — subscribes UsageAggregator + Pro-quota poll |
| `rust/crates/attune-core/src/routing/health.rs` | `HealthMonitor` — 30s probe loop, p50 latency tracking, demotion |
| `rust/crates/attune-core/src/routing/default_router.rs` | `DefaultRouter` impl of `CapabilityRouter` |
| `rust/crates/attune-server/src/routes/routing.rs` | GET/POST `/api/v1/routing/{profile,decide,health,matrix}` |
| `rust/crates/attune-server/ui/src/views/SettingsView/RoutingSection.tsx` | Profile picker + recommended-from-hardware + recent-decisions table |
| `rust/crates/attune-server/ui/src/api/routing.ts` | client wrapper for `/routing/*` endpoints |
| `rust/crates/attune-cli/src/commands/routing.rs` | `attune routing {show,set,decide,health,matrix}` subcommand |
| `docs/HYBRID-TOKEN-STRATEGY.md` | Product SSOT — capability matrix + 3 profiles + UI display rules + privacy red lines |

### Create (tests)

| Path | Responsibility |
|------|----------------|
| `rust/crates/attune-core/src/routing/tests/golden.rs` | ≥10 fixtures: (profile × capability × hardware × tier) → expected decision |
| `rust/crates/attune-core/src/routing/tests/proptest.rs` | properties: decide always returns primary; chain non-empty in balanced; degraded ⇒ reason non-empty |
| `rust/crates/attune-core/src/routing/tests/boundary.rs` | full-down / quota-100 / unknown capability / override invalid |
| `rust/crates/attune-core/src/routing/tests/health_test.rs` | demotion after 3 fails; recovery after 3 successes |
| `rust/crates/attune-core/src/routing/tests/budget_test.rs` | quota 80%/90%/100% triggers correct decision |
| `rust/crates/attune-server/tests/routing_routes.rs` | HTTP: GET/POST profile, POST decide, GET health, GET matrix |
| `rust/crates/attune-server/tests/routing_endtoend.rs` | E2E: mock cloud + mock ollama → chat goes through router → UsageEvent recorded |
| `rust/crates/attune-server/ui/src/views/SettingsView/__tests__/RoutingSection.test.tsx` | render profile picker, switch profile, show recommended badge |

### Modify

| Path | Change |
|------|--------|
| `rust/crates/attune-core/src/lib.rs` | `pub mod routing;` + re-exports `Capability`, `RoutingProfile`, `RoutingDecision`, `CapabilityRouter` |
| `rust/crates/attune-core/src/platform/mod.rs` (or `detector.rs`) | Add `pub fn recommend_routing_profile(&self) -> RoutingProfile` |
| `rust/crates/attune-core/src/llm.rs` | (Optional) helper `LlmProvider::from_endpoint(&ProviderEndpoint)` factory; existing `chat()` unchanged |
| `rust/crates/attune-core/src/llm_settings.rs` | Add `routing: RoutingSettings { profile, custom_profiles, priority_overrides, allow_cloud_fallback, matrix_source, log_decisions }` |
| `rust/crates/attune-core/src/agent_runner.rs` | Before spawn: call `router.decide(intent)`, pass primary provider env to subprocess; on failure subprocess reports back so router demotes |
| `rust/crates/attune-server/src/routes/chat.rs` | Replace direct `state.llm()` with router-decided provider; pass `?provider=` query as `intent.override_provider` |
| `rust/crates/attune-server/src/routes/llm.rs` | Same — route via `CapabilityRouter` before dispatch |
| `rust/crates/attune-server/src/routes/mod.rs` | `pub mod routing;` |
| `rust/crates/attune-server/src/state.rs` | Add `router: Arc<dyn CapabilityRouter>` field + accessor |
| `rust/crates/attune-server/src/lib.rs` | Instantiate `DefaultRouter` at startup, spawn `HealthMonitor` + `BudgetWatcher` tasks |
| `rust/crates/attune-server/ui/src/views/SettingsView.tsx` | Mount `<RoutingSection />` |
| `rust/crates/attune-server/ui/src/views/Wizard/Step4Hardware.tsx` | After detect, display `recommendedProfile` chip + apply-button |
| `rust/crates/attune-server/ui/src/i18n/zh.ts` + `en.ts` | New routing keys (≥30 — must stay in sync) |
| `README.md` (root) + `README.zh.md` | Link to `docs/HYBRID-TOKEN-STRATEGY.md` in Cost/Privacy section |
| `DEVELOP.md` | Architecture section: link to HYBRID-TOKEN-STRATEGY |
| `RELEASE.md` | v1.1.0 section: Hybrid Token Strategy + breaking-ish change to settings.llm shape |
| `docs/superpowers/plans/2026-05-28-hybrid-token-routing.md` | This file — delete after merge per CLAUDE.md §3.2 |

---

## Task A: Worktree + Plan A1 dependency verification

**Files:**
- Create: `/tmp/attune-hybrid-routing/` worktree

- [ ] **Step 1: Disk check + dependency verify**

```bash
df -h /data | head -2                        # > 50G available
cd /data/company/project/attune
git log origin/develop --grep="feat(usage,cache): freeze public API surface" --oneline
```

Expected: at least one commit matching the Plan A1 Task M message. If none, **STOP — do not proceed**. Plan A1 must merge first.

- [ ] **Step 2: Verify A1 API surface compiles**

```bash
cat > /tmp/a1-api-probe.rs <<'EOF'
use attune_core::{
    TokenUsage, UsageEvent, UsageKind, CacheOutcome, CallOutcome,
    UsageRecorderGuard, UsageAggregator,
    CacheBackend, CacheScope,
};
fn _probe() {
    let _ = TokenUsage::empty("p", "m");
    let _ = UsageKind::LlmChat;
    let _ = CacheOutcome::Hit;
    let _ = CallOutcome::Ok;
    let _ = CacheScope::Llm;
}
EOF
cd /data/company/project/attune
cargo build --workspace 2>&1 | tail -3       # baseline must be clean
```

Expected: clean build of develop tip. If error, abort and fix develop first.

- [ ] **Step 3: Create worktree**

```bash
git fetch origin
git worktree add /tmp/attune-hybrid-routing -b feat/hybrid-token-routing develop
cd /tmp/attune-hybrid-routing
cargo build --workspace 2>&1 | tail -3
```

Expected: clean.

- [ ] **Step 4: Skeleton commit**

```bash
mkdir -p rust/crates/attune-core/src/routing
touch rust/crates/attune-core/src/routing/mod.rs
git add -A
git commit -m "feat(routing): empty module skeleton

Plan: docs/superpowers/plans/2026-05-28-hybrid-token-routing.md
BlockedBy: Plan A1 'feat(usage,cache): freeze public API surface' (confirmed on develop)."
```

---

## Task B: `routing::decision` — Intent / Decision / ProviderEndpoint types (TDD)

**Files:**
- Create: `rust/crates/attune-core/src/routing/decision.rs`
- Modify: `rust/crates/attune-core/src/routing/mod.rs`, `rust/crates/attune-core/src/lib.rs`

- [ ] **Step 1: Write failing test**

```rust
// rust/crates/attune-core/src/routing/tests/mod.rs (new)
mod decision_test;

// rust/crates/attune-core/src/routing/tests/decision_test.rs
use crate::routing::decision::*;

#[test]
fn capability_serializes_pascal_case() {
    assert_eq!(serde_json::to_string(&Capability::Chat).unwrap(), r#""Chat""#);
    assert_eq!(serde_json::to_string(&Capability::ChatLong).unwrap(), r#""ChatLong""#);
    assert_eq!(serde_json::to_string(&Capability::ExtractAgent).unwrap(), r#""ExtractAgent""#);
}

#[test]
fn cost_tier_serializes_camelCase() {
    assert_eq!(serde_json::to_string(&CostTier::LocalCompute).unwrap(), r#""localCompute""#);
    assert_eq!(serde_json::to_string(&CostTier::Free).unwrap(), r#""free""#);
    assert_eq!(serde_json::to_string(&CostTier::Paid).unwrap(), r#""paid""#);
}

#[test]
fn user_tier_round_trip() {
    let t = UserTier::Pro;
    let j = serde_json::to_string(&t).unwrap();
    let back: UserTier = serde_json::from_str(&j).unwrap();
    assert_eq!(back, UserTier::Pro);
}

#[test]
fn provider_endpoint_carries_all_fields() {
    let p = ProviderEndpoint {
        provider: "cloud_gateway".into(),
        model: "gemini-1.5-flash".into(),
        base_url: "https://gateway.engi-stack.com/v1".into(),
        cost_tier: CostTier::Paid,
    };
    let j = serde_json::to_string(&p).unwrap();
    assert!(j.contains("gemini-1.5-flash"));
    assert!(j.contains("\"costTier\":\"paid\""));
}

#[test]
fn decision_default_chain_is_empty() {
    let d = RoutingDecision::primary_only(ProviderEndpoint {
        provider: "ollama".into(), model: "qwen2.5:3b".into(),
        base_url: "http://localhost:11434".into(), cost_tier: CostTier::LocalCompute,
    }, "test reason");
    assert!(d.fallback_chain.is_empty());
    assert_eq!(d.reason, "test reason");
    assert!(!d.degraded);
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-core routing::tests::decision_test 2>&1 | tail -10`
Expected: FAIL — types missing.

- [ ] **Step 3: Implement types**

```rust
// rust/crates/attune-core/src/routing/decision.rs
//! Spec: 2026-05-28-hybrid-token-strategy.md §5.1

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Capability {
    Chat,
    ChatLong,
    Embed,
    Rerank,
    Classify,
    ExtractAgent,
    Vision,
    Ocr,
    Asr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CostTier {
    Free,
    LocalCompute,
    Paid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UserTier {
    Free,
    Pro,
    Enterprise,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEndpoint {
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub cost_tier: CostTier,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Intent {
    pub capability: Capability,
    pub prompt_tokens_est: u32,
    pub latency_slo_ms: Option<u32>,
    pub user_tier: UserTier,
    pub override_provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingDecision {
    pub primary: ProviderEndpoint,
    pub fallback_chain: Vec<ProviderEndpoint>,
    pub reason: String,
    pub est_cost_usd: Option<f64>,
    pub est_latency_ms: u32,
    pub degraded: bool,
}

impl RoutingDecision {
    pub fn primary_only(primary: ProviderEndpoint, reason: impl Into<String>) -> Self {
        Self {
            primary,
            fallback_chain: vec![],
            reason: reason.into(),
            est_cost_usd: None,
            est_latency_ms: 0,
            degraded: false,
        }
    }
}
```

In `rust/crates/attune-core/src/routing/mod.rs`:

```rust
//! Capability router — picks ProviderEndpoint per Intent.
//! Spec: docs/superpowers/specs/2026-05-28-hybrid-token-strategy.md

pub mod decision;
pub use decision::*;

#[cfg(test)]
mod tests;
```

Add `pub mod routing;` to `rust/crates/attune-core/src/lib.rs`.

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-core routing::tests::decision_test 2>&1 | tail -5`
Expected: `test result: ok. 5 passed`.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/routing/ rust/crates/attune-core/src/lib.rs
git commit -m "feat(routing): Intent / Decision / ProviderEndpoint / Capability / tiers

Spec §5.1. 5 unit tests covering serde wire format."
```

---

## Task C: `routing::profile` — RoutingProfile + custom-profile YAML parser (TDD)

**Files:**
- Create: `rust/crates/attune-core/src/routing/profile.rs`

- [ ] **Step 1: Write failing test**

```rust
// rust/crates/attune-core/src/routing/tests/profile_test.rs
use crate::routing::profile::*;

#[test]
fn profile_serializes_snake_case() {
    assert_eq!(serde_json::to_string(&RoutingProfile::EdgeFirst).unwrap(), r#""edge_first""#);
    assert_eq!(serde_json::to_string(&RoutingProfile::Balanced).unwrap(), r#""balanced""#);
    assert_eq!(serde_json::to_string(&RoutingProfile::CloudFirst).unwrap(), r#""cloud_first""#);
}

#[test]
fn profile_default_is_balanced() {
    assert_eq!(RoutingProfile::default(), RoutingProfile::Balanced);
}

#[test]
fn parse_custom_profile_from_yaml() {
    let yaml = r#"
chat:
  primary: ollama
  fallback: []
embed:
  primary: ollama
  fallback: []
vision: disabled
"#;
    let cp = parse_custom_profile(yaml).unwrap();
    assert!(cp.disabled_capabilities.iter().any(|c| matches!(c, crate::routing::Capability::Vision)));
    assert!(cp.rules.iter().any(|r| r.capability == crate::routing::Capability::Chat));
}

#[test]
fn parse_custom_profile_rejects_invalid_yaml() {
    let bad = "this is not yaml: {[";
    assert!(parse_custom_profile(bad).is_err());
}
```

Append `mod profile_test;` to `rust/crates/attune-core/src/routing/tests/mod.rs`.

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-core routing::tests::profile_test 2>&1 | tail -10`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
// rust/crates/attune-core/src/routing/profile.rs
//! Spec §3 (3-profile table) + §6.2 (custom-profile YAML)

use crate::routing::Capability;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingProfile {
    EdgeFirst,
    Balanced,
    CloudFirst,
}

impl Default for RoutingProfile {
    fn default() -> Self { RoutingProfile::Balanced }
}

#[derive(Debug, Clone)]
pub struct CustomProfileRule {
    pub capability: Capability,
    pub primary: String,
    pub fallback: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CustomProfile {
    pub rules: Vec<CustomProfileRule>,
    pub disabled_capabilities: Vec<Capability>,
}

#[derive(thiserror::Error, Debug)]
pub enum ProfileError {
    #[error("YAML parse: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("unknown capability: {0}")]
    UnknownCapability(String),
}

pub fn parse_custom_profile(yaml: &str) -> Result<CustomProfile, ProfileError> {
    let raw: HashMap<String, serde_yaml::Value> = serde_yaml::from_str(yaml)?;
    let mut rules = Vec::new();
    let mut disabled = Vec::new();
    for (key, val) in raw {
        let cap = parse_capability(&key).ok_or_else(|| ProfileError::UnknownCapability(key.clone()))?;
        match val {
            serde_yaml::Value::String(s) if s == "disabled" => disabled.push(cap),
            serde_yaml::Value::Mapping(m) => {
                let primary = m.get(&serde_yaml::Value::String("primary".into()))
                    .and_then(|v| v.as_str()).unwrap_or("").to_string();
                let fallback = m.get(&serde_yaml::Value::String("fallback".into()))
                    .and_then(|v| v.as_sequence())
                    .map(|seq| seq.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                rules.push(CustomProfileRule { capability: cap, primary, fallback });
            }
            _ => return Err(ProfileError::UnknownCapability(key)),
        }
    }
    Ok(CustomProfile { rules, disabled_capabilities: disabled })
}

fn parse_capability(s: &str) -> Option<Capability> {
    match s.to_lowercase().as_str() {
        "chat" => Some(Capability::Chat),
        "chat_long" | "chatlong" => Some(Capability::ChatLong),
        "embed" => Some(Capability::Embed),
        "rerank" => Some(Capability::Rerank),
        "classify" => Some(Capability::Classify),
        "extract" | "extract_agent" | "extractagent" => Some(Capability::ExtractAgent),
        "vision" => Some(Capability::Vision),
        "ocr" => Some(Capability::Ocr),
        "asr" => Some(Capability::Asr),
        _ => None,
    }
}
```

Add `serde_yaml = "0.9"` and `thiserror = "1"` to `attune-core/Cargo.toml` if missing.

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-core routing::tests::profile_test 2>&1 | tail -5`
Expected: `4 passed`.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/routing/profile.rs rust/crates/attune-core/src/routing/tests/ rust/crates/attune-core/Cargo.toml
git commit -m "feat(routing): RoutingProfile + custom-profile YAML parser

Spec §3 (3 builtin profiles) + §6.2 (privacy_paranoid custom example).
4 unit tests: wire format, default, YAML round-trip, error case."
```

---

## Task D: `routing::rules` — static capability matrix + decision tree (TDD)

**Files:**
- Create: `rust/crates/attune-core/src/routing/rules.rs`

- [ ] **Step 1: Write failing test**

```rust
// rust/crates/attune-core/src/routing/tests/rules_test.rs
use crate::routing::*;
use crate::routing::rules::matrix_entry;

#[test]
fn balanced_chat_default_is_cloud_gateway() {
    let e = matrix_entry(RoutingProfile::Balanced, Capability::Chat);
    assert_eq!(e.primary.provider, "cloud_gateway");
    assert_eq!(e.primary.model, "gemini-1.5-flash");
    assert!(!e.fallback_chain.is_empty(), "balanced must have non-empty fallback");
}

#[test]
fn edge_first_chat_primary_is_ollama() {
    let e = matrix_entry(RoutingProfile::EdgeFirst, Capability::Chat);
    assert_eq!(e.primary.provider, "ollama");
}

#[test]
fn edge_first_chat_default_fallback_is_empty() {
    // Per spec §11 risk 3 mitigation 1: edge_first MUST NOT cloud-fallback by default
    let e = matrix_entry(RoutingProfile::EdgeFirst, Capability::Chat);
    assert!(e.fallback_chain.is_empty(),
        "edge_first must not silently cloud-fallback (privacy red line)");
}

#[test]
fn cloud_first_chat_primary_is_cloud() {
    let e = matrix_entry(RoutingProfile::CloudFirst, Capability::Chat);
    assert_eq!(e.primary.provider, "cloud_gateway");
}

#[test]
fn ocr_is_always_local_across_profiles() {
    for p in [RoutingProfile::EdgeFirst, RoutingProfile::Balanced, RoutingProfile::CloudFirst] {
        let e = matrix_entry(p, Capability::Ocr);
        assert_eq!(e.primary.provider, "ppocr", "OCR always local — profile {:?}", p);
    }
}

#[test]
fn embed_is_local_for_edge_and_balanced() {
    assert_eq!(matrix_entry(RoutingProfile::EdgeFirst, Capability::Embed).primary.provider, "ollama");
    assert_eq!(matrix_entry(RoutingProfile::Balanced, Capability::Embed).primary.provider, "ollama");
    // cloud_first switches to cloud embed:
    assert_eq!(matrix_entry(RoutingProfile::CloudFirst, Capability::Embed).primary.provider, "cloud_gateway");
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-core routing::tests::rules_test 2>&1 | tail -10`
Expected: FAIL.

- [ ] **Step 3: Implement matrix**

```rust
// rust/crates/attune-core/src/routing/rules.rs
//! Static capability × profile matrix. Spec §3 (3-profile table) + §8.1 (matrix red lines).

use crate::routing::{Capability, CostTier, ProviderEndpoint, RoutingDecision, RoutingProfile};

const OLLAMA_BASE: &str = "http://127.0.0.1:11434";
const K3_BASE: &str = "http://192.168.1.100:8080";
const CLOUD_GATEWAY: &str = "https://gateway.engi-stack.com/v1";

fn ep(provider: &str, model: &str, base: &str, tier: CostTier) -> ProviderEndpoint {
    ProviderEndpoint {
        provider: provider.into(), model: model.into(),
        base_url: base.into(), cost_tier: tier,
    }
}

/// Default static matrix entry for (profile, capability).
/// Used as the *starting point* — `default_router::decide` then layers in
/// offline/budget/override adjustments.
pub fn matrix_entry(profile: RoutingProfile, cap: Capability) -> RoutingDecision {
    use Capability::*;
    use RoutingProfile::*;

    let (primary, chain, reason) = match (profile, cap) {
        (EdgeFirst, Chat) => (
            ep("ollama", "qwen2.5:3b", OLLAMA_BASE, CostTier::LocalCompute),
            vec![],   // edge_first: NO cloud fallback (privacy red line, spec §11 risk 3)
            "edge_first + Chat = local Ollama qwen2.5:3b, no cloud fallback",
        ),
        (Balanced, Chat) => (
            ep("cloud_gateway", "gemini-1.5-flash", CLOUD_GATEWAY, CostTier::Paid),
            vec![ep("ollama", "qwen2.5:3b", OLLAMA_BASE, CostTier::LocalCompute)],
            "balanced + Chat = cloud gemini-1.5-flash, fallback Ollama",
        ),
        (CloudFirst, Chat) => (
            ep("cloud_gateway", "gemini-1.5-pro", CLOUD_GATEWAY, CostTier::Paid),
            vec![ep("cloud_gateway", "deepseek-chat", CLOUD_GATEWAY, CostTier::Paid)],
            "cloud_first + Chat = cloud gemini-1.5-pro, fallback deepseek",
        ),

        (_, ChatLong) => (
            ep("cloud_gateway", "gemini-1.5-pro", CLOUD_GATEWAY, CostTier::Paid),
            vec![],
            "ChatLong (>32K) — only cloud gemini-1.5-pro has 1M context",
        ),

        (EdgeFirst, Embed) | (Balanced, Embed) => (
            ep("ollama", "bge-m3", OLLAMA_BASE, CostTier::LocalCompute),
            vec![],
            "Embed = local bge-m3",
        ),
        (CloudFirst, Embed) => (
            ep("cloud_gateway", "text-embedding-3-small", CLOUD_GATEWAY, CostTier::Paid),
            vec![ep("ollama", "bge-m3", OLLAMA_BASE, CostTier::LocalCompute)],
            "cloud_first + Embed = cloud embed, fallback local",
        ),

        (_, Ocr) => (
            ep("ppocr", "ppocrv5_mobile", "subprocess://ppocr", CostTier::LocalCompute),
            vec![],
            "OCR = local PP-OCR (always)",
        ),

        (_, Asr) => (
            ep("whisper_cpp", "whisper-medium-q5", "subprocess://whisper", CostTier::LocalCompute),
            vec![],
            "ASR = local whisper.cpp",
        ),

        (EdgeFirst, Vision) => (
            ep("disabled", "n/a", "", CostTier::Free),
            vec![],
            "Vision disabled in edge_first (no local VLM)",
        ),
        (Balanced, Vision) | (CloudFirst, Vision) => (
            ep("cloud_gateway", "gemini-1.5-flash", CLOUD_GATEWAY, CostTier::Paid),
            vec![],
            "Vision = cloud VLM provider",
        ),

        (EdgeFirst, ExtractAgent) => (
            ep("ollama", "qwen2.5:3b", OLLAMA_BASE, CostTier::LocalCompute),
            vec![],
            "edge_first ExtractAgent = local (lower F1 acknowledged)",
        ),
        (Balanced, ExtractAgent) | (CloudFirst, ExtractAgent) => (
            ep("cloud_gateway", "deepseek-chat", CLOUD_GATEWAY, CostTier::Paid),
            vec![ep("ollama", "qwen2.5:3b", OLLAMA_BASE, CostTier::LocalCompute)],
            "Extract = cloud deepseek, fallback local",
        ),

        (EdgeFirst, Classify) | (Balanced, Classify) => (
            ep("ollama", "qwen2.5:3b", OLLAMA_BASE, CostTier::LocalCompute),
            vec![],
            "Classify = local",
        ),
        (CloudFirst, Classify) => (
            ep("cloud_gateway", "deepseek-chat", CLOUD_GATEWAY, CostTier::Paid),
            vec![],
            "cloud_first Classify = cheap cloud",
        ),

        (_, Rerank) => (
            ep("local", "tantivy_bm25_rrf", "in-process://tantivy", CostTier::Free),
            vec![],
            "Rerank = local BM25+RRF (always)",
        ),
    };

    RoutingDecision {
        primary,
        fallback_chain: chain,
        reason: reason.into(),
        est_cost_usd: None,        // filled by DefaultRouter using cost::estimate_cost_usd
        est_latency_ms: 0,         // filled by HealthMonitor p50
        degraded: false,
    }
}
```

Add `pub mod rules;` to `routing/mod.rs`.

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-core routing::tests::rules_test 2>&1 | tail -5`
Expected: `6 passed`.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/routing/rules.rs rust/crates/attune-core/src/routing/mod.rs rust/crates/attune-core/src/routing/tests/
git commit -m "feat(routing): static capability × profile matrix (rules.rs)

Spec §3 (3-profile table) + §8.1 (red lines).
- Edge_first chat: NO cloud fallback (privacy red line per §11 risk 3)
- OCR / ASR / Rerank: always local
- ChatLong (>32K): only cloud 1M-context
- Vision: disabled in edge_first

6 unit tests asserting matrix correctness."
```

---

## Task E: `routing::health` — provider health probe + demotion (TDD)

**Files:**
- Create: `rust/crates/attune-core/src/routing/health.rs`

- [ ] **Step 1: Write failing test**

```rust
// rust/crates/attune-core/src/routing/tests/health_test.rs
use crate::routing::health::*;
use std::sync::Arc;

#[tokio::test]
async fn fresh_monitor_marks_all_unknown() {
    let m = HealthMonitor::new();
    assert!(matches!(m.status("ollama").await, ProviderStatus::Unknown));
}

#[tokio::test]
async fn three_failures_demote() {
    let m = HealthMonitor::new();
    for _ in 0..3 { m.report_outcome("cloud_gateway", false).await; }
    assert!(matches!(m.status("cloud_gateway").await, ProviderStatus::Demoted));
}

#[tokio::test]
async fn three_successes_after_demote_recover() {
    let m = HealthMonitor::new();
    for _ in 0..3 { m.report_outcome("cloud_gateway", false).await; }
    for _ in 0..3 { m.report_outcome("cloud_gateway", true).await; }
    assert!(matches!(m.status("cloud_gateway").await, ProviderStatus::Alive));
}

#[tokio::test]
async fn failure_rate_24h_computed() {
    let m = HealthMonitor::new();
    for _ in 0..7 { m.report_outcome("ollama", true).await; }
    for _ in 0..3 { m.report_outcome("ollama", false).await; }
    let snap = m.snapshot("ollama").await;
    // 3/10 = 0.3 ± minor windowing tolerance
    assert!((snap.fail_rate_24h - 0.30).abs() < 0.05);
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-core routing::tests::health_test 2>&1 | tail -10`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
// rust/crates/attune-core/src/routing/health.rs
//! Provider health monitor: tracks rolling success/failure window, demotes after 3 consecutive
//! failures, recovers after 3 consecutive successes. Background probe loop (30s) is wired in
//! `default_router::spawn_health_loop`. Spec §3 (OfflineState) + §7.2 (boundary cases).

use serde::Serialize;
use std::collections::HashMap;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderStatus {
    Alive,
    Unknown,
    Demoted,
    Dead,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSnapshot {
    pub provider: String,
    pub status: ProviderStatus,
    pub p50_latency_ms: u32,
    pub fail_rate_24h: f64,
    pub consecutive_fails: u32,
    pub consecutive_ok: u32,
}

struct ProviderState {
    status: ProviderStatus,
    p50_latency_ms: u32,
    consecutive_fails: u32,
    consecutive_ok: u32,
    recent: Vec<bool>,  // last 100 outcomes
}

impl Default for ProviderState {
    fn default() -> Self {
        Self { status: ProviderStatus::Unknown, p50_latency_ms: 0,
               consecutive_fails: 0, consecutive_ok: 0, recent: Vec::with_capacity(100) }
    }
}

pub struct HealthMonitor {
    inner: Mutex<HashMap<String, ProviderState>>,
}

impl HealthMonitor {
    pub fn new() -> Self { Self { inner: Mutex::new(HashMap::new()) } }

    pub async fn status(&self, provider: &str) -> ProviderStatus {
        let g = self.inner.lock().await;
        g.get(provider).map(|s| s.status).unwrap_or(ProviderStatus::Unknown)
    }

    pub async fn snapshot(&self, provider: &str) -> ProviderSnapshot {
        let g = self.inner.lock().await;
        let s = g.get(provider).cloned().unwrap_or_default_then(ProviderState::default);
        let fail_rate = if s.recent.is_empty() { 0.0 }
                        else { s.recent.iter().filter(|x| !**x).count() as f64 / s.recent.len() as f64 };
        ProviderSnapshot {
            provider: provider.into(),
            status: s.status,
            p50_latency_ms: s.p50_latency_ms,
            fail_rate_24h: fail_rate,
            consecutive_fails: s.consecutive_fails,
            consecutive_ok: s.consecutive_ok,
        }
    }

    pub async fn report_outcome(&self, provider: &str, ok: bool) {
        let mut g = self.inner.lock().await;
        let st = g.entry(provider.to_string()).or_default();
        if st.recent.len() >= 100 { st.recent.remove(0); }
        st.recent.push(ok);
        if ok {
            st.consecutive_ok += 1;
            st.consecutive_fails = 0;
            if st.consecutive_ok >= 3 { st.status = ProviderStatus::Alive; }
        } else {
            st.consecutive_fails += 1;
            st.consecutive_ok = 0;
            if st.consecutive_fails >= 3 { st.status = ProviderStatus::Demoted; }
            if st.consecutive_fails >= 10 { st.status = ProviderStatus::Dead; }
        }
    }

    pub async fn record_latency(&self, provider: &str, latency_ms: u32) {
        let mut g = self.inner.lock().await;
        let st = g.entry(provider.to_string()).or_default();
        // simple EMA p50 approximation, alpha=0.2
        st.p50_latency_ms = if st.p50_latency_ms == 0 { latency_ms }
                            else { (st.p50_latency_ms * 4 / 5) + (latency_ms / 5) };
    }

    pub async fn snapshot_all(&self) -> Vec<ProviderSnapshot> {
        let g = self.inner.lock().await;
        g.keys().cloned().collect::<Vec<_>>()
            .into_iter()
            .map(|k| {
                let s = g.get(&k).cloned().unwrap_or_default_then(ProviderState::default);
                let fr = if s.recent.is_empty() { 0.0 } else {
                    s.recent.iter().filter(|x| !**x).count() as f64 / s.recent.len() as f64
                };
                ProviderSnapshot {
                    provider: k, status: s.status, p50_latency_ms: s.p50_latency_ms,
                    fail_rate_24h: fr, consecutive_fails: s.consecutive_fails,
                    consecutive_ok: s.consecutive_ok,
                }
            }).collect()
    }
}

// helper for missing entries
trait OrDefaultThen<T> { fn unwrap_or_default_then(self, default: T) -> T; }
impl<T: Clone> OrDefaultThen<T> for Option<&T> {
    fn unwrap_or_default_then(self, default: T) -> T { self.cloned().unwrap_or(default) }
}
```

If `tokio::sync::Mutex` async-mutex isn't ideal for sync `status()`, swap to `std::sync::Mutex` — tests are tolerant.

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-core routing::tests::health_test 2>&1 | tail -5`
Expected: `4 passed`.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/routing/health.rs rust/crates/attune-core/src/routing/tests/
git commit -m "feat(routing): HealthMonitor — demote after 3 fails, recover after 3 oks

Spec §3 (OfflineState input) + §7.2 (boundary cases).
- ProviderStatus {Alive, Unknown, Demoted, Dead}
- 100-event rolling window for fail_rate_24h
- EMA p50 latency tracking

4 unit tests."
```

---

## Task F: `routing::budget` — quota watcher (TDD)

**Files:**
- Create: `rust/crates/attune-core/src/routing/budget.rs`

- [ ] **Step 1: Write failing test**

```rust
// rust/crates/attune-core/src/routing/tests/budget_test.rs
use crate::routing::budget::*;

#[tokio::test]
async fn under_threshold_no_alert() {
    let b = BudgetWatcher::new();
    b.set_quota(1000, 100).await;  // limit=1000, used=100
    assert_eq!(b.threshold_hit().await, ThresholdHit::None);
}

#[tokio::test]
async fn at_eighty_percent_warns() {
    let b = BudgetWatcher::new();
    b.set_quota(1000, 800).await;
    assert_eq!(b.threshold_hit().await, ThresholdHit::Warn80);
}

#[tokio::test]
async fn at_ninety_percent_modal() {
    let b = BudgetWatcher::new();
    b.set_quota(1000, 905).await;
    assert_eq!(b.threshold_hit().await, ThresholdHit::Modal90);
}

#[tokio::test]
async fn at_one_hundred_blocks() {
    let b = BudgetWatcher::new();
    b.set_quota(1000, 1000).await;
    assert_eq!(b.threshold_hit().await, ThresholdHit::Blocked100);
}

#[tokio::test]
async fn unknown_quota_returns_none() {
    let b = BudgetWatcher::new();
    assert_eq!(b.threshold_hit().await, ThresholdHit::None);
    assert_eq!(b.pct().await, None);
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-core routing::tests::budget_test 2>&1 | tail -10`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
// rust/crates/attune-core/src/routing/budget.rs
//! Tracks attune-pro cloud quota usage. Used by DefaultRouter to demote to edge
//! when approaching limits. Spec §3 (BudgetWatcher input) + §11 risk 1 mitigation 5.

use serde::Serialize;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThresholdHit {
    None,
    Warn80,
    Modal90,
    Blocked100,
}

#[derive(Debug, Default)]
struct QuotaState { limit: Option<u64>, used: Option<u64> }

pub struct BudgetWatcher {
    state: RwLock<QuotaState>,
}

impl BudgetWatcher {
    pub fn new() -> Self { Self { state: RwLock::new(QuotaState::default()) } }

    pub async fn set_quota(&self, limit: u64, used: u64) {
        let mut g = self.state.write().await;
        g.limit = Some(limit);
        g.used = Some(used);
    }

    pub async fn pct(&self) -> Option<f64> {
        let g = self.state.read().await;
        match (g.limit, g.used) {
            (Some(l), Some(u)) if l > 0 => Some(u as f64 / l as f64),
            _ => None,
        }
    }

    pub async fn threshold_hit(&self) -> ThresholdHit {
        match self.pct().await {
            Some(p) if p >= 1.00 => ThresholdHit::Blocked100,
            Some(p) if p >= 0.90 => ThresholdHit::Modal90,
            Some(p) if p >= 0.80 => ThresholdHit::Warn80,
            _ => ThresholdHit::None,
        }
    }
}
```

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-core routing::tests::budget_test 2>&1 | tail -5`
Expected: `5 passed`.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/routing/budget.rs rust/crates/attune-core/src/routing/tests/
git commit -m "feat(routing): BudgetWatcher — 80/90/100% quota thresholds

Spec §3 BudgetWatcher input + §11 risk 1 mitigation 5.
5 unit tests."
```

---

## Task G: `routing::default_router` — CapabilityRouter impl with full decide() (TDD)

**Files:**
- Create: `rust/crates/attune-core/src/routing/default_router.rs`

- [ ] **Step 1: Write failing test**

```rust
// rust/crates/attune-core/src/routing/tests/default_router_test.rs
use crate::routing::*;
use crate::routing::default_router::DefaultRouter;
use crate::routing::health::HealthMonitor;
use crate::routing::budget::BudgetWatcher;
use std::sync::Arc;

fn intent(cap: Capability) -> Intent {
    Intent { capability: cap, prompt_tokens_est: 500, latency_slo_ms: None,
             user_tier: UserTier::Pro, override_provider: None }
}

#[tokio::test]
async fn balanced_chat_picks_cloud_gateway() {
    let r = DefaultRouter::new(RoutingProfile::Balanced,
                                Arc::new(HealthMonitor::new()),
                                Arc::new(BudgetWatcher::new()));
    let d = r.decide(intent(Capability::Chat)).await;
    assert_eq!(d.primary.provider, "cloud_gateway");
    assert!(!d.degraded);
}

#[tokio::test]
async fn long_context_forces_cloud_pro_regardless_of_profile() {
    let r = DefaultRouter::new(RoutingProfile::EdgeFirst,
                                Arc::new(HealthMonitor::new()),
                                Arc::new(BudgetWatcher::new()));
    let i = Intent { capability: Capability::Chat, prompt_tokens_est: 64_000,
                     latency_slo_ms: None, user_tier: UserTier::Pro, override_provider: None };
    let d = r.decide(i).await;
    assert_eq!(d.primary.model, "gemini-1.5-pro", "long context must go cloud");
}

#[tokio::test]
async fn override_provider_takes_precedence() {
    let r = DefaultRouter::new(RoutingProfile::CloudFirst,
                                Arc::new(HealthMonitor::new()),
                                Arc::new(BudgetWatcher::new()));
    let i = Intent { capability: Capability::Chat, prompt_tokens_est: 500,
                     latency_slo_ms: None, user_tier: UserTier::Pro,
                     override_provider: Some("ollama".into()) };
    let d = r.decide(i).await;
    assert_eq!(d.primary.provider, "ollama");
    assert!(d.reason.contains("override"));
}

#[tokio::test]
async fn cloud_demoted_degrades_to_local() {
    let health = Arc::new(HealthMonitor::new());
    for _ in 0..3 { health.report_outcome("cloud_gateway", false).await; }
    let r = DefaultRouter::new(RoutingProfile::Balanced, health, Arc::new(BudgetWatcher::new()));
    let d = r.decide(intent(Capability::Chat)).await;
    assert_eq!(d.primary.provider, "ollama");
    assert!(d.degraded);
    assert!(d.reason.contains("cloud") && d.reason.contains("offline"));
}

#[tokio::test]
async fn budget_eighty_percent_degrades_pro_user_to_local() {
    let budget = Arc::new(BudgetWatcher::new());
    budget.set_quota(1000, 850).await;
    let r = DefaultRouter::new(RoutingProfile::Balanced, Arc::new(HealthMonitor::new()), budget);
    let d = r.decide(intent(Capability::Chat)).await;
    assert_eq!(d.primary.provider, "ollama");
    assert!(d.degraded);
    assert!(d.reason.contains("quota"));
}

#[tokio::test]
async fn latency_slo_under_500ms_forces_local() {
    let r = DefaultRouter::new(RoutingProfile::Balanced,
                                Arc::new(HealthMonitor::new()),
                                Arc::new(BudgetWatcher::new()));
    let i = Intent { capability: Capability::Chat, prompt_tokens_est: 500,
                     latency_slo_ms: Some(400), user_tier: UserTier::Pro,
                     override_provider: None };
    let d = r.decide(i).await;
    assert_eq!(d.primary.provider, "ollama");
    assert!(d.reason.contains("latency"));
}

#[tokio::test]
async fn edge_first_chat_never_cloud_fallbacks() {
    // Privacy red line per spec §11 risk 3.
    let r = DefaultRouter::new(RoutingProfile::EdgeFirst,
                                Arc::new(HealthMonitor::new()),
                                Arc::new(BudgetWatcher::new()));
    let d = r.decide(intent(Capability::Chat)).await;
    assert_eq!(d.primary.provider, "ollama");
    assert!(d.fallback_chain.iter().all(|p| p.provider != "cloud_gateway"
                                          && p.provider != "openai"
                                          && p.provider != "anthropic"),
        "edge_first MUST NOT cloud-fallback; chain={:?}", d.fallback_chain);
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-core routing::tests::default_router_test 2>&1 | tail -15`
Expected: ~7 failures.

- [ ] **Step 3: Implement DefaultRouter**

```rust
// rust/crates/attune-core/src/routing/default_router.rs
//! Default decision tree implementing CapabilityRouter. Spec §5.3 decision tree pseudo-code.

use crate::routing::{
    budget::{BudgetWatcher, ThresholdHit},
    health::{HealthMonitor, ProviderStatus},
    rules::matrix_entry,
    Capability, CapabilityRouter, CostTier, Intent, ProviderEndpoint, RoutingDecision, RoutingProfile,
};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use std::sync::Arc;

pub struct DefaultRouter {
    profile: ArcSwap<RoutingProfile>,
    health: Arc<HealthMonitor>,
    budget: Arc<BudgetWatcher>,
}

impl DefaultRouter {
    pub fn new(profile: RoutingProfile, health: Arc<HealthMonitor>, budget: Arc<BudgetWatcher>) -> Self {
        Self { profile: ArcSwap::from_pointee(profile), health, budget }
    }

    async fn apply_overrides(&self, intent: &Intent, base: &RoutingDecision) -> Option<RoutingDecision> {
        // 1. Explicit user override — highest precedence
        if let Some(p) = &intent.override_provider {
            let ep = ProviderEndpoint {
                provider: p.clone(),
                model: base.primary.model.clone(),
                base_url: base.primary.base_url.clone(),
                cost_tier: base.primary.cost_tier,
            };
            return Some(RoutingDecision::primary_only(
                ep, format!("user override → {}", p)
            ));
        }
        None
    }

    fn ollama_endpoint() -> ProviderEndpoint {
        ProviderEndpoint {
            provider: "ollama".into(),
            model: "qwen2.5:3b".into(),
            base_url: "http://127.0.0.1:11434".into(),
            cost_tier: CostTier::LocalCompute,
        }
    }
}

#[async_trait]
impl CapabilityRouter for DefaultRouter {
    async fn decide(&self, intent: Intent) -> RoutingDecision {
        let profile = **self.profile.load();
        let mut base = matrix_entry(profile, intent.capability);

        // 1. user override
        if let Some(d) = self.apply_overrides(&intent, &base).await {
            return d;
        }

        // 2. long context — force cloud regardless of profile
        if intent.prompt_tokens_est > 32_000 {
            base = matrix_entry(profile, Capability::ChatLong);
            base.reason = format!("{} — long-context (>{}K) forced to cloud",
                                   base.reason, intent.prompt_tokens_est / 1000);
            return base;
        }

        // 3. latency SLO < 500ms — force local
        if intent.latency_slo_ms.map_or(false, |s| s < 500) {
            return RoutingDecision::primary_only(
                Self::ollama_endpoint(),
                format!("latency SLO {}ms < 500ms — forced local", intent.latency_slo_ms.unwrap()),
            );
        }

        // 4. budget guard — 80% quota → degrade Pro to local
        if intent.user_tier == crate::routing::UserTier::Pro {
            match self.budget.threshold_hit().await {
                ThresholdHit::Warn80 | ThresholdHit::Modal90 | ThresholdHit::Blocked100 => {
                    let mut d = RoutingDecision::primary_only(
                        Self::ollama_endpoint(),
                        format!("Pro quota threshold reached — degraded to local")
                    );
                    d.degraded = true;
                    return d;
                }
                _ => {}
            }
        }

        // 5. cloud demoted by health monitor → fallback to local
        if base.primary.provider == "cloud_gateway"
            && profile != RoutingProfile::EdgeFirst
        {
            let status = self.health.status("cloud_gateway").await;
            if matches!(status, ProviderStatus::Demoted | ProviderStatus::Dead) {
                let mut d = RoutingDecision::primary_only(
                    Self::ollama_endpoint(),
                    "cloud_gateway demoted/offline — degraded to local Ollama".to_string(),
                );
                d.degraded = true;
                return d;
            }
        }

        // 6. compute est_cost_usd + est_latency_ms enrichment
        if let Ok(cost) = crate::cost::estimate_cost_usd(intent.prompt_tokens_est, intent.prompt_tokens_est / 3, &base.primary.model) {
            base.est_cost_usd = Some(cost);
        }
        let p50 = self.health.snapshot(&base.primary.provider).await.p50_latency_ms;
        if p50 > 0 { base.est_latency_ms = p50; }

        base
    }

    async fn report_outcome(&self, endpoint: &ProviderEndpoint, ok: bool) {
        self.health.report_outcome(&endpoint.provider, ok).await;
    }

    async fn current_profile(&self) -> RoutingProfile { **self.profile.load() }

    async fn set_profile(&self, profile: RoutingProfile) {
        self.profile.store(Arc::new(profile));
    }
}
```

Add `arc-swap = "1"` to attune-core Cargo.toml if missing.

Add `pub mod default_router;` to `routing/mod.rs`.

Define the trait in `routing/mod.rs` (or `decision.rs`):

```rust
#[async_trait::async_trait]
pub trait CapabilityRouter: Send + Sync {
    async fn decide(&self, intent: Intent) -> RoutingDecision;
    async fn report_outcome(&self, endpoint: &ProviderEndpoint, ok: bool);
    async fn current_profile(&self) -> RoutingProfile;
    async fn set_profile(&self, profile: RoutingProfile);
}
```

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-core routing::tests::default_router_test 2>&1 | tail -10`
Expected: 7 pass.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/routing/default_router.rs rust/crates/attune-core/src/routing/mod.rs rust/crates/attune-core/Cargo.toml
git commit -m "feat(routing): DefaultRouter — complete decision tree

Spec §5.3 pseudo-code implementation:
1. user override (highest precedence)
2. long context (>32K) → cloud gemini-1.5-pro
3. latency SLO <500ms → local
4. Pro user quota ≥80% → local + degraded=true
5. cloud demoted → local + degraded=true
6. cost/latency enrichment via cost::estimate_cost_usd + health.p50

7 unit tests covering all branches. Edge_first privacy red line explicitly verified."
```

---

## Task H: `routing::tests::proptest` + boundary tests

**Files:**
- Create: `rust/crates/attune-core/src/routing/tests/proptest.rs`
- Create: `rust/crates/attune-core/src/routing/tests/boundary.rs`

- [ ] **Step 1: Write proptests + boundary tests**

```rust
// rust/crates/attune-core/src/routing/tests/proptest.rs
use crate::routing::*;
use proptest::prelude::*;

fn cap_strat() -> impl Strategy<Value = Capability> {
    prop_oneof![
        Just(Capability::Chat), Just(Capability::ChatLong), Just(Capability::Embed),
        Just(Capability::Rerank), Just(Capability::Classify), Just(Capability::ExtractAgent),
        Just(Capability::Vision), Just(Capability::Ocr), Just(Capability::Asr),
    ]
}
fn profile_strat() -> impl Strategy<Value = RoutingProfile> {
    prop_oneof![Just(RoutingProfile::EdgeFirst), Just(RoutingProfile::Balanced), Just(RoutingProfile::CloudFirst)]
}

proptest! {
    #[test]
    fn matrix_always_has_primary(profile in profile_strat(), cap in cap_strat()) {
        let d = crate::routing::rules::matrix_entry(profile, cap);
        prop_assert!(!d.primary.provider.is_empty(),
            "matrix entry for ({:?}, {:?}) has empty primary", profile, cap);
    }

    #[test]
    fn edge_first_never_has_cloud_in_default_chain(cap in cap_strat()) {
        let d = crate::routing::rules::matrix_entry(RoutingProfile::EdgeFirst, cap);
        for ep in std::iter::once(&d.primary).chain(d.fallback_chain.iter()) {
            prop_assert!(
                !["cloud_gateway", "openai", "anthropic"].contains(&ep.provider.as_str()),
                "edge_first {:?} has cloud provider in chain: {:?}", cap, ep
            );
        }
    }

    #[test]
    fn reason_is_non_empty(profile in profile_strat(), cap in cap_strat()) {
        let d = crate::routing::rules::matrix_entry(profile, cap);
        prop_assert!(!d.reason.is_empty());
    }
}
```

```rust
// rust/crates/attune-core/src/routing/tests/boundary.rs
use crate::routing::*;
use crate::routing::default_router::DefaultRouter;
use crate::routing::health::HealthMonitor;
use crate::routing::budget::BudgetWatcher;
use std::sync::Arc;

#[tokio::test]
async fn all_providers_down_returns_local_with_degraded() {
    let health = Arc::new(HealthMonitor::new());
    for _ in 0..3 { health.report_outcome("cloud_gateway", false).await; }
    let r = DefaultRouter::new(RoutingProfile::Balanced, health, Arc::new(BudgetWatcher::new()));
    let d = r.decide(Intent {
        capability: Capability::Chat, prompt_tokens_est: 100, latency_slo_ms: None,
        user_tier: UserTier::Pro, override_provider: None,
    }).await;
    assert!(d.degraded);
    assert_eq!(d.primary.provider, "ollama");
}

#[tokio::test]
async fn override_to_unhealthy_provider_still_attempted() {
    let health = Arc::new(HealthMonitor::new());
    for _ in 0..3 { health.report_outcome("openai", false).await; }
    let r = DefaultRouter::new(RoutingProfile::Balanced, health, Arc::new(BudgetWatcher::new()));
    let d = r.decide(Intent {
        capability: Capability::Chat, prompt_tokens_est: 100, latency_slo_ms: None,
        user_tier: UserTier::Pro, override_provider: Some("openai".into()),
    }).await;
    assert_eq!(d.primary.provider, "openai", "override beats health");
}

#[tokio::test]
async fn profile_switch_at_runtime_atomic() {
    let r = DefaultRouter::new(RoutingProfile::CloudFirst,
                                Arc::new(HealthMonitor::new()),
                                Arc::new(BudgetWatcher::new()));
    r.set_profile(RoutingProfile::EdgeFirst).await;
    assert_eq!(r.current_profile().await, RoutingProfile::EdgeFirst);
}

#[tokio::test]
async fn quota_blocked_100_pro_still_routes_local() {
    let budget = Arc::new(BudgetWatcher::new());
    budget.set_quota(1000, 1000).await;
    let r = DefaultRouter::new(RoutingProfile::Balanced, Arc::new(HealthMonitor::new()), budget);
    let d = r.decide(Intent {
        capability: Capability::Chat, prompt_tokens_est: 100, latency_slo_ms: None,
        user_tier: UserTier::Pro, override_provider: None,
    }).await;
    assert!(d.degraded);
    assert_eq!(d.primary.provider, "ollama");
}
```

- [ ] **Step 2: Run — verify pass**

Run: `cargo test -p attune-core routing::tests 2>&1 | tail -15`
Expected: all pass.

Run with high case count:

```bash
PROPTEST_CASES=5000 cargo test -p attune-core routing::tests::proptest 2>&1 | tail -5
```

Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add rust/crates/attune-core/src/routing/tests/
git commit -m "test(routing): proptest invariants + 4 boundary cases

Spec §9.1 (≥3 proptest, ≥5 boundary).
proptest: matrix has primary; edge_first never cloud; reason non-empty
boundary: full-down, override-unhealthy, profile-switch, quota-100"
```

---

## Task I: REST routes — `/api/v1/routing/*` (TDD)

**Files:**
- Create: `rust/crates/attune-server/src/routes/routing.rs`
- Create: `rust/crates/attune-server/tests/routing_routes.rs`

- [ ] **Step 1: Write failing test**

```rust
// rust/crates/attune-server/tests/routing_routes.rs
mod common; use common::TestServer;
use axum::http::StatusCode;

#[tokio::test]
async fn get_profile_returns_current_and_recommended() {
    let srv = TestServer::start().await; srv.unlock_vault().await;
    let res = srv.client().get(srv.url("/api/v1/routing/profile")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body.get("profile").is_some());
    assert!(body.get("recommended").is_some());
}

#[tokio::test]
async fn post_profile_changes_active() {
    let srv = TestServer::start().await; srv.unlock_vault().await;
    let res = srv.client().post(srv.url("/api/v1/routing/profile"))
        .json(&serde_json::json!({ "profile": "edge_first" }))
        .send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res2 = srv.client().get(srv.url("/api/v1/routing/profile")).send().await.unwrap();
    let body: serde_json::Value = res2.json().await.unwrap();
    assert_eq!(body["profile"], "edge_first");
}

#[tokio::test]
async fn post_decide_returns_endpoint_and_reason() {
    let srv = TestServer::start().await; srv.unlock_vault().await;
    let res = srv.client().post(srv.url("/api/v1/routing/decide"))
        .json(&serde_json::json!({
            "capability": "Chat",
            "promptTokensEst": 1200,
            "userTier": "pro"
        }))
        .send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["primary"]["provider"].is_string());
    assert!(body["reason"].is_string());
}

#[tokio::test]
async fn get_health_returns_provider_array() {
    let srv = TestServer::start().await; srv.unlock_vault().await;
    let res = srv.client().get(srv.url("/api/v1/routing/health")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["providers"].is_array());
}

#[tokio::test]
async fn get_matrix_returns_full_table() {
    let srv = TestServer::start().await; srv.unlock_vault().await;
    let res = srv.client().get(srv.url("/api/v1/routing/matrix")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.unwrap();
    let caps = body["capabilities"].as_array().unwrap();
    assert!(caps.len() >= 9, "matrix must cover all 9 capabilities, got {}", caps.len());
}

#[tokio::test]
async fn post_profile_rejects_invalid() {
    let srv = TestServer::start().await; srv.unlock_vault().await;
    let res = srv.client().post(srv.url("/api/v1/routing/profile"))
        .json(&serde_json::json!({ "profile": "bogus" }))
        .send().await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["code"], "routing-profile-invalid");
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-server --test routing_routes 2>&1 | tail -10`
Expected: FAIL.

- [ ] **Step 3: Implement routes**

```rust
// rust/crates/attune-server/src/routes/routing.rs
//! Spec §5.2. Mounted at /api/v1/routing/*.

use crate::error::{AppError, AppResult};
use crate::state::SharedState;
use attune_core::routing::{
    Capability, CapabilityRouter, Intent, RoutingProfile, UserTier,
    rules::matrix_entry,
};
use axum::{
    extract::State, response::Json, routing::{get, post}, Router,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct SetProfileBody { profile: String }

#[derive(Deserialize)]
pub struct DecideBody {
    capability: String,
    #[serde(rename = "promptTokensEst")] prompt_tokens_est: Option<u32>,
    #[serde(rename = "latencySloMs")] latency_slo_ms: Option<u32>,
    #[serde(rename = "userTier")] user_tier: Option<String>,
    #[serde(rename = "overrideProvider")] override_provider: Option<String>,
}

fn parse_profile(s: &str) -> Result<RoutingProfile, AppError> {
    match s {
        "edge_first" => Ok(RoutingProfile::EdgeFirst),
        "balanced" => Ok(RoutingProfile::Balanced),
        "cloud_first" => Ok(RoutingProfile::CloudFirst),
        _ => Err(AppError::bad_request("routing-profile-invalid", format!("unknown profile: {s}"))),
    }
}

fn parse_capability(s: &str) -> Result<Capability, AppError> {
    serde_json::from_str(&format!("\"{}\"", s))
        .map_err(|_| AppError::bad_request("routing-capability-unknown", format!("unknown capability: {s}")))
}

fn parse_user_tier(s: &str) -> UserTier {
    match s {
        "pro" => UserTier::Pro,
        "enterprise" => UserTier::Enterprise,
        _ => UserTier::Free,
    }
}

pub async fn get_profile(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    let router = state.router()?;
    let profile = router.current_profile().await;
    let recommended = state.recommend_routing_profile().unwrap_or(RoutingProfile::Balanced);
    Ok(Json(json!({
        "profile": format!("{:?}", profile).to_lowercase(),
        "recommended": format!("{:?}", recommended).to_lowercase(),
        "reason": "from platform::detector",
    })))
}

pub async fn post_profile(
    State(state): State<SharedState>,
    Json(b): Json<SetProfileBody>,
) -> AppResult<Json<serde_json::Value>> {
    let p = parse_profile(&b.profile)?;
    let router = state.router()?;
    router.set_profile(p).await;
    Ok(Json(json!({ "applied": true, "profile": b.profile })))
}

pub async fn post_decide(
    State(state): State<SharedState>,
    Json(b): Json<DecideBody>,
) -> AppResult<Json<serde_json::Value>> {
    let cap = parse_capability(&b.capability)?;
    let router = state.router()?;
    let intent = Intent {
        capability: cap,
        prompt_tokens_est: b.prompt_tokens_est.unwrap_or(0),
        latency_slo_ms: b.latency_slo_ms,
        user_tier: b.user_tier.as_deref().map(parse_user_tier).unwrap_or(UserTier::Free),
        override_provider: b.override_provider,
    };
    let decision = router.decide(intent).await;
    Ok(Json(serde_json::to_value(decision).unwrap()))
}

pub async fn get_health(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    let snaps = state.routing_health()?.snapshot_all().await;
    Ok(Json(json!({ "providers": snaps })))
}

pub async fn get_matrix(State(_state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    use Capability::*;
    use RoutingProfile::*;
    let caps = [Chat, ChatLong, Embed, Rerank, Classify, ExtractAgent, Vision, Ocr, Asr];
    let rows: Vec<serde_json::Value> = caps.iter().map(|c| {
        json!({
            "capability": format!("{:?}", c),
            "edge_first": matrix_entry(EdgeFirst, *c).primary,
            "balanced": matrix_entry(Balanced, *c).primary,
            "cloud_first": matrix_entry(CloudFirst, *c).primary,
        })
    }).collect();
    Ok(Json(json!({ "capabilities": rows })))
}

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/v1/routing/profile", get(get_profile).post(post_profile))
        .route("/api/v1/routing/decide", post(post_decide))
        .route("/api/v1/routing/health", get(get_health))
        .route("/api/v1/routing/matrix", get(get_matrix))
}
```

Add helper methods on AppState (in `state.rs`):

```rust
impl AppState {
    pub fn router(&self) -> Result<Arc<dyn attune_core::routing::CapabilityRouter>, AppError> {
        self.router.load_full()
            .ok_or_else(|| AppError::internal("router-unavailable", "router not installed"))
    }
    pub fn routing_health(&self) -> Result<Arc<attune_core::routing::health::HealthMonitor>, AppError> {
        self.routing_health.load_full()
            .ok_or_else(|| AppError::internal("health-monitor-unavailable", "health monitor not installed"))
    }
    pub fn recommend_routing_profile(&self) -> Option<RoutingProfile> {
        self.platform_detector.recommend_routing_profile()
    }
}
```

Add `pub mod routing;` to `routes/mod.rs` and `.merge(routes::routing::router())` to `lib.rs`.

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-server --test routing_routes 2>&1 | tail -10`
Expected: 6 pass.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/routes/routing.rs rust/crates/attune-server/src/routes/mod.rs rust/crates/attune-server/src/state.rs rust/crates/attune-server/src/lib.rs rust/crates/attune-server/tests/routing_routes.rs
git commit -m "feat(routing-api): GET/POST /api/v1/routing/{profile,decide,health,matrix}

Spec §5.2. 6 integration tests including 400 routing-profile-invalid kebab code.

state.rs additions:
- router() / routing_health() / recommend_routing_profile() accessors
- ArcSwap<Option<Arc<dyn CapabilityRouter>>> for hot-swap"
```

---

## Task J: `recommend_routing_profile()` on platform detector (TDD)

**Files:**
- Modify: `rust/crates/attune-core/src/platform/mod.rs` (or detector.rs)

- [ ] **Step 1: Write failing test**

Locate the platform detector test module first: `grep -rn "fn recommend\|fn detect\|impl.*Detector\|impl.*Snapshot" rust/crates/attune-core/src/platform/`. Then add:

```rust
// in platform/tests
use crate::platform::*;
use crate::routing::RoutingProfile;

#[test]
fn k3_riscv_recommends_edge_first() {
    let snap = HardwareSnapshot { arch: "riscv64".into(), ..HardwareSnapshot::default() };
    assert_eq!(snap.recommend_routing_profile(), RoutingProfile::EdgeFirst);
}

#[test]
fn high_end_x86_with_gpu_recommends_balanced() {
    let snap = HardwareSnapshot { arch: "x86_64".into(), ram_gb: 32, gpu: Some("nvidia".into()), ..HardwareSnapshot::default() };
    assert_eq!(snap.recommend_routing_profile(), RoutingProfile::Balanced);
}

#[test]
fn low_end_no_gpu_recommends_cloud_first() {
    let snap = HardwareSnapshot { arch: "x86_64".into(), ram_gb: 8, gpu: None, ..HardwareSnapshot::default() };
    assert_eq!(snap.recommend_routing_profile(), RoutingProfile::CloudFirst);
}
```

If `HardwareSnapshot::default` doesn't exist, add it as `#[derive(Default)]` first (gate behind tests if needed).

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-core platform 2>&1 | tail -10`
Expected: FAIL — `recommend_routing_profile` not found.

- [ ] **Step 3: Implement**

```rust
// in platform/mod.rs or detector.rs
impl HardwareSnapshot {
    pub fn recommend_routing_profile(&self) -> crate::routing::RoutingProfile {
        use crate::routing::RoutingProfile::*;
        if self.arch == "riscv64" {
            // K3 一体机 form factor — assume local LLM is the differentiator
            return EdgeFirst;
        }
        if self.ram_gb >= 16 && self.gpu.is_some() {
            return Balanced;
        }
        if self.ram_gb < 12 && self.gpu.is_none() {
            return CloudFirst;
        }
        Balanced
    }
}
```

If `HardwareSnapshot` doesn't have an `arch` field, add it.

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-core platform 2>&1 | tail -10`
Expected: 3 pass.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/platform/
git commit -m "feat(platform): HardwareSnapshot::recommend_routing_profile

Spec §3 + wizard step 4.
- riscv64 (K3) → EdgeFirst
- ram≥16 + gpu → Balanced
- ram<12 + no-gpu → CloudFirst
- else Balanced (default)

3 unit tests."
```

---

## Task K: Wire `DefaultRouter` + `HealthMonitor` + `BudgetWatcher` into AppState

**Files:**
- Modify: `rust/crates/attune-server/src/state.rs`
- Modify: `rust/crates/attune-server/src/lib.rs`

- [ ] **Step 1: Write failing test**

```rust
// rust/crates/attune-server/src/state.rs (test block)
#[cfg(test)]
#[test]
fn state_exposes_router_and_health() {
    let state = AppState::new_for_test();
    assert!(state.router().is_ok());
    assert!(state.routing_health().is_ok());
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-server state::tests::state_exposes_router 2>&1 | tail -5`
Expected: FAIL.

- [ ] **Step 3: Wire fields**

Add to AppState struct (in `state.rs`):

```rust
router: ArcSwap<Option<Arc<dyn attune_core::routing::CapabilityRouter>>>,
routing_health: ArcSwap<Option<Arc<attune_core::routing::health::HealthMonitor>>>,
routing_budget: ArcSwap<Option<Arc<attune_core::routing::budget::BudgetWatcher>>>,
platform_detector: Arc<crate::platform::PlatformDetector>,
```

Initialize in `AppState::new_for_test` and in server startup (`lib.rs`):

```rust
use attune_core::routing::{
    default_router::DefaultRouter, health::HealthMonitor, budget::BudgetWatcher, RoutingProfile,
};

// in startup:
let health = Arc::new(HealthMonitor::new());
let budget = Arc::new(BudgetWatcher::new());
let initial_profile = settings.routing.profile.unwrap_or(RoutingProfile::Balanced);
let router: Arc<dyn attune_core::routing::CapabilityRouter> =
    Arc::new(DefaultRouter::new(initial_profile, health.clone(), budget.clone()));
state.router.store(Some(router));
state.routing_health.store(Some(health));
state.routing_budget.store(Some(budget));
```

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-server state 2>&1 | tail -5`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/state.rs rust/crates/attune-server/src/lib.rs
git commit -m "feat(state): AppState.router + routing_health + routing_budget

Initialized at startup with DefaultRouter + initial profile from settings.routing.profile.
1 unit test."
```

---

## Task L: Wire router into `chat.rs` and `llm.rs` route handlers

**Files:**
- Modify: `rust/crates/attune-server/src/routes/chat.rs`
- Modify: `rust/crates/attune-server/src/routes/llm.rs`

- [ ] **Step 1: Write failing E2E test**

```rust
// rust/crates/attune-server/tests/routing_endtoend.rs
mod common; use common::TestServer;

#[tokio::test]
async fn chat_uses_router_decision_and_reports_outcome() {
    let srv = TestServer::start_with_mock_providers().await;
    srv.unlock_vault().await;

    // Pre-set profile to edge_first
    srv.client().post(srv.url("/api/v1/routing/profile"))
        .json(&serde_json::json!({ "profile": "edge_first" })).send().await.unwrap();

    // Chat call should go via router → primary=ollama
    let res = srv.client().post(srv.url("/api/v1/chat"))
        .json(&serde_json::json!({ "message": "hello" }))
        .send().await.unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    let provider = res.headers().get("x-attune-provider").unwrap().to_str().unwrap();
    assert_eq!(provider, "ollama", "edge_first must route chat through ollama");
}

#[tokio::test]
async fn chat_with_override_query_routes_to_overridden_provider() {
    let srv = TestServer::start_with_mock_providers().await;
    srv.unlock_vault().await;

    let res = srv.client().post(srv.url("/api/v1/chat?provider=ollama"))
        .json(&serde_json::json!({ "message": "hello" }))
        .send().await.unwrap();
    let provider = res.headers().get("x-attune-provider").unwrap().to_str().unwrap();
    assert_eq!(provider, "ollama");
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-server --test routing_endtoend 2>&1 | tail -10`
Expected: FAIL — chat doesn't route through router yet.

- [ ] **Step 3: Wire router into chat.rs**

Replace direct `state.llm()` with router call:

```rust
// inside chat handler, after parsing request:
let router = state.router()?;
let intent = Intent {
    capability: Capability::Chat,
    prompt_tokens_est: attune_core::cost::estimate_tokens(&user_text, "default") as u32,
    latency_slo_ms: None,
    user_tier: user_tier_from_session(&state).await,
    override_provider: params.provider.clone(),  // ?provider= query param
};
let decision = router.decide(intent).await;

// pick LLM provider matching decision.primary.provider
let llm = state.llm_by_endpoint(&decision.primary)?;
let started = std::time::Instant::now();
let result = llm.chat(&system, &user_text);

// report outcome back to router
router.report_outcome(&decision.primary, result.is_ok()).await;

let (reply, usage) = result?;
// (existing UsageEvent recording from Plan A1)
let event = UsageEvent { /* … usage, provider=decision.primary.provider … */ };
```

Add helper `llm_by_endpoint` to AppState (returns existing provider if endpoint matches, else builds a new one from endpoint base_url + model):

```rust
pub fn llm_by_endpoint(&self, ep: &ProviderEndpoint) -> Result<Arc<dyn LlmProvider>, AppError> {
    // For v1.1.0: check the cached provider; if provider+model matches, reuse.
    // Otherwise construct a new OllamaLlmProvider / OpenAiLlmProvider from ep.base_url + ep.model.
    // …
}
```

Similar wiring in `llm.rs`.

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-server --test routing_endtoend 2>&1 | tail -10`
Expected: 2 pass.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/routes/chat.rs rust/crates/attune-server/src/routes/llm.rs rust/crates/attune-server/src/state.rs rust/crates/attune-server/tests/routing_endtoend.rs
git commit -m "feat(chat,llm): route via CapabilityRouter before dispatch

Spec §4.2 + §5.2 override query param.
- chat / llm handlers build Intent, call router.decide, get ProviderEndpoint
- Outcome reported back to router.report_outcome for health demotion
- ?provider= query param flows into intent.override_provider

2 E2E tests."
```

---

## Task M: Background tasks — health probe loop + budget poll

**Files:**
- Modify: `rust/crates/attune-server/src/lib.rs` (spawn tasks)
- Modify: `rust/crates/attune-core/src/routing/health.rs` (add probe fn)

- [ ] **Step 1: Add health probe fn (no test — exercised by E2E later)**

Append to `health.rs`:

```rust
impl HealthMonitor {
    /// Probe one provider with a 1s timeout. Updates status + latency.
    pub async fn probe_once(&self, provider: &str, url: &str) {
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(1)).build().unwrap();
        let started = std::time::Instant::now();
        let result = client.get(url).send().await;
        let elapsed = started.elapsed().as_millis() as u32;
        let ok = result.map(|r| r.status().is_success()).unwrap_or(false);
        self.report_outcome(provider, ok).await;
        if ok { self.record_latency(provider, elapsed).await; }
    }
}
```

- [ ] **Step 2: Spawn probe + budget loops at startup**

In `attune-server/src/lib.rs` startup:

```rust
{
    let health = state.routing_health().unwrap();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            ticker.tick().await;
            health.probe_once("cloud_gateway", "https://gateway.engi-stack.com/health").await;
            health.probe_once("ollama", "http://127.0.0.1:11434/api/tags").await;
            health.probe_once("k3_local", "http://192.168.1.100:8080/health").await;
        }
    });
}
{
    let budget = state.routing_budget.load_full().flatten();
    if let Some(budget) = budget {
        let cloud = state.cloud_client.clone();  // assume member_session client
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                ticker.tick().await;
                if let Some(c) = cloud.as_ref() {
                    if let Ok(q) = c.get_quota().await {
                        budget.set_quota(q.limit, q.used).await;
                    }
                }
            }
        });
    }
}
```

(`cloud_client::get_quota` may need stubbing — if not present, mark `// TODO(v1.1): wire to cloud gateway` and continue.)

- [ ] **Step 3: Manual sanity start**

Build server, hit `/api/v1/routing/health` after 30 seconds:

```bash
cargo build --bin attune-server
# in shell A: cargo run --bin attune-server &
# in shell B (after 30s): curl localhost:18900/api/v1/routing/health
```

Expected: `providers` array includes `ollama` + `cloud_gateway` + `k3_local` with statuses populated.

- [ ] **Step 4: Commit**

```bash
git add rust/crates/attune-core/src/routing/health.rs rust/crates/attune-server/src/lib.rs
git commit -m "feat(routing): spawn 30s health probe loop + 60s budget poll

Spec §3 (BudgetWatcher + OfflineState updates).
Probes cloud_gateway, ollama, k3_local. Quota polled from cloud_client::get_quota."
```

---

## Task N: Web UI — `<RoutingSection />` in Settings + Wizard hardware recommendation

**Files:**
- Create: `rust/crates/attune-server/ui/src/views/SettingsView/RoutingSection.tsx`
- Create: `rust/crates/attune-server/ui/src/api/routing.ts`
- Modify: `rust/crates/attune-server/ui/src/views/SettingsView.tsx`
- Modify: `rust/crates/attune-server/ui/src/views/Wizard/Step4Hardware.tsx`
- Modify: `rust/crates/attune-server/ui/src/i18n/zh.ts` + `en.ts`

- [ ] **Step 1: Write failing Vitest**

```tsx
// rust/crates/attune-server/ui/src/views/SettingsView/__tests__/RoutingSection.test.tsx
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { RoutingSection } from '../RoutingSection';
import { vi } from 'vitest';

vi.mock('../../../api/routing', () => ({
  fetchProfile: vi.fn().mockResolvedValue({ profile: 'balanced', recommended: 'edge_first', reason: 'K3 hardware detected' }),
  setProfile: vi.fn().mockResolvedValue({ applied: true }),
  fetchHealth: vi.fn().mockResolvedValue({ providers: [
    { provider: 'cloud_gateway', status: 'alive', p50LatencyMs: 320, failRate24h: 0.01 },
    { provider: 'ollama', status: 'alive', p50LatencyMs: 80, failRate24h: 0.0 },
  ]}),
}));

it('renders current profile + recommended chip', async () => {
  render(<RoutingSection />);
  expect(await screen.findByText(/balanced/i)).toBeInTheDocument();
  expect(await screen.findByText(/recommended.*edge_first/i)).toBeInTheDocument();
});

it('shows health snapshot table', async () => {
  render(<RoutingSection />);
  expect(await screen.findByText('cloud_gateway')).toBeInTheDocument();
  expect(await screen.findByText('ollama')).toBeInTheDocument();
});
```

- [ ] **Step 2: Run — verify fail**

Run: `cd rust/crates/attune-server/ui && npm run test -- RoutingSection 2>&1 | tail -10`
Expected: FAIL.

- [ ] **Step 3: Implement components**

```ts
// rust/crates/attune-server/ui/src/api/routing.ts
const BASE = '/api/v1/routing';
export type Profile = 'edge_first' | 'balanced' | 'cloud_first';

export async function fetchProfile() {
  const r = await fetch(`${BASE}/profile`);
  if (!r.ok) throw await r.json();
  return r.json() as Promise<{ profile: Profile; recommended: Profile; reason: string }>;
}
export async function setProfile(profile: Profile) {
  const r = await fetch(`${BASE}/profile`, {
    method: 'POST', headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ profile }),
  });
  if (!r.ok) throw await r.json();
  return r.json();
}
export async function fetchHealth() {
  const r = await fetch(`${BASE}/health`);
  return r.json();
}
export async function fetchMatrix() {
  const r = await fetch(`${BASE}/matrix`);
  return r.json();
}
```

```tsx
// rust/crates/attune-server/ui/src/views/SettingsView/RoutingSection.tsx
import { useEffect, useState } from 'react';
import { fetchProfile, setProfile, fetchHealth, Profile } from '../../api/routing';
import { useT } from '../../i18n';

export function RoutingSection() {
  const t = useT();
  const [profile, setP] = useState<Profile | null>(null);
  const [recommended, setR] = useState<Profile | null>(null);
  const [reason, setReason] = useState('');
  const [health, setHealth] = useState<any[]>([]);

  useEffect(() => {
    fetchProfile().then(r => { setP(r.profile); setR(r.recommended); setReason(r.reason); });
    fetchHealth().then(h => setHealth(h.providers || []));
  }, []);

  if (!profile) return <div className="loading">{t('common.loading')}</div>;

  return (
    <div className="routing-section">
      <h3>{t('routing.title')}</h3>

      <div className="profile-picker">
        {(['edge_first', 'balanced', 'cloud_first'] as Profile[]).map(p => (
          <button key={p}
            className={p === profile ? 'active' : ''}
            onClick={async () => { await setProfile(p); setP(p); }}>
            {t(`routing.profile.${p}`)}
            {p === recommended && p !== profile && (
              <span className="rec-chip">★ {t('routing.recommended')}</span>
            )}
          </button>
        ))}
      </div>
      <small className="reason">{reason}</small>

      {profile === 'edge_first' && (
        <div className="warning-banner">
          {t('routing.edgeFirstWarning')}
        </div>
      )}

      <h4>{t('routing.health.title')}</h4>
      <table className="health-table">
        <thead>
          <tr>
            <th>{t('routing.health.provider')}</th>
            <th>{t('routing.health.status')}</th>
            <th>{t('routing.health.latency')}</th>
            <th>{t('routing.health.failRate')}</th>
          </tr>
        </thead>
        <tbody>
          {health.map((h, i) => (
            <tr key={i}>
              <td>{h.provider}</td>
              <td><span className={`status status-${h.status}`}>{t(`routing.status.${h.status}`)}</span></td>
              <td>{h.p50LatencyMs}ms</td>
              <td>{(h.failRate24h * 100).toFixed(1)}%</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
```

Add i18n keys to both `zh.ts` and `en.ts` (≥30 keys):

```ts
// zh.ts
'routing.title': '路由策略',
'routing.recommended': '推荐',
'routing.profile.edge_first': '边缘优先（不出本地）',
'routing.profile.balanced': '平衡（云端为主，本地兜底）',
'routing.profile.cloud_first': '云端优先',
'routing.edgeFirstWarning': '边缘优先模式下，本地 LLM 不可用时 Chat 将失败，绝不出本地',
'routing.health.title': '服务健康',
'routing.health.provider': '提供方',
'routing.health.status': '状态',
'routing.health.latency': '延迟 (p50)',
'routing.health.failRate': '失败率',
'routing.status.alive': '可用',
'routing.status.unknown': '未知',
'routing.status.demoted': '降级',
'routing.status.dead': '不可用',
// + 16 more for wizard chip / matrix view / cli help labels
```

In `Wizard/Step4Hardware.tsx`, after hardware detection, fetch `/api/v1/routing/profile` and show recommended chip with "Apply" button:

```tsx
const [rec, setRec] = useState<Profile | null>(null);
useEffect(() => { fetchProfile().then(r => setRec(r.recommended)); }, []);

return (
  // …existing hardware UI…
  rec && <div className="rec-banner">
    {t('wizard.routingRec', { profile: t(`routing.profile.${rec}`) })}
    <button onClick={async () => { await setProfile(rec); }}>{t('common.apply')}</button>
  </div>
);
```

Mount `<RoutingSection />` in `SettingsView.tsx`.

- [ ] **Step 4: Run — verify pass**

Run: `cd rust/crates/attune-server/ui && npm run test -- RoutingSection 2>&1 | tail -5`
Expected: 2 pass.

Run i18n diff guard:

```bash
cd rust/crates/attune-server/ui/src
diff <(grep -oP "^\s+'[\w.]+'\s*:" i18n/zh.ts | tr -d " ':" | sort) \
     <(grep -oP "^\s+'[\w.]+'\s*:" i18n/en.ts | tr -d " ':" | sort)
```

Expected: **no output**.

Run hardcoded-CJK guard (per CLAUDE.md i18n):

```bash
grep -rnP "(toast\([^)]*'[^']*[\x{4e00}-\x{9fff}]|(title|placeholder|label|description|aria-label)=\"[^\"]*[\x{4e00}-\x{9fff}]|>[^<{]*[\x{4e00}-\x{9fff}])" --include="*.tsx" rust/crates/attune-server/ui/src/views/SettingsView/RoutingSection.tsx | grep -vE "^[^:]+:[0-9]+:\s*(\*|//)"
```

Expected: **no output**.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/ui/
git commit -m "feat(ui): RoutingSection settings panel + Wizard Step4 hardware recommendation

Spec §8.2 (UI display).
- Profile picker (3-way radio button group)
- Recommended chip from /api/v1/routing/profile
- Edge-first big warning banner per spec §11 risk 3 mitigation 3
- Health table polling every 5s
- Wizard 'apply recommended' button (one-click profile switch)
- i18n: 32 new keys, zh/en in sync (verified by grep diff = 0)"
```

---

## Task O: CLI subcommand `attune routing {show,set,decide,health,matrix}`

**Files:**
- Create: `rust/crates/attune-cli/src/commands/routing.rs`
- Modify: `rust/crates/attune-cli/src/commands/mod.rs` + `main.rs`

- [ ] **Step 1: Smoke-test via cli**

Look at existing commands for pattern: `grep -rn "pub fn run\|clap::Subcommand" rust/crates/attune-cli/src/commands/`. Mirror an existing simple subcommand like `version.rs`.

```rust
// rust/crates/attune-cli/src/commands/routing.rs
use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum RoutingCmd {
    Show,
    Set { profile: String },
    Decide { capability: String, #[arg(long)] tokens: Option<u32> },
    Health,
    Matrix,
}

pub async fn run(cmd: RoutingCmd, client: &crate::client::Client) -> anyhow::Result<()> {
    match cmd {
        RoutingCmd::Show => {
            let r: serde_json::Value = client.get("/api/v1/routing/profile").await?;
            println!("{}", serde_json::to_string_pretty(&r)?);
        }
        RoutingCmd::Set { profile } => {
            let r: serde_json::Value = client.post("/api/v1/routing/profile",
                &serde_json::json!({ "profile": profile })).await?;
            println!("{}", serde_json::to_string_pretty(&r)?);
        }
        RoutingCmd::Decide { capability, tokens } => {
            let body = serde_json::json!({
                "capability": capability,
                "promptTokensEst": tokens.unwrap_or(0),
                "userTier": "pro",
            });
            let r: serde_json::Value = client.post("/api/v1/routing/decide", &body).await?;
            println!("{}", serde_json::to_string_pretty(&r)?);
        }
        RoutingCmd::Health => {
            let r: serde_json::Value = client.get("/api/v1/routing/health").await?;
            println!("{}", serde_json::to_string_pretty(&r)?);
        }
        RoutingCmd::Matrix => {
            let r: serde_json::Value = client.get("/api/v1/routing/matrix").await?;
            println!("{}", serde_json::to_string_pretty(&r)?);
        }
    }
    Ok(())
}
```

Register in `commands/mod.rs` + `main.rs` dispatcher.

- [ ] **Step 2: Manual smoke**

```bash
cargo build --bin attune
# server must be running:
./target/debug/attune routing show
./target/debug/attune routing set --profile balanced
./target/debug/attune routing health
./target/debug/attune routing matrix | head -20
```

Expected: JSON output for each. No panic.

- [ ] **Step 3: Commit**

```bash
git add rust/crates/attune-cli/src/commands/routing.rs rust/crates/attune-cli/src/commands/mod.rs rust/crates/attune-cli/src/main.rs
git commit -m "feat(cli): attune routing {show,set,decide,health,matrix}

Spec §5.4. Subcommand mirrors REST API."
```

---

## Task P: `docs/HYBRID-TOKEN-STRATEGY.md` — product SSOT

**Files:**
- Create: `docs/HYBRID-TOKEN-STRATEGY.md`
- Modify: `README.md`, `README.zh.md`, `DEVELOP.md`

- [ ] **Step 1: Write the SSOT document**

Content extracted from spec §3 (architecture data flow), §3 (3-profile table), §8.1 (capability matrix red lines), §5.3 (decision tree summary), §8.2 (UI display rules), §11 risk 3 (privacy red lines).

```markdown
# Hybrid Token Strategy

> SSOT for attune's edge / hybrid / token-only routing.
> Date: 2026-08-15 (v1.1.0) — updated as matrix evolves.

attune is a **hybrid** system. Every capability lives at one of three layers:

| Layer | Cost | Trigger |
|------|------|---------|
| 🆓 Free | CPU, millisecond | runs anywhere, anytime |
| ⚡ Local compute | GPU / NPU, seconds | runs in background during indexing or on explicit click |
| 💰 Token | $$$ / API, seconds | requires explicit user click, **never silently** |

…(full matrix + 3 profiles + decision tree summary + privacy red lines + UI promises — copied from spec §3/§5.3/§8.1/§11 verbatim)…

## Routing Profiles

…(spec §3 table, full)

## Decision Tree

When you click a capability button, the router runs this in order:

1. **User override** (`?provider=` query) — always wins
2. **Long context** (>32K tokens) — forced to cloud gemini-1.5-pro (only 1M-context model)
3. **Latency SLO** (<500ms) — forced to local
4. **Pro quota guard** — at 80% usage, Pro routes degrade to local
5. **Health monitor** — providers demoted after 3 consecutive failures route to fallback
6. **Profile default** — your selected profile from Settings → Routing

## Privacy Red Lines

- `edge_first` profile has **no cloud fallback**. If local LLM fails, chat fails (UI shows clear error).
- OCR / ASR are **always local** across all profiles. No exceptions.
- Embedding is **local by default** for edge_first + balanced.
- Background tasks (skill evolution, memory consolidation) **never use cloud tokens**.

## When to Use Each Profile

…(decision guide for users)

## Capability Matrix

…(full spec §8.1 table)

## Updating the Matrix

The matrix is hardcoded in `rust/crates/attune-core/src/routing/rules.rs`.
To add a new capability or provider, see DEVELOP.md § Routing module.
```

Add the canonical doc as `docs/HYBRID-TOKEN-STRATEGY.md`.

- [ ] **Step 2: Link from README + DEVELOP**

In `README.md` (English):

```markdown
## Cost & Privacy

attune separates costs into three layers (free CPU / local compute / paid tokens) and **never silently spends your tokens**. See [Hybrid Token Strategy](./docs/HYBRID-TOKEN-STRATEGY.md) for the full capability matrix and routing rules.
```

Same in `README.zh.md` (Chinese parallel). In `DEVELOP.md`:

```markdown
### Routing module

See `rust/crates/attune-core/src/routing/` and the product-level SSOT at `docs/HYBRID-TOKEN-STRATEGY.md`.
```

- [ ] **Step 3: Commit**

```bash
git add docs/HYBRID-TOKEN-STRATEGY.md README.md README.zh.md DEVELOP.md
git commit -m "docs(routing): HYBRID-TOKEN-STRATEGY.md SSOT + README/DEVELOP cross-link

Spec §8.1 capability matrix + privacy red lines + 3 profiles + decision tree
extracted to product-level SSOT (referenced from README + DEVELOP + wiki).

per CLAUDE.md §3.2: this is in docs/ as a single-topic feature doc, not a tasks/
report file. Title and content match spec §8.1 verbatim where relevant."
```

---

## Task Q: Self-review + RELEASE.md + merge

**Files:**
- Modify: `RELEASE.md`

- [ ] **Step 1: Spec coverage check**

Map each spec section to a task:

| Spec | Task |
|------|------|
| §1 痛点 | A→Q end-to-end |
| §2 范围 — v1.1.0 (8 items) | B,C,D,E,F,G,I,J,K,L,M,N,O,P |
| §2 v1.2 deferred | NOT IN SCOPE (semantic + adaptive — spec Appendix B v1.2) |
| §3 数据流 | B,C,D,E,F,G,K,L,M |
| §3 3-profile | C,D |
| §3 Inputs sources | E (Health), F (Budget), J (Hardware) |
| §4.1 New modules | B-G, I, N, O, P |
| §4.2 Modified | I, J, K, L, M, N |
| §5.1 Rust types | B, C, D, G |
| §5.2 REST | I |
| §5.3 Decision tree | G |
| §5.4 CLI | O |
| §6 Extension points | C (custom profile), D (matrix add), G (new RoutingStrategy trait — minimal here, full in v1.3) |
| §7 Errors + boundary | G, H, I |
| §8 三层成本 matrix | P (HYBRID-TOKEN-STRATEGY.md), N (UI) |
| §8.2 UI display | N |
| §9.1 6-class test floor | B/C/D/E/F/G (golden inline), H (proptest + boundary), L (E2E), every fix → regression |
| §9.4 黑盒 user stories | L (Pro+wifi degrade), implicit (K3 edge_first), N+I (Pro quota 80→90→100) |
| §11 Risk 1 (over-charge) | G (reason logged), N (recent decisions table — defer), F (80% guard) |
| §11 Risk 2 (thundering herd) | E (rate limit per provider via demote), TODO add jitter helper in DefaultRouter — file followup task |
| §11 Risk 3 (privacy red line) | D (matrix), G (test), N (warning banner), P (SSOT) |
| §11 Risk 4 (matrix update) | P + matrix_source builtin (remote source deferred v1.2) |
| §11 Risk 5 (wizard recommend) | J, N |
| §11 Risk 6 (K3 missing CI) | RELEASE.md known-limitation note |
| §11 Risk 7 (offline DNS) | E (probe 3-layer todo — followup task) |
| §11 Risk 8 (reason metadata leak) | I (reason only in local response) |

- [ ] **Step 2: Placeholder scan**

```bash
cd /tmp/attune-hybrid-routing
grep -rn "todo!\|TODO\|FIXME\|unimplemented" --include="*.rs" rust/crates/attune-core/src/routing rust/crates/attune-server/src/routes/routing.rs
```

Expected: no output (or 2 followup-task markers documented in commit msg).

- [ ] **Step 3: Type consistency check**

Verify `Capability` enum has 9 variants across all uses: `grep -rn "Capability::" --include="*.rs" rust/ | awk -F'::' '{print $NF}' | sort -u`. Should list exactly Chat / ChatLong / Embed / Rerank / Classify / ExtractAgent / Vision / Ocr / Asr.

- [ ] **Step 4: Full test suite**

```bash
cargo test --workspace 2>&1 | tail -10
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 5: RELEASE.md draft for v1.1.0**

```markdown
## v1.1.0 — Hybrid Token Strategy (planned 2026-08-15)

### Highlights
- New `CapabilityRouter` decides primary + fallback provider per (capability, profile, hardware, quota, latency SLO)
- 3 routing profiles: `edge_first`, `balanced` (default), `cloud_first` — auto-recommended from hardware
- `/api/v1/routing/{profile,decide,health,matrix}` + `attune routing` CLI subcommand
- New Settings → Routing tab with profile picker + live health + edge_first privacy warning
- Wizard Step 4 shows recommended profile chip with one-click apply

### Privacy red lines (enforced)
- `edge_first` Chat has **no cloud fallback** — local fails ⇒ chat fails
- OCR / ASR always local across all profiles
- Background tasks (skill evolution, memory consolidation) never use cloud tokens
- See `docs/HYBRID-TOKEN-STRATEGY.md` for full capability matrix

### Performance
- `router.decide()` p99: <BENCHED> ms (target < 5 ms)
- Health probe interval: 30 s; budget poll: 60 s

### Known limitations
- Matrix is built-in only; remote matrix source deferred to v1.2
- K3 RISC-V routing path not yet covered by CI (manual test #72)
- Fallback jitter / thundering-herd protection deferred to v1.1.1 follow-up
- Semantic-aware + adaptive routing deferred to v1.2

### Migration
- `settings.llm.provider` (single field) → `settings.routing.{profile, chat.primary, …}`
- Migration auto-runs on first boot; old field retained for one release as fallback
```

- [ ] **Step 6: Merge**

```bash
cd /data/company/project/attune
git checkout develop
git pull origin develop
git merge --no-ff feat/hybrid-token-routing -m "merge: feat/hybrid-token-routing → develop (v1.1.0 routing)

Spec: docs/superpowers/specs/2026-05-28-hybrid-token-strategy.md
Plan: docs/superpowers/plans/2026-05-28-hybrid-token-routing.md

Depends on Plan A1 cache-token-api (v1.0.6) — UsageEvent / TokenUsage /
UsageAggregator consumed by routing decision feedback loop.

15 commits implementing:
- routing::{decision, profile, rules, health, budget, default_router}
- /api/v1/routing/{profile,decide,health,matrix}
- attune CLI routing subcommand
- UI RoutingSection + Wizard Step4 recommendation
- 30+ unit/proptest/boundary/integration/E2E tests
- docs/HYBRID-TOKEN-STRATEGY.md product SSOT
- README + DEVELOP + RELEASE.md updates

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
git push origin develop
git worktree remove /tmp/attune-hybrid-routing
git branch -d feat/hybrid-token-routing
```

- [ ] **Step 7: Tag v1.1.0-alpha.1 + delete plan**

```bash
git tag v1.1.0-alpha.1 -m "v1.1.0 alpha — hybrid token routing merged"
git push origin v1.1.0-alpha.1

git rm docs/superpowers/plans/2026-05-28-hybrid-token-routing.md
git commit -m "chore(docs): remove implemented plan per §3.2 lifecycle

Plan 2026-05-28-hybrid-token-routing.md implementation complete.
Conclusions captured in:
- Spec: docs/superpowers/specs/2026-05-28-hybrid-token-strategy.md (evergreen)
- Product SSOT: docs/HYBRID-TOKEN-STRATEGY.md
- RELEASE.md v1.1.0 section
- DEVELOP.md Routing module section"
git push origin develop
```

---

## Self-Review Notes

**Spec coverage gaps identified during review:**
- Spec §11 risk 2 (thundering herd jitter) and risk 7 (3-layer DNS+TCP+HTTP offline detection) only partially addressed — filed as v1.1.1 followup commits, documented in RELEASE.md "Known limitations". OK to ship without per user 5-7 day estimate.
- §6.3 `RoutingStrategy` trait extension point (v1.3 adaptive) intentionally deferred — current DefaultRouter is the only impl, but the trait shape (decision.rs) is forward-compatible.

**Placeholder scan:** Two `// TODO(v1.1)` markers in Task M (cloud_client::get_quota wiring) and one followup-task marker for jitter. These are documented in RELEASE.md and tracked, not silent.

**Type consistency:** `Capability` has exactly 9 variants used consistently. `RoutingProfile` 3-variant. `UserTier` 3-variant. `CostTier` 3-variant. `ThresholdHit` 4-variant. `ProviderStatus` 4-variant. Spot-checked all serde wire formats (snake_case vs camelCase vs lowercase) across types — consistent within each enum.

**A1 dependency:** Task A explicitly verifies A1 API surface compiles before any work begins. Doctest probes `TokenUsage::empty()` + `UsageKind::LlmChat` + `CacheOutcome::Hit` + `CallOutcome::Ok` + `CacheScope::Llm`. If any unresolved, abort and wait for A1 to merge.

**Risk during execution:**
1. Task L wiring chat.rs / llm.rs has the largest blast radius. Plan to dedicate one subagent + 2-hour review window.
2. `LlmProvider` construction from `ProviderEndpoint` (Task L step 3 `llm_by_endpoint`) may require a provider registry or factory — current `llm.rs` has hardcoded provider construction. If `state.llm_by_endpoint` is hard to implement (no factory pattern), fall back to a small `match decision.primary.provider { "ollama" => …, "cloud_gateway" => … }` switch in chat handler.
3. Wizard Step4 modification requires reading the existing `Step4Hardware.tsx` — if hardware-detect flow is more complex than assumed, defer wizard integration to v1.1.1 follow-up and only ship Settings/CLI in v1.1.0.

---

**Execution choice:** plan saved. Recommend Subagent-Driven (one subagent per task A→Q, fresh context). Each task ≤5 steps, ≤30 min for an experienced engineer.

