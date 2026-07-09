//! Tests for the Local scheduler KB task adapter over a mock scheduler endpoint.

use attune_core::context_admission::AdmissionReason;
use attune_core::edge_cloud::{
    SchedulerKbTaskAdapter, SchedulerKbTaskSubmitOutcome, SchedulerKbTaskSubmitRequest, LocalSchedulerClient,
    RuntimeProfileResolver,
};
use attune_core::llm::ChatMessage;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
struct RecordedRequest {
    path: String,
    body: Value,
}

fn spawn_one_request_scheduler(
    status_line: &'static str,
    response_body: &'static str,
) -> (String, Arc<Mutex<Option<RecordedRequest>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().unwrap();
    let recorded = Arc::new(Mutex::new(None));
    let record_slot = recorded.clone();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let request = read_http_request(&mut stream);
            let first_line = request.lines().next().unwrap_or_default();
            let path = first_line
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            let body = request
                .split_once("\r\n\r\n")
                .map(|(_, body)| serde_json::from_str(body).unwrap_or(Value::Null))
                .unwrap_or(Value::Null);
            *record_slot.lock().unwrap() = Some(RecordedRequest { path, body });

            let resp = format!(
                "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    thread::sleep(Duration::from_millis(50));
    (format!("http://{addr}"), recorded)
}

fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let n = stream.read(&mut tmp).unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(header_end) = find_header_end(&buf) {
            let headers = String::from_utf8_lossy(&buf[..header_end]);
            let content_len = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length:")
                        .or_else(|| line.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if buf.len() >= header_end + 4 + content_len {
                break;
            }
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

#[test]
fn async_only_task_submits_explicit_async_with_admission_hints() {
    let profiles = RuntimeProfileResolver::static_local_scheduler_profile("");
    let (base, recorded) = spawn_one_request_scheduler(
        "HTTP/1.1 202 Accepted",
        r#"{"job_id":"job_ask","status":"queued","task":"kb.query.ask","model":"llm-summary","service_class":"realtime_answer","scheduled_as":"async","reason":"task_async_only","eta_ms":1200}"#,
    );
    let client = LocalSchedulerClient::with_base(&base, Duration::from_secs(2));
    let adapter = SchedulerKbTaskAdapter::new(&client, &profiles);
    let messages = vec![ChatMessage::user("question with compact evidence")];

    let outcome = adapter
        .submit(SchedulerKbTaskSubmitRequest::interactive(
            "kb.query.ask",
            json!({"query":"q","contexts":["cited context"]}),
            &messages,
        ))
        .unwrap();

    match outcome {
        SchedulerKbTaskSubmitOutcome::Local(local) => {
            assert!(local.explicit_async);
            assert_eq!(local.response.job_id.as_deref(), Some("job_ask"));
            assert_eq!(local.admission.reason, AdmissionReason::TaskAsyncOnly);
            assert_eq!(local.admission.max_output_tokens, 128);
        }
        other => panic!("expected local async outcome, got {other:?}"),
    }

    let req = recorded.lock().unwrap().clone().expect("request recorded");
    assert_eq!(req.path, "/kb/tasks/kb.query.ask:async");
    assert_eq!(req.body["max_output_tokens"], 128);
    assert!(req.body["context_tokens"].as_u64().unwrap() > 0);
    assert_eq!(req.body["timeout_ms"], 120000);
    assert_eq!(req.body["deadline_ms"], 15000);
    assert_eq!(req.body["ttl_ms"], 900000);
    assert!(req.body.get("service_class").is_none());
    assert!(req.body.get("model").is_none());
}

#[test]
fn sync_capable_task_uses_primary_endpoint_with_hints() {
    let profiles = RuntimeProfileResolver::static_local_scheduler_profile("");
    let (base, recorded) = spawn_one_request_scheduler(
        "HTTP/1.1 200 OK",
        r#"{"scheduled_as":"sync","task":"kb.query.vlm_extract","model":"vlm","service_class":"realtime_vlm_compact","outputs":{"text":"ok"},"latency_ms":1800.0}"#,
    );
    let client = LocalSchedulerClient::with_base(&base, Duration::from_secs(2));
    let adapter = SchedulerKbTaskAdapter::new(&client, &profiles);
    let messages = vec![ChatMessage::user("extract this short document image")];

    let outcome = adapter
        .submit(SchedulerKbTaskSubmitRequest::interactive(
            "kb.query.vlm_extract",
            json!({"prompt":"extract fields","image_url":"data:image/jpeg;base64,abc"}),
            &messages,
        ))
        .unwrap();

    match outcome {
        SchedulerKbTaskSubmitOutcome::Local(local) => {
            assert!(!local.explicit_async);
            assert_eq!(local.response.scheduled_as, "sync");
            assert_eq!(local.response.outputs["text"].as_str(), Some("ok"));
            assert_eq!(local.admission.reason, AdmissionReason::FitsSync);
            assert_eq!(local.admission.max_output_tokens, 256);
        }
        other => panic!("expected local sync outcome, got {other:?}"),
    }

    let req = recorded.lock().unwrap().clone().expect("request recorded");
    assert_eq!(req.path, "/kb/tasks/kb.query.vlm_extract");
    assert_eq!(req.body["max_output_tokens"], 256);
    assert!(req.body["context_tokens"].as_u64().unwrap() > 0);
    assert_eq!(req.body["timeout_ms"], 120000);
    assert_eq!(req.body["deadline_ms"], 20000);
}

#[test]
fn local_async_overflow_returns_cloud_fallback_without_scheduler_call() {
    let profiles = RuntimeProfileResolver::static_local_scheduler_profile("");
    let client = LocalSchedulerClient::with_base("http://127.0.0.1:1", Duration::from_millis(50));
    let adapter = SchedulerKbTaskAdapter::new(&client, &profiles);
    let messages = vec![ChatMessage::user(&"长".repeat(8000))];

    let outcome = adapter
        .submit(SchedulerKbTaskSubmitRequest::interactive(
            "kb.query.answer",
            json!({"query":"q","contexts":["huge"]}),
            &messages,
        ))
        .unwrap();

    match outcome {
        SchedulerKbTaskSubmitOutcome::UseCloudIfAllowed(ctx) => {
            assert_eq!(ctx.reason, AdmissionReason::ContextTooLargeForLocalAsync);
            assert_eq!(ctx.model_id, "llm-summary");
        }
        other => panic!("expected cloud fallback decision, got {other:?}"),
    }
}

#[test]
fn forbidden_scheduler_fields_are_rejected_before_http() {
    let profiles = RuntimeProfileResolver::static_local_scheduler_profile("");
    let client = LocalSchedulerClient::with_base("http://127.0.0.1:1", Duration::from_millis(50));
    let adapter = SchedulerKbTaskAdapter::new(&client, &profiles);
    let messages = vec![ChatMessage::user("q")];

    let err = adapter
        .submit(SchedulerKbTaskSubmitRequest::interactive(
            "kb.query.ask",
            json!({"query":"q","model":"llm-chat"}),
            &messages,
        ))
        .unwrap_err();

    assert!(err.to_string().contains("must not set scheduler field"));
}

#[test]
fn non_object_body_is_rejected_before_http() {
    let profiles = RuntimeProfileResolver::static_local_scheduler_profile("");
    let client = LocalSchedulerClient::with_base("http://127.0.0.1:1", Duration::from_millis(50));
    let adapter = SchedulerKbTaskAdapter::new(&client, &profiles);
    let messages = vec![ChatMessage::user("q")];

    let err = adapter
        .submit(SchedulerKbTaskSubmitRequest::interactive(
            "kb.query.ask",
            json!(["not", "object"]),
            &messages,
        ))
        .unwrap_err();

    assert!(err.to_string().contains("must be a JSON object"));
}
