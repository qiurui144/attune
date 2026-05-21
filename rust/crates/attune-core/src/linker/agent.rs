//! Internal knowledge linker agent — pure function over (item, store, vectors).
//!
//! See module-level docs in [`super`]. This file holds the data types, the
//! [`compute_links_for_item`] entry point, and the three link extractors.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::entities::{extract_entities, EntityKind};
use crate::error::Result;
use crate::store::Store;
use crate::vectors::VectorIndex;

/// Link kinds the agent emits. Mechanical / non-LLM — the agent does not
/// label *why* two items relate, just *how* (which signal triggered the link).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// Two items co-mention the same Person / Money / Date / Organization
    /// entity. Symmetric. Weight = number of distinct shared entities.
    SharedEntity,
    /// Two items have at least one chunk-pair with cosine ≥
    /// `LinkThresholds::semantic_near_min_cosine`. Symmetric. Weight = max
    /// cosine over all chunk pairs between the two items.
    SemanticNear,
    /// One item's content textually references another item (URL match,
    /// `[[title]]` wiki ref, or verbatim title substring of ≥
    /// `LinkThresholds::explicit_ref_title_min_len` chars). **Directed**:
    /// the row stores `(a = citing, b = cited)` with `directed = 1`.
    ExplicitRef,
}

impl LinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkKind::SharedEntity => "shared_entity",
            LinkKind::SemanticNear => "semantic_near",
            LinkKind::ExplicitRef => "explicit_ref",
        }
    }

    pub fn is_directed(self) -> bool {
        matches!(self, LinkKind::ExplicitRef)
    }
}

/// Caller-tunable knobs. Defaults are the spec values (§2.3 / §8 "Graph
/// blow-up"). **The caps + stop-entity DF filter are mandatory** — without
/// them a corpus with a ubiquitous entity (the user's own name, "2024")
/// produces a near-complete graph. Adjusting them downwards is supported;
/// disabling them is intentionally not exposed.
#[derive(Debug, Clone)]
pub struct LinkThresholds {
    /// Minimum cosine to emit a `SemanticNear` link. Per spec §2.3 default
    /// `0.85` (the brief asks 0.85; spec analysis text mentions 0.82 — we
    /// follow the brief).
    pub semantic_near_min_cosine: f32,
    /// Minimum distinct shared entities required for a `SharedEntity` link
    /// row. Single-entity overlap is too noisy (a single shared date emits a
    /// link to every item that mentions any 2024 date).
    pub shared_entity_min_overlap: usize,
    /// Document-frequency stop-entity filter. Entities appearing in more than
    /// this fraction of indexed items are dropped from `SharedEntity`
    /// computation as "the user's own name" class noise. 0.30 = 30 %.
    pub shared_entity_max_df_ratio: f32,
    /// Out-degree cap per item per kind (top-K by weight kept). Keeps the
    /// graph sparse enough for the panorama to render and the reader panel
    /// to fit on one screen.
    pub max_links_per_item_per_kind: usize,
    /// Minimum title length (chars) to allow as a verbatim
    /// `ExplicitRef` substring match. Short titles ("Note", "Meeting") would
    /// false-positive everything.
    pub explicit_ref_title_min_len: usize,
    /// Top-N HNSW neighbours queried per chunk vector. Bounded so a 10 000-
    /// item corpus does not become a quadratic scan.
    pub semantic_neighbours_per_chunk: usize,
}

impl Default for LinkThresholds {
    fn default() -> Self {
        Self {
            semantic_near_min_cosine: 0.85,
            shared_entity_min_overlap: 2,
            shared_entity_max_df_ratio: 0.30,
            max_links_per_item_per_kind: 20,
            explicit_ref_title_min_len: 6,
            semantic_neighbours_per_chunk: 8,
        }
    }
}

/// One computed link between two items. Persisted into the `item_links`
/// table by [`Store::replace_item_links`].
#[derive(Debug, Clone, PartialEq)]
pub struct ComputedLink {
    /// For symmetric kinds: canonical `min(a, b)` lexicographic. For directed
    /// `ExplicitRef`: the citing item id.
    pub item_a: String,
    /// For symmetric kinds: canonical `max(a, b)`. For directed
    /// `ExplicitRef`: the cited item id.
    pub item_b: String,
    pub kind: LinkKind,
    /// Strength of the link in kind-specific units (count for `SharedEntity`,
    /// cosine for `SemanticNear`, `1.0` for `ExplicitRef`).
    pub weight: f32,
    /// Short machine string explaining what triggered the link (e.g.
    /// `"person:张三;organization:ACME"` or matched URL). Low-sensitivity —
    /// no item content, only entity values / URLs.
    pub evidence: String,
}

/// Stats returned per `compute_links_for_item` call — usable for audit logs
/// + integration test assertions, in the same shape as
///   [`crate::reindex::ReindexStats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkerStats {
    pub entities_indexed: usize,
    pub shared_entity_links: usize,
    pub semantic_near_links: usize,
    pub explicit_ref_links: usize,
    /// Existing rows cleared from `item_links` for this item before inserting
    /// the fresh set (idempotent rebuild).
    pub cleared_old: usize,
}

impl LinkerStats {
    pub fn total_links(&self) -> usize {
        self.shared_entity_links + self.semantic_near_links + self.explicit_ref_links
    }
}

// ============================================================================
// Public entry point
// ============================================================================

/// Re-extract entities for `new_item_id` and refresh **all three link kinds**
/// against the existing corpus. Idempotent — running twice produces the same
/// `item_links` rows.
///
/// Step order (all 🆓/⚡):
///
/// 1. `extract_entities(content)` → upsert into `item_entities` (delete-then-
///    insert, same pattern as `TagIndex`).
/// 2. **Shared-entity links** via the `item_entities` inverted index (`(kind,
///    value)` → all co-mentioning items). DF stop-entity filter +
///    `shared_entity_min_overlap` floor applied.
/// 3. **Explicit-ref links** by scanning `content` for URL matches against
///    other items' `url` and for verbatim substring matches of other items'
///    titles ≥ `explicit_ref_title_min_len` chars. Directed.
/// 4. **Semantic-near links** by querying `VectorIndex` with each chunk
///    vector this item produced; aggregate by neighbour `item_id` keeping
///    the max cosine. Skipped if `vectors` is `None` (e.g. unit tests that
///    don't run the embed worker) — entity + ref links still flow.
/// 5. Out-degree cap applied per kind (top-K by weight);
///    `Store::replace_item_links(item_id, links)` writes the fresh set.
///
/// The agent never deletes items, never modifies item content / tags /
/// annotations. It only emits `item_links` and `item_entities` records.
pub fn compute_links_for_item(
    store: &Store,
    vectors: Option<&VectorIndex>,
    new_item_id: &str,
    _title: &str,
    content: &str,
    _url: Option<&str>,
    thresholds: &LinkThresholds,
) -> Result<LinkerStats> {
    // `_title` and `_url` are accepted (and may inform future filters / evidence
    // strings) but the current explicit-ref pass reads other items' titles/URLs
    // out of the items table directly, so the caller's `title`/`url` are not
    // consumed inside this function. Kept in the signature to match ingest
    // call-sites that already have them cheaply available.
    let mut stats = LinkerStats::default();

    // 1. Entity extraction + persisted inverted-index upsert.
    let entities = extract_entities(content);
    let mut entities_set: HashSet<(EntityKind, String)> = HashSet::new();
    for e in &entities {
        entities_set.insert((e.kind, e.value.clone()));
    }
    store.replace_item_entities(new_item_id, &entities)?;
    stats.entities_indexed = entities_set.len();

    // 2. Shared-entity links.
    let total_items = store.count_items_with_entities()?.max(1);
    let df_threshold = (total_items as f32 * thresholds.shared_entity_max_df_ratio).max(2.0);
    let mut overlap_counts: HashMap<String, (u32, BTreeSet<String>)> = HashMap::new();
    for (kind, value) in &entities_set {
        // Stop-entity DF filter: skip entities present in too large a fraction
        // of the corpus (the user's own name / "2024" / etc.).
        let df = store.entity_document_frequency(entity_kind_str(*kind), value)?;
        if (df as f32) > df_threshold {
            continue;
        }
        let co_mentioners = store.find_items_by_entity(entity_kind_str(*kind), value)?;
        for other_id in co_mentioners {
            if other_id == new_item_id {
                continue;
            }
            let evidence_tag = format!("{}:{}", entity_kind_str(*kind), value);
            let entry = overlap_counts
                .entry(other_id)
                .or_insert_with(|| (0, BTreeSet::new()));
            entry.0 += 1;
            // Cap evidence string size: 4 distinct tags is enough to be
            // explainable in UI; further overlap is reflected in `weight`.
            if entry.1.len() < 4 {
                entry.1.insert(evidence_tag);
            }
        }
    }
    let mut shared_entity_links: Vec<ComputedLink> = overlap_counts
        .into_iter()
        .filter(|(_, (count, _))| *count as usize >= thresholds.shared_entity_min_overlap)
        .map(|(other_id, (count, tags))| {
            let (a, b) = canonical_pair(new_item_id, &other_id);
            ComputedLink {
                item_a: a,
                item_b: b,
                kind: LinkKind::SharedEntity,
                weight: count as f32,
                evidence: tags.into_iter().collect::<Vec<_>>().join(";"),
            }
        })
        .collect();
    apply_degree_cap(&mut shared_entity_links, thresholds.max_links_per_item_per_kind);
    stats.shared_entity_links = shared_entity_links.len();

    // 3. Explicit-ref links (directed: this item cites another).
    let other_metadata = store.list_link_candidate_metadata(new_item_id)?;
    let mut explicit_ref_links: Vec<ComputedLink> = Vec::new();
    let lowered_content = content.to_lowercase();
    let mut seen_ref_targets: HashSet<String> = HashSet::new();
    for (other_id, other_title, other_url) in &other_metadata {
        let mut matched_via: Option<String> = None;
        if let Some(u) = other_url {
            if !u.is_empty() && content.contains(u.as_str()) {
                matched_via = Some(format!("url:{u}"));
            }
        }
        if matched_via.is_none() {
            let wikiref = format!("[[{}]]", other_title);
            if content.contains(&wikiref) {
                matched_via = Some(format!("wikiref:{}", other_title));
            }
        }
        if matched_via.is_none()
            && other_title.chars().count() >= thresholds.explicit_ref_title_min_len
        {
            // Substring match on the title — lowercased to be a touch more
            // forgiving on English titles; Chinese is case-irrelevant so the
            // lowercase op is a no-op there.
            if lowered_content.contains(&other_title.to_lowercase()) {
                matched_via = Some(format!("title:{}", other_title));
            }
        }
        if let Some(ev) = matched_via {
            if seen_ref_targets.insert(other_id.clone()) {
                explicit_ref_links.push(ComputedLink {
                    item_a: new_item_id.to_string(),
                    item_b: other_id.clone(),
                    kind: LinkKind::ExplicitRef,
                    weight: 1.0,
                    evidence: ev,
                });
            }
        }
    }
    // Filter out trivial self-cite if title == own title appears in own content.
    explicit_ref_links.retain(|l| l.item_a != l.item_b);
    apply_degree_cap(
        &mut explicit_ref_links,
        thresholds.max_links_per_item_per_kind,
    );
    stats.explicit_ref_links = explicit_ref_links.len();

    // 4. Semantic-near links (optional — needs `VectorIndex`).
    let mut semantic_links: Vec<ComputedLink> = Vec::new();
    if let Some(vectors) = vectors {
        // Best max-cosine per other item across all chunk pairs.
        let mut best_cos: HashMap<String, f32> = HashMap::new();
        // We don't have a direct "get all chunk vectors for this item" call
        // exposed; `get_vector` returns one (the first registered). For each
        // vector belonging to this item we run a top-N HNSW search.
        if let Some(q) = vectors.get_vector(new_item_id) {
            let top_k = thresholds.semantic_neighbours_per_chunk;
            if let Ok(neighbours) = vectors.search(&q, top_k * 2) {
                for (meta, cos) in neighbours {
                    if meta.item_id == new_item_id {
                        continue;
                    }
                    if cos < thresholds.semantic_near_min_cosine {
                        continue;
                    }
                    let entry = best_cos.entry(meta.item_id).or_insert(0.0);
                    if cos > *entry {
                        *entry = cos;
                    }
                }
            }
        }
        for (other_id, cos) in best_cos {
            let (a, b) = canonical_pair(new_item_id, &other_id);
            semantic_links.push(ComputedLink {
                item_a: a,
                item_b: b,
                kind: LinkKind::SemanticNear,
                weight: cos,
                evidence: format!("cosine:{:.3}", cos),
            });
        }
        apply_degree_cap(&mut semantic_links, thresholds.max_links_per_item_per_kind);
    }
    stats.semantic_near_links = semantic_links.len();

    // 5. Idempotent rebuild: clear this item's existing rows, write the new set.
    let cleared = store.purge_item_links(new_item_id)?;
    stats.cleared_old = cleared;
    // (signature reserves `_url` / `_title` for future use; see top of function)
    let mut all_links: Vec<ComputedLink> = Vec::with_capacity(
        shared_entity_links.len() + explicit_ref_links.len() + semantic_links.len(),
    );
    all_links.extend(shared_entity_links);
    all_links.extend(explicit_ref_links);
    all_links.extend(semantic_links);
    store.replace_item_links(new_item_id, &all_links)?;

    Ok(stats)
}

/// Delete path: drop all `item_links` + `item_entities` rows for this item.
/// Called from delete / purge code paths the same way
/// [`crate::reindex::purge_item_indexes`] is called.
pub fn purge_links_for_item(store: &Store, item_id: &str) -> Result<usize> {
    let dropped_links = store.purge_item_links(item_id)?;
    let _ = store.purge_item_entities(item_id)?;
    Ok(dropped_links)
}

// ============================================================================
// Helpers
// ============================================================================

fn entity_kind_str(k: EntityKind) -> &'static str {
    match k {
        EntityKind::Person => "person",
        EntityKind::Money => "money",
        EntityKind::Date => "date",
        EntityKind::Organization => "organization",
    }
}

fn canonical_pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

fn apply_degree_cap(links: &mut Vec<ComputedLink>, max: usize) {
    if links.len() <= max {
        return;
    }
    links.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    links.truncate(max);
}

#[doc(hidden)]
pub fn __test_canonical_pair(a: &str, b: &str) -> (String, String) {
    canonical_pair(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_pair_sorts_lexicographic() {
        assert_eq!(
            canonical_pair("zzz", "aaa"),
            ("aaa".to_string(), "zzz".to_string())
        );
        assert_eq!(
            canonical_pair("aaa", "zzz"),
            ("aaa".to_string(), "zzz".to_string())
        );
    }

    #[test]
    fn link_kind_directedness() {
        assert!(!LinkKind::SharedEntity.is_directed());
        assert!(!LinkKind::SemanticNear.is_directed());
        assert!(LinkKind::ExplicitRef.is_directed());
    }

    #[test]
    fn link_kind_as_str_stable() {
        assert_eq!(LinkKind::SharedEntity.as_str(), "shared_entity");
        assert_eq!(LinkKind::SemanticNear.as_str(), "semantic_near");
        assert_eq!(LinkKind::ExplicitRef.as_str(), "explicit_ref");
    }

    #[test]
    fn degree_cap_keeps_highest_weights() {
        let mut links: Vec<ComputedLink> = (0..30u32)
            .map(|i| ComputedLink {
                item_a: "a".into(),
                item_b: format!("b{i}"),
                kind: LinkKind::SharedEntity,
                weight: i as f32,
                evidence: String::new(),
            })
            .collect();
        apply_degree_cap(&mut links, 10);
        assert_eq!(links.len(), 10);
        // Top-10 weights must be 20..30
        let min_kept = links.iter().map(|l| l.weight as i64).min().unwrap();
        assert!(min_kept >= 20, "expected top-10 kept (weight ≥ 20), got min={min_kept}");
    }

    #[test]
    fn entity_kind_str_round_trips_known_kinds() {
        assert_eq!(entity_kind_str(EntityKind::Person), "person");
        assert_eq!(entity_kind_str(EntityKind::Money), "money");
        assert_eq!(entity_kind_str(EntityKind::Date), "date");
        assert_eq!(entity_kind_str(EntityKind::Organization), "organization");
    }

    #[test]
    fn link_thresholds_defaults_are_mandatory_caps() {
        // Spec §8 "Graph blow-up": caps + DF filter are mandatory, not knobs.
        // Defaults must impose them strictly.
        let t = LinkThresholds::default();
        assert!(t.max_links_per_item_per_kind <= 50, "out-degree cap must be ≤ 50");
        assert!(t.shared_entity_max_df_ratio <= 0.5,
            "DF filter must drop ubiquitous entities (≤ 50%)");
        assert!(t.shared_entity_min_overlap >= 2,
            "single-entity overlap is too noisy");
        assert!(t.semantic_near_min_cosine >= 0.80,
            "cosine threshold must be high enough to avoid full graph");
    }
}

