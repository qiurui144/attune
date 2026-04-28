// npu-vault/crates/vault-core/src/scanner.rs

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use walkdir::WalkDir;

use crate::chunker;
use crate::crypto::Key32;
use crate::error::{Result, VaultError};
use crate::parser;
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

/// 全量扫描指定目录
pub fn scan_directory(
    store: &Store,
    dek: &Key32,
    dir_id: &str,
    dir_path: &Path,
    recursive: bool,
    file_types: &[String],
) -> Result<ScanResult> {
    let mut result = ScanResult {
        total_files: 0,
        new_files: 0,
        updated_files: 0,
        skipped_files: 0,
        errors: 0,
    };

    let walker = if recursive {
        WalkDir::new(dir_path)
    } else {
        WalkDir::new(dir_path).max_depth(1)
    };

    let suffixes: Vec<String> = file_types
        .iter()
        .map(|t| {
            if t.starts_with('.') {
                t.clone()
            } else {
                format!(".{t}")
            }
        })
        .collect();

    for entry in walker.into_iter().filter_map(|e| {
        e.map_err(|err| { log::warn!("WalkDir error: {err}"); }).ok()
    }) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        // 检查文件类型
        let ext = path
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
            .unwrap_or_default();
        if !suffixes.is_empty() && !suffixes.iter().any(|s| s == &ext) {
            continue;
        }

        result.total_files += 1;

        match process_single_file(store, dek, dir_id, path) {
            Ok(FileAction::New) => result.new_files += 1,
            Ok(FileAction::Updated) => result.updated_files += 1,
            Ok(FileAction::Skipped) => result.skipped_files += 1,
            Err(e) => {
                log::warn!("Failed to process {}: {}", path.display(), e);
                result.errors += 1;
            }
        }
    }

    // 更新 last_scan
    store.update_dir_last_scan(dir_id)?;

    Ok(result)
}

#[derive(Debug)]
enum FileAction {
    New,
    Updated,
    Skipped,
}

fn process_single_file(store: &Store, dek: &Key32, dir_id: &str, path: &Path) -> Result<FileAction> {
    let path_str = path.to_string_lossy().to_string();
    let hash = parser::file_hash(path)?;

    // 检查是否已索引且未变更
    let was_existing = if let Some(existing) = store.get_indexed_file(&path_str)? {
        if existing.file_hash == hash {
            return Ok(FileAction::Skipped);
        }
        // 文件已变更: 删除旧数据
        if let Some(item_id) = &existing.item_id {
            store.delete_item(item_id)?;
        }
        true
    } else {
        false
    };

    // 解析文件
    let (title, content) = parser::parse_file(path)?;
    if content.trim().is_empty() {
        return Ok(FileAction::Skipped);
    }

    // 插入知识条目
    let item_id = store.insert_item(dek, &title, &content, None, "file", None, None)?;

    // v0.6 Phase B F-Pro：从 bind_dir 读 corpus_domain 并赋给 item，driving cross-domain penalty。
    // 失败 fallback 'general'（默认行为，不破坏现有数据）。
    let corpus_domain = store.get_dir_corpus_domain(dir_id).unwrap_or_else(|_| "general".into());
    if corpus_domain != "general" {
        if let Err(e) = store.set_item_corpus_domain(&item_id, &corpus_domain) {
            log::warn!("F-Pro set_item_corpus_domain failed for {item_id}: {e}");
        }
    }

    // F2 (W3 batch A, 2026-04-27)：写 chunk_breadcrumbs sidecar 让 Citation 透传 path。
    // per reviewer I4：scanner / webdav 路径同步覆盖，避免文件夹监听 / WebDAV 来源
    // 的 item 永远 placeholder。
    if let Err(e) = store.upsert_chunk_breadcrumbs_from_content(dek, &item_id, &content) {
        log::warn!("F2 upsert_chunk_breadcrumbs failed for item {item_id}: {e}");
    }

    // 提取章节 + 分块，加入 embedding 队列
    let sections = chunker::extract_sections(&content);
    let mut chunk_counter = 0;

    // v0.6 Phase B F-Pro Stage 2：domain prefix 注入到 chunk_text 让 embedding 把同领域文档拉近。
    // bge-m3 对前缀敏感（业界 corpus tagging 通行技巧），同领域文档在向量空间自然聚集，
    // 跨领域距离自然变远 → 跨域污染症状大幅缓解。仅 corpus_domain != general 时注入
    // （general 文档不需要 tag，避免污染老 vault 数据）。
    let prefix = if corpus_domain != "general" {
        format!("[领域: {}] ", corpus_domain)
    } else {
        String::new()
    };
    let with_prefix = |s: &str| -> String {
        if prefix.is_empty() { s.to_string() } else { format!("{}{}", prefix, s) }
    };

    // Level 1: 章节
    for (section_idx, section_text) in &sections {
        if !section_text.trim().is_empty() {
            let tagged = with_prefix(section_text);
            store.enqueue_embedding(&item_id, chunk_counter, &tagged, 1, 1, *section_idx)?;
            chunk_counter += 1;
        }
    }

    // Level 2: 段落块
    for (section_idx, section_text) in &sections {
        let chunks = chunker::chunk(section_text, chunker::DEFAULT_CHUNK_SIZE, chunker::DEFAULT_OVERLAP);
        for chunk_text in &chunks {
            let tagged = with_prefix(chunk_text);
            store.enqueue_embedding(&item_id, chunk_counter, &tagged, 2, 2, *section_idx)?;
            chunk_counter += 1;
        }
    }

    // Auto-enqueue classification task
    store.enqueue_classify(&item_id, 3)?;

    // 记录文件索引
    store.upsert_indexed_file(dir_id, &path_str, &hash, &item_id)?;

    if was_existing {
        Ok(FileAction::Updated)
    } else {
        Ok(FileAction::New)
    }
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
}
