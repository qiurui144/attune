#!/usr/bin/env python3
"""Run all standard long-text GitHub corpora E2E gates.

This wrapper keeps airplane manuals and the mechanical-design handbook as
parallel benchmark corpora while preserving the existing single-corpus runners.
"""
from __future__ import annotations

import os
import shlex
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]


@dataclass(frozen=True)
class CorpusConfig:
    name: str
    env_key: str
    script: Path
    corpus_dir: str
    manifest_template: str
    golden_template: str


CORPORA = {
    "airplane": CorpusConfig(
        name="airplane",
        env_key="AIRPLANE",
        script=REPO_ROOT / "tests/e2e/airplane_manual_longtext_e2e.py",
        corpus_dir="~/attune-e2e-corpora/airplane-manual-collection",
        manifest_template="/tmp/attune-airplane-longtext-{profile}.json",
        golden_template="/tmp/attune-airplane-longtext-{profile}-golden.json",
    ),
    "mechanical_design": CorpusConfig(
        name="mechanical_design",
        env_key="MECHANICAL_DESIGN",
        script=REPO_ROOT / "tests/e2e/mechanical_design_longtext_e2e.py",
        corpus_dir="~/attune-e2e-corpora/handbook-of-mechanical-design",
        manifest_template="/tmp/attune-mechanical-design-longtext-{profile}.json",
        golden_template="/tmp/attune-mechanical-design-longtext-{profile}-golden.json",
    ),
}


def parse_corpora(raw: str) -> list[CorpusConfig]:
    names = [name.strip() for name in raw.split(",") if name.strip()]
    if not names:
        raise SystemExit("ATTUNE_LONGTEXT_CORPORA selected no corpora")
    unknown = [name for name in names if name not in CORPORA]
    if unknown:
        raise SystemExit(f"unknown long-text corpora: {', '.join(unknown)}")
    return [CORPORA[name] for name in names]


def env_bool(name: str, default: bool) -> bool:
    raw = os.environ.get(name)
    if raw is None:
        return default
    return raw.strip().lower() in {"1", "true", "yes", "on"}


def quote_cmd(cmd: list[str]) -> str:
    return " ".join(shlex.quote(part) for part in cmd)


def child_env(config: CorpusConfig, profile: str, multiple: bool) -> dict[str, str]:
    env = os.environ.copy()
    if multiple:
        for key in (
            "ATTUNE_LONGTEXT_CORPUS_DIR",
            "ATTUNE_LONGTEXT_MANIFEST",
            "ATTUNE_LONGTEXT_GOLDEN",
            "ATTUNE_LONGTEXT_BIND_DIR",
        ):
            env.pop(key, None)

    prefix = f"ATTUNE_{config.env_key}_LONGTEXT"
    env["ATTUNE_LONGTEXT_CORPUS_DIR"] = os.environ.get(
        f"{prefix}_CORPUS_DIR",
        config.corpus_dir,
    )
    env["ATTUNE_LONGTEXT_MANIFEST"] = os.environ.get(
        f"{prefix}_MANIFEST",
        config.manifest_template.format(profile=profile),
    )
    env["ATTUNE_LONGTEXT_GOLDEN"] = os.environ.get(
        f"{prefix}_GOLDEN",
        config.golden_template.format(profile=profile),
    )
    bind_dir = os.environ.get(f"{prefix}_BIND_DIR")
    if bind_dir:
        env["ATTUNE_LONGTEXT_BIND_DIR"] = bind_dir
    return env


def run_corpus(config: CorpusConfig, profile: str, multiple: bool) -> int:
    cmd = [sys.executable, str(config.script)]
    print(f"=== longtext corpus {config.name} ===", flush=True)
    print(f"+ {quote_cmd(cmd)}", flush=True)
    proc = subprocess.run(cmd, cwd=REPO_ROOT, env=child_env(config, profile, multiple), check=False)
    if proc.returncode != 0:
        print(f"=== longtext corpus {config.name} FAIL rc={proc.returncode} ===")
    return proc.returncode


def main() -> int:
    profile = os.environ.get("ATTUNE_LONGTEXT_PROFILE", "edge_scheduler_comprehensive")
    corpora = parse_corpora(os.environ.get("ATTUNE_LONGTEXT_CORPORA", "airplane,mechanical_design"))
    dry_run = env_bool("ATTUNE_LONGTEXT_DRY_RUN", False)
    print("=== standard GitHub longtext corpora E2E ===", flush=True)
    print(f"[longtext-corpora] profile={profile}", flush=True)
    print(f"[longtext-corpora] corpora={','.join(config.name for config in corpora)}", flush=True)
    print(f"[longtext-corpora] dry_run={1 if dry_run else 0}", flush=True)

    failures = 0
    for config in corpora:
        if run_corpus(config, profile, multiple=len(corpora) > 1) != 0:
            failures += 1

    if failures:
        print(f"=== standard GitHub longtext corpora E2E FAIL failures={failures} ===")
        return 1
    status = "DRY RUN PASS" if dry_run else "PASS"
    print(f"=== standard GitHub longtext corpora E2E {status} ===")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
