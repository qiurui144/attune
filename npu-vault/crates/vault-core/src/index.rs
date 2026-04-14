// npu-vault/crates/vault-core/src/index.rs

use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter, ReloadPolicy};

use crate::error::{Result, VaultError};

const HEAP_SIZE: usize = 50_000_000; // 50 MB writer heap

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
}

impl FulltextIndex {
    /// 创建内存索引（测试用）
    pub fn open_memory() -> Result<Self> {
        let schema = Self::build_schema();
        let index = Index::create_in_ram(schema.clone());
        Self::register_tokenizers(&index);
        let f_item_id = schema.get_field("item_id").unwrap();
        let f_title = schema.get_field("title").unwrap();
        let f_content = schema.get_field("content").unwrap();
        let f_source_type = schema.get_field("source_type").unwrap();
        Ok(Self { index, schema, f_item_id, f_title, f_content, f_source_type })
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
        let f_item_id = schema.get_field("item_id").unwrap();
        let f_title = schema.get_field("title").unwrap();
        let f_content = schema.get_field("content").unwrap();
        let f_source_type = schema.get_field("source_type").unwrap();
        Ok(Self { index, schema, f_item_id, f_title, f_content, f_source_type })
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

    /// 添加文档到索引（upsert 语义：先删除同 item_id 的旧文档再添加）
    pub fn add_document(&self, item_id: &str, title: &str, content: &str, source_type: &str) -> Result<()> {
        let mut writer: IndexWriter = self.index.writer(HEAP_SIZE)
            .map_err(|e| VaultError::Crypto(format!("tantivy writer: {e}")))?;
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
        // Wait for merging threads to finish to ensure segments are available
        writer.wait_merging_threads()
            .map_err(|e| VaultError::Crypto(format!("tantivy wait: {e}")))?;
        Ok(())
    }

    /// 删除文档（by item_id）
    pub fn delete_document(&self, item_id: &str) -> Result<()> {
        let mut writer: IndexWriter = self.index.writer(HEAP_SIZE)
            .map_err(|e| VaultError::Crypto(format!("tantivy writer: {e}")))?;
        let term = tantivy::Term::from_field_text(self.f_item_id, item_id);
        writer.delete_term(term);
        writer.commit()
            .map_err(|e| VaultError::Crypto(format!("tantivy commit: {e}")))?;
        Ok(())
    }

    /// BM25 搜索 → Vec<(item_id, score)>
    pub fn search(&self, query_str: &str, top_k: usize) -> Result<Vec<(String, f32)>> {
        let reader = self.index.reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|e| VaultError::Crypto(format!("tantivy reader: {e}")))?;
        let searcher = reader.searcher();

        let query_parser = QueryParser::for_index(&self.index, vec![self.f_title, self.f_content]);
        let query = query_parser.parse_query(query_str)
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
        let reader = self.index.reader()
            .map_err(|e| VaultError::Crypto(format!("tantivy reader: {e}")))?;
        Ok(reader.searcher().num_docs() as usize)
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
}
