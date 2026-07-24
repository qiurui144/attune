#!/usr/bin/env python3
"""Compare two Attune eval reports and flag regressions."""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any


QUALITY_HIGHER_IS_BETTER = [
    ("retrieval.hit_at_5", ("metrics", "retrieval", "hit_at_5")),
    ("answer.citation_hit_rate", ("metrics", "answer", "citation_hit_rate")),
    ("answer.answer_accuracy", ("metrics", "answer", "answer_accuracy")),
]

QUALITY_LOWER_IS_BETTER = [
    ("summary.terminal_error_rate", ("summary", "terminal_error_rate")),
    ("performance.chat_p95_ms", ("metrics", "performance", "chat_p95_ms")),
    ("summary.failures", ("summary", "failures")),
]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--fail-on-regression", action="store_true")
    parser.add_argument("--quality-tolerance", type=float, default=0.000001)
    parser.add_argument("--latency-tolerance-ms", type=float, default=0.0)
    return parser.parse_args()


def load_report(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise ValueError(f"{path}: expected JSON object")
    if data.get("schema_version") != "attune.eval.report.v1":
        raise ValueError(f"{path}: schema_version must be attune.eval.report.v1")
    return data


def get_number(data: dict[str, Any], path: tuple[str, ...]) -> float | None:
    cur: Any = data
    for key in path:
        if not isinstance(cur, dict) or key not in cur:
            return None
        cur = cur[key]
    if isinstance(cur, bool) or not isinstance(cur, (int, float)):
        return None
    return float(cur)


def failure_layer_counts(report: dict[str, Any]) -> dict[str, int]:
    counts: dict[str, int] = {}
    failures = report.get("failures")
    if not isinstance(failures, list):
        return counts
    for failure in failures:
        if not isinstance(failure, dict):
            continue
        layer = failure.get("failure_layer") or "unclassified"
        if not isinstance(layer, str) or not layer:
            layer = "unclassified"
        counts[layer] = counts.get(layer, 0) + 1
    return counts


def compare(args: argparse.Namespace, baseline: dict[str, Any], candidate: dict[str, Any]) -> dict[str, Any]:
    regressions: list[dict[str, Any]] = []
    improvements: list[dict[str, Any]] = []
    unchanged: list[dict[str, Any]] = []

    if baseline.get("suite_id") != candidate.get("suite_id"):
        regressions.append(
            {
                "metric": "suite_id",
                "baseline": baseline.get("suite_id"),
                "candidate": candidate.get("suite_id"),
                "reason": "candidate suite differs from baseline",
            }
        )

    if baseline.get("summary", {}).get("pass") is True and candidate.get("summary", {}).get("pass") is not True:
        regressions.append(
            {
                "metric": "summary.pass",
                "baseline": True,
                "candidate": candidate.get("summary", {}).get("pass"),
                "reason": "candidate report did not pass",
            }
        )

    for metric, path in QUALITY_HIGHER_IS_BETTER:
        base = get_number(baseline, path)
        cand = get_number(candidate, path)
        classify_metric(
            metric,
            base,
            cand,
            higher_is_better=True,
            tolerance=args.quality_tolerance,
            regressions=regressions,
            improvements=improvements,
            unchanged=unchanged,
        )

    for metric, path in QUALITY_LOWER_IS_BETTER:
        tolerance = args.latency_tolerance_ms if metric.endswith("chat_p95_ms") else args.quality_tolerance
        base = get_number(baseline, path)
        cand = get_number(candidate, path)
        classify_metric(
            metric,
            base,
            cand,
            higher_is_better=False,
            tolerance=tolerance,
            regressions=regressions,
            improvements=improvements,
            unchanged=unchanged,
        )

    base_layers = failure_layer_counts(baseline)
    cand_layers = failure_layer_counts(candidate)
    for layer in sorted(set(base_layers) | set(cand_layers)):
        base = float(base_layers.get(layer, 0))
        cand = float(cand_layers.get(layer, 0))
        classify_metric(
            f"failure_layer.{layer}",
            base,
            cand,
            higher_is_better=False,
            tolerance=0.0,
            regressions=regressions,
            improvements=improvements,
            unchanged=unchanged,
        )

    return {
        "schema_version": "attune.eval.report_diff.v1",
        "baseline": str(args.baseline),
        "candidate": str(args.candidate),
        "suite_id": candidate.get("suite_id"),
        "pass": not regressions,
        "regressions": regressions,
        "improvements": improvements,
        "unchanged": unchanged,
    }


def classify_metric(
    metric: str,
    baseline: float | None,
    candidate: float | None,
    *,
    higher_is_better: bool,
    tolerance: float,
    regressions: list[dict[str, Any]],
    improvements: list[dict[str, Any]],
    unchanged: list[dict[str, Any]],
) -> None:
    if baseline is None and candidate is None:
        unchanged.append({"metric": metric, "baseline": None, "candidate": None, "reason": "metric absent in both"})
        return
    if baseline is not None and candidate is None:
        regressions.append(
            {
                "metric": metric,
                "baseline": baseline,
                "candidate": None,
                "reason": "candidate metric missing",
            }
        )
        return
    if baseline is None and candidate is not None:
        improvements.append(
            {
                "metric": metric,
                "baseline": None,
                "candidate": candidate,
                "reason": "candidate added metric",
            }
        )
        return

    assert baseline is not None and candidate is not None
    delta = candidate - baseline
    if higher_is_better:
        if candidate + tolerance < baseline:
            bucket = regressions
            reason = "candidate is lower"
        elif candidate > baseline + tolerance:
            bucket = improvements
            reason = "candidate is higher"
        else:
            bucket = unchanged
            reason = "within tolerance"
    else:
        if candidate > baseline + tolerance:
            bucket = regressions
            reason = "candidate is higher"
        elif candidate + tolerance < baseline:
            bucket = improvements
            reason = "candidate is lower"
        else:
            bucket = unchanged
            reason = "within tolerance"
    bucket.append(
        {
            "metric": metric,
            "baseline": baseline,
            "candidate": candidate,
            "delta": delta,
            "reason": reason,
        }
    )


def main() -> int:
    args = parse_args()
    try:
        baseline = load_report(args.baseline)
        candidate = load_report(args.candidate)
        diff = compare(args, baseline, candidate)
        if args.out:
            args.out.parent.mkdir(parents=True, exist_ok=True)
            args.out.write_text(json.dumps(diff, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    except Exception as exc:
        print(f"report-diff failed: {exc}", file=sys.stderr)
        return 1

    if diff["regressions"]:
        for item in diff["regressions"]:
            print(
                f"regression {item['metric']}: baseline={item.get('baseline')} "
                f"candidate={item.get('candidate')} ({item.get('reason')})",
                file=sys.stderr,
            )
        return 1 if args.fail_on_regression else 0

    print("report-diff PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
