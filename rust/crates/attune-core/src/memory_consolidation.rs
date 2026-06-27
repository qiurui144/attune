//! A1 Memory Consolidation MVP — 周期把 chunk_summaries 按时间窗口聚合成 episodic memory。
//!
//! 设计稿：`docs/superpowers/specs/2026-04-27-memory-consolidation-design.md`。
//!
//! 三阶段（与 [`crate::skill_evolution`] 同构）：
//!   1. [`prepare_consolidation_cycle`] **持 vault 锁**：扫 chunk_summaries → 按天分桶 →
//!      解密摘要 → 返回 bundles。同时过滤已有 memory 的 (kind, hashes) 组合（幂等）。
//!   2. [`generate_episodic_memories`] **无锁**：每 bundle 一次 LLM 调用。LLM 失败的 bundle
//!      返回 None，不影响其他 bundle。
//!   3. [`apply_consolidation_result`] **持 vault 锁**：`INSERT OR IGNORE` 写 memories；
//!      返回新增条数。

use std::collections::{BTreeMap, BTreeSet};

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::llm::{ChatMessage, LlmProvider};
use crate::memory::MemoryScope;
use crate::store::Store;

/// 默认时间窗口：1 天。
pub const DEFAULT_WINDOW_SECS: i64 = 24 * 3600;
/// 单 bundle 最少 chunk 数；过少视为信号太薄，跳过。
pub const MIN_CHUNKS_PER_BUNDLE: usize = 5;
/// 单 bundle 最多 chunk 数（防 LLM prompt 爆 token）。
pub const MAX_CHUNKS_PER_BUNDLE: usize = 50;
/// 单周期最多生成 bundle 数（防 LLM 调用风暴）。
pub const MAX_BUNDLES_PER_CYCLE: usize = 4;
/// prepare 阶段从 chunk_summaries 取多少条 ── 上限保护（5 × MAX_BUNDLES × MAX_CHUNKS）。
const PREPARE_FETCH_LIMIT: usize = 1000;
/// 只看最近 30 天的 chunk_summaries — 历史更老的留给 W5+ 的 semantic memory。
const LOOKBACK_SECS: i64 = 30 * 24 * 3600;

/// 单个时间窗口内待合并的 chunk 集合。
#[derive(Debug, Clone)]
pub struct ConsolidationBundle {
    pub window_start: i64,
    pub window_end: i64,
    /// 升序排列（按 chunk_hash 字典序），保证幂等键稳定。
    pub chunks: Vec<BundleChunk>,
}

#[derive(Debug, Clone)]
pub struct BundleChunk {
    pub chunk_hash: String,
    pub item_id: String,
    pub summary: String,
}

impl ConsolidationBundle {
    /// 按字典序的 chunk_hash 列表 — 用作 memories 表的幂等键。
    pub fn sorted_hashes(&self) -> Vec<String> {
        self.chunks.iter().map(|c| c.chunk_hash.clone()).collect()
    }

}

// ── Phase 1：prepare（持 vault 锁） ────────────────────────────────────────────

/// 扫 chunk_summaries → 按 day-window 分桶 → 解密摘要 → 过滤已 consolidated → 返回 bundles。
///
/// 返回 `Ok(None)` 表示无 eligible bundle（idle cycle，调用方应跳过本周期）。
pub fn prepare_consolidation_cycle(
    store: &Store,
    dek: &Key32,
    now_secs: i64,
) -> Result<Option<Vec<ConsolidationBundle>>> {
    let since = now_secs - LOOKBACK_SECS;
    let heads = store.list_chunk_summaries_for_consolidation(since, PREPARE_FETCH_LIMIT)?;
    if heads.is_empty() {
        return Ok(None);
    }

    // 按 window_start 分桶（每天一个 bucket）
    let mut buckets: BTreeMap<i64, Vec<(String, String, Vec<u8>)>> = BTreeMap::new();
    for h in heads {
        // 当前窗口起点 = floor(created_at / DEFAULT_WINDOW_SECS) * DEFAULT_WINDOW_SECS
        let window_start = (h.created_at_secs / DEFAULT_WINDOW_SECS) * DEFAULT_WINDOW_SECS;
        buckets
            .entry(window_start)
            .or_default()
            .push((h.chunk_hash, h.item_id, h.summary_encrypted));
    }

    // 排除当前正在进行的窗口（now 所在的当天） — 避免半天数据被过早 consolidate。
    let current_window = (now_secs / DEFAULT_WINDOW_SECS) * DEFAULT_WINDOW_SECS;
    buckets.remove(&current_window);

    let mut bundles: Vec<ConsolidationBundle> = Vec::new();
    for (window_start, mut entries) in buckets {
        if entries.len() < MIN_CHUNKS_PER_BUNDLE {
            continue;
        }
        // 排序（按 chunk_hash 升序，作为幂等键）
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        // 截断到 MAX_CHUNKS_PER_BUNDLE
        entries.truncate(MAX_CHUNKS_PER_BUNDLE);

        // 幂等检查：相同 (kind=episodic, hashes_json) 已存在则跳过
        let hashes: Vec<String> = entries.iter().map(|e| e.0.clone()).collect();
        let hashes_json = serde_json::to_string(&hashes)
            .map_err(|e| VaultError::InvalidInput(format!("hashes serialize: {e}")))?;
        if store.memory_exists("episodic", &hashes_json)? {
            continue;
        }

        // 解密摘要（在 vault lock 内）
        let mut chunks = Vec::with_capacity(entries.len());
        let mut decrypt_failures = 0usize;
        for (chunk_hash, item_id, summary_enc) in entries {
            let summary = match crypto::decrypt(dek, &summary_enc) {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(_) => {
                    decrypt_failures += 1;
                    continue; // 损坏的摘要跳过（不阻塞整 bundle）
                }
            };
            chunks.push(BundleChunk {
                chunk_hash,
                item_id,
                summary,
            });
        }
        if chunks.len() < MIN_CHUNKS_PER_BUNDLE {
            // MVP 已知风险：若解密反复失败，本 bundle 永远无法消化（每周期重试）。
            // W5+ 解决方案：写一条 placeholder memory 占住 hashes 集合阻止重抓。
            log::warn!(
                "memory_consolidation: window {window_start} skipped — only {} of {} chunks decryptable ({} failures)",
                chunks.len(),
                chunks.len() + decrypt_failures,
                decrypt_failures,
            );
            continue;
        }

        bundles.push(ConsolidationBundle {
            window_start,
            window_end: window_start + DEFAULT_WINDOW_SECS,
            chunks,
        });

        if bundles.len() >= MAX_BUNDLES_PER_CYCLE {
            break;
        }
    }

    if bundles.is_empty() {
        Ok(None)
    } else {
        Ok(Some(bundles))
    }
}

// ── Phase 2：generate（无锁） ────────────────────────────────────────────────

/// 单 bundle 的 LLM 调用 — worker 路径用此函数，每次调用前自行 check H1 governor 配额。
/// 返回 None 表示 LLM 失败 / 空响应，调用方应跳过该 bundle 的 apply。
pub fn generate_one_episodic_memory(
    llm: &dyn LlmProvider,
    bundle: &ConsolidationBundle,
) -> Option<String> {
    match llm.chat_with_history(&[ChatMessage::user(&build_prompt(bundle))]) {
        Ok((s, _usage)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

/// 批量便利包装：每 bundle 一次 LLM 调用。**仅供测试 / `run_consolidation_cycle`**；
/// 生产 worker 必须用 [`generate_one_episodic_memory`] 在循环中按 bundle check 配额，
/// 否则会绕过 H1 LLM rate limit（每次调用都该消耗 1 配额，而非每周期 1 配额）。
pub fn generate_episodic_memories(
    llm: &dyn LlmProvider,
    bundles: &[ConsolidationBundle],
) -> Vec<Option<String>> {
    bundles
        .iter()
        .map(|b| generate_one_episodic_memory(llm, b))
        .collect()
}

fn build_prompt(bundle: &ConsolidationBundle) -> String {
    let summaries = bundle
        .chunks
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{}. {}", i + 1, c.summary))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"以下是用户在过去一天接触/学习的 {n} 个知识片段摘要：

{summaries}

请用 1 段（约 200 字）总结这段时间的学习焦点：覆盖的主题、知识脉络、可能形成的认知。
要求：
- 中文
- 第三人称口吻（如"用户关注了..."）
- 只输出一段总结，不要标题、不要列表、不要解释

总结："#,
        n = bundle.chunks.len(),
        summaries = summaries,
    )
}

// ── Phase 3：apply（持 vault 锁） ─────────────────────────────────────────────

/// 把 (bundles, summaries) 配对写入 memories 表。INSERT OR IGNORE 保证幂等。
/// 返回新增条数（已存在的 / summary 为 None 的 / 失败的均不计）。
pub fn apply_consolidation_result(
    store: &Store,
    dek: &Key32,
    bundles: &[ConsolidationBundle],
    summaries: &[Option<String>],
    model: &str,
    now_secs: i64,
) -> Result<usize> {
    // Legacy signature = all memories global; delegate to the scoped version with
    // scope-derivation disabled so behavior is byte-identical (chat-centric IA).
    apply_consolidation_result_inner(store, dek, bundles, summaries, model, now_secs, false)
}

/// scope-aware apply (chat-centric IA, 2026-06-26): derive each bundle's memory
/// scope from its source items' project membership.
///
/// Scope derivation per bundle (decision: never misattribute across projects):
///   - every chunk's item unowned → `Global`
///   - the union of project ids over all chunks is exactly one project P → `Project(P)`
///   - the union spans ≥ 2 projects (or mixes owned + a different project) → `Global`
///
/// Episodic memories come from `chunk_summaries` (document-derived), not from loose
/// conversations — so this path only ever assigns `Project(P)` or `Global`, never
/// `Conversation(_)` (that recall is lazily derived, Task C3).
pub fn apply_consolidation_result_scoped(
    store: &Store,
    dek: &Key32,
    bundles: &[ConsolidationBundle],
    summaries: &[Option<String>],
    model: &str,
    now_secs: i64,
) -> Result<usize> {
    apply_consolidation_result_inner(store, dek, bundles, summaries, model, now_secs, true)
}

/// Resolve a bundle's memory scope from its chunks' item→project_file membership.
/// `Global` unless every owned chunk resolves to one and the same single project.
fn derive_bundle_scope(store: &Store, bundle: &ConsolidationBundle) -> Result<MemoryScope> {
    let mut projects: BTreeSet<String> = BTreeSet::new();
    for chunk in &bundle.chunks {
        for pid in store.project_ids_for_item(&chunk.item_id)? {
            projects.insert(pid);
        }
    }
    let mut iter = projects.into_iter();
    match (iter.next(), iter.next()) {
        // exactly one project across all chunks → that project's scope
        (Some(only), None) => Ok(MemoryScope::Project(only)),
        // none owned, or spans ≥ 2 projects → global (do not misattribute)
        _ => Ok(MemoryScope::Global),
    }
}

fn apply_consolidation_result_inner(
    store: &Store,
    dek: &Key32,
    bundles: &[ConsolidationBundle],
    summaries: &[Option<String>],
    model: &str,
    now_secs: i64,
    scoped: bool,
) -> Result<usize> {
    if bundles.len() != summaries.len() {
        return Err(VaultError::InvalidInput(format!(
            "bundles ({}) and summaries ({}) length mismatch",
            bundles.len(),
            summaries.len()
        )));
    }
    let mut inserted = 0usize;
    for (bundle, summary) in bundles.iter().zip(summaries.iter()) {
        let Some(s) = summary else { continue };
        let hashes = bundle.sorted_hashes();
        let scope = if scoped {
            derive_bundle_scope(store, bundle)?
        } else {
            MemoryScope::Global
        };
        match store.insert_memory_scoped(
            dek,
            "episodic",
            bundle.window_start,
            bundle.window_end,
            &hashes,
            s,
            model,
            now_secs,
            &scope,
        ) {
            Ok(n) => inserted += n,
            Err(e) => {
                log::warn!("memory insert skipped: {e}");
            }
        }
    }
    Ok(inserted)
}

// ── 测试辅助：单调用 API ─────────────────────────────────────────────────────

/// 测试便利：单周期完整跑（仅供测试用，生产路径必须用三阶段以释放锁）。
pub fn run_consolidation_cycle(
    store: &Store,
    dek: &Key32,
    llm: &dyn LlmProvider,
    now_secs: i64,
    model: &str,
) -> Result<usize> {
    let Some(bundles) = prepare_consolidation_cycle(store, dek, now_secs)? else {
        return Ok(0);
    };
    let summaries = generate_episodic_memories(llm, &bundles);
    apply_consolidation_result(store, dek, &bundles, &summaries, model, now_secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Key32;
    use crate::llm::MockLlmProvider;

    /// MockLlmProvider 支持 push_response 队列。这里包装成"每次返回同一字符串"。
    fn fixed_llm(response: &str) -> MockLlmProvider {
        let llm = MockLlmProvider::new("test-model");
        // 给足够多的 response 应付测试可能的多次调用
        for _ in 0..16 {
            llm.push_response(response);
        }
        llm
    }

    fn seed_chunk_summary(
        store: &Store,
        dek: &Key32,
        chunk_hash: &str,
        item_id: &str,
        summary: &str,
        created_at_iso: &str,
    ) {
        store
            .__test_seed_chunk_summary(dek, chunk_hash, item_id, summary, created_at_iso)
            .unwrap();
    }

    #[test]
    fn prepare_returns_none_when_no_summaries() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let r = prepare_consolidation_cycle(&store, &dek, 100_000).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn prepare_skips_windows_below_min_chunks() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        // 仅 3 chunks 同一天 → 不够 MIN_CHUNKS_PER_BUNDLE (5)
        for i in 0..3 {
            seed_chunk_summary(
                &store,
                &dek,
                &format!("h{i}"),
                "item",
                &format!("summary {i}"),
                "2026-04-26 12:00:00",
            );
        }
        let now = chrono::DateTime::parse_from_rfc3339("2026-04-27T12:00:00Z")
            .unwrap()
            .timestamp();
        let r = prepare_consolidation_cycle(&store, &dek, now).unwrap();
        assert!(r.is_none(), "should skip window with < 5 chunks");
    }

    #[test]
    fn prepare_buckets_chunks_by_day() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        // Day 1: 6 chunks
        for i in 0..6 {
            seed_chunk_summary(
                &store, &dek, &format!("d1-h{i}"), "item", &format!("summary d1 {i}"),
                "2026-04-25 10:00:00",
            );
        }
        // Day 2: 7 chunks
        for i in 0..7 {
            seed_chunk_summary(
                &store, &dek, &format!("d2-h{i}"), "item", &format!("summary d2 {i}"),
                "2026-04-26 14:00:00",
            );
        }
        let now = chrono::DateTime::parse_from_rfc3339("2026-04-27T12:00:00Z")
            .unwrap()
            .timestamp();
        let bundles = prepare_consolidation_cycle(&store, &dek, now).unwrap().unwrap();
        assert_eq!(bundles.len(), 2, "two distinct day-windows");
        assert!(bundles.iter().all(|b| b.chunks.len() >= 5));
    }

    #[test]
    fn prepare_excludes_current_window() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let now_iso = "2026-04-27 14:00:00";
        // 当天 6 chunks
        for i in 0..6 {
            seed_chunk_summary(&store, &dek, &format!("h{i}"), "item", &format!("s {i}"), now_iso);
        }
        let now = chrono::DateTime::parse_from_rfc3339("2026-04-27T15:00:00Z")
            .unwrap()
            .timestamp();
        let r = prepare_consolidation_cycle(&store, &dek, now).unwrap();
        assert!(r.is_none(), "current day window must be excluded");
    }

    #[test]
    fn prepare_skips_already_consolidated_bundle() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        for i in 0..6 {
            seed_chunk_summary(
                &store, &dek, &format!("h{i}"), "item", &format!("s {i}"),
                "2026-04-25 10:00:00",
            );
        }
        let now = chrono::DateTime::parse_from_rfc3339("2026-04-27T12:00:00Z")
            .unwrap()
            .timestamp();
        // 第一次跑出 1 个 bundle
        let bundles = prepare_consolidation_cycle(&store, &dek, now).unwrap().unwrap();
        assert_eq!(bundles.len(), 1);
        // 直接 apply 写入 memories
        let summaries = vec![Some("manual summary".to_string())];
        apply_consolidation_result(&store, &dek, &bundles, &summaries, "model", now).unwrap();
        // 第二次跑：相同 bundle 应被排除
        let r2 = prepare_consolidation_cycle(&store, &dek, now).unwrap();
        assert!(r2.is_none(), "already-consolidated bundle should be excluded");
    }

    #[test]
    fn generate_returns_one_summary_per_bundle() {
        let bundles = vec![ConsolidationBundle {
            window_start: 0,
            window_end: 86400,
            chunks: vec![BundleChunk {
                chunk_hash: "h1".into(),
                item_id: "i".into(),
                summary: "user studied Rust ownership".into(),
            }],
        }];
        let llm = fixed_llm("总结：用户研究了 Rust 所有权。");
        let result = generate_episodic_memories(&llm, &bundles);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_deref(), Some("总结：用户研究了 Rust 所有权。"));
    }

    #[test]
    fn generate_returns_none_for_empty_llm_response() {
        let bundles = vec![ConsolidationBundle {
            window_start: 0,
            window_end: 86400,
            chunks: vec![BundleChunk {
                chunk_hash: "h".into(),
                item_id: "i".into(),
                summary: "x".into(),
            }],
        }];
        let llm = fixed_llm("   "); // 空白响应
        let r = generate_episodic_memories(&llm, &bundles);
        assert_eq!(r[0], None);
    }

    #[test]
    fn apply_skips_none_summaries() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let bundles = vec![
            ConsolidationBundle {
                window_start: 1000,
                window_end: 2000,
                chunks: vec![BundleChunk {
                    chunk_hash: "a".into(),
                    item_id: "i".into(),
                    summary: "s".into(),
                }],
            },
            ConsolidationBundle {
                window_start: 2000,
                window_end: 3000,
                chunks: vec![BundleChunk {
                    chunk_hash: "b".into(),
                    item_id: "i".into(),
                    summary: "s".into(),
                }],
            },
        ];
        let summaries = vec![None, Some("summary 2".into())];
        let n = apply_consolidation_result(&store, &dek, &bundles, &summaries, "m", 0).unwrap();
        assert_eq!(n, 1, "only one bundle had a Some summary");
    }

    #[test]
    fn apply_is_idempotent_on_rerun() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let bundles = vec![ConsolidationBundle {
            window_start: 1000,
            window_end: 2000,
            chunks: vec![BundleChunk {
                chunk_hash: "a".into(),
                item_id: "i".into(),
                summary: "s".into(),
            }],
        }];
        let summaries = vec![Some("first run summary".into())];
        let n1 = apply_consolidation_result(&store, &dek, &bundles, &summaries, "m", 0).unwrap();
        assert_eq!(n1, 1);
        // 二次 apply 同样的 bundle → INSERT OR IGNORE 返回 0
        let n2 = apply_consolidation_result(&store, &dek, &bundles, &summaries, "m", 0).unwrap();
        assert_eq!(n2, 0);
    }

    fn one_chunk_bundle(hash: &str, item_id: &str) -> ConsolidationBundle {
        ConsolidationBundle {
            window_start: 1000,
            window_end: 2000,
            chunks: vec![BundleChunk {
                chunk_hash: hash.into(),
                item_id: item_id.into(),
                summary: "s".into(),
            }],
        }
    }

    fn scope_of_only_memory(store: &Store, dek: &Key32) -> (String, Option<String>) {
        let rows = store.list_recent_memories(dek, 100).unwrap();
        assert_eq!(rows.len(), 1, "expected exactly one consolidated memory");
        (rows[0].scope_kind.clone(), rows[0].scope_id.clone())
    }

    #[test]
    fn consolidation_scopes_to_project_when_chunks_belong_to_project() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let p = store.create_project("Case", "generic").unwrap();
        store.add_file_to_project(&p.id, "item-1", "evidence").unwrap();

        let bundles = vec![one_chunk_bundle("h1", "item-1")];
        let summaries = vec![Some("project summary".to_string())];
        let n = apply_consolidation_result_scoped(&store, &dek, &bundles, &summaries, "m", 0).unwrap();
        assert_eq!(n, 1);
        let (kind, id) = scope_of_only_memory(&store, &dek);
        assert_eq!(kind, "project");
        assert_eq!(id, Some(p.id), "memory must carry the owning project's scope");
    }

    #[test]
    fn consolidation_global_when_chunks_unowned() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let bundles = vec![one_chunk_bundle("h1", "item-unowned")];
        let summaries = vec![Some("global summary".to_string())];
        apply_consolidation_result_scoped(&store, &dek, &bundles, &summaries, "m", 0).unwrap();
        let (kind, id) = scope_of_only_memory(&store, &dek);
        assert_eq!(kind, "global", "unowned item → global scope");
        assert_eq!(id, None);
    }

    #[test]
    fn consolidation_global_when_chunks_span_multiple_projects() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let pa = store.create_project("A", "generic").unwrap();
        let pb = store.create_project("B", "generic").unwrap();
        store.add_file_to_project(&pa.id, "item-a", "evidence").unwrap();
        store.add_file_to_project(&pb.id, "item-b", "evidence").unwrap();
        // one bundle whose two chunks belong to two different projects
        let bundle = ConsolidationBundle {
            window_start: 1000,
            window_end: 2000,
            chunks: vec![
                BundleChunk { chunk_hash: "ha".into(), item_id: "item-a".into(), summary: "s".into() },
                BundleChunk { chunk_hash: "hb".into(), item_id: "item-b".into(), summary: "s".into() },
            ],
        };
        let summaries = vec![Some("spanning summary".to_string())];
        apply_consolidation_result_scoped(&store, &dek, &[bundle], &summaries, "m", 0).unwrap();
        let (kind, id) = scope_of_only_memory(&store, &dek);
        assert_eq!(kind, "global", "cross-project bundle must NOT misattribute → global");
        assert_eq!(id, None);
    }

    #[test]
    fn legacy_apply_consolidation_writes_global_scope() {
        // Backward-compat: the non-scoped apply must keep writing global rows.
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let p = store.create_project("Case", "generic").unwrap();
        store.add_file_to_project(&p.id, "item-1", "evidence").unwrap();
        let bundles = vec![one_chunk_bundle("h1", "item-1")];
        let summaries = vec![Some("s".to_string())];
        apply_consolidation_result(&store, &dek, &bundles, &summaries, "m", 0).unwrap();
        let (kind, id) = scope_of_only_memory(&store, &dek);
        assert_eq!(kind, "global", "legacy apply ignores project membership");
        assert_eq!(id, None);
    }

    #[test]
    fn apply_rejects_length_mismatch() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let err = apply_consolidation_result(
            &store,
            &dek,
            &[ConsolidationBundle { window_start: 0, window_end: 1, chunks: vec![] }],
            &[],
            "m",
            0,
        )
        .unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));
    }
}
