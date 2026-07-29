//! 集成测试:HttpCapacityClient 对 mock local scheduler 真发 HTTP。
//!
//! 用 `std::net::TcpListener` 起一次性 HTTP server（零新依赖,同 catalog_integration_test
//! 模式）,验证 attune 端按真实 `/models` + `/capacity` schema 派生容量信号 +
//! 失败降级路径(§1.6 离线,无真设备)。
//!
//! 真机 load-aware（本地真忙 → 真溢出云）= §7.3 PENDING(本地调度器真设备,本机非测试环境)。

use attune_core::edge_cloud::{CapacityProbe, CapacityState, HttpCapacityClient};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy)]
struct MockResponse {
    status_line: &'static str,
    body: &'static str,
}

fn ok(body: &'static str) -> MockResponse {
    MockResponse {
        status_line: "HTTP/1.1 200 OK",
        body,
    }
}

fn response(status_line: &'static str, body: &'static str) -> MockResponse {
    MockResponse { status_line, body }
}

/// 起一次性 HTTP server,按 path 返回 `/models` 或 `/capacity`。返回 base_url。
fn spawn_mock_scheduler(models: MockResponse, capacity: MockResponse) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        // 每个 query 最多两个请求；多留一点给失败路径和平台重试。
        for _ in 0..8 {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or_default();
                let resp = if first_line.starts_with("GET /models ") {
                    models
                } else if first_line.starts_with("GET /capacity ") {
                    capacity
                } else {
                    response("HTTP/1.1 404 Not Found", r#"{"error":"not_found"}"#)
                };
                let resp = format!(
                    "{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp.status_line,
                    resp.body.len(),
                    resp.body
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
fn derives_ready_fast_signal_from_models_and_capacity() {
    let base = spawn_mock_scheduler(
        ok(
            r#"{"models":[{"name":"llm-chat","state":"READY_FAST","queue_depth":0,"queue_capacity":4,"p50_latency_ms":1200.0}],"revision":7}"#,
        ),
        ok(r#"{"memory":{"available_gb":8.0},"revision":7}"#),
    );
    let client = HttpCapacityClient::with_base(&base, Duration::from_secs(2));
    let sig = client.query("llm-chat");
    assert_eq!(sig.state, CapacityState::ReadyFast);
    assert_eq!(sig.eta_ms, 0);
    assert_eq!(sig.mem_headroom_mb, 8192);
}

#[test]
fn derives_queued_eta_from_queue_depth_and_latency_sample() {
    let base = spawn_mock_scheduler(
        ok(
            r#"{"models":[{"name":"llm-chat","state":"QUEUED","queue_depth":3,"queue_capacity":4,"p50_latency_ms":250.0}],"revision":7}"#,
        ),
        ok(r#"{"memory":{"available_gb":2.0},"revision":7}"#),
    );
    let client = HttpCapacityClient::with_base(&base, Duration::from_secs(2));
    let sig = client.query("llm-chat");
    assert_eq!(sig.state, CapacityState::Queued);
    assert_eq!(sig.eta_ms, 750);
    assert_eq!(sig.mem_headroom_mb, 2048);
}

#[test]
fn queued_without_latency_uses_conservative_queue_slots() {
    let base = spawn_mock_scheduler(
        ok(
            r#"{"models":[{"name":"llm-chat","state":"QUEUED","queue_depth":3,"queue_capacity":4}],"revision":7}"#,
        ),
        ok(r#"{"dram_total_gb":32.0,"dram_used_gb":28.0,"revision":7}"#),
    );
    let client = HttpCapacityClient::with_base(&base, Duration::from_secs(2));
    let sig = client.query("llm-chat");
    assert_eq!(sig.state, CapacityState::Queued);
    assert_eq!(sig.eta_ms, 3000);
    assert_eq!(sig.mem_headroom_mb, 4096);
}

#[test]
fn memory_snapshot_failure_keeps_model_state() {
    let base = spawn_mock_scheduler(
        ok(
            r#"{"models":[{"name":"llm-chat","state":"READY_SLOW","queue_depth":0,"queue_capacity":4,"p99_latency_ms":2200.0}],"revision":7}"#,
        ),
        response(
            "HTTP/1.1 503 Service Unavailable",
            r#"{"error":"memory_unavailable"}"#,
        ),
    );
    let client = HttpCapacityClient::with_base(&base, Duration::from_secs(2));
    let sig = client.query("llm-chat");
    assert_eq!(sig.state, CapacityState::ReadySlow);
    assert_eq!(sig.eta_ms, 2200);
    assert_eq!(sig.mem_headroom_mb, 0);
}

#[test]
fn missing_model_degrades_to_unknown() {
    let base = spawn_mock_scheduler(
        ok(r#"{"models":[{"name":"embedding-int8","state":"READY_FAST"}],"revision":7}"#),
        ok(r#"{"memory":{"available_gb":8.0},"revision":7}"#),
    );
    let client = HttpCapacityClient::with_base(&base, Duration::from_secs(2));
    let sig = client.query("llm-chat");
    assert_eq!(sig.state, CapacityState::Unknown);
    assert_eq!(sig.eta_ms, 0);
    assert_eq!(sig.mem_headroom_mb, 0);
}

#[test]
fn non_2xx_models_degrades_to_unknown() {
    let base = spawn_mock_scheduler(
        response("HTTP/1.1 503 Service Unavailable", r#"{"error":"boom"}"#),
        ok(r#"{"memory":{"available_gb":8.0},"revision":7}"#),
    );
    let client = HttpCapacityClient::with_base(&base, Duration::from_secs(2));
    let sig = client.query("x");
    // 非 2xx → Unknown 降级(不崩)。
    assert_eq!(sig.state, CapacityState::Unknown);
}

#[test]
fn malformed_models_json_degrades_to_unknown() {
    let base = spawn_mock_scheduler(
        ok("not json at all {{{"),
        ok(r#"{"memory":{"available_gb":8.0},"revision":7}"#),
    );
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
