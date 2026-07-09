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
                apply_admission_hints(
                    &mut body,
                    task.timeout_ms,
                    task.deadline_ms,
                    None,
                    ctx.context_tokens,
                    ctx.max_output_tokens,
                );
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
                apply_admission_hints(
                    &mut body,
                    task.timeout_ms,
                    task.deadline_ms,
                    ctx.ttl_ms,
                    ctx.context_tokens,
                    ctx.max_output_tokens,
                );
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
    ] {
        if body.contains_key(key) {
            return Err(VaultError::InvalidInput(format!(
                "scheduler KB task {task_name} must not set scheduler field: {key}"
            )));
        }
    }
    Ok(())
}

fn apply_admission_hints(
    body: &mut Map<String, Value>,
    timeout_ms: u32,
    deadline_ms: u32,
    ttl_ms: Option<u32>,
    context_tokens: u32,
    max_output_tokens: u32,
) {
    if timeout_ms > 0 {
        insert_if_absent(body, "timeout_ms", timeout_ms);
    }
    if deadline_ms > 0 {
        insert_if_absent(body, "deadline_ms", deadline_ms);
    }
    if let Some(ttl_ms) = ttl_ms {
        insert_if_absent(body, "ttl_ms", ttl_ms);
    }
    body.insert("context_tokens".to_string(), Value::from(context_tokens));
    body.insert(
        "max_output_tokens".to_string(),
        Value::from(max_output_tokens),
    );
}

fn insert_if_absent(body: &mut Map<String, Value>, key: &str, value: u32) {
    body.entry(key.to_string())
        .or_insert_with(|| Value::from(value));
}

fn submit_local_task(
    client: &LocalSchedulerClient,
    task_name: &str,
    body: &Map<String, Value>,
    explicit_async: bool,
    admission: SchedulerKbTaskAdmission,
) -> Result<SchedulerKbTaskSubmitOutcome> {
    let response = client.submit_kb_task(task_name, body, explicit_async)?;
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
