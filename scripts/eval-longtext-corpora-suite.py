#!/usr/bin/env python3
"""Repeat long-text search/chat/multiturn gates across standard corpora.

Run this after the corpora have been materialized, bound, and embedded by
tests/e2e/longtext_corpora_e2e.py or the single-corpus E2E runners.
"""
from __future__ import annotations

import argparse
import json
import os
import shlex
import statistics
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parent.parent


@dataclass(frozen=True)
class CorpusConfig:
    name: str
    env_key: str
    result_prefix: str
    default_manifest_template: str
    repo_manifest: Path


CORPORA = {
    "airplane": CorpusConfig(
        name="airplane",
        env_key="AIRPLANE",
        result_prefix="attune-airplane-longtext",
        default_manifest_template="/tmp/attune-airplane-longtext-{profile}.json",
        repo_manifest=REPO_ROOT / "tests/e2e/airplane_manual_longtext_cases.json",
    ),
    "mechanical_design": CorpusConfig(
        name="mechanical_design",
        env_key="MECHANICAL_DESIGN",
        result_prefix="attune-mechanical-design-longtext",
        default_manifest_template="/tmp/attune-mechanical-design-longtext-{profile}.json",
        repo_manifest=REPO_ROOT / "tests/e2e/mechanical_design_longtext_cases.json",
    ),
}


def env_bool(name: str, default: bool) -> bool:
    raw = os.environ.get(name)
    if raw is None:
        return default
    return raw.strip().lower() in {"1", "true", "yes", "on"}


def env_int(name: str, default: int) -> int:
    raw = os.environ.get(name)
    if raw is None or raw.strip() == "":
        return default
    return int(raw)


def parse_corpora(raw: str) -> list[CorpusConfig]:
    names = [name.strip() for name in raw.split(",") if name.strip()]
    if not names:
        raise SystemExit("selected no corpora")
    unknown = [name for name in names if name not in CORPORA]
    if unknown:
        raise SystemExit(f"unknown corpora: {', '.join(unknown)}")
    return [CORPORA[name] for name in names]


def quote_cmd(cmd: list[str]) -> str:
    return " ".join(shlex.quote(part) for part in cmd)


def manifest_path(config: CorpusConfig, profile: str, dry_run: bool) -> Path:
    env_name = f"ATTUNE_{config.env_key}_LONGTEXT_MANIFEST"
    if os.environ.get(env_name):
        return Path(os.environ[env_name])
    default_tmp = Path(config.default_manifest_template.format(profile=profile))
    if dry_run or default_tmp.exists():
        return default_tmp
    return config.repo_manifest


def result_path(result_dir: Path, config: CorpusConfig, profile: str, gate: str) -> Path:
    return result_dir / f"{config.result_prefix}-{profile}-{gate}.json"


def gate_command(
    args: argparse.Namespace,
    config: CorpusConfig,
    manifest: Path,
    gate: str,
    out: Path,
) -> list[str]:
    if gate == "search":
        return [
            sys.executable,
            str(REPO_ROOT / "scripts/eval-airplane-manual-longtext-search.py"),
            "--manifest",
            str(manifest),
            "--base-url",
            args.base_url,
            "--profile",
            args.profile,
            "--limit",
            str(args.search_limit),
            "--out",
            str(out),
        ] + (["--token", args.token] if args.token else [])

    if gate.startswith("chat-repeat-"):
        cmd = [
            sys.executable,
            str(REPO_ROOT / "scripts/eval-airplane-manual-longtext-chat.py"),
            "--manifest",
            str(manifest),
            "--base-url",
            args.base_url,
            "--profile",
            args.profile,
            "--timeout",
            str(args.chat_timeout),
            "--poll-timeout",
            str(args.chat_poll_timeout),
            "--poll-interval",
            str(args.chat_poll_interval),
            "--out",
            str(out),
        ]
        if args.token:
            cmd.extend(["--token", args.token])
        if args.require_scheduler_generation:
            cmd.append("--require-scheduler-generation")
        if args.require_prompt_cache_metadata:
            cmd.append("--require-prompt-cache-metadata")
        if args.require_answer_budget_metadata:
            cmd.append("--require-answer-budget-metadata")
        if args.scheduler_generation_p95_ms_max is not None:
            cmd.extend(["--scheduler-generation-p95-ms-max", str(args.scheduler_generation_p95_ms_max)])
        return cmd

    if gate.startswith("multiturn-repeat-"):
        cmd = [
            sys.executable,
            str(REPO_ROOT / "scripts/eval-airplane-manual-longtext-multiturn.py"),
            "--manifest",
            str(manifest),
            "--base-url",
            args.base_url,
            "--profile",
            args.profile,
            "--timeout",
            str(args.chat_timeout),
            "--poll-timeout",
            str(args.chat_poll_timeout),
            "--poll-interval",
            str(args.chat_poll_interval),
            "--out",
            str(out),
        ]
        if args.token:
            cmd.extend(["--token", args.token])
        return cmd

    raise ValueError(f"unknown gate: {gate}")


def run_gate(
    args: argparse.Namespace,
    config: CorpusConfig,
    manifest: Path,
    gate: str,
    out: Path,
) -> dict[str, Any] | None:
    cmd = gate_command(args, config, manifest, gate, out)
    if args.fail_on_targets and "--fail-on-targets" not in cmd:
        cmd.append("--fail-on-targets")
    print(f"[longtext-suite] {config.name} {gate}")
    print(f"+ {quote_cmd(cmd)}")
    if args.dry_run:
        return None
    subprocess.run(cmd, cwd=REPO_ROOT, check=True, timeout=args.eval_timeout)
    return json.loads(out.read_text(encoding="utf-8"))


def number(value: Any) -> float | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    return None


def aggregate(records: list[dict[str, Any]]) -> dict[str, Any]:
    by_corpus: dict[str, list[dict[str, Any]]] = {}
    for record in records:
        by_corpus.setdefault(record["corpus"], []).append(record)

    corpora: dict[str, Any] = {}
    for corpus, rows in by_corpus.items():
        errors = 0
        units = 0
        p95_samples: list[float] = []
        p50_samples: list[float] = []
        for row in rows:
            summary = row.get("summary") or {}
            errors += int(summary.get("errors") or 0)
            units += int(summary.get("queries") or summary.get("turns") or 0)
            latency = summary.get("latency_ms") if isinstance(summary.get("latency_ms"), dict) else {}
            p95 = number(latency.get("p95"))
            p50 = number(latency.get("p50"))
            if p95 is not None:
                p95_samples.append(p95)
            if p50 is not None:
                p50_samples.append(p50)
        corpora[corpus] = {
            "runs": len(rows),
            "units": units,
            "errors": errors,
            "terminal_error_rate": errors / units if units else 0.0,
            "latency_p50_median_ms": statistics.median(p50_samples) if p50_samples else 0.0,
            "latency_p95_median_ms": statistics.median(p95_samples) if p95_samples else 0.0,
            "latency_p95_max_ms": max(p95_samples) if p95_samples else 0.0,
        }
    return corpora


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--token", default=os.environ.get("ATTUNE_TOKEN", ""))
    parser.add_argument("--profile", default=os.environ.get("ATTUNE_LONGTEXT_PROFILE", "edge_scheduler_comprehensive"))
    parser.add_argument("--corpora", default=os.environ.get("ATTUNE_LONGTEXT_CORPORA", "airplane,mechanical_design"))
    parser.add_argument("--results-dir", type=Path, default=Path(os.environ.get("ATTUNE_LONGTEXT_RESULTS_DIR", "/tmp")))
    parser.add_argument("--repeat-chat", type=int, default=env_int("ATTUNE_LONGTEXT_REPEAT_CHAT", 3))
    parser.add_argument(
        "--repeat-multiturn",
        type=int,
        default=env_int("ATTUNE_LONGTEXT_REPEAT_MULTITURN", env_int("ATTUNE_LONGTEXT_REPEAT_CHAT", 3)),
    )
    parser.add_argument("--search-limit", type=int, default=env_int("ATTUNE_LONGTEXT_SEARCH_LIMIT", 10))
    parser.add_argument("--chat-timeout", type=float, default=float(os.environ.get("ATTUNE_LONGTEXT_CHAT_TIMEOUT_SEC", "120")))
    parser.add_argument(
        "--chat-poll-timeout",
        type=float,
        default=float(os.environ.get("ATTUNE_LONGTEXT_CHAT_POLL_TIMEOUT_SEC", "180")),
    )
    parser.add_argument(
        "--chat-poll-interval",
        type=float,
        default=float(os.environ.get("ATTUNE_LONGTEXT_CHAT_POLL_INTERVAL_SEC", "0.25")),
    )
    parser.add_argument("--eval-timeout", type=int, default=env_int("ATTUNE_LONGTEXT_EVAL_TIMEOUT_SEC", 3600))
    parser.add_argument("--out", type=Path, default=None)
    parser.add_argument("--dry-run", action="store_true", default=env_bool("ATTUNE_LONGTEXT_DRY_RUN", False))
    parser.add_argument("--fail-on-targets", action="store_true", default=env_bool("ATTUNE_LONGTEXT_FAIL_ON_TARGETS", False))
    parser.add_argument(
        "--require-scheduler-generation",
        action="store_true",
        default=env_bool("ATTUNE_LONGTEXT_REQUIRE_SCHEDULER_GENERATION", False),
    )
    parser.add_argument(
        "--require-prompt-cache-metadata",
        action="store_true",
        default=env_bool("ATTUNE_LONGTEXT_REQUIRE_PROMPT_CACHE_METADATA", False),
    )
    parser.add_argument(
        "--require-answer-budget-metadata",
        action="store_true",
        default=env_bool("ATTUNE_LONGTEXT_REQUIRE_ANSWER_BUDGET_METADATA", False),
    )
    scheduler_p95 = os.environ.get("ATTUNE_LONGTEXT_SCHEDULER_GENERATION_P95_MS_MAX", "").strip()
    parser.add_argument(
        "--scheduler-generation-p95-ms-max",
        type=float,
        default=float(scheduler_p95) if scheduler_p95 else None,
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    corpora = parse_corpora(args.corpora)
    args.results_dir.mkdir(parents=True, exist_ok=True)
    out = args.out or args.results_dir / f"attune-longtext-corpora-{args.profile}-suite-summary.json"

    records: list[dict[str, Any]] = []
    planned: list[dict[str, str]] = []
    print("=== longtext corpora repeat suite ===")
    print(f"[longtext-suite] profile={args.profile}")
    print(f"[longtext-suite] corpora={','.join(config.name for config in corpora)}")
    print(f"[longtext-suite] repeat_chat={args.repeat_chat} repeat_multiturn={args.repeat_multiturn}")
    print(f"[longtext-suite] dry_run={1 if args.dry_run else 0}")

    for config in corpora:
        manifest = manifest_path(config, args.profile, args.dry_run)
        if not args.dry_run and not manifest.exists():
            raise SystemExit(f"manifest for {config.name} does not exist: {manifest}")
        gates = ["search"]
        gates.extend(f"chat-repeat-{idx:02d}" for idx in range(1, args.repeat_chat + 1))
        gates.extend(f"multiturn-repeat-{idx:02d}" for idx in range(1, args.repeat_multiturn + 1))
        for gate in gates:
            out_path = result_path(args.results_dir, config, args.profile, gate)
            if args.dry_run:
                planned.append({"corpus": config.name, "gate": gate, "out": str(out_path)})
            result = run_gate(args, config, manifest, gate, out_path)
            if result is not None:
                records.append(
                    {
                        "corpus": config.name,
                        "gate": gate,
                        "out": str(out_path),
                        "summary": result.get("summary", {}),
                    }
                )

    summary = {
        "profile": args.profile,
        "corpora": [config.name for config in corpora],
        "repeat_chat": args.repeat_chat,
        "repeat_multiturn": args.repeat_multiturn,
        "dry_run": args.dry_run,
        "planned": planned,
        "records": records,
        "aggregate": aggregate(records) if records else {},
    }
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary["aggregate"] if records else {"planned": planned}, ensure_ascii=False, indent=2))
    print(f"[longtext-suite] wrote {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
