//! Email IMAP 入库 capability。
//!
//! v0.7 scaffold：定义 `EmailProvider` trait + `MockEmailProvider` 实现 + 单测。
//! v0.8 真 IMAP：用 async-imap crate, OAuth2 for Gmail, App password for IMAP server。
//!
//! 使用方式（v0.8 设计）：
//! ```text
//! let provider = ImapEmailProvider::new(config).await?;
//! let messages = provider.list_messages(since_unix).await?;
//! for msg_meta in messages {
//!     let full = provider.fetch_message(msg_meta.uid).await?;
//!     // 入 vault: parser → embed → store
//! }
//! ```
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::future::Future;

/// IMAP 服务器连接配置。
///
/// password 字段在 vault 内应走 `crypto::encrypt_field` 加密存储，
/// 解密后传给 provider 构造函数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    /// IMAP 服务器主机名，例如 `imap.gmail.com` / `outlook.office365.com`
    pub server: String,
    /// 端口（IMAPS 一般 993，STARTTLS 一般 143）
    pub port: u16,
    /// 登录用户名（一般是 email 地址）
    pub username: String,
    /// 密码或 App Password（Gmail / Outlook 需开应用专用密码）
    pub password: String,
    /// 是否启用 TLS（建议 true）
    pub use_tls: bool,
    /// 监听的文件夹路径，例如 `INBOX` / `INBOX/Work`
    pub folder: String,
}

/// 单封邮件的应用层表示。
///
/// `body_text` 必填，`body_html` 可选（如有则 v0.8 走 HTML→纯文本清洗）。
/// `attachments` 仅存附件文件名，v0.8 真生产化时增加内容读取接口。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmailMessage {
    /// IMAP UID，folder 内唯一
    pub uid: u64,
    /// From 头解析后字符串，例如 `Alice <alice@example.com>`
    pub from: String,
    /// Subject 头（已 MIME 解码）
    pub subject: String,
    /// 纯文本正文（如原始邮件仅 HTML，v0.8 应用层负责清洗后填入）
    pub body_text: String,
    /// 可选 HTML 正文，保留给富格式展示
    pub body_html: Option<String>,
    /// Date 头转 unix 秒
    pub date_unix: i64,
    /// 附件文件名列表
    pub attachments: Vec<String>,
}

/// Email 入库 capability。
///
/// 注：使用 Rust 1.75+ 原生 `async fn in trait`。trait object (`dyn EmailProvider`)
/// 的 future Send-bound 在 stable 上目前需要调用方在具体类型上工作；如果未来需要
/// `Box<dyn EmailProvider>` 多态，再切换到 `#[async_trait]` macro。
pub trait EmailProvider: Send + Sync {
    /// 列出 `since_unix` 之后到达的邮件元信息（不下载正文/附件）。
    ///
    /// v0.8 真 IMAP：用 `SEARCH SINCE` 命令；当前 scaffold 返完整 message 简化。
    fn list_messages(
        &self,
        since_unix: i64,
    ) -> impl Future<Output = Result<Vec<EmailMessage>>> + Send;

    /// 拉取指定 UID 的完整邮件（含正文 + 附件文件名）。
    fn fetch_message(
        &self,
        uid: u64,
    ) -> impl Future<Output = Result<EmailMessage>> + Send;
}

/// 测试用 mock，返回 2 条 hardcoded 邮件。
///
/// 集成测试 / Web UI 演示可用此 provider 代替真实 IMAP。
pub struct MockEmailProvider;

impl MockEmailProvider {
    pub fn new() -> Self {
        Self
    }

    fn fixture() -> Vec<EmailMessage> {
        vec![
            EmailMessage {
                uid: 1001,
                from: "Alice <alice@example.com>".to_string(),
                subject: "Project kickoff meeting".to_string(),
                body_text: "Hi team, let's sync tomorrow at 10am.".to_string(),
                body_html: None,
                date_unix: 1_715_000_000,
                attachments: vec![],
            },
            EmailMessage {
                uid: 1002,
                from: "Bob <bob@example.com>".to_string(),
                subject: "Quarterly report attached".to_string(),
                body_text: "See attached Q1 report. -- Bob".to_string(),
                body_html: Some("<p>See attached Q1 report. -- Bob</p>".to_string()),
                date_unix: 1_715_086_400,
                attachments: vec!["q1_report.pdf".to_string()],
            },
        ]
    }
}

impl Default for MockEmailProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl EmailProvider for MockEmailProvider {
    async fn list_messages(&self, since_unix: i64) -> Result<Vec<EmailMessage>> {
        Ok(Self::fixture()
            .into_iter()
            .filter(|m| m.date_unix >= since_unix)
            .collect())
    }

    async fn fetch_message(&self, uid: u64) -> Result<EmailMessage> {
        Self::fixture()
            .into_iter()
            .find(|m| m.uid == uid)
            .ok_or_else(|| {
                crate::error::VaultError::Classification(format!(
                    "mock: email uid {uid} not found"
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_lists_all_messages_when_since_zero() {
        let p = MockEmailProvider::new();
        let msgs = p.list_messages(0).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].uid, 1001);
        assert_eq!(msgs[1].uid, 1002);
    }

    #[tokio::test]
    async fn mock_filters_by_since_unix() {
        let p = MockEmailProvider::new();
        // 只第二封 >= 1_715_086_400
        let msgs = p.list_messages(1_715_086_400).await.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].uid, 1002);
    }

    #[tokio::test]
    async fn mock_fetch_existing_uid() {
        let p = MockEmailProvider::new();
        let m = p.fetch_message(1002).await.unwrap();
        assert_eq!(m.subject, "Quarterly report attached");
        assert_eq!(m.attachments, vec!["q1_report.pdf".to_string()]);
    }

    #[tokio::test]
    async fn mock_fetch_unknown_uid_errors() {
        let p = MockEmailProvider::new();
        let err = p.fetch_message(9999).await;
        assert!(err.is_err());
    }

    #[test]
    fn config_roundtrips_json() {
        let cfg = EmailConfig {
            server: "imap.gmail.com".into(),
            port: 993,
            username: "u@example.com".into(),
            password: "app-pwd".into(),
            use_tls: true,
            folder: "INBOX".into(),
        };
        let j = serde_json::to_string(&cfg).unwrap();
        let back: EmailConfig = serde_json::from_str(&j).unwrap();
        assert_eq!(back.server, cfg.server);
        assert_eq!(back.port, 993);
        assert!(back.use_tls);
    }
}
