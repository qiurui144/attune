// A1 集成测试：完整 prepare → generate → apply 周期，使用真 Store + tempfile + MockLlm。
//
// 验证：
// 1. 跨 2 天的 chunk_summaries → 2 个 episodic memories
// 2. 重跑同样数据 → 0 新增（幂等）
// 3. memories 表内容可解密读出，summary 与 LLM 响应一致

use attune_core::crypto::Key32;
use attune_core::llm::MockLlmProvider;
use attune_core::memory_consolidation::{
    apply_consolidation_result, generate_episodic_memories, prepare_consolidation_cycle,
    run_consolidation_cycle,
};
use attune_core::store::Store;

fn fixed_llm(response: &str) -> MockLlmProvider {
    let llm = MockLlmProvider::new("test-model");
    for _ in 0..16 {
        llm.push_response(response);
    }
    llm
}

fn temp_store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.sqlite");
    let store = Store::open(&path).unwrap();
    // 返回 dir 让调用方持有 — drop 时自动清理 /tmp（reviewer N3：避免 mem::forget 泄漏）
    (store, dir)
}

#[test]
fn full_cycle_creates_one_memory_per_day_window() {
    let (store, _tmp) = temp_store();
    let dek = Key32::generate();

    // Day 1（2026-04-25）：6 chunks
    for i in 0..6 {
        store
            .__test_seed_chunk_summary(
                &dek,
                &format!("d1-{i}"),
                "item-1",
                &format!("Day 1 chunk {i} 内容"),
                "2026-04-25 10:00:00",
            )
            .unwrap();
    }
    // Day 2（2026-04-26）：6 chunks
    for i in 0..6 {
        store
            .__test_seed_chunk_summary(
                &dek,
                &format!("d2-{i}"),
                "item-2",
                &format!("Day 2 chunk {i} 内容"),
                "2026-04-26 14:00:00",
            )
            .unwrap();
    }

    // now = 2026-04-27 12:00 UTC （超过 D1 + D2 的窗口，安全 consolidate）
    let now = chrono::DateTime::parse_from_rfc3339("2026-04-27T12:00:00Z")
        .unwrap()
        .timestamp();

    let llm = fixed_llm("用户在这一天集中学习了某个主题。");
    let n = run_consolidation_cycle(&store, &dek, &llm, now, "test-model").unwrap();
    assert_eq!(n, 2, "should create exactly one memory per day window");
    assert_eq!(store.memory_count().unwrap(), 2);

    // 二次跑：相同 chunks → 0 新增
    let n2 = run_consolidation_cycle(&store, &dek, &llm, now, "test-model").unwrap();
    assert_eq!(n2, 0, "rerun on same data must be idempotent");
    assert_eq!(store.memory_count().unwrap(), 2);
}

#[test]
fn memories_persist_summary_text_round_trip() {
    let (store, _tmp) = temp_store();
    let dek = Key32::generate();
    for i in 0..6 {
        store
            .__test_seed_chunk_summary(
                &dek,
                &format!("h{i}"),
                "item",
                &format!("chunk {i}"),
                "2026-04-25 10:00:00",
            )
            .unwrap();
    }
    let now = chrono::DateTime::parse_from_rfc3339("2026-04-27T12:00:00Z")
        .unwrap()
        .timestamp();
    let llm = fixed_llm("UNIQUE-TEST-PHRASE-12345");
    run_consolidation_cycle(&store, &dek, &llm, now, "qwen2.5:3b").unwrap();

    let recent = store.list_recent_memories(&dek, 10).unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].summary, "UNIQUE-TEST-PHRASE-12345");
    assert_eq!(recent[0].kind, "episodic");
    assert_eq!(recent[0].model, "qwen2.5:3b");
    assert_eq!(recent[0].source_chunk_hashes.len(), 6);
}

#[test]
fn three_stage_api_separately_callable() {
    // 验证 prepare/generate/apply 可独立调用（生产 worker 路径）
    let (store, _tmp) = temp_store();
    let dek = Key32::generate();
    for i in 0..6 {
        store
            .__test_seed_chunk_summary(
                &dek,
                &format!("h{i}"),
                "item",
                &format!("c{i}"),
                "2026-04-25 10:00:00",
            )
            .unwrap();
    }
    let now = chrono::DateTime::parse_from_rfc3339("2026-04-27T12:00:00Z")
        .unwrap()
        .timestamp();

    let bundles = prepare_consolidation_cycle(&store, &dek, now)
        .unwrap()
        .expect("should return bundles");
    assert_eq!(bundles.len(), 1);

    let llm = fixed_llm("staged response");
    let summaries = generate_episodic_memories(&llm, &bundles);
    assert_eq!(summaries.len(), 1);
    assert!(summaries[0].is_some());

    let n = apply_consolidation_result(&store, &dek, &bundles, &summaries, "m", now).unwrap();
    assert_eq!(n, 1);
}

#[test]
fn empty_chunk_summaries_yields_no_memories() {
    let (store, _tmp) = temp_store();
    let dek = Key32::generate();
    let now = chrono::DateTime::parse_from_rfc3339("2026-04-27T12:00:00Z")
        .unwrap()
        .timestamp();
    let llm = fixed_llm("never called");
    let n = run_consolidation_cycle(&store, &dek, &llm, now, "m").unwrap();
    assert_eq!(n, 0);
    assert_eq!(store.memory_count().unwrap(), 0);
}
