//! ingest_document — 唯一的统一入库函数。
//!
//! 把 0.6 之前散在 4 处（routes/upload · routes/ingest · scanner ·
//! scanner_webdav）的五步收成一个函数：
//!   1. parse —— `parser::parse_bytes` 把原始字节解析成 (title, content)
//!   2. content_hash 短路判重 —— 命中 → 返回 Duplicate，跳过其余四步
//!   3. insert_item —— 写加密 item 行（domain / tags 从 RawDocument 透传）
//!   4. upsert_chunk_breadcrumbs_from_content —— 写 Citation sidecar
//!   5a. enqueue_embedding —— Level-1 章节 + Level-2 段落块两层；
//!       corpus_domain != "general" 时对每个 chunk_text 注入 `[领域: X] ` 前缀
//!       （F-Pro 跨域防污染，bge-m3 corpus tagging）
//!   5b. set_item_corpus_domain —— corpus_domain 非空非 general 时写 item 领域标签
//!   5c. enqueue_classify —— 自动分类任务
//!
//! 不碰 VectorIndex / FulltextIndex（server AppState 的独立 Mutex）：向量写入
//! 经 embed_queue defer 给 server 后台 worker。FTS 即时索引由 server 层薄壳
//! caller 在拿到 item_id 后自己补 `fulltext.add_document`（保持锁顺序单纯）。
//!
//! Updated 态（旧 item 替换）：caller 负责在调用前完成增量检测（各源的机制不同：
//! 本地文件夹用 indexed_files/mtime、WebDAV 用 ETag、Email 用 Message-ID），
//! 检测到变更后调 `ingest_document_replacing` 并传入旧 item_id。
//! `ingest_document` 本身只负责"这份文档怎么入库"，不做源特定的增量判断。

use crate::crypto::Key32;
use crate::error::Result;
use crate::ingest::connector::RawDocument;
use crate::store::items::compute_content_hash;
use crate::store::Store;
use crate::{chunker, parser};

/// 一次 `ingest_document` 的结果，区分四态便于 caller 统计与回归断言。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    /// 新文档已入库。`chunks_enqueued` = L1 + L2 入队总数。
    Inserted { item_id: String, chunks_enqueued: usize },
    /// content_hash 命中已有 item —— 跳过入库，返回已存在的 item_id。
    Duplicate { item_id: String },
    /// 同 source_ref 的旧文档内容已变 —— 旧 item 软删 + enqueue purge，
    /// 新内容作为新 item 入库。
    Updated { item_id: String, old_item_id: String },
    /// 解析后内容为空或 modified_marker 未变 —— 不入库。
    Skipped { reason: String },
}

/// 把一份 `RawDocument` 走完统一五步（Inserted / Duplicate / Skipped 三态）。
///
/// `dek` 是 vault 数据加密密钥。caller 必须已确认 vault 处于 Unlocked。
/// Updated 检测（增量判断 + 旧 item 软删）由 caller 在调用前完成，
/// 检测到变更时改调 `ingest_document_replacing`。
pub fn ingest_document(store: &Store, dek: &Key32, raw: &RawDocument) -> Result<IngestOutcome> {
    ingest_document_inner(store, dek, raw, None)
}

/// 带已知 `old_item_id` 的入库函数。caller 在调用前已自行完成旧 item 删除 +
/// purge 入队，此处直接走新文档五步并将 old_item_id 透传到 Updated 态结果。
pub fn ingest_document_replacing(
    store: &Store,
    dek: &Key32,
    raw: &RawDocument,
    old_item_id: &str,
) -> Result<IngestOutcome> {
    ingest_document_inner(store, dek, raw, Some(old_item_id.to_string()))
}

fn ingest_document_inner(
    store: &Store,
    dek: &Key32,
    raw: &RawDocument,
    old_item_id: Option<String>,
) -> Result<IngestOutcome> {
    // 1. parse
    let filename = raw.parse_filename();
    let (parsed_title, content) = parser::parse_bytes(&raw.content, &filename)?;
    if content.trim().is_empty() {
        return Ok(IngestOutcome::Skipped {
            reason: "empty content after parse".into(),
        });
    }
    // 源给的 title 优先，缺失时用 parser 提取的兜底。
    let title = if raw.title.trim().is_empty() {
        parsed_title
    } else {
        raw.title.clone()
    };

    // 2. content_hash 短路判重
    let content_hash = compute_content_hash(&content);
    if let Some(existing_id) = store.find_item_by_content_hash(&content_hash)? {
        return Ok(IngestOutcome::Duplicate { item_id: existing_id });
    }

    // 3. insert_item — domain / tags 从 RawDocument 一等字段透传（决策 1）。
    let source_type = raw.source_kind.item_source_type();
    let item_id = store.insert_item(
        dek,
        &title,
        &content,
        Some(&raw.uri),
        source_type,
        raw.domain.as_deref(),
        raw.tags.as_deref(),
    )?;

    // corpus_domain：非空且非 "general" 时启用 F-Pro 跨域防污染（决策 2）。
    let active_corpus_domain: Option<&str> = raw
        .corpus_domain
        .as_deref()
        .filter(|d| !d.is_empty() && *d != "general");

    // 4. breadcrumbs sidecar（失败不阻塞入库 —— 仅 Citation path 缺失）
    if let Err(e) = store.upsert_chunk_breadcrumbs_from_content(dek, &item_id, &content) {
        log::warn!("ingest: upsert_chunk_breadcrumbs failed for {item_id}: {e}");
    }

    // 5a. embedding：Level-1 章节 + Level-2 段落块。
    //     corpus_domain 启用时给每个 chunk_text 注入 `[领域: X] ` 前缀，让 bge-m3
    //     在向量空间把同领域文档聚集、缓解跨域污染。
    let sections = chunker::extract_sections(&content);
    let tag_chunk = |s: &str| -> String {
        match active_corpus_domain {
            Some(d) => format!("[领域: {d}] {s}"),
            None => s.to_string(),
        }
    };
    let mut chunk_counter: usize = 0;

    // L1：每个章节作为整体入队（section_idx = 该章节在 sections 中的位置）。
    for (section_idx, section_text) in &sections {
        if section_text.trim().is_empty() {
            continue;
        }
        let tagged = tag_chunk(section_text);
        store.enqueue_embedding(&item_id, chunk_counter, &tagged, 1, 1, *section_idx)?;
        chunk_counter += 1;
    }

    // L2：每个章节再按滑动窗口拆成小块入队（跳过空 section）。
    for (section_idx, section_text) in &sections {
        if section_text.trim().is_empty() {
            continue;
        }
        for chunk_text in
            chunker::chunk(section_text, chunker::DEFAULT_CHUNK_SIZE, chunker::DEFAULT_OVERLAP)
        {
            if chunk_text.trim().is_empty() {
                continue;
            }
            let tagged = tag_chunk(&chunk_text);
            store.enqueue_embedding(&item_id, chunk_counter, &tagged, 2, 2, *section_idx)?;
            chunk_counter += 1;
        }
    }

    // 5b. item 级 corpus_domain 标签（search 按 query intent 跨域降权依赖此列）。
    if let Some(d) = active_corpus_domain {
        if let Err(e) = store.set_item_corpus_domain(&item_id, d) {
            log::warn!("ingest: set_item_corpus_domain failed for {item_id}: {e}");
        }
    }

    // 5c. classify（失败不阻塞 —— 文档已可被搜到，仅缺自动分类）
    if let Err(e) = store.enqueue_classify(&item_id, 3) {
        log::warn!("ingest: enqueue_classify failed for {item_id}: {e}");
    }

    // doc_create 信号（失败静默，不阻塞）
    let _ = store.record_signal_event("doc_create", &item_id, None);

    match old_item_id {
        Some(old) => Ok(IngestOutcome::Updated { item_id, old_item_id: old }),
        None => Ok(IngestOutcome::Inserted { item_id, chunks_enqueued: chunk_counter }),
    }
}
