"""SQLite FTS5 全文搜索 + jieba 中文分词辅助"""

import logging

import jieba

logger = logging.getLogger(__name__)

# 初始化 jieba（静默模式）
jieba.setLogLevel(logging.WARNING)


def tokenize_for_search(text: str) -> str:
    """使用 jieba 分词，将文本转换为空格分隔的搜索词

    FTS5 simple tokenizer 按空格分词，所以需要预处理。
    """
    words = jieba.cut_for_search(text)
    return " ".join(w.strip() for w in words if w.strip())


def build_fts_query(query: str) -> str:
    """将用户查询转换为 FTS5 查询语法

    对每个分词结果用双引号包裹（精确词匹配），用 OR 连接。
    注：双引号内的词做短语匹配，适合 jieba 已分好的语义词元。
    """
    words = jieba.cut_for_search(query)
    # 去除词中的双引号，避免破坏 FTS5 语法（如 "Python"3" 是非法查询）
    terms = [w.strip().replace('"', "") for w in words if len(w.strip()) > 1]
    if not terms:
        return query
    return " OR ".join(f'"{t}"' for t in terms if t)
