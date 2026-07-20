#!/usr/bin/env python3
"""Mechanical-design handbook long-text KB end-to-end gate.

Flow:
  1. Materialize the selected handbook volumes under the server user's HOME.
  2. Build a manifest that points at that local corpus.
  3. Ask Attune to build the vector DB with POST /api/v1/index/bind.
  4. Wait for embeddings to drain.
  5. Run vector-search, repeated chat-capable single-turn, and multi-turn gates.

This is opt-in because the source repository uses Git LFS PDFs and full OCR can
be expensive on edge hardware.
"""
from __future__ import annotations

import os
import shutil
import sys
import time
import urllib.parse
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT / "tests/e2e"))

import airplane_manual_longtext_e2e as shared_runner  # noqa: E402
from airplane_longtext_support import (  # noqa: E402
    PROFILE_ALIASES,
    load_manifest,
    profile_doc_ids as support_profile_doc_ids,
    request_json as support_request_json,
    resolve_profile_name,
)

BASE_URL = os.environ.get("ATTUNE_BASE_URL", "http://localhost:18905").rstrip("/")
PASSWORD = os.environ.get("ATTUNE_E2E_PASSWORD", "e2e-pass-2026")
PROFILE_LIMITS = {
    "smoke": 2,
    "edge_scheduler_30b": 5,
    "edge_scheduler_comprehensive": 5,
    "stress": 5,
}


env_bool = shared_runner.env_bool
env_int = shared_runner.env_int
quote_cmd = shared_runner.quote_cmd
run_cmd = shared_runner.run_cmd
ensure_under_home = shared_runner.ensure_under_home
setup_and_unlock = shared_runner.setup_and_unlock
run_background_bind_ux_gate = shared_runner.run_background_bind_ux_gate
wait_for_embeddings = shared_runner.wait_for_embeddings
json_compact = shared_runner.json_compact


def request_json(
    method: str,
    path: str,
    body: dict[str, Any] | None = None,
    token: str = "",
    timeout: float = 60.0,
    allow_statuses: set[int] | None = None,
) -> tuple[int, dict[str, Any]]:
    return support_request_json(
        BASE_URL,
        method,
        path,
        body=body,
        token=token,
        timeout=timeout,
        allow_statuses=allow_statuses,
    )


def build_manifest(profile: str, corpus_dir: Path, manifest: Path, golden: Path, dry_run: bool) -> None:
    if profile not in PROFILE_LIMITS and profile != "all":
        if not any(candidate in PROFILE_LIMITS for candidate in PROFILE_ALIASES.get(profile, ())):
            raise SystemExit(f"unknown ATTUNE_LONGTEXT_PROFILE={profile!r}")
    default_limit = 5 if profile == "all" else profile_limit(profile)
    limit = env_int("ATTUNE_LONGTEXT_LIMIT_DOCS", default_limit)

    cmd = [
        sys.executable,
        str(REPO_ROOT / "scripts/build-mechanical-design-longtext-dataset.py"),
        "--repo-dir",
        str(corpus_dir),
        "--out",
        str(manifest),
        "--golden-out",
        str(golden),
        "--limit-docs",
        str(limit),
        "--no-github-api",
    ]
    if env_bool("ATTUNE_LONGTEXT_MATERIALIZE", True):
        cmd.append("--materialize")
    run_cmd(cmd, env_int("ATTUNE_LONGTEXT_MATERIALIZE_TIMEOUT_SEC", 7200), dry_run)


def profile_limit(profile: str) -> int:
    if profile in PROFILE_LIMITS:
        return PROFILE_LIMITS[profile]
    for candidate in PROFILE_ALIASES.get(profile, ()):
        if candidate in PROFILE_LIMITS:
            return PROFILE_LIMITS[candidate]
    return 5


def profile_doc_ids(manifest: dict[str, Any], profile: str) -> list[str]:
    return sorted(support_profile_doc_ids(manifest, profile))


def is_lfs_pointer(path: Path) -> bool:
    if not path.is_file() or path.stat().st_size > 1024:
        return False
    try:
        head = path.read_text(encoding="utf-8", errors="ignore")[:256]
    except OSError:
        return False
    return head.startswith("version https://git-lfs.github.com/spec/v1")


def verify_selected_files(manifest_path: Path, profile: str) -> tuple[int, int]:
    manifest = load_manifest(manifest_path)
    source_root = Path(manifest["source_root"])
    docs = {doc["id"]: doc for doc in manifest.get("documents", [])}
    missing: list[Path] = []
    pointers: list[Path] = []
    total_bytes = 0
    for doc_id in profile_doc_ids(manifest, profile):
        doc = docs[doc_id]
        path = source_root / doc["file"]
        if not path.exists():
            missing.append(path)
        elif is_lfs_pointer(path):
            pointers.append(path)
        elif path.is_file():
            total_bytes += path.stat().st_size
    if missing or pointers:
        details = []
        if missing:
            details.append(f"missing {len(missing)} files: {', '.join(str(p) for p in missing[:3])}")
        if pointers:
            details.append(
                f"still Git LFS pointers {len(pointers)} files: {', '.join(str(p) for p in pointers[:3])}"
            )
        raise SystemExit("selected corpus is not materialized; " + "; ".join(details))
    return len(profile_doc_ids(manifest, profile)), total_bytes


def prepare_profile_corpus_view(manifest_path: Path, profile: str, corpus_dir: Path) -> Path:
    if env_bool("ATTUNE_LONGTEXT_BIND_FULL_CORPUS", False):
        print("[longtext] binding full mechanical-design corpus directory")
        return corpus_dir

    manifest = load_manifest(manifest_path)
    source_root = Path(manifest["source_root"])
    docs = {doc["id"]: doc for doc in manifest.get("documents", [])}
    view_root = Path(
        os.environ.get(
            "ATTUNE_LONGTEXT_BIND_DIR",
            f"~/attune-e2e-corpora/handbook-of-mechanical-design-{profile}-view",
        )
    ).expanduser()
    view_root = ensure_under_home(view_root)
    if view_root.exists():
        shutil.rmtree(view_root)
    view_root.mkdir(parents=True, exist_ok=True)

    linked = 0
    for doc_id in profile_doc_ids(manifest, profile):
        doc = docs[doc_id]
        src = source_root / doc["file"]
        dst = view_root / doc["file"]
        dst.parent.mkdir(parents=True, exist_ok=True)
        try:
            os.link(src, dst)
        except OSError:
            os.symlink(src, dst)
        linked += 1
    print(f"[longtext] mechanical-design profile bind view={view_root} files={linked}")
    return view_root


def corpus_is_bound(corpus_dir: Path, token: str) -> bool:
    try:
        _, status = request_json("GET", "/api/v1/index/status", token=token, timeout=30)
    except Exception:
        return False
    wanted = corpus_dir.expanduser().resolve(strict=False)
    for item in status.get("directories", []):
        raw = item.get("path")
        if not isinstance(raw, str):
            continue
        if Path(raw).expanduser().resolve(strict=False) == wanted:
            return True
    return False


def wait_for_background_scan(token: str, dir_id: str, timeout: int) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    task_id = f"bind-scan-{dir_id}"
    tick = 0
    last: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        _, status = request_json("GET", "/api/v1/index/status", token=token, timeout=30)
        scans = status.get("background_scans", [])
        if not isinstance(scans, list):
            raise SystemExit(f"index status background_scans is not a list: {status}")
        task = None
        for candidate in scans:
            if not isinstance(candidate, dict):
                continue
            if candidate.get("dir_id") == dir_id or candidate.get("task_id") == task_id:
                task = candidate
                break
        if task:
            last = task
            state = task.get("status")
            if state == "done":
                return task
            if state == "failed":
                raise SystemExit(f"background bind scan failed: {task}")
        tick += 1
        if tick == 1 or tick % 30 == 0:
            print(f"[longtext] waiting for mechanical-design bind scan: dir_id={dir_id} last={json_compact(last or {})}")
        time.sleep(2)
    raise SystemExit(f"background bind scan did not finish within {timeout}s; last={last}")


def bind_corpus(corpus_dir: Path, token: str) -> None:
    background = env_bool("ATTUNE_LONGTEXT_BIND_BACKGROUND", True)
    body = {
        "path": str(corpus_dir),
        "recursive": True,
        "file_types": ["pdf"],
        "corpus_domain": "mechanical_design",
        "background": background,
    }
    timeout = env_int("ATTUNE_LONGTEXT_BIND_TIMEOUT_SEC", 7200)
    mode = "background" if background else "synchronous"
    print(f"[longtext] binding mechanical-design corpus via /api/v1/index/bind mode={mode} timeout={timeout}s")
    try:
        _, data = request_json(
            "POST",
            "/api/v1/index/bind",
            body,
            token=token,
            timeout=30 if background else timeout,
        )
    except Exception as exc:
        text = str(exc).lower()
        if "already" in text or "已绑定" in text:
            print("[longtext] mechanical-design corpus already bound; continuing")
            return
        if corpus_is_bound(corpus_dir, token):
            print(f"[longtext] bind request ended with {exc!r}, but corpus is bound; continuing")
            return
        raise
    if background:
        if data.get("status") != "accepted" or data.get("background") is not True:
            raise SystemExit(f"background corpus bind did not return accepted/background response: {data}")
        dir_id = str(data.get("dir_id") or "")
        if not dir_id:
            raise SystemExit(f"background corpus bind response missing dir_id: {data}")
        scan = wait_for_background_scan(token, dir_id, timeout)
        total = scan.get("total", 0)
        print(
            "[longtext] mechanical-design background bind scan: "
            f"total={total} new={scan.get('new')} updated={scan.get('updated')} "
            f"skipped={scan.get('skipped')} degraded={scan.get('degraded')} "
            f"errors={scan.get('errors')} elapsed_ms={scan.get('elapsed_ms')}"
        )
        if not isinstance(total, int) or total <= 0:
            raise SystemExit(f"background bind scan found no files: {scan}")
        return
    scan = data.get("scan", {})
    total = scan.get("total", 0)
    print(
        f"[longtext] mechanical-design bind scan: total={total} "
        f"new={scan.get('new')} updated={scan.get('updated')} skipped={scan.get('skipped')}"
    )
    if not isinstance(total, int) or total <= 0:
        raise SystemExit(f"bind scan found no files: {data}")


def run_gates(profile: str, manifest: Path, token: str, dry_run: bool) -> None:
    result_dir = Path(os.environ.get("ATTUNE_LONGTEXT_RESULTS_DIR", "/tmp")).expanduser()
    result_dir.mkdir(parents=True, exist_ok=True)
    fail_targets = env_bool("ATTUNE_LONGTEXT_FAIL_ON_TARGETS", True)
    timeout = env_int("ATTUNE_LONGTEXT_EVAL_TIMEOUT_SEC", 3600)

    search_cmd = [
        sys.executable,
        str(REPO_ROOT / "scripts/eval-airplane-manual-longtext-search.py"),
        "--manifest",
        str(manifest),
        "--base-url",
        BASE_URL,
        "--profile",
        profile,
        "--limit",
        os.environ.get("ATTUNE_LONGTEXT_SEARCH_LIMIT", "10"),
        "--out",
        str(result_dir / f"attune-mechanical-design-longtext-{profile}-search.json"),
    ]
    if token:
        search_cmd.extend(["--token", token])
    if fail_targets:
        search_cmd.append("--fail-on-targets")
    run_cmd(search_cmd, timeout, dry_run)

    chat_cmd = [
        sys.executable,
        str(REPO_ROOT / "scripts/eval-airplane-manual-longtext-chat.py"),
        "--manifest",
        str(manifest),
        "--base-url",
        BASE_URL,
        "--profile",
        profile,
        "--timeout",
        os.environ.get("ATTUNE_LONGTEXT_CHAT_TIMEOUT_SEC", "120"),
        "--poll-timeout",
        os.environ.get("ATTUNE_LONGTEXT_CHAT_POLL_TIMEOUT_SEC", "180"),
        "--poll-interval",
        os.environ.get("ATTUNE_LONGTEXT_CHAT_POLL_INTERVAL_SEC", "0.25"),
        "--out",
        str(result_dir / f"attune-mechanical-design-longtext-{profile}-chat.json"),
    ]
    if token:
        chat_cmd.extend(["--token", token])
    if env_bool("ATTUNE_LONGTEXT_REQUIRE_SCHEDULER_GENERATION", False):
        chat_cmd.append("--require-scheduler-generation")
    if env_bool("ATTUNE_LONGTEXT_REQUIRE_PROMPT_CACHE_METADATA", False):
        chat_cmd.append("--require-prompt-cache-metadata")
    if env_bool("ATTUNE_LONGTEXT_REQUIRE_ANSWER_BUDGET_METADATA", False):
        chat_cmd.append("--require-answer-budget-metadata")
    scheduler_p95 = os.environ.get("ATTUNE_LONGTEXT_SCHEDULER_GENERATION_P95_MS_MAX", "").strip()
    if scheduler_p95:
        chat_cmd.extend(["--scheduler-generation-p95-ms-max", scheduler_p95])
    if fail_targets:
        chat_cmd.append("--fail-on-targets")
    run_cmd(chat_cmd, timeout, dry_run)

    if env_bool("ATTUNE_LONGTEXT_MULTITURN", True):
        multiturn_cmd = [
            sys.executable,
            str(REPO_ROOT / "scripts/eval-airplane-manual-longtext-multiturn.py"),
            "--manifest",
            str(manifest),
            "--base-url",
            BASE_URL,
            "--profile",
            profile,
            "--timeout",
            os.environ.get("ATTUNE_LONGTEXT_CHAT_TIMEOUT_SEC", "120"),
            "--poll-timeout",
            os.environ.get("ATTUNE_LONGTEXT_CHAT_POLL_TIMEOUT_SEC", "180"),
            "--poll-interval",
            os.environ.get("ATTUNE_LONGTEXT_CHAT_POLL_INTERVAL_SEC", "0.25"),
            "--out",
            str(result_dir / f"attune-mechanical-design-longtext-{profile}-multiturn.json"),
        ]
        if token:
            multiturn_cmd.extend(["--token", token])
        query_id = os.environ.get("ATTUNE_LONGTEXT_MULTITURN_QUERY_ID", "").strip()
        if query_id:
            multiturn_cmd.extend(["--query-id", query_id])
        if fail_targets:
            multiturn_cmd.append("--fail-on-targets")
        run_cmd(multiturn_cmd, timeout, dry_run)
    else:
        print("[longtext] mechanical-design multi-turn chat gate skipped (ATTUNE_LONGTEXT_MULTITURN=0)")


def run_web_gate(profile: str, manifest: Path, token: str, dry_run: bool) -> None:
    if not env_bool("ATTUNE_LONGTEXT_UI", False):
        print("[longtext] mechanical-design Web UI gate skipped (ATTUNE_LONGTEXT_UI=0)")
        return
    cmd = [
        sys.executable,
        str(REPO_ROOT / "tests/e2e/playwright/airplane_manual_longtext_ui_e2e.py"),
        "--manifest",
        str(manifest),
        "--base-url",
        BASE_URL,
        "--profile",
        profile,
        "--password",
        PASSWORD,
    ]
    if token:
        cmd.extend(["--token", token])
    query_id = os.environ.get("ATTUNE_LONGTEXT_UI_QUERY_ID", "").strip()
    if query_id:
        cmd.extend(["--query-id", query_id])
    run_cmd(cmd, env_int("ATTUNE_LONGTEXT_UI_TIMEOUT_SEC", 600), dry_run)


def main() -> int:
    dry_run = env_bool("ATTUNE_LONGTEXT_DRY_RUN", False)
    profile = os.environ.get("ATTUNE_LONGTEXT_PROFILE", "edge_scheduler_comprehensive")
    corpus_dir = ensure_under_home(
        Path(
            os.environ.get(
                "ATTUNE_LONGTEXT_CORPUS_DIR",
                "~/attune-e2e-corpora/handbook-of-mechanical-design",
            )
        )
    )
    manifest = Path(
        os.environ.get(
            "ATTUNE_LONGTEXT_MANIFEST",
            f"/tmp/attune-mechanical-design-longtext-{profile}.json",
        )
    )
    golden = Path(
        os.environ.get(
            "ATTUNE_LONGTEXT_GOLDEN",
            f"/tmp/attune-mechanical-design-longtext-{profile}-golden.json",
        )
    )

    print("=== mechanical design handbook longtext E2E ===")
    print(f"[longtext] base_url={BASE_URL}")
    print(f"[longtext] profile={profile}")
    print(f"[longtext] corpus_dir={corpus_dir}")
    print(f"[longtext] manifest={manifest}")

    build_manifest(profile, corpus_dir, manifest, golden, dry_run)
    if dry_run:
        result_dir = Path(os.environ.get("ATTUNE_LONGTEXT_RESULTS_DIR", "/tmp")).expanduser()
        print(f"[longtext] dry-run result search={result_dir / f'attune-mechanical-design-longtext-{profile}-search.json'}")
        print(f"[longtext] dry-run result chat={result_dir / f'attune-mechanical-design-longtext-{profile}-chat.json'}")
        print(f"[longtext] dry-run result multiturn={result_dir / f'attune-mechanical-design-longtext-{profile}-multiturn.json'}")
        print("=== mechanical design handbook longtext E2E DRY RUN PASS ===")
        return 0

    generated_manifest = load_manifest(manifest)
    resolved_profile = resolve_profile_name(generated_manifest, profile)
    if resolved_profile != profile:
        print(f"[longtext] profile alias resolved: {profile} -> {resolved_profile}")
    docs_count, bytes_count = verify_selected_files(manifest, profile)
    print(f"[longtext] selected mechanical-design docs materialized: {docs_count}, bytes={bytes_count}")
    bind_dir = prepare_profile_corpus_view(manifest, profile, corpus_dir)
    token = setup_and_unlock()
    run_background_bind_ux_gate(token)
    bind_corpus(bind_dir, token)
    wait_for_embeddings(token)
    run_gates(profile, manifest, token, dry_run)
    run_web_gate(profile, manifest, token, dry_run)
    print(f"=== mechanical design handbook longtext E2E PASS profile={profile} target_p95_chat_ms=15000 ===")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:  # noqa: BLE001 - preserve a concise tail line for run_all.sh.
        print(f"=== mechanical design handbook longtext E2E FAIL: {exc} ===")
        raise SystemExit(1)
