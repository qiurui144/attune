use attune_server::state;
use attune_server::build_router;
use clap::Parser;
use std::sync::Arc;
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

#[cfg(test)]
mod tests {
    use attune_server::is_allowed_origin;

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

    // 硬件画像检测 + 默认启用平台优化（在打开 vault / 加载模型之前，
    // 这样 env var 对后续的 ORT / reqwest / Ollama 子进程都生效）
    let hw = attune_core::platform::HardwareProfile::detect();
    tracing::info!("hardware: {}", hw.summary());
    let applied = hw.apply_recommended_env();
    for (key, reason) in &applied {
        tracing::info!("hardware: set {}={} — {}", key, std::env::var(key).unwrap_or_default(), reason);
    }
    if applied.is_empty() && (hw.has_amd_gpu || hw.has_nvidia_gpu) {
        tracing::debug!("hardware: GPU detected but env vars already set by user/system");
    }

    let cli = Cli::parse();

    let vault = attune_core::vault::Vault::open_default()
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

    let app = build_router(shared_state.clone());

    // NAS 模式安全告警：非 loopback host 且无 TLS 时提醒用户
    let is_loopback = {
        use std::net::IpAddr;
        cli.host == "localhost"
            || cli.host
                .parse::<IpAddr>()
                .map(|ip| ip.is_loopback())
                .unwrap_or(false)
    };
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
            tracing::info!("attune-server listening on https://{addr}");
            let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
                .await
                .expect("failed to load TLS cert/key");
            axum_server::bind_rustls(addr, config)
                .serve(app.into_make_service())
                .await
                .expect("server error");
        }
        _ => {
            tracing::info!("attune-server listening on http://{addr}");
            let listener = tokio::net::TcpListener::bind(&addr)
                .await
                .expect("bind failed");
            axum::serve(listener, app).await.expect("server error");
        }
    }
}
