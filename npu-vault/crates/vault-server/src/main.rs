mod middleware;
mod routes;
mod state;

use axum::middleware as axum_mw;
use axum::routing::{delete, get, post};
use axum::http::{HeaderValue, Method};
use axum::Router;
use clap::Parser;
use std::sync::Arc;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "npu-vault-server", version, about = "npu-vault HTTP API server")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value = "18900")]
    port: u16,
    /// Path to TLS certificate (PEM) - enables HTTPS
    #[arg(long)]
    tls_cert: Option<String>,
    /// Path to TLS private key (PEM)
    #[arg(long)]
    tls_key: Option<String>,
    /// Disable Bearer token authentication (local dev only).
    /// WARNING: Never use on network-accessible hosts.
    #[arg(long)]
    no_auth: bool,
}

fn is_allowed_origin(s: &str) -> bool {
    s.starts_with("chrome-extension://")
        || s.starts_with("http://localhost")
        || s.starts_with("http://127.0.0.1")
        || s.starts_with("https://localhost")
        || s.starts_with("https://127.0.0.1")
}

#[cfg(test)]
mod tests {
    use super::is_allowed_origin;

    #[test]
    fn cors_allows_chrome_extension() {
        assert!(is_allowed_origin("chrome-extension://abcdefghijklmnop"));
    }

    #[test]
    fn cors_allows_localhost() {
        assert!(is_allowed_origin("http://localhost:18900"));
        assert!(is_allowed_origin("http://127.0.0.1:18900"));
        assert!(is_allowed_origin("https://localhost:18900"));
        assert!(is_allowed_origin("https://127.0.0.1:18900"));
    }

    #[test]
    fn cors_blocks_evil_origin() {
        assert!(!is_allowed_origin("https://evil.com"));
        assert!(!is_allowed_origin("http://192.168.1.100:18900"));
        assert!(!is_allowed_origin("null"));
        assert!(!is_allowed_origin(""));
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .init();

    let cli = Cli::parse();

    let vault = vault_core::vault::Vault::open_default()
        .expect("Failed to open vault");
    let require_auth = if cli.no_auth {
        tracing::warn!(
            "⚠  Authentication DISABLED via --no-auth. \
             Do NOT use in production or on network-accessible hosts."
        );
        false
    } else {
        true
    };
    let shared_state = Arc::new(state::AppState::new(vault, require_auth));

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

    let app = Router::new()
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
        // Ingest + Items + Search
        .route("/api/v1/ingest", post(routes::ingest::ingest))
        .route("/api/v1/items", get(routes::items::list_items))
        .route("/api/v1/items/{id}", get(routes::items::get_item).delete(routes::items::delete_item).patch(routes::items::update_item))
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
        // Web UI (embedded single-page HTML)
        .route("/", get(routes::ui::index))
        .route("/ui", get(routes::ui::index))
        // Guard middleware for all other routes
        // vault_guard added first (inner layer), bearer_auth_guard added second (outer layer, executes first)
        .layer(axum_mw::from_fn_with_state(shared_state.clone(), middleware::vault_guard))
        .layer(axum_mw::from_fn_with_state(shared_state.clone(), middleware::bearer_auth_guard))
        .layer(cors)
        .with_state(shared_state);

    // NAS 模式安全告警：非 loopback host 且无 TLS 时提醒用户
    let is_loopback = cli.host == "127.0.0.1" || cli.host == "localhost" || cli.host == "::1";
    let has_tls = cli.tls_cert.is_some() && cli.tls_key.is_some();
    if !is_loopback && !has_tls {
        tracing::warn!(
            "⚠  WARNING: Server bound to non-loopback address '{}' without TLS. \
             All traffic (including tokens and vault data) is transmitted in plaintext. \
             Enable TLS with --tls-cert and --tls-key for NAS/remote access.",
            cli.host
        );
    }
    if !is_loopback && !require_auth {
        tracing::warn!(
            "⚠  WARNING: Authentication is DISABLED on a non-loopback interface '{}'. \
             Any host on the network can access your vault without credentials.",
            cli.host
        );
    }

    let addr: std::net::SocketAddr = format!("{}:{}", cli.host, cli.port)
        .parse()
        .expect("invalid address");

    match (cli.tls_cert.as_ref(), cli.tls_key.as_ref()) {
        (Some(cert), Some(key)) => {
            tracing::info!("npu-vault-server listening on https://{addr}");
            let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
                .await
                .expect("failed to load TLS cert/key");
            axum_server::bind_rustls(addr, config)
                .serve(app.into_make_service())
                .await
                .expect("server error");
        }
        _ => {
            tracing::info!("npu-vault-server listening on http://{addr}");
            let listener = tokio::net::TcpListener::bind(&addr)
                .await
                .expect("bind failed");
            axum::serve(listener, app).await.expect("server error");
        }
    }
}
