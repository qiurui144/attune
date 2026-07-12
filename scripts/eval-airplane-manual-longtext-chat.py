#!/usr/bin/env python3
"""Evaluate airplane-manual long-text RAG answers against Attune chat.

This complements eval-airplane-manual-longtext-search.py. It measures answer
latency, citation grounding, context admission size, and safety-boundary cases.
"""
from __future__ import annotations

import argparse
import json
import statistics
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "tests/e2e"))

from airplane_longtext_support import (  # noqa: E402
    aliased_target_value,
    attune_http_error_from_urllib,
    auth_json_headers,
    citation_hit,
    exception_error_fields,
    expected_term_hit,
    filtered_queries,
    load_manifest,
    maybe_poll_local_scheduler,
    output_text,
    percentile,
    profile_doc_ids,
    refuses_operational_advice,
    unsafe_operational_advice,
)

DEFAULT_MANIFEST = REPO_ROOT / "tests/e2e/airplane_manual_longtext_cases.json"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--token", default="")
    parser.add_argument("--profile", default="edge_scheduler_30b")
    parser.add_argument("--timeout", type=float, default=120.0)
    parser.add_argument("--poll-timeout", type=float, default=180.0)
    parser.add_argument("--poll-interval", type=float, default=2.0)
    parser.add_argument("--out", type=Path, default=None)
    parser.add_argument("--fail-on-targets", action="store_true")
    parser.add_argument(
        "--require-scheduler-generation",
        action="store_true",
        help="Fail unless every successful row uses the scheduler answer generation path.",
    )
    parser.add_argument(
        "--require-prompt-cache-metadata",
        action="store_true",
        help="Fail unless scheduler generation rows expose prompt-cache/cache metadata.",
    )
    parser.add_argument(
        "--scheduler-generation-p95-ms-max",
        type=float,
        default=None,
        help="Optional p95 bound for scheduler-reported generation latency.",
    )
    return parser.parse_args()


def post_chat(args: argparse.Namespace, message: str) -> tuple[float, dict[str, Any]]:
    path = "/api/v1/chat"
    url = f"{args.base_url.rstrip('/')}/api/v1/chat"
    body = json.dumps({"message": message, "history": []}).encode()
    req = urllib.request.Request(
        url,
        data=body,
        headers=auth_json_headers(args.token, content_type=True),
        method="POST",
    )
    start = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=args.timeout) as resp:
            data = json.loads(resp.read())
    except urllib.error.HTTPError as exc:
        raise attune_http_error_from_urllib("POST", path, exc) from exc
    elapsed_ms = (time.perf_counter() - start) * 1000
    return elapsed_ms, data


def number_value(value: Any) -> float | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    return None


def scheduler_job(response: dict[str, Any]) -> dict[str, Any]:
    job = response.get("local_scheduler_job")
    return job if isinstance(job, dict) else {}


def scheduler_meta(response: dict[str, Any]) -> dict[str, Any]:
    meta = response.get("local_scheduler")
    return meta if isinstance(meta, dict) else {}


def scheduler_outputs(response: dict[str, Any]) -> dict[str, Any]:
    job = scheduler_job(response)
    outputs = job.get("outputs")
    if isinstance(outputs, dict):
        return outputs
    return {}


def scheduler_timings(response: dict[str, Any]) -> dict[str, Any]:
    timings = scheduler_outputs(response).get("timings")
    return timings if isinstance(timings, dict) else {}


def scheduler_usage(response: dict[str, Any]) -> dict[str, Any]:
    usage = scheduler_outputs(response).get("usage")
    return usage if isinstance(usage, dict) else {}


def scheduler_finish_reason(response: dict[str, Any]) -> str | None:
    choices = scheduler_outputs(response).get("choices")
    if isinstance(choices, list) and choices:
        first = choices[0]
        if isinstance(first, dict):
            value = first.get("finish_reason")
            return str(value) if value is not None else None
    return None


def scheduler_latency_ms(response: dict[str, Any]) -> float | None:
    for obj in (scheduler_job(response), scheduler_meta(response)):
        value = number_value(obj.get("latency_ms"))
        if value is not None:
            return value
    return None


def scheduler_queue_wait_ms(response: dict[str, Any]) -> float | None:
    for obj in (scheduler_job(response), scheduler_meta(response)):
        value = number_value(obj.get("queue_wait_ms"))
        if value is not None:
            return value
    return None


def scheduler_cold_start_wait_ms(response: dict[str, Any]) -> float | None:
    for obj in (scheduler_job(response), scheduler_meta(response)):
        value = number_value(obj.get("cold_start_wait_ms"))
        if value is not None:
            return value
    return None


def cache_metadata(value: Any, prefix: str = "") -> dict[str, Any]:
    found: dict[str, Any] = {}
    if isinstance(value, dict):
        for key, item in value.items():
            path = f"{prefix}.{key}" if prefix else str(key)
            key_l = str(key).casefold()
            if "cache" in key_l:
                found[path] = item
            found.update(cache_metadata(item, path))
    elif isinstance(value, list):
        for idx, item in enumerate(value):
            found.update(cache_metadata(item, f"{prefix}[{idx}]"))
    return found


def scheduler_cache_stats(response: dict[str, Any]) -> dict[str, Any]:
    scope = {
        "local_scheduler": scheduler_meta(response),
        "local_scheduler_job": scheduler_job(response),
        "outputs": scheduler_outputs(response),
    }
    metadata = cache_metadata(scope)
    hit = False
    tokens = 0.0
    for path, value in metadata.items():
        path_l = path.casefold()
        if isinstance(value, bool) and "hit" in path_l and value:
            hit = True
        if any(marker in path_l for marker in ("token", "cached", "read")):
            number = number_value(value)
            if number is not None and number > 0:
                tokens += number
    return {
        "metadata": metadata,
        "metadata_count": len(metadata),
        "hit": hit,
        "tokens": tokens,
    }


def scheduler_generation_used(response: dict[str, Any]) -> bool:
    meta = scheduler_meta(response)
    return str(meta.get("task") or "") in {"kb.query.ask", "kb.answer"}


def deterministic_safety_refusal_used(response: dict[str, Any]) -> bool:
    meta = scheduler_meta(response)
    return str(meta.get("task") or "") == "local.safety.refusal"


def evaluate(args: argparse.Namespace) -> dict[str, Any]:
    manifest = load_manifest(args.manifest)
    doc_ids = profile_doc_ids(manifest, args.profile)
    queries = filtered_queries(manifest, doc_ids)
    rows = []

    for query in queries:
        start = time.perf_counter()
        try:
            chat_ms, response = post_chat(args, query["query"])
            response = maybe_poll_local_scheduler(
                args.base_url,
                response,
                token=args.token,
                request_timeout=args.timeout,
                poll_timeout=args.poll_timeout,
                poll_interval=args.poll_interval,
            )
            total_ms = (time.perf_counter() - start) * 1000
            terminal_error = response.get("local_scheduler_terminal_error")
            if not isinstance(terminal_error, dict):
                terminal_error = {}
            content = output_text(response) or str(response.get("content") or "")
            citations = response.get("citations") if isinstance(response.get("citations"), list) else []
            expected_refusal = query.get("expected_behavior") == "retrieve_for_citation_but_refuse_operational_advice"
            cite_hit = citation_hit(citations, query["acceptable_hits"], query.get("acceptable_files", []))
            term_hit = expected_term_hit(content, query.get("expect_any", []))
            refusal_hit = refuses_operational_advice(content) and not unsafe_operational_advice(content)
            answer_accuracy_hit = (cite_hit and refusal_hit) if expected_refusal else (cite_hit and term_hit)
            scheduler = scheduler_meta(response)
            scheduler_job_value = scheduler_job(response)
            cache_stats = scheduler_cache_stats(response)
            timings = scheduler_timings(response)
            usage = scheduler_usage(response)
            row = {
                "id": query["id"],
                "category": query.get("category"),
                "error": terminal_error.get("error"),
                "error_status": terminal_error.get("error_status"),
                "error_code": terminal_error.get("error_code"),
                "scheduler_error": terminal_error.get("scheduler_error"),
                "retryable": terminal_error.get("retryable"),
                "may_degrade": terminal_error.get("may_degrade"),
                "latency_ms": total_ms,
                "initial_chat_latency_ms": chat_ms,
                "citations_count": len(citations),
                "citation_hit": cite_hit,
                "answer_term_hit": term_hit,
                "answer_accuracy_hit": answer_accuracy_hit,
                "knowledge_count": response.get("knowledge_count"),
                "compression_chunks": (response.get("compression_stats") or {}).get("chunks")
                if isinstance(response.get("compression_stats"), dict)
                else None,
                "expected_refusal": expected_refusal,
                "refusal_hit": refusal_hit if expected_refusal else None,
                "unsafe_operational_advice": unsafe_operational_advice(content) if expected_refusal else False,
                "local_scheduler": response.get("local_scheduler"),
                "local_scheduler_job": response.get("local_scheduler_job"),
                "scheduler_generation_used": scheduler_generation_used(response),
                "deterministic_safety_refusal_used": deterministic_safety_refusal_used(response),
                "scheduler_task": scheduler.get("task"),
                "scheduler_scheduled_as": scheduler.get("scheduled_as"),
                "scheduler_status": scheduler_job_value.get("status") or scheduler.get("status"),
                "scheduler_model": scheduler_job_value.get("model") or scheduler.get("model"),
                "scheduler_service_class": scheduler_job_value.get("service_class") or scheduler.get("service_class"),
                "scheduler_latency_ms": scheduler_latency_ms(response),
                "scheduler_queue_wait_ms": scheduler_queue_wait_ms(response),
                "scheduler_cold_start_wait_ms": scheduler_cold_start_wait_ms(response),
                "scheduler_prompt_cache_metadata_count": cache_stats["metadata_count"],
                "scheduler_prompt_cache_hit": cache_stats["hit"],
                "scheduler_prompt_cache_tokens": cache_stats["tokens"],
                "scheduler_finish_reason": scheduler_finish_reason(response),
                "scheduler_prompt_eval_ms": timings.get("prompt_ms"),
                "scheduler_decode_ms": timings.get("predicted_ms"),
                "scheduler_prompt_tokens": timings.get("prompt_n") or usage.get("prompt_tokens"),
                "scheduler_output_tokens": timings.get("predicted_n") or usage.get("completion_tokens"),
            }
            if terminal_error:
                row["answer_accuracy_hit"] = False
            rows.append(row)
        except Exception as exc:  # noqa: BLE001 - report per-query failure.
            err = exception_error_fields(exc)
            rows.append(
                {
                    "id": query["id"],
                    "category": query.get("category"),
                    **err,
                    "latency_ms": 0.0,
                    "initial_chat_latency_ms": 0.0,
                    "citations_count": 0,
                    "citation_hit": False,
                    "answer_term_hit": False,
                    "answer_accuracy_hit": False,
                    "knowledge_count": None,
                    "compression_chunks": None,
                    "expected_refusal": query.get("expected_behavior") == "retrieve_for_citation_but_refuse_operational_advice",
                    "refusal_hit": False if query.get("expected_behavior") == "retrieve_for_citation_but_refuse_operational_advice" else None,
                    "unsafe_operational_advice": False,
                    "local_scheduler": None,
                    "local_scheduler_job": None,
                    "scheduler_generation_used": False,
                    "deterministic_safety_refusal_used": False,
                    "scheduler_task": None,
                    "scheduler_scheduled_as": None,
                    "scheduler_status": None,
                    "scheduler_model": None,
                    "scheduler_service_class": None,
                    "scheduler_latency_ms": None,
                    "scheduler_queue_wait_ms": None,
                    "scheduler_cold_start_wait_ms": None,
                    "scheduler_prompt_cache_metadata_count": 0,
                    "scheduler_prompt_cache_hit": False,
                    "scheduler_prompt_cache_tokens": 0.0,
                    "scheduler_finish_reason": None,
                    "scheduler_prompt_eval_ms": None,
                    "scheduler_decode_ms": None,
                    "scheduler_prompt_tokens": None,
                    "scheduler_output_tokens": None,
                }
            )

    latencies = [row["latency_ms"] for row in rows if not row.get("error")]
    scheduler_latencies = [
        row["scheduler_latency_ms"]
        for row in rows
        if not row.get("error") and isinstance(row.get("scheduler_latency_ms"), (int, float))
    ]
    scheduler_queue_waits = [
        row["scheduler_queue_wait_ms"]
        for row in rows
        if not row.get("error") and isinstance(row.get("scheduler_queue_wait_ms"), (int, float))
    ]
    scheduler_cold_waits = [
        row["scheduler_cold_start_wait_ms"]
        for row in rows
        if not row.get("error") and isinstance(row.get("scheduler_cold_start_wait_ms"), (int, float))
    ]
    scheduler_prompt_eval_ms = [
        row["scheduler_prompt_eval_ms"]
        for row in rows
        if not row.get("error") and isinstance(row.get("scheduler_prompt_eval_ms"), (int, float))
    ]
    scheduler_decode_ms = [
        row["scheduler_decode_ms"]
        for row in rows
        if not row.get("error") and isinstance(row.get("scheduler_decode_ms"), (int, float))
    ]
    scheduler_prompt_tokens = [
        row["scheduler_prompt_tokens"]
        for row in rows
        if not row.get("error") and isinstance(row.get("scheduler_prompt_tokens"), (int, float))
    ]
    scheduler_output_tokens = [
        row["scheduler_output_tokens"]
        for row in rows
        if not row.get("error") and isinstance(row.get("scheduler_output_tokens"), (int, float))
    ]
    citation_rows = [row for row in rows if not row.get("error") and not row.get("expected_refusal")]
    safety_rows = [row for row in rows if row.get("expected_refusal")]
    successful_rows = [row for row in rows if not row.get("error")]
    generation_required_rows = [row for row in successful_rows if not row.get("expected_refusal")]
    scheduler_generation_rows = [row for row in successful_rows if row.get("scheduler_generation_used")]
    prompt_cache_metadata_rows = [
        row for row in scheduler_generation_rows if row.get("scheduler_prompt_cache_metadata_count", 0) > 0
    ]
    summary = {
        "manifest": str(args.manifest),
        "profile": args.profile,
        "queries": len(rows),
        "errors": sum(1 for row in rows if row.get("error")),
        "error_codes": sorted(
            {
                str(row.get("error_code"))
                for row in rows
                if row.get("error") and row.get("error_code")
            }
        ),
        "citation_hit_rate": (
            statistics.fmean(1.0 if row["citation_hit"] else 0.0 for row in citation_rows)
            if citation_rows
            else 0.0
        ),
        "answer_accuracy_rate": (
            statistics.fmean(1.0 if row["answer_accuracy_hit"] else 0.0 for row in rows)
            if rows
            else 0.0
        ),
        "answer_term_hit_rate": (
            statistics.fmean(1.0 if row["answer_term_hit"] else 0.0 for row in citation_rows)
            if citation_rows
            else 0.0
        ),
        "unsafe_operational_advice_rate": (
            statistics.fmean(1.0 if row["unsafe_operational_advice"] else 0.0 for row in safety_rows)
            if safety_rows
            else 0.0
        ),
        "latency_ms": {
            "p50": percentile(latencies, 50),
            "p95": percentile(latencies, 95),
            "max": max(latencies) if latencies else 0.0,
        },
        "scheduler_generation": {
            "rows": len(scheduler_generation_rows),
            "required_rows": len(generation_required_rows),
            "coverage_rate": (
                sum(1 for row in generation_required_rows if row.get("scheduler_generation_used"))
                / len(generation_required_rows)
                if generation_required_rows
                else 0.0
            ),
            "async_rows": sum(1 for row in scheduler_generation_rows if row.get("scheduler_scheduled_as") == "async"),
            "deterministic_safety_refusal_rows": sum(
                1 for row in successful_rows if row.get("deterministic_safety_refusal_used")
            ),
            "latency_ms": {
                "p50": percentile(scheduler_latencies, 50),
                "p95": percentile(scheduler_latencies, 95),
                "max": max(scheduler_latencies) if scheduler_latencies else 0.0,
            },
            "queue_wait_ms": {
                "p50": percentile(scheduler_queue_waits, 50),
                "p95": percentile(scheduler_queue_waits, 95),
                "max": max(scheduler_queue_waits) if scheduler_queue_waits else 0.0,
            },
            "cold_start_wait_ms": {
                "p50": percentile(scheduler_cold_waits, 50),
                "p95": percentile(scheduler_cold_waits, 95),
                "max": max(scheduler_cold_waits) if scheduler_cold_waits else 0.0,
            },
            "prompt_eval_ms": {
                "p50": percentile(scheduler_prompt_eval_ms, 50),
                "p95": percentile(scheduler_prompt_eval_ms, 95),
                "max": max(scheduler_prompt_eval_ms) if scheduler_prompt_eval_ms else 0.0,
            },
            "decode_ms": {
                "p50": percentile(scheduler_decode_ms, 50),
                "p95": percentile(scheduler_decode_ms, 95),
                "max": max(scheduler_decode_ms) if scheduler_decode_ms else 0.0,
            },
            "prompt_tokens": {
                "p50": percentile(scheduler_prompt_tokens, 50),
                "p95": percentile(scheduler_prompt_tokens, 95),
                "max": max(scheduler_prompt_tokens) if scheduler_prompt_tokens else 0.0,
            },
            "output_tokens": {
                "p50": percentile(scheduler_output_tokens, 50),
                "p95": percentile(scheduler_output_tokens, 95),
                "max": max(scheduler_output_tokens) if scheduler_output_tokens else 0.0,
            },
            "prompt_cache_metadata_rows": len(prompt_cache_metadata_rows),
            "prompt_cache_metadata_rate": (
                len(prompt_cache_metadata_rows) / len(scheduler_generation_rows)
                if scheduler_generation_rows
                else 0.0
            ),
            "prompt_cache_hits": sum(1 for row in scheduler_generation_rows if row.get("scheduler_prompt_cache_hit")),
            "prompt_cache_tokens": sum(
                float(row.get("scheduler_prompt_cache_tokens") or 0.0)
                for row in scheduler_generation_rows
            ),
            "finish_reason_length_rows": sum(
                1 for row in scheduler_generation_rows if row.get("scheduler_finish_reason") == "length"
            ),
        },
        "max_knowledge_count": max(
            [row["knowledge_count"] for row in rows if isinstance(row.get("knowledge_count"), int)] or [0]
        ),
        "max_compression_chunks": max(
            [row["compression_chunks"] for row in rows if isinstance(row.get("compression_chunks"), int)] or [0]
        ),
    }
    return {"summary": summary, "rows": rows}


def check_targets(result: dict[str, Any], manifest: dict[str, Any]) -> bool:
    targets = manifest.get("evaluation_targets", {})
    answer = targets.get("rag_answer", {})
    context = targets.get("context_admission", {})
    summary = result["summary"]
    scheduler_generation = summary.get("scheduler_generation", {})
    failures = []
    if summary["answer_accuracy_rate"] < answer.get("answer_accuracy_rate_min", 0.0):
        failures.append(f"answer_accuracy_rate {summary['answer_accuracy_rate']:.3f} below target")
    if summary["citation_hit_rate"] < answer.get("citation_hit_rate_min", 0.0):
        failures.append(f"citation_hit_rate {summary['citation_hit_rate']:.3f} below target")
    if summary["unsafe_operational_advice_rate"] > answer.get("unsafe_operational_advice_rate_max", 1.0):
        failures.append("unsafe operational advice detected")
    if summary["latency_ms"]["p95"] > aliased_target_value(
        answer,
        "edge_scheduler_30b_p95_latency_ms_max",
        float("inf"),
    ):
        failures.append(f"p95 latency {summary['latency_ms']['p95']:.1f}ms above target")
    if summary["max_compression_chunks"] > aliased_target_value(
        context,
        "edge_scheduler_30b_max_final_chunks",
        float("inf"),
    ):
        failures.append(f"max compression chunks {summary['max_compression_chunks']} above target")
    args = result.get("_args", {})
    if args.get("require_scheduler_generation") and scheduler_generation.get("coverage_rate", 0.0) < 1.0:
        failures.append(
            f"scheduler generation coverage {scheduler_generation.get('coverage_rate', 0.0):.3f} below 1.000"
        )
    if args.get("require_prompt_cache_metadata") and scheduler_generation.get("prompt_cache_metadata_rate", 0.0) < 1.0:
        failures.append(
            "scheduler prompt-cache metadata missing from one or more generation rows"
        )
    scheduler_p95_max = args.get("scheduler_generation_p95_ms_max")
    if scheduler_p95_max is not None:
        observed = scheduler_generation.get("latency_ms", {}).get("p95", 0.0)
        if observed > scheduler_p95_max:
            failures.append(
                f"scheduler generation p95 latency {observed:.1f}ms above target {scheduler_p95_max:.1f}ms"
            )
    if failures:
        print("target failures:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return False
    return True


def main() -> int:
    args = parse_args()
    result = evaluate(args)
    result["_args"] = {
        "require_scheduler_generation": args.require_scheduler_generation,
        "require_prompt_cache_metadata": args.require_prompt_cache_metadata,
        "scheduler_generation_p95_ms_max": args.scheduler_generation_p95_ms_max,
    }
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
