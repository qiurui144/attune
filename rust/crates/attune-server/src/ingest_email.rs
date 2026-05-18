//! Email 增量同步 —— bind-email route 与周期 worker 共用的入库逻辑。

use std::collections::HashMap;
use std::sync::Arc;

use attune_core::ingest::{
    ingest_document, DocumentSink, EmailConfig, EmailConnector, IngestOutcome, RawDocument,
    SourceConnector,
};

use crate::state::AppState;

/// 对一个 Email 账户做一次按 UID 增量的全文件夹同步。
///
/// `corpus_domain` 回填进每份 RawDocument，驱动 F-Pro 跨域防污染前缀注入。
/// 阻塞函数 —— caller 必须在 `spawn_blocking` 或独立线程里调。
///
/// 持锁设计：IMAP 网络抓取全程不持 vault 锁；每封邮件的 DB 写操作才短暂拿锁，
/// 写完即释放，避免后台 worker 在慢网络 / 大邮箱时阻塞前台请求。
pub fn sync_email_account(
    state: &Arc<AppState>,
    dir_id: &str,
    config: EmailConfig,
    corpus_domain: &str,
) -> Result<serde_json::Value, String> {
    // 阶段 0：从 email_folder_uids 表读每文件夹的 UID 增量游标，注入连接器。
    let mut connector = EmailConnector::new(config.clone());
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        for folder in config.effective_folders() {
            let since = vault.store().get_folder_uid(dir_id, &folder).unwrap_or(0);
            connector.set_folder_since(&folder, since);
        }
    }

    // 阶段 1：锁外做全部 IMAP 网络 I/O（connect + login + fetch），物化到 Vec。
    let mut docs: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|doc| docs.push(doc));
        connector
            .fetch_documents(&mut sink)
            .map_err(|e| e.to_string())?;
    }

    // 阶段 2：逐文档短暂持锁做去重判断 + DB 写，写完即 drop guard。
    let mut total = 0usize;
    let mut new_items = 0usize;
    let mut updated_items = 0usize;
    let mut skipped_items = 0usize;
    let mut errors: Vec<String> = Vec::new();
    // 每文件夹本轮见到的最大 UID —— 全部成功后推进增量游标。
    let mut max_uid: HashMap<String, u32> = HashMap::new();

    for mut doc in docs {
        total += 1;
        doc.corpus_domain = Some(corpus_domain.to_string());

        // modified_marker 形如 "INBOX:123" 或 "INBOX:123#att0" —— 取 folder + uid。
        let marker = doc.modified_marker.clone().unwrap_or_default();
        let (folder, uid) = parse_marker(&marker);
        let source_ref = doc.source_ref.clone();

        // 仅"已确定处理"的邮件才推进 UID 游标：ingest 出错 / vault 中途锁定的
        // 邮件不推进，下轮重新抓取，避免静默丢邮件。
        let mut handled = false;

        {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            let dek = match vault.dek_db() {
                Ok(k) => k,
                Err(e) => {
                    errors.push(format!("{source_ref}: vault locked: {e}"));
                    continue;
                }
            };
            let store = vault.store();

            // Message-ID 增量判断：indexed_files 已记录同 source_ref 则跳过
            // （ingest_document 内部的 content_hash 短路也会兜底转发邮件）。
            let existing = store.get_indexed_file(&source_ref).ok().flatten();
            if existing.is_some() {
                skipped_items += 1;
                handled = true;
            } else {
                match ingest_document(store, &dek, &doc) {
                    Ok(IngestOutcome::Inserted { item_id, .. }) => {
                        let _ = store.upsert_indexed_file(dir_id, &source_ref, &marker, &item_id);
                        new_items += 1;
                        handled = true;
                    }
                    Ok(IngestOutcome::Updated { item_id, .. }) => {
                        let _ = store.upsert_indexed_file(dir_id, &source_ref, &marker, &item_id);
                        updated_items += 1;
                        handled = true;
                    }
                    Ok(IngestOutcome::Duplicate { item_id }) => {
                        // 内容与已有 item 撞 hash（转发邮件）—— 记 indexed_files 避免下轮重判。
                        let _ = store.upsert_indexed_file(dir_id, &source_ref, &marker, &item_id);
                        skipped_items += 1;
                        handled = true;
                    }
                    Ok(IngestOutcome::Skipped { .. }) => {
                        // ingest 主动跳过是终态决定（邮件字节不变，重抓只会再跳）—— 推进游标。
                        skipped_items += 1;
                        handled = true;
                    }
                    Err(e) => {
                        errors.push(format!("{source_ref}: ingest {e}"));
                    }
                }
            }
            // vault guard 在此隐式 drop，下一封邮件前释放锁。
        }

        // 仅已处理的邮件推进该文件夹的 UID 游标。
        if handled {
            if let Some(uid) = uid {
                let entry = max_uid.entry(folder).or_insert(0);
                *entry = (*entry).max(uid);
            }
        }
    }

    // 全部处理完毕后推进每文件夹的 UID 游标 + 记录 last_sync（best-effort）。
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let store = vault.store();
        for (folder, uid) in &max_uid {
            let prev = store.get_folder_uid(dir_id, folder).unwrap_or(0);
            if *uid > prev {
                let _ = store.set_folder_uid(dir_id, folder, *uid);
            }
        }
        let _ = store.touch_email_account_sync(dir_id);
    }

    Ok(serde_json::json!({
        "total_documents": total,
        "new_items": new_items,
        "updated_items": updated_items,
        "skipped_items": skipped_items,
        "errors": errors,
    }))
}

/// 拆 modified_marker（"INBOX:123" / "INBOX:123#att0"）为 (folder, Option<uid>)。
/// 附件 marker 含 "#attN" 后缀，uid 仍取冒号后到 '#' 前的数字段。
fn parse_marker(marker: &str) -> (String, Option<u32>) {
    let (folder, rest) = match marker.split_once(':') {
        Some((f, r)) => (f.to_string(), r),
        None => return (marker.to_string(), None),
    };
    let uid_str = rest.split('#').next().unwrap_or(rest);
    (folder, uid_str.parse::<u32>().ok())
}

#[cfg(test)]
mod tests {
    use super::parse_marker;

    #[test]
    fn parse_marker_handles_plain_and_attachment() {
        assert_eq!(parse_marker("INBOX:42"), ("INBOX".to_string(), Some(42)));
        assert_eq!(parse_marker("Sent:7#att0"), ("Sent".to_string(), Some(7)));
        assert_eq!(parse_marker("garbage"), ("garbage".to_string(), None));
    }

    #[test]
    fn parse_marker_handles_edge_cases() {
        assert_eq!(parse_marker(""), ("".to_string(), None));
        assert_eq!(parse_marker("INBOX:"), ("INBOX".to_string(), None));
        assert_eq!(parse_marker("INBOX:notanumber"), ("INBOX".to_string(), None));
    }
}
