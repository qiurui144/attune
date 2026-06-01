//! Git 增量同步 —— bind-git / sync-git route 与（未来）周期 worker 共用的入库逻辑。
//!
//! 与 ingest_webdav / ingest_rss 同三段式：
//!   阶段 0 — 锁内读 git_sources 配置 + token + cursor 快照；
//!   阶段 1 — 锁外做 clone/walk（网络 I/O，物化到 Vec）；
//!   阶段 2 — 逐文档短暂持锁做 indexed_files dedup + ingest；末尾删上游消失文件
//!            + 推进 commit 游标（成功才推进）。
//!
//! 增量语义：connector 每文件 marker = 内容 SHA-256。indexed_files.file_hash 命中
//! 即跳过（未变）；变更走 ingest_document_replacing；本次 fetch 缺失的旧文件 = 上游
//! 删除 → delete_item + enqueue_reindex(purge) + 删 indexed_files 行。force-push 不
//! 影响此路径（始终全量 walk + 逐文件 hash 比对，content_hash 短路兜底不重嵌）。

use std::collections::HashSet;
use std::sync::Arc;

use attune_core::ingest::git::{GitConnector, GitSourceConfig};
use attune_core::ingest::{
    ingest_document, ingest_document_replacing, DocumentSink, IngestOutcome, RawDocument,
    SourceConnector,
};
use attune_core::net::url_guard;
use attune_core::store::git_sources::GitSourceRow;

use crate::state::AppState;

/// 把 git_sources 行物化成 connector 配置（token 注入内存，不落盘）。
fn config_from_row(row: &GitSourceRow, token: Option<String>) -> GitSourceConfig {
    let include_glob: Vec<String> =
        serde_json::from_str(&row.include_glob).unwrap_or_default();
    let exclude_glob: Vec<String> =
        serde_json::from_str(&row.exclude_glob).unwrap_or_default();
    GitSourceConfig {
        url: row.url.clone(),
        branch: row.branch.clone(),
        subdir: row.subdir.clone(),
        include_glob,
        exclude_glob,
        corpus_domain: Some(row.corpus_domain.clone()),
        token,
        max_files: row.max_files,
        max_file_bytes: row.max_file_bytes,
        max_total_bytes: row.max_total_bytes,
        last_commit_sha: row.last_commit_sha.clone(),
        allow_hosts: Vec::new(),
    }
}

/// 对一个 git 源做一次同步（首次 bind = 全量；后续 sync = 增量）。
///
/// 阻塞函数 —— caller 必须在 `spawn_blocking` 或独立线程里调。
/// `token` 是调用方解密后的明文凭据（仅本调用生命周期，不落盘）。
pub fn sync_git_source(
    state: &Arc<AppState>,
    dir_id: &str,
    token: Option<String>,
) -> Result<serde_json::Value, String> {
    // 阶段 0：读配置快照（释锁后离线 clone）。
    let config: GitSourceConfig = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|e| e.to_string())?;
        let row = vault
            .store()
            .get_git_source(&dek, dir_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("git source {dir_id} not found"))?;
        config_from_row(&row, token)
    };

    // SSRF 校验（route 入口也会校验, 这里再校验一次防 worker 直调路径）。
    let connector = GitConnector::with_cloner(config, Box::new(attune_core::ingest::git::Git2Cloner))
        .map_err(|e| format!("{e}"))?;
    connector
        .check_ssrf(&|h| url_guard::system_resolve(h))
        .map_err(|e| format!("{e}"))?;

    // 阶段 1：锁外 clone + walk，物化所有匹配文档。
    let mut docs: Vec<RawDocument> = Vec::new();
    let fetch_result = {
        let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
        connector.fetch_documents(&mut sink)
    };
    if let Err(e) = fetch_result {
        // 同步失败：保留旧游标，仅 touch synced（防 tight-loop），返回 Err。
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let _ = vault.store().touch_git_synced(dir_id);
        return Err(format!("{e}"));
    }
    let commit_sha = connector.take_last_commit().unwrap_or_default();

    // 阶段 2：逐文档短暂持锁 dedup + ingest；记录本次出现的 source_ref 供删除检测。
    let mut total = 0usize;
    let mut new_files = 0usize;
    let mut updated_files = 0usize;
    let mut skipped_files = 0usize;
    let mut deleted_files = 0usize;
    let mut errors: Vec<String> = Vec::new();
    let mut seen_refs: HashSet<String> = HashSet::new();

    for doc in docs {
        total += 1;
        let source_ref = doc.source_ref.clone();
        let marker = doc.modified_marker.clone().unwrap_or_default();
        seen_refs.insert(source_ref.clone());

        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = match vault.dek_db() {
            Ok(k) => k,
            Err(e) => {
                errors.push(format!("{source_ref}: vault locked: {e}"));
                continue;
            }
        };
        let store = vault.store();

        // 增量短路：同 source_ref 已记录且内容 SHA 未变 → 跳过。
        let existing = store.get_indexed_file(&source_ref).ok().flatten();
        if let Some(ref ex) = existing {
            if ex.file_hash == marker && !marker.is_empty() {
                skipped_files += 1;
                continue;
            }
        }

        // 内容已变（或首入）：删旧 item + 入队 purge + 信号。
        let old_item_id: Option<String> = existing.as_ref().and_then(|ex| {
            ex.item_id.as_ref().map(|id| {
                let _ = store.delete_item(id);
                if let Err(e) = store.enqueue_reindex(id, "purge") {
                    tracing::warn!("sync_git_source: enqueue_reindex(purge) failed for {id}: {e}");
                }
                let _ = store.record_signal_event("doc_update", id, None);
                id.clone()
            })
        });

        let outcome = if let Some(ref old_id) = old_item_id {
            ingest_document_replacing(store, &dek, &doc, old_id)
        } else {
            ingest_document(store, &dek, &doc)
        };

        match outcome {
            Ok(IngestOutcome::Inserted { item_id, .. }) => {
                let _ = store.upsert_indexed_file(dir_id, &source_ref, &marker, &item_id);
                if old_item_id.is_some() {
                    updated_files += 1;
                } else {
                    new_files += 1;
                }
            }
            Ok(IngestOutcome::Updated { item_id, .. }) => {
                let _ = store.upsert_indexed_file(dir_id, &source_ref, &marker, &item_id);
                updated_files += 1;
            }
            Ok(IngestOutcome::Duplicate { .. }) | Ok(IngestOutcome::Skipped { .. }) => {
                skipped_files += 1;
            }
            Err(e) => {
                errors.push(format!("{source_ref}: ingest {e}"));
            }
        }
        // vault guard 隐式 drop。
    }

    // 删除检测：indexed_files 里属于本 dir 但本次未出现的 source_ref = 上游已删。
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let store = vault.store();
        if let Ok(prev) = store.list_indexed_files_for_dir(dir_id) {
            for row in prev {
                if !seen_refs.contains(&row.path) {
                    if let Some(item_id) = &row.item_id {
                        let _ = store.delete_item(item_id);
                        if let Err(e) = store.enqueue_reindex(item_id, "purge") {
                            tracing::warn!("sync_git_source: purge deleted {item_id}: {e}");
                        }
                        let _ = store.record_signal_event("doc_delete", item_id, None);
                    }
                    let _ = store.delete_indexed_file(&row.path);
                    deleted_files += 1;
                }
            }
        }
    }

    // 终态：推进 commit 游标 + last_scan（成功才推进）。
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let store = vault.store();
        let _ = store.update_dir_last_scan(dir_id);
        if !commit_sha.is_empty() {
            let _ = store.update_git_cursor(dir_id, &commit_sha);
        } else {
            let _ = store.touch_git_synced(dir_id);
        }
    }

    Ok(serde_json::json!({
        "commit": commit_sha,
        "total_files": total,
        "new_files": new_files,
        "updated_files": updated_files,
        "skipped_files": skipped_files,
        "deleted_files": deleted_files,
        "errors": errors,
    }))
}
