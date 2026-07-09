#!/usr/bin/env python3
"""Evaluate airplane-manual long-text vector search against Attune.

This script measures retrieval accuracy and latency. It intentionally evaluates
the search/vector layer first; answer/citation evaluation should be layered on
top after the local scheduler chat path is running against the same corpus.

Usage:
  python3 scripts/eval-airplane-manual-longtext-search.py \
    --base-url http://127.0.0.1:8787 \
    --token "$ATTUNE_TOKEN" \
    --profile local_scheduler_30b
"""
from __future__ import annotations

import argparse
import json
import statistics
import sys
import time
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "tests/e2e"))

from airplane_longtext_support import (  # noqa: E402
    auth_json_headers,
    filtered_queries,
    load_manifest,
    percentile,
    profile_doc_ids,
)

DEFAULT_MANIFEST = REPO_ROOT / "tests/e2e/airplane_manual_longtext_cases.json"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--token", default="")
    parser.add_argument("--profile", default="local_scheduler_30b")
    parser.add_argument("--limit", type=int, default=10)
    parser.add_argument("--warmup", type=int, default=1)
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("--out", type=Path, default=None)
    parser.add_argument("--fail-on-targets", action="store_true")
    return parser.parse_args()


def request_search(args: argparse.Namespace, query: str) -> tuple[float, list[dict[str, Any]]]:
    qs = urllib.parse.urlencode({"q": query, "limit": args.limit})
    url = f"{args.base_url.rstrip('/')}/api/v1/search?{qs}"
    req = urllib.request.Request(url, headers=auth_json_headers(args.token))
    start = time.perf_counter()
    with urllib.request.urlopen(req, timeout=args.timeout) as resp:
        data = json.loads(resp.read())
    elapsed_ms = (time.perf_counter() - start) * 1000
    items = data.get("results", data.get("items", data if isinstance(data, list) else []))
    if not isinstance(items, list):
        items = []
    return elapsed_ms, [item for item in items if isinstance(item, dict)]


def flatten_item(item: dict[str, Any]) -> str:
    fields = []
    for key in ("id", "doc_id", "item_id", "title", "path", "file", "source", "source_path", "url"):
        val = item.get(key)
        if val is not None:
            fields.append(str(val))
    metadata = item.get("metadata")
    if isinstance(metadata, dict):
        for key in ("id", "doc_id", "path", "file", "source_path", "title"):
            val = metadata.get(key)
            if val is not None:
                fields.append(str(val))
    # Include a compact JSON fallback so evaluators still work if Attune changes
    # result field names but keeps source metadata in the object.
    fields.append(json.dumps(item, ensure_ascii=False, sort_keys=True)[:4000])
    return "\n".join(fields).lower()


def hit_rank(
    items: list[dict[str, Any]],
    acceptable_hits: list[str],
    acceptable_files: list[str],
) -> tuple[int | None, set[str]]:
    needles: dict[str, list[str]] = {}
    for hit in acceptable_hits:
        needles.setdefault(hit, []).append(hit.lower())
    for hit, file in zip(acceptable_hits, acceptable_files):
        file_lower = file.lower()
        title = Path(file).stem.lower()
        needles.setdefault(hit, []).extend([file_lower, title])

    found: set[str] = set()
    first_rank: int | None = None
    for rank, item in enumerate(items, 1):
        haystack = flatten_item(item)
        for hit, hit_needles in needles.items():
            if any(needle and needle in haystack for needle in hit_needles):
                found.add(hit)
                if first_rank is None:
                    first_rank = rank
    return first_rank, found


def evaluate(args: argparse.Namespace) -> dict[str, Any]:
    manifest = load_manifest(args.manifest)
    doc_ids = profile_doc_ids(manifest, args.profile)
    queries = filtered_queries(manifest, doc_ids)
    if not queries:
        raise SystemExit(f"no queries apply to profile {args.profile}")

    for _ in range(max(args.warmup, 0)):
        request_search(args, queries[0]["query"])

    rows = []
    for query in queries:
        try:
            elapsed_ms, items = request_search(args, query["query"])
            rank, found = hit_rank(items, query["acceptable_hits"], query.get("acceptable_files", []))
            acceptable = set(query["acceptable_hits"])
            rows.append(
                {
                    "id": query["id"],
                    "category": query.get("category"),
                    "latency_ms": elapsed_ms,
                    "hit": rank is not None and rank <= args.limit,
                    "first_hit_rank": rank,
                    "recall": len(found.intersection(acceptable)) / max(len(acceptable), 1),
                    "mrr": 1.0 / rank if rank else 0.0,
                    "found_hits": sorted(found),
                    "top_count": len(items),
                }
            )
        except Exception as exc:  # noqa: BLE001 - script should report per-query failures.
            rows.append(
                {
                    "id": query["id"],
                    "category": query.get("category"),
                    "error": str(exc),
                    "latency_ms": 0.0,
                    "hit": False,
                    "first_hit_rank": None,
                    "recall": 0.0,
                    "mrr": 0.0,
                    "found_hits": [],
                    "top_count": 0,
                }
            )

    latencies = [row["latency_ms"] for row in rows if not row.get("error")]
    summary = {
        "manifest": str(args.manifest),
        "profile": args.profile,
        "limit": args.limit,
        "queries": len(rows),
        "errors": sum(1 for row in rows if row.get("error")),
        "hit_at_5": sum(
            1
            for row in rows
            if row["first_hit_rank"] is not None and row["first_hit_rank"] <= 5
        )
        / len(rows),
        "hit_at_10": sum(
            1
            for row in rows
            if row["first_hit_rank"] is not None and row["first_hit_rank"] <= 10
        )
        / len(rows),
        "hit_at_k": sum(1 for row in rows if row["hit"]) / len(rows),
        "recall_at_k": statistics.fmean(row["recall"] for row in rows),
        "mrr_at_10": statistics.fmean(
            (1.0 / row["first_hit_rank"])
            if row["first_hit_rank"] is not None and row["first_hit_rank"] <= 10
            else 0.0
            for row in rows
        ),
        "mrr": statistics.fmean(row["mrr"] for row in rows),
        "latency_ms": {
            "p50": percentile(latencies, 50),
            "p95": percentile(latencies, 95),
            "max": max(latencies) if latencies else 0.0,
        },
    }
    return {"summary": summary, "rows": rows}


def check_targets(result: dict[str, Any], manifest: dict[str, Any]) -> bool:
    targets = manifest.get("evaluation_targets", {}).get("vector_search", {})
    summary = result["summary"]
    failures = []
    if summary["hit_at_5"] < targets.get("hit_at_5_min", 0.0):
        failures.append(f"hit_at_5 {summary['hit_at_5']:.3f} below target")
    if summary["recall_at_k"] < targets.get("recall_at_10_min", 0.0):
        failures.append(f"recall_at_k {summary['recall_at_k']:.3f} below target")
    if summary["mrr_at_10"] < targets.get("mrr_at_10_min", 0.0):
        failures.append(f"mrr_at_10 {summary['mrr_at_10']:.3f} below target")
    if summary["latency_ms"]["p50"] > targets.get("warm_p50_latency_ms_max", float("inf")):
        failures.append(f"p50 latency {summary['latency_ms']['p50']:.1f}ms above target")
    if summary["latency_ms"]["p95"] > targets.get("warm_p95_latency_ms_max", float("inf")):
        failures.append(f"p95 latency {summary['latency_ms']['p95']:.1f}ms above target")

    if failures:
        print("target failures:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return False
    return True


def main() -> int:
    args = parse_args()
    result = evaluate(args)
    manifest = load_manifest(args.manifest)
    text = json.dumps(result, ensure_ascii=False, indent=2)
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(text + "\n", encoding="utf-8")
    print(json.dumps(result["summary"], ensure_ascii=False, indent=2))
    if args.fail_on_targets and not check_targets(result, manifest):
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
