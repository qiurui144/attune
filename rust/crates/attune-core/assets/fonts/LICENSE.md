# Embedded font: AttuneCJK-subset.ttf

This is a subset of **WenQuanYi Micro Hei** (文泉驿微米黑) covering the common
CJK Unified Ideographs block (U+4E00–U+9FFF) plus ASCII, Latin-1, and CJK/fullwidth
punctuation, generated with `pyftsubset`.

WenQuanYi Micro Hei is dual-licensed under **Apache License 2.0** and GPLv3 with
font-embedding exception. Attune uses it under the **Apache-2.0** terms, which are
compatible with this repository's license.

Upstream: http://wenq.org/
Subset command:
  pyftsubset wqy-microhei.ttc --text-file=<cjk_charset> \
    --output-file=AttuneCJK-subset.ttf --layout-features='*' \
    --no-hinting --desubroutinize --font-number=0

Purpose: embedded via `include_bytes!` into the PDF export renderer (typst) so that
Chinese text renders correctly on any host without relying on system fonts
(spec R1 — CJK glyph correctness is the top export risk).
