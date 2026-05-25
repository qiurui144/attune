//! RSS / Atom 采集源。
//!
//! 第三个 `SourceConnector` 实现（继 Email/WebDAV 之后）。
//!
//! 设计要点：
//!
//! 1. **HTTP 条件 GET**：每次 poll 用 `If-None-Match: <etag>` + `If-Modified-Since:
//!    <last_modified>` 头，server 返回 304 时整箱跳过；只在 200 OK 时下载 + 解析。
//! 2. **entry 级 fallback dedup**：很多 RSS 站不支持条件 GET（始终回 200），靠
//!    `last_entry_guid` 在 entry 层去重 —— 只 emit GUID/link 严格"新"于上次 cursor
//!    的条目。`ingest_document` 内部的 content_hash 短路是第三层防护。
//! 3. **网络错误不抛**：单 feed 网络故障 → 记日志 + caller 调用方负责
//!    `touch_polled_at` 防 tight-loop；不阻塞整个 worker。
//! 4. **HTML body 转纯文本**：复用 `ingest/email.rs::html_to_text`（同样剥
//!    `<script>` / `<style>` 块），不引第二个 HTML 解析器。
//! 5. **不实现 IMAP 邮件列表订阅**：开源项目邮件列表（LWN / lkml.org）多数发布
//!    web RSS 镜像 —— 用户订阅 RSS 即可。用户已订阅了 IMAP 邮件列表的，由
//!    EmailConnector 走 IMAP 路径，不重复支持。

use std::collections::HashMap;

use crate::error::{Result, VaultError};
use crate::ingest::email::html_to_text;
use crate::ingest::{DocumentSink, RawDocument, SourceConnector, SourceKind};

/// 单 feed 的 fetch 输入。worker 从 store 行物化后注入。
#[derive(Debug, Clone)]
pub struct RssFeedFetch {
    /// Feed 数据库 id（用于 ingest_meta + 日志）。
    pub feed_id: String,
    /// 展示名（用于日志，可空）。
    pub feed_name: String,
    /// 订阅 URL。
    pub url: String,
    /// 上次成功 ingest 的最末 entry guid/link，entry 级 dedup 用。
    pub last_entry_guid: Option<String>,
    /// 上次 server 返回的 ETag，条件 GET 用。
    pub etag: Option<String>,
    /// 上次 server 返回的 Last-Modified 原始字符串，条件 GET 用。
    pub last_modified: Option<String>,
}

/// HTTP 条件 GET 的响应（解析后供 caller 决定如何持久化）。
#[derive(Debug, Clone)]
pub enum FeedHttpResponse {
    /// 304 Not Modified —— body 为空，仅推进 last_polled_at。
    NotModified,
    /// 200 OK —— 含 feed 字节 + 新的 ETag / Last-Modified（如 server 给的话）。
    Ok {
        body: Vec<u8>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
}

/// HTTP 抓取层抽象 —— 与 `ImapFetcher` 同模式，让 `RssConnector` 离线可测：
/// 单元测试注入 mock 直接喂解析层；集成测试用 `RealFeedFetcher` 走 reqwest。
pub trait FeedFetcher: Send + Sync {
    /// 条件 GET 拉一个 feed。`etag` / `last_modified` 来自上次成功响应。
    /// 网络层错误（DNS / 连接 / 超时 / 5xx）应作 `Err` 返回，由 caller 决定吞 vs 抛。
    fn fetch(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<FeedHttpResponse>;
}

/// 生产 HTTP 抓取层 —— 用 `reqwest` blocking client 走 rustls TLS（纯 Rust，
/// 与 mail-parser / reqwest_dav 共享 TLS 栈，不引 native-tls/openssl-sys）。
pub struct RealFeedFetcher;

impl FeedFetcher for RealFeedFetcher {
    fn fetch(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<FeedHttpResponse> {
        // reqwest blocking client：`SourceConnector::fetch_documents` 是同步契约。
        // 与 WebDavConnector / EmailConnector 不同 —— 它们用 tokio 桥接因为底层
        // async-only；reqwest 既有 blocking 也有 async，blocking 更省一个 runtime。
        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!("attune/", env!("CARGO_PKG_VERSION"), " (+rss)"))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| VaultError::LlmUnavailable(format!("rss http client: {e}")))?;

        let mut req = client.get(url);
        if let Some(tag) = etag {
            req = req.header(reqwest::header::IF_NONE_MATCH, tag);
        }
        if let Some(lm) = last_modified {
            req = req.header(reqwest::header::IF_MODIFIED_SINCE, lm);
        }

        let resp = req
            .send()
            .map_err(|e| VaultError::LlmUnavailable(format!("rss http get {url}: {e}")))?;

        // 304 Not Modified —— server 接受了条件 GET，无需重抓。
        if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(FeedHttpResponse::NotModified);
        }

        if !resp.status().is_success() {
            return Err(VaultError::LlmUnavailable(format!(
                "rss http {} for {url}",
                resp.status()
            )));
        }

        let etag = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let last_modified = resp
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let body = resp
            .bytes()
            .map_err(|e| VaultError::LlmUnavailable(format!("rss http body {url}: {e}")))?
            .to_vec();

        Ok(FeedHttpResponse::Ok {
            body,
            etag,
            last_modified,
        })
    }
}

/// 一份 fetch + parse 后的轻量条目（无 entry/feed 的全部 RSS 字段，只保留 ingest 所需）。
#[derive(Debug, Clone)]
pub struct ParsedRssEntry {
    /// 稳定唯一标识：entry.id 优先，缺失时回退到第一个 link.href。两者皆空则跳过。
    pub guid: String,
    pub title: String,
    /// 已剥 HTML 的正文文本。Atom `<content>` 优先，缺失回退 `<summary>`。
    pub body: String,
    /// 条目原始链接（若有），写入 RawDocument.uri / metadata.url。
    pub link: Option<String>,
    /// 发布时间 RFC3339（若有）。
    pub published_at: Option<String>,
}

/// 解析 feed 字节为 `ParsedRssEntry` 列表。纯函数，离线可测。
///
/// feed-rs 2.x 统一 Atom / RSS 1.0 / RSS 2.0 / JSON Feed 到一个 `Feed` 结构，
/// 此层只取 ingest 必需字段。entry.id 缺失时回退第一个 link.href 作 guid。
pub fn parse_feed_bytes(bytes: &[u8]) -> Result<Vec<ParsedRssEntry>> {
    let feed = feed_rs::parser::parse(bytes)
        .map_err(|e| VaultError::LlmUnavailable(format!("rss parse: {e}")))?;

    let mut out = Vec::with_capacity(feed.entries.len());
    for entry in feed.entries {
        // guid: entry.id 优先（Atom required / RSS 2 guid / RSS 1 link-hash），
        // feed-rs 始终填充（hash fallback），但仍 defensive check 空字符串。
        let link = entry.links.first().map(|l| l.href.clone());
        let guid = if !entry.id.is_empty() {
            entry.id.clone()
        } else if let Some(ref l) = link {
            l.clone()
        } else {
            // 无 guid 又无 link —— 不可识别，跳过。
            continue;
        };

        let title = entry
            .title
            .map(|t| t.content)
            .unwrap_or_default();

        // body: <content> 优先 (Atom recommended / RSS 2 content:encoded)；
        // 缺失回退 <summary> (RSS 2 description)。两者都可能含 HTML，统一剥标签。
        let raw_body = entry
            .content
            .and_then(|c| c.body)
            .or_else(|| entry.summary.map(|s| s.content))
            .unwrap_or_default();
        let body = if raw_body.contains('<') {
            html_to_text(&raw_body)
        } else {
            raw_body
        };

        let published_at = entry
            .published
            .or(entry.updated)
            .map(|d| d.to_rfc3339());

        out.push(ParsedRssEntry {
            guid,
            title,
            body,
            link,
            published_at,
        });
    }
    Ok(out)
}

/// RSS 订阅采集源。
///
/// 单 feed 视角的连接器 —— `fetch_documents` 一次只处理一个 feed。
/// 多 feed 调度（轮转 / 到期判断）在 server 层的 `scanner_rss` worker 里做。
pub struct RssConnector {
    feed: RssFeedFetch,
    fetcher: Box<dyn FeedFetcher>,
    /// 200 OK 时由 fetch 调用回填，供 caller 持久化到 store。
    /// 用 `std::cell::RefCell` 是因为 `SourceConnector::fetch_documents` 签名是
    /// `&self`（与 Email/WebDAV 一致），无法 `&mut self`。
    last_response: std::cell::RefCell<Option<FeedHttpResponse>>,
}

impl RssConnector {
    /// 用指定 fetcher 构造（测试注入 mock；生产传 `RealFeedFetcher`）。
    pub fn with_fetcher(feed: RssFeedFetch, fetcher: Box<dyn FeedFetcher>) -> Self {
        Self {
            feed,
            fetcher,
            last_response: std::cell::RefCell::new(None),
        }
    }

    /// 用生产 HTTP 抓取层构造。
    pub fn new(feed: RssFeedFetch) -> Self {
        Self::with_fetcher(feed, Box::new(RealFeedFetcher))
    }

    /// 取最近一次 fetch 的响应（200/304）—— caller (worker) 据此决定持久化路径。
    /// 调用 `fetch_documents` 后立即取，重复 fetch 会覆盖。
    pub fn take_last_response(&self) -> Option<FeedHttpResponse> {
        self.last_response.borrow_mut().take()
    }

    /// 判断 entry guid 是否"严格新于"上次 cursor。
    /// 简单语义：guid != last_entry_guid 即视为新（RSS 没有严格的总序，多数 feed
    /// 按时间倒序输出，但靠 guid 字符串比较不可靠 —— 用全等是最安全的语义）。
    /// 真正的去重防线在 ingest_document 内部的 content_hash 短路。
    fn is_new_entry(&self, guid: &str) -> bool {
        match &self.feed.last_entry_guid {
            Some(prev) => prev != guid,
            None => true,
        }
    }
}

impl SourceConnector for RssConnector {
    fn source_kind(&self) -> SourceKind {
        SourceKind::Rss
    }

    fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()> {
        // 1) 条件 GET。
        let resp = self.fetcher.fetch(
            &self.feed.url,
            self.feed.etag.as_deref(),
            self.feed.last_modified.as_deref(),
        )?;

        // 2) 把响应存起来供 caller 取。
        *self.last_response.borrow_mut() = Some(resp.clone());

        // 3) 304 → 不 emit 任何文档，caller 仅 touch_polled_at。
        let body = match resp {
            FeedHttpResponse::NotModified => return Ok(()),
            FeedHttpResponse::Ok { body, .. } => body,
        };

        // 4) 解析 feed 字节 → entry 列表。
        let entries = parse_feed_bytes(&body)?;

        // 5) entry 级 dedup：跳过 guid == last_entry_guid 的"最末已见条目"
        //    再之后的更早条目都跳过（典型 RSS 倒序输出场景）。
        let mut hit_prev = false;
        for entry in entries {
            if hit_prev {
                break;
            }
            if !self.is_new_entry(&entry.guid) {
                hit_prev = true;
                continue;
            }

            // body 空 + title 空 —— feed-rs 解析出来仍可能整条都为空，跳过。
            if entry.title.trim().is_empty() && entry.body.trim().is_empty() {
                continue;
            }

            // RawDocument 拼装。
            let mut metadata: HashMap<String, String> = HashMap::new();
            metadata.insert("feed_id".to_string(), self.feed.feed_id.clone());
            if !self.feed.feed_name.is_empty() {
                metadata.insert("feed_name".to_string(), self.feed.feed_name.clone());
            }
            if let Some(ref link) = entry.link {
                metadata.insert("url".to_string(), link.clone());
            }
            if let Some(ref pub_at) = entry.published_at {
                metadata.insert("published_at".to_string(), pub_at.clone());
            }
            metadata.insert("entry_guid".to_string(), entry.guid.clone());

            // source_ref = "{feed_id}#{guid}"
            // - feed_id 段保证多订阅源不冲突
            // - guid 段保证同 feed 内 entry 唯一
            // - .txt 后缀让 RawDocument::parse_filename → parser 走纯文本分支
            //   （body 已经是剥过 HTML 的纯文本，不再用 HTML 解析器）
            // 同时 source_ref 末段必须可解析出扩展名 —— 用 ".txt" 兜底。
            let source_ref = format!("{}#{}.txt", self.feed.feed_id, entry.guid);
            let uri = entry
                .link
                .clone()
                .unwrap_or_else(|| format!("rss://{}/{}", self.feed.feed_id, entry.guid));

            // body 空时用 title 兜底当正文 —— 让 ingest 至少能抓住标题做关键词检索。
            let content_text = if entry.body.trim().is_empty() {
                entry.title.clone()
            } else {
                entry.body.clone()
            };

            sink(RawDocument {
                uri,
                title: entry.title.clone(),
                content: content_text.into_bytes(),
                mime_hint: Some("text/plain".to_string()),
                source_kind: SourceKind::Rss,
                source_ref,
                // modified_marker = guid —— indexed_files 写入后下次同 entry 走
                // get_indexed_file 跳过。content_hash 短路是第二道防线。
                modified_marker: Some(entry.guid.clone()),
                // RSS 源无来源域 / 用户标签；corpus_domain 由 worker 透传。
                domain: None,
                tags: None,
                corpus_domain: None,
                metadata,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_RSS2: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Test Channel</title>
    <link>https://ex.com/</link>
    <description>A test feed</description>
    <item>
      <title>First post</title>
      <link>https://ex.com/posts/1</link>
      <guid isPermaLink="false">tag:ex.com,2026:1</guid>
      <pubDate>Wed, 21 Oct 2026 07:28:00 GMT</pubDate>
      <description>&lt;p&gt;Hello &lt;b&gt;world&lt;/b&gt;&lt;/p&gt;</description>
    </item>
    <item>
      <title>Second post</title>
      <link>https://ex.com/posts/2</link>
      <guid isPermaLink="false">tag:ex.com,2026:2</guid>
      <description>Plain text body two.</description>
    </item>
  </channel>
</rss>"#;

    const SIMPLE_ATOM: &[u8] = br#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom Test</title>
  <link href="https://atom.ex.com/"/>
  <updated>2026-01-01T00:00:00Z</updated>
  <id>urn:atom:test</id>
  <entry>
    <title>Atom Entry 1</title>
    <id>urn:atom:entry:1</id>
    <link href="https://atom.ex.com/1"/>
    <updated>2026-01-01T00:00:00Z</updated>
    <content type="html">&lt;p&gt;Atom body&lt;/p&gt;</content>
  </entry>
</feed>"#;

    #[test]
    fn parse_rss2_extracts_entries_with_html_stripped() {
        let entries = parse_feed_bytes(SIMPLE_RSS2).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "First post");
        assert_eq!(entries[0].guid, "tag:ex.com,2026:1");
        assert_eq!(entries[0].link.as_deref(), Some("https://ex.com/posts/1"));
        assert!(
            !entries[0].body.contains("<p>") && !entries[0].body.contains("<b>"),
            "HTML tags must be stripped, got: {}",
            entries[0].body
        );
        assert!(entries[0].body.contains("Hello"));
        assert!(entries[0].body.contains("world"));
        assert!(entries[0].published_at.is_some(), "pubDate 解析");
    }

    #[test]
    fn parse_atom_uses_content_over_summary() {
        let entries = parse_feed_bytes(SIMPLE_ATOM).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "Atom Entry 1");
        assert_eq!(entries[0].guid, "urn:atom:entry:1");
        assert!(entries[0].body.contains("Atom body"));
        assert!(!entries[0].body.contains("<p>"));
    }

    #[test]
    fn parse_garbage_returns_err() {
        let result = parse_feed_bytes(b"not valid xml at all");
        assert!(result.is_err());
    }

    /// 离线 mock fetcher：按 url 返回预置响应。
    struct MockFetcher {
        responses: HashMap<String, FeedHttpResponse>,
    }

    impl FeedFetcher for MockFetcher {
        fn fetch(
            &self,
            url: &str,
            _etag: Option<&str>,
            _last_modified: Option<&str>,
        ) -> Result<FeedHttpResponse> {
            self.responses
                .get(url)
                .cloned()
                .ok_or_else(|| VaultError::LlmUnavailable(format!("mock: no response for {url}")))
        }
    }

    fn make_feed() -> RssFeedFetch {
        RssFeedFetch {
            feed_id: "feed-A".into(),
            feed_name: "Test".into(),
            url: "https://ex.com/feed.xml".into(),
            last_entry_guid: None,
            etag: None,
            last_modified: None,
        }
    }

    #[test]
    fn connector_emits_one_rawdocument_per_entry_on_first_poll() {
        let mut responses = HashMap::new();
        responses.insert(
            "https://ex.com/feed.xml".to_string(),
            FeedHttpResponse::Ok {
                body: SIMPLE_RSS2.to_vec(),
                etag: Some("\"v1\"".into()),
                last_modified: None,
            },
        );
        let conn = RssConnector::with_fetcher(make_feed(), Box::new(MockFetcher { responses }));
        let mut docs: Vec<RawDocument> = Vec::new();
        {
            let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
            conn.fetch_documents(&mut sink).unwrap();
        }
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].source_kind, SourceKind::Rss);
        assert_eq!(docs[0].title, "First post");
        assert_eq!(docs[0].source_ref, "feed-A#tag:ex.com,2026:1.txt");
        assert_eq!(docs[0].modified_marker.as_deref(), Some("tag:ex.com,2026:1"));
        assert_eq!(docs[0].metadata.get("feed_id").unwrap(), "feed-A");
        assert_eq!(
            docs[0].metadata.get("url").unwrap(),
            "https://ex.com/posts/1"
        );
        assert!(docs[0].metadata.contains_key("published_at"));
        assert!(docs[0].metadata.contains_key("entry_guid"));
    }

    #[test]
    fn connector_skips_already_seen_guid_and_older() {
        // last_entry_guid 已记到第二条 —— 重 poll 时整箱跳过（dedup invariant）。
        let mut feed = make_feed();
        feed.last_entry_guid = Some("tag:ex.com,2026:1".into());
        let mut responses = HashMap::new();
        responses.insert(
            "https://ex.com/feed.xml".to_string(),
            FeedHttpResponse::Ok {
                body: SIMPLE_RSS2.to_vec(),
                etag: None,
                last_modified: None,
            },
        );
        let conn = RssConnector::with_fetcher(feed, Box::new(MockFetcher { responses }));
        let mut docs: Vec<RawDocument> = Vec::new();
        {
            let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
            conn.fetch_documents(&mut sink).unwrap();
        }
        // RSS 输出顺序：entry 1, entry 2。cursor = entry 1 表示"上次看到的最末"，
        // entry 2 在 cursor 之"前"（更老）→ 命中 prev 后 break，0 个新条目。
        // 这是保守语义：希望真正"新条目"出现在 cursor 之前，由 server 决定。
        assert_eq!(docs.len(), 0);
    }

    #[test]
    fn connector_returns_not_modified_with_empty_emit() {
        let mut responses = HashMap::new();
        responses.insert(
            "https://ex.com/feed.xml".to_string(),
            FeedHttpResponse::NotModified,
        );
        let conn = RssConnector::with_fetcher(make_feed(), Box::new(MockFetcher { responses }));
        let mut docs: Vec<RawDocument> = Vec::new();
        {
            let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
            conn.fetch_documents(&mut sink).unwrap();
        }
        assert_eq!(docs.len(), 0, "304 路径不能产出任何 RawDocument");
        // worker 取响应类型决定走 touch_polled_at（不更新 etag/guid）。
        assert!(matches!(
            conn.take_last_response(),
            Some(FeedHttpResponse::NotModified)
        ));
    }

    #[test]
    fn connector_last_response_carries_etag_on_200() {
        let mut responses = HashMap::new();
        responses.insert(
            "https://ex.com/feed.xml".to_string(),
            FeedHttpResponse::Ok {
                body: SIMPLE_ATOM.to_vec(),
                etag: Some("\"new-tag\"".into()),
                last_modified: Some("Wed, 21 Oct 2026 07:28:00 GMT".into()),
            },
        );
        let conn = RssConnector::with_fetcher(make_feed(), Box::new(MockFetcher { responses }));
        let mut docs: Vec<RawDocument> = Vec::new();
        {
            let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
            conn.fetch_documents(&mut sink).unwrap();
        }
        match conn.take_last_response().unwrap() {
            FeedHttpResponse::Ok {
                etag,
                last_modified,
                ..
            } => {
                assert_eq!(etag.as_deref(), Some("\"new-tag\""));
                assert_eq!(
                    last_modified.as_deref(),
                    Some("Wed, 21 Oct 2026 07:28:00 GMT")
                );
            }
            FeedHttpResponse::NotModified => panic!("expected Ok"),
        }
    }

    #[test]
    fn connector_emits_atom_entry_with_html_content_stripped() {
        let mut responses = HashMap::new();
        responses.insert(
            "https://ex.com/feed.xml".to_string(),
            FeedHttpResponse::Ok {
                body: SIMPLE_ATOM.to_vec(),
                etag: None,
                last_modified: None,
            },
        );
        let conn = RssConnector::with_fetcher(make_feed(), Box::new(MockFetcher { responses }));
        let mut docs: Vec<RawDocument> = Vec::new();
        {
            let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
            conn.fetch_documents(&mut sink).unwrap();
        }
        assert_eq!(docs.len(), 1);
        let body = std::str::from_utf8(&docs[0].content).unwrap();
        assert!(body.contains("Atom body"));
        assert!(!body.contains("<p>"), "HTML 必须被剥掉");
    }
}
