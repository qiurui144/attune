//! suggestion_cards / suggestion_mutes — 建议卡 dismiss + per-kind mute 持久化。
//!
//! 建议卡本身**不落库生成内容**(它由 [`crate::suggestions::evaluate`] 每次按当前信号
//! 实时重算,零成本)。这里只持久化两类用户决策:
//! - `suggestion_dismissed`:用户忽略过的卡 signature(去重 + 不再打扰)。
//! - `suggestion_mutes`:用户永久关闭的 kind。
//!
//! 这两张表只存**非敏感**的 signature/kind 字符串(无文件内容、无 secret),故**不加密**
//! (与 skill_signals 同级)。生命周期:dismiss 90 天后由 cleanup 清(signature 会随信号
//! 变化失效);mute 永久,直到用户 unmute。
//!
//! per spec docs/superpowers/specs/2026-06-17-suggestions-and-thirdparty-accounts.md
//! 能力 A §3 DB tables。

use rusqlite::params;

use crate::error::Result;
use crate::store::Store;

impl Store {
    /// 记录一次 dismiss(幂等:重复 dismiss 同 signature 不报错)。
    pub fn dismiss_suggestion(&self, signature: &str) -> Result<()> {
        if signature.is_empty() || signature.len() > 128 {
            return Err(crate::error::VaultError::InvalidInput(
                "signature empty or too long (max 128)".into(),
            ));
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO suggestion_dismissed (signature, dismissed_at) \
             VALUES (?1, datetime('now'))",
            params![signature],
        )?;
        Ok(())
    }

    /// 读取所有已 dismiss 的 signature(供 evaluate 过滤)。
    pub fn list_dismissed_signatures(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT signature FROM suggestion_dismissed")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| e.into())
    }

    /// 永久关闭某 kind(幂等)。`kind` 必须是已知的 SuggestionKind kebab 字符串。
    pub fn mute_suggestion_kind(&self, kind: &str) -> Result<()> {
        if crate::suggestions::SuggestionKind::parse(kind).is_none() {
            return Err(crate::error::VaultError::InvalidInput(format!(
                "unknown suggestion kind: {kind:?}"
            )));
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO suggestion_mutes (kind, muted_at) VALUES (?1, datetime('now'))",
            params![kind],
        )?;
        Ok(())
    }

    /// 取消某 kind 的 mute(幂等)。
    pub fn unmute_suggestion_kind(&self, kind: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM suggestion_mutes WHERE kind = ?1",
            params![kind],
        )?;
        Ok(())
    }

    /// 读取所有 muted kind(供 evaluate 过滤)。未知/损坏 kind 静默跳过。
    pub fn list_muted_suggestion_kinds(&self) -> Result<Vec<crate::suggestions::SuggestionKind>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT kind FROM suggestion_mutes")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            let kind = row?;
            if let Some(k) = crate::suggestions::SuggestionKind::parse(&kind) {
                out.push(k);
            }
        }
        Ok(out)
    }

    /// 聚合反复落空的查询:对 search_miss 信号按 query 分组计数,返回 count ≥
    /// `min_count` 的前 `limit` 个(降序)。零成本纯 SQL,供建议引擎 enrich 规则用。
    ///
    /// 空 query 不计(record_signal_event 的非 search_miss kind query 为空)。
    pub fn aggregate_missed_queries(
        &self,
        min_count: u32,
        limit: usize,
    ) -> Result<Vec<crate::suggestions::MissedQuery>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT query, COUNT(*) AS c FROM skill_signals \
             WHERE kind = 'search_miss' AND query <> '' \
             GROUP BY query HAVING c >= ?1 ORDER BY c DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![min_count as i64, limit as i64], |r| {
            Ok(crate::suggestions::MissedQuery {
                query: r.get::<_, String>(0)?,
                miss_count: r.get::<_, i64>(1)?.max(0) as u32,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| e.into())
    }

    /// 清理超过 `days_threshold` 天的 dismiss 记录(signature 会随信号变化失效,
    /// 长期保留无意义)。mute 永久不清。返回删除行数。
    pub fn purge_dismissed_suggestions_older_than_days(&self, days_threshold: u32) -> Result<usize> {
        let modifier = format!("-{days_threshold} days");
        let n = self.conn.execute(
            "DELETE FROM suggestion_dismissed WHERE dismissed_at < datetime('now', ?1)",
            params![modifier],
        )?;
        Ok(n)
    }

    /// suggestion_dismissed / suggestion_mutes 表迁移(幂等)。
    /// 纯追加表:老 vault 下次 open 自动建空表,SCHEMA_VERSION 不 bump。
    pub(crate) fn migrate_suggestions(conn: &rusqlite::Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS suggestion_dismissed (\
               signature TEXT PRIMARY KEY, dismissed_at TEXT NOT NULL);\
             CREATE TABLE IF NOT EXISTS suggestion_mutes (\
               kind TEXT PRIMARY KEY, muted_at TEXT NOT NULL);",
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_memory().unwrap()
    }

    #[test]
    fn dismiss_roundtrip_and_idempotent() {
        let s = store();
        s.dismiss_suggestion("organize-abc").unwrap();
        s.dismiss_suggestion("organize-abc").unwrap(); // 幂等
        s.dismiss_suggestion("enrich-xyz").unwrap();
        let mut got = s.list_dismissed_signatures().unwrap();
        got.sort();
        assert_eq!(got, vec!["enrich-xyz", "organize-abc"]);
    }

    #[test]
    fn dismiss_rejects_empty_and_overlong() {
        let s = store();
        assert!(s.dismiss_suggestion("").is_err());
        assert!(s.dismiss_suggestion(&"x".repeat(129)).is_err());
    }

    #[test]
    fn mute_unmute_roundtrip() {
        let s = store();
        s.mute_suggestion_kind("organize").unwrap();
        s.mute_suggestion_kind("organize").unwrap(); // 幂等
        s.mute_suggestion_kind("profile").unwrap();
        let mut got: Vec<_> = s
            .list_muted_suggestion_kinds()
            .unwrap()
            .iter()
            .map(|k| k.as_str())
            .collect();
        got.sort();
        assert_eq!(got, vec!["organize", "profile"]);
        s.unmute_suggestion_kind("organize").unwrap();
        let got: Vec<_> = s
            .list_muted_suggestion_kinds()
            .unwrap()
            .iter()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(got, vec!["profile"]);
    }

    #[test]
    fn mute_rejects_unknown_kind() {
        let s = store();
        assert!(s.mute_suggestion_kind("bogus").is_err());
        // typo guard: 即使 INSERT 没跑,表里也不该有它。
        assert!(s.list_muted_suggestion_kinds().unwrap().is_empty());
    }

    #[test]
    fn unmute_nonexistent_is_noop() {
        let s = store();
        s.unmute_suggestion_kind("organize").unwrap(); // 不存在,幂等无副作用
        assert!(s.list_muted_suggestion_kinds().unwrap().is_empty());
    }

    #[test]
    fn purge_removes_only_old_dismissed() {
        let s = store();
        // 新插一条(now),手动插一条 100 天前的。
        s.dismiss_suggestion("fresh").unwrap();
        s.conn
            .execute(
                "INSERT INTO suggestion_dismissed (signature, dismissed_at) \
                 VALUES ('stale', datetime('now', '-100 days'))",
                [],
            )
            .unwrap();
        let removed = s.purge_dismissed_suggestions_older_than_days(90).unwrap();
        assert_eq!(removed, 1);
        let got = s.list_dismissed_signatures().unwrap();
        assert_eq!(got, vec!["fresh"]);
    }

    #[test]
    fn aggregate_missed_queries_groups_and_filters() {
        let s = store();
        // "rust gc" 落空 3 次,"async" 1 次。
        for _ in 0..3 {
            s.record_skill_signal("rust gc", 0, false).unwrap();
        }
        s.record_skill_signal("async", 0, false).unwrap();
        // 一条非 search_miss 信号(空 query)不该混进来。
        s.record_signal_event("doc_create", "item-1", None).unwrap();

        let got = s.aggregate_missed_queries(3, 10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].query, "rust gc");
        assert_eq!(got[0].miss_count, 3);

        // min_count=1 → 两条都出,降序。
        let all = s.aggregate_missed_queries(1, 10).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].query, "rust gc"); // 3 > 1
    }

    #[test]
    fn corrupted_kind_in_table_skipped() {
        let s = store();
        // 直接塞一个非法 kind(模拟数据损坏 / 老版本写入)。
        s.conn
            .execute(
                "INSERT INTO suggestion_mutes (kind, muted_at) VALUES ('legacy_garbage', datetime('now'))",
                [],
            )
            .unwrap();
        s.mute_suggestion_kind("enrich").unwrap();
        // list 跳过损坏 kind,只返回合法的。
        let got: Vec<_> = s
            .list_muted_suggestion_kinds()
            .unwrap()
            .iter()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(got, vec!["enrich"]);
    }
}
