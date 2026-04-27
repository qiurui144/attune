//! chunk_summaries — Chunk 摘要缓存（上下文压缩流水线）。
//!
//! 成本/触发契约：这层缓存让 💰 LLM 摘要只跑一次；chat 流程命中缓存后属 🆓 层。
//! 压缩逻辑放在 `attune_core::context_compress`，此处只负责持久化。

use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::store::Store;

#[allow(unused_imports)]
use crate::store::types::*;

impl Store {
    /// 按 (chunk_hash, strategy) 查缓存；缺失返回 None。
    /// 命中 → 返回解密后的摘要文本。
    pub fn get_chunk_summary(
        &self,
        dek: &Key32,
        chunk_hash: &str,
        strategy: &str,
    ) -> Result<Option<String>> {
        let row: Option<Vec<u8>> = self.conn.query_row(
            "SELECT summary FROM chunk_summaries WHERE chunk_hash = ?1 AND strategy = ?2",
            params![chunk_hash, strategy],
            |r| r.get(0),
        ).map(Some).or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
        match row {
            Some(enc) => {
                let plain = crypto::decrypt(dek, &enc)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .ok();
                Ok(plain)
            }
            None => Ok(None),
        }
    }

    /// 存摘要。重复的 (chunk_hash, strategy) 走 REPLACE 覆盖（同策略不应产生多条）。
    pub fn put_chunk_summary(
        &self,
        dek: &Key32,
        chunk_hash: &str,
        strategy: &str,
        item_id: &str,
        model: &str,
        summary: &str,
        orig_chars: usize,
    ) -> Result<()> {
        if !matches!(strategy, "economical" | "accurate") {
            return Err(VaultError::InvalidInput(format!("unknown strategy: {strategy}")));
        }
        let enc = crypto::encrypt(dek, summary.as_bytes())?;
        self.conn.execute(
            "INSERT OR REPLACE INTO chunk_summaries
                (chunk_hash, strategy, item_id, model, summary, orig_chars, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))",
            params![chunk_hash, strategy, item_id, model, enc, orig_chars as i64],
        )?;
        Ok(())
    }

    /// 统计缓存命中率用
    pub fn chunk_summary_count(&self) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM chunk_summaries",
            [],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// 仅供集成测试用：seed 一条 chunk_summary 并指定 created_at（ISO8601 字符串）。
    /// 生产路径走 [`Self::put_chunk_summary`]，由 SQLite `datetime('now')` 自动填时间。
    ///
    /// **Feature-gated**：仅在启用 `test-utils` feature 时编译。生产二进制不暴露。
    /// 集成测试在 `Cargo.toml` 加 `attune-core = { features = ["test-utils"] }` 即可调用。
    #[cfg(any(test, feature = "test-utils"))]
    #[doc(hidden)]
    pub fn __test_seed_chunk_summary(
        &self,
        dek: &Key32,
        chunk_hash: &str,
        item_id: &str,
        summary: &str,
        created_at_iso: &str,
    ) -> Result<()> {
        let enc = crypto::encrypt(dek, summary.as_bytes())?;
        self.conn.execute(
            "INSERT INTO chunk_summaries \
                (chunk_hash, strategy, item_id, model, summary, orig_chars, created_at) \
             VALUES (?1, 'economical', ?2, 'test-model', ?3, ?4, ?5)",
            params![chunk_hash, item_id, enc, summary.len() as i64, created_at_iso],
        )?;
        Ok(())
    }
}
