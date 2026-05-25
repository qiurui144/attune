//! RSS 增量同步 —— add-feed / poll-now route 与周期 worker 共用的入库逻辑。
//!
//! 与 ingest_webdav / ingest_email 三段式相同：
//!   阶段 0 — 锁内读 cursor + 凭据快照；
//!   阶段 1 — 锁外做全部 HTTP I/O（条件 GET + feed 解析），物化到 Vec；
//!   阶段 2 — 逐文档短暂持锁做 indexed_files dedup + ingest + 推进 cursor。

use std::sync::Arc;

use attune_core::ingest::{
    ingest_document, DocumentSink, FeedHttpResponse, IngestOutcome, RawDocument, RssConnector,
    RssFeedFetch, SourceConnector,
};

use crate::state::AppState;

/// 对一个 RSS 订阅做一次条件 GET 增量同步。
///
/// 阻塞函数 —— caller 必须在 `spawn_blocking` 或独立线程里调。
///
/// 持锁设计：HTTP 网络抓取全程不持 vault 锁；每个 entry 的 DB 写操作才短暂拿锁，
/// 写完即释放，避免后台 worker 在慢网络 / 大 feed 时阻塞前台请求。
pub fn sync_rss_feed(state: &Arc<AppState>, feed_id: &str) -> Result<serde_json::Value, String> {
    // 阶段 0：从 rss_feeds 表读 cursor + 解密 URL，物化成 RssFeedFetch（释锁后离线 fetch）。
    let fetch_input: RssFeedFetch = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|e| e.to_string())?;
        let row = vault
            .store()
            .get_rss_feed(&dek, feed_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("rss feed {feed_id} not found"))?;
        if !row.enabled {
            return Err(format!("rss feed {feed_id} is disabled"));
        }
        RssFeedFetch {
            feed_id: row.id.clone(),
            feed_name: row.name.clone(),
            url: row.url.clone(),
            last_entry_guid: row.last_entry_guid.clone(),
            etag: row.etag.clone(),
            last_modified: row.last_modified.clone(),
        }
    };

    // 阶段 1：锁外做 HTTP + 解析。
    let connector = RssConnector::new(fetch_input);
    let mut docs: Vec<RawDocument> = Vec::new();
    let fetch_result = {
        let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
        connector.fetch_documents(&mut sink)
        // sink Box drop 在 block 末，docs 借用结束。
    };

    // 网络 / 解析失败：touch last_polled_at（防 tight-loop 重试同一 broken feed），返回 Err。
    if let Err(e) = fetch_result {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let _ = vault.store().touch_rss_polled_at(feed_id);
        return Err(format!("rss fetch {feed_id}: {e}"));
    }
    let response = connector.take_last_response();

    // 304 Not Modified —— 仅 touch last_polled_at，不动 etag/guid，不 ingest。
    if matches!(response, Some(FeedHttpResponse::NotModified)) {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let _ = vault.store().touch_rss_polled_at(feed_id);
        return Ok(serde_json::json!({
            "status": "not_modified",
            "new_entries": 0,
        }));
    }

    // 阶段 2：逐 entry 短暂持锁做 indexed_files dedup + ingest。
    let mut total = 0usize;
    let mut new_entries = 0usize;
    let mut skipped = 0usize;
    let mut errors: Vec<String> = Vec::new();
    // 记每个成功入库 entry 的 guid，用来推进 last_entry_guid（取首条 = feed 中"最新"）。
    let mut newest_ingested_guid: Option<String> = None;

    for doc in docs {
        total += 1;
        let source_ref = doc.source_ref.clone();
        let guid = doc.modified_marker.clone().unwrap_or_default();

        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = match vault.dek_db() {
            Ok(k) => k,
            Err(e) => {
                errors.push(format!("{source_ref}: vault locked: {e}"));
                continue;
            }
        };
        let store = vault.store();

        // indexed_files 短路：同 source_ref 已记录 → 跳过 ingest（content_hash 短路是
        // ingest_document 内的第二层防护）。
        if store.get_indexed_file(&source_ref).ok().flatten().is_some() {
            skipped += 1;
            continue;
        }

        match ingest_document(store, &dek, &doc) {
            Ok(IngestOutcome::Inserted { item_id, .. }) => {
                let _ = store.upsert_indexed_file(feed_id, &source_ref, &guid, &item_id);
                if newest_ingested_guid.is_none() {
                    newest_ingested_guid = Some(guid.clone());
                }
                new_entries += 1;
            }
            Ok(IngestOutcome::Updated { item_id, .. }) => {
                let _ = store.upsert_indexed_file(feed_id, &source_ref, &guid, &item_id);
                if newest_ingested_guid.is_none() {
                    newest_ingested_guid = Some(guid.clone());
                }
                new_entries += 1;
            }
            Ok(IngestOutcome::Duplicate { item_id }) => {
                let _ = store.upsert_indexed_file(feed_id, &source_ref, &guid, &item_id);
                if newest_ingested_guid.is_none() {
                    newest_ingested_guid = Some(guid.clone());
                }
                skipped += 1;
            }
            Ok(IngestOutcome::Skipped { .. }) => {
                skipped += 1;
            }
            Err(e) => {
                errors.push(format!("{source_ref}: ingest {e}"));
            }
        }
        // vault guard 隐式 drop。
    }

    // 200 OK 路径终态：写回 ETag/Last-Modified + last_entry_guid + last_polled_at。
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let store = vault.store();
        match response {
            Some(FeedHttpResponse::Ok {
                etag, last_modified, ..
            }) => {
                let _ =
                    store.update_rss_etag_lastmod(feed_id, etag.as_deref(), last_modified.as_deref());
            }
            _ => {
                // 不可达 —— 304 已早 return；fetch Err 已早 return。
                let _ = store.touch_rss_polled_at(feed_id);
            }
        }
        // 仅在确实有 entry 成功入库（含 Duplicate）时推进 guid。
        if let Some(ref g) = newest_ingested_guid {
            let _ = store.update_rss_last_entry(feed_id, g);
        }
    }

    Ok(serde_json::json!({
        "status": "ok",
        "total_entries": total,
        "new_entries": new_entries,
        "skipped": skipped,
        "errors": errors,
    }))
}
