#!/usr/bin/env python3
"""
Parse lawcontrol PostgreSQL dump → split each crawled_data row into a single
markdown file under tmp/lawcontrol-corpus/.

PG dump 格式：每行 1 条 INSERT INTO public.crawled_data (...) VALUES (...);
字段 13 个，按顺序：
  (id, source, source_id, data_type, title, content, metadata,
   is_synced_to_kb, dify_document_id, crawled_at, synced_at, raw_html, url)

Output：
  tmp/lawcontrol-corpus/regulation/<safe_title>.md  (8K+ 法规)
  tmp/lawcontrol-corpus/case/<safe_title>.md         (2.5K+ 案例)

Usage:
    python3 scripts/parse-legal-dump.py /tmp/lawcontrol_seed.sql tmp/lawcontrol-corpus
"""

import os
import re
import sys
from pathlib import Path


def parse_values(text: str) -> list:
    """Stream parse PG dump VALUES tuple. Handles '' escape, NULL, numbers, strings."""
    pos = 0
    values = []
    n = len(text)
    while pos < n:
        # skip leading whitespace and commas
        while pos < n and text[pos] in " \t,":
            pos += 1
        if pos >= n or text[pos] == ")":
            break

        # NULL
        if text[pos:pos + 4].upper() == "NULL":
            values.append(None)
            pos += 4
            continue

        # quoted string
        if text[pos] == "'":
            pos += 1
            buf_parts = []
            while pos < n:
                if text[pos] == "'":
                    if pos + 1 < n and text[pos + 1] == "'":
                        buf_parts.append("'")
                        pos += 2
                    else:
                        pos += 1
                        break
                else:
                    buf_parts.append(text[pos])
                    pos += 1
            values.append("".join(buf_parts))
            continue

        # number / true / false / other literal — collect until comma or close paren
        buf_parts = []
        while pos < n and text[pos] not in ",)":
            buf_parts.append(text[pos])
            pos += 1
        values.append("".join(buf_parts).strip())

    return values


SAFE_NAME_RE = re.compile(r"[^\w一-龥\-_.]+", re.UNICODE)


def safe_filename(name: str, max_len: int = 80) -> str:
    """Slugify title for filesystem (preserve CJK, replace illegal chars with _)."""
    s = SAFE_NAME_RE.sub("_", name).strip("_")
    if len(s) > max_len:
        s = s[:max_len]
    return s or "untitled"


def parse_dump(dump_path: Path, out_dir: Path) -> dict:
    """Parse SQL dump, write one .md per row. Return per-data_type counts."""
    out_dir.mkdir(parents=True, exist_ok=True)

    counts: dict[str, int] = {}
    seen_filenames: dict[str, set] = {}
    insert_re_prefix = "INSERT INTO public.crawled_data "

    total_processed = 0
    total_skipped = 0

    with dump_path.open("r", encoding="utf-8", errors="replace") as f:
        for line_idx, line in enumerate(f, 1):
            if not line.startswith(insert_re_prefix):
                continue

            # find " VALUES (" — the open paren of the tuple
            v_idx = line.find(" VALUES (")
            if v_idx < 0:
                continue
            # body starts right after the opening paren
            tuple_body = line[v_idx + len(" VALUES (") :]
            # strip trailing );\n
            tuple_body = tuple_body.rstrip().rstrip(";").rstrip()
            if tuple_body.endswith(")"):
                tuple_body = tuple_body[:-1]

            values = parse_values(tuple_body)
            if len(values) < 6:
                total_skipped += 1
                continue

            row_id = values[0]
            source = values[1] or ""
            data_type = values[3] or "unknown"
            title = values[4] or ""
            content = values[5] or ""
            url = values[12] if len(values) > 12 else ""

            if not content.strip() or not title.strip():
                total_skipped += 1
                continue

            # category subdir per data_type
            sub = out_dir / data_type
            sub.mkdir(parents=True, exist_ok=True)

            base = safe_filename(title)
            seen = seen_filenames.setdefault(data_type, set())
            # de-dupe by id suffix when title collision
            fn = f"{base}.md"
            if fn in seen:
                fn = f"{base}_{row_id}.md"
            seen.add(fn)

            md_lines = [
                f"# {title}",
                "",
                f"- 来源: {source}",
                f"- 类型: {data_type}",
                f"- 原始ID: {row_id}",
            ]
            if url:
                md_lines.append(f"- URL: {url}")
            md_lines.append("")
            md_lines.append(content)
            md_text = "\n".join(md_lines)

            (sub / fn).write_text(md_text, encoding="utf-8")
            counts[data_type] = counts.get(data_type, 0) + 1
            total_processed += 1

            if total_processed % 1000 == 0:
                print(
                    f"[progress] line {line_idx}: {total_processed} written, "
                    f"types={counts}",
                    flush=True,
                )

    return {
        "counts_by_type": counts,
        "total_written": total_processed,
        "total_skipped": total_skipped,
    }


def main() -> None:
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        sys.exit(1)

    dump = Path(sys.argv[1])
    out = Path(sys.argv[2])

    if not dump.exists():
        print(f"error: dump not found: {dump}", file=sys.stderr)
        sys.exit(2)

    print(f"[start] parsing {dump} ({dump.stat().st_size / 1024 / 1024:.1f} MB)")
    print(f"[start] output dir: {out}")

    summary = parse_dump(dump, out)

    print()
    print("=" * 60)
    print("Summary")
    print("=" * 60)
    print(f"Total written : {summary['total_written']}")
    print(f"Total skipped : {summary['total_skipped']}")
    print(f"By type:")
    for k, v in sorted(summary["counts_by_type"].items(), key=lambda x: -x[1]):
        print(f"  {k:20} {v:>6}")
    print(f"Output: {out}")


if __name__ == "__main__":
    main()
