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
    let mut outcomes: Vec<(String, u32, bool)> = Vec::new();

    for mut doc in docs {
        total += 1;
        doc.corpus_domain = Some(corpus_domain.to_string());

        // modified_marker 形如 "INBOX:123" 或 "INBOX:123#att0" —— 取 folder + uid。
        let marker = doc.modified_marker.clone().unwrap_or_default();
        let (folder, uid) = parse_marker(&marker);
        let source_ref = doc.source_ref.clone();

        // 仅"已确定处理"的文档才算 handled；ingest 出错 / vault 中途锁定均为未处理。
        let mut handled = false;

        {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            match vault.dek_db() {
                Err(e) => {
                    // vault 中途锁定：本文档未处理，handled 保持 false，落到下方记账。
                    errors.push(format!("{source_ref}: vault locked: {e}"));
                }
                Ok(dek) => {
                    let store = vault.store();
                    // Message-ID 增量判断：indexed_files 已记录同 source_ref 则跳过。
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
                                // ingest 主动跳过是终态决定（重抓只会再跳）—— 视为已处理。
                                skipped_items += 1;
                                handled = true;
                            }
                            Err(e) => {
                                errors.push(format!("{source_ref}: ingest {e}"));
                            }
                        }
                    }
                }
            }
            // vault guard 在此隐式 drop。
        }

        // 游标记账：一封邮件的多份文档共享 UID，全部记入 outcomes，
        // compute_folder_cursors 保证任一文档失败时游标不越过该 UID。
        if let Some(uid) = uid {
            outcomes.push((folder, uid, handled));
        }
    }

    // 全部处理完毕后推进每文件夹的 UID 游标 —— 仅推进到「所有文档都成功」的 UID
    // 之前（compute_folder_cursors 保证），失败邮件下轮按 since_uid 重新抓取。
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let store = vault.store();
        for (folder, target) in compute_folder_cursors(&outcomes) {
            let prev = store.get_folder_uid(dir_id, &folder).unwrap_or(0);
            if target > prev {
                let _ = store.set_folder_uid(dir_id, &folder, target);
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

/// 给定本轮每个文档的 (folder, uid, handled) 结果，算出每文件夹 UID 游标应推进到
/// 的目标值。关键不变量：一封邮件展开成多份文档（正文 + 每附件），它们共享同一个
/// UID；只要该 UID 的任一文档未处理成功，游标就不能越过它 —— 否则下轮 since_uid
/// 过滤会永久跳过那封邮件，静默丢数据。故某文件夹有失败文档时，游标只推进到
/// 「最小失败 UID 减 1」；全部成功则推进到见过的最大 UID。
fn compute_folder_cursors(outcomes: &[(String, u32, bool)]) -> HashMap<String, u32> {
    let mut max_seen: HashMap<String, u32> = HashMap::new();
    let mut min_failed: HashMap<String, u32> = HashMap::new();
    for (folder, uid, handled) in outcomes {
        let seen = max_seen.entry(folder.clone()).or_insert(0);
        *seen = (*seen).max(*uid);
        if !handled {
            let mf = min_failed.entry(folder.clone()).or_insert(u32::MAX);
            *mf = (*mf).min(*uid);
        }
    }
    let mut cursors = HashMap::new();
    for (folder, seen_max) in max_seen {
        let target = match min_failed.get(&folder) {
            Some(mf) => mf.saturating_sub(1),
            None => seen_max,
        };
        cursors.insert(folder, target);
    }
    cursors
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
    use super::{compute_folder_cursors, parse_marker};

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

    #[test]
    fn cursor_all_handled_advances_to_max() {
        let out = vec![
            ("INBOX".to_string(), 5, true),
            ("INBOX".to_string(), 9, true),
            ("INBOX".to_string(), 7, true),
        ];
        assert_eq!(compute_folder_cursors(&out).get("INBOX"), Some(&9));
    }

    #[test]
    fn cursor_stops_before_failed_uid_shared_by_email_documents() {
        // UID 8：正文文档成功、附件文档失败 —— 游标只能到 7，下轮重抓 UID 8。
        let out = vec![
            ("INBOX".to_string(), 6, true),
            ("INBOX".to_string(), 8, true),
            ("INBOX".to_string(), 8, false),
            ("INBOX".to_string(), 9, true),
        ];
        assert_eq!(
            compute_folder_cursors(&out).get("INBOX"),
            Some(&7),
            "UID8 有失败文档，游标必须停在 7"
        );
    }

    #[test]
    fn cursor_per_folder_independent() {
        let out = vec![
            ("INBOX".to_string(), 10, true),
            ("Sent".to_string(), 3, false),
            ("Sent".to_string(), 5, true),
        ];
        let c = compute_folder_cursors(&out);
        assert_eq!(c.get("INBOX"), Some(&10));
        assert_eq!(c.get("Sent"), Some(&2), "Sent UID3 失败 → 游标 2");
    }
}
