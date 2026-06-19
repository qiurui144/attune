//! 监控引擎确定性层测试 —— spec §9.1 六类下限（happy / edge / error / adversarial /
//! dedup / resource）。这些路径零 LLM（编译期签名守卫）。

use super::*;
use crate::entities::{Entity, EntityKind};
use std::collections::HashMap;

const NOW: &str = "2026-06-19T00:00:00Z";

fn ent(kind: EntityKind, value: &str) -> Entity {
    Entity {
        kind,
        value: value.to_string(),
        byte_start: 0,
        byte_end: value.len(),
    }
}

fn item(id: &str, title: &str, content: &str, created: &str) -> ItemMeta {
    ItemMeta {
        id: id.to_string(),
        title: title.to_string(),
        content: content.to_string(),
        source_type: "rss".to_string(),
        vector: None,
        entities: vec![],
        created_at: created.to_string(),
        content_hash: format!("h-{id}"),
    }
}

fn kw_watch(id: &str, keywords: &[&str]) -> Watch {
    Watch {
        id: id.to_string(),
        label: format!("watch-{id}"),
        keywords: keywords.iter().map(|s| s.to_string()).collect(),
        entities: vec![],
        anchor_text: String::new(),
        anchor_vec: None,
        source_ids: vec![],
        match_threshold: 0.5,
        source_weights: HashMap::new(),
        enabled: true,
    }
}

// ── happy ──────────────────────────────────────────────────────────────────

#[test]
fn happy_keyword_match_with_reasons() {
    let m = WatchMatcher::default();
    let items = vec![
        item("i1", "RVV news", "the RVV extension lands", "2026-06-18T00:00:00Z"),
        item("i2", "Other", "totally unrelated topic", "2026-06-18T00:00:00Z"),
        item("i3", "RVV deep", "more about RVV vectors", "2026-06-18T00:00:00Z"),
    ];
    let w = kw_watch("w1", &["RVV"]);
    let hits = m.evaluate(&items, &[w], &HashMap::new(), NOW);
    assert_eq!(hits.len(), 2, "2 items contain RVV");
    assert!(hits.iter().all(|h| h.reasons.iter().any(|r| r.contains("RVV"))));
    assert!(hits.iter().all(|h| h.watch_id == "w1"));
}

#[test]
fn happy_triage_orders_by_score_then_recency() {
    let m = WatchMatcher::default();
    // both match same keyword; i_new is more recent → ranks first.
    let items = vec![
        item("old", "RVV old", "RVV here", "2026-01-01T00:00:00Z"),
        item("new", "RVV new", "RVV here", "2026-06-18T00:00:00Z"),
    ];
    let hits = m.evaluate(&items, &[kw_watch("w", &["RVV"])], &HashMap::new(), NOW);
    assert_eq!(hits[0].item_id, "new", "more recent ranks first (recency factor)");
    assert!(hits[0].score >= hits[1].score);
}

#[test]
fn happy_vector_match() {
    let mut w = kw_watch("wv", &[]);
    w.anchor_vec = Some(vec![1.0, 0.0, 0.0]);
    w.match_threshold = 0.8;
    let mut it = item("v1", "doc", "body", "2026-06-18T00:00:00Z");
    it.vector = Some(vec![0.99, 0.01, 0.0]);
    let mut it2 = item("v2", "doc2", "body2", "2026-06-18T00:00:00Z");
    it2.vector = Some(vec![0.0, 1.0, 0.0]); // orthogonal → no match
    let hits = WatchMatcher::default().evaluate(&[it, it2], &[w], &HashMap::new(), NOW);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item_id, "v1");
    assert!(hits[0].reasons.iter().any(|r| r.contains("向量相似")));
}

// ── edge ─────────────────────────────────────────────────────────────────

#[test]
fn edge_no_match_returns_empty() {
    let m = WatchMatcher::default();
    let items = vec![item("i", "x", "nothing relevant", "2026-06-18T00:00:00Z")];
    let hits = m.evaluate(&items, &[kw_watch("w", &["RVV"])], &HashMap::new(), NOW);
    assert!(hits.is_empty());
}

#[test]
fn edge_disabled_watch_skipped() {
    let mut w = kw_watch("w", &["RVV"]);
    w.enabled = false;
    let items = vec![item("i", "RVV", "RVV body", "2026-06-18T00:00:00Z")];
    assert!(WatchMatcher::default()
        .evaluate(&items, &[w], &HashMap::new(), NOW)
        .is_empty());
}

#[test]
fn edge_no_criteria_watch_skipped() {
    let w = kw_watch("w", &[]); // no keywords, entities, anchor → has_criteria()=false
    assert!(!w.has_criteria());
    let items = vec![item("i", "anything", "anything", "2026-06-18T00:00:00Z")];
    assert!(WatchMatcher::default()
        .evaluate(&items, &[w], &HashMap::new(), NOW)
        .is_empty());
}

#[test]
fn edge_source_binding_filters() {
    let mut w = kw_watch("w", &["RVV"]);
    w.source_ids = vec!["git".to_string()]; // only git source
    let mut it = item("i", "RVV", "RVV body", "2026-06-18T00:00:00Z");
    it.source_type = "rss".to_string(); // not git → filtered out
    assert!(WatchMatcher::default()
        .evaluate(&[it], &[w], &HashMap::new(), NOW)
        .is_empty());
}

#[test]
fn edge_entity_overlap_match() {
    let mut w = kw_watch("we", &[]);
    w.entities = vec![ent(EntityKind::Organization, "某某有限公司")];
    w.match_threshold = 0.4;
    let mut it = item("i", "news", "公司动态", "2026-06-18T00:00:00Z");
    it.entities = vec![ent(EntityKind::Organization, "某某有限公司")];
    let hits = WatchMatcher::default().evaluate(&[it], &[w], &HashMap::new(), NOW);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].reasons.iter().any(|r| r.contains("实体重叠")));
}

// ── error / robustness ──────────────────────────────────────────────────

#[test]
fn error_anchor_dim_mismatch_degrades_no_panic() {
    let mut w = kw_watch("w", &["RVV"]);
    w.anchor_vec = Some(vec![1.0, 0.0]); // dim 2
    let mut it = item("i", "RVV", "RVV body", "2026-06-18T00:00:00Z");
    it.vector = Some(vec![1.0, 0.0, 0.0]); // dim 3 — mismatch
    // keyword still matches; vector silently contributes 0 (no panic).
    let hits = WatchMatcher::default().evaluate(&[it], &[w], &HashMap::new(), NOW);
    assert_eq!(hits.len(), 1, "keyword carries the match despite vec mismatch");
}

#[test]
fn error_malformed_created_at_no_panic() {
    let items = vec![item("i", "RVV", "RVV", "not-a-date")];
    let hits = WatchMatcher::default().evaluate(&items, &[kw_watch("w", &["RVV"])], &HashMap::new(), NOW);
    assert_eq!(hits.len(), 1, "bad date → recency 1.0, still matches");
}

// ── adversarial ─────────────────────────────────────────────────────────

#[test]
fn adversarial_regex_metachars_treated_literally() {
    // keyword with regex metachars must NOT be executed as regex.
    let w = kw_watch("w", &[".*", "a+b"]);
    let it = item("i", "lit", "this has a+b literally and .* too", "2026-06-18T00:00:00Z");
    let hits = WatchMatcher::default().evaluate(&[it.clone()], &[w], &HashMap::new(), NOW);
    assert_eq!(hits.len(), 1);
    // a control item that would match `.*` if it were regex but lacks the literal.
    let w2 = kw_watch("w2", &[".*"]);
    let plain = item("p", "plain", "no metachar sequence present here", "2026-06-18T00:00:00Z");
    assert!(
        WatchMatcher::default().evaluate(&[plain], &[w2], &HashMap::new(), NOW).is_empty(),
        "'.*' must be literal, not match-anything"
    );
}

#[test]
fn adversarial_keyword_repetition_does_not_inflate_score() {
    // distinct-keyword ratio, not raw count → repeated word can't exceed 1.0.
    let w = kw_watch("w", &["spam"]);
    let many = "spam ".repeat(1000);
    let it_many = item("m", "t", &many, "2026-06-18T00:00:00Z");
    let it_one = item("o", "t2", "spam once", "2026-06-18T00:00:00Z");
    let hits = WatchMatcher::default().evaluate(&[it_many, it_one], &[w], &HashMap::new(), NOW);
    // both fully match (ratio 1.0), score difference comes only from recency (equal) → equal.
    assert_eq!(hits.len(), 2);
    assert!((hits[0].score - hits[1].score).abs() < 1e-4, "repetition does not inflate");
}

#[test]
fn adversarial_very_long_keyword() {
    let long_kw = "x".repeat(10_000);
    let w = kw_watch("w", &[&long_kw]);
    let it = item("i", "t", "short content", "2026-06-18T00:00:00Z");
    // no panic; no match.
    assert!(WatchMatcher::default()
        .evaluate(&[it], &[w], &HashMap::new(), NOW)
        .is_empty());
}

// ── dedup ───────────────────────────────────────────────────────────────

#[test]
fn dedup_same_item_twice_in_batch_unique() {
    let it = item("dup", "RVV", "RVV body", "2026-06-18T00:00:00Z");
    let hits = WatchMatcher::default().evaluate(
        &[it.clone(), it.clone()],
        &[kw_watch("w", &["RVV"])],
        &HashMap::new(),
        NOW,
    );
    assert_eq!(hits.len(), 1, "same item id deduped within a watch");
}

#[test]
fn dedup_same_content_hash_grouped() {
    let mut a = item("a", "RVV Title A", "RVV from source A", "2026-06-18T00:00:00Z");
    let mut b = item("b", "RVV Title B", "RVV from source B", "2026-06-18T00:00:00Z");
    a.content_hash = "same".to_string();
    b.content_hash = "same".to_string();
    let hits = WatchMatcher::default().evaluate(&[a, b], &[kw_watch("w", &["RVV"])], &HashMap::new(), NOW);
    assert_eq!(hits.len(), 2);
    let g0 = hits[0].dedup_group.clone();
    let g1 = hits[1].dedup_group.clone();
    assert!(g0.is_some() && g0 == g1, "same content_hash → same dedup_group");
}

#[test]
fn dedup_same_normalized_title_grouped() {
    let a = item("a", "RVV Update!", "RVV body a", "2026-06-18T00:00:00Z");
    let b = item("b", "rvv update", "RVV body b", "2026-06-18T00:00:00Z"); // same after normalize
    let hits = WatchMatcher::default().evaluate(&[a, b], &[kw_watch("w", &["RVV"])], &HashMap::new(), NOW);
    let groups: Vec<_> = hits.iter().filter_map(|h| h.dedup_group.clone()).collect();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0], groups[1], "normalized-title match → same group");
}

#[test]
fn dedup_distinct_items_not_grouped() {
    let a = item("a", "RVV alpha", "RVV alpha body", "2026-06-18T00:00:00Z");
    let b = item("b", "RVV beta different", "RVV beta body", "2026-06-18T00:00:00Z");
    let hits = WatchMatcher::default().evaluate(&[a, b], &[kw_watch("w", &["RVV"])], &HashMap::new(), NOW);
    assert!(hits.iter().all(|h| h.dedup_group.is_none()), "distinct titles → no group");
}

// ── triage: source weight + interaction ──────────────────────────────────

#[test]
fn triage_source_weight_boosts() {
    let mut w = kw_watch("w", &["RVV"]);
    w.source_weights.insert("git".to_string(), 3.0);
    let mut git_it = item("g", "RVV git", "RVV body", "2026-06-18T00:00:00Z");
    git_it.source_type = "git".to_string();
    let rss_it = item("r", "RVV rss", "RVV body", "2026-06-18T00:00:00Z");
    let hits = WatchMatcher::default().evaluate(&[rss_it, git_it], &[w], &HashMap::new(), NOW);
    assert_eq!(hits[0].item_id, "g", "weighted source ranks first");
}

#[test]
fn triage_interaction_signal_boosts() {
    let w = kw_watch("w", &["RVV"]);
    let a = item("a", "RVV a", "RVV body", "2026-06-18T00:00:00Z");
    let b = item("b", "RVV b", "RVV body", "2026-06-18T00:00:00Z");
    let mut sig = HashMap::new();
    sig.insert("b".to_string(), InteractionSignals { interaction_count: 20 });
    let hits = WatchMatcher::default().evaluate(&[a, b], &[w], &sig, NOW);
    assert_eq!(hits[0].item_id, "b", "interacted item ranks first");
    assert!(hits[0].reasons.iter().any(|r| r.contains("历史交互")));
}

// ── resource (scale) ──────────────────────────────────────────────────────

#[test]
fn resource_100_watch_500_item_linear() {
    let m = WatchMatcher::default();
    let items: Vec<ItemMeta> = (0..500)
        .map(|i| item(&format!("i{i}"), &format!("title {i}"), &format!("RVV content {i}"), "2026-06-18T00:00:00Z"))
        .collect();
    let watches: Vec<Watch> = (0..100).map(|i| kw_watch(&format!("w{i}"), &["RVV"])).collect();
    let start = std::time::Instant::now();
    let hits = m.evaluate(&items, &watches, &HashMap::new(), NOW);
    let elapsed = start.elapsed();
    // 100 watch × 500 item all contain "RVV" → 50_000 hits.
    assert_eq!(hits.len(), 50_000);
    // sanity: incremental match over new batch is tractable (not O(N^2) over full vault).
    assert!(elapsed.as_secs() < 10, "scale case must stay tractable, took {elapsed:?}");
}

// ── idempotence ───────────────────────────────────────────────────────────

#[test]
fn evaluate_is_idempotent() {
    let m = WatchMatcher::default();
    let items = vec![
        item("a", "RVV a", "RVV body a", "2026-06-18T00:00:00Z"),
        item("b", "RVV b", "RVV body b", "2026-06-17T00:00:00Z"),
    ];
    let w = vec![kw_watch("w", &["RVV"])];
    let h1 = m.evaluate(&items, &w, &HashMap::new(), NOW);
    let h2 = m.evaluate(&items, &w, &HashMap::new(), NOW);
    let sig = |hs: &[WatchHit]| {
        hs.iter()
            .map(|h| (h.watch_id.clone(), h.item_id.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(sig(&h1), sig(&h2), "same input → same hit signature/order");
}
