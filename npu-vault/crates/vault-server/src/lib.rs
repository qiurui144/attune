pub mod routes;
pub mod state;
pub mod middleware;

use axum::middleware as axum_mw;
use axum::routing::{delete, get, post};
use axum::http::{HeaderValue, Method};
use axum::Router;
use std::sync::Arc;
use tower_http::cors::{AllowOrigin, CorsLayer};

pub fn is_allowed_origin(s: &str) -> bool {
    s.starts_with("chrome-extension://")
        || s.starts_with("http://localhost")
        || s.starts_with("http://127.0.0.1")
        || s.starts_with("https://localhost")
        || s.starts_with("https://127.0.0.1")
}

pub fn build_router(shared_state: Arc<state::AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _req| {
            is_allowed_origin(origin.to_str().unwrap_or(""))
        }))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
        ])
        .allow_credentials(true);

    Router::new()
        // Vault endpoints (no guard needed)
        .route("/api/v1/vault/status", get(routes::vault::vault_status))
        .route("/api/v1/vault/setup", post(routes::vault::vault_setup))
        .route("/api/v1/vault/unlock", post(routes::vault::vault_unlock))
        .route("/api/v1/vault/lock", post(routes::vault::vault_lock))
        .route("/api/v1/vault/change-password", post(routes::vault::vault_change_password))
        .route("/api/v1/vault/device-secret/export", get(routes::vault::export_device_secret))
        .route("/api/v1/vault/device-secret/import", post(routes::vault::import_device_secret))
        // Status (health check bypasses guard)
        .route("/api/v1/status/health", get(routes::status::health))
        .route("/api/v1/status/diagnostics", get(routes::status::diagnostics))
        // Chat (RAG)
        .route("/api/v1/chat", post(routes::chat::chat))
        .route("/api/v1/chat/history", get(routes::chat::chat_history))
        // Chat Sessions
        .route("/api/v1/chat/sessions", get(routes::chat_sessions::list_sessions))
        .route(
            "/api/v1/chat/sessions/{id}",
            get(routes::chat_sessions::get_session).delete(routes::chat_sessions::delete_session),
        )
        // Ingest + Items + Search
        .route("/api/v1/ingest", post(routes::ingest::ingest))
        .route("/api/v1/feedback", post(routes::feedback::submit_feedback))
        .route("/api/v1/items", get(routes::items::list_items))
        .route("/api/v1/items/stale", get(routes::items::list_stale_items))
        .route("/api/v1/items/{id}", get(routes::items::get_item).delete(routes::items::delete_item).patch(routes::items::update_item))
        .route("/api/v1/items/{id}/stats", get(routes::items::get_item_stats))
        .route("/api/v1/settings", get(routes::settings::get_settings).patch(routes::settings::update_settings))
        .route("/api/v1/search", get(routes::search::search))
        .route("/api/v1/search/relevant", post(routes::search::search_relevant))
        .route("/api/v1/classify/rebuild", post(routes::classify::rebuild))
        .route("/api/v1/classify/drain", post(routes::classify::drain))
        .route("/api/v1/classify/status", get(routes::classify::status))
        .route("/api/v1/classify/{id}", post(routes::classify::classify_one))
        .route("/api/v1/tags", get(routes::tags::all_dimensions))
        .route("/api/v1/tags/{dimension}", get(routes::tags::dimension_histogram))
        .route("/api/v1/clusters", get(routes::clusters::list))
        .route("/api/v1/clusters/rebuild", post(routes::clusters::rebuild))
        .route("/api/v1/clusters/{id}", get(routes::clusters::detail))
        .route("/api/v1/plugins", get(routes::plugins::list))
        .route("/api/v1/profile/export", get(routes::profile::export))
        .route("/api/v1/profile/import", post(routes::profile::import))
        .route("/api/v1/behavior/click", post(routes::behavior::log_click))
        .route("/api/v1/behavior/history", get(routes::behavior::history))
        .route("/api/v1/behavior/popular", get(routes::behavior::popular))
        // Status (full status requires vault access)
        .route("/api/v1/status", get(routes::status::status))
        // Index management
        .route("/api/v1/index/bind", post(routes::index::bind_directory))
        .route("/api/v1/index/bind-remote", post(routes::remote::bind_remote))
        .route("/api/v1/index/unbind", delete(routes::index::unbind_directory))
        .route("/api/v1/index/status", get(routes::index::index_status))
        // File upload
        .route("/api/v1/upload", post(routes::upload::upload_file))
        // WebSocket endpoints (no vault_guard needed)
        .route("/ws/scan-progress", get(routes::ws::scan_progress))
        // Web UI (embedded single-page HTML)
        .route("/", get(routes::ui::index))
        .route("/ui", get(routes::ui::index))
        // Guard middleware for all other routes
        .layer(axum_mw::from_fn_with_state(shared_state.clone(), middleware::vault_guard))
        .layer(axum_mw::from_fn_with_state(shared_state.clone(), middleware::bearer_auth_guard))
        .layer(cors)
        .with_state(shared_state)
}
