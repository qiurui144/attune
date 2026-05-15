//! skill_signals — 本地搜索失败信号（SkillClaw 风格自动技能进化）。
//!
//! 所有方法属于 `impl Store`（inherent impl 跨文件分裂，rustc 自动合并）。

use rusqlite::params;

use crate::error::Result;
use crate::store::Store;

#[allow(unused_imports)]
use crate::store::types::*;

/// R6 P1-4 fix: 已知 skill_signals kind 允许集。新加 kind 必须先入此白名单。
/// 拒绝 typo 静默写入 unknown kind 让 `count_unprocessed_signals_by_kind` 永远 0.
const KNOWN_SIGNAL_KINDS: &[&str] = &[
    "search_miss",
    "doc_create",
    "doc_update",
    "doc_delete",
    "citation_hit",
    "annotation_marker",
    "click_through",
    "dwell",
];

fn is_known_signal_kind(kind: &str) -> bool {
    KNOWN_SIGNAL_KINDS.contains(&kind)
}

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
    /// `kind` 必须从已知集选（拒绝 typo 静默写入 unknown kind 污染 skill_evolution 计数）：
    /// - `search_miss` — 搜索 0 命中（原信号）
    /// - `doc_create` / `doc_update` / `doc_delete` — 文档生命周期
    /// - `citation_hit` — chat 引用某 chunk
    /// - `annotation_marker` — 用户加批注（⭐ 重点 / 🤔 存疑 / ❓ 不懂）
    /// - `click_through` / `dwell` — 行为反馈（保留扩展位）
    ///
    /// `ref_id` 通常是 item_id / annotation_id / chunk hash，便于 skill_evolution
    /// 反查上下文。
    ///
    /// 未知 kind 返回 `VaultError::InvalidInput`，caller 应保证 kind 在编译期已知
    /// （建议封装成 const & str 或枚举）。
    pub fn record_signal_event(&self, kind: &str, ref_id: &str, query: Option<&str>) -> Result<()> {
        if !is_known_signal_kind(kind) {
            return Err(crate::error::VaultError::Crypto(format!(
                "unknown skill_signals kind: {kind:?} (R6 P1-4 fix: typo guard)"
            )));
        }
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

    /// 获取未处理的失败信号数量。
    ///
    /// R17 P0 fix (S4-Q1): 同 `get_unprocessed_signals` — 仅计 search_miss kind，
    /// 让 evolver 触发阈值不被 Phase B 信号污染。
    pub fn count_unprocessed_signals(&self) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM skill_signals WHERE processed = 0 AND kind = 'search_miss'",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// 取出最近 N 条未处理 search_miss 信号（evolver 主消费路径）。
    ///
    /// R17 P0 fix (S4-Q1): 之前不按 kind 过滤 → Phase B 加 doc_*/citation_hit/
    /// annotation_marker 后，evolver 会拉到这些 kind 但其 query 字段为空，
    /// LLM prompt "近期失败查询" 列表充斥空字符串 → 扩展词学习被污染 + 浪费 token。
    /// 现在强制 `kind='search_miss'` 让 evolver 只看真正的搜索失败信号；
    /// 其他 kind 信号留给未来 dedicated consumer（如 citation-based skill 强化）。
    pub fn get_unprocessed_signals(&self, limit: usize) -> Result<Vec<SkillSignal>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, query, knowledge_count, web_used, created_at
             FROM skill_signals
             WHERE processed = 0 AND kind = 'search_miss'
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

    /// 标记一批信号为已处理。
    ///
    /// R6 P1-7 fix: 包在 `unchecked_transaction` 里，避免半批失败留下"部分 processed
    /// + 部分未 processed"的悬而未决状态 — caller 重试会重复处理已 mark 的子集。
    pub fn mark_signals_processed(&self, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        for id in ids {
            tx.execute(
                "UPDATE skill_signals SET processed = 1 WHERE id = ?1",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
