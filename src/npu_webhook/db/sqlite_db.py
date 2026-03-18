"""SQLite schema + 迁移"""

SCHEMA_SQL = """
-- 知识条目
CREATE TABLE IF NOT EXISTS knowledge_items (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    content     TEXT NOT NULL,
    url         TEXT,
    source_type TEXT NOT NULL DEFAULT 'webpage',
    domain      TEXT,
    tags        TEXT DEFAULT '[]',
    metadata    TEXT DEFAULT '{}',
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    is_deleted  INTEGER NOT NULL DEFAULT 0
);

-- FTS5 全文索引
CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_fts USING fts5(
    title, content, tokenize='simple'
);

-- Embedding 任务队列
CREATE TABLE IF NOT EXISTS embedding_queue (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id     TEXT NOT NULL REFERENCES knowledge_items(id),
    priority    INTEGER NOT NULL DEFAULT 1,
    status      TEXT NOT NULL DEFAULT 'pending',
    attempts    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- 绑定的本地目录
CREATE TABLE IF NOT EXISTS bound_directories (
    id          TEXT PRIMARY KEY,
    path        TEXT NOT NULL UNIQUE,
    recursive   INTEGER NOT NULL DEFAULT 1,
    file_types  TEXT DEFAULT '["md","txt","pdf","docx","py","js"]',
    is_active   INTEGER NOT NULL DEFAULT 1,
    last_scan   TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- 技能
CREATE TABLE IF NOT EXISTS skills (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    description TEXT,
    template    TEXT NOT NULL,
    match_pattern TEXT,
    extract_rule TEXT DEFAULT '{}',
    is_enabled  INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- 系统配置 KV
CREATE TABLE IF NOT EXISTS app_config (
    key TEXT PRIMARY KEY, value TEXT NOT NULL
);

-- 优化历史记录
CREATE TABLE IF NOT EXISTS optimization_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    category    TEXT NOT NULL,
    action      TEXT NOT NULL,
    before_metrics TEXT DEFAULT '{}',
    after_metrics  TEXT DEFAULT '{}',
    improvement TEXT,
    version     TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
"""


class SQLiteDB:
    """SQLite 数据库管理"""

    def __init__(self, db_path: str) -> None:
        self.db_path = db_path
        # TODO Phase 1: 初始化连接 + 执行 schema
