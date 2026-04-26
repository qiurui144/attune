//! conversations / conversation_messages — Chat 会话与消息（加密内容）。
//!
//! 所有方法属于 `impl Store`（inherent impl 跨文件分裂，rustc 自动合并）。

use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::store::Store;

#[allow(unused_imports)]
use crate::store::types::*;

impl Store {
    // ── Conversation Session CRUD ─────────────────────────────────────────────

    pub fn create_conversation(&self, dek: &Key32, title: &str) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let enc_title = crypto::encrypt(dek, title.as_bytes())?;
        self.conn.execute(
            "INSERT INTO conversations (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![id, enc_title, now],
        )?;
        Ok(id)
    }

    pub fn list_conversations(&self, dek: &Key32, limit: usize, offset: usize) -> Result<Vec<ConversationSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, created_at, updated_at FROM conversations
             ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit as i64, offset as i64], |row| {
            let enc_title: Vec<u8> = row.get(1)?;
            Ok((
                row.get::<_, String>(0)?,
                enc_title,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut results = Vec::new();
        for row in rows {
            let (id, enc_title, created_at, updated_at) = row.map_err(VaultError::Database)?;
            let title = String::from_utf8(crypto::decrypt(dek, &enc_title)?)
                .map_err(|e| VaultError::Crypto(format!("conversation title utf8: {e}")))?;
            results.push(ConversationSummary { id, title, created_at, updated_at });
        }
        Ok(results)
    }

    pub fn get_conversation_messages(&self, dek: &Key32, conv_id: &str) -> Result<Vec<ConvMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, role, content, citations, created_at
             FROM conversation_messages
             WHERE conversation_id = ?1 ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(params![conv_id], |row| {
            let enc_content: Vec<u8> = row.get(2)?;
            let citations_json: Option<String> = row.get(3)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                enc_content,
                citations_json,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut results = Vec::new();
        for row in rows {
            let (id, role, enc_content, citations_json, created_at) = row.map_err(VaultError::Database)?;
            let content = String::from_utf8(crypto::decrypt(dek, &enc_content)?)
                .map_err(|e| VaultError::Crypto(format!("conversation message utf8: {e}")))?;
            let citations: Vec<Citation> = citations_json
                .and_then(|j| serde_json::from_str::<Vec<Citation>>(&j).ok())
                .unwrap_or_default();
            results.push(ConvMessage { id, role, content, citations, created_at });
        }
        Ok(results)
    }

    pub fn append_message(
        &self,
        dek: &Key32,
        conv_id: &str,
        role: &str,
        content: &str,
        citations: &[Citation],
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let enc = crypto::encrypt(dek, content.as_bytes())?;
        let citations_json: Option<String> = if citations.is_empty() {
            None
        } else {
            Some(serde_json::to_string(citations)
                .map_err(VaultError::Json)?)
        };
        self.conn.execute(
            "INSERT INTO conversation_messages (id, conversation_id, role, content, citations, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, conv_id, role, enc, citations_json, now],
        )?;
        // Update conversation updated_at
        self.conn.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![now, conv_id],
        )?;
        Ok(id)
    }

    /// 在单一事务中写入 user + assistant 一对消息，保证原子性。
    /// 若 user 写入成功但 assistant 失败，事务回滚，两条均不写入。
    pub fn append_conversation_turn(
        &self,
        dek: &Key32,
        conv_id: &str,
        user_content: &str,
        assistant_content: &str,
        citations: &[Citation],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

        // user message
        let user_enc = crypto::encrypt(dek, user_content.as_bytes())?;
        let user_id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO conversation_messages (id, conversation_id, role, content, citations, created_at)
             VALUES (?1, ?2, 'user', ?3, NULL, ?4)",
            params![user_id, conv_id, user_enc, now],
        )?;

        // assistant message
        let asst_enc = crypto::encrypt(dek, assistant_content.as_bytes())?;
        let asst_id = uuid::Uuid::new_v4().to_string();
        let citations_json: Option<String> = if citations.is_empty() {
            None
        } else {
            Some(serde_json::to_string(citations).map_err(VaultError::Json)?)
        };
        tx.execute(
            "INSERT INTO conversation_messages (id, conversation_id, role, content, citations, created_at)
             VALUES (?1, ?2, 'assistant', ?3, ?4, ?5)",
            params![asst_id, conv_id, asst_enc, citations_json, now],
        )?;

        // update conversation timestamp
        tx.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![now, conv_id],
        )?;

        tx.commit()?;
        Ok(())
    }

    pub fn delete_conversation(&self, conv_id: &str) -> Result<()> {
        // CASCADE 会自动删 conversation_messages
        self.conn.execute("DELETE FROM conversations WHERE id = ?1", params![conv_id])?;
        Ok(())
    }

    pub fn get_conversation_by_id(&self, dek: &Key32, conv_id: &str) -> Result<Option<ConversationSummary>> {
        use rusqlite::OptionalExtension;
        let row = self.conn
            .query_row(
                "SELECT id, title, created_at, updated_at FROM conversations WHERE id = ?1",
                params![conv_id],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                )),
            )
            .optional()
            .map_err(VaultError::Database)?;
        match row {
            Some((id, enc_title, created_at, updated_at)) => {
                let title = String::from_utf8(crypto::decrypt(dek, &enc_title)?)
                .map_err(|e| VaultError::Crypto(format!("conversation title utf8: {e}")))?;
                Ok(Some(ConversationSummary { id, title, created_at, updated_at }))
            }
            None => Ok(None),
        }
    }
}
