#!/usr/bin/env python3
"""Validate Attune RAG eval manifests.

This validator intentionally uses only the Python standard library so PR CI can
run it before optional JSON Schema dependencies are available. It enforces the
project's stable manifest contract and cross-references.
"""
from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any


SCHEMA_VERSIONS = {
    "corpus": "attune.eval.corpus.v1",
    "scenario": "attune.eval.scenario.v1",
    "suite": "attune.eval.suite.v1",
}


@dataclass(frozen=True)
class Manifest:
    kind: str
    ident: str
    path: Path
    data: dict[str, Any]


class ValidationError(Exception):
    pass


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--suite", required=True)
    parser.add_argument("--dry-run", action="store_true", help="Validate and print resolved suite summary.")
    return parser.parse_args()


def load_json(path: Path) -> dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ValidationError(f"{path}: invalid JSON: {exc}") from exc
    if not isinstance(data, dict):
        raise ValidationError(f"{path}: expected JSON object")
    return data


def require(data: dict[str, Any], path: Path, field: str, expected_type: type | tuple[type, ...] | None = None) -> Any:
    if field not in data:
        raise ValidationError(f"{path}: missing required field {field!r}")
    value = data[field]
    if expected_type is not None and not isinstance(value, expected_type):
        raise ValidationError(
            f"{path}: field {field!r} expected {type_name(expected_type)}, got {type(value).__name__}"
        )
    return value


def type_name(expected_type: type | tuple[type, ...]) -> str:
    if isinstance(expected_type, tuple):
        return " or ".join(t.__name__ for t in expected_type)
    return expected_type.__name__


def require_schema(data: dict[str, Any], path: Path, kind: str) -> None:
    version = require(data, path, "schema_version", str)
    expected = SCHEMA_VERSIONS[kind]
    if version != expected:
        raise ValidationError(f"{path}: schema_version must be {expected!r}, got {version!r}")


def require_nonempty_str_list(data: dict[str, Any], path: Path, field: str) -> list[str]:
    raw = require(data, path, field, list)
    if not raw:
        raise ValidationError(f"{path}: field {field!r} must not be empty")
    bad = [value for value in raw if not isinstance(value, str) or not value.strip()]
    if bad:
        raise ValidationError(f"{path}: field {field!r} must contain only non-empty strings")
    return raw


def validate_corpus(path: Path, data: dict[str, Any]) -> Manifest:
    require_schema(data, path, "corpus")
    ident = require(data, path, "corpus_id", str)
    require(data, path, "domain", str)
    require(data, path, "license", str)
    source = require(data, path, "source", dict)
    require(source, path, "type", str)
    scale = require(data, path, "scale", dict)
    tier = require(scale, path, "tier", str)
    if tier not in {"T0", "T1", "T2", "T3", "T4"}:
        raise ValidationError(f"{path}: scale.tier must be one of T0..T4, got {tier!r}")
    documents = require(scale, path, "documents", int)
    if documents < 1:
        raise ValidationError(f"{path}: scale.documents must be >= 1")
    expected_chunks = require(scale, path, "expected_chunks", int)
    if expected_chunks < 1:
        raise ValidationError(f"{path}: scale.expected_chunks must be >= 1")
    profiles = require(data, path, "profiles", dict)
    if not profiles:
        raise ValidationError(f"{path}: profiles must not be empty")
    indexing = require(data, path, "indexing", dict)
    parser_modes = require(indexing, path, "parser_modes", list)
    if not parser_modes or any(not isinstance(mode, str) or not mode for mode in parser_modes):
        raise ValidationError(f"{path}: indexing.parser_modes must contain non-empty strings")
    max_pending = require(indexing, path, "max_pending_seconds", int)
    if max_pending < 1:
        raise ValidationError(f"{path}: indexing.max_pending_seconds must be >= 1")
    return Manifest("corpus", ident, path, data)


def validate_scenario(path: Path, data: dict[str, Any]) -> Manifest:
    require_schema(data, path, "scenario")
    ident = require(data, path, "scenario_id", str)
    require(data, path, "domain", str)
    require(data, path, "scenario_type", str)
    require(data, path, "difficulty", str)
    require(data, path, "corpus_id", str)
    turns = require(data, path, "turns", list)
    if not turns:
        raise ValidationError(f"{path}: turns must not be empty")
    for idx, turn in enumerate(turns):
        if not isinstance(turn, dict):
            raise ValidationError(f"{path}: turns[{idx}] must be an object")
        for field in (
            "turn_id",
            "message",
            "answer_mode",
            "requires_citations",
            "expected_sources",
            "must_include",
            "must_not_include",
            "latency_budget_ms",
        ):
            require(turn, path, field)
        if not isinstance(turn["requires_citations"], bool):
            raise ValidationError(f"{path}: turns[{idx}].requires_citations must be boolean")
        for field in ("expected_sources", "must_include", "must_not_include"):
            if not isinstance(turn[field], list) or any(not isinstance(value, str) for value in turn[field]):
                raise ValidationError(f"{path}: turns[{idx}].{field} must be a string array")
        if not isinstance(turn["latency_budget_ms"], int) or turn["latency_budget_ms"] < 1:
            raise ValidationError(f"{path}: turns[{idx}].latency_budget_ms must be a positive integer")
    scheduler = data.get("scheduler")
    if scheduler is not None and not isinstance(scheduler, dict):
        raise ValidationError(f"{path}: scheduler must be an object when present")
    return Manifest("scenario", ident, path, data)


def validate_suite(path: Path, data: dict[str, Any]) -> Manifest:
    require_schema(data, path, "suite")
    ident = require(data, path, "suite_id", str)
    require(data, path, "purpose", str)
    require_nonempty_str_list(data, path, "corpora")
    require_nonempty_str_list(data, path, "scenarios")
    require_nonempty_str_list(data, path, "gates")
    thresholds = require(data, path, "thresholds", dict)
    if not thresholds:
        raise ValidationError(f"{path}: thresholds must not be empty")
    return Manifest("suite", ident, path, data)


def collect_manifests(root: Path) -> tuple[dict[str, Manifest], dict[str, Manifest], dict[str, Manifest]]:
    eval_root = root / "tests" / "eval"
    corpora: dict[str, Manifest] = {}
    scenarios: dict[str, Manifest] = {}
    suites: dict[str, Manifest] = {}
    for path in sorted((eval_root / "corpora").glob("**/*.json")):
        manifest = validate_corpus(path, load_json(path))
        add_unique(corpora, manifest)
    for path in sorted((eval_root / "scenarios").glob("**/*.json")):
        manifest = validate_scenario(path, load_json(path))
        add_unique(scenarios, manifest)
    for path in sorted((eval_root / "suites").glob("**/*.json")):
        manifest = validate_suite(path, load_json(path))
        add_unique(suites, manifest)
    return corpora, scenarios, suites


def add_unique(target: dict[str, Manifest], manifest: Manifest) -> None:
    previous = target.get(manifest.ident)
    if previous is not None:
        raise ValidationError(
            f"duplicate {manifest.kind} id {manifest.ident!r}: {previous.path} and {manifest.path}"
        )
    target[manifest.ident] = manifest


def validate_cross_references(
    suite: Manifest,
    corpora: dict[str, Manifest],
    scenarios: dict[str, Manifest],
) -> tuple[list[Manifest], list[Manifest]]:
    corpus_ids = suite.data["corpora"]
    scenario_ids = suite.data["scenarios"]
    missing_corpora = [ident for ident in corpus_ids if ident not in corpora]
    if missing_corpora:
        raise ValidationError(f"{suite.path}: unknown corpora: {', '.join(missing_corpora)}")
    missing_scenarios = [ident for ident in scenario_ids if ident not in scenarios]
    if missing_scenarios:
        raise ValidationError(f"{suite.path}: unknown scenarios: {', '.join(missing_scenarios)}")

    resolved_corpora = [corpora[ident] for ident in corpus_ids]
    resolved_scenarios = [scenarios[ident] for ident in scenario_ids]
    suite_corpus_set = set(corpus_ids)
    for scenario in resolved_scenarios:
        scenario_corpus = scenario.data["corpus_id"]
        if scenario_corpus not in corpora:
            raise ValidationError(f"{scenario.path}: unknown corpus_id {scenario_corpus!r}")
        if scenario_corpus not in suite_corpus_set:
            raise ValidationError(
                f"{suite.path}: scenario {scenario.ident!r} uses corpus {scenario_corpus!r} "
                "which is not listed in suite.corpora"
            )
    validate_scale_suite_contract(suite, resolved_corpora, resolved_scenarios)
    return resolved_corpora, resolved_scenarios


def validate_scale_suite_contract(suite: Manifest, corpora: list[Manifest], scenarios: list[Manifest]) -> None:
    if not suite.ident.startswith("k3_rag_scale_"):
        return
    domains = {corpus.data["domain"] for corpus in corpora}
    domains.update(scenario.data["domain"] for scenario in scenarios)
    if len(domains) != 1:
        raise ValidationError(f"{suite.path}: scale suite must use a single industry domain, got {sorted(domains)}")
    domain = next(iter(domains))
    if domain == "mixed_enterprise":
        raise ValidationError(f"{suite.path}: scale suite must use a single industry domain, not mixed_enterprise")
    minimum_documents = {"T2": 1000, "T3": 10000}
    for corpus in corpora:
        tier = corpus.data["scale"]["tier"]
        documents = corpus.data["scale"]["documents"]
        if tier in minimum_documents and documents < minimum_documents[tier]:
            raise ValidationError(
                f"{corpus.path}: {tier} single industry corpus must have >= {minimum_documents[tier]} documents"
            )
        policy = corpus.data.get("scale_policy")
        if not isinstance(policy, dict) or policy.get("single_industry") is not True:
            raise ValidationError(f"{corpus.path}: scale_policy.single_industry must be true")
        if policy.get("industry_domain") != domain:
            raise ValidationError(f"{corpus.path}: scale_policy.industry_domain must be {domain!r}")
    covered: set[str] = set()
    for scenario in scenarios:
        coverage = scenario.data.get("coverage")
        if isinstance(coverage, dict):
            question_types = coverage.get("question_types")
            if isinstance(question_types, list):
                covered.update(value for value in question_types if isinstance(value, str))
    required = {
        "fact_lookup",
        "operation_guidance",
        "decision_assistance",
        "summary",
        "multiturn",
        "negative_evidence",
        "out_of_manual_industry_general",
        "multi_intent_decomposition",
        "per_topic_source_quota",
        "terminology_constraints",
    }
    missing = required - covered
    if missing:
        raise ValidationError(f"{suite.path}: scale suite missing question coverage: {', '.join(sorted(missing))}")


def summary_payload(suite: Manifest, corpora: list[Manifest], scenarios: list[Manifest]) -> dict[str, Any]:
    return {
        "schema_version": suite.data["schema_version"],
        "suite_id": suite.ident,
        "purpose": suite.data["purpose"],
        "corpora": [
            {
                "corpus_id": corpus.ident,
                "domain": corpus.data["domain"],
                "tier": corpus.data["scale"]["tier"],
                "documents": corpus.data["scale"]["documents"],
                "path": str(corpus.path),
            }
            for corpus in corpora
        ],
        "scenarios": [
            {
                "scenario_id": scenario.ident,
                "domain": scenario.data["domain"],
                "scenario_type": scenario.data["scenario_type"],
                "turns": len(scenario.data["turns"]),
                "corpus_id": scenario.data["corpus_id"],
                "path": str(scenario.path),
            }
            for scenario in scenarios
        ],
        "gates": suite.data["gates"],
        "thresholds": suite.data["thresholds"],
    }


def main() -> int:
    args = parse_args()
    root = args.root.resolve()
    try:
        corpora, scenarios, suites = collect_manifests(root)
        suite = suites.get(args.suite)
        if suite is None:
            raise ValidationError(f"unknown suite {args.suite!r}")
        resolved_corpora, resolved_scenarios = validate_cross_references(suite, corpora, scenarios)
        payload = summary_payload(suite, resolved_corpora, resolved_scenarios)
    except ValidationError as exc:
        print(f"manifest validation failed: {exc}", file=sys.stderr)
        return 1
    print(json.dumps(payload, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
