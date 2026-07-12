#!/usr/bin/env python3
"""Probe the edge scheduler contract surface Attune consumes.

This is an Attune-side gate. It does not mutate scheduler state and does not
call private worker endpoints. In strict mode it requires the post-v2 contract
signals Attune needs for long-text E2E: schema versions, prompt-cache metadata,
refusal policy, model state, capacity and readiness.
"""
from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.parse
import urllib.request
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--timeout", type=float, default=5.0)
    parser.add_argument(
        "--strict",
        dest="strict",
        action="store_true",
        default=True,
        help="Require v2 schema/refusal/prompt-cache contract fields (default).",
    )
    parser.add_argument(
        "--no-strict",
        dest="strict",
        action="store_false",
        help="Only validate the minimal legacy-compatible contract.",
    )
    return parser.parse_args()


def normalize_base(base_url: str) -> str:
    base = base_url.strip().rstrip("/")
    if base.endswith("/v1"):
        base = base[:-3].rstrip("/")
    return base


def request(base_url: str, path: str, timeout: float, accept_json: bool = True) -> tuple[int, Any]:
    url = f"{base_url}{path}"
    req = urllib.request.Request(url, method="GET")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode(errors="replace")
            if not accept_json:
                return resp.status, raw
            if not raw:
                return resp.status, {}
            return resp.status, json.loads(raw)
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode(errors="replace")
        raise RuntimeError(f"GET {path} failed HTTP {exc.code}: {raw[:300]}") from exc
    except Exception as exc:  # noqa: BLE001 - CLI should preserve endpoint context.
        raise RuntimeError(f"GET {path} failed: {exc}") from exc


def request_first(base_url: str, paths: list[str], timeout: float, accept_json: bool = True) -> tuple[str, int, Any]:
    errors: list[str] = []
    for path in paths:
        try:
            status, data = request(base_url, path, timeout, accept_json=accept_json)
            return path, status, data
        except RuntimeError as exc:
            errors.append(str(exc))
    joined = "; ".join(errors)
    raise RuntimeError(f"all probes failed for {paths}: {joined}")


def require(condition: bool, failures: list[str], message: str) -> None:
    if not condition:
        failures.append(message)


def flatten(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True).casefold()


def validate_contract(contract: dict[str, Any], strict: bool, failures: list[str]) -> None:
    tasks = contract.get("runtime_tasks")
    require(isinstance(tasks, list) and tasks, failures, "/benchmark/contract.runtime_tasks must be non-empty")
    task_names = {str(task.get("name")) for task in tasks or [] if isinstance(task, dict)}
    require("kb.query.ask" in task_names, failures, "runtime task kb.query.ask is required")

    models = contract.get("models")
    require(isinstance(models, list) and models, failures, "/benchmark/contract.models must be non-empty")
    require(
        any(
            isinstance(model, dict)
            and (
                int(model.get("max_context_tokens_sync") or 0) > 0
                or int(model.get("max_context_tokens_async") or 0) > 0
            )
            for model in models or []
        ),
        failures,
        "at least one contract model must expose context token caps",
    )

    endpoints = contract.get("application_api") or contract.get("endpoints") or {}
    endpoint_text = flatten(endpoints)
    require("/kb/tasks/{task}" in endpoint_text, failures, "contract must advertise /kb/tasks/{task}")
    require("/jobs/" in endpoint_text, failures, "contract must advertise /jobs/{job_id}")

    if not strict:
        return

    schema_versions = contract.get("schema_versions")
    require(isinstance(schema_versions, dict) and schema_versions, failures, "schema_versions must be present")
    schema_text = flatten(schema_versions)
    for name in ("benchmark_contract.v2", "models.v2", "capacity.v2", "jobs.v2", "kb_task"):
        require(name in schema_text, failures, f"schema_versions missing {name}")

    contract_text = flatten(contract)
    require("cache" in contract_text, failures, "prompt-cache metadata contract is required")
    require("scheduler_refusal_v1" in contract_text, failures, "scheduler_refusal_v1 refusal policy is required")


def validate_models(models: dict[str, Any], failures: list[str]) -> None:
    entries = models.get("models")
    require(isinstance(entries, list) and entries, failures, "/models.models must be non-empty")
    require(
        any(isinstance(model, dict) and str(model.get("name") or "").strip() for model in entries or []),
        failures,
        "/models must include named models",
    )
    require(
        any(
            isinstance(model, dict)
            and str(model.get("state") or model.get("lifecycle") or "").strip()
            for model in entries or []
        ),
        failures,
        "/models must expose state or lifecycle",
    )


def validate_capacity(capacity: dict[str, Any], failures: list[str]) -> None:
    memory = capacity.get("memory")
    require(isinstance(memory, dict) or float(capacity.get("dram_total_gb") or 0) > 0, failures, "/capacity must expose memory or dram_total_gb")
    if isinstance(memory, dict):
        require(
            bool(str(memory.get("status") or "").strip())
            or memory.get("available_gb") is not None
            or memory.get("used_gb") is not None,
            failures,
            "/capacity.memory must expose status or memory gauges",
        )


def main() -> int:
    args = parse_args()
    base_url = normalize_base(args.base_url)
    failures: list[str] = []

    ready_path, ready_status, _ = request_first(base_url, ["/ready?hot=1", "/ready"], args.timeout, accept_json=False)
    health_path, health_status, _ = request_first(
        base_url, ["/health", "/healthz"], args.timeout, accept_json=False
    )
    _, _, contract = request_first(base_url, ["/benchmark/contract"], args.timeout)
    _, _, models = request_first(base_url, ["/models"], args.timeout)
    _, _, capacity = request_first(base_url, ["/capacity"], args.timeout)

    require(200 <= ready_status < 300, failures, f"{ready_path} must return 2xx")
    require(200 <= health_status < 300, failures, f"{health_path} must return 2xx")
    require(isinstance(contract, dict), failures, "/benchmark/contract must be a JSON object")
    require(isinstance(models, dict), failures, "/models must be a JSON object")
    require(isinstance(capacity, dict), failures, "/capacity must be a JSON object")

    if isinstance(contract, dict):
        validate_contract(contract, args.strict, failures)
    if isinstance(models, dict):
        validate_models(models, failures)
    if isinstance(capacity, dict):
        validate_capacity(capacity, failures)

    summary = {
        "base_url": base_url,
        "strict": args.strict,
        "ready_path": ready_path,
        "health_path": health_path,
        "contract_version": contract.get("contract_version") if isinstance(contract, dict) else None,
        "models": len(models.get("models", [])) if isinstance(models, dict) else 0,
        "failures": failures,
    }
    print(json.dumps(summary, ensure_ascii=False, indent=2))
    return 2 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
