"""文件解析器：Markdown / 纯文本 / 代码 / PDF / DOCX"""

import logging
from pathlib import Path

logger = logging.getLogger(__name__)

# 代码文件扩展名
CODE_EXTENSIONS = {
    ".py", ".js", ".ts", ".jsx", ".tsx", ".java", ".go", ".rs",
    ".c", ".cpp", ".h", ".hpp", ".cs", ".rb", ".php", ".sh",
    ".yaml", ".yml", ".toml", ".json", ".xml", ".sql",
}


def parse_file(path: str | Path) -> tuple[str, str]:
    """解析文件内容，返回 (title, content)

    支持: .md, .txt, 代码文件, .pdf, .docx
    """
    path = Path(path)
    suffix = path.suffix.lower()

    try:
        if suffix == ".pdf":
            return _parse_pdf(path)
        elif suffix == ".docx":
            return _parse_docx(path)
        elif suffix == ".md":
            return _parse_markdown(path)
        elif suffix in CODE_EXTENSIONS:
            return _parse_code(path)
        else:
            return _parse_text(path)
    except Exception:
        logger.exception("Failed to parse file: %s", path)
        return path.name, ""


def _parse_markdown(path: Path) -> tuple[str, str]:
    """解析 Markdown，提取第一个标题作为 title"""
    content = path.read_text(encoding="utf-8", errors="replace")
    title = path.stem
    for line in content.splitlines():
        line = line.strip()
        if line.startswith("# "):
            title = line[2:].strip()
            break
    return title, content


def _parse_text(path: Path) -> tuple[str, str]:
    """解析纯文本"""
    content = path.read_text(encoding="utf-8", errors="replace")
    title = path.stem
    first_line = content.strip().split("\n", 1)[0][:100] if content.strip() else path.stem
    if first_line:
        title = first_line
    return title, content


def _parse_code(path: Path) -> tuple[str, str]:
    """解析代码文件，保留原始内容"""
    content = path.read_text(encoding="utf-8", errors="replace")
    return f"{path.name}", content


def _parse_pdf(path: Path) -> tuple[str, str]:
    """解析 PDF（使用 PyMuPDF）"""
    import pymupdf

    doc = pymupdf.open(str(path))
    title = doc.metadata.get("title", "") or path.stem
    pages = []
    for page in doc:
        text = page.get_text()
        if text.strip():
            pages.append(text)
    doc.close()
    return title, "\n\n".join(pages)


def _parse_docx(path: Path) -> tuple[str, str]:
    """解析 DOCX"""
    from docx import Document

    doc = Document(str(path))
    title = path.stem
    # 尝试从核心属性获取标题
    if doc.core_properties.title:
        title = doc.core_properties.title
    paragraphs = [p.text for p in doc.paragraphs if p.text.strip()]
    return title, "\n\n".join(paragraphs)
