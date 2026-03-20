# 长文本质量提升 + 零摩擦安装 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现两层语义索引（章节+段落）+ 文件上传端点 + 动态注入预算 + 扩展文件标签页 + 系统托盘，将长文本质量和安装体验提升到产品化水准。

**Architecture:** 后端新增 `extract_sections()`（章节级分割）后，`pipeline.py` 为每条内容同时入队 Level 1（章节）和 Level 2（段落）两层 embedding；搜索时两阶段召回（章节锚定 → 段落精排 → 父章节上下文扩展）替代固定 300 字截断；`api/upload.py` 接收 multipart 文件，经 `parse_bytes()` 解析后走相同入库流程；前端侧边栏新增文件标签页；`tray.py` 提供系统托盘入口。

**Tech Stack:** Python/FastAPI (backend), SQLite ALTER TABLE (schema migration), ChromaDB `$and`+`$in` (composite where), Preact/JSX (extension UI), pystray+Pillow (system tray)

---

## File Structure

| 状态 | 文件 | 职责 |
|---|---|---|
| 新增 | `src/npu_webhook/api/upload.py` | `POST /api/v1/upload` multipart 文件上传端点 |
| 新增 | `src/npu_webhook/tray.py` | 系统托盘主进程（pystray + uvicorn 子线程）|
| 新增 | `extension/src/sidepanel/pages/FilePage.jsx` | 文件上传标签页 |
| 新增 | `tests/test_upload.py` | 上传 API 测试 |
| 修改 | `src/npu_webhook/core/chunker.py` | 新增 `extract_sections()` 章节级分割 |
| 修改 | `src/npu_webhook/core/parser.py` | 新增 `parse_bytes()` 非破坏性重载 |
| 修改 | `src/npu_webhook/core/search.py` | 两阶段层级检索 + `_allocate_budget` 动态预算 |
| 修改 | `src/npu_webhook/db/sqlite_db.py` | `embedding_queue` 加 `level`/`section_idx` 列 + `enqueue_embedding` 签名 |
| 修改 | `src/npu_webhook/scheduler/queue.py` | `_process_batch` 补充 `level`/`section_idx` metadata |
| 修改 | `src/npu_webhook/indexer/pipeline.py` | 两层入队（Level 1 章节 + Level 2 段落）|
| 修改 | `src/npu_webhook/config.py` | `SearchConfig` 新增 `injection_budget`；`IngestConfig` 新增 `max_upload_mb` |
| 修改 | `src/npu_webhook/app_state.py` | 新增 `session_upload_ids: dict[str, float]` |
| 修改 | `src/npu_webhook/main.py` | 注册 `upload_router` |
| 修改 | `extension/src/sidepanel/App.jsx` | 注册第四个标签"文件" |
| 修改 | `extension/src/shared/api.js` | 新增 `uploadFile(file, sessionId)` |
| 修改 | `extension/src/background/worker.js` | 新增会话感知加权（`session_upload_ids`）|
| 修改 | `tests/test_search.py` | 层级检索测试 + 动态预算测试 |
| 修改 | `tests/test_api.py` | `POST /upload` 端点测试 |

---

## Task 1: Config + AppState 基础层更新

**Files:**
- Modify: `src/npu_webhook/config.py`
- Modify: `src/npu_webhook/app_state.py`

- [ ] **Step 1: 更新 `SearchConfig` 和 `IngestConfig`**

```python
# src/npu_webhook/config.py

class IngestConfig(BaseModel):
    min_content_length: int = 100
    excluded_domains: list[str] = ["mail.google.com", "web.whatsapp.com"]
    max_upload_mb: int = 20  # 新增：文件上传最大尺寸

class SearchConfig(BaseModel):
    default_top_k: int = 10
    rrf_k: int = 60
    vector_weight: float = 0.6
    fulltext_weight: float = 0.4
    injection_budget: int = 2000  # 新增：注入预算字符数
```

- [ ] **Step 2: 更新 `AppState` 新增 session_upload_ids**

```python
# src/npu_webhook/app_state.py

@dataclass
class AppState:
    # ... 现有字段不变 ...
    session_upload_ids: dict = field(default_factory=dict)  # item_id → upload_timestamp
```

`field(default_factory=dict)` 需要从 `dataclasses` 导入。现有文件已有 `from dataclasses import dataclass, field`，确认 `field` 已导入。

- [ ] **Step 3: 运行现有测试验证无回归**

```bash
cd /data/company/project/npu-webhook && source .venv/bin/activate && python -m pytest tests/test_api.py -q
```

预期：全部通过（配置变更向后兼容，新字段有默认值）

- [ ] **Step 4: Commit**

```bash
git add src/npu_webhook/config.py src/npu_webhook/app_state.py
git commit -m "feat: add injection_budget, max_upload_mb config + session_upload_ids state"
```

---

## Task 2: SQLite Schema 迁移 + `enqueue_embedding` 签名

**Files:**
- Modify: `src/npu_webhook/db/sqlite_db.py:137-148` (`_init_schema`)
- Modify: `src/npu_webhook/db/sqlite_db.py:314-327` (`enqueue_embedding`)

- [ ] **Step 1: 写失败测试**

在 `tests/test_search.py` 中添加：

```python
def test_enqueue_embedding_with_level():
    """enqueue_embedding 接受 level / section_idx 参数"""
    import tempfile
    from pathlib import Path
    from npu_webhook.db.sqlite_db import SQLiteDB

    with tempfile.TemporaryDirectory() as tmpdir:
        db = SQLiteDB(Path(tmpdir) / "test.db")
        item_id = db.insert_item(title="t", content="c" * 200, source_type="file")
        qid = db.enqueue_embedding(item_id, chunk_index=0, chunk_text="text",
                                   priority=1, level=1, section_idx=2)
        row = db.conn.execute(
            "SELECT level, section_idx FROM embedding_queue WHERE id = ?", (qid,)
        ).fetchone()
        assert row["level"] == 1
        assert row["section_idx"] == 2
        db.close()
```

- [ ] **Step 2: 运行确认失败**

```bash
python -m pytest tests/test_search.py::test_enqueue_embedding_with_level -v
```

预期：`FAIL`（`enqueue_embedding` 尚不接受 `level` 参数）

- [ ] **Step 3: 更新 `_init_schema` 添加增量迁移**

在 `sqlite_db.py` 的 `_init_schema` 方法中，在 `knowledge_items` 列迁移列表之后，**新增** `embedding_queue` 列迁移：

```python
def _init_schema(self) -> None:
    self.conn.executescript(SCHEMA_SQL)
    # knowledge_items 增量迁移
    for col, default in [
        ("quality_score", "1.0"),
        ("last_used_at", "NULL"),
        ("use_count", "0"),
    ]:
        try:
            self.conn.execute(f"ALTER TABLE knowledge_items ADD COLUMN {col} REAL DEFAULT {default}")
        except sqlite3.OperationalError:
            pass
    # embedding_queue 增量迁移（两层索引元数据）
    for col_def in [
        "level INTEGER NOT NULL DEFAULT 2",
        "section_idx INTEGER NOT NULL DEFAULT 0",
    ]:
        try:
            self.conn.execute(f"ALTER TABLE embedding_queue ADD COLUMN {col_def}")
        except sqlite3.OperationalError:
            pass  # 列已存在
    self.conn.commit()
```

- [ ] **Step 4: 更新 `enqueue_embedding` 签名**

```python
def enqueue_embedding(
    self,
    item_id: str,
    chunk_index: int = 0,
    chunk_text: str = "",
    priority: int = 1,
    level: int = 2,          # 新增：1=章节, 2=段落
    section_idx: int = 0,    # 新增：关联的父章节索引
) -> int:
    cur = self.conn.execute(
        """INSERT INTO embedding_queue (item_id, chunk_index, chunk_text, priority, level, section_idx)
           VALUES (?, ?, ?, ?, ?, ?)""",
        (item_id, chunk_index, chunk_text, priority, level, section_idx),
    )
    self.conn.commit()
    return cur.lastrowid  # type: ignore[return-value]
```

- [ ] **Step 5: 运行测试确认通过**

```bash
python -m pytest tests/test_search.py::test_enqueue_embedding_with_level -v
```

预期：`PASS`

- [ ] **Step 6: 运行全量测试验证无回归**

```bash
python -m pytest tests/ -q --ignore=tests/test_extension.py
```

预期：全部通过（现有调用方未传 `level`/`section_idx`，使用默认值 `level=2, section_idx=0`）

- [ ] **Step 7: Commit**

```bash
git add src/npu_webhook/db/sqlite_db.py tests/test_search.py
git commit -m "feat: add level/section_idx to embedding_queue schema + enqueue_embedding"
```

---

## Task 3: Chunker `extract_sections()`

**Files:**
- Modify: `src/npu_webhook/core/chunker.py`

`extract_sections()` 是独立的纯函数，将内容按语义边界分割为章节列表，不依赖 `chunk()` 的改动。

- [ ] **Step 1: 写失败测试**

新建 `tests/test_chunker.py`（如不存在）或在其中添加：

```python
"""分块策略测试（含新增 extract_sections）"""
from npu_webhook.core.chunker import Chunker


def test_extract_sections_markdown():
    """Markdown 按 ## / ### 标题边界分割"""
    chunker = Chunker()
    text = "# 总览\n引言段落\n\n## 章节一\n内容A\n\n## 章节二\n内容B"
    sections = chunker.extract_sections(text, source_type="webpage")
    assert len(sections) >= 2
    # 每个 section 是 (section_idx, section_text)
    assert all(isinstance(s, tuple) and len(s) == 2 for s in sections)
    assert "章节一" in sections[0][1] or "章节二" in sections[-1][1]


def test_extract_sections_code():
    """代码文件按顶层 def/class 边界分割"""
    chunker = Chunker()
    code = "def foo():\n    pass\n\ndef bar():\n    return 1\n\nclass Baz:\n    pass"
    sections = chunker.extract_sections(code, source_type="file")
    assert len(sections) >= 2


def test_extract_sections_plain_text():
    """纯文本按段落边界每 ~1500 字分割"""
    chunker = Chunker()
    # 3000 字内容，每1500一段
    text = ("这是一个段落。" * 50 + "\n\n") * 4
    sections = chunker.extract_sections(text, source_type="note")
    assert len(sections) >= 2
    # section_idx 从 0 开始递增
    idxs = [s[0] for s in sections]
    assert idxs == list(range(len(sections)))


def test_extract_sections_short_content():
    """短内容（不足一节）返回单节"""
    chunker = Chunker()
    sections = chunker.extract_sections("短文本", source_type="note")
    assert len(sections) == 1
    assert sections[0] == (0, "短文本")
```

- [ ] **Step 2: 运行确认失败**

```bash
python -m pytest tests/test_chunker.py -v
```

预期：`FAIL`（`Chunker` 没有 `extract_sections` 方法）

- [ ] **Step 3: 实现 `extract_sections()`**

在 `chunker.py` 的 `Chunker` 类中添加：

```python
SECTION_SIZE = 1500  # 纯文本每节最大字符数

def extract_sections(self, text: str, source_type: str = "webpage") -> list[tuple[int, str]]:
    """将内容按语义边界切割为章节列表，返回 [(section_idx, section_text), ...]

    - Markdown / webpage: 按 ##/### 标题边界
    - 代码文件 (file): 按顶层 def/class 边界
    - 其他 (note/ai_chat/...): 按段落边界每 SECTION_SIZE 字切割
    """
    text = text.strip()
    if not text:
        return []

    if source_type in ("webpage",) or self._is_markdown(text):
        raw = self._split_by_markdown_headings(text)
    elif source_type == "file" and self._is_code(text):
        raw = self._split_by_code_boundaries(text)
    else:
        raw = self._split_by_paragraphs(text, self.SECTION_SIZE)

    # 过滤空节
    sections = [(i, s.strip()) for i, s in enumerate(raw) if s.strip()]
    return sections if sections else [(0, text)]

@staticmethod
def _is_markdown(text: str) -> bool:
    """启发式：文本中有 ## 标题则视为 Markdown"""
    return any(line.startswith("## ") or line.startswith("### ") for line in text.splitlines()[:50])

@staticmethod
def _is_code(text: str) -> bool:
    """启发式：文本中有 def / class 顶层定义则视为代码"""
    return any(
        line.startswith("def ") or line.startswith("class ") or
        line.startswith("function ") or line.startswith("const ") or
        line.startswith("export ")
        for line in text.splitlines()[:100]
    )

@staticmethod
def _split_by_markdown_headings(text: str) -> list[str]:
    """按 ## 或 ### 标题切分，保留标题行在对应节中"""
    lines = text.splitlines(keepends=True)
    sections: list[list[str]] = [[]]
    for line in lines:
        stripped = line.lstrip()
        if stripped.startswith("## ") or stripped.startswith("### "):
            if any(l.strip() for l in sections[-1]):
                sections.append([])
        sections[-1].append(line)
    return ["".join(s) for s in sections]

@staticmethod
def _split_by_code_boundaries(text: str) -> list[str]:
    """按顶层 def/class/function/const/export 边界切分"""
    import re
    pattern = re.compile(r'^(def |class |function |const |export )', re.MULTILINE)
    positions = [m.start() for m in pattern.finditer(text)]
    if not positions:
        return [text]
    sections = []
    for i, pos in enumerate(positions):
        end = positions[i + 1] if i + 1 < len(positions) else len(text)
        sections.append(text[pos:end])
    # 文件头部（首个定义之前的内容）
    if positions[0] > 0:
        sections.insert(0, text[:positions[0]])
    return sections

@staticmethod
def _split_by_paragraphs(text: str, max_size: int) -> list[str]:
    """按空行分段，积累到 max_size 后切节"""
    paragraphs = re.split(r'\n\s*\n', text) if __import__('re').search(r'\n\s*\n', text) else [text]
    sections: list[str] = []
    current: list[str] = []
    current_len = 0
    for para in paragraphs:
        para = para.strip()
        if not para:
            continue
        if current_len + len(para) > max_size and current:
            sections.append("\n\n".join(current))
            current = []
            current_len = 0
        current.append(para)
        current_len += len(para)
    if current:
        sections.append("\n\n".join(current))
    return sections
```

注意：`_split_by_paragraphs` 用到了 `re`，在方法中 import 或在文件顶部添加 `import re`（推荐顶部）。

- [ ] **Step 4: 在 `chunker.py` 顶部添加 `import re`**

`chunker.py` 当前无 `import`，在文件顶部添加：

```python
import re
```

并将 `_split_by_paragraphs` 中的 `__import__('re')` 替换为直接使用 `re`。

- [ ] **Step 5: 运行测试确认通过**

```bash
python -m pytest tests/test_chunker.py -v
```

预期：4 个测试全部 `PASS`

- [ ] **Step 6: 运行全量测试**

```bash
python -m pytest tests/ -q --ignore=tests/test_extension.py
```

- [ ] **Step 7: Commit**

```bash
git add src/npu_webhook/core/chunker.py tests/test_chunker.py
git commit -m "feat: add Chunker.extract_sections() for semantic section splitting"
```

---

## Task 4: Parser `parse_bytes()`

**Files:**
- Modify: `src/npu_webhook/core/parser.py`

- [ ] **Step 1: 写失败测试**

在 `tests/test_indexer.py` 中添加（或新建 `tests/test_parser.py`）：

```python
def test_parse_bytes_markdown():
    """parse_bytes 解析 Markdown bytes，返回 (title, content)"""
    from npu_webhook.core.parser import parse_bytes
    md = b"# My Title\n\nSome content here."
    title, content = parse_bytes(md, "doc.md")
    assert title == "My Title"
    assert "content" in content


def test_parse_bytes_txt():
    """parse_bytes 解析纯文本"""
    from npu_webhook.core.parser import parse_bytes
    data = b"First line\nSecond line"
    title, content = parse_bytes(data, "notes.txt")
    assert "First line" in content


def test_parse_bytes_unsupported_falls_back():
    """parse_bytes 对未知扩展名不崩溃，当作纯文本"""
    from npu_webhook.core.parser import parse_bytes
    title, content = parse_bytes(b"hello world", "data.unknown")
    assert content  # 不为空
```

- [ ] **Step 2: 运行确认失败**

```bash
python -m pytest tests/ -k "parse_bytes" -v
```

- [ ] **Step 3: 实现 `parse_bytes()`**

在 `parser.py` 末尾添加：

```python
def parse_bytes(data: bytes, filename: str) -> tuple[str, str]:
    """从内存 bytes 解析文件，返回 (title, content)。
    filename 仅用于类型检测（扩展名），不做磁盘操作。
    复用现有各格式解析逻辑。
    """
    suffix = Path(filename).suffix.lower()
    name_stem = Path(filename).stem

    try:
        if suffix == ".pdf":
            return _parse_pdf_bytes(data, name_stem)
        elif suffix == ".docx":
            return _parse_docx_bytes(data, name_stem)
        elif suffix == ".md":
            content = data.decode("utf-8", errors="replace")
            title = name_stem
            for line in content.splitlines():
                if line.strip().startswith("# "):
                    title = line.strip()[2:].strip()
                    break
            return title, content
        elif suffix in CODE_EXTENSIONS:
            return filename, data.decode("utf-8", errors="replace")
        else:
            content = data.decode("utf-8", errors="replace")
            title = content.strip().split("\n", 1)[0][:100] if content.strip() else name_stem
            return title or name_stem, content
    except Exception:
        logger.exception("Failed to parse bytes for: %s", filename)
        return name_stem, ""


def _parse_pdf_bytes(data: bytes, name_stem: str) -> tuple[str, str]:
    import io
    import pymupdf
    doc = pymupdf.open(stream=io.BytesIO(data), filetype="pdf")
    title = doc.metadata.get("title", "") or name_stem
    pages = [page.get_text() for page in doc if page.get_text().strip()]
    doc.close()
    return title, "\n\n".join(pages)


def _parse_docx_bytes(data: bytes, name_stem: str) -> tuple[str, str]:
    import io
    from docx import Document
    doc = Document(io.BytesIO(data))
    title = doc.core_properties.title or name_stem
    paragraphs = [p.text for p in doc.paragraphs if p.text.strip()]
    return title, "\n\n".join(paragraphs)
```

- [ ] **Step 4: 运行测试确认通过**

```bash
python -m pytest tests/ -k "parse_bytes" -v
```

- [ ] **Step 5: Commit**

```bash
git add src/npu_webhook/core/parser.py
git commit -m "feat: add parse_bytes() non-destructive overload for in-memory file parsing"
```

---

## Task 5: Queue Worker — 补充 level/section_idx Metadata

**Files:**
- Modify: `src/npu_webhook/scheduler/queue.py:78-101` (`_process_batch`)

这个改动让 ChromaDB 中存储的 embedding 记录携带 `level` 和 `section_idx`，是两阶段搜索的前提。

- [ ] **Step 1: 写失败测试**

在 `tests/test_search.py` 中添加：

```python
def test_queue_worker_writes_level_metadata():
    """queue worker 处理时 ChromaDB metadata 包含 level 和 section_idx"""
    import tempfile
    from pathlib import Path
    from unittest.mock import MagicMock, patch
    from npu_webhook.db.sqlite_db import SQLiteDB
    from npu_webhook.scheduler.queue import EmbeddingQueueWorker
    from npu_webhook.core.vectorstore import VectorStore
    from npu_webhook.db.chroma_db import ChromaDB

    with tempfile.TemporaryDirectory() as tmpdir:
        db = SQLiteDB(Path(tmpdir) / "test.db")
        chroma = ChromaDB(Path(tmpdir) / "chroma")

        # Mock embedding engine 返回固定向量
        mock_engine = MagicMock()
        mock_engine.embed.return_value = [[0.1] * 256]

        vs = VectorStore(chroma, engine=mock_engine)
        worker = EmbeddingQueueWorker(db=db, vector_store=vs)

        item_id = db.insert_item(title="t", content="c" * 200, source_type="file")
        db.enqueue_embedding(item_id, chunk_index=0, chunk_text="章节内容",
                             priority=1, level=1, section_idx=3)

        worker._process_batch()

        # 验证 ChromaDB 中存储了 level 和 section_idx
        results = chroma.query([0.1] * 256, top_k=1)
        assert results["metadatas"][0][0]["level"] == 1
        assert results["metadatas"][0][0]["section_idx"] == 3

        db.close()
```

- [ ] **Step 2: 运行确认失败**

```bash
python -m pytest tests/test_search.py::test_queue_worker_writes_level_metadata -v
```

- [ ] **Step 3: 更新 `_process_batch` metadata 构造**

在 `scheduler/queue.py` 的 `_process_batch` 方法中，找到 `metadatas.append(...)` 这段，修改为：

```python
metadatas.append({
    "item_id": item_id,
    "chunk_index": chunk_index,
    "source_type": item["source_type"] if item else "",
    "created_at": item["created_at"] if item else "",
    "level": task.get("level", 2),            # 新增
    "section_idx": task.get("section_idx", 0), # 新增
})
```

`task` 是 `dict(r)` 的结果，`dequeue_embeddings` 返回完整行数据，迁移后包含这两列。

- [ ] **Step 4: 运行测试确认通过**

```bash
python -m pytest tests/test_search.py::test_queue_worker_writes_level_metadata -v
```

- [ ] **Step 5: Commit**

```bash
git add src/npu_webhook/scheduler/queue.py tests/test_search.py
git commit -m "feat: pass level/section_idx to ChromaDB metadata in queue worker"
```

---

## Task 6: Pipeline 两层入队

**Files:**
- Modify: `src/npu_webhook/indexer/pipeline.py`

`pipeline.py` 的 `process_file()` 目前只入队 Level 2 段落，需同时入队 Level 1 章节。

- [ ] **Step 1: 写失败测试**

在 `tests/test_indexer.py` 中添加：

```python
def test_pipeline_enqueues_two_levels():
    """process_file 应同时入队 Level 1（章节）和 Level 2（段落）"""
    import tempfile
    from pathlib import Path
    from npu_webhook.core.chunker import Chunker
    from npu_webhook.core.vectorstore import VectorStore
    from npu_webhook.db.chroma_db import ChromaDB
    from npu_webhook.db.sqlite_db import SQLiteDB
    from npu_webhook.indexer.pipeline import IndexPipeline

    with tempfile.TemporaryDirectory() as tmpdir:
        db = SQLiteDB(Path(tmpdir) / "test.db")
        chroma = ChromaDB(Path(tmpdir) / "chroma")
        vs = VectorStore(chroma, engine=None)
        chunker = Chunker()
        pipeline = IndexPipeline(db, chunker, vs)

        # 创建有两个 ## 章节的 Markdown 文件
        md_file = Path(tmpdir) / "test.md"
        md_file.write_text("## 章节一\n内容A " * 50 + "\n\n## 章节二\n内容B " * 50)

        pipeline.process_file(str(md_file), dir_id="test")

        # 验证队列中同时有 level=1 和 level=2 的任务
        rows = db.conn.execute(
            "SELECT level FROM embedding_queue GROUP BY level"
        ).fetchall()
        levels = {r["level"] for r in rows}
        assert 1 in levels, "应有 Level 1 章节任务"
        assert 2 in levels, "应有 Level 2 段落任务"

        db.close()
```

- [ ] **Step 2: 运行确认失败**

```bash
python -m pytest tests/test_indexer.py::test_pipeline_enqueues_two_levels -v
```

- [ ] **Step 3: 更新 `pipeline.py` 两层入队**

在 `process_file()` 中，找到"分块并投递 embedding 队列"部分，替换为：

```python
# 提取章节（Level 1）
from npu_webhook.core.chunker import Chunker as _Chunker  # 已在类属性中
sections = self.chunker.extract_sections(content, source_type="file")

# Level 1：每个章节整体入队
for section_idx, section_text in sections:
    if section_text.strip():
        self.db.enqueue_embedding(
            item_id=item_id,
            chunk_index=section_idx,  # Level 1 用 section_idx 作 chunk_index
            chunk_text=section_text,
            priority=priority,
            level=1,
            section_idx=section_idx,
        )

# Level 2：每个章节再细分为段落块
chunk_counter = 0
for section_idx, section_text in sections:
    chunks = self.chunker.chunk(section_text)
    for chunk_text in chunks:
        self.db.enqueue_embedding(
            item_id=item_id,
            chunk_index=chunk_counter,
            chunk_text=chunk_text,
            priority=priority,
            level=2,
            section_idx=section_idx,
        )
        chunk_counter += 1
```

同时更新文件更新路径（`item_id` 已有时）：**删除旧向量 + 取消旧队列任务** 在现有代码中已正确处理（`cancel_embeddings_for_item` + `delete_by_item_ids`），重建时走上述两层入队即可。

> **注意：** `ingest.py` 的 `/ingest` 端点同样需要相同更新（AI 对话捕获也需两层索引）。在 `api/ingest.py` 中找到 for 循环，用相同模式替换。

- [ ] **Step 4: 同步更新 `api/ingest.py`**

```python
# api/ingest.py — 两层入队替换原来的单层
if state.chunker:
    from npu_webhook.core.chunker import Chunker
    sections = state.chunker.extract_sections(req.content, source_type=req.source_type)

    # Level 1: 章节
    for section_idx, section_text in sections:
        if section_text.strip():
            state.db.enqueue_embedding(
                item_id=item_id,
                chunk_index=section_idx,
                chunk_text=section_text,
                priority=1,
                level=1,
                section_idx=section_idx,
            )

    # Level 2: 段落块
    chunk_counter = 0
    for section_idx, section_text in sections:
        chunks = state.chunker.chunk(section_text)
        for chunk_text in chunks:
            state.db.enqueue_embedding(
                item_id=item_id,
                chunk_index=chunk_counter,
                chunk_text=chunk_text,
                priority=1,
                level=2,
                section_idx=section_idx,
            )
            chunk_counter += 1
```

- [ ] **Step 5: 运行测试确认通过**

```bash
python -m pytest tests/test_indexer.py::test_pipeline_enqueues_two_levels -v
```

- [ ] **Step 6: 运行全量测试**

```bash
python -m pytest tests/ -q --ignore=tests/test_extension.py
```

- [ ] **Step 7: Commit**

```bash
git add src/npu_webhook/indexer/pipeline.py src/npu_webhook/api/ingest.py
git commit -m "feat: pipeline and ingest enqueue both Level 1 (section) and Level 2 (chunk)"
```

---

## Task 7: 层级检索引擎 + 动态预算

**Files:**
- Modify: `src/npu_webhook/core/search.py`

这是本次最核心的改动：新增 `_allocate_budget()` 函数和两阶段层级搜索方法，替换 `search()` 中的注入截断逻辑。

- [ ] **Step 1: 写失败测试**

在 `tests/test_search.py` 中添加：

```python
def test_allocate_budget_weighted():
    """_allocate_budget 按 score 加权分配，总量不超过预算"""
    from npu_webhook.core.search import _allocate_budget
    results = [
        {"score": 0.8, "content": "A" * 500},
        {"score": 0.2, "content": "B" * 500},
    ]
    allocated = _allocate_budget(results, budget=1000)
    total = sum(len(r["inject_content"]) for r in allocated)
    assert total <= 1000
    # 高分项分配更多
    assert len(allocated[0]["inject_content"]) > len(allocated[1]["inject_content"])


def test_allocate_budget_zero_score_fallback():
    """total_score=0 时均分而非除零"""
    from npu_webhook.core.search import _allocate_budget
    results = [
        {"score": 0.0, "content": "A" * 600},
        {"score": 0.0, "content": "B" * 600},
    ]
    allocated = _allocate_budget(results, budget=1000)
    # 不应抛出异常，且每项都有分配
    assert all("inject_content" in r for r in allocated)


def test_allocate_budget_minimum_per_item():
    """每项最少分配 100 字"""
    from npu_webhook.core.search import _allocate_budget
    results = [{"score": 1.0, "content": "X" * 1000}]
    allocated = _allocate_budget(results, budget=50)  # 预算小于最小值
    assert len(allocated[0]["inject_content"]) >= 50  # 至少分配预算全部
```

- [ ] **Step 2: 运行确认失败**

```bash
python -m pytest tests/test_search.py -k "allocate_budget" -v
```

- [ ] **Step 3: 实现 `_allocate_budget()`**

在 `search.py` 中添加（在 `HybridSearchEngine` 类定义之前）：

```python
INJECTION_BUDGET = 2000  # 默认注入预算（字符数）


def _allocate_budget(results: list[dict], budget: int) -> list[dict]:
    """按 score 加权分配注入预算，防零除。

    每项最少 100 字（或均分兜底）。结果原地修改并返回。
    """
    total_score = sum(r.get("score", 0) for r in results)
    if total_score <= 0:
        per_item = budget // max(len(results), 1)
        for r in results:
            r["inject_content"] = r.get("content", "")[:per_item]
        return results
    for r in results:
        share = r.get("score", 0) / total_score
        alloc = max(int(budget * share), 100)
        r["inject_content"] = r.get("content", "")[:alloc]
    return results
```

- [ ] **Step 4: 运行测试确认通过**

```bash
python -m pytest tests/test_search.py -k "allocate_budget" -v
```

- [ ] **Step 5: 添加 `search_relevant()` 方法（供 `/search/relevant` 调用）**

在 `HybridSearchEngine` 类中添加新方法（在 `search()` 之后）：

```python
def search_relevant(
    self,
    query: str,
    top_k: int = 3,
    source_types: list[str] | None = None,
    context: list[str] | None = None,
    min_score: float = 0.0,
    injection_budget: int = INJECTION_BUDGET,
) -> list[dict]:
    """两阶段层级检索：章节锚定 → 段落精排 → 父章节上下文扩展 + 动态预算分配。

    如果向量引擎不可用，回退到普通 search()。
    """
    if not self.vector_store.available:
        results = self.search(query, top_k=top_k, source_types=source_types,
                              context=context, min_score=min_score)
        return _allocate_budget(results, injection_budget)

    search_query = query
    if context:
        ctx_text = " | ".join(c[:150] for c in context[-3:])
        search_query = f"{ctx_text} || {query}"

    query_embedding = self.vector_store.engine.embed([search_query])[0]  # type: ignore[union-attr]

    # Stage 1: 章节级召回（Level 1）
    where_l1: dict = {"level": {"$eq": 1}}
    if source_types:
        where_l1 = {"$and": [where_l1, {"source_type": {"$in": source_types}}]}
    section_hits = self.vector_store.chroma.query(query_embedding, top_k=5, where=where_l1)

    candidate_sections: list[int] = []
    candidate_item_ids: list[str] = []
    if section_hits and section_hits.get("metadatas"):
        for meta in section_hits["metadatas"][0]:
            sidx = meta.get("section_idx", 0)
            iid = meta.get("item_id", "")
            if sidx not in candidate_sections:
                candidate_sections.append(sidx)
            if iid not in candidate_item_ids:
                candidate_item_ids.append(iid)

    # Stage 2: 段落级精排（Level 2，限定在候选章节内）
    if candidate_sections and candidate_item_ids:
        where_l2: dict = {
            "$and": [
                {"level": {"$eq": 2}},
                {"section_idx": {"$in": candidate_sections}},
            ]
        }
        if source_types:
            where_l2["$and"].append({"source_type": {"$in": source_types}})
        chunk_hits = self.vector_store.chroma.query(query_embedding, top_k=top_k * 2, where=where_l2)
    else:
        # 无章节命中，回退到全 Level 2 搜索
        where_l2 = {"level": {"$eq": 2}}
        if source_types:
            where_l2 = {"$and": [where_l2, {"source_type": {"$in": source_types}}]}
        chunk_hits = self.vector_store.chroma.query(query_embedding, top_k=top_k * 2, where=where_l2)

    # Stage 3: 上下文扩展（用父章节全文替代 512 字截断片段）
    results: list[dict] = []
    seen_items: set[str] = set()

    if chunk_hits and chunk_hits.get("ids"):
        ids = chunk_hits["ids"][0]
        distances = chunk_hits.get("distances", [[]])[0]
        metadatas = chunk_hits.get("metadatas", [[]])[0]

        for i, doc_id in enumerate(ids):
            meta = metadatas[i] if i < len(metadatas) else {}
            item_id = meta.get("item_id", "")
            if item_id in seen_items:
                continue
            seen_items.add(item_id)

            score = 1.0 - (distances[i] if i < len(distances) else 0)
            if score < min_score:
                continue

            db_item = self.db.get_item(item_id)
            if not db_item:
                continue

            # 取父章节全文（Level 1，同 section_idx）
            section_idx = meta.get("section_idx", 0)
            parent_where = {
                "$and": [
                    {"level": {"$eq": 1}},
                    {"item_id": {"$eq": item_id}},
                    {"section_idx": {"$eq": section_idx}},
                ]
            }
            try:
                parent_hits = self.vector_store.chroma.query(
                    query_embedding, top_k=1, where=parent_where
                )
                if parent_hits and parent_hits.get("documents") and parent_hits["documents"][0]:
                    inject_content = parent_hits["documents"][0][0]
                else:
                    inject_content = db_item["content"]
            except Exception:
                inject_content = db_item["content"]

            results.append({
                "id": item_id,
                "title": db_item["title"],
                "content": inject_content,
                "source_type": db_item["source_type"],
                "url": db_item.get("url"),
                "created_at": db_item.get("created_at"),
                "score": score,
                "section_idx": section_idx,
            })
            if len(results) >= top_k:
                break

    # 回退：没有层级结果则用普通搜索
    if not results:
        results = self.search(query, top_k=top_k, source_types=source_types,
                              context=context, min_score=min_score)

    return _allocate_budget(results, injection_budget)
```

- [ ] **Step 6: 修改 `api/search.py` 调用 `search_relevant()`（必做）**

`api/search.py` 中 `/search/relevant` 端点当前调用的是 `state.search_engine.search(...)`，需改为 `search_relevant()`，否则两阶段层级检索不会触发。

找到 `api/search.py` 中处理 `POST /search/relevant` 的函数（关键字 `relevant`），将搜索调用改为：

```python
# 修改前（旧调用）：
results = state.search_engine.search(
    query=req.query, top_k=req.top_k, ...
)

# 修改后（层级检索）：
from npu_webhook.config import settings as _settings
results = state.search_engine.search_relevant(
    query=req.query,
    top_k=req.top_k,
    source_types=req.source_types,
    context=req.context,
    min_score=req.min_score or 0.0,
    injection_budget=_settings.search.injection_budget,
)
```

返回值格式与原 `search()` 兼容（都是 `list[dict]`），无需更改响应模型。

- [ ] **Step 7: 运行全量测试**

```bash
python -m pytest tests/ -q --ignore=tests/test_extension.py
```

- [ ] **Step 8: Commit**

```bash
git add src/npu_webhook/core/search.py src/npu_webhook/api/search.py tests/test_search.py
git commit -m "feat: hierarchical two-stage search + _allocate_budget dynamic injection budget"
```

---

## Task 8: 文件上传 API

**Files:**
- Create: `src/npu_webhook/api/upload.py`
- Create: `tests/test_upload.py`

- [ ] **Step 1: 写失败测试**

新建 `tests/test_upload.py`：

```python
"""POST /upload 端点测试"""
import io
import pytest
from httpx import ASGITransport, AsyncClient
from npu_webhook.main import app


@pytest.mark.asyncio
async def test_upload_markdown():
    """上传 Markdown 文件返回 item_id 和 chunks_queued"""
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        content = ("# 测试文档\n\n## 章节一\n" + "内容A " * 100 + "\n\n## 章节二\n" + "内容B " * 100).encode()
        resp = await client.post(
            "/api/v1/upload",
            files={"file": ("test.md", io.BytesIO(content), "text/markdown")},
        )
        assert resp.status_code == 200
        data = resp.json()
        assert "id" in data
        assert data["chunks_queued"] > 0
        assert data["status"] == "processing"


@pytest.mark.asyncio
async def test_upload_too_large():
    """文件超过 20MB 返回 413"""
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        large_content = b"x" * (21 * 1024 * 1024)  # 21MB
        resp = await client.post(
            "/api/v1/upload",
            files={"file": ("big.txt", io.BytesIO(large_content), "text/plain")},
        )
        assert resp.status_code == 413


@pytest.mark.asyncio
async def test_upload_unsupported_format():
    """不支持的格式返回 415"""
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        resp = await client.post(
            "/api/v1/upload",
            files={"file": ("image.png", io.BytesIO(b"\x89PNG"), "image/png")},
        )
        assert resp.status_code == 415


@pytest.mark.asyncio
async def test_upload_with_session_id():
    """带 session_id 的上传，item_id 应记录到 session_upload_ids"""
    from npu_webhook.app_state import state
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        content = ("# 会话测试\n\n" + "内容 " * 200).encode()
        resp = await client.post(
            "/api/v1/upload",
            files={"file": ("session_test.md", io.BytesIO(content), "text/markdown")},
            data={"session_id": "test-session-001"},
        )
        assert resp.status_code == 200
        item_id = resp.json()["id"]
        assert item_id in state.session_upload_ids
```

- [ ] **Step 2: 运行确认失败**

```bash
python -m pytest tests/test_upload.py -v
```

预期：`FAIL`（`/api/v1/upload` 尚不存在）

- [ ] **Step 3: 创建 `api/upload.py`**

新建 `src/npu_webhook/api/upload.py`：

```python
"""POST /upload - 原始文件上传端点"""
import logging
import time
from pathlib import Path

from fastapi import APIRouter, Form, HTTPException, UploadFile

from npu_webhook.app_state import state
from npu_webhook.config import settings
from npu_webhook.core.parser import parse_bytes

logger = logging.getLogger(__name__)

router = APIRouter(prefix="/api/v1", tags=["upload"])

# 支持的格式白名单（复用 parser.py 能力）
ALLOWED_EXTENSIONS = {".pdf", ".docx", ".md", ".txt", ".py", ".js", ".ts", ".jsx", ".tsx"}

SESSION_TTL = 86400  # 24h


@router.post("/upload")
async def upload_file(
    file: UploadFile,
    session_id: str | None = Form(None),
) -> dict:
    """接收原始文件，自动解析并两层入库（FTS5 立即可搜，向量搜索异步就绪）"""
    if not state.db:
        raise HTTPException(status_code=503, detail="Database not initialized")

    # 格式检查
    suffix = Path(file.filename or "").suffix.lower()
    if suffix not in ALLOWED_EXTENSIONS:
        raise HTTPException(status_code=415, detail=f"Unsupported file type: {suffix}")

    # 读取文件内容
    data = await file.read()

    # 大小检查
    max_bytes = settings.ingest.max_upload_mb * 1024 * 1024
    if len(data) > max_bytes:
        raise HTTPException(
            status_code=413,
            detail=f"File too large (max {settings.ingest.max_upload_mb}MB)",
        )

    # 解析
    try:
        title, content = parse_bytes(data, file.filename or "upload")
    except Exception as e:
        logger.exception("Failed to parse uploaded file: %s", file.filename)
        raise HTTPException(status_code=422, detail=f"Failed to parse file: {e}") from e

    if not content.strip():
        raise HTTPException(status_code=422, detail="File content is empty after parsing")

    # 存入 SQLite（FTS5 立即可搜）
    item_id = state.db.insert_item(
        title=title,
        content=content,
        source_type="file",
        metadata={"filename": file.filename, "upload_source": "browser"},
    )

    # 两层 embedding 入队
    chunks_queued = 0
    if state.chunker:
        sections = state.chunker.extract_sections(content, source_type="file")

        # Level 1: 章节
        for section_idx, section_text in sections:
            if section_text.strip():
                state.db.enqueue_embedding(
                    item_id=item_id,
                    chunk_index=section_idx,
                    chunk_text=section_text,
                    priority=1,
                    level=1,
                    section_idx=section_idx,
                )
                chunks_queued += 1

        # Level 2: 段落块
        chunk_counter = 0
        for section_idx, section_text in sections:
            chunks = state.chunker.chunk(section_text)
            for chunk_text in chunks:
                state.db.enqueue_embedding(
                    item_id=item_id,
                    chunk_index=chunk_counter,
                    chunk_text=chunk_text,
                    priority=1,
                    level=2,
                    section_idx=section_idx,
                )
                chunk_counter += 1
                chunks_queued += 1

    # 记录 session_upload_ids（用于注入加权）
    if session_id and state.session_upload_ids is not None:
        # 清理过期条目
        now = time.time()
        expired = [k for k, ts in state.session_upload_ids.items() if now - ts > SESSION_TTL]
        for k in expired:
            del state.session_upload_ids[k]
        state.session_upload_ids[item_id] = now

    logger.info("Uploaded and indexed: %s (%d queue tasks)", title, chunks_queued)
    return {
        "id": item_id,
        "title": title,
        "chunks_queued": chunks_queued,
        "status": "processing",
    }
```

- [ ] **Step 4: 运行测试（upload_markdown, too_large, unsupported_format 应通过）**

```bash
python -m pytest tests/test_upload.py -v -k "not session_id"
```

注意：`test_upload_too_large` 需要读 21MB 字节，测试会较慢但应通过。

- [ ] **Step 5: 修复 session_id 测试（在 main.py 注册后运行）**

等 Task 9 完成后再运行 `test_upload_with_session_id`。

- [ ] **Step 6: Commit**

```bash
git add src/npu_webhook/api/upload.py tests/test_upload.py
git commit -m "feat: add POST /api/v1/upload multipart file upload endpoint"
```

---

## Task 9: Main.py 注册 Upload Router

**Files:**
- Modify: `src/npu_webhook/main.py`

- [ ] **Step 1: 注册路由**

在 `main.py` 中，找到 `from npu_webhook.api.setup import router as setup_router` 之后，添加：

```python
from npu_webhook.api.upload import router as upload_router
```

并在 `app.include_router(setup_router)` 之后添加：

```python
app.include_router(upload_router)
```

- [ ] **Step 2: 运行全量上传测试**

```bash
python -m pytest tests/test_upload.py -v
```

预期：4 个测试全部 `PASS`

- [ ] **Step 3: 运行全量测试**

```bash
python -m pytest tests/ -q --ignore=tests/test_extension.py
```

- [ ] **Step 4: Commit**

```bash
git add src/npu_webhook/main.py
git commit -m "feat: register upload router in main.py"
```

---

## Task 10: 扩展 FilePage.jsx

**Files:**
- Create: `extension/src/sidepanel/pages/FilePage.jsx`

- [ ] **Step 1: 创建 `FilePage.jsx`**

```jsx
// extension/src/sidepanel/pages/FilePage.jsx
import { h } from 'preact';
import { useState, useRef } from 'preact/hooks';
import { api } from '../../shared/api.js';
import { MSG } from '../../shared/messages.js';

const ALLOWED_TYPES = ['.pdf', '.docx', '.md', '.txt', '.py', '.js', '.ts'];
const SESSION_ID_KEY = 'npu_session_id';

function getSessionId() {
  let sid = sessionStorage.getItem(SESSION_ID_KEY);
  if (!sid) {
    sid = Math.random().toString(36).slice(2);
    sessionStorage.setItem(SESSION_ID_KEY, sid);
  }
  return sid;
}

export default function FilePage() {
  const [files, setFiles] = useState([]);   // [{name, id, status, chunks}]
  const [dragging, setDragging] = useState(false);
  const [uploading, setUploading] = useState(false);
  const inputRef = useRef(null);

  async function uploadFile(file) {
    const ext = '.' + file.name.split('.').pop().toLowerCase();
    if (!ALLOWED_TYPES.includes(ext)) {
      alert(`不支持的格式：${ext}。支持：${ALLOWED_TYPES.join(' ')}`);
      return;
    }
    setUploading(true);
    setFiles((prev) => [...prev, { name: file.name, id: null, status: 'uploading', chunks: 0 }]);
    try {
      const result = await api.uploadFile(file, getSessionId());
      setFiles((prev) =>
        prev.map((f) =>
          f.name === file.name && f.status === 'uploading'
            ? { ...f, id: result.id, status: 'done', chunks: result.chunks_queued }
            : f,
        ),
      );
    } catch (err) {
      setFiles((prev) =>
        prev.map((f) =>
          f.name === file.name && f.status === 'uploading' ? { ...f, status: 'error' } : f,
        ),
      );
    } finally {
      setUploading(false);
    }
  }

  function handleDrop(e) {
    e.preventDefault();
    setDragging(false);
    Array.from(e.dataTransfer.files).forEach(uploadFile);
  }

  async function handleDelete(id) {
    if (!id) return;
    try {
      await api.deleteItem(id);
      setFiles((prev) => prev.filter((f) => f.id !== id));
    } catch { /* */ }
  }

  return (
    <div class="fp-container">
      <div
        class={`fp-dropzone${dragging ? ' fp-dropzone--active' : ''}`}
        onDragOver={(e) => { e.preventDefault(); setDragging(true); }}
        onDragLeave={() => setDragging(false)}
        onDrop={handleDrop}
        onClick={() => inputRef.current?.click()}
      >
        <span>拖拽文件到此处，或点击选择</span>
        <span class="fp-hint">支持：PDF DOCX MD TXT Python JS TS</span>
        <input
          ref={inputRef}
          type="file"
          accept={ALLOWED_TYPES.join(',')}
          multiple
          style="display:none"
          onChange={(e) => Array.from(e.target.files).forEach(uploadFile)}
        />
      </div>

      {files.length > 0 && (
        <ul class="fp-list">
          {files.map((f, i) => (
            <li key={i} class="fp-item">
              <span class={`fp-status fp-status--${f.status}`}>
                {f.status === 'uploading' ? '上传中...' : f.status === 'done' ? '✓' : '✗'}
              </span>
              <span class="fp-name">{f.name}</span>
              {f.status === 'done' && (
                <span class="fp-meta">已处理（{f.chunks} 个段落）</span>
              )}
              {f.id && (
                <button class="fp-btn-delete" onClick={() => handleDelete(f.id)}>删除</button>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
```

- [ ] **Step 2: 在 `sidepanel.css` 中添加文件页样式**

在 `extension/src/sidepanel/sidepanel.css` 末尾追加：

```css
/* FilePage */
.fp-container { padding: 12px; }
.fp-dropzone {
  border: 2px dashed #555; border-radius: 8px;
  padding: 24px 16px; text-align: center; cursor: pointer;
  display: flex; flex-direction: column; gap: 6px;
}
.fp-dropzone--active { border-color: #4a9eff; background: rgba(74,158,255,0.05); }
.fp-hint { font-size: 11px; color: #888; }
.fp-list { list-style: none; padding: 0; margin: 12px 0 0; display: flex; flex-direction: column; gap: 8px; }
.fp-item { display: flex; align-items: center; gap: 8px; font-size: 13px; }
.fp-status--uploading { color: #f0a500; }
.fp-status--done { color: #4caf50; }
.fp-status--error { color: #f44336; }
.fp-meta { color: #888; font-size: 11px; }
.fp-btn-delete { margin-left: auto; font-size: 11px; color: #f44336; background: none; border: none; cursor: pointer; }
```

- [ ] **Step 3: Commit**

```bash
git add extension/src/sidepanel/pages/FilePage.jsx extension/src/sidepanel/sidepanel.css
git commit -m "feat: add FilePage.jsx file upload tab for sidepanel"
```

---

## Task 11: 扩展 App.jsx + api.js + worker.js

**Files:**
- Modify: `extension/src/sidepanel/App.jsx`
- Modify: `extension/src/shared/api.js`
- Modify: `extension/src/background/worker.js`

- [ ] **Step 1: 更新 `App.jsx` 注册第四个标签（增量修改，不替换整个文件）**

使用 Edit 工具进行增量修改，保留现有 `App` 函数体不变：

**修改1** — 添加 FilePage import（在 StatusPage import 之后）：
```js
import FilePage from './pages/FilePage.jsx';
```

**修改2** — 在 TABS 数组中，在 `status` 之前插入：
```js
{ id: 'files', label: '文件' },
```

**修改3** — 在 PAGE_MAP 中添加：
```js
files: FilePage,
```

最终 `App.jsx` 的 TABS 和 PAGE_MAP 应为：

```jsx
const TABS = [
  { id: 'search', label: '搜索' },
  { id: 'timeline', label: '时间线' },
  { id: 'files', label: '文件' },
  { id: 'status', label: '状态' },
];

const PAGE_MAP = {
  search: SearchPage,
  timeline: TimelinePage,
  files: FilePage,
  status: StatusPage,
};
```

`App` 函数体（JSX render 逻辑）完全不变。

- [ ] **Step 2: 在 `api.js` 中新增 `uploadFile()`**

在 `api.js` 的 `API` 类末尾添加：

```js
/** 上传原始文件到 /upload（multipart/form-data）*/
async uploadFile(file, sessionId = null) {
  const form = new FormData();
  form.append('file', file);
  if (sessionId) form.append('session_id', sessionId);
  const resp = await fetch(`${this.baseUrl}/upload`, {
    method: 'POST',
    body: form,
    // 不设 Content-Type，让浏览器自动设置 boundary
  });
  if (!resp.ok) throw new Error(`Upload error: ${resp.status}`);
  return resp.json();
}
```

- [ ] **Step 3: 在 `worker.js` 中添加会话感知加权**

在 `worker.js` 中，找到 `SEARCH_RELEVANT` 的 handler，在返回结果之前添加加权逻辑。需要维护一个内存 Set 记录当前会话上传的 item_id：

```js
// 在文件顶部 dedup 变量声明附近添加：
const sessionUploadedIds = new Set();

// 在 MSG.UPLOAD_COMPLETE 处理（或在 SEARCH_RELEVANT 返回结果时）添加：
// handleMessage 中 SEARCH_RELEVANT case 末尾：
case MSG.SEARCH_RELEVANT: {
  // ... 现有逻辑 ...
  const results = await api.searchRelevant({...});
  // 会话感知加权：本次会话上传的文件优先
  if (Array.isArray(results?.results)) {
    for (const r of results.results) {
      if (sessionUploadedIds.has(r.id)) r.score = (r.score || 0) * 1.5;
    }
    results.results.sort((a, b) => (b.score || 0) - (a.score || 0));
  }
  return results;
}
```

同时添加 `MSG.FILE_UPLOADED` 消息处理，让 FilePage 上传成功后通知 worker：

```js
case MSG.FILE_UPLOADED:
  if (msg.item_id) sessionUploadedIds.add(msg.item_id);
  return { ok: true };
```

在 `messages.js` 中的 MSG 常量添加：

```js
FILE_UPLOADED: 'FILE_UPLOADED',
```

并在 FilePage 上传成功后发送：

```js
// FilePage.jsx uploadFile 成功后
chrome.runtime.sendMessage({ type: MSG.FILE_UPLOADED, item_id: result.id });
```

- [ ] **Step 4: 构建验证**

```bash
cd /data/company/project/npu-webhook/extension && npm run build 2>&1 | tail -20
```

预期：构建成功，无错误

- [ ] **Step 5: Commit**

```bash
git add extension/src/sidepanel/App.jsx extension/src/shared/api.js \
        extension/src/background/worker.js extension/src/shared/messages.js
git commit -m "feat: add file tab to sidepanel, uploadFile() API, session-aware score weighting"
```

---

## Task 12: 系统托盘 `tray.py`

**Files:**
- Create: `src/npu_webhook/tray.py`
- Modify: `packaging/pyinstaller.spec` (已有 spec 文件时添加 pystray 依赖)

`pystray` + `Pillow` 提供跨平台系统托盘，uvicorn 在子线程运行，主线程为托盘 GUI 事件循环。

- [ ] **Step 1: 创建 `tray.py`**

```python
"""系统托盘入口：pystray + uvicorn 子线程"""
import threading
import logging
import time
from typing import Any

logger = logging.getLogger(__name__)


def _create_icon() -> Any:
    """创建简单的绿色圆形托盘图标"""
    from PIL import Image, ImageDraw
    img = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    draw.ellipse([8, 8, 56, 56], fill=(76, 175, 80, 255))  # 绿色
    return img


def _start_server(stop_event: threading.Event) -> None:
    """在子线程中运行 uvicorn"""
    import uvicorn
    from npu_webhook.config import settings
    from npu_webhook.main import app

    config = uvicorn.Config(
        app=app,
        host=settings.server.host,
        port=settings.server.port,
        log_level="info",
    )
    server = uvicorn.Server(config)

    # 监听 stop_event，优雅关闭
    def _check_stop() -> None:
        while not stop_event.is_set():
            time.sleep(1)
        server.should_exit = True

    threading.Thread(target=_check_stop, daemon=True).start()
    server.run()


def main() -> None:
    """系统托盘主进程"""
    import pystray
    from pystray import MenuItem as item

    stop_event = threading.Event()

    # 启动 uvicorn 子线程
    server_thread = threading.Thread(
        target=_start_server, args=(stop_event,), daemon=True, name="uvicorn"
    )
    server_thread.start()

    def on_quit(icon: Any, item_: Any) -> None:
        stop_event.set()
        icon.stop()

    def on_open_browser(icon: Any, item_: Any) -> None:
        import webbrowser
        webbrowser.open("http://localhost:18900")

    icon = pystray.Icon(
        "npu-webhook",
        _create_icon(),
        "npu-webhook",
        menu=pystray.Menu(
            item("打开状态页", on_open_browser),
            item("退出", on_quit),
        ),
    )
    logger.info("System tray started")
    icon.run()


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: 验证托盘可导入（不运行 GUI）**

```bash
python -c "from npu_webhook.tray import _create_icon; print('tray.py OK')"
```

预期：`tray.py OK`（如果 pystray/Pillow 未安装会有 ImportError，按需安装）

- [ ] **Step 3: 检查 pystray/Pillow 依赖**

```bash
cd /data/company/project/npu-webhook && source .venv/bin/activate
pip show pystray pillow 2>&1 | grep -E "^Name|not found"
```

如未安装：

```bash
pip install "pystray>=0.19" "Pillow>=10"
```

并在 `pyproject.toml` 的 optional dependencies 中记录（或在 packaging 专用 requirements 中）。

- [ ] **Step 4: Commit**

```bash
git add src/npu_webhook/tray.py
git commit -m "feat: add system tray entry point (pystray + uvicorn thread)"
```

---

## Task 13: 全量验证

- [ ] **Step 1: 运行全量后端测试**

```bash
cd /data/company/project/npu-webhook && source .venv/bin/activate
python -m pytest tests/ -q --ignore=tests/test_extension.py -v
```

预期：全部通过（包括所有新增测试）

- [ ] **Step 2: 运行扩展构建**

```bash
cd extension && npm run build 2>&1 | tail -30
```

预期：无报错，`dist/` 产出正常

- [ ] **Step 3: 启动后端验证 API**

```bash
# 终端1：启动后端
source .venv/bin/activate && python -m npu_webhook.main &
sleep 3
# 验证上传
curl -s -X POST http://localhost:18900/api/v1/upload \
  -F "file=@README.md" | python -m json.tool
# 验证健康检查
curl -s http://localhost:18900/api/v1/status/health
```

- [ ] **Step 4: 最终 commit**

```bash
git add -A
git commit -m "chore: Phase 3 long-text + packaging complete - all tests passing"
```

---

## 依赖顺序速查

```
Task 1 (config/state)
  └─→ Task 2 (schema migration)
        └─→ Task 5 (queue metadata)
              └─→ Task 7 (hierarchical search)
  └─→ Task 3 (extract_sections)
        └─→ Task 6 (pipeline two-layer)
              └─→ Task 7 (hierarchical search)
  └─→ Task 4 (parse_bytes)
        └─→ Task 8 (upload API)
              └─→ Task 9 (main.py router)

Task 10 (FilePage) ─ 独立
Task 11 (App/api/worker) ─ 依赖 Task 10
Task 12 (tray) ─ 独立
Task 13 (验证) ─ 依赖全部
```
