//! Email 采集源测试 —— 解析层离线测试（不连真 IMAP）+ 连接器测试（mock fetcher）。

use attune_core::ingest::email::{
    EmailConfig, EmailConnector, FetchedMail, ImapFetcher, parse_email_bytes,
};
use attune_core::ingest::{DocumentSink, RawDocument, SourceConnector, SourceKind};
use std::collections::HashMap;

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

/// 离线 mock：按 folder 返回预置邮件，模拟 IMAP UID SEARCH since_uid:* 语义。
struct MockImapFetcher {
    by_folder: HashMap<String, Vec<FetchedMail>>,
}

impl ImapFetcher for MockImapFetcher {
    fn fetch_since(&self, folder: &str, since_uid: u32) -> attune_core::error::Result<Vec<FetchedMail>> {
        let all = self.by_folder.get(folder).cloned().unwrap_or_default();
        // 模拟 IMAP UID SEARCH since_uid:* 语义 —— 只返回 UID > since_uid。
        Ok(all.into_iter().filter(|m| m.uid > since_uid).collect())
    }
}

fn config() -> EmailConfig {
    EmailConfig {
        host: "imap.example.com".into(),
        port: 993,
        username: "bob@example.com".into(),
        password: "pw".into(),
        folders: vec!["INBOX".into()],
    }
}

#[test]
fn connector_emits_one_rawdocument_per_email() {
    let mut by_folder = HashMap::new();
    by_folder.insert(
        "INBOX".to_string(),
        vec![FetchedMail { uid: 1, raw: PLAIN.to_vec() }],
    );
    let connector = EmailConnector::with_fetcher(config(), Box::new(MockImapFetcher { by_folder }));

    let mut docs: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
        connector.fetch_documents(&mut sink).unwrap();
    }
    assert_eq!(docs.len(), 1);
    let doc = &docs[0];
    assert_eq!(doc.source_kind, SourceKind::Email);
    assert_eq!(doc.title, "Quarterly Notes");
    assert_eq!(
        doc.source_ref, "<plain-001@example.com>.txt",
        "source_ref = Message-ID + .txt（.txt 后缀驱动 parser 走纯文本分支）"
    );
    assert_eq!(doc.modified_marker.as_deref(), Some("INBOX:1"), "增量标记 = folder:uid");
    assert_eq!(doc.metadata.get("from").map(String::as_str), Some("alice@example.com"));
    assert_eq!(doc.metadata.get("folder").map(String::as_str), Some("INBOX"));
}

#[test]
fn connector_emits_extra_rawdocument_per_attachment() {
    let mut by_folder = HashMap::new();
    by_folder.insert(
        "INBOX".to_string(),
        vec![FetchedMail { uid: 7, raw: WITH_ATTACHMENT.to_vec() }],
    );
    let connector = EmailConnector::with_fetcher(config(), Box::new(MockImapFetcher { by_folder }));

    let mut docs: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
        connector.fetch_documents(&mut sink).unwrap();
    }
    // 1 封邮件正文 + 1 个 PDF 附件 = 2 份 RawDocument。
    assert_eq!(docs.len(), 2);
    let attachment_doc = docs
        .iter()
        .find(|d| d.source_ref.contains("#att"))
        .expect("attachment doc exists");
    // source_ref = "{msg_id}#att{idx}/{filename}" —— #att 后缀防与正文/其它附件碰撞；
    // 末段是文件名，让 RawDocument::parse_filename 取到扩展名供 parser 路由。
    assert_eq!(attachment_doc.source_ref, "<att-003@example.com>#att0/report.pdf");
    assert!(attachment_doc.content.starts_with(b"%PDF"));
    assert_eq!(attachment_doc.parse_filename(), "report.pdf");
}

#[test]
fn connector_respects_since_uid_increment() {
    let mut by_folder = HashMap::new();
    by_folder.insert(
        "INBOX".to_string(),
        vec![
            FetchedMail { uid: 1, raw: PLAIN.to_vec() },
            FetchedMail { uid: 2, raw: HTML_ONLY.to_vec() },
        ],
    );
    let mut cfg = config();
    cfg.folders = vec!["INBOX".into()];
    let mut connector = EmailConnector::with_fetcher(cfg, Box::new(MockImapFetcher { by_folder }));
    connector.set_folder_since("INBOX", 1); // 只要 UID > 1

    let mut docs: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
        connector.fetch_documents(&mut sink).unwrap();
    }
    assert_eq!(docs.len(), 1, "UID=1 被增量游标跳过，只剩 UID=2");
    assert_eq!(docs[0].title, "HTML Newsletter");
}

#[test]
fn connector_body_rawdocument_ingests_successfully() {
    // 回归守卫：连接器产出的正文 RawDocument 必须能真正过 ingest_document 入库。
    // source_ref 必须带 .txt —— 否则 parse_filename 取到 "@domain.tld" 末段，
    // Path::extension() 解出非白名单扩展名，正文整封 ingest 失败。
    use attune_core::crypto::Key32;
    use attune_core::ingest::{ingest_document, IngestOutcome};
    use attune_core::store::Store;

    let mut by_folder = HashMap::new();
    by_folder.insert(
        "INBOX".to_string(),
        vec![FetchedMail { uid: 1, raw: PLAIN.to_vec() }],
    );
    let connector = EmailConnector::with_fetcher(config(), Box::new(MockImapFetcher { by_folder }));
    let mut docs: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
        connector.fetch_documents(&mut sink).unwrap();
    }
    assert_eq!(docs.len(), 1);

    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let outcome = ingest_document(&store, &dek, &docs[0])
        .expect("email body RawDocument must ingest, not Err");
    assert!(
        matches!(outcome, IngestOutcome::Inserted { .. }),
        "email body 应入库为 Inserted，实际 {outcome:?}"
    );
}
