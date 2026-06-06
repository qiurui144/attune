//! attune-server-headless — 纯 axum 模式入口（K3 / NAS / 服务器）。
//!
//! 笔电桌面用户走 attune-desktop（含 Tauri WebView 壳）。
//! 两者共享 attune_server::run_in_runtime() 后端逻辑。

use attune_server::{run_in_runtime, ServerConfig};
use clap::Parser;

#[derive(Parser)]
#[command(name = "attune-server-headless", version, about = "Attune HTTP API server (headless mode)")]
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
    /// 一键化部署: 启动前下载 PP-OCR 必需模型 (~16 MB)，下载完后继续启动 server。
    /// 标准应用安装时跑此 flag (postinst.sh 调用)，cargo/源码部署也用此一次性补齐。
    /// HF_ENDPOINT=https://hf-mirror.com 加速 CN 镜像。
    #[arg(long)]
    bootstrap_models: bool,
    /// 仅下载模型, 完成后退出（不启动 server）。适合 CI / postinst 场景。
    #[arg(long)]
    bootstrap_only: bool,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // 一键化部署: bootstrap models 路径 (postinst.sh 或开发者首次部署)
    if cli.bootstrap_models || cli.bootstrap_only {
        eprintln!("=== Attune bootstrap-models: 下载必需模型 ===");
        // B4 (2026-06-06): ensure_models_downloaded() uses reqwest::blocking, whose
        // embedded current-thread runtime panics on drop inside this #[tokio::main]
        // async context ("Cannot drop a runtime ..."). postinst.sh runs --bootstrap-models
        // on every install, so this crashed fresh installs. Run it on a blocking thread.
        let bootstrap = tokio::task::spawn_blocking(
            attune_core::ocr::ppocr::PpOcrProvider::ensure_models_downloaded,
        )
        .await
        .unwrap_or_else(|e| {
            Err(attune_core::error::VaultError::ModelLoad(format!(
                "bootstrap task join error: {e}"
            )))
        });
        match bootstrap {
            Ok(()) => eprintln!("✓ PP-OCR models ready"),
            Err(e) => {
                eprintln!("✗ PP-OCR bootstrap failed: {e}");
                eprintln!("  CN 镜像: HF_ENDPOINT=https://hf-mirror.com {} --bootstrap-only",
                    std::env::current_exe()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| "attune-server-headless".into()));
                std::process::exit(1);
            }
        }
        if cli.bootstrap_only {
            eprintln!("=== bootstrap-only 完成 ===");
            return;
        }
        eprintln!("=== bootstrap 完成, 继续启动 server ===");
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
