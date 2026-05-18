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
