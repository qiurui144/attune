pub mod routes;
pub mod state;
pub(crate) mod middleware;

use axum::middleware as axum_mw;
use axum::routing::{delete, get, post};
use axum::http::{HeaderValue, Method};
use axum::Router;
use std::sync::Arc;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing_subscriber::EnvFilter;

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
        // Health check（前缀外，方便 Tauri / monitor 直接探活）
        .route("/health", get(routes::status::health))
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
        // LLM 运维端点（Wizard + Settings）
        .route("/api/v1/llm/test", post(routes::llm::test_llm))
        .route("/api/v1/models/pull", post(routes::llm::pull_model))
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
        // 批注（annotations）CRUD — 所有调用都是用户显式操作，不在建库流水线里自动触发
        .route("/api/v1/annotations",
            get(routes::annotations::list_annotations)
                .post(routes::annotations::create_annotation))
        // AI 分析 — 💰 层：用户显式点"🤖 AI 分析"才触发。不走建库管道。
        .route("/api/v1/annotations/ai", post(routes::annotations::ai_analyze))
        .route("/api/v1/annotations/{id}",
            axum::routing::patch(routes::annotations::update_annotation)
                .delete(routes::annotations::delete_annotation))
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
        .route("/api/v1/patent/search", post(routes::patent::search))
        .route("/api/v1/patent/databases", get(routes::patent::databases))
        .route("/api/v1/profile/export", get(routes::profile::export))
        .route("/api/v1/profile/import", post(routes::profile::import))
        // Projects / Case 卷宗（Sprint 1 Phase B）
        .route(
            "/api/v1/projects",
            get(routes::projects::list_projects).post(routes::projects::create_project),
        )
        .route("/api/v1/projects/{id}", get(routes::projects::get_project))
        .route(
            "/api/v1/projects/{id}/files",
            get(routes::projects::list_project_files).post(routes::projects::add_file_to_project),
        )
        .route(
            "/api/v1/projects/{id}/timeline",
            get(routes::projects::list_project_timeline),
        )
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
        // File upload（multipart body limit 匹配 MAX_UPLOAD_BYTES 100MB；
        // axum 默认 2MB 对扫描版 PDF 不够）。
        // ⚠ 100MB 必须与 routes::upload::MAX_UPLOAD_BYTES 同步。两处都存在是有意设计
        // （此处是框架层拦截 + upload.rs 是应用层第二道防线），见 upload.rs 注释。
        .route("/api/v1/upload",
            post(routes::upload::upload_file)
                .layer(axum::extract::DefaultBodyLimit::max(100 * 1024 * 1024)))
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

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub no_auth: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 18900,
            tls_cert: None,
            tls_key: None,
            no_auth: false,
        }
    }
}

/// 启动 attune-server 在当前 tokio runtime 上。
///
/// 用法：
/// - `attune-server-headless` binary 直接 await
/// - `attune-desktop` (Tauri) 也调这个函数把 axum 跑在 Tauri 的 tokio runtime
pub async fn run_in_runtime(
    config: ServerConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().expect("'info' is a valid log directive")))
        .try_init();

    let hw = attune_core::platform::HardwareProfile::detect();
    tracing::info!("hardware: {}", hw.summary());
    let applied = hw.apply_recommended_env();
    for (key, reason) in &applied {
        tracing::info!(
            "hardware: set {}={} — {}",
            key,
            std::env::var(key).unwrap_or_default(),
            reason
        );
    }

    let vault = attune_core::vault::Vault::open_default()?;
    let require_auth = !config.no_auth;
    if config.no_auth {
        tracing::warn!("⚠  Authentication DISABLED via config.no_auth.");
    }

    let shared_state = Arc::new(state::AppState::new(vault, require_auth));
    let app = build_router(shared_state);

    let is_loopback = {
        use std::net::IpAddr;
        config.host == "localhost"
            || config.host
                .parse::<IpAddr>()
                .map(|ip| ip.is_loopback())
                .unwrap_or(false)
    };
    let has_tls = config.tls_cert.is_some() && config.tls_key.is_some();
    if !is_loopback && !has_tls {
        tracing::warn!("⚠  Server bound to non-loopback '{}' without TLS.", config.host);
    }
    if !is_loopback && !require_auth {
        tracing::warn!("⚠  Auth disabled on non-loopback '{}'.", config.host);
    }

    let addr: std::net::SocketAddr = format!("{}:{}", config.host, config.port).parse()?;

    match (config.tls_cert.as_ref(), config.tls_key.as_ref()) {
        (Some(cert), Some(key)) => {
            tracing::info!("attune-server listening on https://{addr}");
            let tls_config =
                axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
            axum_server::bind_rustls(addr, tls_config)
                .serve(app.into_make_service())
                .await?;
        }
        _ => {
            tracing::info!("attune-server listening on http://{addr}");
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            axum::serve(listener, app).await?;
        }
    }

    Ok(())
}
