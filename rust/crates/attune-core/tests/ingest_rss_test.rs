//! RSS 采集源端到端集成测试。
//!
//! 三类覆盖：
//! 1. parse 层 — feed-rs 对 RSS 2.0 / Atom 解析；HTML 剥标签
//! 2. connector 层 — 走 mock FeedFetcher，验证条件 GET / 304 / entry dedup
//! 3. ingest 层 — connector 产出的 RawDocument 真正过 `ingest_document` 入库

use std::collections::HashMap;

use attune_core::crypto::Key32;
use attune_core::ingest::{
    ingest_document, parse_feed_bytes, DocumentSink, FeedFetcher, FeedHttpResponse,
    IngestOutcome, RawDocument, RssConnector, RssFeedFetch, SourceConnector, SourceKind,
};
use attune_core::store::Store;

const RSS2_TWO_ITEMS: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Open Source Weekly</title>
    <link>https://oss.example.com/</link>
    <description>OSS news</description>
    <item>
      <title>Linux 6.18 released</title>
      <link>https://oss.example.com/linux-6-18</link>
      <guid isPermaLink="false">oss-weekly-001</guid>
      <pubDate>Mon, 19 May 2026 12:00:00 GMT</pubDate>
      <description>&lt;p&gt;Linus &lt;b&gt;released&lt;/b&gt; the new kernel.&lt;/p&gt;</description>
    </item>
    <item>
      <title>Rust 1.99 stable</title>
      <link>https://oss.example.com/rust-1-99</link>
      <guid isPermaLink="false">oss-weekly-002</guid>
      <description>Plain text description for rust release.</description>
    </item>
  </channel>
</rss>"#;

const ATOM_ONE_ENTRY: &[u8] = br#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Tech Blog</title>
  <link href="https://blog.example.com/"/>
  <updated>2026-05-19T12:00:00Z</updated>
  <id>urn:tech:blog</id>
  <entry>
    <title>RVV in production</title>
    <id>urn:tech:blog:001</id>
    <link href="https://blog.example.com/rvv-prod"/>
    <updated>2026-05-19T12:00:00Z</updated>
    <content type="html">&lt;p&gt;We shipped RVV optimizations to prod.&lt;/p&gt;</content>
  </entry>
</feed>"#;

struct MockFetcher {
    responses: HashMap<String, FeedHttpResponse>,
}

impl MockFetcher {
    fn new(responses: HashMap<String, FeedHttpResponse>) -> Self {
        Self { responses }
    }
}

impl FeedFetcher for MockFetcher {
    fn fetch(
        &self,
        url: &str,
        _etag: Option<&str>,
        _last_modified: Option<&str>,
    ) -> attune_core::error::Result<FeedHttpResponse> {
        self.responses.get(url).cloned().ok_or_else(|| {
            attune_core::error::VaultError::LlmUnavailable(format!("mock: no response for {url}"))
        })
    }
}

fn make_feed(last_guid: Option<&str>, etag: Option<&str>) -> RssFeedFetch {
    RssFeedFetch {
        feed_id: "feed-test".into(),
        feed_name: "Test Feed".into(),
        url: "https://ex.com/feed.xml".into(),
        last_entry_guid: last_guid.map(String::from),
        etag: etag.map(String::from),
        last_modified: None,
    }
}

#[test]
fn parse_rss2_with_two_items() {
    let entries = parse_feed_bytes(RSS2_TWO_ITEMS).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].title, "Linux 6.18 released");
    assert_eq!(entries[0].guid, "oss-weekly-001");
    assert!(entries[0].body.contains("Linus"));
    assert!(entries[0].body.contains("released"));
    assert!(!entries[0].body.contains("<p>"));
}

#[test]
fn parse_atom_with_one_entry() {
    let entries = parse_feed_bytes(ATOM_ONE_ENTRY).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].title, "RVV in production");
    assert!(entries[0].body.contains("RVV"));
}

#[test]
fn first_poll_emits_all_entries() {
    let mut responses = HashMap::new();
    responses.insert(
        "https://ex.com/feed.xml".to_string(),
        FeedHttpResponse::Ok {
            body: RSS2_TWO_ITEMS.to_vec(),
            etag: Some("\"first\"".into()),
            last_modified: None,
        },
    );
    let fetcher = MockFetcher::new(responses);
    let conn = RssConnector::with_fetcher(make_feed(None, None), Box::new(fetcher));
    let mut docs: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
        conn.fetch_documents(&mut sink).unwrap();
    }
    assert_eq!(docs.len(), 2);
    assert!(docs.iter().all(|d| d.source_kind == SourceKind::Rss));
}

#[test]
fn conditional_get_passes_etag_to_fetcher() {
    let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(
        String,
        Option<String>,
        Option<String>,
    )>::new()));

    struct CaptureFetcher {
        received: std::sync::Arc<std::sync::Mutex<Vec<(String, Option<String>, Option<String>)>>>,
    }
    impl FeedFetcher for CaptureFetcher {
        fn fetch(
            &self,
            url: &str,
            etag: Option<&str>,
            last_modified: Option<&str>,
        ) -> attune_core::error::Result<FeedHttpResponse> {
            self.received.lock().unwrap().push((
                url.to_string(),
                etag.map(String::from),
                last_modified.map(String::from),
            ));
            Ok(FeedHttpResponse::NotModified)
        }
    }
    let conn = RssConnector::with_fetcher(
        make_feed(Some("oss-weekly-001"), Some("\"prev-etag\"")),
        Box::new(CaptureFetcher {
            received: received.clone(),
        }),
    );

    let mut docs: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
        conn.fetch_documents(&mut sink).unwrap();
    }
    assert_eq!(docs.len(), 0, "304 路径不产文档");
    let recv = received.lock().unwrap();
    assert_eq!(recv.len(), 1);
    assert_eq!(recv[0].0, "https://ex.com/feed.xml");
    assert_eq!(
        recv[0].1.as_deref(),
        Some("\"prev-etag\""),
        "If-None-Match 必须用上次 ETag"
    );
}

#[test]
fn dedup_invariant_re_poll_returns_zero_new() {
    let mut responses = HashMap::new();
    responses.insert(
        "https://ex.com/feed.xml".to_string(),
        FeedHttpResponse::Ok {
            body: RSS2_TWO_ITEMS.to_vec(),
            etag: None,
            last_modified: None,
        },
    );
    let fetcher_1 = MockFetcher::new(responses.clone());
    let conn_1 = RssConnector::with_fetcher(make_feed(None, None), Box::new(fetcher_1));
    let mut docs_1: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|d| docs_1.push(d));
        conn_1.fetch_documents(&mut sink).unwrap();
    }
    assert_eq!(docs_1.len(), 2);
    let cursor = docs_1[0].modified_marker.clone().unwrap();
    assert_eq!(cursor, "oss-weekly-001");

    let fetcher_2 = MockFetcher::new(responses);
    let conn_2 = RssConnector::with_fetcher(make_feed(Some(&cursor), None), Box::new(fetcher_2));
    let mut docs_2: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|d| docs_2.push(d));
        conn_2.fetch_documents(&mut sink).unwrap();
    }
    assert_eq!(
        docs_2.len(),
        0,
        "二次 poll 同响应 + cursor 已推进 → 0 个新 entry（dedup invariant）"
    );
}

#[test]
fn end_to_end_ingest_rss_entry_into_store() {
    let mut responses = HashMap::new();
    responses.insert(
        "https://ex.com/feed.xml".to_string(),
        FeedHttpResponse::Ok {
            body: RSS2_TWO_ITEMS.to_vec(),
            etag: None,
            last_modified: None,
        },
    );
    let fetcher = MockFetcher::new(responses);
    let conn = RssConnector::with_fetcher(make_feed(None, None), Box::new(fetcher));
    let mut docs: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
        conn.fetch_documents(&mut sink).unwrap();
    }
    assert_eq!(docs.len(), 2);

    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let mut inserted = 0;
    for doc in &docs {
        let outcome = ingest_document(&store, &dek, doc)
            .expect("RSS RawDocument must ingest, not Err");
        if matches!(outcome, IngestOutcome::Inserted { .. }) {
            inserted += 1;
        }
    }
    assert_eq!(inserted, 2, "两条 entry 都应 Inserted");
}

#[test]
fn fetch_error_propagates_to_caller() {
    struct ErrFetcher;
    impl FeedFetcher for ErrFetcher {
        fn fetch(
            &self,
            _: &str,
            _: Option<&str>,
            _: Option<&str>,
        ) -> attune_core::error::Result<FeedHttpResponse> {
            Err(attune_core::error::VaultError::LlmUnavailable(
                "DNS failed".into(),
            ))
        }
    }
    let conn = RssConnector::with_fetcher(make_feed(None, None), Box::new(ErrFetcher));
    let mut docs: Vec<RawDocument> = Vec::new();
    let result = {
        let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
        conn.fetch_documents(&mut sink)
    };
    assert!(result.is_err());
    assert_eq!(docs.len(), 0);
}

#[test]
fn empty_body_entry_is_skipped() {
    const EMPTY_BODY_RSS: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>x</title><link>https://e</link><description>x</description>
  <item><guid>empty-1</guid></item>
  <item><title>Has title</title><guid>has-title</guid></item>
</channel></rss>"#;
    let mut responses = HashMap::new();
    responses.insert(
        "https://ex.com/feed.xml".to_string(),
        FeedHttpResponse::Ok {
            body: EMPTY_BODY_RSS.to_vec(),
            etag: None,
            last_modified: None,
        },
    );
    let conn = RssConnector::with_fetcher(make_feed(None, None), Box::new(MockFetcher::new(responses)));
    let mut docs: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
        conn.fetch_documents(&mut sink).unwrap();
    }
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].title, "Has title");
}
