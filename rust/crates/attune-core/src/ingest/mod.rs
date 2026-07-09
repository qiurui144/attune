//! ingest — 统一采集抽象。
//!
//! 一个「源」（本地文件夹 / WebDAV / 邮箱 / RSS / 云盘）实现 [`SourceConnector`]，
//! 把自己的内容逐个交成 [`RawDocument`]；[`ingest_document`] 是唯一入库函数，
//! 走完 parse → 判重 → insert → breadcrumbs → embed(L1+L2) → classify 五步。
//! 各源不再各自复制 pipeline。

mod connector;
pub mod email;
pub mod git;
pub mod local;
mod pipeline;
pub mod rss;

pub use connector::{DocumentSink, RawDocument, SourceConnector, SourceKind};
pub use email::{
    EmailConfig, EmailConnector, FetchedMail, ImapFetcher, MailAttachment, MailMessage,
};
pub use git::GitSourceConfig;
pub use pipeline::{
    enqueue_content_embeddings, ingest_document, ingest_document_replacing,
    ingest_document_replacing_with_options, ingest_document_with_options,
    ingest_document_with_profile, IngestOptions, IngestOutcome,
};
pub use rss::{
    parse_feed_bytes, FeedFetcher, FeedHttpResponse, ParsedRssEntry, RealFeedFetcher, RssConnector,
    RssFeedFetch,
};
