//! skill_signals — 本地搜索失败信号（SkillClaw 风格自动技能进化）。
//!
//! 所有方法属于 `impl Store`（inherent impl 跨文件分裂，rustc 自动合并）。

use rusqlite::params;

use crate::error::Result;
use crate::store::Store;

#[allow(unused_imports)]
use crate::store::types::*;

impl Store {
    /// 记录一次本地搜索失败信号（非阻塞写入，失败时静默忽略）
    pub fn record_skill_signal(&self, query: &str, knowledge_count: usize, web_used: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO skill_signals (query, knowledge_count, web_used) VALUES (?1, ?2, ?3)",
            params![query, knowledge_count as i64, web_used as i64],
        )?;
        Ok(())
    }

    /// 获取未处理的失败信号数量
    pub fn count_unprocessed_signals(&self) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM skill_signals WHERE processed = 0",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// 取出最近 N 条未处理信号
    pub fn get_unprocessed_signals(&self, limit: usize) -> Result<Vec<SkillSignal>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, query, knowledge_count, web_used, created_at
             FROM skill_signals WHERE processed = 0
             ORDER BY created_at ASC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(SkillSignal {
                id: row.get(0)?,
                query: row.get(1)?,
                knowledge_count: row.get::<_, i64>(2)? as usize,
                web_used: row.get::<_, i64>(3)? != 0,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(|e| e.into())
    }

    /// 标记一批信号为已处理
    pub fn mark_signals_processed(&self, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        for id in ids {
            self.conn.execute(
                "UPDATE skill_signals SET processed = 1 WHERE id = ?1",
                params![id],
            )?;
        }
        Ok(())
    }
}
