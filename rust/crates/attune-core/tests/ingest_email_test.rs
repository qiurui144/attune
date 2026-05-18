//! Email 采集源测试 —— 解析层离线测试（不连真 IMAP）。

use attune_core::ingest::email::parse_email_bytes;

const PLAIN: &[u8] = include_bytes!("fixtures/email/plain.eml");
const HTML_ONLY: &[u8] = include_bytes!("fixtures/email/html-only.eml");
const WITH_ATTACHMENT: &[u8] = include_bytes!("fixtures/email/with-attachment.eml");

#[test]
fn parse_plain_email_extracts_subject_and_body() {
    let msg = parse_email_bytes(PLAIN).expect("plain email parses");
    assert_eq!(msg.subject, "Quarterly Notes");
    assert_eq!(msg.message_id, "<plain-001@example.com>");
    assert!(msg.body.contains("body of a plain text email"));
    assert!(msg.body.contains("two lines worth"));
    assert_eq!(msg.from.as_deref(), Some("alice@example.com"));
    assert!(msg.attachments.is_empty());
}

#[test]
fn parse_html_only_email_strips_tags() {
    let msg = parse_email_bytes(HTML_ONLY).expect("html email parses");
    assert_eq!(msg.subject, "HTML Newsletter");
    assert!(msg.body.contains("Heading"));
    assert!(msg.body.contains("Paragraph one"));
    assert!(!msg.body.contains("<h1>"));
    assert!(!msg.body.contains("<p>"));
}

#[test]
fn parse_email_with_attachment_extracts_pdf_bytes() {
    let msg = parse_email_bytes(WITH_ATTACHMENT).expect("multipart email parses");
    assert_eq!(msg.subject, "Report Attached");
    assert!(msg.body.contains("attached report"));
    assert_eq!(msg.attachments.len(), 1, "应提取 1 个附件");
    let att = &msg.attachments[0];
    assert_eq!(att.filename, "report.pdf");
    assert!(att.content.starts_with(b"%PDF"), "附件应是解码后的 PDF 字节");
}

#[test]
fn parse_invalid_bytes_returns_err() {
    let result = parse_email_bytes(&[0xFF, 0xFE, 0x00, 0x01]);
    assert!(result.is_err());
}
