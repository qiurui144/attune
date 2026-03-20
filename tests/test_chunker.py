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
    # 3000+ 字内容，每1500一段
    text = ("这是一个段落。" * 100 + "\n\n") * 3
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
