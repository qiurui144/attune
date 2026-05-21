//! skill_expansions — per-query learned expansions persisted by
//! [`crate::skill_evolution::agent::SelfEvolvingSkillAgent`].
//!
//! Schema lives in [`crate::store::SCHEMA_SQL`] (`CREATE TABLE IF NOT EXISTS
//! skill_expansions ...`). All methods are inherent `impl Store` — rustc merges
//! cross-file inherent impls automatically (same pattern as
//! `crate::store::signals`).
//!
//! Why a sibling to `app_settings.search.learned_expansions`?
//!
//! - The legacy `skill_evolution` cycle clusters failed queries by LLM-extracted
//!   *topic* and writes one entry per topic to the settings blob — *topic-keyed*.
//! - The new agent writes one row per *query pattern* (lowercased original
//!   query) so heuristic / cheap regenerations can de-duplicate by exact query
//!   without rewriting the whole settings blob, and so the row carries
//!   per-pattern provenance (`generated_by`, `confidence`).
//!
//! Application order in search expansion is intentionally:
//!   1. exact query_pattern hit from this table (high-precision)
//!   2. fall back to topic-keyed `learned_expansions` (legacy)
//!
//! see `skill_evolution::expand_query_with_table` (added in agent.rs).

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::store::Store;

/// Maximum expansion terms persisted per query_pattern. Mirrors the per-topic
/// `truncate(8)` in `merge_expansions_into_settings` — bounded slot defense.
pub const MAX_EXPANSIONS_PER_PATTERN: usize = 8;

/// Provenance tag — how this row was generated. Reflected in the
/// `generated_by` TEXT column so a UI can render badges and the agent can
/// re-rank rows (LLM > heuristic) on conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpansionSource {
    /// Zero-cost path — derived from past `skill_signals` queries + a stoplist,
    /// no LLM call. Default confidence 0.4 (low — token co-occurrence).
    Heuristic,
    /// LLM-generated path — same prompt shape as `skill_evolution::generate_expansions`,
    /// but written per query_pattern not per topic. Default confidence 0.7.
    Llm,
}

impl ExpansionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ExpansionSource::Heuristic => "heuristic",
            ExpansionSource::Llm => "llm",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "heuristic" => Some(ExpansionSource::Heuristic),
            "llm" => Some(ExpansionSource::Llm),
            _ => None,
        }
    }

    /// Default confidence for a freshly-generated row. Caller may override.
    pub fn default_confidence(self) -> f32 {
        match self {
            ExpansionSource::Heuristic => 0.4,
            ExpansionSource::Llm => 0.7,
        }
    }
}

/// One row in `skill_expansions`. Returned by `list_skill_expansions` and friends.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillExpansionRow {
    pub query_pattern: String,
    pub expansions: Vec<String>,
    pub generated_by: ExpansionSource,
    pub confidence: f32,
    pub created_at: String,
    pub updated_at: String,
}

impl Store {
    /// Upsert one expansion row. Conflict resolution:
    ///
    /// - LLM-generated rows replace heuristic rows (LLM wins on the
    ///   `generated_by` column).
    /// - Same-source upserts overwrite expansions + bump `updated_at` (we trust
    ///   the latest cycle).
    /// - LLM rows are *not* downgraded by a later heuristic call — we keep the
    ///   higher-provenance row.
    /// - Per-row terms truncated to [`MAX_EXPANSIONS_PER_PATTERN`] regardless
    ///   of caller input (defence-in-depth against unbounded LLM output).
    pub fn upsert_skill_expansion(
        &self,
        query_pattern: &str,
        expansions: &[String],
        generated_by: ExpansionSource,
        confidence: f32,
    ) -> Result<bool> {
        let pattern = query_pattern.trim().to_lowercase();
        if pattern.is_empty() {
            return Ok(false);
        }
        if pattern.len() > 256 {
            // Defence: a query that long is unlikely to be a real search; never
            // index unbounded patterns.
            return Ok(false);
        }

        // Truncate + dedup + drop empties (canonical persistence shape).
        let mut seen = std::collections::BTreeSet::new();
        let cleaned: Vec<String> = expansions
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s.len() <= 64)
            .filter(|s| seen.insert(s.clone()))
            .take(MAX_EXPANSIONS_PER_PATTERN)
            .collect();
        if cleaned.is_empty() {
            return Ok(false);
        }

        // Read existing row to decide replace vs skip.
        let existing: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT generated_by, expansions FROM skill_expansions WHERE query_pattern = ?1",
                params![pattern],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .ok();

        if let Some((existing_source, _)) = &existing {
            if existing_source == "llm" && generated_by == ExpansionSource::Heuristic {
                // Don't downgrade an LLM row with a heuristic one.
                return Ok(false);
            }
        }

        let json = serde_json::to_string(&cleaned).map_err(crate::error::VaultError::from)?;

        if existing.is_some() {
            self.conn.execute(
                "UPDATE skill_expansions \
                 SET expansions = ?1, generated_by = ?2, confidence = ?3, \
                     updated_at = datetime('now') \
                 WHERE query_pattern = ?4",
                params![json, generated_by.as_str(), confidence as f64, pattern],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO skill_expansions \
                 (query_pattern, expansions, generated_by, confidence) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![pattern, json, generated_by.as_str(), confidence as f64],
            )?;
        }
        Ok(true)
    }

    /// Exact-match lookup (case-insensitive — patterns are stored lowercased).
    pub fn get_skill_expansion(&self, query_pattern: &str) -> Result<Option<SkillExpansionRow>> {
        let pattern = query_pattern.trim().to_lowercase();
        if pattern.is_empty() {
            return Ok(None);
        }
        let row = self
            .conn
            .query_row(
                "SELECT query_pattern, expansions, generated_by, confidence, created_at, updated_at \
                 FROM skill_expansions WHERE query_pattern = ?1",
                params![pattern],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, f64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .ok();

        Ok(row.and_then(|(qp, expansions_json, gen_by, conf, created, updated)| {
            let expansions: Vec<String> = serde_json::from_str(&expansions_json).ok()?;
            let generated_by = ExpansionSource::parse(&gen_by)?;
            Some(SkillExpansionRow {
                query_pattern: qp,
                expansions,
                generated_by,
                confidence: conf as f32,
                created_at: created,
                updated_at: updated,
            })
        }))
    }

    /// List rows in `updated_at DESC` order (newest first). UI rendering /
    /// agent re-run for "stale rows" path.
    pub fn list_skill_expansions(&self, limit: usize) -> Result<Vec<SkillExpansionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT query_pattern, expansions, generated_by, confidence, created_at, updated_at \
             FROM skill_expansions ORDER BY updated_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut out: Vec<SkillExpansionRow> = Vec::new();
        for r in rows {
            let (qp, exp_json, gen_by, conf, c, u) = r?;
            let expansions: Vec<String> = serde_json::from_str(&exp_json).unwrap_or_default();
            let Some(source) = ExpansionSource::parse(&gen_by) else { continue };
            out.push(SkillExpansionRow {
                query_pattern: qp,
                expansions,
                generated_by: source,
                confidence: conf as f32,
                created_at: c,
                updated_at: u,
            });
        }
        Ok(out)
    }

    /// Delete a single row (user-visible UI: "forget this expansion").
    pub fn delete_skill_expansion(&self, query_pattern: &str) -> Result<bool> {
        let pattern = query_pattern.trim().to_lowercase();
        if pattern.is_empty() {
            return Ok(false);
        }
        let n = self.conn.execute(
            "DELETE FROM skill_expansions WHERE query_pattern = ?1",
            params![pattern],
        )?;
        Ok(n > 0)
    }

    /// Total row count (cheap stat for UI / agent governance).
    pub fn count_skill_expansions(&self) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM skill_expansions",
            [],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_and_get_roundtrip() {
        let store = Store::open_memory().unwrap();
        let inserted = store
            .upsert_skill_expansion(
                "rust 所有权",
                &["ownership".into(), "borrow checker".into()],
                ExpansionSource::Heuristic,
                0.4,
            )
            .unwrap();
        assert!(inserted);
        let row = store.get_skill_expansion("Rust 所有权").unwrap().unwrap();
        // Patterns are stored lowercased.
        assert_eq!(row.query_pattern, "rust 所有权");
        assert_eq!(row.expansions.len(), 2);
        assert_eq!(row.generated_by, ExpansionSource::Heuristic);
    }

    #[test]
    fn llm_replaces_heuristic_but_not_vice_versa() {
        let store = Store::open_memory().unwrap();
        store
            .upsert_skill_expansion(
                "q",
                &["a".into()],
                ExpansionSource::Heuristic,
                0.4,
            )
            .unwrap();
        // LLM upgrades.
        let upgraded = store
            .upsert_skill_expansion("q", &["b".into()], ExpansionSource::Llm, 0.8)
            .unwrap();
        assert!(upgraded);
        let row = store.get_skill_expansion("q").unwrap().unwrap();
        assert_eq!(row.generated_by, ExpansionSource::Llm);
        assert_eq!(row.expansions, vec!["b".to_string()]);

        // Heuristic must not overwrite an LLM row.
        let downgrade = store
            .upsert_skill_expansion("q", &["c".into()], ExpansionSource::Heuristic, 0.4)
            .unwrap();
        assert!(!downgrade);
        let row = store.get_skill_expansion("q").unwrap().unwrap();
        assert_eq!(row.generated_by, ExpansionSource::Llm);
    }

    #[test]
    fn empty_query_pattern_returns_false() {
        let store = Store::open_memory().unwrap();
        let ok = store
            .upsert_skill_expansion("   ", &["x".into()], ExpansionSource::Heuristic, 0.4)
            .unwrap();
        assert!(!ok);
    }

    #[test]
    fn truncates_to_max_expansions() {
        let store = Store::open_memory().unwrap();
        let terms: Vec<String> = (0..20).map(|i| format!("t{i}")).collect();
        store
            .upsert_skill_expansion("q", &terms, ExpansionSource::Heuristic, 0.4)
            .unwrap();
        let row = store.get_skill_expansion("q").unwrap().unwrap();
        assert_eq!(row.expansions.len(), MAX_EXPANSIONS_PER_PATTERN);
    }

    #[test]
    fn delete_removes_row() {
        let store = Store::open_memory().unwrap();
        store
            .upsert_skill_expansion("q", &["a".into()], ExpansionSource::Heuristic, 0.4)
            .unwrap();
        assert!(store.delete_skill_expansion("Q").unwrap());
        assert!(store.get_skill_expansion("q").unwrap().is_none());
    }
}
