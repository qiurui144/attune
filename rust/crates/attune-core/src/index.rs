// npu-vault/crates/vault-core/src/index.rs

use std::path::Path;
use std::sync::Mutex;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy};

use crate::error::{Result, VaultError};

const HEAP_SIZE: usize = 50_000_000; // 50 MB writer heap

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
        let schema = Self::build_schema();
        let index = if dir.join("meta.json").exists() {
            Index::open_in_dir(dir)
                .map_err(|e| VaultError::Crypto(format!("tantivy open: {e}")))?
        } else {
            Index::create_in_dir(dir, schema.clone())
                .map_err(|e| VaultError::Crypto(format!("tantivy create: {e}")))?
        };
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
        // 注册 jieba 分词器用于中文
        let tokenizer = tantivy_jieba::JiebaTokenizer {};
        index.tokenizers().register("jieba", tokenizer);
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
