//! 集成测试:HttpCapacityClient 对一个 mock k3-scheduler `/capacity` 端点真发 HTTP。
//!
//! 用 `std::net::TcpListener` 起一次性 HTTP server（零新依赖,同 catalog_integration_test
//! 模式）,验证 attune 端真解析 scheduler 容量信号 + 失败降级路径(§1.6 离线,无真设备)。
//!
//! 真机 load-aware（本地真忙 → 真溢出云）= §7.3 PENDING(K3 真设备,本机非测试环境)。

use attune_core::edge_cloud::{CapacityProbe, CapacityState, HttpCapacityClient};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

/// 起一次性 HTTP server,对任意请求返回固定 `body`(application/json)。返回 base_url。
fn spawn_mock_scheduler(body: &'static str, status_line: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        // 服务两个请求(够多个 query),然后退出。
        for _ in 0..4 {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let resp = format!(
                    "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        }
    });
    // 给 server 线程一点起步时间。
    thread::sleep(Duration::from_millis(50));
    format!("http://{addr}")
}

#[test]
fn parses_ready_fast_signal() {
    let base = spawn_mock_scheduler(
        r#"{"state":"READY_FAST","eta_ms":0,"mem_headroom_mb":8192}"#,
        "HTTP/1.1 200 OK",
    );
    let client = HttpCapacityClient::with_base(&base, Duration::from_secs(2));
    let sig = client.query("qwen2.5-7b");
    assert_eq!(sig.state, CapacityState::ReadyFast);
    assert_eq!(sig.mem_headroom_mb, 8192);
}

#[test]
fn parses_queued_signal_with_eta() {
    let base = spawn_mock_scheduler(
        r#"{"state":"QUEUED","eta_ms":3500,"mem_headroom_mb":2048}"#,
        "HTTP/1.1 200 OK",
    );
    let client = HttpCapacityClient::with_base(&base, Duration::from_secs(2));
    let sig = client.query("qwen2.5-7b");
    assert_eq!(sig.state, CapacityState::Queued);
    assert_eq!(sig.eta_ms, 3500);
}

#[test]
fn non_2xx_degrades_to_unknown() {
    let base = spawn_mock_scheduler(r#"{"error":"boom"}"#, "HTTP/1.1 503 Service Unavailable");
    let client = HttpCapacityClient::with_base(&base, Duration::from_secs(2));
    let sig = client.query("x");
    // 非 2xx → Unknown 降级(不崩)。
    assert_eq!(sig.state, CapacityState::Unknown);
}

#[test]
fn malformed_json_degrades_to_unknown() {
    let base = spawn_mock_scheduler("not json at all {{{", "HTTP/1.1 200 OK");
    let client = HttpCapacityClient::with_base(&base, Duration::from_secs(2));
    let sig = client.query("x");
    assert_eq!(sig.state, CapacityState::Unknown);
}

#[test]
fn unreachable_endpoint_degrades_to_unknown_not_panic() {
    // 指向一个没人监听的端口 → 连接拒/超时 → Unknown 降级(绝不崩)。
    let client = HttpCapacityClient::with_base("http://127.0.0.1:1", Duration::from_millis(300));
    let sig = client.query("x");
    assert_eq!(sig.state, CapacityState::Unknown);
    assert_eq!(sig.eta_ms, 0);
}
