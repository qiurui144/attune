//! G2 gate — MCP 工具调用的权限闸门。
//!
//! 调用顺序(每次 `tools/call`,业务执行前):
//!   1. verify scoped token (HMAC + 过期 + nonce 吊销, 见 vault::verify_scoped_token)
//!   2. **high-risk denylist?** (export/delete/settings) → 永久拒 (硬编码, 先于权限位)
//!   3. tool → required_scope, token 权限位含? 否 → 拒
//!   4. audit: 落 agent_audit (allow 与 deny 都落; 写失败 → fail-closed 拒)
//!
//! 本模块的 *决策* 部分是纯函数(`decide`),不碰 IO,可单测。token verify + audit 写盘
//! 在 transport 层组合(那里有 SharedState)。

use crate::mcp::tools;

/// gate 决策结果。`Allow` 携带 required_scope (审计用);`Deny` 携带 kebab reason + MCP code。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    Allow,
    /// (kebab reason, MCP json-rpc error code)
    Deny(&'static str, i64),
}

/// MCP json-rpc error codes (本项目约定, -32000..-32099 为应用层)。
pub const ERR_UNAUTHORIZED: i64 = -32001;
pub const ERR_FORBIDDEN_SCOPE: i64 = -32002;
pub const ERR_HIGH_RISK: i64 = -32003;
pub const ERR_UNKNOWN_TOOL: i64 = -32601;

/// 高危动作硬黑名单 —— scoped token **永久拒绝**,不论权限位。
///
/// 即使未来某个 MCP 工具叫这些名字、或 token 错配了含这些字样的权限位,gate 也先在此挡掉。
/// 本 spec 的 6 工具均不在此列(MCP 不暴露 export/delete/settings)。
pub const HIGH_RISK_TOOLS: &[&str] = &[
    "export",
    "delete",
    "settings",
    "vault_export",
    "vault_delete",
    "delete_item",
    "update_settings",
    "purge",
    "wipe",
    "reset",
];

/// 纯决策:给定工具名 + token 已校验的权限位集,返回放行/拒绝。
///
/// **denylist 先于 scope 检查** —— 这是安全不变量(spec §3 信任不变量 #2)。
pub fn decide(tool: &str, granted_scopes: &[String]) -> GateDecision {
    // 1. 高危永久拒 (先于一切, 含未知工具名命中黑名单的情况)
    if is_high_risk(tool) {
        return GateDecision::Deny("high-risk-denied", ERR_HIGH_RISK);
    }

    // 2. 工具必须是已知的 6 工具之一
    let required = match tools::required_scope(tool) {
        Some(s) => s,
        None => return GateDecision::Deny("unknown-tool", ERR_UNKNOWN_TOOL),
    };

    // 3. 权限位检查
    if granted_scopes.iter().any(|s| s == required) {
        GateDecision::Allow
    } else {
        GateDecision::Deny("scope-missing", ERR_FORBIDDEN_SCOPE)
    }
}

/// 工具是否命中高危黑名单(大小写不敏感 + 子串语义防绕过)。
pub fn is_high_risk(tool: &str) -> bool {
    let lower = tool.to_ascii_lowercase();
    HIGH_RISK_TOOLS.iter().any(|h| lower == *h)
        // 子串匹配挡 "foo_delete" / "export_all" 这类变体名(本 spec 工具名不含这些动词,
        // 安全侧 fail-safe: 任何含 delete/export/settings/purge/wipe 的工具名都视为高危)。
        || ["delete", "export", "purge", "wipe", "destroy"]
            .iter()
            .any(|verb| lower.contains(verb))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scopes(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn allow_when_scope_granted() {
        assert_eq!(
            decide("vault_search", &scopes(&["search"])),
            GateDecision::Allow
        );
        assert_eq!(
            decide("vault_chat", &scopes(&["chat"])),
            GateDecision::Allow
        );
        assert_eq!(decide("ingest", &scopes(&["ingest"])), GateDecision::Allow);
        assert_eq!(
            decide("annotate", &scopes(&["ingest"])),
            GateDecision::Allow
        );
        assert_eq!(
            decide("agent_invoke", &scopes(&["chat"])),
            GateDecision::Allow
        );
        assert_eq!(
            decide("job_status", &scopes(&["search"])),
            GateDecision::Allow
        );
    }

    #[test]
    fn deny_when_scope_missing() {
        // 无 chat 权限调 vault_chat → 拒 (安全对抗 b)
        assert_eq!(
            decide("vault_chat", &scopes(&["search"])),
            GateDecision::Deny("scope-missing", ERR_FORBIDDEN_SCOPE)
        );
        assert_eq!(
            decide("agent_invoke", &scopes(&["search", "ingest"])),
            GateDecision::Deny("scope-missing", ERR_FORBIDDEN_SCOPE)
        );
        assert_eq!(
            decide("ingest", &scopes(&["search"])),
            GateDecision::Deny("scope-missing", ERR_FORBIDDEN_SCOPE)
        );
    }

    #[test]
    fn high_risk_always_denied_even_with_matching_scope() {
        // 安全对抗 a: scoped token 试调 export/delete/settings → 必拒, 不论权限位
        for tool in ["export", "delete", "settings"] {
            // 即使给了全部三权 + 一个名字含 delete 的伪权限位, 仍拒
            let granted = scopes(&["search", "chat", "ingest", "delete"]);
            assert_eq!(
                decide(tool, &granted),
                GateDecision::Deny("high-risk-denied", ERR_HIGH_RISK),
                "{tool} must be permanently denied"
            );
        }
    }

    #[test]
    fn high_risk_denylist_precedes_scope_and_unknown() {
        // denylist 先于 scope: 含 delete 字样的工具名即使配了对应权限位也挡
        assert_eq!(
            decide("delete_item", &scopes(&["ingest"])),
            GateDecision::Deny("high-risk-denied", ERR_HIGH_RISK)
        );
        // 子串变体防绕过
        assert!(is_high_risk("export_everything"));
        assert!(is_high_risk("FORCE_DELETE"));
        assert!(!is_high_risk("vault_search"));
        assert!(!is_high_risk("ingest"));
    }

    #[test]
    fn unknown_tool_denied() {
        assert_eq!(
            decide("frobnicate", &scopes(&["search", "chat", "ingest"])),
            GateDecision::Deny("unknown-tool", ERR_UNKNOWN_TOOL)
        );
    }
}
