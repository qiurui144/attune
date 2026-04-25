//! 在 Tauri 主进程的 tokio runtime 上跑 attune-server。
//!
//! 启动顺序：
//! 1. spawn 后台 task → run_in_runtime
//! 2. 健康检查轮询 :18900/health 直到 200（30s 超时）
//! 3. 通知 Tauri 主线程加载 WebView URL

use attune_server::{run_in_runtime, ServerConfig};
use std::time::Duration;

const SERVER_HOST: &str = "127.0.0.1";
const SERVER_PORT: u16 = 18900;
const HEALTH_TIMEOUT_SECS: u64 = 30;

pub fn server_url() -> String {
    format!("http://{}:{}", SERVER_HOST, SERVER_PORT)
}

/// Spawn attune-server 在 Tauri 的 tokio runtime。
pub fn spawn_server() -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async {
        let config = ServerConfig {
            host: SERVER_HOST.to_string(),
            port: SERVER_PORT,
            tls_cert: None,
            tls_key: None,
            no_auth: false,
        };
        if let Err(e) = run_in_runtime(config).await {
            tracing::error!("embedded attune-server crashed: {e}");
        }
    })
}

/// 阻塞等 :18900/health 返回 200。
pub async fn wait_for_ready() -> Result<(), String> {
    let url = format!("{}/health", server_url());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|e| e.to_string())?;

    let deadline = std::time::Instant::now() + Duration::from_secs(HEALTH_TIMEOUT_SECS);
    while std::time::Instant::now() < deadline {
        match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => {
                tracing::info!("embedded attune-server ready at {}", server_url());
                return Ok(());
            }
            _ => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
    Err(format!(
        "attune-server did not become ready within {}s",
        HEALTH_TIMEOUT_SECS
    ))
}
