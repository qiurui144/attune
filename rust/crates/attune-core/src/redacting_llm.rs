//! [`RedactingLlmProvider`] — a privacy decorator that redacts PII out of every outbound LLM
//! payload and restores placeholders in the response.
//!
//! ## Why this exists (I1 — raw private-doc cloud egress regression of F-17)
//!
//! `chat.rs` redacts the chat outbound (`Redactor::redact_batch`) before the cloud LLM, but the
//! document-intelligence agents (`compare` / `deep_summary` / `chapters`) call `llm.chat(..)` /
//! `chat_with_history(..)` with RAW document content built from the user's private docs — so a
//! contract / record full of phone numbers, emails, and IDs went to DeepSeek verbatim. That is
//! the exact PII-egress F-17 closed for chat, reopened for doc-intel.
//!
//! Rather than thread redaction through the internal prompt-building of three modules (and miss
//! the next call site someone adds), this decorator wraps the `LlmProvider` handed to doc-intel
//! and enforces redaction at the trait boundary every outbound payload flows through:
//!
//! * `chat`, `chat_with_history`, `chat_with_history_opts`, `chat_with_options`,
//!   `chat_multimodal` are the foundational sinks; the higher-level helpers
//!   (`chat_with_format_json`, `chat_with_retry`, `chat_few_shot`) delegate to them, so overriding
//!   the sinks covers every call site.
//! * Each call's `system` + `user` + history are redacted with one `redact_batch` (global-unique
//!   placeholders within the call), sent to the inner provider, and the response is `restore`d so
//!   the caller still sees real values — the PII just never crosses the wire.
//!
//! ## Bound to the privacy toggle (I2)
//!
//! The decorator is constructed only on the LLM-enabled path. The route layer additionally refuses
//! the whole op when the user has disabled cloud LLM (`privacy.llm == false`) — see the document
//! route. The decorator's job is the redaction guarantee; the toggle is enforced before we get
//! here.

use crate::llm::{Attachment, ChatMessage, LlmCallOptions, LlmProvider};
use crate::pii::Redactor;
use crate::usage::TokenUsage;
use std::sync::Arc;

/// Wraps an [`LlmProvider`] so every outbound payload is PII-redacted and the response is restored.
pub struct RedactingLlmProvider {
    inner: Arc<dyn LlmProvider>,
    redactor: Arc<Redactor>,
}

impl RedactingLlmProvider {
    pub fn new(inner: Arc<dyn LlmProvider>, redactor: Arc<Redactor>) -> Self {
        Self { inner, redactor }
    }

    /// Convenience: wrap with the default builtin redactor (L1 patterns).
    pub fn with_default_redactor(inner: Arc<dyn LlmProvider>) -> Self {
        Self::new(inner, Arc::new(Redactor::new()))
    }
}

impl LlmProvider for RedactingLlmProvider {
    fn chat(&self, system: &str, user: &str) -> crate::error::Result<(String, TokenUsage)> {
        let (segs, mappings) = self.redactor.redact_batch(&[system, user]);
        let (raw, usage) = self.inner.chat(&segs[0], &segs[1])?;
        Ok((self.redactor.restore(&raw, &mappings), usage))
    }

    fn chat_with_history(
        &self,
        messages: &[ChatMessage],
    ) -> crate::error::Result<(String, TokenUsage)> {
        let (red_msgs, mappings) = self.redact_messages(messages);
        let (raw, usage) = self.inner.chat_with_history(&red_msgs)?;
        Ok((self.redactor.restore(&raw, &mappings), usage))
    }

    fn chat_with_history_opts(
        &self,
        messages: &[ChatMessage],
        opts: &LlmCallOptions,
    ) -> crate::error::Result<(String, TokenUsage)> {
        let (red_msgs, mappings) = self.redact_messages(messages);
        let (raw, usage) = self.inner.chat_with_history_opts(&red_msgs, opts)?;
        Ok((self.redactor.restore(&raw, &mappings), usage))
    }

    fn chat_with_options(
        &self,
        messages: &[ChatMessage],
        opts: &LlmCallOptions,
    ) -> crate::error::Result<String> {
        let (red_msgs, mappings) = self.redact_messages(messages);
        let raw = self.inner.chat_with_options(&red_msgs, opts)?;
        Ok(self.redactor.restore(&raw, &mappings))
    }

    fn chat_multimodal(
        &self,
        system: &str,
        user: &str,
        attachments: &[Attachment],
    ) -> crate::error::Result<(String, TokenUsage)> {
        // Redact the text channels. Image bytes are passed through unchanged — image PII is out of
        // scope for the text redactor; the text-built prompt is the regression I1 targets.
        let mut segments: Vec<String> = vec![system.to_string(), user.to_string()];
        for a in attachments {
            if let Attachment::TextFile { content, .. } = a {
                segments.push(content.clone());
            }
        }
        let (segs, mappings) = self.redactor.redact_batch(&segments);
        let red_system = segs[0].clone();
        let red_user = segs[1].clone();
        // Rebuild attachments with redacted text-file content (in order).
        let mut idx = 2usize;
        let red_attachments: Vec<Attachment> = attachments
            .iter()
            .map(|a| match a {
                Attachment::TextFile { name, .. } => {
                    let content = segs.get(idx).cloned().unwrap_or_default();
                    idx += 1;
                    Attachment::TextFile { name: name.clone(), content }
                }
                other => other.clone(),
            })
            .collect();
        let (raw, usage) = self
            .inner
            .chat_multimodal(&red_system, &red_user, &red_attachments)?;
        Ok((self.redactor.restore(&raw, &mappings), usage))
    }

    fn determinism_level(&self) -> crate::llm::DeterminismLevel {
        self.inner.determinism_level()
    }

    fn is_available(&self) -> bool {
        self.inner.is_available()
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }

    fn is_local(&self) -> bool {
        self.inner.is_local()
    }
}

impl RedactingLlmProvider {
    /// Redact every message's content via one `redact_batch` (placeholders are globally unique
    /// across the messages of THIS call), returning the redacted messages + the mappings to
    /// restore the response with.
    fn redact_messages(
        &self,
        messages: &[ChatMessage],
    ) -> (Vec<ChatMessage>, Vec<crate::pii::PiiMatch>) {
        let segments: Vec<&str> = messages.iter().map(|m| m.content.as_str()).collect();
        let (red_segs, mappings) = self.redactor.redact_batch(&segments);
        let red_msgs = messages
            .iter()
            .zip(red_segs)
            .map(|(m, content)| ChatMessage { role: m.role.clone(), content })
            .collect();
        (red_msgs, mappings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::RecordingMockLlm;

    fn pii_doc() -> &'static str {
        // Contains a phone, an email, and a Chinese ID — the three classic PII kinds.
        "联系人张三 手机 13800138000 邮箱 zhangsan@example.com 身份证 11010119900307123X"
    }

    #[test]
    fn chat_redacts_phone_email_id_before_the_wire() {
        let inner = Arc::new(RecordingMockLlm::new("ok"));
        let wrapped = RedactingLlmProvider::with_default_redactor(inner.clone());
        let (_resp, _u) = wrapped.chat("你是助手", pii_doc()).unwrap();

        // What the inner (wire-facing) provider actually received must NOT contain the raw PII.
        let seen = inner.calls();
        let sent = seen.iter().map(|c| c.user.clone()).collect::<Vec<_>>().join(" ")
            + &seen.iter().map(|c| c.system.clone()).collect::<Vec<_>>().join(" ");
        assert!(!sent.contains("13800138000"), "phone reached the wire: {sent}");
        assert!(!sent.contains("zhangsan@example.com"), "email reached the wire: {sent}");
        assert!(!sent.contains("11010119900307123X"), "ID reached the wire: {sent}");
        assert!(sent.contains("PHONE_") || sent.contains("EMAIL_"), "placeholders expected: {sent}");
    }

    #[test]
    fn chat_with_history_redacts_every_message() {
        let inner = Arc::new(RecordingMockLlm::new("ok"));
        let wrapped = RedactingLlmProvider::with_default_redactor(inner.clone());
        let msgs = vec![
            ChatMessage::system("分析以下文档"),
            ChatMessage::user(pii_doc()),
            ChatMessage::assistant("好的"),
        ];
        let _ = wrapped.chat_with_history(&msgs).unwrap();
        // RecordingMockLlm flattens history to (system, last-user); the PII-bearing user message is
        // the last user, so it is what reached the inner provider — and must be redacted.
        let seen = inner.calls();
        let joined = seen
            .iter()
            .map(|c| format!("{} {}", c.system, c.user))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!joined.contains("13800138000"), "phone reached the wire in history: {joined}");
        assert!(!joined.contains("zhangsan@example.com"), "email reached the wire: {joined}");
    }

    #[test]
    fn response_placeholders_are_restored_for_the_caller() {
        // If the model echoes a placeholder, the caller must get the real value back (the PII
        // round-trips locally, never on the wire).
        let inner = Arc::new(RecordingMockLlm::new("mock").with_response("回拨 [PHONE_1] 即可"));
        let wrapped = RedactingLlmProvider::with_default_redactor(inner.clone());
        let (resp, _u) = wrapped.chat("助手", "我的电话是 13800138000").unwrap();
        assert!(
            resp.contains("13800138000"),
            "placeholder must be restored to the real value for the caller: {resp}"
        );
    }

    #[test]
    fn clean_text_passes_through_unchanged() {
        let inner = Arc::new(RecordingMockLlm::new("mock").with_response("answer"));
        let wrapped = RedactingLlmProvider::with_default_redactor(inner.clone());
        let (resp, _u) = wrapped.chat("system", "no pii here at all").unwrap();
        assert_eq!(resp, "answer");
        let seen = inner.calls();
        assert_eq!(seen[0].user, "no pii here at all", "clean text must be byte-identical");
    }
}
