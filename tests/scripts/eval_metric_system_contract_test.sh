#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ASSETS="$ROOT/tests/eval/assets/public_knowledge_assets.json"
REPORT_SCHEMA="$ROOT/tests/eval/schemas/report.schema.json"
RUNNER="$ROOT/scripts/eval/run-suite.py"

python3 - "$ASSETS" "$REPORT_SCHEMA" "$RUNNER" <<'PY'
import json
import sys
from pathlib import Path

assets = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
report_schema = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
runner = Path(sys.argv[3]).read_text(encoding="utf-8")

required_categories = {
    "retrieval",
    "citation",
    "answer",
    "summary",
    "multiturn",
    "performance",
    "stability",
    "frontend",
}
required_metrics = {
    "retrieval_hit_at_5_min",
    "retrieval_mrr_min",
    "empty_retrieval_rate_max",
    "citation_hit_rate_min",
    "wrong_citation_rate_max",
    "answer_accuracy_min",
    "forbidden_term_violation_rate_max",
    "summary_coverage_min",
    "source_preservation_min",
    "multiturn_source_continuity_min",
    "negative_evidence_refusal_rate_min",
    "out_of_manual_boundary_rate_min",
    "search_p95_ms_max",
    "hot_chat_p95_ms_max",
    "summary_p95_ms_max",
    "terminal_error_rate_max",
    "async_job_timeout_rate_max",
    "web_demo_flow_pass_rate_min",
}

system = assets.get("metric_system")
assert isinstance(system, dict), "asset registry missing top-level metric_system"
assert required_categories.issubset(system.keys()), "asset registry metric categories incomplete"
defaults = assets.get("metric_defaults")
assert isinstance(defaults, dict), "asset registry missing metric_defaults"
for asset in assets["assets"]:
    metrics = {**defaults, **asset["evaluation"]["metrics"]}
    missing = required_metrics - set(metrics.keys())
    assert not missing, f"{asset['asset_id']} missing metrics {sorted(missing)}"

schema_text = json.dumps(report_schema, ensure_ascii=False)
for category in required_categories - {"frontend"}:
    assert category in schema_text, f"report schema missing metrics.{category}"
for key in ("summary_coverage", "source_preservation", "multiturn_source_continuity", "out_of_manual_boundary_rate", "web_demo"):
    assert key in runner, f"runner missing {key}"

print("eval metric system contract PASS")
PY
