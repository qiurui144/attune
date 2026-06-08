//! WebDAV 增量同步 —— bind-remote route 与周期 worker 共用的入库逻辑。

use std::sync::Arc;

use attune_core::ingest::{
    ingest_document, ingest_document_replacing, DocumentSink, IngestOutcome, RawDocument,
    SourceConnector,
};
use attune_core::scanner_webdav::{WebDavConfig, WebDavConnector};

use crate::state::AppState;

/// 对一个 WebDAV remote 做一次全量 ETag 增量同步。
///
/// `corpus_domain` 回填进每份 `RawDocument`，驱动 F-Pro 跨域防污染前缀注入。
/// 阻塞函数 —— caller 必须在 `spawn_blocking` 或独立线程里调。
///
/// 持锁设计：网络 list + 下载全程**不持** vault 锁；每个文档的 DB 写操作才
/// 短暂拿锁，写完即释放，避免后台 worker 在慢网络/大目录时阻塞前台请求。
pub fn sync_webdav_dir(
    state: &Arc<AppState>,
    dir_id: &str,
    config: WebDavConfig,
    corpus_domain: &str,
) -> Result<serde_json::Value, String> {
    // F-17 G1: REAL OutboundGate enforcement for the WebDAV egress. Read the
    // live `settings.privacy.webdav` toggle + current vault unlock state and
    // hand them to the connector so OutboundGate refuses BEFORE any PROPFIND/GET
    // when the user disabled webdav or the vault is locked. Fail-closed: missing
    // privacy block ⇒ disabled (matches the 5-egress default-off contract).
    let (webdav_enabled, vault_unlocked) = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let unlocked = matches!(
            vault.state(),
            attune_core::vault::VaultState::Unlocked
        );
        let enabled = vault
            .store()
            .get_meta(attune_core::llm_settings::SETTINGS_META_KEY)
            .ok()
            .flatten()
            .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
            .and_then(|s| {
                s.get("privacy")
                    .and_then(|p| p.get("webdav"))
                    .and_then(|v| v.as_bool())
            })
            .unwrap_or(false);
        (enabled, unlocked)
    };
    let connector =
        WebDavConnector::new(config).with_outbound_policy(webdav_enabled, vault_unlocked);

    // 阶段 1：锁外做全部网络 I/O（list + 逐文件 fetch），物化到 Vec。
    let mut docs: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|doc| docs.push(doc));
        connector
            .fetch_documents(&mut sink)
            .map_err(|e| e.to_string())?;
    }

    // 阶段 2：逐文档短暂持锁做增量判断 + DB 写，写完即 drop guard。
    let mut total = 0usize;
    let mut new_files = 0usize;
    let mut updated_files = 0usize;
    let mut skipped_files = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for mut doc in docs {
        total += 1;
        doc.corpus_domain = Some(corpus_domain.to_string());

        let source_ref = doc.source_ref.clone();
        let etag = doc.modified_marker.clone().unwrap_or_default();
        let filename = source_ref.rsplit('/').next().unwrap_or(&source_ref).to_string();

        // 每个文档单独拿锁：增量判断 + ingest + upsert_indexed_file，完成即 drop。
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = match vault.dek_db() {
            Ok(k) => k,
            Err(e) => {
                errors.push(format!("{filename}: vault locked: {e}"));
                continue;
            }
        };
        let store = vault.store();

        // ETag 增量判断：ETag 未变则跳过。
        let existing = store.get_indexed_file(&source_ref).ok().flatten();
        if let Some(ref ex) = existing {
            if ex.file_hash == etag && !etag.is_empty() {
                skipped_files += 1;
                continue;
            }
        }

        // 内容已变（或首次入库）：删旧 item + 入队 purge + 记 doc_update 信号。
        let old_item_id: Option<String> = existing.as_ref().and_then(|ex| {
            ex.item_id.as_ref().map(|id| {
                let _ = store.delete_item(id);
                if let Err(e) = store.enqueue_reindex(id, "purge") {
                    tracing::warn!("sync_webdav_dir: enqueue_reindex(purge) failed for {id}: {e}");
                }
                if let Err(e) = store.record_signal_event("doc_update", id, None) {
                    tracing::debug!("sync_webdav_dir: record_signal_event failed for {id}: {e}");
                }
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
                let _ = store.upsert_indexed_file(dir_id, &source_ref, &etag, &item_id);
                if old_item_id.is_some() {
                    updated_files += 1;
                } else {
                    new_files += 1;
                }
            }
            Ok(IngestOutcome::Updated { item_id, .. }) => {
                let _ = store.upsert_indexed_file(dir_id, &source_ref, &etag, &item_id);
                updated_files += 1;
            }
            Ok(IngestOutcome::Duplicate { .. }) | Ok(IngestOutcome::Skipped { .. }) => {
                skipped_files += 1;
            }
            Err(e) => {
                errors.push(format!("{filename}: ingest {e}"));
            }
        }
        // vault guard 在此隐式 drop，下一个文档前释放锁。
    }

    // 所有文档处理完毕后更新 last_etag_sync（best-effort，失败忽略）。
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let _ = vault.store().touch_webdav_remote_sync(dir_id);
    }

    Ok(serde_json::json!({
        "total_files": total,
        "new_files": new_files,
        "updated_files": updated_files,
        "skipped_files": skipped_files,
        "errors": errors,
    }))
}
