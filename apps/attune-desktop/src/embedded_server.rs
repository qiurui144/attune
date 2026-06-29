//! 在 Tauri 主进程的 tokio runtime 上跑 attune-server。
//!
//! 启动顺序：
//! 1. spawn 后台 task → run_in_runtime
//! 2. 健康检查轮询 server_url()/health 直到 200（30s 超时）
//! 3. 通知 Tauri 主线程加载 WebView URL

use attune_server::{run_in_runtime, ServerConfig};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const SERVER_HOST: &str = "127.0.0.1";
const DEFAULT_SERVER_PORT: u16 = 18900;
const HEALTH_TIMEOUT_SECS: u64 = 30;

static SERVER_ERROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn server_error_slot() -> &'static Mutex<Option<String>> {
    SERVER_ERROR.get_or_init(|| Mutex::new(None))
}

pub fn server_port() -> u16 {
    std::env::var("ATTUNE_DESKTOP_PORT")
        .ok()
        .and_then(|raw| raw.parse::<u16>().ok())
        .unwrap_or(DEFAULT_SERVER_PORT)
}

pub fn server_url() -> String {
    format!("http://{}:{}", SERVER_HOST, server_port())
}

/// Spawn attune-server 在 Tauri 的 tokio runtime。
pub fn spawn_server() -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async {
        let config = ServerConfig {
            host: SERVER_HOST.to_string(),
            port: server_port(),
            tls_cert: None,
            tls_key: None,
            no_auth: false,
        };
        tracing::info!("embedded attune-server starting at {}", server_url());
        if let Err(e) = run_in_runtime(config).await {
            let msg = e.to_string();
            if let Ok(mut slot) = server_error_slot().lock() {
                *slot = Some(msg.clone());
            }
            tracing::error!("embedded attune-server crashed: {msg}");
        }
    })
}

/// 阻塞等 server_url()/health 返回 200。
pub async fn wait_for_ready() -> Result<(), String> {
    let url = format!("{}/health", server_url());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|e| e.to_string())?;

    let deadline = std::time::Instant::now() + Duration::from_secs(HEALTH_TIMEOUT_SECS);
    while std::time::Instant::now() < deadline {
        if let Ok(slot) = server_error_slot().lock() {
            if let Some(e) = slot.as_ref() {
                return Err(format!("attune-server exited before readiness: {e}"));
            }
        }
        match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => {
                tracing::info!("embedded attune-server ready at {}", server_url());
                return Ok(());
            }
            _ => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
    Err(format!(
        "attune-server did not become ready within {}s; health url={url}",
        HEALTH_TIMEOUT_SECS,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn with_desktop_port_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let old = std::env::var("ATTUNE_DESKTOP_PORT").ok();
        match value {
            Some(v) => std::env::set_var("ATTUNE_DESKTOP_PORT", v),
            None => std::env::remove_var("ATTUNE_DESKTOP_PORT"),
        }
        let out = f();
        match old {
            Some(v) => std::env::set_var("ATTUNE_DESKTOP_PORT", v),
            None => std::env::remove_var("ATTUNE_DESKTOP_PORT"),
        }
        out
    }

    #[test]
    fn server_port_defaults_to_canonical_port() {
        with_desktop_port_env(None, || {
            assert_eq!(server_port(), 18900);
            assert_eq!(server_url(), "http://127.0.0.1:18900");
        });
    }

    #[test]
    fn server_port_uses_valid_env_override() {
        with_desktop_port_env(Some("19090"), || {
            assert_eq!(server_port(), 19090);
            assert_eq!(server_url(), "http://127.0.0.1:19090");
        });
    }

    #[test]
    fn server_port_ignores_invalid_env_override() {
        with_desktop_port_env(Some("not-a-port"), || {
            assert_eq!(server_port(), 18900);
        });
    }
}
