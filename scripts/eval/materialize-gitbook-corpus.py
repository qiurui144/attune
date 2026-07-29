#!/usr/bin/env python3
"""Materialize a GitBook-style documentation space into text files.

This is intentionally dependency-free so K3/nightly hosts can run it without a
browser or Node toolchain. It snapshots public HTML pages under one GitBook
space, extracts readable text, and writes one Markdown-ish file per page plus a
manifest. It is for external-fetch-only eval assets; generated output must not
be committed unless the upstream license explicitly allows redistribution.
"""
from __future__ import annotations

import argparse
import hashlib
import html
import json
import re
import time
from collections import deque
from html.parser import HTMLParser
from pathlib import Path
from typing import Any
from urllib.parse import urldefrag, urljoin, urlparse
from urllib.request import Request, urlopen


class TextAndLinksParser(HTMLParser):
    def __init__(self, base_url: str) -> None:
        super().__init__(convert_charrefs=True)
        self.base_url = base_url
        self.title_parts: list[str] = []
        self.text_parts: list[str] = []
        self.links: list[str] = []
        self._skip_depth = 0
        self._in_title = False

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        tag_l = tag.lower()
        if tag_l in {"script", "style", "noscript", "svg"}:
            self._skip_depth += 1
            return
        if tag_l == "title":
            self._in_title = True
        if tag_l == "a":
            href = dict(attrs).get("href")
            if href:
                self.links.append(urljoin(self.base_url, href))
        if tag_l in {"h1", "h2", "h3", "p", "li", "br", "tr", "section", "article"}:
            self.text_parts.append("\n")

    def handle_endtag(self, tag: str) -> None:
        tag_l = tag.lower()
        if tag_l in {"script", "style", "noscript", "svg"} and self._skip_depth:
            self._skip_depth -= 1
            return
        if tag_l == "title":
            self._in_title = False
        if tag_l in {"h1", "h2", "h3", "p", "li", "tr", "section", "article"}:
            self.text_parts.append("\n")

    def handle_data(self, data: str) -> None:
        if self._skip_depth:
            return
        text = html.unescape(data).strip()
        if not text:
            return
        if self._in_title:
            self.title_parts.append(text)
        self.text_parts.append(text)
        self.text_parts.append(" ")

    @property
    def title(self) -> str:
        title = " ".join(self.title_parts)
        return compact_text(title)[:160] or "untitled"

    @property
    def text(self) -> str:
        return compact_lines("".join(self.text_parts))


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root-url", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--max-pages", type=int, default=500)
    parser.add_argument("--timeout", type=int, default=20)
    parser.add_argument("--sleep-ms", type=int, default=100)
    parser.add_argument("--min-chars", type=int, default=200)
    return parser.parse_args()


def compact_text(text: str) -> str:
    return re.sub(r"\s+", " ", text).strip()


def compact_lines(text: str) -> str:
    lines = [compact_text(line) for line in text.splitlines()]
    return "\n".join(line for line in lines if line)


def same_space(root: str, candidate: str) -> bool:
    root_p = urlparse(root)
    cand_p = urlparse(candidate)
    if cand_p.scheme not in {"http", "https"}:
        return False
    if cand_p.netloc != root_p.netloc:
        return False
    root_path = root_p.path.rstrip("/")
    return cand_p.path.rstrip("/").startswith(root_path)


def canonicalize(url: str) -> str:
    url, _ = urldefrag(url)
    parsed = urlparse(url)
    normalized_path = re.sub(r"/+", "/", parsed.path).rstrip("/")
    return parsed._replace(path=normalized_path or "/", query="").geturl()


def fetch(url: str, timeout: int) -> tuple[str, bytes]:
    req = Request(
        url,
        headers={
            "user-agent": "Attune-RAG-Eval-Materializer/1.0 (+external-fetch-only)",
            "accept": "text/html,application/xhtml+xml",
        },
    )
    with urlopen(req, timeout=timeout) as resp:
        content_type = resp.headers.get("content-type", "")
        body = resp.read()
    return content_type, body


def safe_name(idx: int, url: str, title: str) -> str:
    slug_src = title if title and title != "untitled" else urlparse(url).path
    slug = re.sub(r"[^A-Za-z0-9._-]+", "-", slug_src).strip("-").lower()[:80]
    digest = hashlib.sha256(url.encode("utf-8")).hexdigest()[:10]
    return f"{idx:04d}-{slug or 'page'}-{digest}.md"


def main() -> int:
    args = parse_args()
    root = canonicalize(args.root_url)
    out_dir = args.out
    pages_dir = out_dir / "pages"
    pages_dir.mkdir(parents=True, exist_ok=True)

    queue: deque[str] = deque([root])
    seen: set[str] = set()
    written: list[dict[str, Any]] = []
    failures: list[dict[str, Any]] = []

    while queue and len(seen) < args.max_pages:
        url = canonicalize(queue.popleft())
        if url in seen or not same_space(root, url):
            continue
        seen.add(url)
        try:
            content_type, body = fetch(url, args.timeout)
            if "html" not in content_type.lower():
                continue
            parser = TextAndLinksParser(url)
            parser.feed(body.decode("utf-8", errors="replace"))
            text = parser.text
            if len(text) >= args.min_chars:
                filename = safe_name(len(written) + 1, url, parser.title)
                rel_path = Path("pages") / filename
                page_hash = hashlib.sha256(text.encode("utf-8")).hexdigest()
                (out_dir / rel_path).write_text(
                    f"# {parser.title}\n\nSource: {url}\nFetched: {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}\nHash: {page_hash}\n\n{text}\n",
                    encoding="utf-8",
                )
                written.append(
                    {
                        "url": url,
                        "title": parser.title,
                        "path": str(rel_path),
                        "chars": len(text),
                        "sha256": page_hash,
                    }
                )
            for link in parser.links:
                candidate = canonicalize(link)
                if candidate not in seen and same_space(root, candidate):
                    queue.append(candidate)
            if args.sleep_ms > 0:
                time.sleep(args.sleep_ms / 1000)
        except Exception as exc:  # noqa: BLE001 - materializer records and continues.
            failures.append({"url": url, "error": str(exc)[:500]})

    manifest = {
        "schema_version": "attune.eval.materialized_gitbook.v1",
        "root_url": root,
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "page_count": len(written),
        "visited_count": len(seen),
        "max_pages": args.max_pages,
        "pages": written,
        "failures": failures,
    }
    (out_dir / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(json.dumps({k: manifest[k] for k in ("root_url", "page_count", "visited_count", "max_pages")}, ensure_ascii=False))
    return 0 if written else 1


if __name__ == "__main__":
    raise SystemExit(main())
