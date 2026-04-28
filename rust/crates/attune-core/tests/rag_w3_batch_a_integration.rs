// W3 batch A 集成测试：F1 + F2 + C1 端到端
//
// 设计稿：docs/superpowers/specs/2026-04-27-w3-batch-a-design.md
// 上游参照：吴师兄 §6 高频缓存 + 自有 sidecar 表设计模式

use attune_core::crypto::Key32;
use attune_core::store::{Store, DEFAULT_WEB_SEARCH_TTL_SECS};
use attune_core::web_search::WebSearchResult;

fn temp_store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.sqlite");
    let store = Store::open(&path).unwrap();
    (store, dir)
}

// ── C1 集成 ────────────────────────────────────────────────────────────

#[test]
fn c1_cache_round_trip_through_real_sqlite_file() {
    let (store, _tmp) = temp_store();
    let dek = Key32::generate();
    let results = vec![WebSearchResult {
        title: "test".into(),
        url: "https://t.com".into(),
        snippet: "snippet".into(),
        published_date: None,
    }];
    store
        .put_web_search_cached(&dek, "rust async", &results, DEFAULT_WEB_SEARCH_TTL_SECS, 1000)
        .unwrap();
    let hit = store
        .get_web_search_cached(&dek, "rust async", 2000)
        .unwrap()
        .expect("应命中");
    assert_eq!(hit[0].url, "https://t.com");
}

#[test]
fn c1_default_ttl_is_30_days() {
    // 锁住默认 TTL = 30 天，避免有人改默认值导致缓存过期变化
    assert_eq!(DEFAULT_WEB_SEARCH_TTL_SECS, 30 * 24 * 3600);
}

// ── F2 集成 ────────────────────────────────────────────────────────────
// 注意：chunk_breadcrumbs.item_id 走 FK CASCADE 到 items 表（per reviewer I3）。
// 集成测试必须先 insert_item 拿到真实 item_id 再 upsert。

#[test]
fn f2_breadcrumb_pipeline_writes_then_chat_reads() {
    let (store, _tmp) = temp_store();
    let dek = Key32::generate();
    let content = "# 公司手册\n\n## 第三章 福利\n\n年假 15 天。";
    let item_id = store
        .insert_item(&dek, "公司手册", content, None, "file", None, None)
        .unwrap();
    let n = store
        .upsert_chunk_breadcrumbs_from_content(&dek, &item_id, content)
        .unwrap();
    assert!(n >= 2);

    // ChatEngine 路径模拟：search 拿到 item，查 first_chunk_breadcrumb
    let bc = store.get_first_chunk_breadcrumb(&dek, &item_id).unwrap().unwrap();
    assert!(!bc.0.is_empty(), "breadcrumb 应非空");
    assert_eq!(bc.1, 0, "第一个 chunk 的 offset_start = 0");
    assert!(bc.2 > 0, "offset_end > 0");
}

#[test]
fn f2_old_vault_without_sidecar_returns_none() {
    // 模拟老 vault 升级：表已建（schema 自动），但没数据
    let (store, _tmp) = temp_store();
    let dek = Key32::generate();
    let bc = store.get_chunk_breadcrumb(&dek, "never-indexed", 0).unwrap();
    assert!(bc.is_none(), "未 upsert 的 item 返回 None，让 Citation 优雅降级为空 Vec");
}

#[test]
fn f2_reindex_overwrites_old_breadcrumbs() {
    let (store, _tmp) = temp_store();
    let dek = Key32::generate();
    let v1 = "# 旧标题\n\n旧内容";
    let item_id = store.insert_item(&dek, "doc", v1, None, "file", None, None).unwrap();
    store.upsert_chunk_breadcrumbs_from_content(&dek, &item_id, v1).unwrap();
    let bc1 = store.get_first_chunk_breadcrumb(&dek, &item_id).unwrap().unwrap();
    assert_eq!(bc1.0[0], "旧标题");

    // 文件被改后重扫
    let v2 = "# 新标题\n\n新内容";
    store.upsert_chunk_breadcrumbs_from_content(&dek, &item_id, v2).unwrap();
    let bc2 = store.get_first_chunk_breadcrumb(&dek, &item_id).unwrap().unwrap();
    assert_eq!(bc2.0[0], "新标题", "重扫应覆盖 path");
}

#[test]
fn f2_search_result_has_breadcrumb_field() {
    // 编译时检查：SearchResult 公开字段已加（防止未来意外回退）
    use attune_core::search::SearchResult;
    let sr = SearchResult {
        item_id: "x".into(),
        score: 0.5,
        title: "T".into(),
        content: "C".into(),
        source_type: "file".into(),
        inject_content: None,
        breadcrumb: vec!["A".into(), "B".into()],
        chunk_offset_start: Some(0),
        chunk_offset_end: Some(100),
        corpus_domain: String::new(),  // F-Pro Stage 1 新增字段
    };
    assert_eq!(sr.breadcrumb.len(), 2);
    assert_eq!(sr.chunk_offset_start, Some(0));
}

// ── 跨 feature ──────────────────────────────────────────────────────────

#[test]
fn citation_ends_to_end_with_breadcrumb_from_indexer() {
    use attune_core::Citation;
    // 模拟 ChatEngine 流程 ：
    //   1. insert_item 拿到 item_id
    //   2. indexer 写入 chunk_breadcrumbs（FK 校验通过）
    //   3. search 拿到 SearchResult.breadcrumb
    //   4. ChatEngine 映射到 Citation.breadcrumb
    let (store, _tmp) = temp_store();
    let dek = Key32::generate();
    let content = "# 文档\n\n## 章节 A\n\n正文";
    let item_id = store.insert_item(&dek, "文档", content, None, "file", None, None).unwrap();
    store.upsert_chunk_breadcrumbs_from_content(&dek, &item_id, content).unwrap();

    let (path, start, end) = store.get_first_chunk_breadcrumb(&dek, &item_id).unwrap().unwrap();
    let citation = Citation {
        item_id: item_id.clone(),
        title: "文档".into(),
        relevance: 0.9,
        breadcrumb: path.clone(),
        chunk_offset_start: Some(start),
        chunk_offset_end: Some(end),
    };
    let json = serde_json::to_string(&citation).unwrap();
    assert!(json.contains("\"breadcrumb\":["));
    assert!(!citation.breadcrumb.is_empty());
    assert!(citation.chunk_offset_end.unwrap() > citation.chunk_offset_start.unwrap());
}
