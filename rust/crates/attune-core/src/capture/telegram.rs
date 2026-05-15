//! Telegram bot 入库 capability。
//!
//! v0.7 scaffold：定义 `TelegramProvider` trait + `MockTelegramProvider` 实现 + 单测。
//! v0.8 真 bot：用 teloxide crate + getUpdates polling（或 webhook）。
//!
//! 用户与 bot 私聊 / 把 bot 拉进群，bot token 写入 vault 配置，
//! attune 后台轮询 getUpdates，把消息走 parser → embed → store 流水线入 vault。
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::future::Future;

/// Telegram bot 连接配置。
///
/// `bot_token` 通过 BotFather 创建；`allowed_chat_ids` 是白名单，
/// 不在列表内的 chat 消息被 provider 丢弃（安全：避免任意人通过 bot 推送污染 vault）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// BotFather 颁发的 token（格式 `<bot_id>:<secret>`）
    pub bot_token: String,
    /// 允许入库的 chat_id 列表（私聊为正数，群为负数）
    pub allowed_chat_ids: Vec<i64>,
}

/// 单条 Telegram 消息的应用层表示。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramMessage {
    /// chat_id（私聊用户 id 或群 id）
    pub chat_id: i64,
    /// message_id（chat 内自增）
    pub message_id: i64,
    /// 发送者 username（无 @），匿名则为空串
    pub from_username: String,
    /// 文本正文（无文本的消息此处为空串）
    pub text: String,
    /// 消息发送时间 unix 秒
    pub date_unix: i64,
}

/// Telegram 入库 capability。
///
/// 仅 polling 模型；webhook 模型 v0.8 再加（需要公网入口）。
pub trait TelegramProvider: Send + Sync {
    /// 拉取自上次 offset 之后的新消息。
    ///
    /// 内部应维护 update_offset 状态（v0.8 持久化到 vault），
    /// 仅返回 `allowed_chat_ids` 命中的消息。
    fn poll_updates(&self) -> impl Future<Output = Result<Vec<TelegramMessage>>> + Send;
}

/// 测试用 mock，返回 2 条 hardcoded 消息。
pub struct MockTelegramProvider;

impl MockTelegramProvider {
    pub fn new() -> Self {
        Self
    }

    fn fixture() -> Vec<TelegramMessage> {
        vec![
            TelegramMessage {
                chat_id: 12345,
                message_id: 1,
                from_username: "alice".to_string(),
                text: "Hey, found a great article: https://example.com/article".to_string(),
                date_unix: 1_715_000_000,
            },
            TelegramMessage {
                chat_id: 12345,
                message_id: 2,
                from_username: "alice".to_string(),
                text: "也记一下今天的会议要点 ...".to_string(),
                date_unix: 1_715_086_400,
            },
        ]
    }
}

impl Default for MockTelegramProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl TelegramProvider for MockTelegramProvider {
    async fn poll_updates(&self) -> Result<Vec<TelegramMessage>> {
        Ok(Self::fixture())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_returns_two_messages() {
        let p = MockTelegramProvider::new();
        let msgs = p.poll_updates().await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].from_username, "alice");
        assert_eq!(msgs[0].chat_id, 12345);
    }

    #[tokio::test]
    async fn mock_preserves_chinese_text() {
        let p = MockTelegramProvider::new();
        let msgs = p.poll_updates().await.unwrap();
        assert!(msgs[1].text.contains("今天的会议要点"));
    }

    #[test]
    fn config_roundtrips_json() {
        let cfg = TelegramConfig {
            bot_token: "111:secret".into(),
            allowed_chat_ids: vec![12345, -100200300],
        };
        let j = serde_json::to_string(&cfg).unwrap();
        let back: TelegramConfig = serde_json::from_str(&j).unwrap();
        assert_eq!(back.bot_token, "111:secret");
        assert_eq!(back.allowed_chat_ids, vec![12345, -100200300]);
    }
}
