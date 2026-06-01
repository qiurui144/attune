pub mod acp_chat;
pub mod error;
pub mod eval;
pub mod routes;
pub mod state;
pub(crate) mod middleware;
pub(crate) mod ingest_webdav;
pub(crate) mod ingest_email;
pub(crate) mod ingest_rss;
pub(crate) mod ingest_git;

// T1 (v1.0.6 KB-bench, plan 2026-05-28-kb-bench-integration.md Step 9):
// in-process eval-mode harness used by `tests/eval_determinism_test.rs`.
// Marked `#[doc(hidden)]` so the symbol does not appear in user-facing API
// docs — production callers (Chrome ext / Web UI / attune-cli) never import
// this module. Plain `pub` (not feature-gated) keeps the integration test
// crate compileable without a separate `--features test-support` cargo flag.
#[doc(hidden)]
pub mod test_support;

use axum::middleware as axum_mw;
use axum::routing::{delete, get, post};
use axum::http::{HeaderValue, Method};
use axum::Router;
use std::sync::Arc;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::writer::MakeWriterExt;

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
        .route("/api/v1/vault/reset-with-recovery-key", post(routes::vault::vault_reset_with_recovery_key))
        .route("/api/v1/vault/change-password", post(routes::vault::vault_change_password))
        .route("/api/v1/vault/forgot-password-reset", post(routes::vault::vault_forgot_password_reset))
        .route("/api/v1/vault/device-secret/export", get(routes::vault::export_device_secret))
        .route("/api/v1/vault/device-secret/import", post(routes::vault::import_device_secret))
        // Status (health check bypasses guard)
        .route("/api/v1/status/health", get(routes::status::health))
        .route("/api/v1/status/diagnostics", get(routes::status::diagnostics))
        // v1.0.1 active version notification(per spec C3)
        .route("/api/v1/version", get(routes::version::get_version))
        // LLM 运维端点（Wizard + Settings）
        .route("/api/v1/llm/test", post(routes::llm::test_llm))
        .route("/api/v1/llm/probe-k3", post(routes::llm::probe_k3))
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
        // Plugin UI form runtime
        .route("/api/v1/forms/{plugin_id}/{form_id}", get(routes::forms::get_form))
        .route("/api/v1/forms/{plugin_id}/{form_id}/schema", get(routes::forms::get_form_schema))
        .route("/api/v1/forms/{plugin_id}/{form_id}/submit", post(routes::forms::submit_form))
        // Member state + settings locks (UI 灰显决策源)
        .route("/api/v1/member/state", get(routes::member::get_state))
        .route("/api/v1/member/locks", get(routes::member::get_locks))
        .route("/api/v1/member/login-token", post(routes::member::login_token))
        .route("/api/v1/member/login-password", post(routes::member::login_password))
        .route("/api/v1/member/logout", post(routes::member::logout))
        // DSAR (GDPR Art.15/17/20 + 中国 PIPL §44-50) — cloud member 数据主权操作
        // 桌面 UI 经此 proxy 到 cloud accounts，密码不持久化
        .route("/api/v1/dsar/export", post(routes::dsar::export_data))
        .route("/api/v1/dsar/delete", post(routes::dsar::delete_account))
        .route("/api/v1/dsar/cancel-deletion", post(routes::dsar::cancel_deletion))
        // OCR 场景预设 (CRUD, builtin 不可删/改)
        .route("/api/v1/ocr/profiles",
            get(routes::ocr_profiles::list_profiles)
                .post(routes::ocr_profiles::create_profile))
        .route("/api/v1/ocr/profiles/{id}",
            axum::routing::put(routes::ocr_profiles::update_profile)
                .delete(routes::ocr_profiles::delete_profile))
        // Office helper (v0.7.1) — OCR 同步 + ASR 异步 + WS 进度
        .route("/api/v1/office/ocr",
            axum::routing::post(routes::office::post_ocr))
        .route("/api/v1/office/transcribe",
            axum::routing::post(routes::office::post_transcribe))
        .route("/api/v1/office/jobs/{job_id}",
            get(routes::office::get_job)
                .delete(routes::office::delete_job))
        .route("/api/v1/office/jobs/ws",
            get(routes::office::ws_jobs))
        // Folder links — 只读 (写入由 attune-cli link-folder)
        .route("/api/v1/folder-links", get(routes::folder_links::list_folder_links))
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
        // PATCH content 需与 /upload 同享 100MB body limit。
        // 否则 axum 默认 2MB 先拦截，update_item 的 MAX_CONTENT_LEN=100MB 检查成死代码，
        // 且用户能 upload 100MB 文档却无法 PATCH 编辑（2MB 上限）。GET/DELETE 无 body 不受影响。
        .route("/api/v1/items/{id}",
            get(routes::items::get_item)
                .delete(routes::items::delete_item)
                .patch(routes::items::update_item)
                .layer(axum::extract::DefaultBodyLimit::max(100 * 1024 * 1024)))
        .route("/api/v1/items/{id}/stats", get(routes::items::get_item_stats))
        // 取回原始证据文件（变体 A「查看证据原文」）
        .route("/api/v1/items/{id}/original", get(routes::items::get_item_original))
        // v0.6 Phase A.5.4 per-file 隐私分级
        .route("/api/v1/items/protected", get(routes::items::list_protected_items))
        .route(
            "/api/v1/items/{id}/privacy_tier",
            get(routes::items::get_item_privacy)
                .patch(routes::items::set_item_privacy),
        )
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
        // E1 marketplace toggle (W4, 2026-04-27)
        .route("/api/v1/plugins/{id}/toggle", post(routes::plugins::toggle))
        // law-pro 接入阶段 2: 前端触发 plugin agent binary（civil_loan_agent 算金额）
        .route("/api/v1/agents/{agent_id}/run", post(routes::agents::run_agent))
        // E4 (2026-05-01) — PluginHub marketplace (默认 Mock provider；attune-pro 注入 hub-client)
        .route("/api/v1/marketplace/plugins", get(routes::marketplace::list_plugins))
        .route("/api/v1/marketplace/plugins/{id}/install", post(routes::marketplace::install_plugin))
        .route("/api/v1/skills", get(routes::skills::list_skills))
        .route("/api/v1/patent/search", post(routes::patent::search))
        .route("/api/v1/patent/databases", get(routes::patent::databases))
        .route("/api/v1/profile/export", get(routes::profile::export))
        .route("/api/v1/profile/import", post(routes::profile::import))
        // F1 topic distribution (W4, 2026-04-27) — 桌面"我的画像"页后端
        .route("/api/v1/profile/topic_distribution", get(routes::profile::topic_distribution))
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
        // G1 Browse signals (W3 batch B, 2026-04-27)
        // per spec docs/superpowers/specs/2026-04-27-w3-batch-b-design.md §3
        // OPT-5 kebab unification: /browse-signals 是新 canonical, /browse_signals 旧 alias 留 1 release
        .route("/api/v1/browse-signals",
               post(routes::browse_signals::record_batch)
                   .get(routes::browse_signals::list)
                   .delete(routes::browse_signals::delete))
        .route("/api/v1/browse_signals",
               post(routes::browse_signals::record_batch)
                   .get(routes::browse_signals::list)
                   .delete(routes::browse_signals::delete))
        // C1 Web search cache (W4-002, 2026-04-27) — close C1 loop
        // Settings UI "清空 Web 搜索缓存" 入口
        .route("/api/v1/web-search-cache",
               get(routes::web_search_cache::count)
                   .delete(routes::web_search_cache::delete))
        .route("/api/v1/web_search_cache",
               get(routes::web_search_cache::count)
                   .delete(routes::web_search_cache::delete))
        // AI 底座状态（v0.6.0-rc.3, 2026-04-27）— Embedding / Rerank / OCR / ASR / LLM 可用性
        .route("/api/v1/ai-stack", get(routes::ai_stack::status))
        .route("/api/v1/ai_stack", get(routes::ai_stack::status))
        // G2 Auto bookmark candidates (W4, 2026-04-27)
        // POST 不暴露：仅由 routes::browse_signals::record_batch high_engagement 路径写
        .route("/api/v1/auto-bookmarks",
               get(routes::auto_bookmarks::list)
                   .delete(routes::auto_bookmarks::delete))
        .route("/api/v1/auto_bookmarks",
               get(routes::auto_bookmarks::list)
                   .delete(routes::auto_bookmarks::delete))
        // v0.6 Phase A.5.3 隐私出网审计日志（GET 列表 + CSV 导出）
        // 写入由 attune-core::Store::record_outbound 在 LLM provider hook 中触发，不暴露 POST
        .route("/api/v1/audit/outbound", get(routes::audit::list))
        .route("/api/v1/audit/outbound/export.csv", get(routes::audit::export_csv))
        // v0.7 F1: 新 audit_log 表 + RFC4180 CSV
        .route("/api/v1/audit/log", get(routes::audit::list_log))
        .route("/api/v1/audit/log.csv", get(routes::audit::export_log_csv))
        // v0.7 F3: demo sample data 一键加载
        .route("/api/v1/demo/load", post(routes::demo::load_demo))
        // v0.6 Phase A.5.5 隐私 tier 检测（Settings UI Privacy 页用）
        .route("/api/v1/privacy/tier", get(routes::privacy::tier))
        // v1.0.6 Privacy Logic Strategy — 5 outbound points 总览 + 切换 + lock + wipe
        // per docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md §5.1
        .route("/api/v1/privacy/status", get(routes::privacy::status))
        .route("/api/v1/privacy/settings", axum::routing::patch(routes::privacy::settings_patch))
        .route("/api/v1/privacy/lock", post(routes::privacy::lock))
        .route("/api/v1/privacy/wipe-cloud-session", post(routes::privacy::wipe_cloud_session))
        // Status (full status requires vault access)
        .route("/api/v1/status", get(routes::status::status))
        // Index management
        .route("/api/v1/index/bind", post(routes::index::bind_directory))
        .route("/api/v1/index/bind-remote", post(routes::remote::bind_remote))
        .route("/api/v1/index/email-accounts", get(routes::email::list_email_accounts))
        .route("/api/v1/index/bind-email", post(routes::email::bind_email))
        .route(
            "/api/v1/index/email-accounts/{dir_id}",
            axum::routing::delete(routes::email::delete_email_account),
        )
        .route(
            "/api/v1/index/email-accounts/{dir_id}/sync",
            post(routes::email::sync_email_account_now),
        )
        // RSS / Atom 订阅 —— 第三采集源
        .route("/api/v1/sources/rss/feeds", get(routes::rss::list_feeds))
        .route("/api/v1/sources/rss/feeds", post(routes::rss::add_feed))
        .route(
            "/api/v1/sources/rss/feeds/{id}",
            axum::routing::delete(routes::rss::delete_feed),
        )
        .route(
            "/api/v1/sources/rss/feeds/{id}",
            axum::routing::patch(routes::rss::update_feed),
        )
        .route(
            "/api/v1/sources/rss/feeds/{id}/poll",
            post(routes::rss::poll_feed_now),
        )
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
        // favicon：返回 204 避免浏览器自动请求落空刷 console error
        .route("/favicon.ico", get(|| async { axum::http::StatusCode::NO_CONTENT }))
        // Guard middleware for all other routes
        .layer(axum_mw::from_fn_with_state(shared_state.clone(), middleware::vault_guard))
        .layer(axum_mw::from_fn_with_state(shared_state.clone(), middleware::bearer_auth_guard))
        // 访问日志 — 放最外层 (axum middleware 调用顺序: 外 -> 内 -> handler -> 内 -> 外),
        // 保证 4xx 也能记 (即便 vault_guard 拒了请求, access_log 仍记 status).
        .layer(axum_mw::from_fn_with_state(shared_state.clone(), middleware::access_log))
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
    // 双 sink — stdout (人类可读) + 文件 daily rotation (生产排障)。
    // 文件位于 data_dir()/logs/attune-server.YYYY-MM-DD（tracing-appender 自动加日期）。
    // 默认保留 7 天 — 由用户手动清理（tracing-appender 0.2 没有自动清理）。
    let log_dir = attune_core::platform::data_dir().join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let file_appender = tracing_appender::rolling::daily(&log_dir, "attune-server");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    // guard 必须 leak 让 worker thread 在进程整个生命周期内存活；正常退出会刷盘。
    Box::leak(Box::new(guard));

    let env_filter = EnvFilter::from_default_env()
        .add_directive("info".parse().expect("'info' is a valid log directive"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stdout.and(non_blocking))
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
    // ACP-4 Task 2 — wire the usage telemetry aggregator + flusher at boot so
    // every governed LLM call records into the `usage_events` table (closes the
    // audit-C "usage recorder NOT wired" gap). Telemetry-only; failure degrades
    // gracefully without affecting request handling.
    let _usage_flusher = shared_state.install_usage_aggregator();
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

    // 安装信号 stream **同步在 spawn 之前**，确保 SIGTERM/SIGINT 不被默认 handler 抢走。
    // tokio::signal::unix::signal 必须在收到信号前调，且要 hold stream 不被 drop。
    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to install SIGTERM handler");
    tracing::info!("shutdown signal handlers installed (SIGINT + SIGTERM)");
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<&'static str>();
    tokio::spawn(async move {
        let reason: &'static str;
        #[cfg(unix)]
        {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => reason = "SIGINT (Ctrl-C)",
                _ = sigterm.recv() => reason = "SIGTERM",
            }
        }
        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c().await.expect("ctrl_c");
            reason = "Ctrl-C";
        }
        tracing::info!("received {reason}, graceful shutdown initiated");
        let _ = shutdown_tx.send(reason);
    });

    match (config.tls_cert.as_ref(), config.tls_key.as_ref()) {
        (Some(cert), Some(key)) => {
            tracing::info!("attune-server listening on https://{addr}");
            let tls_config =
                axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
            // graceful shutdown via axum_server Handle，等 oneshot 收到信号后 30s 内 drain
            let handle = axum_server::Handle::new();
            let shutdown_handle = handle.clone();
            tokio::spawn(async move {
                let _ = shutdown_rx.await;
                shutdown_handle.graceful_shutdown(Some(std::time::Duration::from_secs(30)));
            });
            axum_server::bind_rustls(addr, tls_config)
                .handle(handle)
                .serve(app.into_make_service())
                .await?;
        }
        _ => {
            tracing::info!("attune-server listening on http://{addr}");
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            tokio::select! {
                res = axum::serve(listener, app) => { res?; }
                _ = &mut shutdown_rx => {}
            }
        }
    }

    tracing::info!("attune-server shut down cleanly");
    Ok(())
}

