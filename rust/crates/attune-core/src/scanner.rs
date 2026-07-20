// npu-vault/crates/vault-core/src/scanner.rs

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::crypto::Key32;
use crate::error::{Result, VaultError};
use crate::ingest::{IngestOptions, RawDocument};
use crate::store::Store;

/// 扫描结果
#[derive(Debug, Clone)]
pub struct ScanResult {
    pub total_files: usize,
    pub new_files: usize,
    pub updated_files: usize,
    pub skipped_files: usize,
    pub degraded_files: usize,
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
    scan_directory_with_options(
        store,
        dek,
        dir_id,
        dir_path,
        recursive,
        file_types,
        &crate::ingest::IngestOptions::default(),
    )
}

/// 全量扫描指定目录，智能解析经调用方提供的 scheduler/options 承接。
pub fn scan_directory_with_options(
    store: &Store,
    dek: &Key32,
    dir_id: &str,
    dir_path: &Path,
    recursive: bool,
    file_types: &[String],
    ingest_options: &IngestOptions,
) -> Result<ScanResult> {
    use crate::ingest::local::LocalFolderConnector;
    use crate::ingest::SourceConnector;

    let mut result = ScanResult {
        total_files: 0,
        new_files: 0,
        updated_files: 0,
        skipped_files: 0,
        degraded_files: 0,
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
    {
        let mut sink: crate::ingest::DocumentSink<'_> =
            Box::new(|doc| scan_one_document(store, dek, dir_id, ingest_options, doc, &mut result));
        connector.fetch_documents(&mut sink)?;
    }

    store.update_dir_last_scan(dir_id)?;
    Ok(result)
}

fn scan_one_document(
    store: &Store,
    dek: &Key32,
    dir_id: &str,
    ingest_options: &IngestOptions,
    doc: RawDocument,
    result: &mut ScanResult,
) {
    use crate::ingest::{
        ingest_document_replacing_with_options, ingest_document_with_options, IngestOutcome,
    };

    result.total_files += 1;
    let marker = doc.modified_marker.clone().unwrap_or_default();

    // SHA-256 增量判断：indexed_files.file_hash 即上次的内容 hash。
    // 与旧 process_single_file 逻辑等价（两者均读文件内容算 SHA-256，无 mtime 预过滤）。
    let prior = store.get_indexed_file(&doc.source_ref).ok().flatten();
    let prior_active_item_id = prior
        .as_ref()
        .and_then(|row| row.item_id.as_ref())
        .and_then(|item_id| match store.item_exists(item_id) {
            Ok(true) => Some(item_id.clone()),
            Ok(false) => None,
            Err(e) => {
                log::warn!("scanner: item_exists({item_id}) failed: {e}");
                None
            }
        });
    let had_prior = prior_active_item_id.is_some();
    let old_item_id: Option<String> = match &prior {
        Some(row)
            if row.file_hash == marker
                && !marker.is_empty()
                && prior_active_item_id.is_some() =>
        {
            result.skipped_files += 1;
            return;
        }
        Some(row) if row.file_hash == marker && !marker.is_empty() => {
            log::warn!(
                "scanner: indexed file {} points to deleted/missing item; re-ingesting unchanged source",
                doc.source_ref
            );
            None
        }
        Some(_) => {
            // 文件已变 → 旧 item 软删 + enqueue purge + doc_update 信号。
            // scanner 拿不到 VectorIndex / FulltextIndex 锁，必须 defer 到 server worker。
            if let Some(old) = prior_active_item_id.as_ref() {
                if let Err(e) = store.delete_item(old) {
                    log::warn!("scanner: delete_item({old}) failed: {e}");
                }
                if let Err(e) = store.enqueue_reindex(old, "purge") {
                    log::warn!(
                        "scanner: enqueue_reindex(purge) failed for {old}: {e} — orphan 向量风险"
                    );
                }
                if let Err(e) = store.record_signal_event("doc_update", old, None) {
                    log::debug!("scanner: record_signal_event failed for {old}: {e}");
                }
            }
            prior_active_item_id.clone()
        }
        None => None,
    };

    let outcome = match &old_item_id {
        Some(old) => ingest_document_replacing_with_options(store, dek, &doc, old, ingest_options),
        None => ingest_document_with_options(store, dek, &doc, ingest_options),
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
        Ok(IngestOutcome::Degraded {
            item_id, reason, ..
        }) => {
            // Store a marker that can never equal the source SHA-256. On the
            // next scan the unchanged source therefore follows the replacing
            // path, removes this partial/metadata-only item and retries OCR.
            let retry_marker = crate::ingest::retryable_degraded_marker(&marker);
            if let Err(error) =
                store.upsert_indexed_file(dir_id, &doc.source_ref, &retry_marker, &item_id)
            {
                log::warn!(
                    "scanner: failed to persist retryable marker for {}: {error}",
                    doc.source_ref
                );
                result.errors += 1;
                return;
            }
            log::warn!(
                "scanner: indexed {} with retryable degraded extraction: {reason}",
                doc.source_ref
            );
            result.degraded_files += 1;
            if had_prior {
                result.updated_files += 1;
            } else {
                result.new_files += 1;
            }
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

/// 创建文件监听器（返回 watcher 和事件接收器）
pub fn create_watcher() -> Result<(
    RecommendedWatcher,
    mpsc::Receiver<notify::Result<notify::Event>>,
)> {
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
pub fn watch_directory(
    watcher: &mut RecommendedWatcher,
    path: &Path,
    recursive: bool,
) -> Result<()> {
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
        let result = scan_directory(
            &store,
            &dek,
            &dir_id,
            tmp.path(),
            true,
            &["md".into(), "txt".into()],
        )
        .unwrap();
        assert_eq!(result.total_files, 0);
    }

    #[test]
    fn scan_with_files() {
        let (store, dek, tmp) = setup_test();

        // Create test files
        let mut f1 = std::fs::File::create(tmp.path().join("doc1.md")).unwrap();
        f1.write_all(b"# Title 1\n\nContent of document 1.")
            .unwrap();

        let mut f2 = std::fs::File::create(tmp.path().join("doc2.txt")).unwrap();
        f2.write_all(b"Plain text document content here.").unwrap();

        // Create unsupported file (should be skipped)
        std::fs::File::create(tmp.path().join("image.png")).unwrap();

        let dir_id = store
            .bind_directory(tmp.path().to_str().unwrap(), true, &["md", "txt"])
            .unwrap();
        let result = scan_directory(
            &store,
            &dek,
            &dir_id,
            tmp.path(),
            true,
            &["md".into(), "txt".into()],
        )
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
    fn scan_reingests_unchanged_file_when_indexed_item_was_deleted() {
        let (store, dek, tmp) = setup_test();

        let path = tmp.path().join("doc.md");
        std::fs::write(&path, b"# Test\n\nContent.").unwrap();
        let source_ref = path.to_string_lossy().to_string();

        let dir_id = store
            .bind_directory(tmp.path().to_str().unwrap(), true, &["md"])
            .unwrap();

        let first =
            scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["md".into()]).unwrap();
        assert_eq!(first.new_files, 1);
        let first_item = store
            .get_indexed_file(&source_ref)
            .unwrap()
            .unwrap()
            .item_id
            .unwrap();

        assert!(store.delete_item(&first_item).unwrap());
        assert_eq!(store.item_count().unwrap(), 0);

        let second =
            scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["md".into()]).unwrap();
        assert_eq!(second.total_files, 1);
        assert_eq!(
            second.skipped_files, 0,
            "deleted indexed item must not make an unchanged source skip"
        );
        assert_eq!(second.new_files, 1);
        let second_item = store
            .get_indexed_file(&source_ref)
            .unwrap()
            .unwrap()
            .item_id
            .unwrap();
        assert_ne!(second_item, first_item);
        assert_eq!(store.item_count().unwrap(), 1);
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
    fn unchanged_degraded_source_is_retried_and_replaces_partial_item() {
        let (store, dek, tmp) = setup_test();
        let path = tmp.path().join("broken-scan.png");
        std::fs::write(&path, b"not a decodable image").unwrap();
        let source_ref = path.to_string_lossy().to_string();
        let dir_id = store
            .bind_directory(tmp.path().to_str().unwrap(), true, &["png"])
            .unwrap();

        let first =
            scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["png".into()]).unwrap();
        assert_eq!(first.degraded_files, 1);
        let first_row = store.get_indexed_file(&source_ref).unwrap().unwrap();
        assert!(first_row.file_hash.starts_with("retryable-degraded:"));
        let first_item = first_row.item_id.unwrap();

        let second =
            scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["png".into()]).unwrap();
        assert_eq!(
            second.skipped_files, 0,
            "degraded hash must not short-circuit"
        );
        assert_eq!(second.degraded_files, 1);
        let second_row = store.get_indexed_file(&source_ref).unwrap().unwrap();
        assert!(second_row.file_hash.starts_with("retryable-degraded:"));
        assert_ne!(second_row.item_id.as_deref(), Some(first_item.as_str()));
        assert_eq!(
            store.item_count().unwrap(),
            1,
            "partial item must be replaced"
        );
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

        assert!(
            store.count_embed_queue_by_level(1).unwrap() >= 1,
            "L1 必须入队"
        );
        assert!(
            store.count_embed_queue_by_level(2).unwrap() >= 1,
            "L2 必须入队"
        );
        assert_eq!(
            store.pending_count_by_type("classify").unwrap(),
            1,
            "classify 必须入队"
        );
    }
}
