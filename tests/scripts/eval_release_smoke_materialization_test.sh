#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - "$ROOT" <<'PY'
import json
import sys
from pathlib import Path

root = Path(sys.argv[1])
scenario_by_id = {}
for path in (root / "tests/eval/scenarios").glob("**/*.json"):
    scenario = json.loads(path.read_text(encoding="utf-8"))
    scenario_by_id[scenario["scenario_id"]] = scenario

corpus_by_id = {}
for path in (root / "tests/eval/corpora").glob("**/*.json"):
    corpus = json.loads(path.read_text(encoding="utf-8"))
    corpus_by_id[corpus["corpus_id"]] = corpus

for suite_path in sorted((root / "tests/eval/suites").glob("k3_rag_release_smoke*.json")):
    suite = json.loads(suite_path.read_text(encoding="utf-8"))
    expected_sources = set()
    for scenario_id in suite["scenarios"]:
        scenario = scenario_by_id.get(scenario_id)
        assert scenario is not None, f"{suite['suite_id']} missing scenario {scenario_id}"
        for turn in scenario["turns"]:
            expected_sources.update(turn.get("expected_sources", []))

    materialized = set()
    for corpus_id in suite["corpora"]:
        corpus = corpus_by_id.get(corpus_id)
        assert corpus is not None, f"{suite['suite_id']} missing corpus {corpus_id}"
        for doc in corpus.get("generated_documents", []):
            materialized.add(str(doc.get("id") or ""))
            materialized.add(str(doc.get("filename") or "").rsplit(".", 1)[0])
            materialized.add(str(doc.get("title") or ""))

    missing = sorted(source for source in expected_sources if source not in materialized)
    assert not missing, (
        f"{suite['suite_id']} expected sources are not materialized as no-OCR generated docs: {missing}"
    )
PY

echo "eval release smoke materialization contract PASS"
