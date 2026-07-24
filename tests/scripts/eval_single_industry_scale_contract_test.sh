#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - "$ROOT" <<'PY'
import importlib.util
import sys
from pathlib import Path

root = Path(sys.argv[1])
validator_path = root / "scripts" / "eval" / "validate-manifests.py"
spec = importlib.util.spec_from_file_location("attune_eval_validate", validator_path)
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
sys.modules[spec.name] = module
spec.loader.exec_module(module)

corpora, scenarios, suites = module.collect_manifests(root)

scale_suite_ids = ["k3_rag_scale_thousand", "k3_rag_scale_ten_thousand"]
required_types = {
    "fact_lookup",
    "operation_guidance",
    "decision_assistance",
    "summary",
    "multiturn",
    "negative_evidence",
    "out_of_manual_industry_general",
}
minimum_documents = {
    "T2": 1000,
    "T3": 10000,
}

for suite_id in scale_suite_ids:
    suite = suites[suite_id]
    resolved_corpora, resolved_scenarios = module.validate_cross_references(suite, corpora, scenarios)
    domains = {corpus.data["domain"] for corpus in resolved_corpora}
    domains.update(scenario.data["domain"] for scenario in resolved_scenarios)
    assert len(domains) == 1, f"{suite_id} must use one industry domain, got {sorted(domains)}"
    domain = next(iter(domains))
    assert domain != "mixed_enterprise", f"{suite_id} must not use mixed_enterprise as scale corpus"
    for corpus in resolved_corpora:
        tier = corpus.data["scale"]["tier"]
        if tier in minimum_documents:
            docs = corpus.data["scale"]["documents"]
            assert docs >= minimum_documents[tier], f"{corpus.ident} {tier} needs >= {minimum_documents[tier]} docs"
        assert corpus.data.get("scale_policy", {}).get("single_industry") is True
        assert corpus.data.get("scale_policy", {}).get("industry_domain") == domain
    covered = set()
    for scenario in resolved_scenarios:
        covered.update(scenario.data.get("coverage", {}).get("question_types", []))
    missing = required_types - covered
    assert not missing, f"{suite_id} missing question coverage: {sorted(missing)}"
    thresholds = suite.data["thresholds"]
    for metric in (
        "retrieval_hit_at_5_min",
        "citation_hit_rate_min",
        "answer_accuracy_min",
        "summary_coverage_min",
        "multiturn_source_continuity_min",
        "negative_evidence_refusal_rate_min",
        "out_of_manual_boundary_rate_min",
        "terminal_error_rate_max",
        "hot_chat_p95_ms_max",
        "summary_p95_ms_max",
        "search_p95_ms_max",
        "pending_embeddings_drain_seconds_max",
    ):
        assert metric in thresholds, f"{suite_id} missing threshold {metric}"

print("eval single-industry scale contract PASS")
PY
