//! attune-server-headless — 纯 axum 模式入口（local-scheduler / NAS / 服务器）。
//!
//! 笔电桌面用户走 attune-desktop（含 Tauri WebView 壳）。
//! 两者共享 attune_server::run_in_runtime() 后端逻辑。

use attune_server::{run_in_runtime, ServerConfig};
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "attune-server-headless",
    version,
    about = "Attune HTTP API server (headless mode)"
)]
struct Cli {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value = "18900")]
    port: u16,
    #[arg(long)]
    tls_cert: Option<String>,
    #[arg(long)]
    tls_key: Option<String>,
    #[arg(long)]
    no_auth: bool,
    /// Legacy compatibility flag. Local model lifecycle is scheduler-owned.
    #[arg(long)]
    bootstrap_models: bool,
    /// Legacy compatibility flag. Exits successfully after reporting scheduler ownership.
    #[arg(long)]
    bootstrap_only: bool,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if cli.bootstrap_models || cli.bootstrap_only {
        eprintln!("=== Attune bootstrap-models: local model lifecycle is scheduler-owned ===");
        eprintln!("Attune no longer downloads OCR/ASR/embedding/rerank assets directly.");
        if cli.bootstrap_only {
            eprintln!("=== bootstrap-only 完成 ===");
            return;
        }
        eprintln!("=== bootstrap compatibility step complete, starting server ===");
    }

    let config = ServerConfig {
        host: cli.host,
        port: cli.port,
        tls_cert: cli.tls_cert,
        tls_key: cli.tls_key,
        no_auth: cli.no_auth,
    };
    if let Err(e) = run_in_runtime(config).await {
        eprintln!("attune-server-headless exited with error: {e}");
        std::process::exit(1);
    }
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
