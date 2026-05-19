// npu-vault/crates/vault-core/src/scanner.rs

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::crypto::Key32;
use crate::error::{Result, VaultError};
use crate::store::Store;

/// 扫描结果
#[derive(Debug, Clone)]
pub struct ScanResult {
    pub total_files: usize,
    pub new_files: usize,
    pub updated_files: usize,
    pub skipped_files: usize,
    pub errors: usize,
}

/// 全量扫描指定目录。
pub fn scan_directory(
    store: &Store,
    dek: &Key32,
    dir_id: &str,
    dir_path: &Path,
    recursive: bool,
    file_types: &[String],
) -> Result<ScanResult> {
    use crate::ingest::local::LocalFolderConnector;
    use crate::ingest::{ingest_document, ingest_document_replacing, IngestOutcome, SourceConnector};

    let mut result = ScanResult {
        total_files: 0,
        new_files: 0,
        updated_files: 0,
        skipped_files: 0,
        errors: 0,
    };

    // F-Pro：从 bound_dir 读 corpus_domain，透传给 connector → RawDocument →
    // ingest_document（item 级标签 + chunk `[领域: X]` 前缀注入）。
    let corpus_domain = store
        .get_dir_corpus_domain(dir_id)
        .ok()
        .filter(|d| !d.is_empty() && d != "general");
    let connector = LocalFolderConnector::new(
        dir_path.to_path_buf(),
        recursive,
        file_types.to_vec(),
        corpus_domain,
    );
    let mut docs = Vec::new();
    {
        let mut sink: crate::ingest::DocumentSink<'_> = Box::new(|doc| docs.push(doc));
        connector.fetch_documents(&mut sink)?;
    }

    for doc in docs {
        result.total_files += 1;
        let marker = doc.modified_marker.clone().unwrap_or_default();

        // SHA-256 增量判断：indexed_files.file_hash 即上次的内容 hash。
        // 与旧 process_single_file 逻辑等价（两者均读文件内容算 SHA-256，无 mtime 预过滤）。
        let prior = store.get_indexed_file(&doc.source_ref).ok().flatten();
        let old_item_id: Option<String> = match &prior {
            Some(row) if row.file_hash == marker && !marker.is_empty() => {
                result.skipped_files += 1;
                continue;
            }
            Some(row) => {
                // 文件已变 → 旧 item 软删 + enqueue purge + doc_update 信号。
                // scanner 拿不到 VectorIndex / FulltextIndex 锁，必须 defer 到 server worker。
                if let Some(old) = &row.item_id {
                    if let Err(e) = store.delete_item(old) {
                        log::warn!("scanner: delete_item({old}) failed: {e}");
                    }
                    if let Err(e) = store.enqueue_reindex(old, "purge") {
                        log::warn!("scanner: enqueue_reindex(purge) failed for {old}: {e} — orphan 向量风险");
                    }
                    if let Err(e) = store.record_signal_event("doc_update", old, None) {
                        log::debug!("scanner: record_signal_event failed for {old}: {e}");
                    }
                }
                row.item_id.clone()
            }
            None => None,
        };

        let outcome = match &old_item_id {
            Some(old) => ingest_document_replacing(store, dek, &doc, old),
            None => ingest_document(store, dek, &doc),
        };
        match outcome {
            Ok(IngestOutcome::Inserted { item_id, .. }) => {
                let _ = store.upsert_indexed_file(dir_id, &doc.source_ref, &marker, &item_id);
                result.new_files += 1;
            }
            Ok(IngestOutcome::Updated { item_id, .. }) => {
                let _ = store.upsert_indexed_file(dir_id, &doc.source_ref, &marker, &item_id);
                result.updated_files += 1;
            }
            Ok(IngestOutcome::Duplicate { item_id }) => {
                let _ = store.upsert_indexed_file(dir_id, &doc.source_ref, &marker, &item_id);
                result.skipped_files += 1;
            }
            Ok(IngestOutcome::Skipped { .. }) => {
                result.skipped_files += 1;
            }
            Err(e) => {
                log::warn!("scanner: ingest {} failed: {e}", doc.source_ref);
                result.errors += 1;
            }
        }
    }

    store.update_dir_last_scan(dir_id)?;
    Ok(result)
}

/// 创建文件监听器（返回 watcher 和事件接收器）
pub fn create_watcher() -> Result<(RecommendedWatcher, mpsc::Receiver<notify::Result<notify::Event>>)> {
    let (tx, rx) = mpsc::channel();
    let watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        notify::Config::default().with_poll_interval(Duration::from_secs(2)),
    )
    .map_err(|e| VaultError::Io(std::io::Error::other(e.to_string())))?;
    Ok((watcher, rx))
}

/// 添加监听路径
pub fn watch_directory(watcher: &mut RecommendedWatcher, path: &Path, recursive: bool) -> Result<()> {
    let mode = if recursive {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    watcher
        .watch(path, mode)
        .map_err(|e| VaultError::Io(std::io::Error::other(e.to_string())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn setup_test() -> (Store, Key32, TempDir) {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let tmp = TempDir::new().unwrap();
        (store, dek, tmp)
    }

    #[test]
    fn scan_empty_directory() {
        let (store, dek, tmp) = setup_test();
        let dir_id = store
            .bind_directory(tmp.path().to_str().unwrap(), true, &["md", "txt"])
            .unwrap();
        let result =
            scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["md".into(), "txt".into()])
                .unwrap();
        assert_eq!(result.total_files, 0);
    }

    #[test]
    fn scan_with_files() {
        let (store, dek, tmp) = setup_test();

        // Create test files
        let mut f1 = std::fs::File::create(tmp.path().join("doc1.md")).unwrap();
        f1.write_all(b"# Title 1\n\nContent of document 1.").unwrap();

        let mut f2 = std::fs::File::create(tmp.path().join("doc2.txt")).unwrap();
        f2.write_all(b"Plain text document content here.").unwrap();

        // Create unsupported file (should be skipped)
        std::fs::File::create(tmp.path().join("image.png")).unwrap();

        let dir_id = store
            .bind_directory(tmp.path().to_str().unwrap(), true, &["md", "txt"])
            .unwrap();
        let result =
            scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["md".into(), "txt".into()])
                .unwrap();

        assert_eq!(result.total_files, 2, "Should find 2 supported files");
        assert_eq!(result.new_files + result.updated_files, 2);
        assert_eq!(store.item_count().unwrap(), 2);
    }

    #[test]
    fn scan_skips_unchanged_files() {
        let (store, dek, tmp) = setup_test();

        let mut f = std::fs::File::create(tmp.path().join("doc.md")).unwrap();
        f.write_all(b"# Test\n\nContent.").unwrap();

        let dir_id = store
            .bind_directory(tmp.path().to_str().unwrap(), true, &["md"])
            .unwrap();

        // First scan
        let r1 = scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["md".into()]).unwrap();
        assert_eq!(r1.new_files, 1);

        // Second scan (no changes)
        let r2 = scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["md".into()]).unwrap();
        assert_eq!(r2.skipped_files, 1, "Unchanged file should be skipped");
        assert_eq!(r2.new_files, 0);
    }

    #[test]
    fn scan_detects_modified_files() {
        let (store, dek, tmp) = setup_test();

        let path = tmp.path().join("doc.md");
        std::fs::write(&path, b"# Original\n\nOld content.").unwrap();

        let dir_id = store
            .bind_directory(tmp.path().to_str().unwrap(), true, &["md"])
            .unwrap();
        scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["md".into()]).unwrap();

        // Modify file
        std::fs::write(&path, b"# Updated\n\nNew content.").unwrap();

        let r2 = scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["md".into()]).unwrap();
        // Should process the modified file (either new or updated)
        assert_eq!(r2.skipped_files, 0, "Modified file should not be skipped");
    }

    #[test]
    fn create_watcher_works() {
        let (mut watcher, _rx) = create_watcher().unwrap();
        let tmp = TempDir::new().unwrap();
        watch_directory(&mut watcher, tmp.path(), true).unwrap();
        // Just verify it doesn't crash
    }

    #[test]
    fn scan_enqueues_level2_and_classify() {
        // 回归保护：本地扫描入库必须同时有 L1 + L2 embedding 与 classify 任务
        // （WebDAV 旧实现漏抄的两步，统一 pipeline 后任何源都不应再漏）。
        let (store, dek, tmp) = setup_test();
        std::fs::write(
            tmp.path().join("doc.md"),
            b"# Heading One\n\nFirst body paragraph.\n\n# Heading Two\n\nSecond body.",
        )
        .unwrap();
        let dir_id = store
            .bind_directory(tmp.path().to_str().unwrap(), true, &["md"])
            .unwrap();
        scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["md".into()]).unwrap();

        assert!(store.count_embed_queue_by_level(1).unwrap() >= 1, "L1 必须入队");
        assert!(store.count_embed_queue_by_level(2).unwrap() >= 1, "L2 必须入队");
        assert_eq!(store.pending_count_by_type("classify").unwrap(), 1, "classify 必须入队");
    }
}
