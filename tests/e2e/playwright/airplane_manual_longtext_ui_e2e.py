#!/usr/bin/env python3
"""Airplane-manual long-text Web UI E2E.

This script assumes the API long-text E2E has already materialized manuals,
bound the corpus, and drained the embedding queue. It verifies the same corpus
through the browser surface: indexed item visibility, chat input, visible
answer/citations, edge scheduler status rendering, and end-to-end response latency.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import time
import urllib.parse
from pathlib import Path
from typing import Any

from playwright.sync_api import Page, Response, sync_playwright


REPO_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO_ROOT / "tests/e2e"))

from airplane_longtext_support import (  # noqa: E402
    FAILED_LOCAL_SCHEDULER,
    TERMINAL_LOCAL_SCHEDULER,
    aliased_target_value,
    citation_hit,
    expected_term_hit,
    load_manifest,
    local_scheduler_status,
    output_text,
    profile_doc_ids,
    request_scheduler_job,
    request_json as support_request_json,
    unwrap_local_scheduler_job,
)

DEFAULT_MANIFEST = REPO_ROOT / "tests/e2e/airplane_manual_longtext_cases.json"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, default=Path(os.environ.get("ATTUNE_LONGTEXT_MANIFEST", DEFAULT_MANIFEST)))
    parser.add_argument("--base-url", default=os.environ.get("ATTUNE_BASE_URL", "http://localhost:18905"))
    parser.add_argument("--profile", default=os.environ.get("ATTUNE_LONGTEXT_PROFILE", "edge_scheduler_comprehensive"))
    parser.add_argument("--query-id", default=os.environ.get("ATTUNE_LONGTEXT_UI_QUERY_ID", ""))
    parser.add_argument("--password", default=os.environ.get("ATTUNE_E2E_PASSWORD", os.environ.get("ATTUNE_VAULT_PW", "e2e-pass-2026")))
    parser.add_argument("--token", default=os.environ.get("ATTUNE_TOKEN", ""))
    parser.add_argument("--headless", default=os.environ.get("ATTUNE_HEADLESS", "1"))
    parser.add_argument("--channel", default=os.environ.get("ATTUNE_PLAYWRIGHT_CHANNEL", "chrome"))
    parser.add_argument("--executable-path", default=os.environ.get("ATTUNE_PLAYWRIGHT_EXECUTABLE", ""))
    parser.add_argument("--timeout-ms", type=int, default=int(os.environ.get("ATTUNE_LONGTEXT_UI_TIMEOUT_MS", "120000")))
    parser.add_argument(
        "--poll-interval-ms",
        type=int,
        default=int(os.environ.get("ATTUNE_LONGTEXT_UI_POLL_INTERVAL_MS", "250")),
    )
    parser.add_argument("--screenshot-dir", type=Path, default=Path(os.environ.get("ATTUNE_LONGTEXT_UI_SHOTS", "docs/screenshots/airplane-longtext-ui")))
    return parser.parse_args()


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


def request_json(
    args: argparse.Namespace,
    method: str,
    path: str,
    body: dict[str, Any] | None = None,
    token: str = "",
    timeout: float = 30.0,
    allow_statuses: set[int] | None = None,
) -> tuple[int, dict[str, Any]]:
    return support_request_json(
        args.base_url,
        method,
        path,
        body=body,
        token=token,
        timeout=timeout,
        allow_statuses=allow_statuses,
    )


def ensure_token(args: argparse.Namespace) -> str:
    if args.token:
        return args.token
    request_json(args, "POST", "/api/v1/vault/setup", {"password": args.password}, allow_statuses={400, 409})
    _, unlocked = request_json(args, "POST", "/api/v1/vault/unlock", {"password": args.password})
    token = unlocked.get("token")
    if not isinstance(token, str) or not token:
        raise RuntimeError("vault unlock did not return a token")
    return token


def ensure_wizard_complete(args: argparse.Namespace, token: str) -> None:
    request_json(
        args,
        "PATCH",
        "/api/v1/settings",
        {"wizard": {"complete": True, "current_step": 5}},
        token=token,
        allow_statuses={403},
    )


def select_query(manifest: dict[str, Any], profile: str, query_id: str) -> dict[str, Any]:
    doc_ids = profile_doc_ids(manifest, profile)
    queries = [
        q
        for q in manifest.get("queries", [])
        if set(q.get("acceptable_hits", [])).intersection(doc_ids)
    ]
    if query_id:
        for q in queries:
            if q.get("id") == query_id:
                return q
        raise SystemExit(f"query {query_id!r} does not apply to profile {profile!r}")

    preferred = manifest.get("web_e2e", {}).get("default_query_id") or "a320_qrh_abnormal"
    for q in queries:
        if q.get("id") == preferred:
            return q
    if not queries:
        raise SystemExit(f"no UI query applies to profile {profile!r}")
    return queries[0]


def visible_any(page: Page, labels: list[str]) -> bool:
    for label in labels:
        try:
            if page.get_by_text(label, exact=True).first.is_visible():
                return True
        except Exception:
            pass
    return False


def wait_visible_any(page: Page, labels: list[str], timeout_ms: int) -> None:
    deadline = time.monotonic() + (timeout_ms / 1000.0)
    while time.monotonic() < deadline:
        if visible_any(page, labels):
            return
        page.wait_for_timeout(250)
    raise RuntimeError(f"timed out waiting for visible text: {' / '.join(labels)}")


def maybe_poll_local_scheduler(args: argparse.Namespace, response: dict[str, Any], token: str) -> tuple[str, dict[str, Any] | None]:
    content = output_text(response) or str(response.get("content") or "")
    scheduler = response.get("local_scheduler")
    if not isinstance(scheduler, dict):
        return content, None
    job_id = scheduler.get("job_id")
    status = local_scheduler_status(scheduler)
    if not job_id or status in TERMINAL_LOCAL_SCHEDULER:
        return content, None

    deadline = time.monotonic() + (args.timeout_ms / 1000)
    last_job: dict[str, Any] | None = None
    poll_interval = max(args.poll_interval_ms, 100) / 1000.0
    while time.monotonic() < deadline:
        time.sleep(poll_interval)
        _, data = request_scheduler_job(args.base_url, str(job_id), token=token, timeout=30)
        job = unwrap_local_scheduler_job(data)
        last_job = job
        status = local_scheduler_status(job)
        if status in TERMINAL_LOCAL_SCHEDULER:
            outputs = job.get("outputs", job)
            final_text = output_text(outputs)
            return final_text or content, job
    return content, last_job


def number_value(value: Any) -> float | int | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return value
    return None


def first_number(*values: Any) -> float | int | None:
    for value in values:
        parsed = number_value(value)
        if parsed is not None:
            return parsed
    return None


def scheduler_job_summary(job: dict[str, Any] | None) -> dict[str, Any] | None:
    if not isinstance(job, dict):
        return None
    outputs = job.get("outputs") if isinstance(job.get("outputs"), dict) else {}
    timings = outputs.get("timings") if isinstance(outputs.get("timings"), dict) else {}
    usage = outputs.get("usage") if isinstance(outputs.get("usage"), dict) else {}
    prompt_details = (
        usage.get("prompt_tokens_details")
        if isinstance(usage.get("prompt_tokens_details"), dict)
        else {}
    )
    summary = {
        "job_id": job.get("job_id") or job.get("id"),
        "status": local_scheduler_status(job) or None,
        "task": job.get("task"),
        "scheduled_as": job.get("scheduled_as"),
        "service_class": job.get("service_class"),
        "model": job.get("model") or outputs.get("model"),
        "latency_ms": number_value(job.get("latency_ms")),
        "queue_wait_ms": number_value(job.get("queue_wait_ms")),
        "prompt_eval_ms": number_value(timings.get("prompt_ms")),
        "decode_ms": number_value(timings.get("predicted_ms")),
        "prompt_tokens": first_number(timings.get("prompt_n"), usage.get("prompt_tokens")),
        "output_tokens": first_number(timings.get("predicted_n"), usage.get("completion_tokens")),
        "prompt_cache_tokens": first_number(
            timings.get("cache_n"),
            prompt_details.get("cached_tokens"),
        ),
    }
    return {key: value for key, value in summary.items() if value not in (None, "")}


def screenshot(page: Page, args: argparse.Namespace, name: str) -> None:
    args.screenshot_dir.mkdir(parents=True, exist_ok=True)
    try:
        page.screenshot(path=str(args.screenshot_dir / f"{name}.png"), full_page=False)
    except Exception as exc:  # noqa: BLE001
        print(f"[ui] screenshot {name} failed: {exc}")


def verify_background_bind_visible(page: Page, args: argparse.Namespace, token: str) -> None:
    if not env_bool("ATTUNE_LONGTEXT_UI_BACKGROUND_BIND", True):
        print("[ui] background bind visibility gate skipped (ATTUNE_LONGTEXT_UI_BACKGROUND_BIND=0)")
        return

    root = Path(
        os.environ.get(
            "ATTUNE_LONGTEXT_UI_BACKGROUND_BIND_DIR",
            f"~/attune-e2e-corpora/ui-background-bind-ux-{int(time.time())}",
        )
    ).expanduser()
    if root.exists():
        shutil.rmtree(root)
    root.mkdir(parents=True, exist_ok=True)
    file_count = env_int("ATTUNE_LONGTEXT_UI_BACKGROUND_BIND_FILES", 64)
    for idx in range(file_count):
        (root / f"ui-background-{idx:03d}.md").write_text(
            "\n".join(
                [
                    f"# UI background indexing gate {idx}",
                    "",
                    "Playwright verifies that a background folder bind does not block the page",
                    "and that progress is visible in the sidebar.",
                    "attune-ui-background-indexing-gate",
                ]
            ),
            encoding="utf-8",
        )

    target_ms = env_int("ATTUNE_LONGTEXT_UI_BACKGROUND_BIND_RETURN_MS_MAX", 2000)
    started = time.perf_counter()
    _, data = request_json(
        args,
        "POST",
        "/api/v1/index/bind",
        {
            "path": str(root),
            "recursive": True,
            "file_types": ["md", "txt"],
            "corpus_domain": "ux",
            "background": True,
        },
        token=token,
        timeout=10,
    )
    elapsed_ms = int((time.perf_counter() - started) * 1000)
    if data.get("status") != "accepted" or data.get("background") is not True:
        raise RuntimeError(f"background bind did not return accepted/background response: {data}")
    if elapsed_ms > target_ms:
        raise RuntimeError(f"background bind returned too slowly: {elapsed_ms}ms > {target_ms}ms")

    wait_visible_any(
        page,
        ["后台任务", "Background tasks", "正在后台扫描", "后台索引完成"],
        min(args.timeout_ms, 15_000),
    )
    screenshot(page, args, "00-background-indexing")
    dir_id = str(data.get("dir_id") or "")
    if dir_id:
        encoded = urllib.parse.quote(dir_id, safe="")
        request_json(args, "DELETE", f"/api/v1/index/unbind?dir_id={encoded}", token=token, allow_statuses={404})
    print(f"[ui] background bind visible return_ms={elapsed_ms} files={file_count}")


def launch_browser(playwright: Any, args: argparse.Namespace) -> Any:
    headless = str(args.headless).strip().lower() not in {"0", "false", "no"}
    executable_path = args.executable_path.strip()
    if executable_path:
        return playwright.chromium.launch(executable_path=executable_path, headless=headless)
    channel = args.channel.strip()
    if channel:
        try:
            return playwright.chromium.launch(channel=channel, headless=headless)
        except Exception as exc:  # noqa: BLE001
            print(f"[ui] launch channel={channel!r} failed, falling back to bundled chromium: {exc}")
    return playwright.chromium.launch(headless=headless)


def wait_main(page: Page, args: argparse.Namespace) -> None:
    page.goto(args.base_url, wait_until="networkidle", timeout=args.timeout_ms)
    if page.get_by_role("button", name="解锁").count() > 0:
        page.get_by_label("主密码").fill(args.password, timeout=10_000)
        page.get_by_role("button", name="解锁").click()
    page.locator('button[aria-label="新对话"], button[aria-label="New chat"]').first.wait_for(
        state="visible",
        timeout=args.timeout_ms,
    )
    dismiss_first_run_modals(page)


def dismiss_first_run_modals(page: Page) -> None:
    for name in ("我知道了", "知道了", "Got it", "OK"):
        try:
            page.get_by_role("button", name=name).first.click(timeout=1_500)
            page.wait_for_timeout(300)
            return
        except Exception:
            pass


def click_button(page: Page, names: list[str], timeout: int = 5_000) -> None:
    for name in names:
        locator = page.locator(f'button[aria-label="{name}"]').first
        if locator.count() > 0:
            locator.click(timeout=timeout)
            return
    for name in names:
        try:
            page.get_by_role("button", name=name).first.click(timeout=timeout)
            return
        except Exception:
            pass
    joined = " / ".join(names)
    raise RuntimeError(f"button not found: {joined}")


def click_items(page: Page) -> None:
    click_button(page, ["条目", "Items"])


def click_new_chat(page: Page) -> None:
    click_button(page, ["新对话", "New chat"])


def verify_item_visible(page: Page, query: dict[str, Any], args: argparse.Namespace) -> str:
    click_items(page)
    first_file = query.get("acceptable_files", [""])[0]
    needle = Path(first_file).stem
    if not needle:
        needle = query.get("expect_any", [""])[0]
    search = page.locator('input[type="text"]').first
    search.fill(needle, timeout=10_000)
    page.wait_for_timeout(800)
    visible = page.get_by_text(needle, exact=False).first
    visible.wait_for(state="visible", timeout=30_000)
    screenshot(page, args, "01-items-indexed")
    return needle


def send_chat_and_capture(page: Page, query: dict[str, Any], args: argparse.Namespace) -> tuple[float, dict[str, Any]]:
    click_new_chat(page)
    textbox = page.get_by_label("对话输入框")
    if textbox.count() == 0:
        textbox = page.get_by_label("Chat input")
    textbox.fill(query["query"], timeout=10_000)
    start = time.perf_counter()
    with page.expect_response(
        lambda r: r.request.method == "POST" and r.url.rstrip("/").endswith("/api/v1/chat"),
        timeout=args.timeout_ms,
    ) as resp_info:
        try:
            page.get_by_role("button", name="发送消息").click(timeout=5_000)
        except Exception:
            page.get_by_role("button", name="Send message").click(timeout=5_000)
    resp: Response = resp_info.value
    elapsed_ms = (time.perf_counter() - start) * 1000
    if resp.status >= 400:
        text = resp.text()
        try:
            parsed = json.loads(text) if text else {}
        except Exception:
            parsed = {"raw": text}
        code = parsed.get("code") if isinstance(parsed, dict) else None
        retryable = parsed.get("retryable") if isinstance(parsed, dict) else None
        may_degrade = parsed.get("may_degrade") if isinstance(parsed, dict) else None
        raise RuntimeError(
            f"chat UI request failed HTTP {resp.status} code={code} "
            f"retryable={retryable} may_degrade={may_degrade}: {text[:500]}"
        )
    data = resp.json()
    if not isinstance(data, dict):
        raise RuntimeError("chat UI response was not a JSON object")
    return elapsed_ms, data


def assert_visible_answer(page: Page, content: str, args: argparse.Namespace) -> None:
    probe = content.strip()
    if len(probe) > 80:
        probe = probe[:80]
    if probe:
        page.get_by_text(probe[: min(len(probe), 40)], exact=False).first.wait_for(
            state="visible",
            timeout=args.timeout_ms,
        )


def main() -> int:
    args = parse_args()
    manifest = load_manifest(args.manifest)
    query = select_query(manifest, args.profile, args.query_id)
    target_ms = aliased_target_value(
        manifest.get("evaluation_targets", {}).get("rag_answer", {}),
        "edge_scheduler_30b_p95_latency_ms_max",
        10_000,
    )

    token = ensure_token(args)
    ensure_wizard_complete(args, token)

    console_errors: list[str] = []
    with sync_playwright() as p:
        browser = launch_browser(p, args)
        context = browser.new_context(locale="zh-CN", viewport={"width": 1440, "height": 900})
        context.add_init_script(
            f"sessionStorage.setItem('attune_token', {json.dumps(token)});"
        )
        page = context.new_page()
        page.on(
            "console",
            lambda msg: console_errors.append(msg.text)
            if msg.type == "error"
            and not any(noise in msg.text for noise in ("favicon", "ERR_CONNECTION_REFUSED", "ws/scan-progress"))
            else None,
        )
        try:
            print("=== airplane manual longtext Web UI E2E ===")
            print(f"[ui] profile={args.profile} query={query['id']}")
            wait_main(page, args)
            verify_background_bind_visible(page, args, token)
            screenshot(page, args, "00-main")
            item_needle = verify_item_visible(page, query, args)
            print(f"[ui] indexed item visible: {item_needle}")

            turn_start = time.perf_counter()
            initial_ms, response = send_chat_and_capture(page, query, args)
            final_content, job = maybe_poll_local_scheduler(args, response, token)
            if isinstance(job, dict) and local_scheduler_status(job) in FAILED_LOCAL_SCHEDULER:
                raise RuntimeError(
                    "edge scheduler job ended with "
                    f"{local_scheduler_status(job)}: {json.dumps(job.get('error') or job, ensure_ascii=False)[:500]}"
                )
            assert_visible_answer(page, final_content, args)
            if isinstance(response.get("local_scheduler"), dict):
                wait_visible_any(page, ["边缘调度器", "Edge scheduler", "本地调度器", "Local scheduler"], min(args.timeout_ms, 30_000))
            if isinstance(response.get("citations"), list) and response.get("citations"):
                wait_visible_any(page, ["📎 引用", "📎 Citations"], min(args.timeout_ms, 30_000))
            total_ms = (time.perf_counter() - turn_start) * 1000
            screenshot(page, args, "02-chat-answer")

            citations = response.get("citations") if isinstance(response.get("citations"), list) else []
            checks = {
                "answer_term_hit": expected_term_hit(final_content, query.get("expect_any", [])),
                "citation_hit": citation_hit(citations, query.get("acceptable_hits", []), query.get("acceptable_files", [])),
                "citation_visible": not citations or visible_any(page, ["📎 引用", "📎 Citations"]),
                "latency_target": total_ms <= float(target_ms),
            }
            if isinstance(response.get("local_scheduler"), dict):
                checks["local_scheduler_status_visible"] = visible_any(page, ["边缘调度器", "Edge scheduler", "本地调度器", "Local scheduler"])

            failed = [name for name, ok in checks.items() if not ok]
            print(
                json.dumps(
                    {
                        "checks": checks,
                        "latency_ms": total_ms,
                        "initial_chat_latency_ms": initial_ms,
                        "target_ms": target_ms,
                        "scheduler_job": scheduler_job_summary(job),
                    },
                    ensure_ascii=False,
                    indent=2,
                )
            )
            if console_errors:
                print("[ui] console errors:")
                for err in console_errors[:10]:
                    print(f"  - {err[:300]}")
                failed.append("console_errors")
            if failed:
                raise RuntimeError(f"UI checks failed: {', '.join(failed)}")
        finally:
            context.close()
            browser.close()

    print(f"=== airplane manual longtext Web UI E2E PASS query={query['id']} ===")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:  # noqa: BLE001
        print(f"=== airplane manual longtext Web UI E2E FAIL: {exc} ===")
        raise SystemExit(1)
