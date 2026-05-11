"""文档分块策略：滑动窗口分块"""


class Chunker:
    """滑动窗口分块器

    按字符数分块（中文1字符≈1token，适用于 bge 模型）。
    优先在句子边界（。！？\\n）处分割。
    """

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
