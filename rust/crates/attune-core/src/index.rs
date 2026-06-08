// npu-vault/crates/vault-core/src/index.rs

use std::path::Path;
use std::sync::Mutex;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy};

use crate::error::{Result, VaultError};

const HEAP_SIZE: usize = 50_000_000; // 50 MB writer heap

/// 分词器版本标记。改变 analyzer 链（jieba / LowerCaser / Stemmer 组合）时 +1。
///
/// 为什么需要：tantivy 的 meta.json 只持久化字段引用的分词器**名字**（"jieba"），
/// 不持久化运行期注册的 analyzer 链。若我们升级 analyzer（如加 LowerCaser），旧
/// 磁盘段里的 token 仍是旧规则切出来的（大小写敏感 / 未词干化），用新 analyzer 去
/// 查会产生不一致命中。本标记在 open() 时比对，一旦不符就清空索引目录强制重建，
/// 让 unlock 时的全量 rebuild（state.rs 从加密 SQL 重灌全部 item）用新 analyzer 落盘。
///
/// v1: bare jieba（≤ v1.2）
/// v2: jieba → LowerCaser → English Stemmer（2026-06-08 多语言分词强化）
const TOKENIZER_VERSION: u32 = 2;
const TOKENIZER_VERSION_FILE: &str = "tokenizer_version";

/// FulltextIndex 持久持有唯一 IndexWriter，避免多线程并发重复创建 writer 导致 panic。
/// Tantivy 规定：同一 Index 同时只能有一个活跃 IndexWriter；
/// 用 Mutex<IndexWriter> 保护，所有写操作共享该 writer。
pub struct FulltextIndex {
    index: Index,
    #[allow(dead_code)]
    schema: Schema,
    // field handles
    f_item_id: Field,
    f_title: Field,
    f_content: Field,
    #[allow(dead_code)]
    f_source_type: Field,
    writer: Mutex<IndexWriter>,
    // OSS-S13 P0 fix: IndexReader 一次创建并复用，避免每次 search() 重新分配段读取器导致的并发态内存泄漏
    reader: IndexReader,
}

impl FulltextIndex {
    /// 创建内存索引（测试用）
    pub fn open_memory() -> Result<Self> {
        let schema = Self::build_schema();
        let index = Index::create_in_ram(schema.clone());
        Self::register_tokenizers(&index);
        let f_item_id = schema.get_field("item_id").expect("schema field 'item_id' defined in build_schema");
        let f_title = schema.get_field("title").expect("schema field 'title' defined in build_schema");
        let f_content = schema.get_field("content").expect("schema field 'content' defined in build_schema");
        let f_source_type = schema.get_field("source_type").expect("schema field 'source_type' defined in build_schema");
        let writer = index.writer(HEAP_SIZE)
            .map_err(|e| VaultError::Crypto(format!("tantivy writer: {e}")))?;
        let reader = index.reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e| VaultError::Crypto(format!("tantivy reader: {e}")))?;
        Ok(Self { index, schema, f_item_id, f_title, f_content, f_source_type, writer: Mutex::new(writer), reader })
    }

    /// 打开持久化索引
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        // 分词器版本迁移：旧索引（缺标记或版本不符）的 token 用旧 analyzer 切出，
        // 与新 analyzer 不一致 → 清空目录强制重建。unlock 时 state.rs 会从加密 SQL
        // 全量重灌 item，用新 analyzer 重新落盘，保证一致。索引是派生缓存，清空安全
        // （SSOT 是加密 vault，不丢数据）。
        Self::migrate_tokenizer_version(dir)?;
        let schema = Self::build_schema();
        let index = if dir.join("meta.json").exists() {
            Index::open_in_dir(dir)
                .map_err(|e| VaultError::Crypto(format!("tantivy open: {e}")))?
        } else {
            Index::create_in_dir(dir, schema.clone())
                .map_err(|e| VaultError::Crypto(format!("tantivy create: {e}")))?
        };
        Self::register_tokenizers(&index);
        // 重建/创建成功后写当前版本标记（仅磁盘索引；内存索引不需要）。
        let _ = std::fs::write(
            dir.join(TOKENIZER_VERSION_FILE),
            TOKENIZER_VERSION.to_string(),
        );
        let f_item_id = schema.get_field("item_id").expect("schema field 'item_id' defined in build_schema");
        let f_title = schema.get_field("title").expect("schema field 'title' defined in build_schema");
        let f_content = schema.get_field("content").expect("schema field 'content' defined in build_schema");
        let f_source_type = schema.get_field("source_type").expect("schema field 'source_type' defined in build_schema");
        let writer = index.writer(HEAP_SIZE)
            .map_err(|e| VaultError::Crypto(format!("tantivy writer: {e}")))?;
        let reader = index.reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e| VaultError::Crypto(format!("tantivy reader: {e}")))?;
        Ok(Self { index, schema, f_item_id, f_title, f_content, f_source_type, writer: Mutex::new(writer), reader })
    }

    /// 检查磁盘索引的分词器版本标记；缺失或不符 → 删除整个索引目录内容，
    /// 让后续 open 走 create 路径（空索引），unlock 时全量重建。
    ///
    /// 触发条件：
    ///   - 已有 meta.json（旧索引）但无 tokenizer_version 文件 → v1 旧索引，需迁移
    ///   - tokenizer_version 文件存在但值 != TOKENIZER_VERSION → 跨版本，需迁移
    ///   - 无 meta.json（全新目录）→ 不需迁移（本就是空的）
    fn migrate_tokenizer_version(dir: &Path) -> Result<()> {
        let has_index = dir.join("meta.json").exists();
        if !has_index {
            return Ok(()); // 全新目录，create 路径会处理
        }
        let marker = dir.join(TOKENIZER_VERSION_FILE);
        let on_disk: Option<u32> = std::fs::read_to_string(&marker)
            .ok()
            .and_then(|s| s.trim().parse().ok());
        if on_disk == Some(TOKENIZER_VERSION) {
            return Ok(()); // 版本一致，无需迁移
        }
        // 不一致（含旧索引无标记的 None 情形）→ 清空目录内容。
        log::info!(
            "fulltext tokenizer version mismatch (disk={on_disk:?}, code={TOKENIZER_VERSION}); \
             wiping index dir to force rebuild with new analyzer"
        );
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
        Ok(())
    }

    fn build_schema() -> Schema {
        let mut builder = Schema::builder();
        let jieba_indexing = TextFieldIndexing::default()
            .set_tokenizer("jieba")
            .set_index_option(IndexRecordOption::WithFreqsAndPositions);
        let jieba_text = TextOptions::default()
            .set_indexing_options(jieba_indexing.clone());
        let jieba_text_stored = TextOptions::default()
            .set_indexing_options(jieba_indexing)
            .set_stored();

        builder.add_text_field("item_id", STRING | STORED);
        builder.add_text_field("title", jieba_text_stored);
        builder.add_text_field("content", jieba_text);
        builder.add_text_field("source_type", STRING | STORED);
        builder.build()
    }

    fn register_tokenizers(index: &Index) {
        // 多语言分词链：jieba 切词（中文 + 把英文按空格/标点切出）→ LowerCaser
        // （英文大小写不敏感）→ English Stemmer（running→run 等词干归并）。
        //
        // CJK 不受 LowerCaser / Stemmer 影响（无大小写、非英文词干），只有拉丁
        // token 被归一。index 与 query 共用此 analyzer（QueryParser 走字段的
        // "jieba" 分词器，tokenize_cjk_query 也取同一个），保证对称。
        use tantivy::tokenizer::{Language, LowerCaser, Stemmer, TextAnalyzer};
        let analyzer = TextAnalyzer::builder(tantivy_jieba::JiebaTokenizer {})
            .filter(LowerCaser)
            .filter(Stemmer::new(Language::English))
            .build();
        index.tokenizers().register("jieba", analyzer);
    }
}

/// 用 index 里注册的 jieba 分词器切中文 query，以空格拼接返回
///
/// 用途：绕过 QueryParser 对多字 CJK 的单 token 误判。
fn tokenize_cjk_query(index: &Index, q: &str) -> String {
    use tantivy::tokenizer::TokenStream;
    let mut tokenizer = match index.tokenizer_for_field(
        index.schema().get_field("content").expect("schema field 'content' defined in build_schema")
    ) {
        Ok(t) => t,
        Err(_) => return q.to_string(),
    };
    let mut stream = tokenizer.token_stream(q);
    let mut tokens: Vec<String> = Vec::new();
    while let Some(tok) = stream.next() {
        if !tok.text.trim().is_empty() {
            tokens.push(tok.text.clone());
        }
    }
    if tokens.is_empty() { q.to_string() } else { tokens.join(" ") }
}

impl FulltextIndex {

    /// 添加文档到索引（upsert 语义：先删除同 item_id 的旧文档再添加）
    pub fn add_document(&self, item_id: &str, title: &str, content: &str, source_type: &str) -> Result<()> {
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        // Delete existing document with same item_id (upsert semantics)
        let term = tantivy::Term::from_field_text(self.f_item_id, item_id);
        writer.delete_term(term);
        writer.add_document(doc!(
            self.f_item_id => item_id,
            self.f_title => title,
            self.f_content => content,
            self.f_source_type => source_type,
        )).map_err(|e| VaultError::Crypto(format!("tantivy add: {e}")))?;
        writer.commit()
            .map_err(|e| VaultError::Crypto(format!("tantivy commit: {e}")))?;
        // OSS-S13 P0 fix: 写后立即 reload 让全局 reader 看到新 commit，
        // 避免 OnCommitWithDelay 在测试 / 紧跟读场景下的延迟可见性
        self.reader.reload()
            .map_err(|e| VaultError::Crypto(format!("tantivy reload: {e}")))?;
        Ok(())
    }

    /// 删除文档（by item_id）
    pub fn delete_document(&self, item_id: &str) -> Result<()> {
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let term = tantivy::Term::from_field_text(self.f_item_id, item_id);
        writer.delete_term(term);
        writer.commit()
            .map_err(|e| VaultError::Crypto(format!("tantivy commit: {e}")))?;
        // OSS-S13 P0 fix: 写后立即 reload，同 add_document 注释
        self.reader.reload()
            .map_err(|e| VaultError::Crypto(format!("tantivy reload: {e}")))?;
        Ok(())
    }

    /// BM25 搜索 → Vec<(item_id, score)>
    ///
    /// 对中文 query 的特殊处理：
    ///   Tantivy 的 QueryParser 对多字 CJK 字符串可能当作一个整 token 处理，
    ///   不会调用字段的 jieba 分词器。结果："股东决议" 返回 0 命中，但
    ///   "股东 决议"（带空格）能命中。
    ///
    /// 解决：若 query 含中文字符，先用 jieba 分词，把每个 token 之间插入
    /// 空格再交给 QueryParser。QueryParser 默认是 should/OR 模式，任意
    /// token 命中即可返回，保证召回。
    pub fn search(&self, query_str: &str, top_k: usize) -> Result<Vec<(String, f32)>> {
        // 空查询直接返回：避免 tantivy AllQuery 全量扫描
        if query_str.trim().is_empty() {
            return Ok(vec![]);
        }
        // OSS-S13 P0 fix: 复用预创建的全局 IndexReader，OnCommitWithDelay 自动跟随 writer 提交刷新
        let searcher = self.reader.searcher();

        // 若含中文，先 jieba 分词再拼回空格分隔
        let effective_query = if query_str.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) {
            tokenize_cjk_query(&self.index, query_str)
        } else {
            query_str.to_string()
        };

        let query_parser = QueryParser::for_index(&self.index, vec![self.f_title, self.f_content]);
        let query = query_parser.parse_query(&effective_query)
            .map_err(|e| VaultError::Crypto(format!("tantivy query: {e}")))?;

        let top_docs = searcher.search(&query, &TopDocs::with_limit(top_k))
            .map_err(|e| VaultError::Crypto(format!("tantivy search: {e}")))?;

        let mut results = Vec::new();
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)
                .map_err(|e| VaultError::Crypto(format!("tantivy doc: {e}")))?;
            if let Some(item_id) = doc.get_first(self.f_item_id).and_then(|v| v.as_str()) {
                results.push((item_id.to_string(), score));
            }
        }
        Ok(results)
    }

    pub fn doc_count(&self) -> Result<usize> {
        // OSS-S13 P0 fix: 同样复用全局 reader 而非每次新建
        Ok(self.reader.searcher().num_docs() as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_memory_index() {
        let idx = FulltextIndex::open_memory().unwrap();
        assert_eq!(idx.doc_count().unwrap(), 0);
    }

    #[test]
    fn add_and_search() {
        let idx = FulltextIndex::open_memory().unwrap();
        idx.add_document("item1", "Rust编程", "Rust是一门系统编程语言", "note").unwrap();
        idx.add_document("item2", "Python学习", "Python是一门脚本语言", "note").unwrap();

        let results = idx.search("Rust", 10).unwrap();
        assert!(!results.is_empty(), "Should find Rust document");
        assert_eq!(results[0].0, "item1");
    }

    #[test]
    fn delete_document() {
        let idx = FulltextIndex::open_memory().unwrap();
        idx.add_document("item1", "Test", "Content", "note").unwrap();
        assert_eq!(idx.doc_count().unwrap(), 1);

        idx.delete_document("item1").unwrap();
        assert_eq!(idx.doc_count().unwrap(), 0);
    }

    #[test]
    fn persistent_index() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tantivy");

        // Create and add
        {
            let idx = FulltextIndex::open(&path).unwrap();
            idx.add_document("id1", "Title", "Content here", "note").unwrap();
        }
        // Reopen and verify
        {
            let idx = FulltextIndex::open(&path).unwrap();
            assert_eq!(idx.doc_count().unwrap(), 1);
            let results = idx.search("Content", 10).unwrap();
            assert!(!results.is_empty());
        }
    }

    /// 分词器版本迁移：缺失/不符的版本标记 → open 时清空旧索引强制重建。
    #[test]
    fn tokenizer_version_migration_wipes_stale_index() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ft");

        // 1) 建索引 + 加文档，open 会写当前版本标记。
        {
            let idx = FulltextIndex::open(&path).unwrap();
            idx.add_document("id1", "Title", "Running content", "note").unwrap();
            assert_eq!(idx.doc_count().unwrap(), 1);
        }
        assert!(path.join("meta.json").exists());
        assert!(path.join(super::TOKENIZER_VERSION_FILE).exists());

        // 2) 模拟旧索引：把版本标记改成旧值（或删掉），下次 open 必须清空重建。
        std::fs::write(path.join(super::TOKENIZER_VERSION_FILE), "1").unwrap();

        // 3) 重新 open：版本不符 → 清空 → 空索引（doc 丢失是预期，派生缓存由 unlock 重灌）。
        {
            let idx = FulltextIndex::open(&path).unwrap();
            assert_eq!(
                idx.doc_count().unwrap(),
                0,
                "stale-version index must be wiped on open (rebuilt by unlock)"
            );
            // 标记应已更新为当前版本。
            let v = std::fs::read_to_string(path.join(super::TOKENIZER_VERSION_FILE)).unwrap();
            assert_eq!(v.trim(), super::TOKENIZER_VERSION.to_string());
            // 重灌后可正常工作（多语言 analyzer 生效）。
            idx.add_document("id2", "T", "Running 检索", "note").unwrap();
            assert!(!idx.search("running", 10).unwrap().is_empty());
        }

        // 4) 再 open（版本已一致）→ 不清空，doc 保留。
        {
            let idx = FulltextIndex::open(&path).unwrap();
            assert_eq!(
                idx.doc_count().unwrap(),
                1,
                "matching-version index must NOT be wiped"
            );
        }
    }

    /// 旧索引无版本标记（v1.2 之前）→ open 视为 stale，清空重建。
    #[test]
    fn tokenizer_version_missing_marker_treated_as_stale() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ft");
        {
            let idx = FulltextIndex::open(&path).unwrap();
            idx.add_document("id1", "T", "content", "note").unwrap();
        }
        // 删除版本标记，模拟 v1.2 之前建的索引。
        std::fs::remove_file(path.join(super::TOKENIZER_VERSION_FILE)).unwrap();
        assert!(path.join("meta.json").exists());

        let idx = FulltextIndex::open(&path).unwrap();
        assert_eq!(idx.doc_count().unwrap(), 0, "marker-less index must be wiped");
    }

    /// 多语言分词：英文大小写不敏感（LowerCaser）+ 词干归并（Stemmer）+ CJK 仍走 jieba。
    ///
    /// 在 jieba 之后挂 LowerCaser/Stemmer 之前，"Running" 索引为大小写敏感的
    /// 原 token，搜 "running" 命不中；本测试钉死修复后行为。
    #[test]
    fn multilingual_tokenizer_lowercases_english_and_keeps_cjk() {
        let idx = FulltextIndex::open_memory().unwrap();
        idx.add_document("doc1", "项目 Running 测试", "向量 search 检索 hybrid recall", "note")
            .unwrap();

        // 1) 英文大小写不敏感：原文 "Running"（大写 R），搜小写 "running" 必须命中。
        let r = idx.search("running", 10).unwrap();
        assert!(
            r.iter().any(|(id, _)| id == "doc1"),
            "lowercase 'running' should hit 'Running' after LowerCaser"
        );

        // 2) 英文词干归并：搜 "run" 经 Stemmer 应命中 "running"（running→run）。
        let r = idx.search("run", 10).unwrap();
        assert!(
            r.iter().any(|(id, _)| id == "doc1"),
            "stemmed 'run' should hit 'Running' after English Stemmer"
        );

        // 3) CJK 仍正确分词：搜 "检索" 必须命中。
        let r = idx.search("检索", 10).unwrap();
        assert!(
            r.iter().any(|(id, _)| id == "doc1"),
            "CJK '检索' must still segment + hit via jieba"
        );
    }

    /// OSS-S13 P0 regression: 多次 search 应该复用同一个 IndexReader，不重新分配
    /// 修复前每次 search() 都会通过 reader_builder 重新构造 reader，并发态下导致内存泄漏。
    /// 修复后 reader 在 struct 字段上一次性创建，多次 search 应该指向同一对象。
    #[test]
    fn search_reuses_index_reader_oss_s13() {
        let idx = FulltextIndex::open_memory().unwrap();
        for i in 0..50 {
            idx.add_document(
                &format!("item{i}"),
                &format!("Title {i}"),
                "Rust 编程 trait closure",
                "note",
            ).unwrap();
        }
        // 1000 次 search — 修复前会反复 build IndexReader，修复后只复用 self.reader
        for _ in 0..1000 {
            let results = idx.search("Rust", 5).unwrap();
            assert!(!results.is_empty(), "Search should return results");
        }
        // 直接断言 doc_count 也走 cached reader
        assert_eq!(idx.doc_count().unwrap(), 50);
    }
}
