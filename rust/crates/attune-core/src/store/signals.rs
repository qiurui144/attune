//! skill_signals — 本地搜索失败信号（SkillClaw 风格自动技能进化）。
//!
//! 所有方法属于 `impl Store`（inherent impl 跨文件分裂，rustc 自动合并）。

use rusqlite::params;

use crate::error::Result;
use crate::store::Store;

#[allow(unused_imports)]
use crate::store::types::*;

impl Store {
    /// 记录一次本地搜索失败信号（非阻塞写入，失败时静默忽略）。
    /// 内部固定 kind='search_miss'（向后兼容旧调用方）。
    pub fn record_skill_signal(&self, query: &str, knowledge_count: usize, web_used: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO skill_signals (query, knowledge_count, web_used, kind) VALUES (?1, ?2, ?3, 'search_miss')",
            params![query, knowledge_count as i64, web_used as i64],
        )?;
        Ok(())
    }

    /// v0.7 自学习闭环 Phase B：通用信号事件。
    ///
    /// `kind` 已知值：
    /// - `search_miss` — 搜索 0 命中（原信号）
    /// - `doc_create` / `doc_update` / `doc_delete` — 文档生命周期
    /// - `citation_hit` — chat 引用某 chunk
    /// - `annotation_marker` — 用户加批注（⭐ 重点 / 🤔 存疑 / ❓ 不懂）
    /// - `click_through` / `dwell` — 行为反馈（保留扩展位）
    ///
    /// `ref_id` 通常是 item_id / annotation_id / chunk hash，便于 skill_evolution
    /// 反查上下文。
    pub fn record_signal_event(&self, kind: &str, ref_id: &str, query: Option<&str>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO skill_signals (query, knowledge_count, web_used, kind, ref_id)
             VALUES (?1, 0, 0, ?2, ?3)",
            params![query.unwrap_or(""), kind, ref_id],
        )?;
        Ok(())
    }

    /// 按 kind 过滤未处理信号数（skill_evolution 触发阈值判断用）。
    pub fn count_unprocessed_signals_by_kind(&self, kind: &str) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM skill_signals WHERE processed = 0 AND kind = ?1",
            params![kind],
            |row| row.get(0),
        )?;
        Ok(count as usize)
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
