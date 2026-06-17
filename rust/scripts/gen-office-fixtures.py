#!/usr/bin/env python3
"""Deterministic Office / ZIP-container fixture generator.

Produces the committed fixtures consumed by:
  - tests/office_formats_test.rs     (extraction: known CN+EN content per format)
  - tests/office_adversarial_test.rs (P0 security: zip-bomb / path-traversal /
                                       billion-laughs XXE / oversized single entry)

All fixtures are byte-deterministic: stdlib `zipfile` with a fixed timestamp
(2020-01-01 00:00:00) and fixed compression, no RNG. Re-running yields identical
bytes — verify with `python3 gen-office-fixtures.py --check` (prints sha256).

Why hand-rolled containers (no python-docx / openpyxl):
  The attune parser only reads the minimal entries it cares about — for docx
  `word/document.xml`, for pptx `ppt/slides/slideN.xml`, for xlsx the calamine
  workbook, for epub any `*.xhtml/*.html`. We craft exactly those so the fixtures
  stay tiny, dependency-free, and reproducible on a clean checkout / CI.

OUTPUT DIR: ../crates/attune-core/tests/fixtures/office/

── Benign extraction fixtures (KNOWN content, ≥1 mixed CN+EN) ──────────────────
  known.docx   word/document.xml with "Hello World 你好世界 ATTUNE_DOCX_MARKER"
  known.xlsx   a real (calamine-readable) minimal workbook with cells
               "Name 姓名 / Alice 爱丽丝 / ATTUNE_XLSX_MARKER 标记"
  known.pptx   two slides, "Slide One 第一页 ATTUNE_PPTX_S1" / "...ATTUNE_PPTX_S2"
  known.epub   one xhtml chapter "EPUB Chapter 章节 ATTUNE_EPUB_MARKER"
  known.rtf    "Hello RTF 世界 ATTUNE_RTF_MARKER" (plain RTF, not a zip)
  known.csv    "name,城市\nAlice,北京\nATTUNE_CSV_MARKER,上海"

── Adversarial fixtures (P0 security) ─────────────────────────────────────────
  bomb.docx    a tiny docx whose word/document.xml decompresses to ~100 MB of
               'A' (high-ratio deflate). Bounded-decompression probe.
  bomb.xlsx    a tiny xlsx whose sharedStrings.xml decompresses to ~100 MB.
  traversal.docx  a docx that ALSO contains an entry literally named
               "../../../../tmp/attune_evil_marker" — the parser must never
               write it to disk (it extracts to memory). FS-escape probe.
  laughs.docx  word/document.xml carrying a billion-laughs DTD entity bomb +
               an external-entity (XXE) reference to file:///etc/passwd. The
               parser must NOT expand entities / fetch the external resource.
  big_entry.docx  word/document.xml is a single ~50 MB entry (no compression
               trick — genuinely large) to probe an output-size bound.

Usage:
  python3 gen-office-fixtures.py          # (re)generate all fixtures
  python3 gen-office-fixtures.py --check  # print sha256 of each fixture
"""
import hashlib
import os
import sys
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.normpath(os.path.join(HERE, "..", "crates", "attune-core", "tests", "fixtures", "office"))

# Fixed timestamp so zip headers are byte-stable across runs.
FIXED_DATE = (2020, 1, 1, 0, 0, 0)

# Decompressed target for the zip-bomb entries (~100 MB). Big enough that an
# unbounded read_to_string would balloon memory, small enough to generate fast.
BOMB_DECOMPRESSED = 100 * 1024 * 1024
# A genuinely-large (already-decompressed-ish) single entry (~50 MB).
BIG_ENTRY_SIZE = 50 * 1024 * 1024


def _zinfo(name, compress=zipfile.ZIP_DEFLATED):
    zi = zipfile.ZipInfo(name, date_time=FIXED_DATE)
    zi.compress_type = compress
    zi.external_attr = 0o600 << 16
    return zi


def write_zip(path, entries):
    """entries: list of (name, bytes, compress_type)."""
    with zipfile.ZipFile(path, "w") as zf:
        for name, data, comp in entries:
            zf.writestr(_zinfo(name, comp), data)


# ── benign content ─────────────────────────────────────────────────────────────

CONTENT_TYPES = (
    b'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    b'<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
    b'<Default Extension="xml" ContentType="application/xml"/>'
    b'</Types>'
)


def docx_document_xml(text):
    return (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
        '<w:body><w:p><w:r><w:t>' + text + '</w:t></w:r></w:p></w:body></w:document>'
    ).encode("utf-8")


def slide_xml(text):
    return (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" '
        'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">'
        '<p:cSld><p:spTree><p:sp><p:txBody>'
        '<a:p><a:r><a:t>' + text + '</a:t></a:r></a:p>'
        '</p:txBody></p:sp></p:spTree></p:cSld></p:sld>'
    ).encode("utf-8")


def epub_xhtml(text):
    return (
        '<?xml version="1.0" encoding="UTF-8"?>'
        '<html xmlns="http://www.w3.org/1999/xhtml"><head><title>'
        + text + '</title></head><body><p>' + text + '</p></body></html>'
    ).encode("utf-8")


def make_known_docx():
    txt = "Hello World 你好世界 ATTUNE_DOCX_MARKER"
    write_zip(os.path.join(OUT, "known.docx"), [
        ("[Content_Types].xml", CONTENT_TYPES, zipfile.ZIP_DEFLATED),
        ("word/document.xml", docx_document_xml(txt), zipfile.ZIP_DEFLATED),
    ])


def make_known_pptx():
    write_zip(os.path.join(OUT, "known.pptx"), [
        ("[Content_Types].xml", CONTENT_TYPES, zipfile.ZIP_DEFLATED),
        ("ppt/slides/slide1.xml", slide_xml("Slide One 第一页 ATTUNE_PPTX_S1"), zipfile.ZIP_DEFLATED),
        ("ppt/slides/slide2.xml", slide_xml("Slide Two 第二页 ATTUNE_PPTX_S2"), zipfile.ZIP_DEFLATED),
    ])


def make_known_epub():
    write_zip(os.path.join(OUT, "known.epub"), [
        ("mimetype", b"application/epub+zip", zipfile.ZIP_STORED),
        ("META-INF/container.xml", b"<container/>", zipfile.ZIP_DEFLATED),
        ("OEBPS/chapter1.xhtml", epub_xhtml("EPUB Chapter 章节 ATTUNE_EPUB_MARKER"), zipfile.ZIP_DEFLATED),
    ])


def make_known_xlsx():
    # calamine needs a real OOXML workbook. Build the minimal set of parts it
    # reads: content types, workbook rels, workbook, one inline-string sheet.
    ct = (
        b'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        b'<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        b'<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        b'<Default Extension="xml" ContentType="application/xml"/>'
        b'<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
        b'<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
        b'</Types>'
    )
    root_rels = (
        b'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        b'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        b'<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>'
        b'</Relationships>'
    )
    workbook = (
        b'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        b'<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
        b'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
        b'<sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>'
    )
    wb_rels = (
        b'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        b'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        b'<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>'
        b'</Relationships>'
    )
    # Inline strings (t="inlineStr") avoid a sharedStrings part.
    def cell(ref, text):
        return ('<c r="' + ref + '" t="inlineStr"><is><t>' + text + '</t></is></c>')
    sheet = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
        '<sheetData>'
        '<row r="1">' + cell("A1", "Name 姓名") + cell("B1", "City 城市") + '</row>'
        '<row r="2">' + cell("A2", "Alice 爱丽丝") + cell("B2", "Beijing 北京") + '</row>'
        '<row r="3">' + cell("A3", "ATTUNE_XLSX_MARKER 标记") + cell("B3", "Shanghai 上海") + '</row>'
        '</sheetData></worksheet>'
    ).encode("utf-8")
    write_zip(os.path.join(OUT, "known.xlsx"), [
        ("[Content_Types].xml", ct, zipfile.ZIP_DEFLATED),
        ("_rels/.rels", root_rels, zipfile.ZIP_DEFLATED),
        ("xl/workbook.xml", workbook, zipfile.ZIP_DEFLATED),
        ("xl/_rels/workbook.xml.rels", wb_rels, zipfile.ZIP_DEFLATED),
        ("xl/worksheets/sheet1.xml", sheet, zipfile.ZIP_DEFLATED),
    ])


def make_known_rtf():
    # Plain ASCII RTF — the marker is the token the test asserts. The \'e9 hex
    # escape encodes 'é' (Latin-1 0xE9) to exercise the parser's hex-escape path.
    # CN handling is covered by docx/xlsx/pptx/csv (RTF's ansicpg path is lossy
    # for CJK and not a product target).
    rtf = (r"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0 Arial;}}"
           r"\f0\pard Hello RTF caf\'e9 World ATTUNE_RTF_MARKER\par}")
    with open(os.path.join(OUT, "known.rtf"), "wb") as f:
        f.write(rtf.encode("ascii"))


def make_known_csv():
    csv = "name,城市\nAlice,北京\nATTUNE_CSV_MARKER,上海\n"
    with open(os.path.join(OUT, "known.csv"), "wb") as f:
        f.write(csv.encode("utf-8"))


# ── adversarial content ────────────────────────────────────────────────────────

def make_bomb_docx():
    # word/document.xml: minimal wrapper + a giant run of 'A' that deflates to a
    # few KB but inflates to ~100 MB.
    payload = (
        '<?xml version="1.0" encoding="UTF-8"?>'
        '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
        '<w:body><w:p><w:r><w:t>' + ("A" * BOMB_DECOMPRESSED) + '</w:t></w:r></w:p></w:body></w:document>'
    ).encode("ascii")
    write_zip(os.path.join(OUT, "bomb.docx"), [
        ("[Content_Types].xml", CONTENT_TYPES, zipfile.ZIP_DEFLATED),
        ("word/document.xml", payload, zipfile.ZIP_DEFLATED),
    ])


def make_bomb_xlsx():
    # A standalone (calamine-readable) workbook would require valid structure;
    # for the bomb we only need the inflate ratio to be observable when the
    # parser opens the archive. We embed a huge inline-string sheet so calamine
    # would have to materialize ~100 MB if it has no bound.
    ct = (
        b'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        b'<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        b'<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        b'<Default Extension="xml" ContentType="application/xml"/>'
        b'<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
        b'<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
        b'</Types>'
    )
    root_rels = (
        b'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        b'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        b'<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>'
        b'</Relationships>'
    )
    workbook = (
        b'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        b'<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
        b'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
        b'<sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>'
    )
    wb_rels = (
        b'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        b'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        b'<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>'
        b'</Relationships>'
    )
    sheet = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
        '<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>'
        + ("A" * BOMB_DECOMPRESSED) +
        '</t></is></c></row></sheetData></worksheet>'
    ).encode("ascii")
    write_zip(os.path.join(OUT, "bomb.xlsx"), [
        ("[Content_Types].xml", ct, zipfile.ZIP_DEFLATED),
        ("_rels/.rels", root_rels, zipfile.ZIP_DEFLATED),
        ("xl/workbook.xml", workbook, zipfile.ZIP_DEFLATED),
        ("xl/_rels/workbook.xml.rels", wb_rels, zipfile.ZIP_DEFLATED),
        ("xl/worksheets/sheet1.xml", sheet, zipfile.ZIP_DEFLATED),
    ])


def make_traversal_docx():
    # A valid-enough docx PLUS a malicious entry name. The parser reads
    # word/document.xml into memory; it must never honor the traversal name by
    # writing to /tmp.
    write_zip(os.path.join(OUT, "traversal.docx"), [
        ("[Content_Types].xml", CONTENT_TYPES, zipfile.ZIP_DEFLATED),
        ("word/document.xml", docx_document_xml("Benign body 正文 ATTUNE_TRAVERSAL_BODY"), zipfile.ZIP_DEFLATED),
        ("../../../../tmp/attune_evil_marker", b"PWNED_TRAVERSAL_PAYLOAD", zipfile.ZIP_DEFLATED),
        ("..\\..\\..\\..\\tmp\\attune_evil_win", b"PWNED_TRAVERSAL_WIN", zipfile.ZIP_DEFLATED),
    ])


def make_laughs_docx():
    # billion-laughs + external-entity (XXE) in word/document.xml. If the parser
    # expanded entities, &lol9; would explode to ~3 GB; if it fetched the
    # SYSTEM entity it would read /etc/passwd. attune's strip_xml_tags is a
    # char scanner (no entity expansion), so this must be inert — the test
    # asserts no blowup, no /etc/passwd content, bounded time.
    doc = (
        '<?xml version="1.0"?>'
        '<!DOCTYPE w:document ['
        '<!ENTITY xxe SYSTEM "file:///etc/passwd">'
        '<!ENTITY lol "lol">'
        '<!ENTITY lol1 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">'
        '<!ENTITY lol2 "&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;">'
        '<!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;">'
        '<!ENTITY lol4 "&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;">'
        '<!ENTITY lol5 "&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;">'
        '<!ENTITY lol6 "&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;">'
        '<!ENTITY lol7 "&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;">'
        '<!ENTITY lol8 "&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;">'
        '<!ENTITY lol9 "&lol8;&lol8;&lol8;&lol8;&lol8;&lol8;&lol8;&lol8;&lol8;&lol8;">'
        ']>'
        '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
        '<w:body><w:p><w:r><w:t>safe text 安全 ATTUNE_LAUGHS_BODY &lol9; &xxe;</w:t></w:r></w:p></w:body>'
        '</w:document>'
    ).encode("utf-8")
    write_zip(os.path.join(OUT, "laughs.docx"), [
        ("[Content_Types].xml", CONTENT_TYPES, zipfile.ZIP_DEFLATED),
        ("word/document.xml", doc, zipfile.ZIP_DEFLATED),
    ])


def make_big_entry_docx():
    # A genuinely large single entry (~50 MB, not a compression trick) to probe
    # an output-size bound distinct from the high-ratio bomb.
    payload = (
        '<?xml version="1.0" encoding="UTF-8"?>'
        '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
        '<w:body><w:p><w:r><w:t>' + ("B" * BIG_ENTRY_SIZE) + '</w:t></w:r></w:p></w:body></w:document>'
    ).encode("ascii")
    write_zip(os.path.join(OUT, "big_entry.docx"), [
        ("[Content_Types].xml", CONTENT_TYPES, zipfile.ZIP_DEFLATED),
        ("word/document.xml", payload, zipfile.ZIP_DEFLATED),
    ])


FIXTURES = [
    "known.docx", "known.xlsx", "known.pptx", "known.epub", "known.rtf", "known.csv",
    "bomb.docx", "bomb.xlsx", "traversal.docx", "laughs.docx", "big_entry.docx",
]


def generate_all():
    os.makedirs(OUT, exist_ok=True)
    make_known_docx()
    make_known_xlsx()
    make_known_pptx()
    make_known_epub()
    make_known_rtf()
    make_known_csv()
    make_bomb_docx()
    make_bomb_xlsx()
    make_traversal_docx()
    make_laughs_docx()
    make_big_entry_docx()


def check():
    for name in FIXTURES:
        p = os.path.join(OUT, name)
        if not os.path.exists(p):
            print(f"{name}: MISSING")
            continue
        h = hashlib.sha256(open(p, "rb").read()).hexdigest()
        print(f"{name}: {os.path.getsize(p):>12} bytes  sha256={h}")


if __name__ == "__main__":
    if "--check" in sys.argv:
        check()
    else:
        generate_all()
        print(f"wrote {len(FIXTURES)} fixtures to {OUT}")
        check()
