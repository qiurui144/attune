#!/usr/bin/env python3
"""Generate deterministic PDF test fixtures for attune-core PDF ingest tests.

Produces 4 small PDFs under crates/attune-core/tests/fixtures/pdf/:
  - text-en.pdf      English text layer (known sentence)
  - text-zh.pdf      Chinese text layer (known sentence)
  - mixed-zhen.pdf   Mixed CN+EN text layer
  - scanned.pdf      IMAGE-only PDF (NO text layer) → triggers needs_ocr

Both this generator AND its generated PDFs are committed so fixtures are
reproducible + version-controlled (per docs/TESTING.md §2.1 corpus method).

WHY CHROME (not PyMuPDF/reportlab) for the text PDFs:
  The attune ingest path uses the `pdf-extract` Rust crate. PyMuPDF's built-in
  CJK fonts emit `UniGB-UTF16-H` (pdf-extract PANICS on it), and even an
  embedded-subset Identity-H font from PyMuPDF produces a ToUnicode CMap that
  crashes pdf-extract's `adobe-cmap-parser`. Chrome's headless print-to-PDF
  emits a standard Identity-H + well-formed ToUnicode CMap that pdf-extract
  parses cleanly — and is also what real users get from "Save as PDF". Chrome
  headless output is byte-deterministic (no embedded timestamp/ID), so the
  text fixtures regenerate identically.

The image-only `scanned.pdf` rasterizes text → PNG with PyMuPDF (fitz, a
deterministic rasterizer) then embeds it as an <img> printed via Chrome, so it
has NO text layer → pdf-extract returns ~0 chars and `needs_ocr` routes true.

Determinism: Chrome embeds wall-clock /CreationDate, /ModDate and a random
/Title (temp filename); pikepdf pins those + derives a content-stable /ID, so
all four fixtures regenerate byte-identically (verify with `git diff --stat`).

Requirements: google-chrome / chromium (all PDFs), PyMuPDF `fitz` (rasterize
scanned image), pikepdf (byte-stable normalize). The committed PDFs are the
source of truth for the Rust tests; this script documents how they were made.

Usage:
  python3 scripts/gen-pdf-fixtures.py            # write fixtures
  python3 scripts/gen-pdf-fixtures.py --check     # verify text extraction
"""
import shutil
import subprocess
import sys
import pathlib

REPO = pathlib.Path(__file__).resolve().parent.parent
OUT = REPO / "rust" / "crates" / "attune-core" / "tests" / "fixtures" / "pdf"

# ── Known content asserted by crates/attune-core/tests/pdf_ingest_test.rs ──
#
# text-layer PDFs must have > 100 non-whitespace chars so the parser does NOT
# route them to OCR (needs_ocr threshold is < 100 chars). The English fixture
# is therefore three lines.
TEXT_EN_LINES = [
    "The quick brown fox jumps over the lazy dog.",
    "Rust ownership and borrowing prevent data races at compile time.",
    "Full text search and vector retrieval are combined for hybrid recall.",
]
TEXT_ZH_LINES = [
    "向量检索与全文搜索的混合融合是现代检索系统的核心能力。",
    "借条与银行流水是民间借贷纠纷中常见的关键证据材料之一。",
    "中文分词需要结巴分词器才能正确切分多字词语保证召回率。",
]
TEXT_MIXED_LINES = [
    "项目 Running 测试 with embedding",
    "向量 search 检索 hybrid recall。",
]

CHROME_CANDIDATES = ["google-chrome", "google-chrome-stable", "chromium-browser", "chromium"]

# CJK font stack tried in HTML; resolved at render time by Chrome via fontconfig.
CJK_FONT_STACK = '"AR PL UMing CN", "Noto Serif CJK SC", "Noto Sans CJK SC", serif'
LATIN_FONT_STACK = '"DejaVu Sans", "Liberation Sans", sans-serif'


def _chrome() -> str:
    for c in CHROME_CANDIDATES:
        p = shutil.which(c)
        if p:
            return p
    raise RuntimeError("no chrome/chromium found; install google-chrome or chromium")


def _html(lines, font_stack, w, h) -> str:
    body = "<br>\n".join(lines)
    return (
        '<!doctype html><html><head><meta charset="utf-8">'
        f"<style>@page{{size:{w}px {h}px;margin:20px}} "
        f"body{{font-family:{font_stack};font-size:14px;line-height:1.7}}</style>"
        f"</head><body>\n{body}\n</body></html>"
    )


def _chrome_pdf(html: str, out: pathlib.Path) -> None:
    """Render HTML → PDF via headless Chrome (deterministic, no header/footer)."""
    import tempfile
    chrome = _chrome()
    with tempfile.NamedTemporaryFile("w", suffix=".html", delete=False, encoding="utf-8") as f:
        f.write(html)
        html_path = f.name
    try:
        subprocess.run(
            [
                chrome, "--headless", "--disable-gpu", "--no-sandbox",
                "--no-pdf-header-footer",
                f"--print-to-pdf={out}", html_path,
            ],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=90,
        )
    finally:
        pathlib.Path(html_path).unlink(missing_ok=True)
    if not out.exists() or out.stat().st_size == 0:
        raise RuntimeError(f"Chrome failed to produce {out}")


def make_scanned_pdf(path: pathlib.Path) -> None:
    """Image-only PDF: render text to a raster PNG (fitz), embed as an <img> in
    an HTML page and print to PDF via Chrome → NO text layer.

    pdf_extract on this yields ~0 extractable chars → needs_ocr() == true,
    exercising the OCR routing decision deterministically. fitz is used only to
    rasterize text → PNG (deterministic); Chrome owns the PDF container so the
    whole pipeline stays byte-stable and on a single toolchain.
    """
    import base64
    import fitz  # PyMuPDF — deterministic rasterizer

    cjk = _cjk_font_file()
    src = fitz.open()
    spage = src.new_page(width=480, height=200)
    spage.insert_text((30, 60), "Scanned receipt image only no text layer",
                      fontsize=16, fontname="helv")
    spage.insert_text((30, 110), "扫描件无文字层 金额 1234.56",
                      fontsize=16, fontname="cjk", fontfile=cjk)
    png_bytes = spage.get_pixmap(dpi=150).tobytes("png")
    src.close()

    b64 = base64.b64encode(png_bytes).decode()
    html = (
        '<!doctype html><html><head><meta charset="utf-8">'
        "<style>@page{size:480px 200px;margin:0} body{margin:0} "
        "img{width:480px;height:200px}</style></head><body>"
        f'<img src="data:image/png;base64,{b64}"></body></html>'
    )
    _chrome_pdf(html, path)
    _normalize(path)


def _cjk_font_file() -> str:
    for cand in [
        "/usr/share/fonts/truetype/arphic/uming.ttc",
        "/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    ]:
        if pathlib.Path(cand).exists():
            return cand
    out = subprocess.run(["fc-list", ":lang=zh", "file"], capture_output=True, text=True).stdout
    for line in out.splitlines():
        f = line.split(":")[0].strip()
        if f and pathlib.Path(f).exists():
            return f
    raise RuntimeError("no CJK font found; install fonts-arphic-uming or fonts-noto-cjk")


def _normalize(path: pathlib.Path) -> None:
    """Make the PDF byte-stable across regenerations (no-op without pikepdf).

    Chrome/PyMuPDF embed wall-clock /CreationDate + /ModDate, a random temp
    filename in /Title, and a random trailer /ID. Pin all of them to constants
    so `git diff` is clean when the fixtures are regenerated.
    """
    try:
        import pikepdf
    except ImportError:
        return
    pdf = pikepdf.open(str(path), allow_overwriting_input=True)
    info = pdf.docinfo
    info["/CreationDate"] = pikepdf.String("D:20260101000000Z")
    info["/ModDate"] = pikepdf.String("D:20260101000000Z")
    info["/Title"] = pikepdf.String("attune-pdf-fixture")
    info["/Producer"] = pikepdf.String("attune-fixture-gen")
    info["/Creator"] = pikepdf.String("gen-pdf-fixtures.py")
    # Drop XMP metadata stream if present (carries its own timestamps).
    if "/Metadata" in pdf.Root:
        del pdf.Root["/Metadata"]
    # deterministic_id derives /ID from a content hash; once the timestamps above
    # are pinned the content is stable, so the derived /ID is stable too.
    pdf.save(str(path), fix_metadata_version=False, deterministic_id=True)
    pdf.close()


def generate() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    print(f"  (chrome: {_chrome()})")
    _chrome_pdf(_html(TEXT_EN_LINES, LATIN_FONT_STACK, 480, 320), OUT / "text-en.pdf")
    _chrome_pdf(_html(TEXT_ZH_LINES, CJK_FONT_STACK, 480, 320), OUT / "text-zh.pdf")
    _chrome_pdf(_html(TEXT_MIXED_LINES, CJK_FONT_STACK, 520, 200), OUT / "mixed-zhen.pdf")
    for name in ("text-en.pdf", "text-zh.pdf", "mixed-zhen.pdf"):
        _normalize(OUT / name)
    make_scanned_pdf(OUT / "scanned.pdf")
    for p in sorted(OUT.glob("*.pdf")):
        print(f"  {p.relative_to(REPO)}  ({p.stat().st_size} bytes)")


def check() -> int:
    """Sanity-check extraction with pdftotext (smoke check, not the Rust path)."""
    ok = True
    expectations = {
        "text-en.pdf": ["quick brown fox", "Rust", "ownership"],
        "text-zh.pdf": ["向量", "借条", "分词"],
        "mixed-zhen.pdf": ["Running", "向量"],
    }
    for name, tokens in expectations.items():
        out = subprocess.run(
            ["pdftotext", str(OUT / name), "-"], capture_output=True, text=True
        ).stdout
        for tok in tokens:
            present = tok in out
            ok = ok and present
            print(f"  {name}: '{tok}' -> {'OK' if present else 'MISSING'}")
    scanned_out = subprocess.run(
        ["pdftotext", str(OUT / "scanned.pdf"), "-"], capture_output=True, text=True
    ).stdout
    nws = sum(1 for c in scanned_out if not c.isspace())
    print(f"  scanned.pdf: non-whitespace text-layer chars = {nws} (want < 100 for OCR routing)")
    ok = ok and nws < 100
    return 0 if ok else 1


if __name__ == "__main__":
    if "--check" in sys.argv:
        sys.exit(check())
    generate()
    print("Generated PDF fixtures in", OUT)
