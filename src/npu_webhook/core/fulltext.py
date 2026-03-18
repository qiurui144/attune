"""SQLite FTS5 全文搜索 + jieba 分词"""


class FullTextSearch:
    """FTS5 全文搜索引擎"""

    def __init__(self, db_path: str) -> None:
        self.db_path = db_path
        # TODO Phase 1: 初始化 FTS5
