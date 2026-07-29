#!/usr/bin/env python3
"""Deterministic OCR-image fixture generator for OCR routing/quality tests.

Produces committed image fixtures + corrupt fixtures used by
`tests/ocr_image_test.rs`. These are RASTER images with NO text layer, so the
only way to recover the text is real OCR (PP-OCRv5) — that is exactly the path
under test.

  known_text.png   — white canvas, black KNOWN_TEXT rendered with a bitmap font.
                     Routing + real-OCR-quality fixture.
  known_text.jpg   — same content as JPEG (quality=95) — exercises the `.jpg`
                     arm of the extension match and a lossy codec.
  zero_byte.png    — 0-byte file with .png extension. Graceful-error fixture.
  not_an_image.png — ASCII text body with .png extension (header is "this is not
                     a PNG"). Graceful-error fixture: parser must route to the
                     OCR branch and return Err, not treat the bytes as text.

The KNOWN_TEXT (must match KNOWN_TEXT in ocr_image_test.rs). Kept to plain ASCII
digits + uppercase so PP-OCRv5 mobile scores it reliably and CER is meaningful:
  "ATTUNE OCR TEST 2026"

Determinism: PIL default bitmap font (load_default) renders identically across
runs; no RNG, no timestamps embedded (PNG written without text chunks). Verify
via sha256 in --check mode.

Usage:
  python3 gen_ocr_image_fixtures.py          # (re)generate all fixtures
  python3 gen_ocr_image_fixtures.py --check  # print sha256 of each fixture
"""
import hashlib
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
KNOWN_TEXT = "ATTUNE OCR TEST 2026"


def gen_text_image(path, text, fmt):
    from PIL import Image, ImageDraw, ImageFont

    # Large canvas + scaled-up default font so the mobile detector finds a clean
    # text region. PP-OCRv5 mobile needs reasonably sized glyphs.
    W, H = 640, 200
    img = Image.new("RGB", (W, H), color=(255, 255, 255))
    draw = ImageDraw.Draw(img)
    try:
        font = ImageFont.load_default(size=48)  # Pillow >= 10
    except TypeError:
        font = ImageFont.load_default()
    # Center the text roughly.
    bbox = draw.textbbox((0, 0), text, font=font)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    draw.text(((W - tw) / 2, (H - th) / 2 - bbox[1]), text, fill=(0, 0, 0), font=font)
    if fmt == "PNG":
        img.save(path, format="PNG", optimize=True)
    elif fmt == "JPEG":
        img.save(path, format="JPEG", quality=95)
    else:
        raise ValueError(fmt)


def gen_zero_byte(path):
    open(path, "wb").close()


def gen_not_an_image(path):
    with open(path, "wb") as f:
        f.write(b"this is not a PNG, it is plain ASCII text masquerading as one.\n")


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    check = "--check" in sys.argv
    names = ["known_text.png", "known_text.jpg", "zero_byte.png", "not_an_image.png"]
    if not check:
        gen_text_image(os.path.join(HERE, "known_text.png"), KNOWN_TEXT, "PNG")
        gen_text_image(os.path.join(HERE, "known_text.jpg"), KNOWN_TEXT, "JPEG")
        gen_zero_byte(os.path.join(HERE, "zero_byte.png"))
        gen_not_an_image(os.path.join(HERE, "not_an_image.png"))
        for n in names:
            p = os.path.join(HERE, n)
            print(f"wrote {n} ({os.path.getsize(p)} bytes)")

    for n in names:
        p = os.path.join(HERE, n)
        if os.path.exists(p):
            print(f"sha256 {n}: {sha256(p)}")


if __name__ == "__main__":
    main()
