// npu-vault/crates/vault-core/src/scanner.rs

use std::collections::HashSet;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::crypto::Key32;
use crate::error::{Result, VaultError};
use crate::ingest::IngestOptions;
use crate::ingest::local::{LocalDocumentRead, LocalFileCandidate};
use crate::store::{IndexedFileRow, IndexedFileStatMarker, Store};

/// 扫描结果
#[derive(Debug, Clone)]
pub struct ScanResult {
    pub total_files: usize,
    pub new_files: usize,
    pub updated_files: usize,
    pub skipped_files: usize,
    pub deleted_files: usize,
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
    let mut result = ScanResult {
        total_files: 0,
        new_files: 0,
        updated_files: 0,
        skipped_files: 0,
        deleted_files: 0,
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
    let mut seen_refs = HashSet::new();
    {
        let mut sink = |candidate: LocalFileCandidate| {
            seen_refs.insert(candidate.source_ref.clone());
            scan_one_candidate(
                store,
                dek,
                dir_id,
                ingest_options,
                &connector,
                candidate,
                &mut result,
            );
        };
        connector.fetch_candidates(&mut sink)?;
    }
    purge_removed_local_files(store, dir_id, &seen_refs, &mut result);

    store.update_dir_last_scan(dir_id)?;
    Ok(result)
}

pub(crate) fn indexed_file_can_fast_skip(
    row: &IndexedFileRow,
    current: &IndexedFileStatMarker,
    item_active: bool,
) -> bool {
    item_active
        && !row.file_hash.is_empty()
        && !row.file_hash.starts_with("retryable-degraded:")
        && row.stat.as_ref() == Some(current)
}

fn scan_one_candidate(
    store: &Store,
    dek: &Key32,
    dir_id: &str,
    ingest_options: &IngestOptions,
    connector: &crate::ingest::local::LocalFolderConnector,
    candidate: LocalFileCandidate,
    result: &mut ScanResult,
) {
    let prior = store
        .get_indexed_file_for_dir(dir_id, &candidate.source_ref)
        .ok()
        .flatten();
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

    if let (Some(row), Some(stat)) = (prior.as_ref(), candidate.stat.as_ref()) {
        if indexed_file_can_fast_skip(row, stat, prior_active_item_id.is_some()) {
            result.total_files += 1;
            result.skipped_files += 1;
            return;
        }
    }

    match connector.read_candidate(&candidate) {
        Ok(read) => scan_one_document(store, dek, dir_id, ingest_options, read, result),
        Err(e) => {
            result.total_files += 1;
            result.errors += 1;
            log::warn!(
                "scanner: read local source {} failed: {e}",
                candidate.path.display()
            );
        }
    }
}

fn scan_one_document(
    store: &Store,
    dek: &Key32,
    dir_id: &str,
    ingest_options: &IngestOptions,
    read: LocalDocumentRead,
    result: &mut ScanResult,
) {
    use crate::ingest::{
        ingest_document_replacing_with_options, ingest_document_with_options, IngestOutcome,
    };

    let LocalDocumentRead { document: doc, stat } = read;
    result.total_files += 1;
    let marker = doc.modified_marker.clone().unwrap_or_default();

    // SHA-256 增量判断：indexed_files.file_hash 即上次的内容 hash。
    // 与旧 process_single_file 逻辑等价（两者均读文件内容算 SHA-256，无 mtime 预过滤）。
    let prior = store
        .get_indexed_file_for_dir(dir_id, &doc.source_ref)
        .ok()
        .flatten();
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
            if let Some(item_id) = row.item_id.as_deref() {
                if row.stat != stat {
                    if let Err(e) =
                        store.upsert_indexed_file_with_stat(dir_id, &doc.source_ref, &marker, item_id, stat)
                    {
                        log::warn!(
                            "scanner: failed to refresh stat marker for {}: {e}",
                            doc.source_ref
                        );
                    }
                }
            }
            result.skipped_files += 1;
            return;
        }
        Some(row) if row.file_hash == marker && !marker.is_empty() => {
            log::warn!(
                "scanner: indexed file {} points to deleted/missing item; re-ingesting unchanged source",
                doc.source_ref
            );
            if let Some(stale_item_id) = row.item_id.as_deref() {
                if !enqueue_stale_tracking_purge(store, &doc.source_ref, stale_item_id, result) {
                    return;
                }
            }
            None
        }
        Some(row) => {
            // 文件已变 → 旧 item 软删 + enqueue purge + doc_update 信号。
            // scanner 拿不到 VectorIndex / FulltextIndex 锁，必须 defer 到 server worker。
            let mut replace_old = None;
            if let Some(old) = prior_active_item_id.as_ref() {
                if source_item_has_other_refs(store, old, dir_id, &doc.source_ref, result) {
                    log::info!(
                        "scanner: source {} moved off shared item {old}; keeping old item for other refs",
                        doc.source_ref
                    );
                } else {
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
                    replace_old = Some(old.clone());
                }
            } else if let Some(stale_item_id) = row.item_id.as_deref() {
                if !enqueue_stale_tracking_purge(store, &doc.source_ref, stale_item_id, result) {
                    return;
                }
            }
            replace_old
        }
        None => None,
    };

    let outcome = match &old_item_id {
        Some(old) => ingest_document_replacing_with_options(store, dek, &doc, old, ingest_options),
        None => ingest_document_with_options(store, dek, &doc, ingest_options),
    };
    match outcome {
        Ok(IngestOutcome::Inserted { item_id, .. }) => {
            if persist_indexed_file_with_stat(
                store,
                dir_id,
                &doc.source_ref,
                &marker,
                &item_id,
                stat,
                result,
            ) {
                if had_prior {
                    result.updated_files += 1;
                } else {
                    result.new_files += 1;
                }
            }
        }
        Ok(IngestOutcome::Updated { item_id, .. }) => {
            if persist_indexed_file_with_stat(
                store,
                dir_id,
                &doc.source_ref,
                &marker,
                &item_id,
                stat,
                result,
            ) {
                result.updated_files += 1;
            }
        }
        Ok(IngestOutcome::Duplicate { item_id }) => {
            if persist_indexed_file_with_stat(
                store,
                dir_id,
                &doc.source_ref,
                &marker,
                &item_id,
                stat,
                result,
            ) {
                result.skipped_files += 1;
            }
        }
        Ok(IngestOutcome::Degraded {
            item_id, reason, ..
        }) => {
            // Store a marker that can never equal the source SHA-256. On the
            // next scan the unchanged source therefore follows the replacing
            // path, removes this partial/metadata-only item and retries OCR.
            let retry_marker = crate::ingest::retryable_degraded_marker(&marker);
            if let Err(error) = store.upsert_indexed_file_with_stat(
                dir_id,
                &doc.source_ref,
                &retry_marker,
                &item_id,
                stat,
            )
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

fn persist_indexed_file_with_stat(
    store: &Store,
    dir_id: &str,
    source_ref: &str,
    marker: &str,
    item_id: &str,
    stat: Option<IndexedFileStatMarker>,
    result: &mut ScanResult,
) -> bool {
    if let Err(e) = store.upsert_indexed_file_with_stat(dir_id, source_ref, marker, item_id, stat) {
        result.errors += 1;
        log::warn!("scanner: persist indexed_files row failed for {source_ref}: {e}");
        return false;
    }
    true
}

fn enqueue_stale_tracking_purge(
    store: &Store,
    source_ref: &str,
    item_id: &str,
    result: &mut ScanResult,
) -> bool {
    if let Err(e) = store.enqueue_reindex(item_id, "purge") {
        result.errors += 1;
        log::warn!("scanner: enqueue stale purge failed for {source_ref} item {item_id}: {e}");
        return false;
    }
    true
}

fn source_item_has_other_refs(
    store: &Store,
    item_id: &str,
    dir_id: &str,
    source_ref: &str,
    result: &mut ScanResult,
) -> bool {
    match store.indexed_file_has_other_refs(item_id, dir_id, source_ref) {
        Ok(has_refs) => has_refs,
        Err(e) => {
            result.errors += 1;
            log::warn!(
                "scanner: failed to check indexed_files refs for {source_ref} item {item_id}: {e}"
            );
            true
        }
    }
}

fn purge_removed_local_files(
    store: &Store,
    dir_id: &str,
    seen_refs: &HashSet<String>,
    result: &mut ScanResult,
) {
    let rows = match store.list_indexed_files_for_dir(dir_id) {
        Ok(rows) => rows,
        Err(e) => {
            result.errors += 1;
            log::warn!("scanner: list_indexed_files_for_dir({dir_id}) failed: {e}");
            return;
        }
    };
    for row in rows {
        if seen_refs.contains(&row.path) {
            continue;
        }
        if let Some(item_id) = row.item_id.as_deref() {
            if source_item_has_other_refs(store, item_id, dir_id, &row.path, result) {
                log::info!(
                    "scanner: removed source {} detached from shared item {item_id}",
                    row.path
                );
            } else if let Err(e) = store.delete_item(item_id) {
                result.errors += 1;
                log::warn!("scanner: delete removed local item {item_id} failed: {e}");
            } else {
                if let Err(e) = store.enqueue_reindex(item_id, "purge") {
                result.errors += 1;
                log::warn!("scanner: enqueue purge for removed local item {item_id} failed: {e}");
            }
                if let Err(e) = store.record_signal_event("doc_delete", item_id, None) {
                log::debug!("scanner: record doc_delete failed for {item_id}: {e}");
                }
            }
        }
        if let Err(e) = store.delete_indexed_file_for_dir(dir_id, &row.path) {
            result.errors += 1;
            log::warn!("scanner: delete indexed_files row {} failed: {e}", row.path);
            continue;
        }
        result.deleted_files += 1;
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
    fn scan_fast_skips_unchanged_large_file_before_content_read() {
        let (store, dek, tmp) = setup_test();
        let path = tmp.path().join("huge.md");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(101 * 1024 * 1024).unwrap();
        drop(file);
        let source_ref = path.to_string_lossy().to_string();
        let stat = crate::ingest::local::stat_marker_from_metadata(
            &std::fs::metadata(&path).unwrap(),
        )
        .unwrap();

        let dir_id = store
            .bind_directory(tmp.path().to_str().unwrap(), true, &["md"])
            .unwrap();
        let item_id = store
            .insert_item(&dek, "huge", "already indexed", None, "file", None, None)
            .unwrap();
        store
            .upsert_indexed_file_with_stat(&dir_id, &source_ref, "known-hash", &item_id, Some(stat))
            .unwrap();

        let result =
            scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["md".into()]).unwrap();

        assert_eq!(result.total_files, 1);
        assert_eq!(result.skipped_files, 1);
        assert_eq!(
            result.errors, 0,
            "matching stat marker must skip before the bounded reader rejects the large file"
        );
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
        let purge_tasks = store.dequeue_reindex_tasks(10).unwrap();
        assert_eq!(purge_tasks.len(), 1);
        assert_eq!(purge_tasks[0].1, first_item);
        assert_eq!(purge_tasks[0].2, "purge");
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
    fn scan_purges_indexed_file_when_local_source_is_removed() {
        let (store, dek, tmp) = setup_test();

        let path = tmp.path().join("doc.md");
        std::fs::write(&path, b"# Original\n\nOld content.").unwrap();
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

        std::fs::remove_file(&path).unwrap();

        let second =
            scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["md".into()]).unwrap();
        assert_eq!(second.total_files, 0);
        assert_eq!(second.deleted_files, 1);
        assert!(
            store.get_indexed_file(&source_ref).unwrap().is_none(),
            "removed local source must not leave an indexed_files row"
        );
        assert_eq!(store.item_count().unwrap(), 0);

        let tasks = store.dequeue_reindex_tasks(10).unwrap();
        assert!(
            tasks
                .iter()
                .any(|(_, item_id, action, _)| item_id == &first_item && action == "purge"),
            "removed local source must enqueue a purge for old vectors"
        );
    }

    #[test]
    fn scan_deleting_duplicate_source_keeps_shared_item_until_last_ref() {
        let (store, dek, tmp) = setup_test();

        let path_a = tmp.path().join("a.md");
        let path_b = tmp.path().join("b.md");
        std::fs::write(&path_a, b"# Same\n\nShared body.").unwrap();
        std::fs::write(&path_b, b"# Same\n\nShared body.").unwrap();
        let source_a = path_a.to_string_lossy().to_string();
        let source_b = path_b.to_string_lossy().to_string();

        let dir_id = store
            .bind_directory(tmp.path().to_str().unwrap(), true, &["md"])
            .unwrap();
        let first =
            scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["md".into()]).unwrap();
        assert_eq!(first.total_files, 2);
        assert_eq!(store.item_count().unwrap(), 1, "duplicate content shares one item");
        let shared_item = store
            .get_indexed_file_for_dir(&dir_id, &source_b)
            .unwrap()
            .unwrap()
            .item_id
            .unwrap();

        std::fs::remove_file(&path_a).unwrap();
        let second =
            scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["md".into()]).unwrap();

        assert_eq!(second.deleted_files, 1);
        assert!(
            store
                .get_indexed_file_for_dir(&dir_id, &source_a)
                .unwrap()
                .is_none(),
            "removed source row should be deleted"
        );
        assert!(
            store.item_exists(&shared_item).unwrap(),
            "shared item must stay active while b.md still references it"
        );
        assert_eq!(store.item_count().unwrap(), 1);
        assert!(
            store.dequeue_reindex_tasks(10).unwrap().is_empty(),
            "shared item must not be purged while another source still references it"
        );
    }

    #[test]
    fn scan_updating_duplicate_source_keeps_old_shared_item_for_other_ref() {
        let (store, dek, tmp) = setup_test();

        let path_a = tmp.path().join("a.md");
        let path_b = tmp.path().join("b.md");
        std::fs::write(&path_a, b"# Same\n\nShared body.").unwrap();
        std::fs::write(&path_b, b"# Same\n\nShared body.").unwrap();
        let source_a = path_a.to_string_lossy().to_string();
        let source_b = path_b.to_string_lossy().to_string();

        let dir_id = store
            .bind_directory(tmp.path().to_str().unwrap(), true, &["md"])
            .unwrap();
        scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["md".into()]).unwrap();
        let old_shared = store
            .get_indexed_file_for_dir(&dir_id, &source_b)
            .unwrap()
            .unwrap()
            .item_id
            .unwrap();

        std::fs::write(&path_a, b"# Changed\n\nOnly a.md has this new body.").unwrap();
        let second =
            scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["md".into()]).unwrap();

        assert_eq!(second.updated_files, 1);
        assert_eq!(second.new_files, 0);
        let row_a = store
            .get_indexed_file_for_dir(&dir_id, &source_a)
            .unwrap()
            .unwrap();
        let row_b = store
            .get_indexed_file_for_dir(&dir_id, &source_b)
            .unwrap()
            .unwrap();
        assert_ne!(row_a.item_id, row_b.item_id);
        assert_eq!(row_b.item_id.as_deref(), Some(old_shared.as_str()));
        assert!(store.item_exists(&old_shared).unwrap());
        assert_eq!(store.item_count().unwrap(), 2);
        assert!(
            store.dequeue_reindex_tasks(10).unwrap().is_empty(),
            "old shared item must not be purged because b.md still owns it"
        );
    }

    #[test]
    fn stat_marker_match_allows_fast_skip_without_rehashing() {
        use crate::store::IndexedFileStatMarker;

        let row = crate::store::IndexedFileRow {
            id: "row".into(),
            dir_id: "dir".into(),
            path: "/tmp/doc.md".into(),
            file_hash: "abc123".into(),
            item_id: Some("item".into()),
            stat: Some(IndexedFileStatMarker {
                size: 12,
                mtime_ns: 34,
                ctime_ns: Some(35),
                inode: Some(56),
                dev: Some(78),
            }),
        };
        let marker = IndexedFileStatMarker {
            size: 12,
            mtime_ns: 34,
            ctime_ns: Some(35),
            inode: Some(56),
            dev: Some(78),
        };

        assert!(indexed_file_can_fast_skip(&row, &marker, true));
        assert!(!indexed_file_can_fast_skip(&row, &marker, false));
        assert!(!indexed_file_can_fast_skip(
            &crate::store::IndexedFileRow {
                file_hash: crate::ingest::retryable_degraded_marker("abc123"),
                ..row.clone()
            },
            &marker,
            true
        ));
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
