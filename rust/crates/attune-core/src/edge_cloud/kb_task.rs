//! local scheduler application-task adapter.
//!
//! This layer connects runtime profiles and ContextAdmission to the scheduler's
//! `/kb/tasks/{task}` API. It never performs cloud egress; cloud fallback remains
//! an upper-layer policy decision guarded by Attune privacy/outbound controls.

use crate::context_admission::{
    admit_context, AdmissionLatencyClass, AdmissionReason, AdmissionRejection,
    CloudFallbackContext, ContextAdmissionDecision, ContextAdmissionRequest,
};
use crate::edge_cloud::scheduler::{LocalSchedulerClient, SchedulerKbTaskResponse};
use crate::edge_cloud::RuntimeProfileSet;
use crate::error::{Result, VaultError};
use crate::llm::ChatMessage;
use serde_json::{Map, Value};

pub struct SchedulerKbTaskAdapter<'a> {
    client: &'a LocalSchedulerClient,
    profiles: &'a RuntimeProfileSet,
}

impl<'a> SchedulerKbTaskAdapter<'a> {
    pub fn new(client: &'a LocalSchedulerClient, profiles: &'a RuntimeProfileSet) -> Self {
        SchedulerKbTaskAdapter { client, profiles }
    }

    pub fn submit(
        &self,
        req: SchedulerKbTaskSubmitRequest<'_>,
    ) -> Result<SchedulerKbTaskSubmitOutcome> {
        let task = self.profiles.task(req.task_name).ok_or_else(|| {
            VaultError::InvalidInput(format!("unknown scheduler runtime task: {}", req.task_name))
        })?;
        let runtime = self.profiles.task_model(req.task_name).ok_or_else(|| {
            VaultError::InvalidInput(format!(
                "missing scheduler runtime model profile for task: {}",
                req.task_name
            ))
        })?;

        let mut body = body_object(req.body)?;
        reject_forbidden_scheduler_fields(req.task_name, &body)?;
        let desired_output_tokens = match req.desired_output_tokens {
            Some(tokens) => Some(tokens),
            None => body_u32(&body, "max_output_tokens").transpose()?,
        };

        let admission_req = ContextAdmissionRequest {
            messages: req.admission_messages,
            runtime,
            task: Some(task),
            desired_output_tokens,
            latency_class: req.latency_class,
        };
        let admission = admit_context(admission_req);
        match admission {
            ContextAdmissionDecision::AdmitSync(ctx) => {
                apply_output_limit(&mut body, ctx.max_output_tokens);
                // Scheduler v0.8.2+ rejects app-set fields; kb.query.ask manages limits internally.
                if req.task_name == "kb.query.ask" {
                    body.remove("max_output_tokens");
                }
                submit_local_task(
                    self.client,
                    req.task_name,
                    &body,
                    false,
                    SchedulerKbTaskAdmission {
                        task_name: req.task_name.to_string(),
                        model_id: ctx.model_id,
                        service_class: ctx.service_class,
                        context_tokens: ctx.context_tokens,
                        max_output_tokens: ctx.max_output_tokens,
                        reason: ctx.reason,
                    },
                )
            }
            ContextAdmissionDecision::SubmitAsync(ctx) => {
                apply_output_limit(&mut body, ctx.max_output_tokens);
                // Scheduler v0.8.2+ rejects app-set fields; kb.query.ask manages limits internally.
                if req.task_name == "kb.query.ask" {
                    body.remove("max_output_tokens");
                }
                submit_local_task(
                    self.client,
                    req.task_name,
                    &body,
                    true,
                    SchedulerKbTaskAdmission {
                        task_name: req.task_name.to_string(),
                        model_id: ctx.model_id,
                        service_class: ctx.service_class,
                        context_tokens: ctx.context_tokens,
                        max_output_tokens: ctx.max_output_tokens,
                        reason: ctx.reason,
                    },
                )
            }
            ContextAdmissionDecision::UseCloudIfAllowed(ctx) => {
                Ok(SchedulerKbTaskSubmitOutcome::UseCloudIfAllowed(ctx))
            }
            ContextAdmissionDecision::Reject(ctx) => Ok(SchedulerKbTaskSubmitOutcome::Reject(ctx)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SchedulerKbTaskSubmitRequest<'a> {
    pub task_name: &'a str,
    pub body: Value,
    pub admission_messages: &'a [ChatMessage],
    pub desired_output_tokens: Option<u32>,
    pub latency_class: AdmissionLatencyClass,
}

impl<'a> SchedulerKbTaskSubmitRequest<'a> {
    pub fn interactive(
        task_name: &'a str,
        body: Value,
        admission_messages: &'a [ChatMessage],
    ) -> Self {
        SchedulerKbTaskSubmitRequest {
            task_name,
            body,
            admission_messages,
            desired_output_tokens: None,
            latency_class: AdmissionLatencyClass::Interactive,
        }
    }

    pub fn with_desired_output_tokens(mut self, tokens: u32) -> Self {
        self.desired_output_tokens = Some(tokens);
        self
    }

    pub fn background(mut self) -> Self {
        self.latency_class = AdmissionLatencyClass::Background;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SchedulerKbTaskSubmitOutcome {
    Local(SchedulerKbTaskLocalOutcome),
    UseCloudIfAllowed(CloudFallbackContext),
    Reject(AdmissionRejection),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SchedulerKbTaskLocalOutcome {
    pub response: SchedulerKbTaskResponse,
    pub explicit_async: bool,
    pub admission: SchedulerKbTaskAdmission,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerKbTaskAdmission {
    pub task_name: String,
    pub model_id: String,
    pub service_class: String,
    pub context_tokens: u32,
    pub max_output_tokens: u32,
    pub reason: AdmissionReason,
}

fn body_object(body: Value) -> Result<Map<String, Value>> {
    match body {
        Value::Object(map) => Ok(map),
        _ => Err(VaultError::InvalidInput(
            "scheduler KB task body must be a JSON object".to_string(),
        )),
    }
}

fn reject_forbidden_scheduler_fields(task_name: &str, body: &Map<String, Value>) -> Result<()> {
    for key in [
        "model",
        "priority",
        "resource_key",
        "preferred_device",
        "service_class",
        "timeout_ms",
        "context_tokens",
        "deadline_ms",
        "prompt_tokens",
        "ttl_ms",
    ] {
        if body.contains_key(key) {
            return Err(VaultError::InvalidInput(format!(
                "scheduler KB task {task_name} must not set scheduler field: {key}"
            )));
        }
    }
    Ok(())
}

fn apply_output_limit(body: &mut Map<String, Value>, max_output_tokens: u32) {
    body.insert(
        "max_output_tokens".to_string(),
        Value::from(max_output_tokens),
    );
}

fn submit_local_task(
    client: &LocalSchedulerClient,
    task_name: &str,
    body: &Map<String, Value>,
    explicit_async: bool,
    admission: SchedulerKbTaskAdmission,
) -> Result<SchedulerKbTaskSubmitOutcome> {
    let response = client.submit_kb_task(task_name, body, explicit_async)?;
    response.validate_submission(explicit_async, &format!("scheduler KB task {task_name}"))?;
    Ok(SchedulerKbTaskSubmitOutcome::Local(
        SchedulerKbTaskLocalOutcome {
            response,
            explicit_async,
            admission,
        },
    ))
}

fn body_u32(body: &Map<String, Value>, key: &str) -> Option<Result<u32>> {
    let value = body.get(key)?;
    Some(
        value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| VaultError::InvalidInput(format!("{key} must be a u32"))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge_cloud::RuntimeProfileResolver;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn adapter_records_context_tokens_but_does_not_send_scheduler_field() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            let mut header_end = None;
            while header_end.is_none() {
                let n = stream.read(&mut tmp).unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                header_end = buf.windows(4).position(|w| w == b"\r\n\r\n");
            }
            let header_end = header_end.map(|idx| idx + 4).unwrap();
            let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.eq_ignore_ascii_case("content-length") {
                        value.trim().parse::<usize>().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            while buf.len().saturating_sub(header_end) < content_length {
                let n = stream.read(&mut tmp).unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            let body = String::from_utf8_lossy(&buf[header_end..]).to_string();
            tx.send(body).unwrap();
            let payload = serde_json::json!({
                "scheduled_as": "async",
                "job_id": "job-inline-1",
                "status": "done",
                "task": "kb.query.ask",
                "model": "llm-summary",
                "service_class": "realtime_answer",
                "outputs": {"text": "ok"}
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            stream.write_all(response.as_bytes()).unwrap();
        });

        let base = format!("http://{addr}");
        let client = LocalSchedulerClient::with_base(&base, Duration::from_secs(5));
        let profiles = RuntimeProfileResolver::static_local_scheduler_profile(&base);
        let adapter = SchedulerKbTaskAdapter::new(&client, &profiles);
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: "A320 hydraulic source?".to_string(),
        }];

        let outcome = adapter
            .submit(
                SchedulerKbTaskSubmitRequest::interactive(
                    "kb.query.ask",
                    serde_json::json!({
                        "query": "A320 hydraulic source?",
                        "contexts": [{"text": "blue pump source", "source": "A320-Hydraulic.pdf"}],
                    }),
                    &messages,
                )
                .with_desired_output_tokens(24),
            )
            .unwrap();
        let SchedulerKbTaskSubmitOutcome::Local(local) = outcome else {
            panic!("expected local scheduler outcome");
        };
        assert!(local.admission.context_tokens > 0);
        assert_eq!(local.admission.max_output_tokens, 24);

        let sent: Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
        assert!(sent.get("context_tokens").is_none(), "sent={sent}");
        assert!(sent.get("deadline_ms").is_none(), "sent={sent}");
        assert!(sent.get("timeout_ms").is_none(), "sent={sent}");
        assert!(sent.get("ttl_ms").is_none(), "sent={sent}");
        assert!(sent.get("max_output_tokens").is_none(), "sent={sent}");
        handle.join().unwrap();
    }

    #[test]
    fn adapter_rejects_malformed_or_failed_200_submission() {
        for (scheduled_as, response_status, response_error, expected) in [
            ("sync", "queued", None, "without job_id"),
            ("sync", "running", None, "without job_id"),
            ("sync", "future_scheduler_state", None, "without job_id"),
            ("deferred", "done", None, "without job_id"),
            ("sync", "error", Some("OOM"), "OOM"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let handle = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request);
                let payload = serde_json::json!({
                    "scheduled_as": scheduled_as,
                    "status": response_status,
                    "error": response_error,
                    "task": "kb.query.ask",
                    "outputs": {}
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    payload.len(),
                    payload
                );
                stream.write_all(response.as_bytes()).unwrap();
            });

            let base = format!("http://{addr}");
            let client = LocalSchedulerClient::with_base(&base, Duration::from_secs(5));
            let profiles = RuntimeProfileResolver::static_local_scheduler_profile(&base);
            let adapter = SchedulerKbTaskAdapter::new(&client, &profiles);
            let messages = vec![ChatMessage {
                role: "user".to_string(),
                content: "test".to_string(),
            }];
            let error = adapter
                .submit(SchedulerKbTaskSubmitRequest::interactive(
                    "kb.query.ask",
                    serde_json::json!({"query": "test", "contexts": []}),
                    &messages,
                ))
                .expect_err("malformed/failed 200 response must fail closed");
            assert!(
                error.to_string().contains(expected),
                "scheduled_as={scheduled_as}, status={response_status}, error={error}"
            );
            handle.join().unwrap();
        }
    }

    #[test]
    fn adapter_rejects_app_supplied_context_tokens() {
        let profiles = RuntimeProfileResolver::static_local_scheduler_profile("http://127.0.0.1:1");
        let client =
            LocalSchedulerClient::with_base("http://127.0.0.1:1", Duration::from_millis(50));
        let adapter = SchedulerKbTaskAdapter::new(&client, &profiles);
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: "test".to_string(),
        }];
        let err = adapter
            .submit(SchedulerKbTaskSubmitRequest::interactive(
                "kb.query.ask",
                serde_json::json!({"query": "test", "context_tokens": 12}),
                &messages,
            ))
            .unwrap_err();
        assert!(
            err.to_string().contains("context_tokens"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn adapter_rejects_app_supplied_deadline_ms() {
        let profiles = RuntimeProfileResolver::static_local_scheduler_profile("http://127.0.0.1:1");
        let client =
            LocalSchedulerClient::with_base("http://127.0.0.1:1", Duration::from_millis(50));
        let adapter = SchedulerKbTaskAdapter::new(&client, &profiles);
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: "test".to_string(),
        }];
        let err = adapter
            .submit(SchedulerKbTaskSubmitRequest::interactive(
                "kb.query.ask",
                serde_json::json!({"query": "test", "deadline_ms": 1000}),
                &messages,
            ))
            .unwrap_err();
        assert!(
            err.to_string().contains("deadline_ms"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn adapter_rejects_app_supplied_prompt_tokens() {
        let profiles = RuntimeProfileResolver::static_local_scheduler_profile("http://127.0.0.1:1");
        let client =
            LocalSchedulerClient::with_base("http://127.0.0.1:1", Duration::from_millis(50));
        let adapter = SchedulerKbTaskAdapter::new(&client, &profiles);
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: "test".to_string(),
        }];
        let err = adapter
            .submit(SchedulerKbTaskSubmitRequest::interactive(
                "kb.query.ask",
                serde_json::json!({"query": "test", "prompt_tokens": 100}),
                &messages,
            ))
            .unwrap_err();
        assert!(
            err.to_string().contains("prompt_tokens"),
            "unexpected error: {err}"
        );
    }
}
