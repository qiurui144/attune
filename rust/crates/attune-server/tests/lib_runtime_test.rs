//! 测试 attune_server::run_in_runtime() 高阶 API
//!
//! 验证：lib 暴露的高阶启动函数可以独立运行（无需 Cli），让 attune-desktop
//! 能直接 spawn 它而不依赖 binary。

use attune_server::ServerConfig;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_in_runtime_starts_and_responds_on_health() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let config = ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        tls_cert: None,
        tls_key: None,
        no_auth: true,
    };

    let handle = tokio::spawn(async move { attune_server::run_in_runtime(config).await });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let response = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .expect("server should be reachable");
    assert_eq!(response.status(), 200, "health endpoint should return 200");

    handle.abort();
}
