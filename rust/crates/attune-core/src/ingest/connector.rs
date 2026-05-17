use std::collections::HashMap;

use crate::error::Result;

/// 采集源类别。决定入库 item 的 `source_type` 与去重策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// 本地文件夹（folder watcher / 手动 bind）。
    LocalFolder,
    /// WebDAV 远程目录（Nextcloud / 群晖 / Apache mod_dav）。
    WebDav,
    /// IMAP 邮箱。
    Email,
    /// RSS / Atom 订阅。
    Rss,
    /// 云盘（经 rclone 桥接：Google Drive / Dropbox / OneDrive 等）。
    CloudDrive,
}

impl SourceKind {
    /// 稳定字符串标识，写入 DB / 日志 / 信号。新增 variant 必须同步加分支。
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceKind::LocalFolder => "local_folder",
            SourceKind::WebDav => "webdav",
            SourceKind::Email => "email",
            SourceKind::Rss => "rss",
            SourceKind::CloudDrive => "cloud_drive",
        }
    }

    /// 入库 `items.source_type` 字段值。当前全部归一到 `"file"` 以兼容现有
    /// 检索 / 分类逻辑（它们按 source_type 做加权）；未来如需按源细分再扩展。
    pub fn item_source_type(&self) -> &'static str {
        "file"
    }
}

/// 从某个源拿到的一份未入库原始文档。
///
/// `content` 是**原始字节**（未解析）—— `ingest_document` 内部用
/// [`crate::parser::parse_bytes`] 解析。`modified_marker` 用于增量判断：
/// 本地文件 = SHA-256 / mtime；WebDAV = ETag；邮箱 = UID；RSS = entry id。
/// caller 用它和 `Store::get_indexed_file` 里存的 `file_hash` 比对决定是否跳过。
#[derive(Debug, Clone)]
pub struct RawDocument {
    /// 全局唯一资源定位符（`file:///…` / `https://…` / `imap://…/INBOX/123`）。
    pub uri: String,
    /// 源给出的标题；为空时 `ingest_document` 会用 parser 提取的标题兜底。
    pub title: String,
    /// 原始字节。
    pub content: Vec<u8>,
    /// MIME 提示（源若已知）。当前 parser 主要按文件名扩展名判别，此字段预留。
    pub mime_hint: Option<String>,
    /// 源类别。
    pub source_kind: SourceKind,
    /// 在该源内的稳定引用键，用于 `indexed_files` 去重。
    /// 本地 = 绝对路径；WebDAV = href；邮箱 = Message-ID；RSS = entry link。
    pub source_ref: String,
    /// 增量标记（见结构体文档）。`None` = 该源无增量信息，每次都重新入库。
    pub modified_marker: Option<String>,
    /// 网站域名 / 来源域（来自 Chrome 扩展 `ingest` 时携带的 `domain`）。
    /// 一等字段，`ingest_document` 直接透传给 `Store::insert_item` 第 6 参数。
    /// 非 `/api/v1/ingest` 源（local / upload / webdav / email / rss）传 `None`。
    pub domain: Option<String>,
    /// 用户标签（来自 `ingest` 时携带的 `tags`）。一等字段，`ingest_document`
    /// 直接透传给 `Store::insert_item` 第 7 参数。非 ingest 源传 `None`。
    pub tags: Option<Vec<String>>,
    /// 语料领域分类（`legal` / `tech` / `medical` / `patent` / `general`）。
    /// 对应 `items.corpus_domain`。`Some(d)` 且 `d != "general"` 时，
    /// `ingest_document` 会给每个 chunk_text 注入 `[领域: d] ` 前缀
    /// （v0.6 F-Pro 跨域防污染，bge-m3 corpus tagging）并调
    /// `set_item_corpus_domain`。本地文件夹源从 `Store::get_dir_corpus_domain`
    /// 读取放入；其它源（webdav / email / rss / cloud）传 `None`。
    pub corpus_domain: Option<String>,
    /// 源特定的额外元数据（邮件发件人 / RSS 频道名等），按需消费。
    pub metadata: HashMap<String, String>,
}

impl RawDocument {
    /// 用于 `parser::parse_bytes` 的文件名 —— 取 `source_ref` 末段，
    /// parser 据此扩展名选解析器。无扩展名时 parser 走纯文本分支。
    pub fn parse_filename(&self) -> String {
        self.source_ref
            .rsplit(['/', '\\'])
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.source_ref)
            .to_string()
    }
}

/// 文档回调 sink。`SourceConnector` 每产出一份 `RawDocument` 就调一次。
/// 用回调而非返回 `Vec<RawDocument>`：大邮箱 / 大目录一次性物化会爆内存。
pub type DocumentSink<'a> = Box<dyn FnMut(RawDocument) + 'a>;

/// 一个采集源。实现者负责枚举自己的内容并通过 `sink` 逐个交出。
pub trait SourceConnector {
    /// 该源的类别。
    fn source_kind(&self) -> SourceKind;

    /// 枚举源内文档，每份通过 `sink` 交出。实现者**不**做入库 —— 入库由
    /// 调用方对每份 `RawDocument` 调 [`crate::ingest::ingest_document`] 完成。
    /// 单份文档的可恢复错误（解析失败 / 下载失败）应由实现者吞掉并记日志、
    /// 继续下一份；只有源级致命错误（无法连接 / 鉴权失败）才返回 `Err`。
    fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_document_construct_and_read_fields() {
        let doc = RawDocument {
            uri: "file:///home/u/notes/a.md".into(),
            title: "A Note".into(),
            content: b"# A Note\n\nbody".to_vec(),
            mime_hint: Some("text/markdown".into()),
            source_kind: SourceKind::LocalFolder,
            source_ref: "/home/u/notes/a.md".into(),
            modified_marker: Some("abc123".into()),
            domain: Some("example.com".into()),
            tags: Some(vec!["note".into(), "draft".into()]),
            corpus_domain: Some("legal".into()),
            metadata: std::collections::HashMap::new(),
        };
        assert_eq!(doc.source_kind, SourceKind::LocalFolder);
        assert_eq!(doc.source_ref, "/home/u/notes/a.md");
        assert_eq!(doc.content, b"# A Note\n\nbody");
        assert_eq!(doc.domain.as_deref(), Some("example.com"));
        assert_eq!(doc.tags.as_ref().unwrap().len(), 2);
        assert_eq!(doc.corpus_domain.as_deref(), Some("legal"));
    }

    #[test]
    fn source_kind_as_str_round_trips() {
        for k in [
            SourceKind::LocalFolder,
            SourceKind::WebDav,
            SourceKind::Email,
            SourceKind::Rss,
            SourceKind::CloudDrive,
        ] {
            assert!(!k.as_str().is_empty());
        }
        assert_eq!(SourceKind::WebDav.as_str(), "webdav");
    }

    #[test]
    fn connector_drives_sink_callback() {
        struct TwoDocConnector;
        impl SourceConnector for TwoDocConnector {
            fn source_kind(&self) -> SourceKind {
                SourceKind::LocalFolder
            }
            fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> crate::error::Result<()> {
                for i in 0..2 {
                    sink(RawDocument {
                        uri: format!("mem://{i}"),
                        title: format!("doc {i}"),
                        content: b"x".to_vec(),
                        mime_hint: None,
                        source_kind: SourceKind::LocalFolder,
                        source_ref: format!("mem://{i}"),
                        modified_marker: None,
                        domain: None,
                        tags: None,
                        corpus_domain: None,
                        metadata: std::collections::HashMap::new(),
                    });
                }
                Ok(())
            }
        }
        let mut count = 0usize;
        {
            let mut sink: DocumentSink<'_> = Box::new(|_doc: RawDocument| count += 1);
            TwoDocConnector.fetch_documents(&mut sink).unwrap();
        }
        assert_eq!(count, 2);
    }
}
