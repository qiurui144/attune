"""文档分块策略：滑动窗口分块"""

import re


class Chunker:
    """滑动窗口分块器

    按字符数分块（中文1字符≈1token，适用于 bge 模型）。
    优先在句子边界（。！？\\n）处分割。
    """

    SECTION_SIZE = 1500  # 纯文本每节最大字符数

    def __init__(self, chunk_size: int = 512, overlap: int = 128) -> None:
        self.chunk_size = chunk_size
        self.overlap = overlap

    def chunk(self, text: str) -> list[str]:
        """将文本分块，返回块列表"""
        text = text.strip()
        if not text:
            return []
        if len(text) <= self.chunk_size:
            return [text]

        chunks: list[str] = []
        start = 0
        while start < len(text):
            end = start + self.chunk_size
            if end >= len(text):
                chunks.append(text[start:].strip())
                break

            # 尝试在句子边界处切割
            boundary = self._find_boundary(text, start + self.chunk_size - self.overlap, end)
            if boundary > start:
                end = boundary

            chunk = text[start:end].strip()
            if chunk:
                chunks.append(chunk)

            # 下一个块从 overlap 前开始
            start = end - self.overlap
            if start <= (end - self.chunk_size):
                start = end  # 防止死循环

        return chunks

    @staticmethod
    def _find_boundary(text: str, search_start: int, search_end: int) -> int:
        """在 [search_start, search_end] 范围内找最后一个句子边界"""
        best = -1
        for sep in ("。", "！", "？", "\n", ". ", "! ", "? ", "；", "; "):
            pos = text.rfind(sep, search_start, search_end)
            if pos > best:
                best = pos + len(sep)
        return best

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
            line.startswith("def ")
            or line.startswith("class ")
            or line.startswith("function ")
            or line.startswith("const ")
            or line.startswith("export ")
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
                if any(item.strip() for item in sections[-1]):
                    sections.append([])
            sections[-1].append(line)
        return ["".join(s) for s in sections]

    @staticmethod
    def _split_by_code_boundaries(text: str) -> list[str]:
        """按顶层 def/class/function/const/export 边界切分"""
        pattern = re.compile(r"^(def |class |function |const |export )", re.MULTILINE)
        positions = [m.start() for m in pattern.finditer(text)]
        if not positions:
            return [text]
        sections = []
        for i, pos in enumerate(positions):
            end = positions[i + 1] if i + 1 < len(positions) else len(text)
            sections.append(text[pos:end])
        # 文件头部（首个定义之前的内容）
        if positions[0] > 0:
            sections.insert(0, text[: positions[0]])
        return sections

    @staticmethod
    def _split_by_paragraphs(text: str, max_size: int) -> list[str]:
        """按空行分段，积累到 max_size 后切节"""
        paragraphs = re.split(r"\n\s*\n", text)
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
