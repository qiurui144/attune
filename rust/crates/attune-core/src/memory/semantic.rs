//! Semantic memory (L3) — re-cluster episodic (L2) memories by topic into standing
//! "what the user knows about <topic>" summaries.
//!
//! Three-stage like A1's episodic consolidator (lock discipline mirrors it):
//!   1. [`prepare_semantic_cycle`]  — cluster live episodic memories by embedding
//!      with `hdbscan`, group by cluster, idempotency-filter by `topic_key`.
//!   2. [`generate_one_semantic_memory`] — one LLM call per topic cluster (no lock);
//!      caller checks H1 quota per call.
//!   3. [`apply_semantic_result`]   — `insert_semantic_memory` + `mark_memory_superseded`
//!      for refreshed topics.
//!
//! Cost contract: clustering is CPU-only (tier 2); summarization is tier 3, gated by
//! the same `TaskKind::MemoryConsolidation` quota as the episodic worker.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::crypto::Key32;
use crate::error::{Result, VaultError};
use crate::llm::{ChatMessage, LlmProvider};
use crate::store::MemoryRow;
use crate::store::Store;

/// A topic cluster needs at least this many episodic members to become an L3 memory —
/// fewer is too thin a signal. Plan §2.2 default.
pub const MIN_MEMS_PER_TOPIC: usize = 4;
/// Cap LLM calls per cycle (mirror A1 `MAX_BUNDLES_PER_CYCLE`). Plan §8 decision 2.
pub const MAX_TOPICS_PER_CYCLE: usize = 4;

/// One topic cluster ready to be summarized into a semantic memory.
#[derive(Debug, Clone)]
pub struct SemanticCluster {
    /// sha256 of sorted member memory-ids — idempotency key. A membership change
    /// yields a different key → a fresh insert (the refresh case).
    pub topic_key: String,
    /// Live episodic memory ids in this cluster, sorted (stable key).
    pub member_ids: Vec<String>,
    /// Decrypted episodic summaries (order not significant).
    pub member_summaries: Vec<String>,
    /// Earliest / latest window across members — the L3 row's window span.
    pub window_start: i64,
    pub window_end: i64,
    /// If this topic supersedes older semantic rows, their ids (to mark superseded).
    pub supersedes: Vec<String>,
}

/// Compute the idempotency `topic_key` from a cluster's member ids.
fn topic_key_of(sorted_member_ids: &[String]) -> String {
    let mut hasher = Sha256::new();
    for id in sorted_member_ids {
        hasher.update(id.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

// ── Phase 1: prepare (holds vault lock) ──────────────────────────────────────

/// Cluster live episodic memories by embedding, group into topic clusters, filter
/// out already-built topics by `topic_key`.
///
/// `embeddings` maps `memory_id → embedding`; memories without an embedding are
/// excluded (they are not yet vector-clusterable). Returns `Ok(None)` for an idle
/// cycle (nothing to do).
pub fn prepare_semantic_cycle(
    store: &Store,
    dek: &Key32,
    embeddings: &std::collections::HashMap<String, Vec<f32>>,
) -> Result<Option<Vec<SemanticCluster>>> {
    let episodic = store.list_live_memories(dek, "episodic", false)?;
    // Need embeddings + enough members to cluster at all.
    let embedded: Vec<&MemoryRow> = episodic
        .iter()
        .filter(|m| embeddings.contains_key(&m.id))
        .collect();
    if embedded.len() < MIN_MEMS_PER_TOPIC {
        return Ok(None);
    }

    let labels = run_hdbscan(&embedded, embeddings)?;

    // Group member indices by cluster label (-1 = hdbscan noise, dropped).
    let mut groups: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for (i, &label) in labels.iter().enumerate() {
        if label != -1 {
            groups.entry(label).or_default().push(i);
        }
    }

    // Existing semantic memories — used for the refresh / supersede decision.
    let existing_semantic = store.list_live_memories(dek, "semantic", false)?;

    let mut clusters: Vec<SemanticCluster> = Vec::new();
    for (_label, indices) in groups {
        if indices.len() < MIN_MEMS_PER_TOPIC {
            continue;
        }
        let mut member_ids: Vec<String> =
            indices.iter().map(|&i| embedded[i].id.clone()).collect();
        member_ids.sort();
        let topic_key = topic_key_of(&member_ids);

        // Idempotency: this exact membership already has a semantic row → skip.
        if existing_semantic
            .iter()
            .any(|s| s.topic_key.as_deref() == Some(topic_key.as_str()))
        {
            continue;
        }

        // Refresh: an older semantic row whose member set is a strict subset of this
        // one is superseded by the larger cluster. Subset overlap keeps it
        // conservative — only supersede when the new cluster genuinely extends the
        // old topic. A semantic row keeps its member ids in source_chunk_hashes.
        let member_set: std::collections::HashSet<&str> =
            member_ids.iter().map(|s| s.as_str()).collect();
        let supersedes: Vec<String> = existing_semantic
            .iter()
            .filter(|s| {
                let old = &s.source_chunk_hashes;
                !old.is_empty()
                    && old.len() < member_ids.len()
                    && old.iter().all(|h| member_set.contains(h.as_str()))
            })
            .map(|s| s.id.clone())
            .collect();

        let member_summaries: Vec<String> = indices
            .iter()
            .map(|&i| embedded[i].summary.clone())
            .collect();
        let window_start = indices
            .iter()
            .map(|&i| embedded[i].window_start)
            .min()
            .unwrap_or(0);
        let window_end = indices
            .iter()
            .map(|&i| embedded[i].window_end)
            .max()
            .unwrap_or(0);

        clusters.push(SemanticCluster {
            topic_key,
            member_ids,
            member_summaries,
            window_start,
            window_end,
            supersedes,
        });
        if clusters.len() >= MAX_TOPICS_PER_CYCLE {
            break;
        }
    }

    if clusters.is_empty() {
        Ok(None)
    } else {
        Ok(Some(clusters))
    }
}

fn run_hdbscan(
    members: &[&MemoryRow],
    embeddings: &std::collections::HashMap<String, Vec<f32>>,
) -> Result<Vec<i32>> {
    let dataset: Vec<Vec<f32>> = members
        .iter()
        .map(|m| embeddings.get(&m.id).cloned().unwrap_or_default())
        .collect();
    // Guard mixed dimensions (model switch) — hdbscan panics otherwise.
    if let Some(d0) = dataset.first().map(|v| v.len()) {
        if dataset.iter().any(|v| v.len() != d0) {
            return Err(VaultError::Classification(
                "memory embedding dimension mismatch in semantic clustering".into(),
            ));
        }
    }
    let clusterer = hdbscan::Hdbscan::default_hyper_params(&dataset);
    clusterer
        .cluster()
        .map_err(|e| VaultError::Classification(format!("semantic hdbscan: {e:?}")))
}

// ── Phase 2: generate (no lock) ──────────────────────────────────────────────

/// One LLM call producing a standing semantic summary for a topic cluster.
/// Returns `None` on LLM failure / empty response — caller skips that cluster.
pub fn generate_one_semantic_memory(
    llm: &dyn LlmProvider,
    cluster: &SemanticCluster,
) -> Option<String> {
    match llm.chat_with_history(&[ChatMessage::user(&build_semantic_prompt(cluster))]) {
        Ok(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

fn build_semantic_prompt(cluster: &SemanticCluster) -> String {
    let summaries = cluster
        .member_summaries
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {}", i + 1, s))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"以下是用户在不同时间段就同一主题接触/学习的 {n} 段情景记忆摘要：

{summaries}

请用 1 段（约 300 字）总结用户在这个主题上**长期积累的认知**：核心知识点、理解脉络、形成的观点或习惯。
要求：
- 中文
- 第三人称口吻（如"用户对……形成了……的理解"）
- 跨时间归纳，不要逐条复述
- 只输出一段总结，不要标题、不要列表、不要解释

总结："#,
        n = cluster.member_summaries.len(),
        summaries = summaries,
    )
}

// ── Phase 3: apply (holds vault lock) ────────────────────────────────────────

/// Outcome of applying one semantic cycle.
#[derive(Debug, Default, Clone, Copy)]
pub struct SemanticApplyResult {
    pub inserted: usize,
    pub superseded: usize,
}

/// Write semantic memories and mark refreshed older topics superseded.
/// `INSERT OR IGNORE` on `topic_key` keeps it idempotent. Returns counts plus the
/// new memory ids (for embedding) parallel to `clusters` (`None` = not inserted).
pub fn apply_semantic_result(
    store: &Store,
    dek: &Key32,
    clusters: &[SemanticCluster],
    summaries: &[Option<String>],
    model: &str,
    now_secs: i64,
) -> Result<(SemanticApplyResult, Vec<Option<String>>)> {
    if clusters.len() != summaries.len() {
        return Err(VaultError::InvalidInput(format!(
            "clusters ({}) and summaries ({}) length mismatch",
            clusters.len(),
            summaries.len()
        )));
    }
    let mut result = SemanticApplyResult::default();
    let mut new_ids: Vec<Option<String>> = Vec::with_capacity(clusters.len());
    for (cluster, summary) in clusters.iter().zip(summaries.iter()) {
        let Some(s) = summary else {
            new_ids.push(None);
            continue;
        };
        match store.insert_semantic_memory(
            dek,
            &cluster.topic_key,
            &cluster.member_ids,
            s,
            model,
            cluster.window_start,
            cluster.window_end,
            now_secs,
        ) {
            Ok((id, 1)) => {
                result.inserted += 1;
                for old in &cluster.supersedes {
                    if let Ok(n) = store.mark_memory_superseded(old, &id) {
                        result.superseded += n;
                    }
                }
                new_ids.push(Some(id));
            }
            Ok((_id, _)) => {
                // Already existed (topic_key idempotent) — nothing to embed.
                new_ids.push(None);
            }
            Err(e) => {
                log::warn!("semantic memory insert skipped: {e}");
                new_ids.push(None);
            }
        }
    }
    Ok((result, new_ids))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::{EmbeddingProvider, MockEmbeddingProvider};
    use crate::llm::MockLlmProvider;
    use std::collections::HashMap;

    fn fixed_llm(response: &str) -> MockLlmProvider {
        let llm = MockLlmProvider::new("test-model");
        for _ in 0..16 {
            llm.push_response(response);
        }
        llm
    }

    /// Seed N episodic memories whose summaries all share a topic word, return ids.
    fn seed_topic(store: &Store, dek: &Key32, prefix: &str, topic_word: &str, n: usize) -> Vec<String> {
        let mut ids = Vec::new();
        for i in 0..n {
            let hash = format!("{prefix}-{i}");
            let summary = format!("用户研究了 {topic_word} 的第 {i} 个方面");
            store
                .insert_memory(
                    dek,
                    "episodic",
                    (i as i64) * 86400,
                    (i as i64 + 1) * 86400,
                    &[hash.clone()],
                    &summary,
                    "m",
                    (i as i64) * 86400,
                )
                .unwrap();
            let id = store
                .list_recent_memories(dek, 1000)
                .unwrap()
                .into_iter()
                .find(|m| m.source_chunk_hashes == vec![hash.clone()])
                .unwrap()
                .id;
            ids.push(id);
        }
        ids
    }

    fn embed_summaries(store: &Store, dek: &Key32, emb: &dyn EmbeddingProvider) -> HashMap<String, Vec<f32>> {
        let mut map = HashMap::new();
        for m in store.list_live_memories(dek, "episodic", false).unwrap() {
            let v = emb.embed(&[m.summary.as_str()]).unwrap().pop().unwrap();
            map.insert(m.id, v);
        }
        map
    }

    #[test]
    fn prepare_returns_none_below_min_members() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        seed_topic(&store, &dek, "t", "Rust", 2);
        let emb = MockEmbeddingProvider::new(64);
        let embs = embed_summaries(&store, &dek, &emb);
        assert!(prepare_semantic_cycle(&store, &dek, &embs).unwrap().is_none());
    }

    #[test]
    fn prepare_groups_same_topic() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        // 两个清晰分离的主题，各 6 条
        seed_topic(&store, &dek, "rust", "Rust 所有权 借用 生命周期", 6);
        seed_topic(&store, &dek, "cook", "川菜 烹饪 火候 调味", 6);
        let emb = MockEmbeddingProvider::new(256);
        let embs = embed_summaries(&store, &dek, &emb);
        let clusters = prepare_semantic_cycle(&store, &dek, &embs).unwrap();
        // hdbscan 至少识别出 1 个 ≥4 成员的主题簇
        assert!(clusters.is_some(), "expected at least one topic cluster");
        let clusters = clusters.unwrap();
        assert!(clusters.iter().all(|c| c.member_ids.len() >= MIN_MEMS_PER_TOPIC));
    }

    #[test]
    fn topic_key_idempotent_across_reruns() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        seed_topic(&store, &dek, "rust", "Rust 所有权 借用 生命周期 并发", 8);
        let emb = MockEmbeddingProvider::new(256);
        let embs = embed_summaries(&store, &dek, &emb);
        let clusters = prepare_semantic_cycle(&store, &dek, &embs).unwrap();
        let Some(clusters) = clusters else { return };
        let summaries: Vec<Option<String>> =
            clusters.iter().map(|_| Some("standing summary".into())).collect();
        let (r1, _) = apply_semantic_result(&store, &dek, &clusters, &summaries, "m", 1000).unwrap();
        assert!(r1.inserted >= 1);
        // 第二次跑：相同 membership → topic_key 命中，prepare 应排除
        let c2 = prepare_semantic_cycle(&store, &dek, &embs).unwrap();
        assert!(c2.is_none(), "already-built topic must be excluded on rerun");
    }

    #[test]
    fn generate_returns_none_on_empty_llm() {
        let cluster = SemanticCluster {
            topic_key: "k".into(),
            member_ids: vec!["a".into()],
            member_summaries: vec!["s".into()],
            window_start: 0,
            window_end: 1,
            supersedes: vec![],
        };
        let llm = fixed_llm("   ");
        assert!(generate_one_semantic_memory(&llm, &cluster).is_none());
    }

    #[test]
    fn apply_skips_none_summaries() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let clusters = vec![
            SemanticCluster {
                topic_key: "k1".into(),
                member_ids: vec!["m1".into()],
                member_summaries: vec!["s".into()],
                window_start: 0,
                window_end: 1,
                supersedes: vec![],
            },
            SemanticCluster {
                topic_key: "k2".into(),
                member_ids: vec!["m2".into()],
                member_summaries: vec!["s".into()],
                window_start: 0,
                window_end: 1,
                supersedes: vec![],
            },
        ];
        let summaries = vec![None, Some("topic 2 summary".into())];
        let (r, new_ids) =
            apply_semantic_result(&store, &dek, &clusters, &summaries, "m", 0).unwrap();
        assert_eq!(r.inserted, 1);
        assert!(new_ids[0].is_none() && new_ids[1].is_some());
    }

    #[test]
    fn apply_rejects_length_mismatch() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let err = apply_semantic_result(&store, &dek, &[], &[Some("x".into())], "m", 0).unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));
    }

    #[test]
    fn apply_marks_subset_topic_superseded() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        // 老 semantic：成员 {m1, m2}
        let (old_id, _) = store
            .insert_semantic_memory(&dek, "old-k", &["m1".into(), "m2".into()], "old", "m", 0, 100, 1000)
            .unwrap();
        // 新 cluster：成员 {m1, m2, m3} ⊃ 老成员 → 应 supersede 老行
        let cluster = SemanticCluster {
            topic_key: "new-k".into(),
            member_ids: vec!["m1".into(), "m2".into(), "m3".into()],
            member_summaries: vec!["a".into(), "b".into(), "c".into()],
            window_start: 0,
            window_end: 200,
            supersedes: vec![old_id.clone()],
        };
        let (r, _) = apply_semantic_result(
            &store, &dek, &[cluster], &[Some("refreshed".into())], "m", 2000,
        )
        .unwrap();
        assert_eq!(r.inserted, 1);
        assert_eq!(r.superseded, 1);
        let live = store.list_live_memories(&dek, "semantic", false).unwrap();
        assert_eq!(live.len(), 1, "old subset topic must drop out of live set");
    }
}
