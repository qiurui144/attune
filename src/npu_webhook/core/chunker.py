"""文档分块策略"""


class Chunker:
    """滑动窗口分块（512 tokens, 128 overlap）"""

    def __init__(self, chunk_size: int = 512, overlap: int = 128) -> None:
        self.chunk_size = chunk_size
        self.overlap = overlap
        # TODO Phase 1
