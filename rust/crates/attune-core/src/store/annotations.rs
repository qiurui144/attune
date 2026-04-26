//! annotations — 用户手动批注 + 未来 AI 分析批注 CRUD。
//!
//! 成本/触发契约：所有批注 CRUD 都是 🆓 零成本 / 用户显式操作。不在建库流水线里
//! 自动生成批注。AI 批注（source='ai'）由独立的"AI 分析"按钮触发，属于 💰 层。
//!
//! Annotation / AnnotationInput 结构定义在 store/types.rs。

use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::store::Store;

#[allow(unused_imports)]
use crate::store::types::*;

impl Store {
    /// 创建批注。生成 UUID，content 字段加密保存（保护个人思考）。
    /// offset_start/offset_end 由调用方验证不越界（routes 层做 item 长度校验）。
    pub fn create_annotation(
        &self,
        dek: &Key32,
        item_id: &str,
        input: &AnnotationInput,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let source = input.source.as_deref().unwrap_or("user");
        if !matches!(source, "user" | "ai") {
            return Err(VaultError::InvalidInput(format!(
                "source must be 'user' or 'ai', got: {source}"
            )));
        }
        let content_enc = crypto::encrypt(dek, input.content.as_bytes())?;
        self.conn.execute(
            "INSERT INTO annotations
                (id, item_id, offset_start, offset_end, text_snippet,
                 label, color, content, source, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now'), datetime('now'))",
            params![
                id,
                item_id,
                input.offset_start,
                input.offset_end,
                input.text_snippet,
                input.label,
                input.color,
                content_enc,
                source,
            ],
        )?;
        Ok(id)
    }

    /// 列出某条目的所有批注（按 offset 升序；越靠前的段落先显示）。
    /// 过滤软删除的 item —— 虽然 delete_item 现在会连坐删批注，但历史遗留数据可能存在
    /// 孤立批注（或未来测试路径绕过 delete_item），JOIN-filter 保底。
    pub fn list_annotations(&self, dek: &Key32, item_id: &str) -> Result<Vec<Annotation>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.id, a.item_id, a.offset_start, a.offset_end, a.text_snippet,
                    a.label, a.color, a.content, a.source, a.created_at, a.updated_at
             FROM annotations a
             JOIN items i ON i.id = a.item_id
             WHERE a.item_id = ?1 AND i.is_deleted = 0
             ORDER BY a.offset_start ASC, a.created_at ASC",
        )?;
        let rows = stmt.query_map(params![item_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Vec<u8>>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, item_id, os, oe, snippet, label, color, content_enc, source, created, updated) = r?;
            let content = crypto::decrypt(dek, &content_enc)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            out.push(Annotation {
                id, item_id,
                offset_start: os, offset_end: oe,
                text_snippet: snippet, label, color, content, source,
                created_at: created, updated_at: updated,
            });
        }
        Ok(out)
    }

    /// 编辑批注。用户手动编辑会把 source 强制置回 'user'（契约：
    /// 任何人类介入都抹掉 AI 标记，避免让用户误以为 AI 参与了最终版本）。
    pub fn update_annotation(
        &self,
        dek: &Key32,
        id: &str,
        input: &AnnotationInput,
    ) -> Result<()> {
        let content_enc = crypto::encrypt(dek, input.content.as_bytes())?;
        // 若调用方明确传 source='ai'（AI 工作流的第二次写入），尊重之；否则回到 user
        let source = input.source.as_deref().unwrap_or("user");
        if !matches!(source, "user" | "ai") {
            return Err(VaultError::InvalidInput(format!(
                "source must be 'user' or 'ai', got: {source}"
            )));
        }
        let n = self.conn.execute(
            "UPDATE annotations
             SET label = ?1, color = ?2, content = ?3, source = ?4,
                 updated_at = datetime('now')
             WHERE id = ?5",
            params![input.label, input.color, content_enc, source, id],
        )?;
        if n == 0 {
            return Err(VaultError::InvalidInput(format!("annotation {id} not found")));
        }
        Ok(())
    }

    /// 删除批注（硬删除，不走软删除 — 个人场景无合规留痕需求）
    pub fn delete_annotation(&self, id: &str) -> Result<()> {
        let n = self.conn.execute("DELETE FROM annotations WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(VaultError::InvalidInput(format!("annotation {id} not found")));
        }
        Ok(())
    }

    /// 统计某条目的批注数（用于 UI 指示，避免拉全部内容）
    pub fn count_annotations(&self, item_id: &str) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM annotations WHERE item_id = ?1",
            params![item_id],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }
}
