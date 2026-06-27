//! Memory scope — the isolation primitive for chat-centric IA (v1.6.0).
//!
//! Each memory row carries a `(scope_kind, scope_id)` column pair. Retrieval is
//! scope-aware: a project-bound conversation may only see that project's memories
//! plus global; a loose conversation sees global only (its own working recall is
//! lazily derived from messages, never persisted). This type encodes that pair and
//! the per-scope retrieval allowlist that enforces the cross-project privacy
//! invariant ("project A memory never appears in project B / loose retrieval").

/// Memory scope — the core isolation type. Encoded as `scope_kind` + optional
/// `scope_id` columns on `memories`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryScope {
    /// Cross-everything (old rows + document-derived memory default).
    Global,
    /// A single project (case-file) scope.
    Project(String),
    /// A single loose conversation scope (lazily derived, never persisted).
    Conversation(String),
}

impl MemoryScope {
    /// The `(scope_kind, scope_id)` column pair — used when writing a row.
    pub fn to_columns(&self) -> (&'static str, Option<String>) {
        match self {
            MemoryScope::Global => ("global", None),
            MemoryScope::Project(id) => ("project", Some(id.clone())),
            MemoryScope::Conversation(id) => ("conversation", Some(id.clone())),
        }
    }

    /// Restore from stored columns. An unknown `kind` (corrupted row or a future
    /// scope this binary does not understand) **falls back to Global, never to a
    /// project**: a fail-safe that tightens, so a mislabelled row can never be
    /// wrongly authorized into some project's retrieval.
    pub fn from_columns(kind: &str, id: Option<&str>) -> MemoryScope {
        match (kind, id) {
            ("project", Some(id)) => MemoryScope::Project(id.to_string()),
            ("conversation", Some(id)) => MemoryScope::Conversation(id.to_string()),
            _ => MemoryScope::Global,
        }
    }

    /// The `(scope_kind, scope_id)` whitelist a retrieval in this scope may hit.
    ///   - `Project(P)`      → `[("project", Some(P)), ("global", None)]`
    ///   - `Conversation(C)` → `[("global", None)]` (conversation recall is lazily
    ///     derived from messages, not stored in `memories`)
    ///   - `Global`          → `[("global", None)]`
    pub fn retrieval_allowlist(&self) -> Vec<(&'static str, Option<String>)> {
        match self {
            MemoryScope::Project(id) => {
                vec![("project", Some(id.clone())), ("global", None)]
            }
            MemoryScope::Conversation(_) | MemoryScope::Global => {
                vec![("global", None)]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_columns_roundtrips() {
        assert_eq!(MemoryScope::Global.to_columns(), ("global", None));
        assert_eq!(
            MemoryScope::Project("p1".into()).to_columns(),
            ("project", Some("p1".into()))
        );
        assert_eq!(
            MemoryScope::Conversation("c1".into()).to_columns(),
            ("conversation", Some("c1".into()))
        );
    }

    #[test]
    fn from_columns_restores() {
        assert_eq!(
            MemoryScope::from_columns("project", Some("p1")),
            MemoryScope::Project("p1".into())
        );
        assert_eq!(MemoryScope::from_columns("global", None), MemoryScope::Global);
        assert_eq!(
            MemoryScope::from_columns("conversation", Some("c1")),
            MemoryScope::Conversation("c1".into())
        );
    }

    #[test]
    fn unknown_kind_falls_back_to_global_not_project() {
        // Privacy fail-safe: a corrupted / future kind must never be treated as a
        // project scope (that could wrongly authorize cross-project retrieval).
        assert_eq!(MemoryScope::from_columns("team", Some("t1")), MemoryScope::Global);
        assert_eq!(MemoryScope::from_columns("", None), MemoryScope::Global);
        // kind known but id missing → also falls back (cannot identify the scope).
        assert_eq!(MemoryScope::from_columns("project", None), MemoryScope::Global);
    }

    #[test]
    fn project_allowlist_includes_global_excludes_other_projects() {
        let al = MemoryScope::Project("p1".into()).retrieval_allowlist();
        assert!(al.contains(&("project", Some("p1".into()))));
        assert!(al.contains(&("global", None)));
        // Critical isolation: another project is not in the allowlist.
        assert!(!al.contains(&("project", Some("p2".into()))));
    }

    #[test]
    fn conversation_allowlist_is_global_only() {
        // A loose conversation's own recall is lazily derived (not in `memories`),
        // so library retrieval only releases global rows.
        let al = MemoryScope::Conversation("c1".into()).retrieval_allowlist();
        assert_eq!(al, vec![("global", None)]);
    }
}
