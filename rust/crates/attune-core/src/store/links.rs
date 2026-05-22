//! Internal knowledge linker — persisted storage layer.
//!
//! Two tables, both `CREATE TABLE IF NOT EXISTS` (declared in `mod.rs`
//! `SCHEMA_SQL` extension below):
//!
//! - `item_entities` — the inverted index `(kind, value) → [item_id]` so a
//!   freshly-ingested item can be linked against the existing corpus without
//!   scanning every other item's content.
//! - `item_links` — the actual emitted link rows
//!   `(item_a, item_b, kind, weight, evidence)`.
//!
//! All methods live on `impl Store` (inherent impl splits across files,
//! rustc auto-merges).

use rusqlite::{params, OptionalExtension};

use crate::entities::Entity;
use crate::error::Result;
use crate::linker::agent::ComputedLink;
#[cfg(test)]
use crate::linker::agent::LinkKind;
use crate::store::Store;

impl Store {
    // ========================================================================
    // item_entities — inverted index
    // ========================================================================

    /// Re-write all entity rows for this item (delete-then-insert, same idiom
    /// as `TagIndex::upsert`). `entities` should be the output of
    /// [`crate::entities::extract_entities`].
    ///
    /// Idempotent: running with the same entity set produces the same rows.
    pub fn replace_item_entities(&self, item_id: &str, entities: &[Entity]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM item_entities WHERE item_id = ?1",
            params![item_id],
        )?;
        // Aggregate (kind, value) → occurrences inside the function to make the
        // table primary-key conflict-free (a single item mentioning the same
        // person twice should yield one row with occurrences=2).
        let mut counts: std::collections::BTreeMap<(&'static str, String), i64> =
            std::collections::BTreeMap::new();
        for e in entities {
            let kind = kind_str(&e.kind);
            *counts.entry((kind, e.value.clone())).or_insert(0) += 1;
        }
        for ((kind, value), occ) in counts {
            tx.execute(
                "INSERT INTO item_entities (item_id, kind, value, occurrences) VALUES (?1, ?2, ?3, ?4)",
                params![item_id, kind, value, occ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Delete all `item_entities` rows for `item_id`. Returns rows affected.
    pub fn purge_item_entities(&self, item_id: &str) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM item_entities WHERE item_id = ?1",
            params![item_id],
        )?;
        Ok(n)
    }

    /// Reverse lookup: which items mention this `(kind, value)` entity?
    /// Used by the linker's shared-entity computation as the cross-item
    /// inverted index. O(log n) via `idx_item_entities_kv`.
    pub fn find_items_by_entity(&self, kind: &str, value: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT item_id FROM item_entities WHERE kind = ?1 AND value = ?2",
        )?;
        let rows = stmt.query_map(params![kind, value], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Document frequency for the stop-entity filter: how many distinct items
    /// mention this entity. Used by [`crate::linker::compute_links_for_item`]
    /// to drop ubiquitous entities (entity in > `df_ratio` of items).
    pub fn entity_document_frequency(&self, kind: &str, value: &str) -> Result<usize> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT COUNT(DISTINCT item_id) FROM item_entities WHERE kind = ?1 AND value = ?2",
        )?;
        let n: i64 = stmt.query_row(params![kind, value], |r| r.get(0))?;
        Ok(n.max(0) as usize)
    }

    /// Distinct count of items having at least one extracted entity. Used as
    /// the denominator for the DF ratio. Falls back to 0 for empty corpus.
    pub fn count_items_with_entities(&self) -> Result<usize> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT COUNT(DISTINCT item_id) FROM item_entities")?;
        let n: i64 = stmt.query_row([], |r| r.get(0))?;
        Ok(n.max(0) as usize)
    }

    /// Aggregate `(kind, value, df)` rows across the whole corpus. Powers the
    /// panorama entity-graph view + the `entity_graph::build_from_items`
    /// recomputation. Caps at `limit` rows ordered by df descending.
    pub fn list_entity_aggregates(&self, limit: usize) -> Result<Vec<(String, String, usize)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT kind, value, COUNT(DISTINCT item_id) AS df FROM item_entities \
             GROUP BY kind, value ORDER BY df DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? as usize,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // ========================================================================
    // item_links — emitted link rows
    // ========================================================================

    /// Idempotent replace: clears all link rows that mention `item_id` on
    /// either side, then inserts the fresh `links` set.
    ///
    /// Mirrors `reindex.rs` "清旧 → 加新". Symmetric kinds use canonical
    /// `(item_a < item_b)` ordering enforced by the caller (the linker).
    /// Returns rows inserted.
    pub fn replace_item_links(&self, item_id: &str, links: &[ComputedLink]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM item_links WHERE item_a = ?1 OR item_b = ?1",
            params![item_id],
        )?;
        let now = chrono::Utc::now().to_rfc3339();
        let mut inserted = 0;
        for link in links {
            // OR IGNORE handles the case where two items co-link in the same
            // direction (would happen only on directed kinds; primary key
            // already prevents dupes for symmetric).
            let n = tx.execute(
                "INSERT OR IGNORE INTO item_links \
                 (item_a, item_b, kind, weight, directed, evidence, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    link.item_a,
                    link.item_b,
                    link.kind.as_str(),
                    link.weight as f64,
                    link.kind.is_directed() as i64,
                    link.evidence,
                    now,
                ],
            )?;
            inserted += n;
        }
        tx.commit()?;
        Ok(inserted)
    }

    /// Drop all link rows that mention `item_id` on either side. Returns rows
    /// affected. Called from delete / purge paths and from the linker's own
    /// idempotent rebuild step.
    pub fn purge_item_links(&self, item_id: &str) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM item_links WHERE item_a = ?1 OR item_b = ?1",
            params![item_id],
        )?;
        Ok(n)
    }

    /// All links touching `item_id` (either side). Returns rows as tuples
    /// `(other_item_id, kind, weight, directed, evidence)`. The caller is
    /// expected to know its own `item_id`, so we only return the *other* side.
    /// `kind` is returned as the canonical string (`shared_entity` /
    /// `semantic_near` / `explicit_ref`).
    pub fn list_links_for_item(
        &self,
        item_id: &str,
    ) -> Result<Vec<LinkRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT item_a, item_b, kind, weight, directed, evidence \
             FROM item_links WHERE item_a = ?1 OR item_b = ?1 \
             ORDER BY weight DESC",
        )?;
        let rows = stmt.query_map(params![item_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, f64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (a, b, kind, weight, directed, evidence) = row?;
            let other = if a == item_id { b } else { a.clone() };
            out.push(LinkRow {
                other_item_id: other,
                kind,
                weight: weight as f32,
                directed: directed != 0,
                evidence,
                citing_is_self: directed != 0 && a == item_id,
            });
        }
        Ok(out)
    }

    /// Metadata `(id, title, url)` for items that are link-candidates against
    /// `new_item_id` — i.e. every other non-deleted item. URL may be absent.
    /// Used by the explicit-ref scan in [`crate::linker::compute_links_for_item`].
    ///
    /// Caps at 10 000 rows for safety (a personal vault is rarely above this;
    /// past that, panorama UI cannot render the graph anyway).
    pub fn list_link_candidate_metadata(
        &self,
        new_item_id: &str,
    ) -> Result<Vec<(String, String, Option<String>)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, title, url FROM items \
             WHERE is_deleted = 0 AND id != ?1 ORDER BY created_at LIMIT 10000",
        )?;
        let rows = stmt.query_map(params![new_item_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Test/admin helper: count rows in `item_links` (debug + integration
    /// tests assert no-orphan after delete).
    pub fn count_all_item_links(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM item_links", [], |r| r.get(0))
            .optional()?
            .unwrap_or(0);
        Ok(n.max(0) as usize)
    }

    /// Test/admin helper: count rows in `item_entities`.
    pub fn count_all_item_entities(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM item_entities", [], |r| r.get(0))
            .optional()?
            .unwrap_or(0);
        Ok(n.max(0) as usize)
    }
}

/// Row returned by [`Store::list_links_for_item`]. Cheap shape — Reader /
/// related route reads it directly.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LinkRow {
    pub other_item_id: String,
    /// `shared_entity` / `semantic_near` / `explicit_ref`.
    pub kind: String,
    pub weight: f32,
    pub directed: bool,
    pub evidence: String,
    /// For directed `explicit_ref`: `true` if this item is the citing side
    /// (i.e. the row's `item_a == item_id`). For symmetric kinds, `false`.
    pub citing_is_self: bool,
}

fn kind_str(k: &crate::entities::EntityKind) -> &'static str {
    use crate::entities::EntityKind::*;
    match k {
        Person => "person",
        Money => "money",
        Date => "date",
        Organization => "organization",
    }
}

/// Tests at the module level exercise the round-trip SQL behaviour with a
/// real in-memory `Store`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Key32;
    use crate::entities::{Entity, EntityKind};

    fn fresh() -> Store {
        Store::open_memory().unwrap()
    }

    fn mk_entity(kind: EntityKind, value: &str) -> Entity {
        Entity {
            kind,
            value: value.to_string(),
            byte_start: 0,
            byte_end: value.len(),
        }
    }

    #[test]
    fn replace_item_entities_is_idempotent() {
        let s = fresh();
        let ents = vec![
            mk_entity(EntityKind::Person, "张三"),
            mk_entity(EntityKind::Organization, "ACME 公司"),
        ];
        s.replace_item_entities("it1", &ents).unwrap();
        s.replace_item_entities("it1", &ents).unwrap();
        assert_eq!(s.count_all_item_entities().unwrap(), 2);
    }

    #[test]
    fn find_items_by_entity_returns_co_mentioners() {
        let s = fresh();
        let e1 = vec![mk_entity(EntityKind::Person, "张三")];
        let e2 = vec![mk_entity(EntityKind::Person, "张三")];
        s.replace_item_entities("a", &e1).unwrap();
        s.replace_item_entities("b", &e2).unwrap();
        let mut found = s.find_items_by_entity("person", "张三").unwrap();
        found.sort();
        assert_eq!(found, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn entity_document_frequency_counts_distinct_items() {
        let s = fresh();
        s.replace_item_entities(
            "a",
            &[
                mk_entity(EntityKind::Person, "张三"),
                mk_entity(EntityKind::Person, "张三"),
            ],
        )
        .unwrap();
        s.replace_item_entities("b", &[mk_entity(EntityKind::Person, "张三")])
            .unwrap();
        // 2 items mention 张三, although item `a` lists him twice
        assert_eq!(s.entity_document_frequency("person", "张三").unwrap(), 2);
    }

    #[test]
    fn purge_item_entities_drops_only_targeted_item() {
        let s = fresh();
        s.replace_item_entities(
            "a",
            &[mk_entity(EntityKind::Person, "张三")],
        )
        .unwrap();
        s.replace_item_entities(
            "b",
            &[mk_entity(EntityKind::Person, "李四")],
        )
        .unwrap();
        assert_eq!(s.purge_item_entities("a").unwrap(), 1);
        assert_eq!(s.count_all_item_entities().unwrap(), 1);
        let remaining = s.find_items_by_entity("person", "李四").unwrap();
        assert_eq!(remaining, vec!["b".to_string()]);
    }

    #[test]
    fn replace_item_links_clears_old_then_inserts() {
        let s = fresh();
        let links = vec![ComputedLink {
            item_a: "a".into(),
            item_b: "b".into(),
            kind: LinkKind::SharedEntity,
            weight: 3.0,
            evidence: "person:张三".into(),
        }];
        let n1 = s.replace_item_links("a", &links).unwrap();
        assert_eq!(n1, 1);
        let n2 = s.replace_item_links("a", &links).unwrap();
        assert_eq!(n2, 1);
        assert_eq!(s.count_all_item_links().unwrap(), 1);
    }

    #[test]
    fn list_links_for_item_both_sides() {
        let s = fresh();
        let links = vec![
            ComputedLink {
                item_a: "a".into(),
                item_b: "b".into(),
                kind: LinkKind::SharedEntity,
                weight: 2.0,
                evidence: "".into(),
            },
            ComputedLink {
                item_a: "c".into(),
                item_b: "a".into(),
                kind: LinkKind::SemanticNear,
                weight: 0.9,
                evidence: "".into(),
            },
        ];
        s.replace_item_links("a", &links).unwrap();
        let rows = s.list_links_for_item("a").unwrap();
        assert_eq!(rows.len(), 2);
        // Should appear sorted by weight DESC: shared_entity (2.0) > semantic_near (0.9)
        assert_eq!(rows[0].kind, "shared_entity");
        assert_eq!(rows[1].kind, "semantic_near");
    }

    #[test]
    fn purge_item_links_drops_both_sides() {
        let s = fresh();
        let links = vec![
            ComputedLink {
                item_a: "a".into(),
                item_b: "b".into(),
                kind: LinkKind::SharedEntity,
                weight: 1.0,
                evidence: "".into(),
            },
            ComputedLink {
                item_a: "c".into(),
                item_b: "a".into(),
                kind: LinkKind::SemanticNear,
                weight: 0.85,
                evidence: "".into(),
            },
        ];
        s.replace_item_links("a", &links).unwrap();
        assert_eq!(s.count_all_item_links().unwrap(), 2);
        let dropped = s.purge_item_links("a").unwrap();
        assert_eq!(dropped, 2);
        assert_eq!(s.count_all_item_links().unwrap(), 0);
    }

    #[test]
    fn list_link_candidate_metadata_excludes_self_and_deleted() {
        let s = fresh();
        let dek = Key32::generate();
        let id_a = s
            .insert_item(&dek, "Alpha", "body A", Some("https://a.local"), "note", None, None)
            .unwrap();
        let id_b = s
            .insert_item(&dek, "Beta", "body B", Some("https://b.local"), "note", None, None)
            .unwrap();
        let id_c = s
            .insert_item(&dek, "Gamma", "body C", None, "note", None, None)
            .unwrap();
        // Soft-delete C
        s.delete_item(&id_c).unwrap();
        let meta = s.list_link_candidate_metadata(&id_a).unwrap();
        let ids: Vec<&String> = meta.iter().map(|(id, _, _)| id).collect();
        assert!(ids.contains(&&id_b));
        assert!(!ids.contains(&&id_a));
        assert!(!ids.contains(&&id_c));
    }
}
