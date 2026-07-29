//! ingest_document — 唯一的统一入库函数。
//!
//! 把 0.6 之前散在 4 处（routes/upload · routes/ingest · scanner ·
//! scanner_webdav）的五步收成一个函数：parse → content_hash 判重 →
//! insert_item（透传 domain/tags）→ breadcrumbs sidecar →
//! enqueue_embedding（L1 章节 + L2 段落块，corpus_domain 注入前缀）+
//! set_item_corpus_domain + enqueue_classify。
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
use crate::document_model::{DocumentNode, NodeKind};
use crate::document_transform::{transform_document, TransformInput};
use crate::error::Result;
use crate::ingest::connector::RawDocument;
use crate::store::items::compute_content_hash;
use crate::store::Store;
use crate::{chunker, parser};
use std::path::Path;

/// 一次 `ingest_document` 的结果，区分四态便于 caller 统计与回归断言。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    /// 新文档已入库。`chunks_enqueued` = L1 + L2 入队总数。
    Inserted {
        item_id: String,
        chunks_enqueued: usize,
    },
    /// content_hash 命中已有 item —— 跳过入库，返回已存在的 item_id。
    Duplicate { item_id: String },
    /// 同 source_ref 的旧文档内容已变 —— 旧 item 软删 + enqueue purge，
    /// 新内容作为新 item 入库。
    Updated {
        item_id: String,
        old_item_id: String,
    },
    /// Useful metadata or partial OCR text was indexed, but extraction was not
    /// complete. Incremental scanners must retain this item as retryable rather
    /// than recording the source hash as permanently complete.
    Degraded {
        item_id: String,
        chunks_enqueued: usize,
        reason: String,
    },
    /// 解析后内容为空或 modified_marker 未变 —— 不入库。
    Skipped { reason: String },
}

#[derive(Debug, Clone)]
pub struct IngestOptions {
    pub parser: parser::ParseOptions,
    pub chunking: chunker::ChunkingOptions,
}

impl Default for IngestOptions {
    fn default() -> Self {
        Self {
            parser: parser::ParseOptions::default(),
            chunking: chunker::ChunkingOptions::default(),
        }
    }
}

pub fn retryable_degraded_marker(source_marker: &str) -> String {
    format!("retryable-degraded:{source_marker}")
}

impl IngestOptions {
    pub fn with_profile(profile: Option<&str>) -> Self {
        Self {
            parser: parser::ParseOptions::with_profile(profile),
            ..Self::default()
        }
    }

    pub fn with_chunking(mut self, chunking: chunker::ChunkingOptions) -> Self {
        self.chunking = chunking;
        self
    }

    pub fn with_scheduler_base(mut self, scheduler_base: Option<&str>) -> Self {
        self.parser = self.parser.with_scheduler_base(scheduler_base);
        self
    }

    pub fn with_scheduler_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.parser = self.parser.with_scheduler_timeout_ms(timeout_ms);
        self
    }

    pub fn with_background_ingest_ocr(mut self) -> Self {
        self.parser = self.parser.with_background_ingest_ocr();
        self
    }
}

pub fn enqueue_content_embeddings(
    store: &Store,
    item_id: &str,
    content: &str,
    corpus_domain: Option<&str>,
    chunking: chunker::ChunkingOptions,
) -> Result<usize> {
    let active_corpus_domain = corpus_domain.filter(|d| !d.is_empty() && *d != "general");
    let tag_chunk = |s: &str| -> String {
        match active_corpus_domain {
            Some(d) => format!("[领域: {d}] {s}"),
            None => s.to_string(),
        }
    };
    let mut span_cursor = 0usize;
    if let Some(count) = enqueue_structured_content_embeddings(
        store,
        item_id,
        content,
        chunking,
        &tag_chunk,
        &mut span_cursor,
    )? {
        return Ok(count);
    }

    let mut chunk_counter: usize = 0;
    let sections = chunker::extract_sections(content);

    if chunking.include_level1 {
        for (section_idx, section_text) in &sections {
            if section_text.trim().is_empty() {
                continue;
            }
            let span = locate_chunk_span(content, section_text, &mut span_cursor);
            let tagged = tag_chunk(section_text);
            store.enqueue_embedding(item_id, chunk_counter, &tagged, 1, 1, *section_idx)?;
            if let Some((span_start, span_end)) = span {
                store.upsert_chunk_span(
                    item_id,
                    chunk_counter,
                    span_start,
                    span_end,
                    1,
                    *section_idx,
                )?;
            }
            chunk_counter += 1;
        }
    }

    if chunking.include_level2 {
        for (section_idx, section_text) in &sections {
            if section_text.trim().is_empty() {
                continue;
            }
            for chunk_text in chunker::chunk(section_text, chunking.chunk_size, chunking.overlap) {
                if chunk_text.trim().is_empty() {
                    continue;
                }
                let span = locate_chunk_span(content, &chunk_text, &mut span_cursor);
                let tagged = tag_chunk(&chunk_text);
                store.enqueue_embedding(item_id, chunk_counter, &tagged, 2, 2, *section_idx)?;
                if let Some((span_start, span_end)) = span {
                    store.upsert_chunk_span(
                        item_id,
                        chunk_counter,
                        span_start,
                        span_end,
                        2,
                        *section_idx,
                    )?;
                }
                chunk_counter += 1;
            }
        }
    }

    Ok(chunk_counter)
}

fn enqueue_structured_content_embeddings(
    store: &Store,
    item_id: &str,
    content: &str,
    chunking: chunker::ChunkingOptions,
    tag_chunk: &dyn Fn(&str) -> String,
    span_cursor: &mut usize,
) -> Result<Option<usize>> {
    let outline = transform_document(TransformInput {
        document_id: item_id.to_string(),
        title: item_id.to_string(),
        source_path: None,
        text: content.to_string(),
    });
    let indexable_nodes = outline
        .nodes
        .iter()
        .filter(|node| structured_node_is_indexable(node))
        .collect::<Vec<_>>();
    if indexable_nodes.len() < 3 {
        return Ok(None);
    }

    let structured_chunks = structured_outline_embedding_chunks(&indexable_nodes, chunking);
    if structured_chunks_exceed_legacy_budget(content, chunking, structured_chunks.len()) {
        return Ok(None);
    }

    let mut chunk_counter = 0usize;
    for (section_idx, chunk) in structured_chunks.into_iter().enumerate() {
        if chunk.level == 1 && !chunking.include_level1
            || chunk.level == 2 && !chunking.include_level2
        {
            continue;
        }
        for chunk_text in chunker::chunk(&chunk.text, chunking.chunk_size, chunking.overlap) {
            if chunk_text.trim().is_empty() {
                continue;
            }
            let span = locate_chunk_span(content, &chunk_text, span_cursor);
            let tagged = tag_chunk(&chunk_text);
            store.enqueue_embedding(
                item_id,
                chunk_counter,
                &tagged,
                chunk.level,
                chunk.level,
                section_idx,
            )?;
            if let Some((span_start, span_end)) = span {
                store.upsert_chunk_span(
                    item_id,
                    chunk_counter,
                    span_start,
                    span_end,
                    chunk.level,
                    section_idx,
                )?;
            }
            chunk_counter += 1;
        }
    }

    if chunk_counter == 0 {
        return Ok(None);
    }
    Ok(Some(chunk_counter))
}

fn locate_chunk_span(
    content: &str,
    chunk_text: &str,
    cursor: &mut usize,
) -> Option<(usize, usize)> {
    let needle = chunk_text.trim();
    if needle.is_empty() {
        return None;
    }
    let start_at = next_char_boundary(content, (*cursor).min(content.len()));
    if let Some(rel) = content.get(start_at..).and_then(|s| s.find(needle)) {
        let start = start_at + rel;
        let end = start + needle.len();
        *cursor = next_char_boundary(content, start.saturating_add(1));
        return Some((start, end));
    }
    if let Some(start) = content.find(needle) {
        let end = start + needle.len();
        *cursor = next_char_boundary(content, start.saturating_add(1));
        return Some((start, end));
    }
    None
}

fn next_char_boundary(content: &str, idx: usize) -> usize {
    if idx >= content.len() {
        return content.len();
    }
    let mut cursor = idx;
    while cursor < content.len() && !content.is_char_boundary(cursor) {
        cursor += 1;
    }
    cursor
}

fn structured_chunks_exceed_legacy_budget(
    content: &str,
    chunking: chunker::ChunkingOptions,
    structured_count: usize,
) -> bool {
    let legacy_count = legacy_embedding_chunk_estimate(content, chunking);
    let allowed = legacy_count.saturating_mul(2).max(512);
    structured_count > allowed
}

fn legacy_embedding_chunk_estimate(content: &str, chunking: chunker::ChunkingOptions) -> usize {
    let sections = chunker::extract_sections(content);
    let mut count = 0usize;
    if chunking.include_level1 {
        count += sections
            .iter()
            .filter(|(_, section_text)| !section_text.trim().is_empty())
            .count();
    }
    if chunking.include_level2 {
        count += sections
            .iter()
            .filter(|(_, section_text)| !section_text.trim().is_empty())
            .map(|(_, section_text)| {
                chunker::chunk(section_text, chunking.chunk_size, chunking.overlap).len()
            })
            .sum::<usize>();
    }
    count.max(1)
}

#[derive(Debug)]
struct StructuredEmbeddingChunk {
    level: i32,
    text: String,
}

fn structured_node_is_indexable(node: &DocumentNode) -> bool {
    if node.text.trim().is_empty() {
        return false;
    }
    !matches!(
        node.kind,
        NodeKind::Title | NodeKind::Toc | NodeKind::HeaderFooter | NodeKind::FigureCaption
    )
}

fn structured_node_level(kind: NodeKind) -> i32 {
    match kind {
        NodeKind::Section | NodeKind::Procedure => 1,
        _ => 2,
    }
}

fn structured_outline_embedding_chunks(
    nodes: &[&DocumentNode],
    chunking: chunker::ChunkingOptions,
) -> Vec<StructuredEmbeddingChunk> {
    let mut chunks = Vec::new();
    let mut current: Option<StructuredChunkBuilder> = None;
    let target_size = chunking
        .chunk_size
        .max(crate::chunker::MIN_CONFIGURED_CHUNK_SIZE);

    for node in nodes {
        let key = StructuredChunkKey::from_node(node);
        if current.as_ref().is_some_and(|builder| {
            builder.key != key || builder.would_exceed(&node.text, target_size)
        }) {
            if let Some(done) = current.take().and_then(StructuredChunkBuilder::finish) {
                chunks.push(done);
            }
        }
        let builder = current.get_or_insert_with(|| StructuredChunkBuilder::new(key));
        builder.push(node);
    }

    if let Some(done) = current.and_then(StructuredChunkBuilder::finish) {
        chunks.push(done);
    }

    chunks
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StructuredChunkKey {
    kind: NodeKind,
    section_path: Vec<String>,
}

impl StructuredChunkKey {
    fn from_node(node: &DocumentNode) -> Self {
        Self {
            kind: structured_embedding_kind(node.kind),
            section_path: node.section_path.clone(),
        }
    }
}

#[derive(Debug)]
struct StructuredChunkBuilder {
    key: StructuredChunkKey,
    body: String,
}

impl StructuredChunkBuilder {
    fn new(key: StructuredChunkKey) -> Self {
        Self {
            key,
            body: String::new(),
        }
    }

    fn would_exceed(&self, next_line: &str, target_size: usize) -> bool {
        !self.body.is_empty() && self.body.len() + next_line.len() + 1 > target_size
    }

    fn push(&mut self, node: &DocumentNode) {
        if !self.body.is_empty() {
            self.body.push('\n');
        }
        self.body.push_str(node.text.trim());
    }

    fn finish(self) -> Option<StructuredEmbeddingChunk> {
        if self.body.trim().is_empty() {
            return None;
        }
        let section = if self.key.section_path.is_empty() {
            String::new()
        } else {
            format!("[section: {}]\n", self.key.section_path.join(" > "))
        };
        Some(StructuredEmbeddingChunk {
            level: structured_node_level(self.key.kind),
            text: format!("[kind: {:?}]\n{section}{}", self.key.kind, self.body),
        })
    }
}

fn structured_embedding_kind(kind: NodeKind) -> NodeKind {
    match kind {
        NodeKind::Title | NodeKind::Toc | NodeKind::HeaderFooter | NodeKind::FigureCaption => {
            NodeKind::Paragraph
        }
        NodeKind::Section => NodeKind::Section,
        NodeKind::ApiReference => NodeKind::ApiReference,
        NodeKind::Procedure | NodeKind::ProcedureStep => NodeKind::ProcedureStep,
        NodeKind::CodeBlock | NodeKind::CommandBlock => NodeKind::CommandBlock,
        NodeKind::ConfigBlock => NodeKind::ConfigBlock,
        NodeKind::Troubleshooting => NodeKind::Troubleshooting,
        NodeKind::Table | NodeKind::TableRow | NodeKind::Paragraph => NodeKind::Paragraph,
    }
}

/// 把一份 `RawDocument` 走完统一五步（Inserted / Duplicate / Skipped 三态）。
///
/// `dek` 是 vault 数据加密密钥。caller 必须已确认 vault 处于 Unlocked。
/// Updated 检测（增量判断 + 旧 item 软删）由 caller 在调用前完成，
/// 检测到变更时改调 `ingest_document_replacing`。
pub fn ingest_document(store: &Store, dek: &Key32, raw: &RawDocument) -> Result<IngestOutcome> {
    ingest_document_inner(store, dek, raw, None, &IngestOptions::default())
}

/// 带已知 `old_item_id` 的入库函数。caller 在调用前已自行完成旧 item 删除 +
/// purge 入队，此处直接走新文档五步并将 old_item_id 透传到 Updated 态结果。
pub fn ingest_document_replacing(
    store: &Store,
    dek: &Key32,
    raw: &RawDocument,
    old_item_id: &str,
) -> Result<IngestOutcome> {
    ingest_document_inner(
        store,
        dek,
        raw,
        Some(old_item_id.to_string()),
        &IngestOptions::default(),
    )
}

/// 带 OCR profile 的入库入口。扫描版 PDF / 图片上传时由 caller 传 profile id
/// （contract / receipt / screenshot / ancient / custom），None = 默认 300 DPI。
pub fn ingest_document_with_profile(
    store: &Store,
    dek: &Key32,
    raw: &RawDocument,
    profile: Option<&str>,
) -> Result<IngestOutcome> {
    ingest_document_inner(store, dek, raw, None, &IngestOptions::with_profile(profile))
}

pub fn ingest_document_with_options(
    store: &Store,
    dek: &Key32,
    raw: &RawDocument,
    options: &IngestOptions,
) -> Result<IngestOutcome> {
    ingest_document_inner(store, dek, raw, None, options)
}

pub fn ingest_document_replacing_with_options(
    store: &Store,
    dek: &Key32,
    raw: &RawDocument,
    old_item_id: &str,
    options: &IngestOptions,
) -> Result<IngestOutcome> {
    ingest_document_inner(store, dek, raw, Some(old_item_id.to_string()), options)
}

fn ingest_document_inner(
    store: &Store,
    dek: &Key32,
    raw: &RawDocument,
    old_item_id: Option<String>,
    options: &IngestOptions,
) -> Result<IngestOutcome> {
    // 1. parse — server 路径通过 scheduler 承接 OCR/ASR；纯文本/结构化解析仍在 core 内完成。
    let filename = raw.parse_filename();
    let (parsed_title, content, degraded_reason) =
        match parser::parse_bytes_with_options_detailed(&raw.content, &filename, &options.parser) {
            Ok(parsed)
                if parsed.content.trim().is_empty() && metadata_fallback_allowed(&filename) =>
            {
                let (fallback_reason, degraded_reason) =
                    empty_parse_fallback_quality(parsed.quality);
                let confirmed_no_text = degraded_reason.is_none();
                log::warn!(
                "ingest: parser returned empty content for {}; inserting metadata-only item: {}",
                raw.source_ref,
                fallback_reason
            );
                (
                    fallback_title_from_filename(&filename),
                    metadata_only_content(
                        raw,
                        &filename,
                        Some(&fallback_reason),
                        confirmed_no_text,
                    ),
                    degraded_reason,
                )
            }
            Ok(parsed) => {
                let degraded_reason = match parsed.quality {
                    parser::ParseQuality::Complete
                    | parser::ParseQuality::CompleteNoText { .. } => None,
                    parser::ParseQuality::RetryableDegraded { reason } => Some(reason),
                };
                (parsed.title, parsed.content, degraded_reason)
            }
            Err(e) if metadata_fallback_allowed(&filename) => {
                let reason = e.to_string();
                log::warn!(
                    "ingest: parser failed for {}; inserting metadata-only item: {reason}",
                    raw.source_ref
                );
                (
                    fallback_title_from_filename(&filename),
                    metadata_only_content(raw, &filename, Some(&reason), false),
                    Some(reason),
                )
            }
            Err(e) => return Err(e),
        };
    if content.trim().is_empty() {
        return Ok(IngestOutcome::Skipped {
            reason: "empty content after parse".into(),
        });
    }
    // 源给的 title 优先，缺失时用 parser 提取的兜底。对本地文件/附件，把
    // source_ref 文件名并入标题，让检索能利用机型、手册号、章节号等路径元数据
    // (e.g. 320FCOM3.pdf, A320-Hydraulic.pdf)，避免扫描件 OCR 后只剩通用标题。
    let title = if raw.title.trim().is_empty() {
        title_with_source_ref(&parsed_title, &raw.source_ref)
    } else {
        raw.title.clone()
    };

    // 2. content_hash 短路判重（仅非 replacing 路径）。
    // replacing 路径下 caller 已软删 old_item（is_deleted=1），find_item_by_content_hash
    // 带 AND is_deleted=0 查不到它；replacing 语义是"用 doc_v2 替换 old_item"，
    // 即便 doc_v2 内容与第三方 item 的 hash 碰撞也应插入独立新 item，所以整个短路跳过。
    let content_hash = compute_content_hash(&content);
    if old_item_id.is_none() {
        if let Some(existing_id) = store.find_item_by_content_hash(&content_hash)? {
            if let Some(reason) = degraded_reason {
                return Ok(IngestOutcome::Degraded {
                    item_id: existing_id,
                    chunks_enqueued: 0,
                    reason,
                });
            }
            return Ok(IngestOutcome::Duplicate {
                item_id: existing_id,
            });
        }
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
    //     corpus_domain 启用时给每个 chunk_text 注入 `[领域: X] ` 前缀，让 embedding
    //     空间把同领域文档聚集、缓解跨域污染。
    let chunk_counter = enqueue_content_embeddings(
        store,
        &item_id,
        &content,
        active_corpus_domain,
        options.chunking,
    )?;

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

    // doc_create 信号喂 skill_evolution，传文档名作 query context（失败静默，不阻塞）
    let _ = store.record_signal_event("doc_create", &item_id, Some(&filename));

    // Internal knowledge linker — 🆓/⚡ tier only (entity overlap + URL/title ref).
    // Per spec §2.4 "Alternative for shared_entity / explicit_ref only": vectors=None,
    // semantic_near pass runs later in the embed worker when chunk vectors exist.
    // 失败静默不阻塞入库 — link 缺失只是降级，文档已可被搜到 / 被读到。
    if let Err(e) = crate::linker::compute_links_for_item(
        store,
        None,
        &item_id,
        &title,
        &content,
        Some(&raw.uri),
        &crate::linker::LinkThresholds::default(),
    ) {
        log::warn!("ingest: linker (entity+ref pass) failed for {item_id}: {e}");
    }

    if let Some(reason) = degraded_reason {
        return Ok(IngestOutcome::Degraded {
            item_id,
            chunks_enqueued: chunk_counter,
            reason,
        });
    }

    match old_item_id {
        Some(old) => Ok(IngestOutcome::Updated {
            item_id,
            old_item_id: old,
        }),
        None => Ok(IngestOutcome::Inserted {
            item_id,
            chunks_enqueued: chunk_counter,
        }),
    }
}

fn title_with_source_ref(parsed_title: &str, source_ref: &str) -> String {
    let parsed = parsed_title.trim();
    let source_stem = Path::new(source_ref)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match (source_stem, parsed.is_empty()) {
        (Some(stem), false) if !parsed.eq_ignore_ascii_case(stem) => {
            format!("{stem} - {parsed}")
        }
        (Some(stem), _) => stem.to_string(),
        (None, false) => parsed.to_string(),
        (None, true) => source_ref.to_string(),
    }
}

fn metadata_fallback_allowed(filename: &str) -> bool {
    if !metadata_fallback_enabled() {
        return false;
    }
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "pdf"
            | "docx"
            | "pptx"
            | "xlsx"
            | "xls"
            | "epub"
            | "rtf"
            | "png"
            | "jpg"
            | "jpeg"
            | "webp"
            | "bmp"
            | "tiff"
            | "tif"
            | "gif"
            | "mp3"
            | "wav"
            | "m4a"
            | "flac"
            | "ogg"
            | "aac"
            | "opus"
            | "wma"
    )
}

fn metadata_fallback_enabled() -> bool {
    for key in [
        "ATTUNE_INGEST_METADATA_FALLBACK",
        "ATTUNE_PARSE_METADATA_FALLBACK",
    ] {
        if let Ok(value) = std::env::var(key) {
            return matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            );
        }
    }
    true
}

fn empty_parse_fallback_quality(quality: parser::ParseQuality) -> (String, Option<String>) {
    match quality {
        parser::ParseQuality::CompleteNoText { reason } => (reason, None),
        parser::ParseQuality::RetryableDegraded { reason } => (reason.clone(), Some(reason)),
        parser::ParseQuality::Complete => {
            let reason = "empty content after parse".to_string();
            (reason.clone(), Some(reason))
        }
    }
}

fn fallback_title_from_filename(filename: &str) -> String {
    Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(filename)
        .to_string()
}

fn metadata_only_content(
    raw: &RawDocument,
    filename: &str,
    reason: Option<&str>,
    confirmed_no_text: bool,
) -> String {
    let title = fallback_title_from_filename(filename);
    let source_ref = truncate_chars(&raw.source_ref, 512);
    let uri = truncate_chars(&raw.uri, 512);
    let terms = source_terms(&raw.source_ref, filename);
    let reason = reason.unwrap_or("parser unavailable");
    let status = if confirmed_no_text {
        "Document parsing status: complete metadata-only record.\n\
         Text extraction and OCR completed, and no searchable text was present. \
         This item can be used for source lookup and citation."
    } else {
        "Document parsing status: metadata-only fallback.\n\
         Full text extraction, OCR, or ASR did not produce usable content. \
         This item can be used for source lookup and citation, but detailed \
         content answers require successful text extraction."
    };
    format!(
        "# {title}\n\n\
         {status}\n\n\
         File name: {filename}\n\
         Source path: {source_ref}\n\
         URI: {uri}\n\
         File size bytes: {}\n\
         Source terms: {terms}\n\
         Parse status: {reason}\n",
        raw.content.len()
    )
}

fn source_terms(source_ref: &str, filename: &str) -> String {
    let mut terms = Vec::new();
    for value in [source_ref, filename] {
        let normalized: String = value
            .chars()
            .map(|ch| if ch.is_alphanumeric() { ch } else { ' ' })
            .collect();
        let collapsed = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
        if !collapsed.is_empty() && !terms.iter().any(|t| t == &collapsed) {
            terms.push(collapsed);
        }
    }
    terms.join(" | ")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        empty_parse_fallback_quality, locate_chunk_span, metadata_only_content,
        title_with_source_ref, IngestOutcome,
    };
    use crate::ingest::{RawDocument, SourceKind};
    use crate::parser::ParseQuality;
    use std::collections::HashMap;

    // ── IngestOutcome derive trait tests ────────────────────────────────────
    // These verify the #[derive(Debug, Clone, PartialEq, Eq)] bounds that
    // callers rely on for matching / asserting outcomes without SQLite.

    #[test]
    fn title_with_source_ref_prefixes_file_stem() {
        assert_eq!(
            title_with_source_ref(
                "Flight Crew Operating Manual",
                "/kb/Airbus/A320/FCOM/320FCOM3.pdf"
            ),
            "320FCOM3 - Flight Crew Operating Manual"
        );
        assert_eq!(
            title_with_source_ref("A320-Hydraulic", "/kb/A320-Hydraulic.pdf"),
            "A320-Hydraulic"
        );
    }

    #[test]
    fn confirmed_no_text_quality_does_not_request_a_retry_marker() {
        let reason = "all detected PDF pages were visually blank".to_string();
        let (fallback_reason, degraded_reason) =
            empty_parse_fallback_quality(ParseQuality::CompleteNoText {
                reason: reason.clone(),
            });
        assert_eq!(fallback_reason, reason);
        assert_eq!(degraded_reason, None);

        let (_, degraded_reason) = empty_parse_fallback_quality(ParseQuality::Complete);
        assert_eq!(
            degraded_reason.as_deref(),
            Some("empty content after parse")
        );
    }

    #[test]
    fn confirmed_no_text_metadata_does_not_claim_extraction_failed() {
        let raw = RawDocument {
            uri: "file:///vault/blank.pdf".to_string(),
            title: String::new(),
            content: Vec::new(),
            mime_hint: Some("application/pdf".to_string()),
            source_kind: SourceKind::LocalFolder,
            source_ref: "/vault/blank.pdf".to_string(),
            modified_marker: Some("source-sha".to_string()),
            domain: None,
            tags: None,
            corpus_domain: None,
            metadata: HashMap::new(),
        };
        let complete = metadata_only_content(
            &raw,
            "blank.pdf",
            Some("all pages were visually blank"),
            true,
        );
        assert!(complete.contains("complete metadata-only record"));
        assert!(complete.contains("no searchable text was present"));
        assert!(!complete.contains("did not produce usable content"));

        let degraded =
            metadata_only_content(&raw, "blank.pdf", Some("OCR backend unavailable"), false);
        assert!(degraded.contains("metadata-only fallback"));
        assert!(degraded.contains("did not produce usable content"));
    }

    #[test]
    fn ingest_outcome_inserted_equality_and_clone() {
        let a = IngestOutcome::Inserted {
            item_id: "id-1".into(),
            chunks_enqueued: 5,
        };
        let b = a.clone();
        assert_eq!(a, b);
        assert_ne!(
            a,
            IngestOutcome::Inserted {
                item_id: "id-2".into(),
                chunks_enqueued: 5
            }
        );
        assert_ne!(
            a,
            IngestOutcome::Inserted {
                item_id: "id-1".into(),
                chunks_enqueued: 6
            }
        );
    }

    #[test]
    fn ingest_outcome_duplicate_equality_and_clone() {
        let a = IngestOutcome::Duplicate {
            item_id: "dup-1".into(),
        };
        assert_eq!(a.clone(), a);
        assert_ne!(
            a,
            IngestOutcome::Duplicate {
                item_id: "dup-2".into()
            }
        );
    }

    #[test]
    fn ingest_outcome_updated_equality_and_clone() {
        let a = IngestOutcome::Updated {
            item_id: "new-1".into(),
            old_item_id: "old-1".into(),
        };
        assert_eq!(a.clone(), a);
        assert_ne!(
            a,
            IngestOutcome::Updated {
                item_id: "new-1".into(),
                old_item_id: "old-2".into()
            }
        );
    }

    #[test]
    fn ingest_outcome_skipped_equality_and_clone() {
        let a = IngestOutcome::Skipped {
            reason: "empty content after parse".into(),
        };
        assert_eq!(a.clone(), a);
        assert_ne!(
            a,
            IngestOutcome::Skipped {
                reason: "other reason".into()
            }
        );
    }

    #[test]
    fn ingest_outcome_variants_not_equal_across_kinds() {
        // Guard: different variants never compare equal even when fields look similar.
        let inserted = IngestOutcome::Inserted {
            item_id: "x".into(),
            chunks_enqueued: 0,
        };
        let duplicate = IngestOutcome::Duplicate {
            item_id: "x".into(),
        };
        let skipped = IngestOutcome::Skipped { reason: "x".into() };
        assert_ne!(inserted, duplicate);
        assert_ne!(duplicate, skipped);
    }

    #[test]
    fn ingest_outcome_debug_contains_variant_name() {
        // Callers use {:?} in panic messages — ensure Debug is implemented.
        let inserted = IngestOutcome::Inserted {
            item_id: "abc".into(),
            chunks_enqueued: 3,
        };
        let dbg = format!("{inserted:?}");
        assert!(
            dbg.contains("Inserted"),
            "Debug must show variant name: {dbg}"
        );
        assert!(
            dbg.contains("abc"),
            "Debug must include field values: {dbg}"
        );

        let dup = IngestOutcome::Duplicate {
            item_id: "dup".into(),
        };
        assert!(format!("{dup:?}").contains("Duplicate"));

        let upd = IngestOutcome::Updated {
            item_id: "n".into(),
            old_item_id: "o".into(),
        };
        assert!(format!("{upd:?}").contains("Updated"));

        let skip = IngestOutcome::Skipped {
            reason: "empty content after parse".into(),
        };
        assert!(format!("{skip:?}").contains("Skipped"));
    }

    #[test]
    fn locate_chunk_span_cursor_stays_on_char_boundary() {
        let content = "前缀 • 项目\n第二段 • TARGET\n第三段";
        let mut cursor = content.find('•').unwrap() + 1;
        let span = locate_chunk_span(content, "第二段 • TARGET", &mut cursor).unwrap();
        assert_eq!(&content[span.0..span.1], "第二段 • TARGET");
        assert!(content.is_char_boundary(cursor));
    }
}
