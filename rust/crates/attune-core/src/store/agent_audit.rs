//! G2 — Agent (MCP) 调用审计日志。
//!
//! 设计目标(镜像 `store::audit` 出网审计风格):
//! - **每次 MCP 工具调用都本地落审计**:谁(agent_source)调了哪工具(tool)、何时(ts_ms)、
//!   放行还是拒绝(decision)、拒绝原因(deny_reason)。
//! - **0 敏感内容落库**:不存 query 原文 / 不存结果 —— 只存元数据。这是入侵检测面 +
//!   成本归因面("哪个 agent 花了多少次 chat"),不是内容审计。
//! - **明文存储**:用户/合规员要直接读 timestamp/tool/decision,不加密。
//! - **allow 与 deny 都落**:deny 留痕才能发现「试探高危动作」的恶意 agent host。
//!
//! ## 字段不变性
//! `decision` 永远是 `allow` 或 `deny`;`deny` 时 `deny_reason` 非空(kebab code),
//! `allow` 时 `deny_reason` 为空串。

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::store::Store;

/// 一次 MCP 工具调用的审计记录。**0 敏感内容**。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAuditEvent {
    /// id 由 SQLite 自增分配,写入前为 0
    #[serde(default)]
    pub id: i64,
    pub ts_ms: i64,
    /// 调用方标识 = scoped token 的 label(人类可读,非 token 明文)。
    pub agent_source: String,
    /// 被调工具名(vault_search / vault_chat / ...)。
    pub tool: String,
    /// "allow" | "deny"
    pub decision: String,
    /// deny 时的 kebab code(scope-missing / high-risk-denied / expired / revoked / ...);
    /// allow 时为空串。
    #[serde(default)]
    pub deny_reason: String,
}

impl AgentAuditEvent {
    /// allow 记录便利构造(当前时间 ms)。
    pub fn allow(agent_source: impl Into<String>, tool: impl Into<String>) -> Self {
        Self {
            id: 0,
            ts_ms: chrono::Utc::now().timestamp_millis(),
            agent_source: agent_source.into(),
            tool: tool.into(),
            decision: "allow".into(),
            deny_reason: String::new(),
        }
    }

    /// deny 记录便利构造(当前时间 ms + kebab reason)。
    pub fn deny(
        agent_source: impl Into<String>,
        tool: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            id: 0,
            ts_ms: chrono::Utc::now().timestamp_millis(),
            agent_source: agent_source.into(),
            tool: tool.into(),
            decision: "deny".into(),
            deny_reason: reason.into(),
        }
    }
}

impl Store {
    /// 记录一次 MCP 工具调用审计。返回新 row 的 id。
    ///
    /// gate 层 fail-closed 语义:写失败 → Err 上抛,调用方据此拒绝放行(防无痕调用)。
    pub fn record_agent_audit(&self, event: &AgentAuditEvent) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO agent_audit (ts_ms, agent_source, tool, decision, deny_reason)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.ts_ms,
                &event.agent_source,
                &event.tool,
                &event.decision,
                &event.deny_reason,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 查询 agent 审计(按时间倒序,可选时段过滤)。
    pub fn list_agent_audit(
        &self,
        from_ms: Option<i64>,
        to_ms: Option<i64>,
        limit: usize,
    ) -> Result<Vec<AgentAuditEvent>> {
        let from = from_ms.unwrap_or(i64::MIN);
        let to = to_ms.unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, ts_ms, agent_source, tool, decision, deny_reason
             FROM agent_audit
             WHERE ts_ms >= ?1 AND ts_ms <= ?2
             ORDER BY ts_ms DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![from, to, limit as i64], |row| {
            Ok(AgentAuditEvent {
                id: row.get(0)?,
                ts_ms: row.get(1)?,
                agent_source: row.get(2)?,
                tool: row.get(3)?,
                decision: row.get(4)?,
                deny_reason: row.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_and_deny_roundtrip() {
        let store = Store::open_memory().unwrap();
        store
            .record_agent_audit(&AgentAuditEvent::allow("hermes-laptop", "vault_search"))
            .unwrap();
        store
            .record_agent_audit(&AgentAuditEvent::deny(
                "hermes-laptop",
                "delete",
                "high-risk-denied",
            ))
            .unwrap();

        let rows = store.list_agent_audit(None, None, 100).unwrap();
        assert_eq!(rows.len(), 2);
        // 时间倒序: deny 后写在前
        assert_eq!(rows[0].decision, "deny");
        assert_eq!(rows[0].deny_reason, "high-risk-denied");
        assert_eq!(rows[1].decision, "allow");
        assert_eq!(rows[1].deny_reason, "");
    }

    #[test]
    fn audit_holds_no_sensitive_content() {
        // 审计表 schema 只有 agent_source/tool/decision/deny_reason —— 没有 query/content 列。
        // 该测试钉死「0 敏感内容」不变量:即使 agent_source 误传内容,也只是 label 字段,
        // 不存在存 query 原文的列。
        let store = Store::open_memory().unwrap();
        store
            .record_agent_audit(&AgentAuditEvent::allow("agentA", "vault_chat"))
            .unwrap();
        let rows = store.list_agent_audit(None, None, 10).unwrap();
        let json = serde_json::to_string(&rows[0]).unwrap();
        // 字段集合固定 —— 无 query / content / message / result key
        assert!(!json.contains("\"query\""));
        assert!(!json.contains("\"content\""));
        assert!(!json.contains("\"message\""));
        assert!(!json.contains("\"result\""));
    }
}
