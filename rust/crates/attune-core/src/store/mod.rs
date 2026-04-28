// npu-vault/crates/vault-core/src/store.rs

mod types;
mod items;
mod dirs;
mod queue;
mod history;
mod conversations;
mod signals;
mod chunk_summaries;
mod annotations;
mod project;
mod memories;
mod web_search_cache;
mod chunk_breadcrumbs;
pub mod browse_signals;  // pub: BrowseSignalInput / BrowseSignalRow 给 attune-server route 用
pub mod auto_bookmarks;  // W4 G2: high engagement auto bookmark candidates (G3 staging)
pub mod audit;            // v0.6 Phase A.5.3: 出网审计日志

pub use types::*;

// re-export 子模块的关键常量（避免 `crate::store::web_search_cache::DEFAULT_TTL_SECS` 长路径）
pub use web_search_cache::DEFAULT_TTL_SECS as DEFAULT_WEB_SEARCH_TTL_SECS;

use rusqlite::{params, Connection};
use std::path::Path;

// crypto + Key32 仅 tests 内引用 (#[cfg(test)] 子模块经常重新 use 它们)；
// 顶部 import 保留是为防未来 mod.rs 主体加 dek 字段时不必再补 import。
// per W3 batch B 遗留代码扫描：标记 allow 不算回归。
#[allow(unused_imports)]
use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS vault_meta (
    key   TEXT PRIMARY KEY,
    value BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS items (
    id           TEXT PRIMARY KEY,
    title        TEXT NOT NULL,
    content      BLOB NOT NULL,
    url          TEXT,
    source_type  TEXT NOT NULL DEFAULT 'note',
    domain       TEXT,
    tags         BLOB,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    is_deleted   INTEGER NOT NULL DEFAULT 0,
    -- v0.6 Phase A.5.4 隐私分级（per 用户决策 2026-04-28）
    -- L0 = 标记为🔒，永不出网（强制本地 LLM）；L1 = 默认（脱敏 → 云）；L3 = 高敏感（LLM 脱敏 → 云）
    privacy_tier TEXT NOT NULL DEFAULT 'L1',
    -- v0.6 Phase B F-Pro 跨域污染防御（per 用户决策 2026-04-28）
    -- 旧 `domain` 字段历史用作"网站域名"（来自 chrome 扩展），与本字段语义冲突
    -- → 新建 corpus_domain 字段表示"领域分类"（legal/tech/general/medical/...）
    -- search 阶段按 query intent 跨领域降权，防止"反洗钱"被 cs-notes 顶占
    corpus_domain TEXT NOT NULL DEFAULT 'general'
);
CREATE INDEX IF NOT EXISTS idx_items_created ON items(created_at);
CREATE INDEX IF NOT EXISTS idx_items_deleted ON items(is_deleted);

CREATE TABLE IF NOT EXISTS embed_queue (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id     TEXT NOT NULL REFERENCES items(id),
    chunk_idx   INTEGER NOT NULL,
    chunk_text  BLOB NOT NULL,
    level       INTEGER NOT NULL DEFAULT 2,
    section_idx INTEGER NOT NULL DEFAULT 0,
    priority    INTEGER NOT NULL DEFAULT 2,
    status      TEXT NOT NULL DEFAULT 'pending',
    attempts    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_eq_status ON embed_queue(status, priority, created_at);
CREATE INDEX IF NOT EXISTS idx_eq_item ON embed_queue(item_id);

CREATE TABLE IF NOT EXISTS bound_dirs (
    id            TEXT PRIMARY KEY,
    path          TEXT UNIQUE NOT NULL,
    recursive     INTEGER NOT NULL DEFAULT 1,
    file_types    TEXT NOT NULL,
    is_active     INTEGER NOT NULL DEFAULT 1,
    last_scan     TEXT,
    -- v0.6 Phase B F-Pro: bind 时声明 corpus 领域，scanner 写入 items.corpus_domain
    -- 'legal' / 'tech' / 'medical' / 'patent' / 'general'（默认）
    corpus_domain TEXT NOT NULL DEFAULT 'general'
);

CREATE TABLE IF NOT EXISTS indexed_files (
    id         TEXT PRIMARY KEY,
    dir_id     TEXT NOT NULL REFERENCES bound_dirs(id),
    path       TEXT UNIQUE NOT NULL,
    file_hash  TEXT NOT NULL,
    item_id    TEXT REFERENCES items(id),
    indexed_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_if_dir ON indexed_files(dir_id);

CREATE TABLE IF NOT EXISTS sessions (
    token      TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS search_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    query BLOB NOT NULL,
    result_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_history_created ON search_history(created_at);

CREATE TABLE IF NOT EXISTS click_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    query BLOB NOT NULL,
    item_id TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_click_item ON click_events(item_id);
CREATE INDEX IF NOT EXISTS idx_click_created ON click_events(created_at);

CREATE TABLE IF NOT EXISTS feedback (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id      TEXT NOT NULL,
    feedback_type TEXT NOT NULL CHECK(feedback_type IN ('relevant','irrelevant','correction')),
    query        TEXT,
    created_at   TEXT NOT NULL
);
-- 注：feedback 表当前只 INSERT 写入（来自 POST /api/v1/feedback），
-- 暂无 SELECT 读取路径；待将来加分析/重排时再加索引。

CREATE TABLE IF NOT EXISTS conversations (
    id          TEXT PRIMARY KEY,
    title       BLOB NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS conversation_messages (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role            TEXT NOT NULL CHECK(role IN ('user','assistant','system')),
    content         BLOB NOT NULL,
    citations       TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_conv_messages_conv_id
    ON conversation_messages(conversation_id);

CREATE TABLE IF NOT EXISTS skill_signals (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    query           TEXT NOT NULL,
    knowledge_count INTEGER NOT NULL DEFAULT 0,
    web_used        INTEGER NOT NULL DEFAULT 0,
    processed       INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_skill_sig_processed ON skill_signals(processed, created_at);

-- Chunk 摘要缓存 —— 上下文压缩流水线（Batch B.1）
--
-- 成本/触发契约：摘要由 💰 LLM 生成，但入缓存后永久复用。chat 流程命中缓存即免费。
-- 按 (chunk_hash, strategy) 组合主键：同一 chunk 在 economical/accurate 两策略下各有一份摘要。
-- item_id 冗余存，用于 item 软删除时级联清理。
--
-- 字段：
--   chunk_hash —— sha256(chunk_text) hex，决定性；内容变 → hash 变 → 缓存自然失效
--   strategy   —— 'economical' (~150 字) | 'accurate' (~300 字)
--   model      —— 生成摘要所用的 LLM 模型名（便于调试质量退化）
--   summary    —— 加密的摘要文本
--   orig_chars —— 原 chunk 字符数（统计用）
CREATE TABLE IF NOT EXISTS chunk_summaries (
    chunk_hash  TEXT NOT NULL,
    strategy    TEXT NOT NULL CHECK(strategy IN ('economical','accurate')),
    item_id     TEXT NOT NULL,
    model       TEXT NOT NULL,
    summary     BLOB NOT NULL,
    orig_chars  INTEGER NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (chunk_hash, strategy)
);
CREATE INDEX IF NOT EXISTS idx_chunk_sum_item ON chunk_summaries(item_id);

-- 批注表：用户手动标注 + 未来 AI 分析批注。
--
-- 设计决策（详见 memory/project_attune_annotation_model.md）：
--   · source = 状态（user/ai）而非分类；用户再手动编辑 → 回到 user
--   · 字符偏移 + snippet 双锚点：offset_start/offset_end 是首选定位，snippet 是 fallback，
--     供文档更新后重新定位（lawcontrol 风格的 location_confidence 后续版本再加）
--   · 5 个预设 emoji 标签由前端枚举：⭐重点 / 📍待深入 / 🤔存疑 / ❓不懂 / 🗑过时
--   · content 字段加密（放个人思考），snippet 不加密（用于定位恢复）
CREATE TABLE IF NOT EXISTS annotations (
    id           TEXT PRIMARY KEY,
    item_id      TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    offset_start INTEGER NOT NULL,
    offset_end   INTEGER NOT NULL,
    text_snippet TEXT NOT NULL,
    label        TEXT,
    color        TEXT NOT NULL DEFAULT 'yellow',
    content      BLOB,
    source       TEXT NOT NULL DEFAULT 'user' CHECK(source IN ('user', 'ai')),
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_annotations_item ON annotations(item_id);
CREATE INDEX IF NOT EXISTS idx_annotations_source ON annotations(source);
CREATE INDEX IF NOT EXISTS idx_annotations_created ON annotations(created_at);

-- Project / Case 卷宗（spec §2.1）
-- 通用 Project 层；行业层（attune-law / attune-sales）通过 metadata_encrypted 存
-- opaque AES-GCM blob，attune-core 不解析其结构。
CREATE TABLE IF NOT EXISTS project (
    id                 TEXT PRIMARY KEY,
    title              TEXT NOT NULL,
    kind               TEXT NOT NULL DEFAULT 'generic',
    metadata_encrypted BLOB,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    archived           INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS project_file (
    project_id TEXT NOT NULL,
    file_id    TEXT NOT NULL,
    role       TEXT NOT NULL DEFAULT '',
    added_at   INTEGER NOT NULL,
    PRIMARY KEY (project_id, file_id),
    FOREIGN KEY (project_id) REFERENCES project(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS project_timeline (
    project_id        TEXT NOT NULL,
    ts                INTEGER NOT NULL,
    event_type        TEXT NOT NULL,
    payload_encrypted BLOB,
    FOREIGN KEY (project_id) REFERENCES project(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_project_timeline_pid ON project_timeline(project_id, ts);

-- A1 Memory Consolidation (2026-04-27)
-- 周期性把 chunk_summaries 按时间窗口聚合成 episodic memory（情景记忆）。
-- 幂等键 = (kind, source_chunk_hashes JSON)，重跑相同 chunk 集合不重复入库。
-- summary_encrypted 存 LLM 总结正文（DEK 加密）。kind 当前仅 'episodic'，
-- W5+ 加 'semantic'（按主题聚合）。
CREATE TABLE IF NOT EXISTS memories (
    id                    TEXT PRIMARY KEY,
    -- W1 仅用 'episodic'；'semantic' 已预先放入 CHECK 集合，避免 W5+ 时
    -- SQLite 麻烦的 ALTER TABLE … DROP CHECK（设计稿 §3 + reviewer I5）
    kind                  TEXT NOT NULL CHECK(kind IN ('episodic', 'semantic')),
    window_start          INTEGER NOT NULL,
    window_end            INTEGER NOT NULL,
    source_chunk_hashes   TEXT NOT NULL,
    source_chunk_count    INTEGER NOT NULL,
    summary_encrypted     BLOB NOT NULL,
    model                 TEXT NOT NULL,
    created_at            INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_memories_window ON memories(window_start, window_end);
CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS uq_memories_source ON memories(kind, source_chunk_hashes);

-- C1 Web search cache (W3 batch A, 2026-04-27)
-- per spec docs/superpowers/specs/2026-04-27-w3-batch-a-design.md §3
-- per ACKNOWLEDGMENTS.md C series — 吴师兄 §6 高频 query 缓存模式
--
-- query_hash = SHA-256(query) hex 作为查找键；query_text + results JSON 字段加密。
-- 30 天默认 TTL，过期由查询时过滤（不主动 GC，与 chunk_summaries 一致的惰性策略）。
CREATE TABLE IF NOT EXISTS web_search_cache (
    query_hash       TEXT PRIMARY KEY,
    query_text_enc   BLOB NOT NULL,
    results_json_enc BLOB NOT NULL,
    created_at_secs  INTEGER NOT NULL,
    ttl_secs         INTEGER NOT NULL DEFAULT 2592000
);
CREATE INDEX IF NOT EXISTS idx_web_cache_created ON web_search_cache(created_at_secs);

-- F2 Chunk breadcrumb 元数据 (W3 batch A, 2026-04-27)
-- per spec docs/superpowers/specs/2026-04-27-w3-batch-a-design.md §4
--
-- 独立于 embed_queue 的辅助表，避免改 VectorMeta serde / 老 .encbin 兼容。
-- 老 vault 升级时 IF NOT EXISTS 创建空表 → ChatEngine 查不到时返回空 Vec 优雅降级。
-- breadcrumb 是 chunker SectionWithPath.path 的 JSON 序列化（升序数组）。
-- 明文存储：标题来自文档结构，非用户笔记内容。
-- offset_start/end 是 chunk 在 item.content 的 char-level 区间。
CREATE TABLE IF NOT EXISTS chunk_breadcrumbs (
    -- per reviewer I3：FK CASCADE 与 annotations 表对称（item 硬删除时清理；
    -- 软删除模型下还需在 store::delete_item 显式清理，与 annotations 一致）
    item_id              TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    chunk_idx            INTEGER NOT NULL,
    -- per R04 P0-1：breadcrumb（章节标题路径如"案件分析 > 原告主张"）属用户敏感
    -- 数据，必须 DEK 加密。修复违反 "All data encrypted on your own device" 承诺。
    -- 字段名 breadcrumb_enc 取代原 breadcrumb_json 明文，schema bump 让老 db 自动重建。
    breadcrumb_enc       BLOB NOT NULL,
    offset_start         INTEGER NOT NULL,
    offset_end           INTEGER NOT NULL,
    PRIMARY KEY (item_id, chunk_idx)
);
-- per reviewer G5：删除冗余 idx_chunk_breadcrumbs_item，PK 前缀已可用

-- G1 浏览状态信号 (W3 batch B, 2026-04-27)
-- per spec docs/superpowers/specs/2026-04-27-w3-batch-b-design.md §3
-- url + title 加密（用户浏览历史属隐私）；engagement 数值明文便于聚合查询。
-- domain_hash = SHA-256(domain) 让"按域名聚合 / 删除"无需暴露域名明文索引
CREATE TABLE IF NOT EXISTS browse_signals (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    url_enc         BLOB NOT NULL,
    title_enc       BLOB NOT NULL,
    domain_hash     TEXT NOT NULL,
    dwell_ms        INTEGER NOT NULL,
    scroll_pct      INTEGER NOT NULL,
    copy_count      INTEGER NOT NULL,
    visit_count     INTEGER NOT NULL,
    created_at_secs INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_browse_signals_domain ON browse_signals(domain_hash, created_at_secs DESC);
CREATE INDEX IF NOT EXISTS idx_browse_signals_created ON browse_signals(created_at_secs DESC);

-- W4 G2 auto bookmark candidates (2026-04-27)
-- per spec docs/superpowers/specs/2026-04-27-w3-batch-b-design.md §3.G2 + W4 plan G2
-- 高 engagement 浏览页 (dwell ≥3min + scroll ≥50% + copy ≥1) 自动入候选表，
-- G3 (W5-6) 后台 worker 抓正文后 promote 到 items + 置 promoted = 1。
-- url/title 加密同 browse_signals — 候选状态也是用户隐私。
CREATE TABLE IF NOT EXISTS auto_bookmarks (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    url_enc         BLOB NOT NULL,
    title_enc       BLOB NOT NULL,
    domain_hash     TEXT NOT NULL,
    dwell_ms        INTEGER NOT NULL,
    scroll_pct      INTEGER NOT NULL,
    copy_count      INTEGER NOT NULL,
    visit_count     INTEGER NOT NULL,
    created_at_secs INTEGER NOT NULL,
    promoted        INTEGER NOT NULL DEFAULT 0,  -- G3 promote to items 后置 1
    promoted_item_id TEXT                          -- promote 时记录关联 item.id
);
CREATE INDEX IF NOT EXISTS idx_auto_bookmarks_pending ON auto_bookmarks(promoted, created_at_secs);
CREATE INDEX IF NOT EXISTS idx_auto_bookmarks_domain ON auto_bookmarks(domain_hash, created_at_secs DESC);

-- v0.6 Phase A.5.3 隐私审计日志（per 用户决策 2026-04-28）
-- 全字段明文：合规员/用户必须可读 timestamp/provider/model/token/hash/redactions
-- 不存原文 + 不存任何 PII（hash 是单向 SHA256[:16]）→ 即使审计 db 泄露也不暴露用户内容
-- direction: 'request' (出网) / 'response' (LLM 答案，可选记录)
-- privacy_tier: L0(全本地) / L1(脱敏→云) / L3(LLM脱敏→云)
-- redactions_json: {"PHONE":2,"EMAIL":1,"CASE_NO":3} 表"这次脱敏命中了多少敏感字段"
CREATE TABLE IF NOT EXISTS outbound_audit (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    ts_ms            INTEGER NOT NULL,
    direction        TEXT NOT NULL,
    provider         TEXT NOT NULL,
    model            TEXT NOT NULL,
    token_estimate   INTEGER NOT NULL DEFAULT 0,
    privacy_tier     TEXT NOT NULL DEFAULT 'L1',
    pre_redact_hash  TEXT NOT NULL,
    post_redact_hash TEXT NOT NULL,
    redactions_json  TEXT NOT NULL DEFAULT '{}',
    session_id       TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_outbound_audit_ts ON outbound_audit(ts_ms DESC);
CREATE INDEX IF NOT EXISTS idx_outbound_audit_session ON outbound_audit(session_id, ts_ms);
"#;

pub struct Store {
    conn: Connection,
}

impl Store {
    /// 打开或创建数据库，初始化 schema
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        conn.execute_batch(SCHEMA_SQL)?;
        Self::migrate_task_type(&conn)?;
        Self::migrate_breadcrumbs_encrypt(&conn)?;
        // v0.6 fix: 复位 stuck 在 processing 的任务回 pending（上次进程崩溃 / kill）
        let _ = conn.execute(
            "UPDATE embed_queue SET status = 'pending' WHERE status = 'processing'",
            [],
        );
        Self::migrate_items_privacy_tier(&conn)?;
        Self::migrate_corpus_domain(&conn)?;
        Ok(Self { conn })
    }

    /// 打开内存数据库（测试用）
    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA_SQL)?;
        Self::migrate_task_type(&conn)?;
        Self::migrate_breadcrumbs_encrypt(&conn)?;
        Self::migrate_items_privacy_tier(&conn)?;
        Self::migrate_corpus_domain(&conn)?;
        Ok(Self { conn })
    }

    /// 迁移：items 新增 corpus_domain 列 + bound_dirs 新增 corpus_domain 列
    /// (v0.6 Phase B F-Pro 跨域污染防御，幂等)
    fn migrate_corpus_domain(conn: &Connection) -> Result<()> {
        let has_items_col: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('items') WHERE name = 'corpus_domain'",
            [],
            |row| row.get(0),
        )?;
        if has_items_col == 0 {
            conn.execute(
                "ALTER TABLE items ADD COLUMN corpus_domain TEXT NOT NULL DEFAULT 'general'",
                [],
            )?;
        }
        let has_dirs_col: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('bound_dirs') WHERE name = 'corpus_domain'",
            [],
            |row| row.get(0),
        )?;
        if has_dirs_col == 0 {
            conn.execute(
                "ALTER TABLE bound_dirs ADD COLUMN corpus_domain TEXT NOT NULL DEFAULT 'general'",
                [],
            )?;
        }
        Ok(())
    }

    /// 迁移：items 新增 privacy_tier 列（v0.6 Phase A.5.4，幂等）
    fn migrate_items_privacy_tier(conn: &Connection) -> Result<()> {
        let has_col: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('items') WHERE name = 'privacy_tier'",
            [],
            |row| row.get(0),
        )?;
        if has_col == 0 {
            conn.execute(
                "ALTER TABLE items ADD COLUMN privacy_tier TEXT NOT NULL DEFAULT 'L1'",
                [],
            )?;
        }
        Ok(())
    }

    /// 迁移: embed_queue 新增 task_type 列（幂等）
    fn migrate_task_type(conn: &Connection) -> Result<()> {
        let has_task_type: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('embed_queue') WHERE name = 'task_type'",
            [],
            |row| row.get(0),
        )?;
        if has_task_type == 0 {
            conn.execute(
                "ALTER TABLE embed_queue ADD COLUMN task_type TEXT NOT NULL DEFAULT 'embed'",
                [],
            )?;
        }
        Ok(())
    }

    /// per R07 P0 + R04 P0-1：chunk_breadcrumbs.breadcrumb_json (TEXT 明文) →
    /// breadcrumb_enc (BLOB DEK 加密) 列名变更迁移。
    ///
    /// 老 vault（W3 batch A 末，commit 28bd691）有 `breadcrumb_json` 列；升级到
    /// W3 末后 SCHEMA_SQL `CREATE TABLE IF NOT EXISTS` 跳过老表 → INSERT 用
    /// `breadcrumb_enc` 列名运行期 SQL error → F2 子系统瘫痪。
    ///
    /// 修复策略：检测老列存在 → DROP TABLE + IF NOT EXISTS 重建。
    /// **老明文 breadcrumb 数据丢失**（acceptable — 下次 indexer ingest 触发重新
    /// upsert 自动 backfill；R07 P0 注释 + RELEASE.md 用户须知"W3 升级后首次
    /// ingest 触发 breadcrumb 重建"）。
    fn migrate_breadcrumbs_encrypt(conn: &Connection) -> Result<()> {
        let has_old_column: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('chunk_breadcrumbs') WHERE name = 'breadcrumb_json'",
            [],
            |row| row.get(0),
        )?;
        if has_old_column > 0 {
            log::info!(
                "F2 P0-1 migration: dropping old chunk_breadcrumbs (breadcrumb_json plaintext) → \
                 next indexer ingest will repopulate with encrypted breadcrumb_enc"
            );
            conn.execute("DROP TABLE chunk_breadcrumbs", [])?;
            // 重建走 SCHEMA_SQL 的 CREATE TABLE IF NOT EXISTS — 但 SCHEMA_SQL 已在 open() 里
            // 跑过一次（IF NOT EXISTS 跳过当时还有老列的表）。这里手动跑 CREATE 确保新 schema 生效。
            conn.execute(
                "CREATE TABLE IF NOT EXISTS chunk_breadcrumbs (\
                    item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,\
                    chunk_idx INTEGER NOT NULL,\
                    breadcrumb_enc BLOB NOT NULL,\
                    offset_start INTEGER NOT NULL,\
                    offset_end INTEGER NOT NULL,\
                    PRIMARY KEY (item_id, chunk_idx)\
                 )",
                [],
            )?;
        }
        Ok(())
    }

    /// Checkpoint WAL to main DB file (for testing at-rest encryption)
    pub fn checkpoint(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    // --- vault_meta ---

    pub fn set_meta(&self, key: &str, value: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO vault_meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let mut stmt = self.conn.prepare("SELECT value FROM vault_meta WHERE key = ?1")?;
        let result = stmt.query_row(params![key], |row| row.get::<_, Vec<u8>>(0));
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn has_meta(&self, key: &str) -> Result<bool> {
        Ok(self.get_meta(key)?.is_some())
    }

    /// 获取当前 token nonce（不存在时返回 0）
    pub fn get_token_nonce(&self) -> Result<u64> {
        match self.get_meta("token_nonce")? {
            Some(bytes) if bytes.len() == 8 => {
                let arr: [u8; 8] = bytes.as_slice().try_into()
                    .map_err(|_| VaultError::Crypto("token nonce size mismatch".into()))?;
                Ok(u64::from_le_bytes(arr))
            }
            _ => Ok(0u64),
        }
    }

    /// 递增 token nonce（每次 lock 调用）
    pub fn increment_token_nonce(&self) -> Result<u64> {
        let current = self.get_token_nonce()?;
        let next = current.wrapping_add(1);
        self.set_meta("token_nonce", &next.to_le_bytes())?;
        Ok(next)
    }

    /// 在单个事务中批量写入 vault_meta（用于 change_password 原子更新）
    /// 使用 unchecked_transaction 与 dequeue_embeddings/append_conversation_turn 保持一致，
    /// 避免与 rusqlite 内部事务状态机冲突。
    pub fn set_meta_batch(&self, entries: &[(&str, &[u8])]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (key, value) in entries {
            tx.execute(
                "INSERT OR REPLACE INTO vault_meta (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, value],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dek() -> Key32 {
        Key32::generate()
    }

    #[test]
    fn open_memory_creates_tables() {
        let store = Store::open_memory().unwrap();
        assert!(!store.has_meta("nonexistent").unwrap());
    }

    #[test]
    fn meta_set_get_roundtrip() {
        let store = Store::open_memory().unwrap();
        store.set_meta("salt", b"test-salt-value").unwrap();
        let value = store.get_meta("salt").unwrap().unwrap();
        assert_eq!(value, b"test-salt-value");
    }

    #[test]
    fn meta_overwrite() {
        let store = Store::open_memory().unwrap();
        store.set_meta("key", b"v1").unwrap();
        store.set_meta("key", b"v2").unwrap();
        assert_eq!(store.get_meta("key").unwrap().unwrap(), b"v2");
    }

    #[test]
    fn insert_and_get_item() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        let id = store
            .insert_item(
                &dek,
                "Test Title",
                "Secret content",
                Some("https://example.com"),
                "note",
                Some("example.com"),
                Some(&["tag1".into(), "tag2".into()]),
            )
            .unwrap();

        let item = store.get_item(&dek, &id).unwrap().unwrap();
        assert_eq!(item.title, "Test Title");
        assert_eq!(item.content, "Secret content");
        assert_eq!(item.url.as_deref(), Some("https://example.com"));
        assert_eq!(item.source_type, "note");
        assert_eq!(item.tags.unwrap(), vec!["tag1", "tag2"]);
    }

    #[test]
    fn get_item_wrong_dek_fails() {
        let store = Store::open_memory().unwrap();
        let dek1 = test_dek();
        let dek2 = test_dek();

        let id = store
            .insert_item(&dek1, "Title", "Secret", None, "note", None, None)
            .unwrap();
        let result = store.get_item(&dek2, &id);
        assert!(result.is_err(), "Should fail with wrong DEK");
    }

    #[test]
    fn content_stored_encrypted() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        let id = store
            .insert_item(&dek, "Title", "Plaintext secret", None, "note", None, None)
            .unwrap();

        // 直接读取原始 BLOB，验证不是明文
        let raw: Vec<u8> = store
            .conn
            .query_row("SELECT content FROM items WHERE id = ?1", params![id], |row| {
                row.get(0)
            })
            .unwrap();
        let raw_str = String::from_utf8_lossy(&raw);
        assert!(
            !raw_str.contains("Plaintext secret"),
            "Content should be encrypted in DB"
        );
    }

    #[test]
    fn list_items_returns_summaries() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        store
            .insert_item(&dek, "Item 1", "content1", None, "note", None, None)
            .unwrap();
        store
            .insert_item(
                &dek,
                "Item 2",
                "content2",
                None,
                "webpage",
                Some("example.com"),
                None,
            )
            .unwrap();

        let items = store.list_items(10, 0).unwrap();
        assert_eq!(items.len(), 2);
        // list_items 不包含 content（不需解密）
        assert!(items.iter().any(|i| i.title == "Item 1"));
        assert!(items.iter().any(|i| i.title == "Item 2"));
    }

    #[test]
    fn delete_item_soft_deletes() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        let id = store
            .insert_item(&dek, "To Delete", "secret", None, "note", None, None)
            .unwrap();
        assert_eq!(store.item_count().unwrap(), 1);

        assert!(store.delete_item(&id).unwrap());
        assert_eq!(store.item_count().unwrap(), 0);
        assert!(store.get_item(&dek, &id).unwrap().is_none());
    }

    #[test]
    fn find_item_by_url_returns_id_when_present() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let url = "https://patents.google.com/patent/US10000000/en";
        let id = store
            .insert_item(&dek, "Patent Title", "abstract text", Some(url), "patent", None, None)
            .unwrap();
        assert_eq!(store.find_item_by_url(url).unwrap(), Some(id));
    }

    #[test]
    fn find_item_by_url_returns_none_when_absent() {
        let store = Store::open_memory().unwrap();
        assert!(store.find_item_by_url("https://missing.example.com").unwrap().is_none());
    }

    #[test]
    fn find_item_by_url_returns_none_after_soft_delete() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let url = "https://patents.google.com/patent/US99999999/en";
        let id = store
            .insert_item(&dek, "Patent", "content", Some(url), "patent", None, None)
            .unwrap();
        store.delete_item(&id).unwrap();
        assert!(store.find_item_by_url(url).unwrap().is_none(), "soft-deleted item must not be found by URL");
    }

    #[test]
    fn item_count_excludes_deleted() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        let id1 = store
            .insert_item(&dek, "A", "a", None, "note", None, None)
            .unwrap();
        store
            .insert_item(&dek, "B", "b", None, "note", None, None)
            .unwrap();
        assert_eq!(store.item_count().unwrap(), 2);

        store.delete_item(&id1).unwrap();
        assert_eq!(store.item_count().unwrap(), 1);
    }

    #[test]
    fn task_type_column_migration() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let id = store.insert_item(&dek, "T", "C", None, "note", None, None).unwrap();
        store.enqueue_classify(&id, 3).unwrap();
        let tasks = store.dequeue_embeddings(10).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].task_type, "classify");
        assert_eq!(tasks[0].item_id, id);
    }

    #[test]
    fn update_and_get_tags() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let id = store.insert_item(&dek, "T", "C", None, "note", None, None).unwrap();
        let tags_json = r#"{"core":{"domain":["技术"]}}"#;
        assert!(store.update_tags(&dek, &id, tags_json).unwrap());
        let retrieved = store.get_tags_json(&dek, &id).unwrap().unwrap();
        assert_eq!(retrieved, tags_json);
    }

    #[test]
    fn log_and_recent_searches() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        store.log_search(&dek, "rust crypto", 5).unwrap();
        store.log_search(&dek, "python script", 3).unwrap();
        store.log_search(&dek, "法律合同", 7).unwrap();

        let history = store.recent_searches(&dek, 10).unwrap();
        assert_eq!(history.len(), 3);
        // 最新的应该在前
        assert_eq!(history[0].query, "法律合同");
        assert_eq!(history[0].result_count, 7);
    }

    #[test]
    fn log_click_and_popular() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        store.log_click(&dek, "q1", "item-a").unwrap();
        store.log_click(&dek, "q2", "item-a").unwrap();
        store.log_click(&dek, "q3", "item-b").unwrap();
        store.log_click(&dek, "q4", "item-a").unwrap();

        let popular = store.popular_items(10).unwrap();
        assert_eq!(popular.len(), 2);
        assert_eq!(popular[0].0, "item-a");
        assert_eq!(popular[0].1, 3);
        assert_eq!(popular[1].0, "item-b");
        assert_eq!(popular[1].1, 1);
    }

    #[test]
    fn search_history_query_encrypted_at_rest() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        store.log_search(&dek, "SECRET_QUERY_XYZ", 0).unwrap();

        // Read raw row
        let raw: Vec<u8> = store.conn.query_row(
            "SELECT query FROM search_history LIMIT 1",
            [],
            |row| row.get(0),
        ).unwrap();
        let raw_str = String::from_utf8_lossy(&raw);
        assert!(!raw_str.contains("SECRET_QUERY_XYZ"), "Query should be encrypted");
    }

    #[test]
    fn list_all_item_ids_excludes_deleted() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let a = store.insert_item(&dek, "A", "c", None, "note", None, None).unwrap();
        store.insert_item(&dek, "B", "c", None, "note", None, None).unwrap();
        let c = store.insert_item(&dek, "C", "c", None, "note", None, None).unwrap();
        store.delete_item(&c).unwrap();
        let ids = store.list_all_item_ids().unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&a));
    }

    #[test]
    fn list_stale_items_basic() {
        use chrono::{Duration, Utc};
        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        let id = store.insert_item(&dek, "New", "content", None, "note", None, None).unwrap();
        let old_ts = (Utc::now() - Duration::days(40)).format("%Y-%m-%dT%H:%M:%S").to_string();
        store.set_updated_at(&id, &old_ts).unwrap();
        let stale = store.list_stale_items(30, 50).unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].id, id);
    }

    #[test]
    fn list_stale_items_empty() {
        let store = Store::open_memory().unwrap();
        let stale = store.list_stale_items(30, 50).unwrap();
        assert!(stale.is_empty());
    }

    #[test]
    fn get_item_stats_basic() {
        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        let id = store.insert_item(&dek, "Test", "content", None, "note", None, None).unwrap();
        let stats = store.get_item_stats(&id).unwrap().unwrap();
        assert_eq!(stats.id, id);
        assert!(stats.chunk_count >= 0);
        assert_eq!(stats.embedding_pending + stats.embedding_done, stats.chunk_count);
    }

    #[test]
    fn get_item_stats_missing() {
        let store = Store::open_memory().unwrap();
        let result = store.get_item_stats("nonexistent-id").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn insert_feedback_valid() {
        let store = Store::open_memory().unwrap();
        let id = store.insert_feedback("item-1", "relevant", Some("my query")).unwrap();
        assert!(id > 0);
    }

    #[test]
    fn insert_feedback_invalid_type() {
        let store = Store::open_memory().unwrap();
        let result = store.insert_feedback("item-1", "bad_type", None);
        assert!(result.is_err());
    }

    #[test]
    fn insert_feedback_no_query() {
        let store = Store::open_memory().unwrap();
        let id = store.insert_feedback("item-1", "irrelevant", None).unwrap();
        assert!(id > 0);
    }

    #[test]
    fn test_create_and_list_conversations() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let id1 = store.create_conversation(&dek, "第一个会话").unwrap();
        let _id2 = store.create_conversation(&dek, "第二个会话").unwrap();
        let list = store.list_conversations(&dek, 10, 0).unwrap();
        assert_eq!(list.len(), 2);
        let ids: Vec<&str> = list.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&id1.as_str()));
    }

    #[test]
    fn test_append_and_get_messages() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let conv_id = store.create_conversation(&dek, "测试会话").unwrap();
        store.append_message(&dek, &conv_id, "user", "你好", &[]).unwrap();
        store.append_message(&dek, &conv_id, "assistant", "你好！有什么可以帮你的？", &[]).unwrap();
        let msgs = store.get_conversation_messages(&dek, &conv_id).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "你好");
        assert_eq!(msgs[1].role, "assistant");
    }

    #[test]
    fn test_delete_conversation_cascades() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let conv_id = store.create_conversation(&dek, "待删除").unwrap();
        store.append_message(&dek, &conv_id, "user", "消息内容", &[]).unwrap();
        store.delete_conversation(&conv_id).unwrap();
        let msgs = store.get_conversation_messages(&dek, &conv_id).unwrap();
        assert!(msgs.is_empty());
        let list = store.list_conversations(&dek, 10, 0).unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn test_citations_json_roundtrip() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let conv_id = store.create_conversation(&dek, "带引用").unwrap();
        let citations = vec![
            Citation { item_id: "abc".to_string(), title: "文档A".to_string(), relevance: 0.9 },
        ];
        store.append_message(&dek, &conv_id, "assistant", "回答内容", &citations).unwrap();
        let msgs = store.get_conversation_messages(&dek, &conv_id).unwrap();
        assert_eq!(msgs[0].citations.len(), 1);
        assert_eq!(msgs[0].citations[0].item_id, "abc");
    }

    // #12: append_message 外键约束（conv_id 不存在）
    #[test]
    fn test_append_message_nonexistent_conv_fails() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        // 直接向不存在的 conv_id 追加消息，应返回 Err（外键约束失败）
        let result = store.append_message(&dek, "nonexistent-conv-id", "user", "hello", &[]);
        assert!(result.is_err(), "append_message to nonexistent conversation should fail");
    }

    // #13: get_conversation_by_id 不存在返回 None
    #[test]
    fn test_get_conversation_by_id_not_found() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let result = store.get_conversation_by_id(&dek, "does-not-exist").unwrap();
        assert!(result.is_none());
    }
}

#[cfg(test)]
mod tests_dir {
    use super::*;

    fn open_store() -> Store {
        Store::open_memory().unwrap()
    }

    #[test]
    fn test_bind_directory_returns_id() {
        let store = open_store();
        let id = store.bind_directory("/tmp/docs", true, &["md", "txt"]).unwrap();
        assert!(!id.is_empty());
    }

    #[test]
    fn test_list_bound_directories_after_bind() {
        let store = open_store();
        store.bind_directory("/tmp/docs", true, &["md"]).unwrap();
        let dirs = store.list_bound_directories().unwrap();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].path, "/tmp/docs");
    }

    #[test]
    fn test_bind_multiple_directories() {
        let store = open_store();
        store.bind_directory("/tmp/a", false, &["txt"]).unwrap();
        store.bind_directory("/tmp/b", true, &["md"]).unwrap();
        let dirs = store.list_bound_directories().unwrap();
        assert_eq!(dirs.len(), 2);
    }

    #[test]
    fn test_unbind_directory_marks_inactive() {
        let store = open_store();
        let id = store.bind_directory("/tmp/docs", true, &["md"]).unwrap();
        store.unbind_directory(&id).unwrap();
        let dirs = store.list_bound_directories().unwrap();
        assert_eq!(dirs.len(), 0);
    }

    #[test]
    fn test_unbind_nonexistent_returns_err() {
        let store = open_store();
        let result = store.unbind_directory("nonexistent-id");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_dir_last_scan() {
        let store = open_store();
        let id = store.bind_directory("/tmp/docs", false, &["md"]).unwrap();
        store.update_dir_last_scan(&id).unwrap();
        let dirs = store.list_bound_directories().unwrap();
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].last_scan.is_some());
    }
}

#[cfg(test)]
mod tests_indexed_files {
    use super::*;

    fn open_store() -> Store {
        Store::open_memory().unwrap()
    }

    fn insert_test_item(store: &Store) -> String {
        let dek = crate::crypto::Key32::generate();
        store
            .insert_item(&dek, "test title", "test content", None, "note", None, None)
            .unwrap()
    }

    #[test]
    fn test_get_indexed_file_returns_none_for_unknown() {
        let store = open_store();
        let result = store.get_indexed_file("/nonexistent.md").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_upsert_indexed_file_insert() {
        let store = open_store();
        let dir_id = store.bind_directory("/tmp/docs", false, &["md"]).unwrap();
        let item_id = insert_test_item(&store);
        store
            .upsert_indexed_file(&dir_id, "/tmp/docs/note.md", "abc123", &item_id)
            .unwrap();
        let row = store.get_indexed_file("/tmp/docs/note.md").unwrap();
        assert!(row.is_some());
        let row = row.unwrap();
        assert_eq!(row.file_hash, "abc123");
        assert_eq!(row.item_id.as_deref(), Some(item_id.as_str()));
    }

    #[test]
    fn test_upsert_indexed_file_updates_hash() {
        let store = open_store();
        let dir_id = store.bind_directory("/tmp/docs", false, &["md"]).unwrap();
        let item_id = insert_test_item(&store);
        store
            .upsert_indexed_file(&dir_id, "/tmp/docs/note.md", "v1", &item_id)
            .unwrap();
        store
            .upsert_indexed_file(&dir_id, "/tmp/docs/note.md", "v2", &item_id)
            .unwrap();
        let row = store
            .get_indexed_file("/tmp/docs/note.md")
            .unwrap()
            .unwrap();
        assert_eq!(row.file_hash, "v2");
    }
}

#[cfg(test)]
mod tests_embed_queue {
    use super::*;

    fn open_store() -> Store {
        Store::open_memory().unwrap()
    }

    fn insert_test_item(store: &Store) -> String {
        let dek = crate::crypto::Key32::generate();
        store
            .insert_item(&dek, "title", "content", None, "note", None, None)
            .unwrap()
    }

    #[test]
    fn test_enqueue_embedding_adds_to_queue() {
        let store = open_store();
        let item_id = insert_test_item(&store);
        store
            .enqueue_embedding(&item_id, 0, "chunk text", 1, 1, 0)
            .unwrap();
        assert_eq!(store.pending_embedding_count().unwrap(), 1);
    }

    #[test]
    fn test_dequeue_embeddings_returns_tasks() {
        let store = open_store();
        let item_id = insert_test_item(&store);
        store
            .enqueue_embedding(&item_id, 0, "chunk A", 1, 1, 0)
            .unwrap();
        store
            .enqueue_embedding(&item_id, 1, "chunk B", 1, 2, 0)
            .unwrap();
        let tasks = store.dequeue_embeddings(10).unwrap();
        assert_eq!(tasks.len(), 2);
        // dequeue 后状态变为 processing，pending 计数应为 0
        assert_eq!(store.pending_embedding_count().unwrap(), 0);
    }

    #[test]
    fn test_dequeue_respects_batch_size() {
        let store = open_store();
        let item_id = insert_test_item(&store);
        for i in 0..5 {
            store
                .enqueue_embedding(&item_id, i, &format!("chunk {i}"), 1, 1, 0)
                .unwrap();
        }
        let tasks = store.dequeue_embeddings(3).unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(store.pending_embedding_count().unwrap(), 2);
    }

    #[test]
    fn test_mark_embedding_done_removes_from_active() {
        let store = open_store();
        let item_id = insert_test_item(&store);
        store
            .enqueue_embedding(&item_id, 0, "chunk", 1, 1, 0)
            .unwrap();
        let tasks = store.dequeue_embeddings(1).unwrap();
        store.mark_embedding_done(tasks[0].id).unwrap();
        // done 状态不再是 pending 或 processing，再次 dequeue 应为空
        let re_tasks = store.dequeue_embeddings(10).unwrap();
        assert_eq!(re_tasks.len(), 0);
    }

    #[test]
    fn test_mark_embedding_failed_retries_within_max() {
        let store = open_store();
        let item_id = insert_test_item(&store);
        store
            .enqueue_embedding(&item_id, 0, "chunk", 1, 1, 0)
            .unwrap();
        let tasks = store.dequeue_embeddings(1).unwrap();
        // max_attempts=3，第一次失败后 attempts=1 < 3，应重新变为 pending
        store.mark_embedding_failed(tasks[0].id, 3).unwrap();
        assert_eq!(store.pending_embedding_count().unwrap(), 1);
    }

    #[test]
    fn test_mark_embedding_failed_abandons_after_max() {
        let store = open_store();
        let item_id = insert_test_item(&store);
        store
            .enqueue_embedding(&item_id, 0, "chunk", 1, 1, 0)
            .unwrap();
        // 连续失败 3 次（max_attempts=3），第3次后状态变为 abandoned
        for _ in 0..3 {
            let tasks = store.dequeue_embeddings(1).unwrap();
            if tasks.is_empty() {
                break;
            }
            store.mark_embedding_failed(tasks[0].id, 3).unwrap();
        }
        assert_eq!(store.pending_embedding_count().unwrap(), 0);
    }

    #[test]
    fn test_mark_task_pending_restores_processing() {
        let store = open_store();
        let item_id = insert_test_item(&store);
        store
            .enqueue_embedding(&item_id, 0, "chunk", 1, 1, 0)
            .unwrap();
        let tasks = store.dequeue_embeddings(1).unwrap();
        // dequeue 后变为 processing，pending 计数为 0
        assert_eq!(store.pending_embedding_count().unwrap(), 0);
        store.mark_task_pending(tasks[0].id).unwrap();
        assert_eq!(store.pending_embedding_count().unwrap(), 1);
    }

    #[test]
    fn test_checkpoint_does_not_error() {
        let store = open_store();
        // open_memory 使用内存数据库，wal_checkpoint 是 no-op 但不应报错
        store.checkpoint().unwrap();
    }

    #[test]
    fn test_enqueue_chunk_text_preserved() {
        let store = open_store();
        let item_id = insert_test_item(&store);
        let text = "Unicode text: 中文 \u{1F511}";
        store
            .enqueue_embedding(&item_id, 0, text, 1, 1, 0)
            .unwrap();
        let tasks = store.dequeue_embeddings(1).unwrap();
        assert_eq!(tasks[0].chunk_text, text);
    }
}

#[cfg(test)]
mod tests_annotations {
    use super::*;
    use crate::crypto::Key32;

    fn setup() -> (Store, Key32, String) {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let item_id = store
            .insert_item(&dek, "test item", "hello world body", None, "note", None, None)
            .unwrap();
        (store, dek, item_id)
    }

    fn make_input(offset_start: i64, offset_end: i64, text: &str, label: Option<&str>) -> AnnotationInput {
        AnnotationInput {
            offset_start,
            offset_end,
            text_snippet: text.to_string(),
            label: label.map(|s| s.to_string()),
            color: "yellow".to_string(),
            content: format!("note about {text}"),
            source: None,
        }
    }

    #[test]
    fn create_and_list_roundtrip() {
        let (store, dek, item_id) = setup();
        let input = make_input(0, 5, "hello", Some("important"));
        let id = store.create_annotation(&dek, &item_id, &input).unwrap();
        assert!(!id.is_empty());

        let anns = store.list_annotations(&dek, &item_id).unwrap();
        assert_eq!(anns.len(), 1);
        assert_eq!(anns[0].id, id);
        assert_eq!(anns[0].offset_start, 0);
        assert_eq!(anns[0].offset_end, 5);
        assert_eq!(anns[0].text_snippet, "hello");
        assert_eq!(anns[0].label.as_deref(), Some("important"));
        assert_eq!(anns[0].color, "yellow");
        assert_eq!(anns[0].content, "note about hello");
        assert_eq!(anns[0].source, "user");
    }

    #[test]
    fn list_orders_by_offset() {
        let (store, dek, item_id) = setup();
        // 故意乱序插入，断言返回按 offset 升序
        store.create_annotation(&dek, &item_id, &make_input(6, 11, "world", None)).unwrap();
        store.create_annotation(&dek, &item_id, &make_input(0, 5, "hello", None)).unwrap();
        let anns = store.list_annotations(&dek, &item_id).unwrap();
        assert_eq!(anns.len(), 2);
        assert_eq!(anns[0].offset_start, 0);
        assert_eq!(anns[1].offset_start, 6);
    }

    #[test]
    fn content_is_encrypted_on_disk() {
        let (store, dek, item_id) = setup();
        let secret = "my private thought 隐私思考";
        let input = AnnotationInput {
            offset_start: 0, offset_end: 5,
            text_snippet: "hello".into(),
            label: None,
            color: "red".into(),
            content: secret.into(),
            source: None,
        };
        store.create_annotation(&dek, &item_id, &input).unwrap();
        // 直接读取密文
        let enc: Vec<u8> = store.conn.query_row(
            "SELECT content FROM annotations LIMIT 1",
            [], |r| r.get(0),
        ).unwrap();
        // 密文不应包含明文
        assert!(!enc.windows(secret.len()).any(|w| w == secret.as_bytes()),
            "encrypted content must not contain plaintext");
        // 解密 list 回读应该还原
        let anns = store.list_annotations(&dek, &item_id).unwrap();
        assert_eq!(anns[0].content, secret);
    }

    #[test]
    fn update_defaults_source_to_user() {
        let (store, dek, item_id) = setup();
        // 先以 AI 身份写入
        let mut input = make_input(0, 5, "hello", None);
        input.source = Some("ai".into());
        let id = store.create_annotation(&dek, &item_id, &input).unwrap();

        // 用户"手动编辑"：不指定 source → 应回到 user
        let mut edited = make_input(0, 5, "hello", Some("edited"));
        edited.content = "user revised".into();
        edited.source = None;  // 默认 user
        store.update_annotation(&dek, &id, &edited).unwrap();

        let anns = store.list_annotations(&dek, &item_id).unwrap();
        assert_eq!(anns[0].source, "user", "human edit must reset source to user");
        assert_eq!(anns[0].content, "user revised");
        assert_eq!(anns[0].label.as_deref(), Some("edited"));
    }

    #[test]
    fn update_respects_explicit_ai_source() {
        let (store, dek, item_id) = setup();
        let id = store.create_annotation(&dek, &item_id, &make_input(0, 5, "hello", None)).unwrap();

        // AI 工作流：显式写 source='ai'
        let mut ai_input = make_input(0, 5, "hello", Some("风险条款"));
        ai_input.source = Some("ai".into());
        store.update_annotation(&dek, &id, &ai_input).unwrap();

        let anns = store.list_annotations(&dek, &item_id).unwrap();
        assert_eq!(anns[0].source, "ai");
    }

    #[test]
    fn invalid_source_rejected() {
        let (store, dek, item_id) = setup();
        let mut input = make_input(0, 5, "hello", None);
        input.source = Some("malicious".into());
        let err = store.create_annotation(&dek, &item_id, &input);
        assert!(err.is_err(), "should reject unknown source");
    }

    #[test]
    fn delete_removes_annotation() {
        let (store, dek, item_id) = setup();
        let id = store.create_annotation(&dek, &item_id, &make_input(0, 5, "hello", None)).unwrap();
        assert_eq!(store.count_annotations(&item_id).unwrap(), 1);
        store.delete_annotation(&id).unwrap();
        assert_eq!(store.count_annotations(&item_id).unwrap(), 0);
    }

    #[test]
    fn delete_cascades_on_item_delete() {
        let (store, dek, item_id) = setup();
        store.create_annotation(&dek, &item_id, &make_input(0, 5, "hello", None)).unwrap();
        assert_eq!(store.count_annotations(&item_id).unwrap(), 1);
        // items 表硬删除会触发 ON DELETE CASCADE
        store.conn.execute("DELETE FROM items WHERE id = ?1", params![item_id]).unwrap();
        assert_eq!(store.count_annotations(&item_id).unwrap(), 0,
            "annotation should cascade-delete when item is removed");
    }

    #[test]
    fn delete_nonexistent_returns_err() {
        let (store, _, _) = setup();
        assert!(store.delete_annotation("no-such-id").is_err());
    }

    #[test]
    fn update_nonexistent_returns_err() {
        let (store, dek, _) = setup();
        let err = store.update_annotation(&dek, "no-such-id", &make_input(0, 5, "x", None));
        assert!(err.is_err());
    }

    #[test]
    fn count_returns_zero_for_item_without_annotations() {
        let (store, _, item_id) = setup();
        assert_eq!(store.count_annotations(&item_id).unwrap(), 0);
    }

    #[test]
    fn soft_deleting_item_cascades_to_annotations() {
        // 用户软删除 item 后：annotations 也应被清除（delete_item 级联 + list 过滤双保险）
        let (store, dek, item_id) = setup();
        store.create_annotation(&dek, &item_id, &make_input(0, 5, "hello", Some("⭐重点"))).unwrap();
        assert_eq!(store.list_annotations(&dek, &item_id).unwrap().len(), 1);

        let deleted = store.delete_item(&item_id).unwrap();
        assert!(deleted);

        // list 过滤软删除 → 返回空
        let anns = store.list_annotations(&dek, &item_id).unwrap();
        assert_eq!(anns.len(), 0, "soft-deleted item's annotations must not be returned");

        // DELETE 语义：实际也被硬删掉了（"忘记"）
        assert_eq!(store.count_annotations(&item_id).unwrap(), 0,
            "delete_item should cascade-delete annotations");
    }

    #[test]
    fn list_filters_orphaned_annotations_from_soft_deleted_items() {
        // 即便绕过 delete_item 路径直接 UPDATE is_deleted=1（模拟历史遗留 / 未来测试路径），
        // list_annotations 的 JOIN 过滤也应挡住。
        let (store, dek, item_id) = setup();
        store.create_annotation(&dek, &item_id, &make_input(0, 5, "hello", None)).unwrap();
        // 直接 UPDATE 跳过 delete_item 的级联
        store.conn.execute(
            "UPDATE items SET is_deleted = 1 WHERE id = ?1",
            params![item_id],
        ).unwrap();
        // 批注还在表里但不应被 list 出
        assert_eq!(store.list_annotations(&dek, &item_id).unwrap().len(), 0);
        // count 是裸 SQL 查表 —— 还能看到（作为内部指标），但外部不可见
        assert_eq!(store.count_annotations(&item_id).unwrap(), 1);
    }
}
