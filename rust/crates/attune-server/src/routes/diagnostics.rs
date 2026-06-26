//! Diagnostics Center — read-only capability metadata (P0 ②, spec 2026-06-26).
//!
//! `GET /api/v1/diagnostics/capabilities` projects the Capability Registry: it
//! refreshes runtime health (from model_bootstrap / provider presence / member
//! state) and returns the id-sorted snapshot. Pure projection — infallible, so
//! a plain `Json` (no error path). On the vault_guard bypass list so the
//! diagnostics center renders pre-unlock (read-only metadata, no DEK).

use axum::extract::State;
use axum::Json;

use crate::state::SharedState;

/// GET /api/v1/diagnostics/capabilities
pub async fn capabilities(
    State(state): State<SharedState>,
) -> Json<Vec<attune_core::capability::Capability>> {
    state.refresh_capability_health();
    Json(state.capabilities.snapshot())
}
