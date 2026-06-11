//! schema migration 集成测试（真持久化文件 + 重开 roundtrip）。
//!
//! `migrate_breadcrumbs_encrypt` 的单元测试用
//! `Store::open_memory()` 跑 — in-memory 不能复现"上一次进程关闭、新进程重开"的真升级场景。
//! 本测试用 tempfile 持久化文件，跨多个 Store::open 调用验证：
//!
//! 1. 老 vault（breadcrumb_json TEXT 列）→ 新 schema (breadcrumb_enc BLOB) 升级真生效
//! 2. 升级后老明文数据丢失（acceptable per RELEASE.md，下次 indexer ingest 自动 backfill）
//! 3. 二次开启幂等 — migration 不重复跑
//! 4. 加密 breadcrumb 跨进程关闭/重开能正确解密

use attune_core::crypto::Key32;
use attune_core::store::Store;
use rusqlite::Connection;
use tempfile::TempDir;

/// 模拟 W3 batch A 之前的老 schema：breadcrumb_json TEXT 明文列。
/// items 表 schema 与当前主线一致（W3 升级只动了 chunk_breadcrumbs 表）。
fn create_old_schema_db(path: &std::path::Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
         CREATE TABLE items (
            id          TEXT PRIMARY KEY,
            title       TEXT NOT NULL,
            content     BLOB NOT NULL,
            url         TEXT,
            source_type TEXT NOT NULL DEFAULT 'note',
            domain      TEXT,
            tags        BLOB,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            is_deleted  INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE chunk_breadcrumbs (
            item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
            chunk_idx INTEGER NOT NULL,
            breadcrumb_json TEXT NOT NULL,
            offset_start INTEGER NOT NULL,
            offset_end INTEGER NOT NULL,
            PRIMARY KEY (item_id, chunk_idx)
         );",
    )
    .unwrap();
    // 写一条老明文 breadcrumb（验证升级会清掉）
    conn.execute(
        "INSERT INTO items (id, title, content, created_at, updated_at)
         VALUES ('item-1', 'Old Doc', x'00', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO chunk_breadcrumbs (item_id, chunk_idx, breadcrumb_json, offset_start, offset_end)
         VALUES ('item-1', 0, '[\"Section A\", \"Subsection\"]', 0, 100)",
        [],
    )
    .unwrap();
}

#[test]
fn migration_drops_old_plaintext_breadcrumb_column() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("vault.sqlite");

    // 1. 创建老 schema + 写一条老明文数据
    create_old_schema_db(&path);

    // 升级前确认老 schema 存在
    {
        let conn = Connection::open(&path).unwrap();
        let has_old: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('chunk_breadcrumbs') WHERE name = 'breadcrumb_json'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_old, 1, "升级前应有老明文列");
    }

    // 2. Store::open 触发 migrate_breadcrumbs_encrypt
    let store = Store::open(&path).unwrap();
    drop(store); // 关闭让 WAL flush

    // 3. 验证老列已 DROP，新列存在
    let conn = Connection::open(&path).unwrap();
    let has_old: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('chunk_breadcrumbs') WHERE name = 'breadcrumb_json'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_old, 0, "升级后老明文列必须消失");

    let has_new: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('chunk_breadcrumbs') WHERE name = 'breadcrumb_enc'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_new, 1, "升级后必须有加密列");

    // 4. 老明文数据丢失（acceptable per RELEASE.md）
    let row_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunk_breadcrumbs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(row_count, 0, "老明文数据必须清掉，等 indexer 重新 backfill");
}

#[test]
fn migration_is_idempotent_on_second_open() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("vault.sqlite");

    // 第一次 open: 全新 schema
    let store = Store::open(&path).unwrap();
    drop(store);

    // 第二次 open: migration 不应重复跑（无老列可 DROP）
    // 不应 panic / SQL error
    let store = Store::open(&path).unwrap();
    drop(store);

    // 第三次 open: 同上
    let _store = Store::open(&path).unwrap();
}

#[test]
fn encrypted_breadcrumb_survives_close_and_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("vault.sqlite");
    let dek = Key32::generate();
    let body = "# Section A\n## Subsection\nbody text";

    // 写入加密 breadcrumb
    let item_id;
    {
        let store = Store::open(&path).unwrap();
        item_id = store
            .insert_item(&dek, "Test Doc", body, None, "note", None, None)
            .unwrap();
        let n = store
            .upsert_chunk_breadcrumbs_from_content(&dek, &item_id, body)
            .unwrap();
        assert!(n > 0, "应至少 upsert 一条 breadcrumb");
        store.checkpoint().unwrap(); // flush WAL
    }

    // 验证文件层 breadcrumb_enc 是 BLOB（不是明文）
    {
        let conn = Connection::open(&path).unwrap();
        let blob: Vec<u8> = conn
            .query_row(
                "SELECT breadcrumb_enc FROM chunk_breadcrumbs WHERE item_id = ?1 LIMIT 1",
                [&item_id],
                |r| r.get(0),
            )
            .unwrap();
        let s = String::from_utf8_lossy(&blob);
        assert!(!s.contains("Section A"), "加密 BLOB 不应含明文 Section A");
        assert!(!s.contains("Subsection"));
    }

    // 重开 + 同 dek 解密成功
    {
        let store = Store::open(&path).unwrap();
        let result = store
            .get_first_chunk_breadcrumb(&dek, &item_id)
            .unwrap();
        let (path_segs, off_start, off_end) = result.expect("第一条 breadcrumb");
        assert!(!path_segs.is_empty(), "path 段非空");
        assert!(path_segs.iter().any(|s| s.contains("Section A")));
        assert!(off_end > off_start);
    }

    // 重开 + 错误 dek：要么 Err 要么 None — 不应返回明文
    {
        let store = Store::open(&path).unwrap();
        let wrong_dek = Key32::generate();
        let result = store.get_first_chunk_breadcrumb(&wrong_dek, &item_id);
        match result {
            Ok(None) => {} // OK
            Ok(Some(_)) => panic!("错 dek 不应能返回 breadcrumb 内容"),
            Err(_) => {}   // OK
        }
    }
}

#[test]
fn migration_runs_in_correct_order_with_task_type() {
    // 验证两个 migration（task_type + breadcrumbs_encrypt）能在同一次 open 内 happy path 跑通
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("vault.sqlite");
    let _store = Store::open(&path).unwrap();
    let _store2 = Store::open(&path).unwrap(); // 二次 open 也应顺利
}
