"""parser.parse_bytes() 单元测试"""


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
