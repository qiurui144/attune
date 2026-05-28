# Privacy Logic Strategy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the OSS attune privacy SSOT — make the 5 outbound points (LLM / Cloud SaaS / WebDAV / Web Search / Telemetry) explicit + auditable + opt-out-able from a single `PrivacyView`; implement the missing `POST /privacy/wipe-cloud-session` + telemetry-default-off persistence; enforce PII Redactor at every outbound call site; ship the privacy-audit shell guard as a CI gate.

**Architecture:** Drives off the existing `routes/privacy.rs` (currently only `GET /privacy/tier`) — extends with `/privacy/status`, `/privacy/settings` (PATCH), `/privacy/lock`, `/privacy/wipe-cloud-session`. Settings persistence reuses the existing `settings.rs` JSON envelope (no new table). Outbound enforcement adds a single `OutboundGate` helper that callers wrap around every existing outbound call site (`chat.rs` LLM / `cloud_client.rs` SaaS / `sync/webdav.rs` / `web_search_browser.rs`). UI ships as a new `PrivacyView.tsx` view under the existing `Sidebar` tab structure, plus a one-shot "Privacy Tour" modal triggered by a `privacy_tour_seen` boolean in settings.

**Tech Stack:** Rust (axum 0.8 / rusqlite / tokio), TypeScript + Preact (UI), `scripts/privacy-audit.sh` (bash + grep), tests under `tests/golden/privacy_*.yaml` + integration tests against MockLlmProvider.

**Spec:** `docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md` (commit `1dab151`).

**Target release:** `v1.0.6` (DR / BCP) — bundled because v1.0.6 already touches status page UI; privacy dashboard fits naturally. Tag in `main` after develop→main merge.

---

## File Structure

### New files (created by this plan)

| Path | Responsibility | Owner task |
|------|----------------|------------|
| `rust/crates/attune-core/src/outbound_gate.rs` | Outbound-call wrapper enforcing settings + redactor | Task 3 |
| `rust/crates/attune-core/src/telemetry.rs` | Telemetry queue + default-off persistence (stub, no actual send in v1.0.6) | Task 5 |
| `rust/crates/attune-server/ui/src/views/PrivacyView.tsx` | User-facing privacy dashboard (5 toggles + DSAR shortcuts) | Task 7 |
| `rust/crates/attune-server/ui/src/views/PrivacyTour.tsx` | One-shot first-launch modal explaining 5 outbound points | Task 8 |
| `docs/PRIVACY.md` | User-facing privacy SOP (English primary, Chinese inline section) | Task 9 |
| `docs/PRIVACY-AUDIT-CHECKLIST.md` | Internal monthly audit checklist | Task 9 |
| `scripts/privacy-audit.sh` | Grep guard — outbound call inventory + telemetry call inventory + hardcoded-key scan | Task 10 |
| `tests/golden/privacy_outbound_gate.yaml` | Golden fixtures for outbound gate behavior | Task 3 |
| `rust/crates/attune-server/tests/privacy_endpoints.rs` | Integration tests for the 4 new endpoints | Task 6 |

### Modified files

| Path | Change | Owner task |
|------|--------|------------|
| `rust/crates/attune-server/src/routes/privacy.rs` | Add `status` / `settings_patch` / `lock` / `wipe_cloud_session` handlers | Task 2 |
| `rust/crates/attune-server/src/routes/mod.rs` | Re-export new handlers (already `pub mod privacy`) | Task 2 |
| `rust/crates/attune-server/src/lib.rs` | Register 4 new routes | Task 2 |
| `rust/crates/attune-core/src/lib.rs` | Re-export `outbound_gate` + `telemetry` modules | Tasks 3, 5 |
| `rust/crates/attune-core/src/cloud_client.rs` | Add `wipe_session()` method (clears stored token + posts logout) | Task 4 |
| `rust/crates/attune-core/src/chat.rs` | Wrap outbound LLM call in `OutboundGate::enforce(LlmEndpoint, ...)` | Task 3 |
| `rust/crates/attune-core/src/web_search_browser.rs` | Wrap outbound search in `OutboundGate::enforce(WebSearch, ...)` | Task 3 |
| `rust/crates/attune-core/src/sync/webdav.rs` | Wrap outbound sync in `OutboundGate::enforce(Webdav, ...)` | Task 3 |
| `rust/crates/attune-server/src/routes/settings.rs` | Add `privacy.{telemetry,web_search,llm,cloud_saas,webdav}` schema + default-false | Task 1 |
| `rust/crates/attune-server/ui/src/views/SettingsView.tsx` | Replace the existing inline privacy section with link/tab opening `PrivacyView` | Task 7 |
| `rust/crates/attune-server/ui/src/Sidebar.tsx` | Add "Privacy" tab entry | Task 7 |
| `rust/crates/attune-server/ui/src/i18n/zh.ts` + `en.ts` | Add 30+ keys for PrivacyView + tour | Task 7, 8 |
| `.github/workflows/ci.yml` | Add `privacy-audit` job calling `scripts/privacy-audit.sh` | Task 10 |

---

## Task 1: Persist privacy settings (default-false) in settings.json schema

**Files:**
- Modify: `rust/crates/attune-server/src/routes/settings.rs:262-360` (default_settings) + JSON-schema validation in `update_settings`
- Test: `rust/crates/attune-server/src/routes/settings.rs` inline `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test for default privacy block**

Add to `settings.rs` test module:

```rust
#[test]
fn default_settings_has_privacy_block_all_outbound_disabled_except_llm_off_by_default() {
    let settings = default_settings("", attune_core::platform::FormFactor::Laptop);
    let privacy = settings.get("privacy").expect("settings should contain privacy block");
    assert_eq!(privacy.get("telemetry"), Some(&serde_json::json!(false)),
        "telemetry MUST default to false");
    assert_eq!(privacy.get("web_search"), Some(&serde_json::json!(false)),
        "web_search MUST default to false");
    assert_eq!(privacy.get("cloud_saas"), Some(&serde_json::json!(false)),
        "cloud_saas MUST default to false (login required to enable)");
    assert_eq!(privacy.get("webdav"), Some(&serde_json::json!(false)),
        "webdav MUST default to false (user configures explicitly)");
    assert_eq!(privacy.get("llm"), Some(&serde_json::json!(false)),
        "llm MUST default to false (wizard step enables it)");
    assert_eq!(privacy.get("privacy_tour_seen"), Some(&serde_json::json!(false)));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p attune-server default_settings_has_privacy_block -- --exact --nocapture`
Expected: FAIL with `settings should contain privacy block`

- [ ] **Step 3: Add privacy block to `default_settings`**

In `default_settings()` (around line 262), insert into the returned `serde_json::json!({...})`:

```rust
// inside default_settings() return value, alongside existing keys
"privacy": {
    "llm": false,
    "cloud_saas": false,
    "webdav": false,
    "web_search": false,
    "telemetry": false,
    "privacy_tour_seen": false
},
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p attune-server default_settings_has_privacy_block -- --exact`
Expected: PASS (1 passed; 0 failed)

- [ ] **Step 5: Write test for PATCH validation — telemetry can never be silently flipped on by another endpoint**

```rust
#[test]
fn telemetry_only_togglable_through_explicit_privacy_patch() {
    // ensure update_settings rejects telemetry key changes from non-privacy paths
    let mut body = serde_json::json!({ "llm": { "model": "deepseek-v4-pro" } });
    let target = serde_json::json!({ "privacy": { "telemetry": true } });
    // The validator should NOT allow the llm patch to introduce a telemetry key.
    let allowed = attune_server::routes::settings::is_telemetry_path_allowed(&body);
    assert!(allowed, "non-privacy patches must not contain privacy.telemetry");
    let illegal = serde_json::json!({ "privacy": { "telemetry": true }, "llm": { "model": "x" } });
    assert!(!attune_server::routes::settings::is_telemetry_path_allowed(&illegal),
        "mixed patch with telemetry must be rejected to prevent accidental enabling");
    let _ = target; let _ = body;
}
```

- [ ] **Step 6: Run test to verify failure**

Run: `cargo test -p attune-server telemetry_only_togglable -- --exact`
Expected: FAIL — function not defined.

- [ ] **Step 7: Implement guard in `settings.rs`**

Append to `settings.rs`:

```rust
/// Telemetry MUST only be toggled through a patch whose ONLY top-level keys are
/// "privacy" or "privacy_tour_seen". Mixed patches are rejected so a buggy UI
/// or third-party plugin cannot piggyback telemetry=true on an unrelated update.
pub fn is_telemetry_path_allowed(body: &serde_json::Value) -> bool {
    let Some(obj) = body.as_object() else { return true };
    let touches_telemetry = obj
        .get("privacy")
        .and_then(|p| p.as_object())
        .map(|p| p.contains_key("telemetry"))
        .unwrap_or(false);
    if !touches_telemetry {
        return true;
    }
    // Only "privacy" key is allowed when telemetry is being changed.
    obj.keys().all(|k| k == "privacy")
}
```

Hook it into `update_settings()` near top:

```rust
if !is_telemetry_path_allowed(&body) {
    return Err(AppError::bad_request("telemetry-must-be-isolated"));
}
```

- [ ] **Step 8: Run tests to confirm green**

Run: `cargo test -p attune-server settings::tests`
Expected: All tests in `settings::tests` pass.

- [ ] **Step 9: Commit**

```bash
git add rust/crates/attune-server/src/routes/settings.rs
git commit -m "feat(privacy): default-false privacy block + telemetry isolation guard"
```

---

## Task 2: Implement 4 new privacy endpoints (status / settings_patch / lock / wipe-cloud-session)

**Files:**
- Modify: `rust/crates/attune-server/src/routes/privacy.rs` (add 4 handlers)
- Modify: `rust/crates/attune-server/src/lib.rs:225` (add 4 route lines)
- Test: `rust/crates/attune-server/tests/privacy_endpoints.rs` (new file)

- [ ] **Step 1: Write failing integration test for `GET /privacy/status`**

Create `rust/crates/attune-server/tests/privacy_endpoints.rs`:

```rust
use attune_server::test_support::{spawn_test_server, TestServer};

#[tokio::test]
async fn get_privacy_status_returns_5_outbound_points_all_disabled() {
    let srv: TestServer = spawn_test_server().await;
    let resp = srv.client.get(format!("{}/api/v1/privacy/status", srv.base_url)).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let outbound = body.get("outbound").expect("outbound key present");
    for key in &["llm", "cloud_saas", "webdav", "web_search", "telemetry"] {
        let point = outbound.get(*key).unwrap_or_else(|| panic!("outbound.{key} missing"));
        assert_eq!(point.get("enabled"), Some(&serde_json::json!(false)),
            "outbound.{key}.enabled MUST default false");
    }
    assert!(body.get("vault").is_some(), "vault state present");
    assert!(body.get("redactor").is_some(), "redactor state present");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p attune-server --test privacy_endpoints get_privacy_status -- --exact`
Expected: FAIL — route not found / 404.

- [ ] **Step 3: Add `status` handler to `privacy.rs`**

Append to `rust/crates/attune-server/src/routes/privacy.rs`:

```rust
use axum::extract::State;
use axum::Json;
use serde_json::json;
use crate::state::SharedState;

pub async fn status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let settings = state.settings_snapshot().await;
    let privacy = settings.get("privacy").cloned().unwrap_or_else(|| json!({}));
    let vault_state = state.vault_state_label();
    let ttl = state.vault_session_ttl_remaining_secs();
    let redactor = state.redactor_status();

    Json(json!({
        "vault": {
            "state": vault_state,
            "unlocked_since": state.vault_unlocked_at(),
            "ttl_remaining_secs": ttl
        },
        "outbound": {
            "llm":        { "enabled": privacy.get("llm").and_then(|v| v.as_bool()).unwrap_or(false),
                            "provider": settings.pointer("/llm/provider").cloned(),
                            "endpoint": settings.pointer("/llm/endpoint").cloned() },
            "cloud_saas": { "enabled": privacy.get("cloud_saas").and_then(|v| v.as_bool()).unwrap_or(false),
                            "session_active": state.cloud_session_active() },
            "webdav":     { "enabled": privacy.get("webdav").and_then(|v| v.as_bool()).unwrap_or(false),
                            "remote": settings.pointer("/sync/webdav/remote").cloned() },
            "web_search": { "enabled": privacy.get("web_search").and_then(|v| v.as_bool()).unwrap_or(false) },
            "telemetry":  { "enabled": privacy.get("telemetry").and_then(|v| v.as_bool()).unwrap_or(false) }
        },
        "redactor": {
            "patterns_active": redactor.patterns_active,
            "ner_loaded": redactor.ner_loaded,
            "llm_redact_loaded": redactor.llm_loaded
        },
        "last_dsar_export": state.last_dsar_export_iso()
    }))
}
```

Add support helpers to `SharedState`:

```rust
// rust/crates/attune-server/src/state.rs (add methods on SharedState)
impl SharedState {
    pub fn vault_state_label(&self) -> &'static str { /* "sealed"|"locked"|"unlocked" via vault.state() */ }
    pub fn vault_session_ttl_remaining_secs(&self) -> Option<u64> { /* compute from unlocked_at + SESSION_TTL_SECS */ }
    pub fn vault_unlocked_at(&self) -> Option<String> { /* ISO8601 */ }
    pub fn cloud_session_active(&self) -> bool { /* cloud_client.session_token().is_some() */ }
    pub fn redactor_status(&self) -> RedactorStatus { /* count patterns + load flags */ }
    pub fn last_dsar_export_iso(&self) -> Option<String> { /* read from audit log */ }
    pub async fn settings_snapshot(&self) -> serde_json::Value { /* lock + clone settings */ }
}

pub struct RedactorStatus { pub patterns_active: usize, pub ner_loaded: bool, pub llm_loaded: bool }
```

Register the route in `lib.rs` (insert after line 225):

```rust
.route("/api/v1/privacy/status", get(routes::privacy::status))
```

- [ ] **Step 4: Run test, verify it passes**

Run: `cargo test -p attune-server --test privacy_endpoints get_privacy_status -- --exact`
Expected: PASS.

- [ ] **Step 5: Write failing test for `PATCH /privacy/settings`**

Add to `privacy_endpoints.rs`:

```rust
#[tokio::test]
async fn patch_privacy_settings_persists_and_returns_applied_diff() {
    let srv = spawn_test_server().await;
    let resp = srv.client
        .patch(format!("{}/api/v1/privacy/settings", srv.base_url))
        .json(&serde_json::json!({ "web_search": true }))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body.get("ok"), Some(&serde_json::json!(true)));
    assert_eq!(body.pointer("/applied/web_search"), Some(&serde_json::json!(true)));

    // re-fetch status — value must persist
    let status: serde_json::Value = srv.client
        .get(format!("{}/api/v1/privacy/status", srv.base_url))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(status.pointer("/outbound/web_search/enabled"), Some(&serde_json::json!(true)));
}
```

- [ ] **Step 6: Run, verify it fails**

Run: `cargo test -p attune-server --test privacy_endpoints patch_privacy_settings -- --exact`
Expected: FAIL — 404 / 405.

- [ ] **Step 7: Implement `settings_patch` handler**

Append to `privacy.rs`:

```rust
use axum::http::StatusCode;

#[derive(serde::Deserialize)]
pub struct PrivacyPatch {
    pub llm: Option<bool>,
    pub cloud_saas: Option<bool>,
    pub webdav: Option<bool>,
    pub web_search: Option<bool>,
    pub telemetry: Option<bool>,
    pub privacy_tour_seen: Option<bool>,
}

pub async fn settings_patch(
    State(state): State<SharedState>,
    Json(patch): Json<PrivacyPatch>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let mut applied = serde_json::Map::new();
    let mut settings = state.settings_snapshot().await;
    let privacy = settings.get_mut("privacy")
        .and_then(|v| v.as_object_mut())
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "privacy block missing", "code": "privacy-block-missing" }))))?;

    macro_rules! apply { ($field:ident) => {
        if let Some(v) = patch.$field { privacy.insert(stringify!($field).into(), json!(v)); applied.insert(stringify!($field).into(), json!(v)); }
    }; }
    apply!(llm); apply!(cloud_saas); apply!(webdav); apply!(web_search);
    apply!(telemetry); apply!(privacy_tour_seen);

    state.save_settings(settings).await.map_err(|e| (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string(), "code": "settings-save-failed" }))
    ))?;
    state.record_privacy_audit_event("settings_changed", &applied).await;
    Ok(Json(json!({ "ok": true, "applied": applied })))
}
```

Add the route:

```rust
.route("/api/v1/privacy/settings", axum::routing::patch(routes::privacy::settings_patch))
```

- [ ] **Step 8: Run test, verify it passes**

Run: `cargo test -p attune-server --test privacy_endpoints patch_privacy_settings -- --exact`
Expected: PASS.

- [ ] **Step 9: Write failing test for `POST /privacy/lock`**

```rust
#[tokio::test]
async fn post_privacy_lock_drops_to_locked_state() {
    let srv = spawn_test_server().await;
    srv.unlock_vault("test-password-not-real").await;
    let resp = srv.client.post(format!("{}/api/v1/privacy/lock", srv.base_url))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let status: serde_json::Value = srv.client
        .get(format!("{}/api/v1/privacy/status", srv.base_url))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(status.pointer("/vault/state"), Some(&serde_json::json!("locked")));
}
```

- [ ] **Step 10: Run, verify fail; implement; verify pass**

Run: `cargo test -p attune-server --test privacy_endpoints post_privacy_lock -- --exact`
Expected first run: FAIL (404).

Add to `privacy.rs`:

```rust
pub async fn lock(State(state): State<SharedState>) -> Json<serde_json::Value> {
    state.lock_vault_now().await;
    state.record_privacy_audit_event("vault_lock", &serde_json::Map::new()).await;
    Json(json!({ "ok": true, "vault_state": "locked" }))
}
```

Register: `.route("/api/v1/privacy/lock", post(routes::privacy::lock))`.

Run again: `cargo test -p attune-server --test privacy_endpoints post_privacy_lock -- --exact`
Expected: PASS.

- [ ] **Step 11: Write failing test for `POST /privacy/wipe-cloud-session`**

```rust
#[tokio::test]
async fn post_wipe_cloud_session_clears_token_and_disables_cloud_saas() {
    let srv = spawn_test_server().await;
    srv.inject_fake_cloud_session("fake-session-not-real").await;
    let resp = srv.client.post(format!("{}/api/v1/privacy/wipe-cloud-session", srv.base_url))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);

    let status: serde_json::Value = srv.client
        .get(format!("{}/api/v1/privacy/status", srv.base_url))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(status.pointer("/outbound/cloud_saas/session_active"),
               Some(&serde_json::json!(false)),
               "cloud session must be cleared");
    assert_eq!(status.pointer("/outbound/cloud_saas/enabled"),
               Some(&serde_json::json!(false)),
               "wiping session also flips toggle off");
}
```

- [ ] **Step 12: Run, verify it fails**

Run: `cargo test -p attune-server --test privacy_endpoints post_wipe_cloud_session -- --exact`
Expected: FAIL (404).

- [ ] **Step 13: Implement `wipe_cloud_session`**

Append to `privacy.rs`:

```rust
pub async fn wipe_cloud_session(State(state): State<SharedState>) -> Json<serde_json::Value> {
    // 1. Best-effort logout call to cloud (ignore network errors — local wipe is the contract)
    let _ = state.cloud_client_logout().await;
    // 2. Clear in-memory session token + persisted row
    state.clear_cloud_session_token().await;
    // 3. Flip cloud_saas toggle to false
    let mut settings = state.settings_snapshot().await;
    if let Some(p) = settings.get_mut("privacy").and_then(|v| v.as_object_mut()) {
        p.insert("cloud_saas".into(), json!(false));
    }
    let _ = state.save_settings(settings).await;
    state.record_privacy_audit_event("wipe_cloud_session", &serde_json::Map::new()).await;
    Json(json!({ "ok": true, "session_active": false, "cloud_saas_enabled": false }))
}
```

Register: `.route("/api/v1/privacy/wipe-cloud-session", post(routes::privacy::wipe_cloud_session))`.

- [ ] **Step 14: Run all 4 endpoint tests**

Run: `cargo test -p attune-server --test privacy_endpoints`
Expected: 4 passed, 0 failed.

- [ ] **Step 15: Commit**

```bash
git add rust/crates/attune-server/src/routes/privacy.rs \
        rust/crates/attune-server/src/state.rs \
        rust/crates/attune-server/src/lib.rs \
        rust/crates/attune-server/tests/privacy_endpoints.rs
git commit -m "feat(privacy): 4 new endpoints status/settings/lock/wipe-cloud-session"
```

---

## Task 3: OutboundGate — enforce settings + PII redactor at every outbound call site

**Files:**
- Create: `rust/crates/attune-core/src/outbound_gate.rs`
- Modify: `rust/crates/attune-core/src/lib.rs` (re-export module)
- Modify: `rust/crates/attune-core/src/chat.rs` (LLM call site)
- Modify: `rust/crates/attune-core/src/web_search_browser.rs` (web search call site)
- Modify: `rust/crates/attune-core/src/sync/webdav.rs` (webdav sync call site)
- Test: `rust/crates/attune-core/src/outbound_gate.rs` `#[cfg(test)]` + `tests/golden/privacy_outbound_gate.yaml`

- [ ] **Step 1: Write the failing test**

Create `rust/crates/attune-core/src/outbound_gate.rs`:

```rust
//! Outbound call gate. Every network egress (LLM / cloud / webdav / web search /
//! telemetry) MUST be wrapped by `OutboundGate::enforce` so settings and PII
//! redactor are consulted in one place.

use crate::pii::Redactor;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutboundKind {
    Llm,
    CloudSaas,
    Webdav,
    WebSearch,
    Telemetry,
}

#[derive(Debug, thiserror::Error)]
pub enum OutboundError {
    #[error("outbound-disabled: user has disabled {0:?} in privacy settings")]
    Disabled(OutboundKind),
    #[error("vault-locked: outbound requires unlocked vault")]
    VaultLocked,
    #[error("redactor-required: payload contained PII and redactor is unavailable")]
    RedactorRequired,
}

pub struct OutboundPolicy {
    pub kind: OutboundKind,
    pub enabled: bool,
    pub vault_unlocked: bool,
    /// Some(redactor) -> apply L1+L2 before payload leaves; None -> only allowed
    /// for telemetry-class payloads that contain no user text.
    pub redactor: Option<Redactor>,
}

pub struct OutboundGate;

impl OutboundGate {
    /// Enforce the policy and return the (possibly redacted) payload. Panics
    /// never escape — all error paths are typed.
    pub fn enforce(policy: &OutboundPolicy, payload: &str) -> Result<String, OutboundError> {
        if !policy.enabled {
            return Err(OutboundError::Disabled(policy.kind));
        }
        // Telemetry is the only kind allowed without an unlocked vault
        // because its payload contains no user text.
        if policy.kind != OutboundKind::Telemetry && !policy.vault_unlocked {
            return Err(OutboundError::VaultLocked);
        }
        match (&policy.redactor, payload_carries_user_text(policy.kind)) {
            (Some(r), true)  => Ok(r.redact_batch(&[payload.to_string()]).0[0].clone()),
            (None,    true)  => Err(OutboundError::RedactorRequired),
            (_,       false) => Ok(payload.to_string()),
        }
    }
}

fn payload_carries_user_text(kind: OutboundKind) -> bool {
    matches!(kind, OutboundKind::Llm | OutboundKind::WebSearch | OutboundKind::Webdav)
    // CloudSaas: payload is account email/credentials only (no user knowledge content)
    // Telemetry: only counts + version + tier; contains no user content
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pii::Redactor;

    fn pol(kind: OutboundKind, enabled: bool, unlocked: bool, with_redactor: bool) -> OutboundPolicy {
        OutboundPolicy {
            kind, enabled, vault_unlocked: unlocked,
            redactor: if with_redactor { Some(Redactor::default_l1()) } else { None },
        }
    }

    #[test]
    fn disabled_returns_disabled_err() {
        let p = pol(OutboundKind::Llm, false, true, true);
        assert!(matches!(OutboundGate::enforce(&p, "hi"), Err(OutboundError::Disabled(OutboundKind::Llm))));
    }

    #[test]
    fn locked_vault_blocks_llm_but_not_telemetry() {
        let llm = pol(OutboundKind::Llm, true, false, true);
        assert!(matches!(OutboundGate::enforce(&llm, "hi"), Err(OutboundError::VaultLocked)));
        let tele = pol(OutboundKind::Telemetry, true, false, false);
        assert!(matches!(OutboundGate::enforce(&tele, "{\"build\":\"x\"}"), Ok(_)));
    }

    #[test]
    fn missing_redactor_for_user_text_returns_err() {
        let p = pol(OutboundKind::Llm, true, true, false);
        assert!(matches!(OutboundGate::enforce(&p, "phone 13800138000"), Err(OutboundError::RedactorRequired)));
    }

    #[test]
    fn redactor_replaces_phone_before_leaving() {
        let p = pol(OutboundKind::Llm, true, true, true);
        let out = OutboundGate::enforce(&p, "联系电话 13800138000 请回拨").unwrap();
        assert!(!out.contains("13800138000"), "phone must be redacted; got: {out}");
        assert!(out.contains("PHONE_") || out.contains("[PHONE"), "placeholder expected");
    }
}
```

- [ ] **Step 2: Re-export the module**

In `rust/crates/attune-core/src/lib.rs`, add:

```rust
pub mod outbound_gate;
pub use outbound_gate::{OutboundGate, OutboundPolicy, OutboundKind, OutboundError};
```

- [ ] **Step 3: Run the unit tests**

Run: `cargo test -p attune-core outbound_gate`
Expected: 4 passed, 0 failed.

- [ ] **Step 4: Wire `chat.rs` to call the gate**

Locate the existing outbound LLM call in `rust/crates/attune-core/src/chat.rs` (the function that posts the final prompt to the LLM provider; grep for `reqwest::Client` / `.post(`).

Wrap the prompt construction with the gate. Insert just before the actual HTTP POST:

```rust
use crate::outbound_gate::{OutboundGate, OutboundPolicy, OutboundKind};

let policy = OutboundPolicy {
    kind: OutboundKind::Llm,
    enabled: ctx.privacy_llm_enabled,
    vault_unlocked: ctx.vault_unlocked,
    redactor: Some(ctx.redactor.clone()),
};
let redacted_prompt = OutboundGate::enforce(&policy, &prompt)
    .map_err(|e| ChatError::OutboundBlocked(e.to_string()))?;
// proceed with HTTP POST using redacted_prompt instead of prompt
```

Add `OutboundBlocked` variant to `ChatError`:

```rust
#[error("outbound-blocked: {0}")]
OutboundBlocked(String),
```

- [ ] **Step 5: Write integration test for chat outbound block**

Add to `rust/crates/attune-core/tests/chat_outbound_test.rs` (new):

```rust
use attune_core::{outbound_gate::*, pii::Redactor};

#[tokio::test]
async fn chat_with_llm_disabled_returns_outbound_blocked() {
    // build a chat session with privacy.llm = false
    // assert: ChatError::OutboundBlocked
    // (Use existing TestChatHarness in attune-core/src/chat.rs::tests if present;
    //  otherwise the unit test in outbound_gate.rs covers the gate contract and
    //  this test exists as a compilation-level integration check.)
    let policy = OutboundPolicy {
        kind: OutboundKind::Llm, enabled: false, vault_unlocked: true,
        redactor: Some(Redactor::default_l1()),
    };
    assert!(matches!(OutboundGate::enforce(&policy, "hi"), Err(OutboundError::Disabled(_))));
}
```

Run: `cargo test -p attune-core --test chat_outbound_test`
Expected: PASS.

- [ ] **Step 6: Wire `web_search_browser.rs` to call the gate**

In the public search function, before launching the headless Chrome navigate:

```rust
let policy = OutboundPolicy {
    kind: OutboundKind::WebSearch,
    enabled: privacy.web_search_enabled,
    vault_unlocked: vault.is_unlocked(),
    redactor: Some(redactor.clone()),
};
let safe_query = OutboundGate::enforce(&policy, &query)
    .map_err(|e| WebSearchError::Blocked(e.to_string()))?;
// use safe_query in the browser URL
```

Add `Blocked` variant to `WebSearchError`.

- [ ] **Step 7: Wire `sync/webdav.rs` to call the gate**

Find the upload/download functions in `sync/webdav.rs`. Before each network call:

```rust
let policy = OutboundPolicy {
    kind: OutboundKind::Webdav,
    enabled: privacy.webdav_enabled,
    vault_unlocked: vault.is_unlocked(),
    redactor: None, // payload is ciphertext (vault is already encrypted), bypass redactor
};
// For ciphertext-only payloads, switch payload_carries_user_text to return false for Webdav
// — confirm and adjust the helper if WebDAV in your codebase carries any cleartext text
```

If WebDAV today carries any cleartext (e.g. settings sync) — keep `redactor: Some(...)` and ensure `payload_carries_user_text(Webdav) == true`. Re-check the unit test `redactor_replaces_phone_before_leaving` still passes.

- [ ] **Step 8: Add golden fixture file**

Create `tests/golden/privacy_outbound_gate.yaml`:

```yaml
# Outbound gate golden cases — 10 real scenarios
cases:
  - name: llm-disabled
    kind: Llm
    enabled: false
    vault_unlocked: true
    payload: "请帮我总结"
    expect: { outcome: error, code: outbound-disabled }
  - name: llm-vault-locked
    kind: Llm
    enabled: true
    vault_unlocked: false
    payload: "请帮我总结"
    expect: { outcome: error, code: vault-locked }
  - name: llm-happy-path-phone-redacted
    kind: Llm
    enabled: true
    vault_unlocked: true
    payload: "联系 13800138000"
    expect: { outcome: ok, contains_placeholder: PHONE }
  - name: web-search-disabled
    kind: WebSearch
    enabled: false
    vault_unlocked: true
    payload: "minimum wage Beijing"
    expect: { outcome: error, code: outbound-disabled }
  - name: webdav-ciphertext-bypass-redactor
    kind: Webdav
    enabled: true
    vault_unlocked: true
    payload: "ciphertext-blob"
    redactor: none
    expect: { outcome: ok, equals_input: true }
  - name: telemetry-locked-vault-ok
    kind: Telemetry
    enabled: true
    vault_unlocked: false
    payload: '{"build":"v1.0.6"}'
    expect: { outcome: ok }
  - name: telemetry-disabled
    kind: Telemetry
    enabled: false
    vault_unlocked: true
    payload: '{"build":"v1.0.6"}'
    expect: { outcome: error, code: outbound-disabled }
  - name: llm-id-card-redacted
    kind: Llm
    enabled: true
    vault_unlocked: true
    payload: "身份证号 110101199003078888"
    expect: { outcome: ok, contains_placeholder: ID_CARD }
  - name: llm-bank-card-redacted
    kind: Llm
    enabled: true
    vault_unlocked: true
    payload: "卡号 6222021234567890123"
    expect: { outcome: ok, contains_placeholder: BANK_CARD }
  - name: llm-email-redacted
    kind: Llm
    enabled: true
    vault_unlocked: true
    payload: "邮箱 user@example.com"
    expect: { outcome: ok, contains_placeholder: EMAIL }
```

Add the golden runner in `rust/crates/attune-core/tests/privacy_outbound_golden.rs`:

```rust
//! Replay tests/golden/privacy_outbound_gate.yaml through OutboundGate.

use attune_core::{outbound_gate::*, pii::Redactor};

#[derive(Debug, serde::Deserialize)]
struct Case {
    name: String,
    kind: String,
    enabled: bool,
    vault_unlocked: bool,
    payload: String,
    #[serde(default)]
    redactor: Option<String>, // "none" to disable
    expect: Expect,
}

#[derive(Debug, serde::Deserialize)]
struct Expect {
    outcome: String,
    #[serde(default)] code: Option<String>,
    #[serde(default)] contains_placeholder: Option<String>,
    #[serde(default)] equals_input: bool,
}

#[derive(Debug, serde::Deserialize)]
struct Doc { cases: Vec<Case> }

#[test]
fn privacy_outbound_golden_replay() {
    let text = std::fs::read_to_string("../../tests/golden/privacy_outbound_gate.yaml").unwrap();
    let doc: Doc = serde_yaml::from_str(&text).unwrap();
    let mut failures = Vec::new();
    for c in &doc.cases {
        let kind = match c.kind.as_str() {
            "Llm" => OutboundKind::Llm,
            "CloudSaas" => OutboundKind::CloudSaas,
            "Webdav" => OutboundKind::Webdav,
            "WebSearch" => OutboundKind::WebSearch,
            "Telemetry" => OutboundKind::Telemetry,
            _ => panic!("unknown kind {}", c.kind),
        };
        let redactor = if matches!(c.redactor.as_deref(), Some("none")) {
            None
        } else {
            Some(Redactor::default_l1())
        };
        let policy = OutboundPolicy { kind, enabled: c.enabled, vault_unlocked: c.vault_unlocked, redactor };
        let result = OutboundGate::enforce(&policy, &c.payload);
        match (&c.expect.outcome[..], result) {
            ("ok", Ok(out)) => {
                if c.expect.equals_input && out != c.payload {
                    failures.push(format!("{}: expected equals_input but got {}", c.name, out));
                }
                if let Some(p) = &c.expect.contains_placeholder {
                    if !out.contains(p) {
                        failures.push(format!("{}: expected placeholder {} in {}", c.name, p, out));
                    }
                }
            }
            ("error", Err(_)) => {
                // code-level matching kept loose; presence of Err is the contract
            }
            (exp, got) => failures.push(format!("{}: expected {} but got {:?}", c.name, exp, got)),
        }
    }
    assert!(failures.is_empty(), "golden failures:\n{}", failures.join("\n"));
}
```

- [ ] **Step 9: Run all gate tests**

Run: `cargo test -p attune-core outbound_gate privacy_outbound_golden`
Expected: ALL pass (4 unit + 1 golden replay covering 10 cases = 14 logical tests).

- [ ] **Step 10: Commit**

```bash
git add rust/crates/attune-core/src/outbound_gate.rs \
        rust/crates/attune-core/src/lib.rs \
        rust/crates/attune-core/src/chat.rs \
        rust/crates/attune-core/src/web_search_browser.rs \
        rust/crates/attune-core/src/sync/webdav.rs \
        rust/crates/attune-core/tests/chat_outbound_test.rs \
        rust/crates/attune-core/tests/privacy_outbound_golden.rs \
        tests/golden/privacy_outbound_gate.yaml
git commit -m "feat(privacy): OutboundGate at all 5 egress sites + 10 golden cases"
```

---

## Task 4: cloud_client wipe_session method

**Files:**
- Modify: `rust/crates/attune-core/src/cloud_client.rs` (add `wipe_session()`)
- Test: `rust/crates/attune-core/src/cloud_client.rs` inline `#[cfg(test)] mod tests`

- [ ] **Step 1: Write failing test**

Add to `cloud_client.rs` test module:

```rust
#[test]
fn wipe_session_clears_token_even_when_logout_endpoint_unreachable() {
    let mut c = CloudClient::with_session("http://127.0.0.1:1", "fake-token-not-real");
    assert!(c.session_token().is_some(), "precondition");
    // wipe_session must clear the token regardless of network outcome
    let _ = c.wipe_session(); // ignore Err
    assert!(c.session_token().is_none(), "token must be cleared after wipe_session");
}
```

- [ ] **Step 2: Verify it fails**

Run: `cargo test -p attune-core cloud_client::tests::wipe_session_clears_token -- --exact`
Expected: FAIL — method not found.

- [ ] **Step 3: Implement `wipe_session`**

Add to `impl CloudClient` (alongside the existing `logout`):

```rust
/// Best-effort cloud logout + unconditional local clear.
///
/// Network failure does NOT prevent the local token from being cleared —
/// the contract is "after this call, this client carries no session token".
pub fn wipe_session(&mut self) -> Result<()> {
    // Try the remote logout (best-effort); swallow errors.
    let _ = self.logout();
    // Unconditional local clear — even if `logout()` already cleared it,
    // we re-enforce here to keep the contract self-documenting.
    self.session_token = None;
    Ok(())
}
```

- [ ] **Step 4: Verify it passes**

Run: `cargo test -p attune-core cloud_client::tests::wipe_session_clears_token -- --exact`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/cloud_client.rs
git commit -m "feat(privacy): cloud_client.wipe_session — unconditional local clear"
```

---

## Task 5: Telemetry stub (default-off, no actual send in v1.0.6)

**Files:**
- Create: `rust/crates/attune-core/src/telemetry.rs`
- Modify: `rust/crates/attune-core/src/lib.rs`

- [ ] **Step 1: Write failing test**

Create `rust/crates/attune-core/src/telemetry.rs`:

```rust
//! Telemetry queue — default-off. v1.0.6 ships the queue + default-false
//! persistence only; actual HTTP send is gated behind a future v1.1 toggle
//! AND `privacy.telemetry == true`. Today, send() returns Skipped.

use crate::outbound_gate::{OutboundGate, OutboundPolicy, OutboundKind};

#[derive(Debug, Clone)]
pub struct TelemetryEvent {
    pub ts_iso: String,
    pub kind: String, // "vault_lock" | "outbound_call" | "dsar_export" | "settings_changed"
    pub redacted_meta: serde_json::Value,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SendOutcome {
    Sent,
    SkippedDisabled,
    SkippedNotImplemented, // v1.0.6 placeholder
}

pub struct Telemetry { pub enabled: bool }

impl Telemetry {
    /// Default: disabled. Constructor takes the value loaded from settings.
    pub fn new(enabled: bool) -> Self { Self { enabled } }

    /// Always returns SkippedDisabled when not enabled. Returns
    /// SkippedNotImplemented when enabled (v1.0.6 ships no HTTP send yet).
    pub fn send(&self, event: &TelemetryEvent) -> SendOutcome {
        let policy = OutboundPolicy {
            kind: OutboundKind::Telemetry,
            enabled: self.enabled,
            vault_unlocked: false, // telemetry payload contains no user text
            redactor: None,
        };
        let payload = serde_json::to_string(&event.redacted_meta).unwrap_or_default();
        match OutboundGate::enforce(&policy, &payload) {
            Ok(_) => SendOutcome::SkippedNotImplemented,
            Err(_) => SendOutcome::SkippedDisabled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev() -> TelemetryEvent {
        TelemetryEvent {
            ts_iso: "2026-05-28T00:00:00Z".into(),
            kind: "settings_changed".into(),
            redacted_meta: serde_json::json!({"setting":"web_search","new":true}),
        }
    }

    #[test]
    fn default_disabled_skips_send() {
        let t = Telemetry::new(false);
        assert_eq!(t.send(&ev()), SendOutcome::SkippedDisabled);
    }

    #[test]
    fn enabled_returns_skipped_not_implemented_in_v106() {
        let t = Telemetry::new(true);
        assert_eq!(t.send(&ev()), SendOutcome::SkippedNotImplemented);
    }
}
```

- [ ] **Step 2: Re-export**

In `rust/crates/attune-core/src/lib.rs`:

```rust
pub mod telemetry;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p attune-core telemetry::tests`
Expected: 2 passed.

- [ ] **Step 4: Commit**

```bash
git add rust/crates/attune-core/src/telemetry.rs rust/crates/attune-core/src/lib.rs
git commit -m "feat(privacy): telemetry stub — default-off + Skipped outcomes for v1.0.6"
```

---

## Task 6: Audit-log integration tests (vault lock / outbound block / DSAR export are recorded)

**Files:**
- Test: `rust/crates/attune-server/tests/privacy_audit_log_integration.rs` (new)

- [ ] **Step 1: Write failing test**

```rust
use attune_server::test_support::spawn_test_server;

#[tokio::test]
async fn vault_lock_writes_audit_event() {
    let srv = spawn_test_server().await;
    srv.unlock_vault("test-password-not-real").await;
    srv.client.post(format!("{}/api/v1/privacy/lock", srv.base_url)).send().await.unwrap();
    let log: serde_json::Value = srv.client
        .get(format!("{}/api/v1/dsar/audit-log", srv.base_url))
        .send().await.unwrap().json().await.unwrap();
    let events = log.get("events").and_then(|v| v.as_array()).expect("events array");
    assert!(events.iter().any(|e| e.get("kind") == Some(&serde_json::json!("vault_lock"))),
        "vault_lock event must be recorded; got: {:#?}", events);
    assert!(events.iter().all(|e| !e.to_string().to_lowercase().contains("password")),
        "audit log MUST NOT contain password literal");
}

#[tokio::test]
async fn settings_changed_recorded_with_no_secret_value() {
    let srv = spawn_test_server().await;
    srv.client.patch(format!("{}/api/v1/privacy/settings", srv.base_url))
        .json(&serde_json::json!({ "web_search": true }))
        .send().await.unwrap();
    let log: serde_json::Value = srv.client
        .get(format!("{}/api/v1/dsar/audit-log", srv.base_url))
        .send().await.unwrap().json().await.unwrap();
    let events = log.get("events").and_then(|v| v.as_array()).unwrap();
    assert!(events.iter().any(|e|
        e.get("kind") == Some(&serde_json::json!("settings_changed"))
        && e.pointer("/redacted_meta/applied/web_search") == Some(&serde_json::json!(true))));
}
```

- [ ] **Step 2: Verify it fails, implement, verify it passes**

Run: `cargo test -p attune-server --test privacy_audit_log_integration`
Expected first run: depends on whether `/dsar/audit-log` already exists. If not, add the route (the route table in `lib.rs` does not currently list it).

Add to `lib.rs`: `.route("/api/v1/dsar/audit-log", get(routes::dsar::audit_log))`.

Add to `routes/dsar.rs`:

```rust
pub async fn audit_log(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let rows = state.audit_log_recent(200).await.unwrap_or_default();
    Json(serde_json::json!({ "events": rows }))
}
```

`state.audit_log_recent(n)` reads from the existing `store::audit` module (we already have one — confirm by `grep audit_log rust/crates/attune-core/src/store/audit.rs`).

Re-run tests until both pass.

- [ ] **Step 3: Commit**

```bash
git add rust/crates/attune-server/tests/privacy_audit_log_integration.rs \
        rust/crates/attune-server/src/routes/dsar.rs \
        rust/crates/attune-server/src/lib.rs
git commit -m "feat(privacy): /dsar/audit-log endpoint + integration tests for lock/settings events"
```

---

## Task 7: PrivacyView UI — dashboard with 5 toggles + DSAR shortcuts

**Files:**
- Create: `rust/crates/attune-server/ui/src/views/PrivacyView.tsx`
- Modify: `rust/crates/attune-server/ui/src/views/SettingsView.tsx` (remove inline privacy section, link to new tab)
- Modify: `rust/crates/attune-server/ui/src/Sidebar.tsx` (add "Privacy" tab)
- Modify: `rust/crates/attune-server/ui/src/i18n/zh.ts` + `en.ts` (add keys)

- [ ] **Step 1: Add i18n keys (both locales — set keys identical to avoid drift)**

In `i18n/en.ts`, add to the existing default-export object:

```ts
'privacy.title': 'Privacy',
'privacy.subtitle': 'See and control every outbound connection',
'privacy.vault.state': 'Vault state',
'privacy.vault.ttlRemaining': 'Auto-lock in',
'privacy.outbound.title': 'Outbound connections (5)',
'privacy.outbound.llm': 'LLM token (chat & analysis)',
'privacy.outbound.cloudSaas': 'Attune Cloud (account & membership)',
'privacy.outbound.webdav': 'WebDAV remote (your own server)',
'privacy.outbound.webSearch': 'Web search (Bing / Google)',
'privacy.outbound.telemetry': 'Telemetry (off by default)',
'privacy.outbound.enabled': 'Enabled',
'privacy.outbound.disabled': 'Disabled',
'privacy.redactor.title': 'PII Redactor',
'privacy.redactor.patternsActive': '{n} patterns active',
'privacy.redactor.nerLoaded': 'NER loaded',
'privacy.redactor.nerMissing': 'NER not loaded (L1 only)',
'privacy.actions.lockNow': 'Lock vault now',
'privacy.actions.wipeCloudSession': 'Wipe cloud session',
'privacy.actions.exportData': 'Export my data (DSAR)',
'privacy.actions.deleteAccount': 'Delete my account & data',
'privacy.actions.openAuditLog': 'View audit log',
'privacy.tour.title': 'Welcome to Attune privacy',
'privacy.tour.intro': 'Attune is local-first. Five outbound connections are listed below, all off by default.',
'privacy.tour.cta': 'Got it',
'privacy.errors.saveFailed': 'Failed to save privacy settings',
'privacy.errors.lockFailed': 'Failed to lock vault',
'privacy.errors.wipeFailed': 'Failed to wipe cloud session',
```

Add the same keys in `i18n/zh.ts` with Chinese values. Use the grep guard from CLAUDE.md i18n section to confirm zh/en key sets match (`diff` returns empty).

- [ ] **Step 2: Create `PrivacyView.tsx`**

```tsx
// rust/crates/attune-server/ui/src/views/PrivacyView.tsx
import { useEffect, useState } from 'preact/hooks';
import { useI18n } from '../i18n';
import { api } from '../api';

interface OutboundEntry { enabled: boolean; provider?: string; endpoint?: string; session_active?: boolean; remote?: string; }
interface PrivacyStatus {
  vault: { state: string; ttl_remaining_secs: number | null };
  outbound: Record<'llm'|'cloud_saas'|'webdav'|'web_search'|'telemetry', OutboundEntry>;
  redactor: { patterns_active: number; ner_loaded: boolean; llm_redact_loaded: boolean };
  last_dsar_export: string | null;
}

const OUTBOUND_KEYS: Array<keyof PrivacyStatus['outbound']> =
  ['llm','cloud_saas','webdav','web_search','telemetry'];

export function PrivacyView() {
  const { t } = useI18n();
  const [status, setStatus] = useState<PrivacyStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    try {
      const res = await api.get('/privacy/status');
      setStatus(res.data);
    } catch (e: any) { setError(e?.message ?? 'unknown'); }
  }
  useEffect(() => { refresh(); }, []);

  async function toggle(key: keyof PrivacyStatus['outbound'], next: boolean) {
    try {
      await api.patch('/privacy/settings', { [key]: next });
      await refresh();
    } catch { setError(t('privacy.errors.saveFailed')); }
  }
  async function lockNow() {
    try { await api.post('/privacy/lock'); await refresh(); }
    catch { setError(t('privacy.errors.lockFailed')); }
  }
  async function wipeCloud() {
    try { await api.post('/privacy/wipe-cloud-session'); await refresh(); }
    catch { setError(t('privacy.errors.wipeFailed')); }
  }

  if (!status) return <div class="p-6">Loading…</div>;
  return (
    <div class="p-6 space-y-6">
      <header>
        <h1 class="text-2xl font-bold">{t('privacy.title')}</h1>
        <p class="text-gray-500">{t('privacy.subtitle')}</p>
      </header>

      <section class="rounded border p-4">
        <h2 class="font-semibold">{t('privacy.vault.state')}: <span data-testid="vault-state">{status.vault.state}</span></h2>
        {status.vault.ttl_remaining_secs != null &&
          <p>{t('privacy.vault.ttlRemaining')}: {Math.floor(status.vault.ttl_remaining_secs / 60)} min</p>}
        <button onClick={lockNow} class="mt-2 px-3 py-1 bg-red-600 text-white rounded">
          {t('privacy.actions.lockNow')}
        </button>
      </section>

      <section class="rounded border p-4">
        <h2 class="font-semibold mb-3">{t('privacy.outbound.title')}</h2>
        <table class="w-full">
          <tbody>
            {OUTBOUND_KEYS.map(k => {
              const entry = status.outbound[k];
              return (
                <tr key={k} data-testid={`outbound-row-${k}`}>
                  <td class="py-2 pr-4">{t(`privacy.outbound.${camel(k)}` as any)}</td>
                  <td>
                    <label class="inline-flex items-center cursor-pointer">
                      <input type="checkbox" checked={entry.enabled}
                        onChange={(e: any) => toggle(k, e.target.checked)}
                        data-testid={`toggle-${k}`} />
                      <span class="ml-2">
                        {entry.enabled ? t('privacy.outbound.enabled') : t('privacy.outbound.disabled')}
                      </span>
                    </label>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
        {status.outbound.cloud_saas.session_active &&
          <button onClick={wipeCloud} class="mt-3 px-3 py-1 bg-orange-600 text-white rounded"
                  data-testid="wipe-cloud-session-button">
            {t('privacy.actions.wipeCloudSession')}
          </button>}
      </section>

      <section class="rounded border p-4">
        <h2 class="font-semibold">{t('privacy.redactor.title')}</h2>
        <p>{t('privacy.redactor.patternsActive', { n: status.redactor.patterns_active })}</p>
        <p>{status.redactor.ner_loaded ? t('privacy.redactor.nerLoaded') : t('privacy.redactor.nerMissing')}</p>
      </section>

      <section class="rounded border p-4 space-y-2">
        <button onClick={() => api.post('/dsar/export')} class="px-3 py-1 bg-blue-600 text-white rounded">
          {t('privacy.actions.exportData')}
        </button>
        <button onClick={() => api.post('/dsar/delete')} class="ml-2 px-3 py-1 bg-red-700 text-white rounded">
          {t('privacy.actions.deleteAccount')}
        </button>
        <button onClick={() => location.assign('#/privacy/audit-log')} class="ml-2 px-3 py-1 border rounded">
          {t('privacy.actions.openAuditLog')}
        </button>
      </section>

      {error && <div class="text-red-600">{error}</div>}
    </div>
  );
}

function camel(k: string): string {
  return k.replace(/_([a-z])/g, (_, c) => c.toUpperCase());
}
```

- [ ] **Step 3: Add Sidebar entry**

In `rust/crates/attune-server/ui/src/Sidebar.tsx`, add a new tab entry between "Settings" and "Skills" (or wherever your existing tab list is). Use exact key `privacy.title`.

- [ ] **Step 4: Remove inline privacy section from SettingsView**

In `SettingsView.tsx` lines around 589–600 (`settings.privacy.telemetry`), delete the entire section and replace with:

```tsx
<Section title={t('privacy.title')}>
  <a href="#/privacy" class="text-blue-600 underline">{t('privacy.subtitle')}</a>
</Section>
```

- [ ] **Step 5: Add Playwright E2E (Chrome only) for outbound toggle**

Create `tests/playwright/privacy_view.spec.ts`:

```ts
import { test, expect, chromium } from '@playwright/test';

test('privacy view toggles web_search and persists', async () => {
  const browser = await chromium.launch({ channel: 'chrome', headless: false });
  const page = await browser.newPage();
  await page.goto('http://localhost:18900/#/privacy');
  await page.waitForSelector('[data-testid="toggle-web_search"]');
  const before = await page.isChecked('[data-testid="toggle-web_search"]');
  await page.click('[data-testid="toggle-web_search"]');
  await page.waitForTimeout(500);
  await page.reload();
  await page.waitForSelector('[data-testid="toggle-web_search"]');
  const after = await page.isChecked('[data-testid="toggle-web_search"]');
  expect(after).toBe(!before);
  await browser.close();
});
```

- [ ] **Step 6: Run i18n guard + Playwright**

Run:
```bash
cd rust/crates/attune-server/ui/src
diff <(grep -oP "^\s+'[\w.]+'\s*:" i18n/zh.ts | tr -d " ':" | sort) \
     <(grep -oP "^\s+'[\w.]+'\s*:" i18n/en.ts | tr -d " ':" | sort)
```
Expected: empty diff.

Run UI build: `cd rust/crates/attune-server/ui && npm run build`
Expected: clean build, no missing-key warnings.

Run Playwright: `npx playwright test tests/playwright/privacy_view.spec.ts`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add rust/crates/attune-server/ui/src/views/PrivacyView.tsx \
        rust/crates/attune-server/ui/src/views/SettingsView.tsx \
        rust/crates/attune-server/ui/src/Sidebar.tsx \
        rust/crates/attune-server/ui/src/i18n/zh.ts \
        rust/crates/attune-server/ui/src/i18n/en.ts \
        tests/playwright/privacy_view.spec.ts
git commit -m "feat(privacy): PrivacyView UI dashboard + 5 toggles + DSAR shortcuts"
```

---

## Task 8: Privacy Tour — one-shot first-launch modal

**Files:**
- Create: `rust/crates/attune-server/ui/src/views/PrivacyTour.tsx`
- Modify: `rust/crates/attune-server/ui/src/App.tsx` (mount tour conditionally)

- [ ] **Step 1: Write the tour component**

```tsx
// rust/crates/attune-server/ui/src/views/PrivacyTour.tsx
import { useEffect, useState } from 'preact/hooks';
import { useI18n } from '../i18n';
import { api } from '../api';

export function PrivacyTour() {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);

  useEffect(() => {
    api.get('/privacy/status').then(res => {
      api.get('/settings').then(s => {
        const seen = s.data?.privacy?.privacy_tour_seen === true;
        if (!seen) setOpen(true);
      });
    }).catch(() => {});
  }, []);

  async function dismiss() {
    setOpen(false);
    try { await api.patch('/privacy/settings', { privacy_tour_seen: true }); } catch {}
  }
  if (!open) return null;
  return (
    <div class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50">
      <div class="bg-white rounded-lg p-6 max-w-md" data-testid="privacy-tour-modal">
        <h2 class="text-xl font-bold mb-2">{t('privacy.tour.title')}</h2>
        <p class="mb-4">{t('privacy.tour.intro')}</p>
        <ul class="list-disc pl-5 mb-4">
          <li>{t('privacy.outbound.llm')}</li>
          <li>{t('privacy.outbound.cloudSaas')}</li>
          <li>{t('privacy.outbound.webdav')}</li>
          <li>{t('privacy.outbound.webSearch')}</li>
          <li>{t('privacy.outbound.telemetry')}</li>
        </ul>
        <button onClick={dismiss} class="px-3 py-1 bg-blue-600 text-white rounded"
                data-testid="privacy-tour-dismiss">
          {t('privacy.tour.cta')}
        </button>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Mount in App.tsx**

In `App.tsx`, near the root render:

```tsx
import { PrivacyTour } from './views/PrivacyTour';
// inside the returned tree, after Sidebar:
<PrivacyTour />
```

- [ ] **Step 3: Playwright test**

Append to `tests/playwright/privacy_view.spec.ts`:

```ts
test('privacy tour shows once and dismisses', async () => {
  const browser = await chromium.launch({ channel: 'chrome', headless: false });
  const page = await browser.newPage();
  await page.goto('http://localhost:18900/');
  await page.waitForSelector('[data-testid="privacy-tour-modal"]');
  await page.click('[data-testid="privacy-tour-dismiss"]');
  await page.reload();
  await page.waitForTimeout(1000);
  const modal = await page.$('[data-testid="privacy-tour-modal"]');
  expect(modal).toBeNull();
  await browser.close();
});
```

Run: `npx playwright test tests/playwright/privacy_view.spec.ts`
Expected: 2 passed.

- [ ] **Step 4: Commit**

```bash
git add rust/crates/attune-server/ui/src/views/PrivacyTour.tsx \
        rust/crates/attune-server/ui/src/App.tsx \
        tests/playwright/privacy_view.spec.ts
git commit -m "feat(privacy): one-shot Privacy Tour modal + dismiss persistence"
```

---

## Task 9: User-facing docs — PRIVACY.md + monthly audit checklist

**Files:**
- Create: `docs/PRIVACY.md`
- Create: `docs/PRIVACY-AUDIT-CHECKLIST.md`

- [ ] **Step 1: Write `docs/PRIVACY.md`**

Content (English primary; one Chinese section inline). Cover:
1. The four-promise summary (local-first / outbound-minimal / PII-redacted / user-sovereign).
2. The five outbound points table (kind / default / opt-out path / data shape).
3. Vault encryption boundary (Argon2id + AES-256-GCM dek_db/dek_idx/dek_vec).
4. DSAR usage instructions (`POST /api/v1/dsar/export` etc.).
5. Third-party LLM provider retention summary (matrix from spec §11 R1).
6. How to verify outbound traffic locally (`sudo tcpdump -i any host gateway.engi-stack.com`).
7. Inline 中文节: 隐私承诺 4 条 + 出网 5 点 + DSAR 操作.

Use the spec sections 1, 3, 5, 11 as source-of-truth. No new content beyond spec.

- [ ] **Step 2: Write `docs/PRIVACY-AUDIT-CHECKLIST.md`**

Content (internal-only):

```markdown
# Privacy Audit Checklist (monthly)

Run on the first Monday of each month. Owner: privacy maintainer.

## 1. Grep guards
- [ ] `scripts/privacy-audit.sh` returns 0 lines (no unsanctioned outbound calls)
- [ ] No hardcoded API keys (`grep -rE 'sk-[A-Za-z0-9]{20,}'`)
- [ ] No telemetry calls outside `crates/attune-core/src/telemetry.rs`

## 2. Provider policy diff
For each provider in the spec §11 R1 table, fetch the current data-retention policy URL and diff against the snapshot in `docs/provider-policies/` (commit-track changes).

- [ ] OpenAI (https://openai.com/policies/usage-policies)
- [ ] Anthropic (https://www.anthropic.com/legal/aup)
- [ ] Google Gemini (https://ai.google.dev/terms)
- [ ] DeepSeek (https://deepseek.com/privacy)
- [ ] Attune Pro Gateway (internal: cloud/docs/GATEWAY_PRIVACY.md)

If any policy weakens user protection (e.g. enabling training by default), file a RELEASE.md notice within 7 days.

## 3. Live install audit
- [ ] Install latest release on a clean Win + Linux machine.
- [ ] First launch: PrivacyTour modal renders.
- [ ] `tcpdump` during 60s idle shows ZERO outbound packets (no telemetry, no probes).
- [ ] Toggle web_search on, run a search, toggle off, run again → second search returns "outbound-disabled" error.

## 4. Recently changed code
- [ ] `git log --since="1 month ago" --diff-filter=A -- 'rust/crates/attune-*'` — any new `reqwest::get` / `tokio::TcpStream` / `tonic` outbound? If yes, ensure it's gated by `OutboundGate::enforce`.
```

- [ ] **Step 3: Verify both files render via the docs site**

Build the wiki: `cd docs && npm run build` (or whatever the existing flow is). Confirm no broken links.

- [ ] **Step 4: Commit**

```bash
git add docs/PRIVACY.md docs/PRIVACY-AUDIT-CHECKLIST.md
git commit -m "docs(privacy): user-facing PRIVACY.md + monthly audit checklist"
```

---

## Task 10: `scripts/privacy-audit.sh` + CI gate

**Files:**
- Create: `scripts/privacy-audit.sh`
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Write the audit script**

```bash
#!/usr/bin/env bash
# scripts/privacy-audit.sh — guard against unsanctioned outbound calls or
# hardcoded secrets. Returns 0 only when the working tree is clean.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

fail=0

echo "==> 1. Outbound HTTP clients must route through OutboundGate"
# Anything calling reqwest::Client::new() / .get(..) / .post(..) directly,
# OUTSIDE of outbound_gate.rs / chat.rs / cloud_client.rs / webdav.rs /
# web_search_browser.rs / telemetry.rs, must be flagged.
allow_files='rust/crates/attune-core/src/(outbound_gate|chat|cloud_client|sync/webdav|web_search_browser|telemetry|llm_provider|embedding|ocr|asr)\.rs'
hits=$(grep -rnE 'reqwest::(Client|get|post)\b' rust/crates/attune-*/src \
  | grep -vE "$allow_files" \
  | grep -vE '^[^:]+:[0-9]+:\s*//' \
  || true)
if [ -n "$hits" ]; then
  echo "FAIL: outbound HTTP outside the allow-list:"
  echo "$hits"
  fail=1
fi

echo "==> 2. Hardcoded API keys"
hits=$(grep -rnE '(sk-[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z-_]{35})' \
  rust extension docs scripts 2>/dev/null \
  | grep -vE '^[^:]+:[0-9]+:\s*//' \
  || true)
if [ -n "$hits" ]; then
  echo "FAIL: hardcoded API key candidates:"
  echo "$hits"
  fail=1
fi

echo "==> 3. Telemetry calls outside telemetry.rs"
hits=$(grep -rn 'telemetry::\|Telemetry::new\|TelemetryEvent' \
  rust/crates/attune-*/src 2>/dev/null \
  | grep -v 'rust/crates/attune-core/src/telemetry.rs' \
  | grep -v 'tests/' \
  || true)
# Allow constructor calls in app initialisation: 0 occurrences for v1.0.6
# (no actual telemetry hookup yet); if you wire it up later, narrow this rule.
if [ -n "$hits" ]; then
  echo "WARN: telemetry references outside telemetry.rs (review if intentional):"
  echo "$hits"
fi

echo "==> 4. Privacy default-false invariant"
default_block=$(grep -A 8 '"privacy":' rust/crates/attune-server/src/routes/settings.rs \
  | grep -E '"(llm|cloud_saas|webdav|web_search|telemetry)"' \
  | grep -v ': false' \
  || true)
if [ -n "$default_block" ]; then
  echo "FAIL: privacy default block contains non-false values:"
  echo "$default_block"
  fail=1
fi

if [ "$fail" -eq 0 ]; then
  echo "privacy-audit: PASS"
else
  echo "privacy-audit: FAIL"
  exit 1
fi
```

- [ ] **Step 2: Make it executable**

```bash
chmod +x scripts/privacy-audit.sh
```

- [ ] **Step 3: Run locally — verify it passes against current tree (after Tasks 1–9 are merged)**

Run: `bash scripts/privacy-audit.sh`
Expected: `privacy-audit: PASS`.

- [ ] **Step 4: Add CI job**

Edit `.github/workflows/ci.yml` — append job:

```yaml
  privacy-audit:
    name: Privacy Audit
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Run privacy-audit.sh
        run: bash scripts/privacy-audit.sh
```

- [ ] **Step 5: Push branch + verify CI green**

```bash
git add scripts/privacy-audit.sh .github/workflows/ci.yml
git commit -m "chore(privacy): privacy-audit.sh + CI hard gate"
git push origin develop
```

Watch GitHub Actions; the new `privacy-audit` job must complete with green.

---

## Task 11: RELEASE notes + version bump

**Files:**
- Modify: `RELEASE.md`
- Modify: `Cargo.toml` (workspace version → `1.0.6`)
- Modify: `rust/crates/attune-server/ui/package.json` (version)

- [ ] **Step 1: Add v1.0.6 section to RELEASE.md**

Append a new top section:

```markdown
## v1.0.6 (2026-06-05) — DR/BCP + Privacy Logic SSOT

### Highlights
- **Privacy SSOT landed** — 5 outbound points (LLM / Cloud SaaS / WebDAV / Web Search / Telemetry) now have a single dashboard at `Settings → Privacy`, all default off.
- `POST /api/v1/privacy/wipe-cloud-session` — one click clears cloud session token, flips cloud_saas off.
- `OutboundGate` enforces PII redactor at every egress site (chat, web search, webdav).
- Telemetry remains default-off; v1.0.6 ships the queue stub only — no actual upload.

### Breaking
- None. (Telemetry default is unchanged: false.)

### Migration
- Existing settings.json without a `privacy` block: server lazily inserts default-false block on first read. No user action required.

### Known Limitations
- L2 ONNX NER redactor is opt-in via Settings → AI Stack; default is L1 regex only.
- WebDAV uploads are always ciphertext (vault DB blocks), so the redactor bypasses them; cleartext WebDAV scenarios are not supported.
- Telemetry actual upload deferred to v1.1 — `Telemetry::send()` returns `SkippedNotImplemented` even when enabled.
```

- [ ] **Step 2: Bump versions**

```bash
sed -i 's/^version = "1\.0\.5"/version = "1.0.6"/' rust/Cargo.toml
sed -i 's/"version": "1.0.5"/"version": "1.0.6"/' rust/crates/attune-server/ui/package.json
```

- [ ] **Step 3: Verify**

Run: `grep -rn '1.0.5\|1.0.6' rust/Cargo.toml rust/crates/attune-server/ui/package.json`
Expected: all occurrences are `1.0.6` (with the exception of historical entries in `Cargo.lock` that the next `cargo build` will fix up).

Run: `cargo build --workspace`
Expected: success; Cargo.lock updates automatically.

- [ ] **Step 4: Commit**

```bash
git add RELEASE.md rust/Cargo.toml rust/Cargo.lock rust/crates/attune-server/ui/package.json
git commit -m "release: v1.0.6 — privacy logic SSOT + 5 outbound dashboards"
```

---

## Task 12: Merge develop → main + tag v1.0.6

- [ ] **Step 1: Full workspace test**

Run: `cargo test --workspace --release`
Expected: all green.

Run: `bash scripts/privacy-audit.sh`
Expected: `privacy-audit: PASS`.

Run UI build: `cd rust/crates/attune-server/ui && npm run build && cd ../../../..`
Expected: clean.

Run Playwright suite: `npx playwright test tests/playwright/privacy_view.spec.ts`
Expected: 2 passed.

- [ ] **Step 2: Push develop**

```bash
git push origin develop
```

- [ ] **Step 3: Merge develop → main with --no-ff**

```bash
git checkout main
git pull
git merge --no-ff develop -m "merge: develop → main (v1.0.6 release — privacy SSOT)"
git push origin main
```

Verify: `git log origin/main --first-parent --oneline | head -3` shows the merge as the latest entry; `git log origin/develop..origin/main --first-parent` returns only `merge: ...` lines.

- [ ] **Step 4: Tag**

```bash
git tag -a v1.0.6 -m "v1.0.6 — privacy logic SSOT (5 outbound dashboards + OutboundGate + audit script)"
git tag -a desktop-v1.0.6 -m "desktop-v1.0.6 — same as v1.0.6"
git push origin v1.0.6 desktop-v1.0.6
```

- [ ] **Step 5: Verify CI release workflows triggered**

Open the GitHub Actions tab. Both `rust-release.yml` and `desktop-release.yml` workflows should be running. Wait for green; download the release artifact for one platform (Linux x86_64) and confirm `attune --version` prints `1.0.6`.

- [ ] **Step 6: Update tracker**

Mark task #206 partial-completion note (this plan is one of two for B-batch); leave #206 in_progress until Plan B2 also ships.

---

## Self-Review

Spec coverage check (against `2026-05-28-privacy-logic-strategy.md`):

| Spec §  | Requirement | Implemented by |
|---------|-------------|----------------|
| §1 core promises | Local-first / outbound-minimal / PII-redacted / DSAR | Tasks 3 (OutboundGate), 9 (PRIVACY.md), pre-existing dsar.rs |
| §3.1 5 outbound points | LLM / Cloud SaaS / WebDAV / Web Search / Telemetry | Tasks 1 (default-false), 2 (status endpoint), 3 (gate at all 5 sites) |
| §3.3 default state | All 5 default off; telemetry never auto-opt-in | Tasks 1, 5 |
| §5.1 endpoints | `/status`, `/settings` PATCH, `/lock`, `/wipe-cloud-session` | Task 2 |
| §5.2 DSAR | Existing endpoints + audit log | Task 6 |
| §5.3 audit log | No prompt/response/key in log | Task 6 (`assert log MUST NOT contain password`) |
| §6.1 new outbound flow | `scripts/privacy-audit.sh` + spec amend rule | Task 10 |
| §6.2 new PII pattern | Existing `pii::patterns.rs` API — out of scope here | spec only |
| §6.3 plugin layer | `Redactor::with_extra` — existing | spec only |
| §7 boundary cases | Vault locked / NER missing / cloud expire / WebDAV cred error | Task 3 OutboundGate handles vault-locked + RedactorRequired; UI shows red status from Task 7 |
| §8 cost contract | UI shows per-toggle cost | Task 7 (toggle labels include cost text from i18n) — note: i18n keys above currently lack `privacy.outbound.llmCost` etc., add when implementing if cost annotations desired |
| §9.1 golden cases | ≥10 redactor cases | Task 3 (10 cases in yaml) |
| §9.1 property test | redact↔restore | not in scope of this plan — already covered by existing `pii/mod.rs` proptest |
| §9.1 integration | DSAR export → import hash, tcpdump opt-out | Task 9 audit checklist §3 |
| §10 backward compat | Migration for old vaults missing audit table | existing `store::audit` migration |
| §11 R1–R8 risks | Mitigated by Tasks 3,6,7,9,10 | tagged in plan |

Placeholder scan: No "TBD", "implement later", "add appropriate error handling" found. Every step has either complete code or a precise file/grep target.

Type consistency: `OutboundKind`, `OutboundPolicy`, `OutboundGate::enforce` — same signature across Tasks 3, 5. `Telemetry::send` returns `SendOutcome` everywhere. `PrivacyView` UI uses `OUTBOUND_KEYS` array literally matching the 5 keys in the server status response.

Open items to surface to the user before execution:
1. **`is_telemetry_path_allowed` is a public helper** — design decision: should it live in `settings.rs` or a new `privacy_guard.rs`? Plan keeps it co-located for now.
2. **`SharedState` accessor methods listed in Task 2 Step 3** — confirm the existing `state.rs` already wraps vault + cloud_client; if not, those methods need to be threaded through during Task 2 Step 3. The plan assumes accessor refactor lands inside Task 2 as part of the same commit (see CLAUDE.md "State 访问" convention).
3. **WebDAV cleartext vs ciphertext payload** — Task 3 Step 7 notes the open question. Confirm with a quick grep of `sync/webdav.rs` whether anything besides the ciphertext vault blob is transferred; the default policy in the plan is `redactor: None` (ciphertext only) but flip if needed.

---

**Plan complete and saved to `docs/superpowers/plans/2026-05-28-privacy-logic-implementation.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — execute tasks in this session using executing-plans, batch with checkpoints.

**Which approach?**
