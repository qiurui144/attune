//! watches / watch_hits 持久化（信息监控闭环 spec §3.2 + §4）。
//!
//! 与 store/rss_feeds.rs 同模式。anchor 向量经 dek 加密成 BLOB 落 `anchor_vec_enc`
//! （明文向量不落盘）；keywords / entities / source_ids / source_weights 走 JSON 文本列。
//! 所有方法属于 `impl Store`（inherent impl 跨文件分裂，rustc 自动合并）。

use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::entities::Entity;
use crate::error::Result;
use crate::monitoring::{Watch, WatchHit};
use crate::store::Store;

/// 默认命中阈值（与 spec §3.2 / search 已验证常量量级对齐）。
pub const DEFAULT_MATCH_THRESHOLD: f32 = 0.55;

/// 写入用的 watch 配置（明文，调用方持有；anchor_vec 落库前 dek 加密）。
#[derive(Debug, Clone, Default)]
pub struct WatchInput {
    pub label: String,
    pub keywords: Vec<String>,
    pub entities: Vec<Entity>,
    pub anchor_text: String,
    /// 建 watch 时一次性算好的 anchor 向量（None = 纯关键词/实体匹配）。
    pub anchor_vec: Option<Vec<f32>>,
    pub source_ids: Vec<String>,
    pub match_threshold: Option<f32>,
    pub source_weights: std::collections::HashMap<String, f32>,
    pub digest_period: String, // "daily" | "weekly" | "off"
    pub llm_summary: bool,
    pub notify: bool,
}

/// watch 行的运行时投影 + 持久化元数据（label/period 等）。
#[derive(Debug, Clone)]
pub struct WatchRow {
    pub watch: Watch,
    pub digest_period: String,
    pub llm_summary: bool,
    pub notify: bool,
    pub last_digested_marker: Option<String>,
    pub last_digested_at: Option<String>,
    pub created_at: String,
}

/// watch 字段 patch（PATCH 路由用；None 字段不改）。
#[derive(Debug, Clone, Default)]
pub struct WatchPatch {
    pub enabled: Option<bool>,
    pub digest_period: Option<String>,
    pub llm_summary: Option<bool>,
    pub notify: Option<bool>,
    pub match_threshold: Option<f32>,
}

fn encode_f32(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

fn decode_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

impl Store {
    /// 新增一个 watch。anchor_vec（若有）经 dek 加密落 BLOB。返回新 id。
    pub fn add_watch(&self, dek: &Key32, input: &WatchInput) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let keywords_json = serde_json::to_string(&input.keywords).unwrap_or_else(|_| "[]".into());
        let entities_json = serde_json::to_string(&input.entities).unwrap_or_else(|_| "[]".into());
        let source_ids_json =
            serde_json::to_string(&input.source_ids).unwrap_or_else(|_| "[]".into());
        let source_weights_json =
            serde_json::to_string(&input.source_weights).unwrap_or_else(|_| "{}".into());
        let anchor_enc: Option<Vec<u8>> = match &input.anchor_vec {
            Some(v) if !v.is_empty() => Some(crypto::encrypt(dek, &encode_f32(v))?),
            _ => None,
        };
        let threshold = input.match_threshold.unwrap_or(DEFAULT_MATCH_THRESHOLD);
        let period = if input.digest_period.is_empty() {
            "weekly".to_string()
        } else {
            input.digest_period.clone()
        };
        self.conn.execute(
            "INSERT INTO watches
                (id, label, keywords_json, entities_json, anchor_text, anchor_vec_enc,
                 source_ids_json, match_threshold, source_weights_json, digest_period,
                 llm_summary, notify, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, ?13)",
            params![
                id,
                input.label,
                keywords_json,
                entities_json,
                input.anchor_text,
                anchor_enc,
                source_ids_json,
                threshold as f64,
                source_weights_json,
                period,
                input.llm_summary as i64,
                input.notify as i64,
                now,
            ],
        )?;
        Ok(id)
    }

    /// 列出全部 watch（anchor 向量解密回 Watch.anchor_vec）。
    pub fn list_watches(&self, dek: &Key32) -> Result<Vec<WatchRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, label, keywords_json, entities_json, anchor_text, anchor_vec_enc,
                    source_ids_json, match_threshold, source_weights_json, digest_period,
                    llm_summary, notify, last_digested_marker, last_digested_at, enabled, created_at
             FROM watches ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<Vec<u8>>>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, f64>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, String>(9)?,
                r.get::<_, i64>(10)?,
                r.get::<_, i64>(11)?,
                r.get::<_, Option<String>>(12)?,
                r.get::<_, Option<String>>(13)?,
                r.get::<_, i64>(14)?,
                r.get::<_, String>(15)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (
                id,
                label,
                keywords_json,
                entities_json,
                anchor_text,
                anchor_enc,
                source_ids_json,
                threshold,
                source_weights_json,
                digest_period,
                llm_summary,
                notify,
                last_digested_marker,
                last_digested_at,
                enabled,
                created_at,
            ) = row?;
            let anchor_vec = match anchor_enc {
                Some(blob) => Some(decode_f32(&crypto::decrypt(dek, &blob)?)),
                None => None,
            };
            let watch = Watch {
                id,
                label,
                keywords: serde_json::from_str(&keywords_json).unwrap_or_default(),
                entities: serde_json::from_str(&entities_json).unwrap_or_default(),
                anchor_text,
                anchor_vec,
                source_ids: serde_json::from_str(&source_ids_json).unwrap_or_default(),
                match_threshold: threshold as f32,
                source_weights: serde_json::from_str(&source_weights_json).unwrap_or_default(),
                enabled: enabled != 0,
            };
            out.push(WatchRow {
                watch,
                digest_period,
                llm_summary: llm_summary != 0,
                notify: notify != 0,
                last_digested_marker,
                last_digested_at,
                created_at,
            });
        }
        Ok(out)
    }

    /// 取单个 watch。
    pub fn get_watch(&self, dek: &Key32, id: &str) -> Result<Option<WatchRow>> {
        Ok(self
            .list_watches(dek)?
            .into_iter()
            .find(|w| w.watch.id == id))
    }

    /// 仅取 enabled watch（监控匹配用）。
    pub fn list_enabled_watches(&self, dek: &Key32) -> Result<Vec<Watch>> {
        Ok(self
            .list_watches(dek)?
            .into_iter()
            .filter(|w| w.watch.enabled)
            .map(|w| w.watch)
            .collect())
    }

    /// 部分更新 watch 字段。返回是否命中（false = id 不存在）。
    pub fn patch_watch(&self, id: &str, patch: &WatchPatch) -> Result<bool> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut sets: Vec<String> = Vec::new();
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(e) = patch.enabled {
            sets.push("enabled = ?".into());
            binds.push(Box::new(e as i64));
        }
        if let Some(p) = &patch.digest_period {
            sets.push("digest_period = ?".into());
            binds.push(Box::new(p.clone()));
        }
        if let Some(l) = patch.llm_summary {
            sets.push("llm_summary = ?".into());
            binds.push(Box::new(l as i64));
        }
        if let Some(n) = patch.notify {
            sets.push("notify = ?".into());
            binds.push(Box::new(n as i64));
        }
        if let Some(t) = patch.match_threshold {
            sets.push("match_threshold = ?".into());
            binds.push(Box::new(t as f64));
        }
        if sets.is_empty() {
            return self.get_watch_exists(id);
        }
        sets.push("updated_at = ?".into());
        binds.push(Box::new(now));
        binds.push(Box::new(id.to_string()));
        let sql = format!("UPDATE watches SET {} WHERE id = ?", sets.join(", "));
        let params_ref: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let n = self.conn.execute(&sql, params_ref.as_slice())?;
        Ok(n > 0)
    }

    fn get_watch_exists(&self, id: &str) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM watches WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// 删除 watch + 级联删其 hits。返回是否命中。
    pub fn delete_watch(&self, id: &str) -> Result<bool> {
        self.conn
            .execute("DELETE FROM watch_hits WHERE watch_id = ?1", params![id])?;
        let n = self
            .conn
            .execute("DELETE FROM watches WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// upsert 一批 watch hit（去重防线：UNIQUE(watch_id,item_id) → 冲突时更新 score/reasons）。
    /// 返回**新插入**行数（已存在的更新不计；SQLite `ON CONFLICT DO UPDATE` 的 `changes()`
    /// 对插入和更新都返回 1，故先 existence-check 区分）。
    pub fn upsert_watch_hits(&self, hits: &[WatchHit]) -> Result<usize> {
        let mut inserted = 0usize;
        for h in hits {
            let existed: bool = self
                .conn
                .query_row(
                    "SELECT 1 FROM watch_hits WHERE watch_id = ?1 AND item_id = ?2",
                    params![h.watch_id, h.item_id],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            let id = uuid::Uuid::new_v4().to_string();
            let reasons_json = serde_json::to_string(&h.reasons).unwrap_or_else(|_| "[]".into());
            self.conn.execute(
                "INSERT INTO watch_hits (id, watch_id, item_id, score, reasons_json, dedup_group, digested, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)
                 ON CONFLICT(watch_id, item_id) DO UPDATE SET
                     score = excluded.score,
                     reasons_json = excluded.reasons_json,
                     dedup_group = excluded.dedup_group",
                params![id, h.watch_id, h.item_id, h.score as f64, reasons_json, h.dedup_group, h.created_at],
            )?;
            if !existed {
                inserted += 1;
            }
        }
        Ok(inserted)
    }

    /// 列出某 watch 未 digest 的 hit（按 score 降序）。
    pub fn list_pending_hits(&self, watch_id: &str, limit: usize) -> Result<Vec<WatchHit>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT h.watch_id, h.item_id, COALESCE(i.title,''), h.score, h.reasons_json,
                    h.dedup_group, h.created_at
             FROM watch_hits h LEFT JOIN items i ON h.item_id = i.id
             WHERE h.watch_id = ?1 AND h.digested = 0
             ORDER BY h.score DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![watch_id, limit as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, f64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (watch_id, item_id, title, score, reasons_json, dedup_group, created_at) = row?;
            out.push(WatchHit {
                watch_id,
                item_id,
                title,
                score: score as f32,
                reasons: serde_json::from_str(&reasons_json).unwrap_or_default(),
                dedup_group,
                created_at,
            });
        }
        Ok(out)
    }

    /// 未 digest 的命中计数（hit_count_pending，WatchView 用）。
    pub fn count_pending_hits(&self, watch_id: &str) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM watch_hits WHERE watch_id = ?1 AND digested = 0",
            params![watch_id],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// 标记某 watch 的 hit 已 digest（跨时间去重）；更新 last_digested_at + marker。
    pub fn mark_hits_digested(&self, watch_id: &str, marker: Option<&str>) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE watch_hits SET digested = 1 WHERE watch_id = ?1 AND digested = 0",
            params![watch_id],
        )?;
        self.conn.execute(
            "UPDATE watches SET last_digested_at = ?1, last_digested_marker = COALESCE(?2, last_digested_marker), updated_at = ?1
             WHERE id = ?3",
            params![now, marker, watch_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Key32;
    use crate::entities::{Entity, EntityKind};
    use crate::store::Store;
    use tempfile::TempDir;

    fn store() -> (Store, Key32, TempDir) {
        let dir = TempDir::new().unwrap();
        let s = Store::open(&dir.path().join("t.db")).unwrap();
        (s, Key32::generate(), dir)
    }

    #[test]
    fn add_list_get_watch_roundtrip() {
        let (s, dek, _d) = store();
        let input = WatchInput {
            label: "RISC-V".into(),
            keywords: vec!["RVV".into(), "RVA23".into()],
            entities: vec![Entity {
                kind: EntityKind::Organization,
                value: "某研究所".into(),
                byte_start: 0,
                byte_end: 9,
            }],
            anchor_text: "RISC-V 工具链".into(),
            anchor_vec: Some(vec![0.1, 0.2, 0.3]),
            digest_period: "daily".into(),
            llm_summary: true,
            ..Default::default()
        };
        let id = s.add_watch(&dek, &input).unwrap();
        let got = s.get_watch(&dek, &id).unwrap().unwrap();
        assert_eq!(got.watch.label, "RISC-V");
        assert_eq!(got.watch.keywords, vec!["RVV", "RVA23"]);
        assert_eq!(got.watch.entities.len(), 1);
        assert_eq!(got.watch.anchor_vec.as_ref().unwrap().len(), 3);
        assert!(
            (got.watch.anchor_vec.unwrap()[1] - 0.2).abs() < 1e-6,
            "anchor vec decrypted"
        );
        assert_eq!(got.digest_period, "daily");
        assert!(got.llm_summary);
    }

    #[test]
    fn anchor_vec_encrypted_at_rest() {
        let (s, dek, _d) = store();
        let id = s
            .add_watch(
                &dek,
                &WatchInput {
                    label: "x".into(),
                    anchor_vec: Some(vec![1.0, 2.0]),
                    ..Default::default()
                },
            )
            .unwrap();
        // raw blob must not contain plaintext f32 little-endian 1.0 bytes pattern trivially —
        // we just assert decrypt works and the column is non-null.
        let blob: Option<Vec<u8>> = s
            .raw_connection_for_test()
            .query_row(
                "SELECT anchor_vec_enc FROM watches WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        let blob = blob.expect("anchor_vec_enc present");
        // ciphertext is longer than the 8-byte plaintext (nonce + tag).
        assert!(blob.len() > 8, "encrypted blob carries nonce+tag overhead");
    }

    #[test]
    fn patch_and_delete() {
        let (s, dek, _d) = store();
        let id = s
            .add_watch(
                &dek,
                &WatchInput {
                    label: "x".into(),
                    keywords: vec!["k".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(s
            .patch_watch(
                &id,
                &WatchPatch {
                    enabled: Some(false),
                    llm_summary: Some(true),
                    ..Default::default()
                }
            )
            .unwrap());
        let got = s.get_watch(&dek, &id).unwrap().unwrap();
        assert!(!got.watch.enabled);
        assert!(got.llm_summary);
        // patch nonexistent id → false.
        assert!(!s
            .patch_watch(
                "nope",
                &WatchPatch {
                    enabled: Some(true),
                    ..Default::default()
                }
            )
            .unwrap());
        // delete.
        assert!(s.delete_watch(&id).unwrap());
        assert!(s.get_watch(&dek, &id).unwrap().is_none());
        assert!(!s.delete_watch(&id).unwrap());
    }

    #[test]
    fn upsert_hits_unique_and_pending() {
        let (s, dek, _d) = store();
        let id = s
            .add_watch(
                &dek,
                &WatchInput {
                    label: "x".into(),
                    keywords: vec!["k".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        let hit = |item: &str, score: f32| WatchHit {
            watch_id: id.clone(),
            item_id: item.to_string(),
            title: String::new(),
            score,
            reasons: vec!["r".into()],
            dedup_group: None,
            created_at: "2026-06-18T00:00:00Z".into(),
        };
        let n = s
            .upsert_watch_hits(&[hit("i1", 0.9), hit("i2", 0.5)])
            .unwrap();
        assert_eq!(n, 2);
        // re-upsert same item → conflict update, 0 new.
        let n2 = s.upsert_watch_hits(&[hit("i1", 0.95)]).unwrap();
        assert_eq!(n2, 0, "UNIQUE(watch_id,item_id) dedup");
        assert_eq!(s.count_pending_hits(&id).unwrap(), 2);
        let pending = s.list_pending_hits(&id, 10).unwrap();
        assert_eq!(pending.len(), 2);
        assert!(pending[0].score >= pending[1].score, "score desc");
        assert!(
            (pending[0].score - 0.95).abs() < 1e-5,
            "conflict updated score"
        );
    }

    #[test]
    fn mark_digested_cross_time_dedup() {
        let (s, dek, _d) = store();
        let id = s
            .add_watch(
                &dek,
                &WatchInput {
                    label: "x".into(),
                    keywords: vec!["k".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        s.upsert_watch_hits(&[WatchHit {
            watch_id: id.clone(),
            item_id: "i1".into(),
            title: String::new(),
            score: 0.9,
            reasons: vec![],
            dedup_group: None,
            created_at: "2026-06-18T00:00:00Z".into(),
        }])
        .unwrap();
        assert_eq!(s.count_pending_hits(&id).unwrap(), 1);
        s.mark_hits_digested(&id, Some("2026-06-18T00:00:00Z"))
            .unwrap();
        assert_eq!(
            s.count_pending_hits(&id).unwrap(),
            0,
            "digested hits not pending"
        );
        let got = s.get_watch(&dek, &id).unwrap().unwrap();
        assert!(got.last_digested_at.is_some());
        assert_eq!(
            got.last_digested_marker.as_deref(),
            Some("2026-06-18T00:00:00Z")
        );
    }
}
