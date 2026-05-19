# ADR 0005: F-17 全路径 PII redact (ChatEngine + outbound audit)

- **Status**: Accepted
- **Date**: 2026-05-14

## Context

attune Phase A.5 隐私三档承诺 L1 默认 = 12 PII 类脱敏 → 云端 LLM. UI 显示已开
PII 脱敏 ✓, 但 routes/chat.rs 自己拼 messages 直接调 `llm.chat_with_history`,
**完全绕过 ChatEngine 的 Redactor**. 用户以为脱敏但服务端真发原文给云端 LLM.

发现于 v0.6.3 PR review: 服务端 PII bypass.

## Decision

**所有 LLM outbound 必经 Redactor**:

1. `attune-core::chat::ChatEngine` 已有 `Redactor::default()` 注入 outbound path
2. `routes/chat.rs` 必须用 ChatEngine 而非直调 llm
3. 抽 `Redactor` 为 chat-independent 单元, 让 attune-pro 行业 plugin 可注入
   `pii::Redactor::with_rules(law_rules)` 扩展 PII 规则 (case_no / 病历术语)
4. **Outbound audit log**: 每次脱敏命中写入 `store::audit_log`, 可 CSV export
   (合规要求)

## Consequences

**好处**:
- 真正落实 Phase A.5 L1 承诺 (PII 不出网)
- attune-pro 行业插件可叠加规则 (law/medical 专用 PII)
- 合规可审计 (CSV export 给企业法务)

**代价**:
- 每次 LLM 调用 +1-5ms regex 扫描开销 (12 类 patterns)
- audit log 持久化 (本 ADR ship 时是 tracing log 占位, v0.7 真持久化 SQL)
- chat.rs 历史的"raw" path 需 deprecate (向后兼容窗口 1 release)

## Implementation 落地

- v0.6.3 (commit b08d527): ChatEngine bypass 修, redact + audit log 全路径
  - 12 PII 类全覆盖: id-card ISO7064 / phone / email / 8 API key vendor
  - audit_log 当前 tracing::info!, v0.7 落 store::audit_log table
- 测试: pii_chat_path_redact_test.rs 验证 LLM provider 真收 redact 后内容

## v0.7 follow-up

- store::audit_log table schema + CSV export endpoint /api/v1/audit/outbound/export.csv
- 已部分实施 (settings.rs ALLOWED_KEYS 含 audit), 完整持久化 + UI 入口 v0.7
