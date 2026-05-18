//! Email IMAP 采集源。
//!
//! EmailConnector 实现 SourceConnector：用单线程 tokio runtime 桥接 async-imap
//! 的 async I/O（与 scanner_webdav.rs::WebDavConnector::drive_blocking 同模式）。
//! 每封邮件 + 每个文档类附件各产一份 RawDocument，逐个交给 sink —— 大邮箱不物化。
//! 解析层（parse_email_bytes / MailMessage）是纯函数，离线可测，不依赖网络。

use crate::error::{Result, VaultError};
use mail_parser::{MessageParser, MimeHeaders};

/// mail-parser 解析出的一封邮件（解析层产物，与 IMAP 抓取解耦）。
#[derive(Debug, Clone)]
pub struct MailMessage {
    /// 邮件主题（Subject header）。
    pub subject: String,
    /// 稳定唯一标识（Message-ID header），保留 <> 包裹形式。缺失时由调用方用 "{folder}:{uid}" 兜底。
    pub message_id: String,
    /// 正文纯文本：text/plain 优先，否则 text/html 剥标签。
    pub body: String,
    /// 发件人地址（From header 第一个地址）。
    pub from: Option<String>,
    /// 发件日期（RFC3339 格式字符串）。
    pub date: Option<String>,
    /// 文档类附件（已按扩展名白名单过滤，已剔除超大附件）。
    pub attachments: Vec<MailAttachment>,
}

/// 一个文档类附件。
#[derive(Debug, Clone)]
pub struct MailAttachment {
    pub filename: String,
    pub content: Vec<u8>,
}

/// 附件大小上限（与本地 upload / WebDAV 一致，超限跳过）。
pub const MAX_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;

/// 受支持的文档类附件扩展名（与 parser 支持集对齐，二进制媒体不入库）。
const SUPPORTED_ATTACHMENT_EXTS: &[&str] = &[
    "md", "txt", "py", "js", "ts", "rs", "go", "java", "pdf", "docx", "html", "htm", "csv",
    "rtf", "pptx", "xlsx", "png", "jpg", "jpeg",
];

fn is_supported_attachment(filename: &str) -> bool {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    SUPPORTED_ATTACHMENT_EXTS.contains(&ext.as_str())
}

/// 极简 HTML → 纯文本：去标签、解最常见实体、压空白。
/// 不追求完美渲染，只要让 text/html-only 邮件可被检索 + 不在正文里塞标签噪声。
///
/// `<style>` / `<script>` 块的内容（CSS 规则、JS 代码）是机器语言，不是可阅读的
/// 邮件正文，整块压制，不输出到文本。
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    // Current tag-name buffer (lower-cased), reset at each `<`.
    let mut tag_name = String::new();
    // When Some("style") or Some("script"), suppress text until the matching close tag.
    let mut suppress: Option<&'static str> = None;

    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag_name.clear();
            }
            '>' => {
                in_tag = false;
                // Determine what tag just closed.
                // tag_name holds the first token inside `<...>`, e.g. "/style", "style", "script".
                let name = tag_name.trim_start_matches('/').trim();
                if name == "style" || name == "script" {
                    if tag_name.starts_with('/') {
                        // Closing tag: clear suppress if it matches.
                        if suppress == Some(if name == "style" { "style" } else { "script" }) {
                            suppress = None;
                        }
                    } else {
                        // Opening tag: start suppressing.
                        suppress = Some(if name == "style" { "style" } else { "script" });
                    }
                }
                tag_name.clear();
            }
            _ if in_tag => {
                // Accumulate the tag name (only the first whitespace-delimited token matters).
                if !ch.is_ascii_whitespace() || !tag_name.is_empty() {
                    if ch.is_ascii_whitespace() {
                        // Stop extending tag_name once we hit whitespace after the name.
                        // Do nothing — tag_name already has the name.
                    } else {
                        tag_name.push(ch.to_ascii_lowercase());
                    }
                }
            }
            _ => {
                // Regular text character: output only when not suppressed.
                if suppress.is_none() {
                    out.push(ch);
                }
            }
        }
    }

    // Decode HTML entities: &amp; LAST so that &amp;lt; → &lt; (not <).
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 解析一封邮件的原始 RFC822 字节为 `MailMessage`。
///
/// 纯函数，不触网 —— IMAP 抓取层把 FETCH 到的字节喂进来。正文取 text/plain
/// 优先，缺失时取 text/html 剥标签；文档类附件按扩展名白名单 + 大小上限过滤。
///
/// API 适配说明（mail-parser 0.11 实测）：
/// - `.message_id()` 返回角括号内的裸 ID（不含 `<>`），此处补回 `<>`。
/// - `.attachments()` 返回 `impl Iterator<Item = &MessagePart>`，`.attachment_name()`
///   由 trait 方法提供；attachment 字节通过匹配 `part.body`（`PartType::Binary` /
///   `PartType::InlineBinary`）取出，mail-parser 0.11 无单独的 `.contents()` 方法。
pub fn parse_email_bytes(raw: &[u8]) -> Result<MailMessage> {
    let parsed = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| VaultError::LlmUnavailable("email parse failed".into()))?;

    // mail-parser 0.11 is lenient and parses arbitrary bytes as an empty message.
    // Reject inputs that yield no recognisable email structure.
    let has_headers = parsed.subject().is_some()
        || parsed.message_id().is_some()
        || parsed.from().is_some()
        || parsed.date().is_some();
    if !has_headers && parsed.body_text(0).is_none() && parsed.body_html(0).is_none() {
        return Err(VaultError::LlmUnavailable("email parse failed: no recognisable content".into()));
    }

    let subject = parsed.subject().unwrap_or_default().to_string();

    // mail-parser 0.11 strips angle brackets from Message-ID; restore them for stable identity.
    let message_id = match parsed.message_id() {
        Some(id) if id.starts_with('<') => id.to_string(),
        Some(id) => format!("<{id}>"),
        None => String::new(),
    };

    let from = parsed
        .from()
        .and_then(|addr| addr.first())
        .and_then(|a| a.address())
        .map(|s| s.to_string());

    let date = parsed.date().map(|d| d.to_rfc3339());

    // 正文：text/plain 优先，否则第一个 text/html 剥标签。
    let body = parsed
        .body_text(0)
        .map(|t| t.into_owned())
        .filter(|t| !t.trim().is_empty())
        .or_else(|| parsed.body_html(0).map(|h| html_to_text(&h)))
        .unwrap_or_default();

    // 附件：按扩展名白名单 + 大小上限过滤。
    // mail-parser 0.11 没有 .contents() 方法，字节在 part.body 的 PartType 枚举里。
    let mut attachments = Vec::new();
    for att in parsed.attachments() {
        let filename = att.attachment_name().unwrap_or("attachment").to_string();
        if !is_supported_attachment(&filename) {
            continue;
        }
        let bytes: &[u8] = match &att.body {
            mail_parser::PartType::Binary(b) | mail_parser::PartType::InlineBinary(b) => b,
            mail_parser::PartType::Text(t) => t.as_bytes(),
            mail_parser::PartType::Html(h) => h.as_bytes(),
            _ => continue,
        };
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            log::warn!("email: skip oversized attachment {filename} ({} bytes)", bytes.len());
            continue;
        }
        attachments.push(MailAttachment {
            filename,
            content: bytes.to_vec(),
        });
    }

    Ok(MailMessage {
        subject,
        message_id,
        body,
        from,
        date,
        attachments,
    })
}

/// Email 账户连接配置（明文，连接器持有；持久化由 store/email_accounts.rs 负责）。
#[derive(Debug, Clone)]
pub struct EmailConfig {
    /// IMAP 服务器主机名（如 imap.gmail.com）。
    pub host: String,
    /// IMAP over TLS 端口（标准 993）。
    pub port: u16,
    pub username: String,
    /// 明文密码 / App Password。
    pub password: String,
    /// 要同步的文件夹（默认 INBOX + Sent）。
    pub folders: Vec<String>,
}

impl EmailConfig {
    /// 文件夹列表为空时回退到默认 INBOX + Sent。
    pub fn effective_folders(&self) -> Vec<String> {
        if self.folders.is_empty() {
            vec!["INBOX".to_string(), "Sent".to_string()]
        } else {
            self.folders.clone()
        }
    }
}

/// 一封从 IMAP 抓回的邮件原始字节 + 其 UID。
#[derive(Debug, Clone)]
pub struct FetchedMail {
    pub uid: u32,
    pub raw: Vec<u8>,
}

/// IMAP 抓取层抽象 —— 把网络 I/O 与连接器逻辑解耦，让连接器离线可测。
///
/// 实现者负责连接 / 登录 / 选文件夹 / `UID SEARCH since_uid:* ` / 逐 UID FETCH。
/// 单封邮件的 FETCH 失败应吞掉记日志继续；只有源级致命错误（连不上 / 鉴权失败 /
/// 文件夹不存在）才返回 Err。
pub trait ImapFetcher {
    /// 抓取 `folder` 内 UID 严格大于 `since_uid` 的全部邮件。
    fn fetch_since(&self, folder: &str, since_uid: u32) -> Result<Vec<FetchedMail>>;
}

use std::collections::HashMap;

use crate::ingest::{DocumentSink, RawDocument, SourceConnector, SourceKind};

/// IMAP 邮箱采集源。
///
/// 持有一个 `ImapFetcher`（生产 = RealImapFetcher，测试 = mock），逐文件夹按
/// UID 增量抓取，把每封邮件正文 + 每个文档类附件转成 RawDocument 交给 sink。
pub struct EmailConnector {
    config: EmailConfig,
    fetcher: Box<dyn ImapFetcher>,
    /// 每文件夹的 IMAP UID 增量起点（fetch_since 只取 UID 严格大于此值的邮件）。
    /// 由 caller（sync_email_account）在 fetch 前从 email_folder_uids 表注入。
    since_by_folder: HashMap<String, u32>,
}

impl EmailConnector {
    /// 用指定 fetcher 构造（测试注入 mock；生产传 RealImapFetcher）。
    pub fn with_fetcher(config: EmailConfig, fetcher: Box<dyn ImapFetcher>) -> Self {
        Self {
            config,
            fetcher,
            since_by_folder: HashMap::new(),
        }
    }

    /// 用生产 IMAP 抓取层构造（rustls TLS over async-imap）。
    pub fn new(config: EmailConfig) -> Self {
        let fetcher = Box::new(RealImapFetcher {
            host: config.host.clone(),
            port: config.port,
            username: config.username.clone(),
            password: config.password.clone(),
        });
        Self::with_fetcher(config, fetcher)
    }

    /// 设置某文件夹的 UID 增量起点（caller 从 email_folder_uids 表读出后注入）。
    pub fn set_folder_since(&mut self, folder: &str, since_uid: u32) {
        self.since_by_folder.insert(folder.to_string(), since_uid);
    }

    /// 把一封邮件展开成 RawDocument 列表（正文 1 份 + 每附件 1 份）交给 sink。
    fn emit_mail(&self, folder: &str, fetched: &FetchedMail, sink: &mut DocumentSink<'_>) {
        let msg = match parse_email_bytes(&fetched.raw) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("email: parse uid {} in {folder} failed: {e}", fetched.uid);
                return;
            }
        };

        // Message-ID 缺失时用 folder:uid 兜底作稳定唯一键。
        let msg_id = if msg.message_id.trim().is_empty() {
            format!("{folder}:{}", fetched.uid)
        } else {
            msg.message_id.clone()
        };
        let marker = format!("{folder}:{}", fetched.uid);

        let mut metadata = HashMap::new();
        if let Some(ref from) = msg.from {
            metadata.insert("from".to_string(), from.clone());
        }
        if let Some(ref date) = msg.date {
            metadata.insert("date".to_string(), date.clone());
        }
        metadata.insert("folder".to_string(), folder.to_string());

        // 正文 RawDocument。source_ref = "{Message-ID}.txt"（跨 folder 去重稳定键）。
        // .txt 后缀是功能性的、不可省：ingest_document 纯靠 parse_filename 取
        // source_ref 末段、再用 Path::extension() 推扩展名来选 parser（mime_hint
        // 不参与路由）。裸 Message-ID 以 "@domain.tld" 结尾，extension() 会解出
        // 非白名单的 "tld" → 正文整封 ingest 失败。.txt 强制走纯文本分支。
        if !msg.body.trim().is_empty() {
            sink(RawDocument {
                uri: format!("imap://{}/{folder}/{}", self.config.host, fetched.uid),
                title: msg.subject.clone(),
                content: msg.body.clone().into_bytes(),
                mime_hint: Some("text/plain".to_string()),
                source_kind: SourceKind::Email,
                source_ref: format!("{msg_id}.txt"),
                modified_marker: Some(marker.clone()),
                domain: None,
                tags: None,
                corpus_domain: None,
                metadata: metadata.clone(),
            });
        }

        // 每个文档类附件单独一份 RawDocument。source_ref = "{msg_id}#att{idx}/{filename}"：
        // #att{idx} 后缀避免与正文 / 其它附件碰撞；末段是文件名，让 parse_filename
        // 取到附件扩展名 → parser 按扩展名选解析器。
        for (idx, att) in msg.attachments.iter().enumerate() {
            let mut att_meta = metadata.clone();
            att_meta.insert("attachment_of".to_string(), msg_id.clone());
            sink(RawDocument {
                uri: format!(
                    "imap://{}/{folder}/{}/att{idx}",
                    self.config.host, fetched.uid
                ),
                title: format!("{} — {}", msg.subject, att.filename),
                content: att.content.clone(),
                mime_hint: None,
                source_kind: SourceKind::Email,
                source_ref: format!("{msg_id}#att{idx}/{}", att.filename),
                modified_marker: Some(format!("{marker}#att{idx}")),
                domain: None,
                tags: None,
                corpus_domain: None,
                metadata: att_meta,
            });
        }
    }
}

impl SourceConnector for EmailConnector {
    fn source_kind(&self) -> SourceKind {
        SourceKind::Email
    }

    fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()> {
        for folder in self.config.effective_folders() {
            let since = self.since_by_folder.get(&folder).copied().unwrap_or(0);
            // 单文件夹抓取失败不致命：记日志、继续下一个文件夹。
            let mails = match self.fetcher.fetch_since(&folder, since) {
                Ok(m) => m,
                Err(e) => {
                    log::warn!("email: fetch folder {folder} failed: {e}");
                    continue;
                }
            };
            for fetched in &mails {
                self.emit_mail(&folder, fetched, sink);
            }
        }
        Ok(())
    }
}

/// 生产 IMAP 抓取层 —— async-imap over tokio-rustls，单线程 runtime 桥接。
pub struct RealImapFetcher {
    host: String,
    port: u16,
    username: String,
    password: String,
}

impl RealImapFetcher {
    /// 建 rustls TLS 配置（webpki 根证书，纯 Rust，不引 native-tls）。
    fn tls_connector() -> tokio_rustls::TlsConnector {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let cfg = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        tokio_rustls::TlsConnector::from(std::sync::Arc::new(cfg))
    }

    /// 异步连接 + 登录 + 抓取某文件夹 since_uid 之后的邮件。
    async fn fetch_async(&self, folder: &str, since_uid: u32) -> Result<Vec<FetchedMail>> {
        use futures::StreamExt;

        let tcp = tokio::net::TcpStream::connect((self.host.as_str(), self.port))
            .await
            .map_err(|e| VaultError::LlmUnavailable(format!("imap connect: {e}")))?;
        let dns = rustls::pki_types::ServerName::try_from(self.host.clone())
            .map_err(|e| VaultError::LlmUnavailable(format!("imap server name: {e}")))?;
        let tls = Self::tls_connector()
            .connect(dns, tcp)
            .await
            .map_err(|e| VaultError::LlmUnavailable(format!("imap tls: {e}")))?;

        let client = async_imap::Client::new(tls);
        let mut session = client
            .login(&self.username, &self.password)
            .await
            .map_err(|(e, _)| VaultError::LlmUnavailable(format!("imap login: {e}")))?;

        // 选文件夹（不存在则视为该文件夹无邮件，返回空而非致命错误）。
        if session.select(folder).await.is_err() {
            let _ = session.logout().await;
            log::warn!("email: select folder {folder} failed, treating as empty");
            return Ok(Vec::new());
        }

        // UID SEARCH (since_uid+1):* —— 只要严格大于游标的 UID。
        let lower = since_uid.saturating_add(1);
        // uid_search returns a HashSet<Uid> with non-deterministic iteration order; sort ascending
        // so the caller receives a deterministic stream and UID-checkpoint logic is predictable.
        let uids = match session.uid_search(format!("UID {lower}:*")).await {
            Ok(u) => u,
            Err(e) => {
                let _ = session.logout().await;
                return Err(VaultError::LlmUnavailable(format!("imap uid search: {e}")));
            }
        };
        let mut uids: Vec<u32> = uids.into_iter().collect();
        uids.sort_unstable();

        let mut out = Vec::new();
        for uid in uids {
            // IMAP server 对 "lower:*" 在无更高 UID 时会回 lower 自身 —— 二次过滤。
            if uid <= since_uid {
                continue;
            }
            let mut stream = match session.uid_fetch(uid.to_string(), "RFC822").await {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("email: uid_fetch {uid} in {folder} failed: {e}");
                    continue;
                }
            };
            while let Some(item) = stream.next().await {
                match item {
                    Ok(fetch) => {
                        if let Some(body) = fetch.body() {
                            out.push(FetchedMail { uid, raw: body.to_vec() });
                        }
                    }
                    Err(e) => log::warn!("email: fetch stream uid {uid} error: {e}"),
                }
            }
            // uid_fetch 的 stream 借用 &mut session —— 必须 drop 后才能再调 session 方法。
            drop(stream);
        }
        let _ = session.logout().await;
        Ok(out)
    }
}

impl ImapFetcher for RealImapFetcher {
    fn fetch_since(&self, folder: &str, since_uid: u32) -> Result<Vec<FetchedMail>> {
        // SourceConnector::fetch_documents 是同步契约 —— 单线程 tokio runtime
        // 桥接内部 async I/O（与 WebDavConnector::drive_blocking 同模式）。
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| VaultError::LlmUnavailable(format!("imap runtime: {e}")))?;
        runtime.block_on(self.fetch_async(folder, since_uid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_to_text_strips_tags_and_collapses_whitespace() {
        let html = "<p>Hello   &amp;   <b>World</b></p>\n<div>line two</div>";
        let text = html_to_text(html);
        assert_eq!(text, "Hello & World line two");
    }

    #[test]
    fn html_to_text_drops_style_and_script_blocks() {
        let style_html = "<style>.foo { color: red; }</style><p>Real content</p>";
        let style_text = html_to_text(style_html);
        assert!(!style_text.contains("color"), "CSS 'color' should be suppressed");
        assert!(!style_text.contains("red"), "CSS 'red' should be suppressed");
        assert!(style_text.contains("Real content"), "body text must be kept");

        let script_html = "<script>alert('x')</script><p>Body</p>";
        let script_text = html_to_text(script_html);
        assert!(!script_text.contains("alert"), "JS 'alert' should be suppressed");
        assert!(script_text.contains("Body"), "body text must be kept");
    }

    #[test]
    fn supported_attachment_filters_media() {
        assert!(is_supported_attachment("report.pdf"));
        assert!(is_supported_attachment("notes.md"));
        assert!(!is_supported_attachment("video.mp4"));
        assert!(!is_supported_attachment("archive.zip"));
    }
}
