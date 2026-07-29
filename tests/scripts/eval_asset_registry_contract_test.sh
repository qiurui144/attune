#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
REGISTRY="$ROOT/tests/eval/assets/public_knowledge_assets.json"
DOC="$ROOT/docs/specs/2026-07-23-attune-rag-evaluation-assets.md"

python3 - "$REGISTRY" "$DOC" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
doc_path = Path(sys.argv[2])
data = json.loads(path.read_text(encoding="utf-8"))
doc = doc_path.read_text(encoding="utf-8")

assert data["schema_version"] == "attune.eval.asset_registry.v1"
assets = data["assets"]
assert isinstance(assets, list) and len(assets) >= 8

required_domains = {
    "networking",
    "aviation",
    "mechanical",
    "security",
    "cloud_native",
    "embedded",
    "observability",
    "software_engineering",
}
assert required_domains.issubset({asset["domain"] for asset in assets})

for asset in assets:
    assert asset["asset_id"]
    assert asset["domain"]
    assert asset["tier"] in {"T0", "T1", "T2", "T3"}
    assert asset["source"]["type"] in {"generated", "git", "https", "gitbook", "pdf"}
    assert asset["source"]["url"]
    assert asset["source"].get("pin") or asset["source"].get("snapshot_id")
    assert asset["license"]["status"] in {"clear", "review_required", "generated"}
    assert asset["redistribution"] in {"vendored_generated_only", "external_fetch_only", "redistributable_reference"}
    assert asset["materialization"]["mode"] in {
        "inline_generated",
        "git_sparse_checkout",
        "git_lfs_checkout",
        "http_snapshot",
        "gitbook_html_snapshot",
        "pdf_text_layer_snapshot",
    }
    assert asset["evaluation"]["scenario_types"]
    assert asset["evaluation"]["expected_questions"]
    assert asset["evaluation"]["expected_sources"]
    assert asset["evaluation"]["must_include_terms"]
    assert asset["evaluation"]["metrics"]["retrieval_hit_at_5_min"] > 0
    assert asset["evaluation"]["metrics"]["citation_hit_rate_min"] > 0
    assert asset["evaluation"]["metrics"]["answer_accuracy_min"] > 0
    assert asset["evaluation"]["metrics"]["hot_chat_p95_ms_max"] > 0
    assert asset["asset_id"] in doc

text = path.read_text(encoding="utf-8")
for forbidden in ("TBD", "TODO", "<repo>", "<commit>", "<license>"):
    assert forbidden not in text

print("eval asset registry contract PASS")
PY
