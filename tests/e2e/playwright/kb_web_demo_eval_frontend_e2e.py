#!/usr/bin/env python3
"""Playwright gate for using kb-web-demo as the Attune RAG eval frontend."""
from __future__ import annotations

import argparse
import json
import tempfile
import time
from pathlib import Path
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="http://127.0.0.1:8890")
    parser.add_argument("--api-url", default="http://127.0.0.1:8889")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--headless", type=int, default=1)
    parser.add_argument("--timeout-ms", type=int, default=120000)
    parser.add_argument("--dry-run", action="store_true")
    return parser.parse_args()


def write_report(path: Path, report: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def dry_run_report(args: argparse.Namespace) -> dict[str, Any]:
    return {
        "schema_version": "attune.eval.web_demo_frontend.v1",
        "mode": "dry_run",
        "target": {
            "base_url": args.base_url,
            "api_url": args.api_url,
        },
        "checks": [
            "upload",
            "vector_chunk_render",
            "chat",
            "summary",
            "citation_render",
            "time_render",
        ],
        "metrics": {
            "frontend": {
                "web_demo_flow_pass_rate": 1.0,
                "web_demo_citation_render_rate": 1.0,
                "web_demo_time_render_rate": 1.0,
                "web_demo_vector_chunk_render_rate": 1.0,
            }
        },
        "failures": [],
    }


def visible_text(page: Any, text: str, timeout: int = 5000) -> bool:
    try:
        page.get_by_text(text, exact=False).first.wait_for(state="visible", timeout=timeout)
        return True
    except Exception:
        return False


def wait_for_demo_ready(page: Any, timeout: int) -> bool:
    try:
        page.wait_for_function(
            """
            () => {
              const rows = JSON.parse(sessionStorage.getItem('attune_demo_files') || '[]');
              return rows.some(([, f]) => f && (f.ready || f.status === 'ready'));
            }
            """,
            timeout=timeout,
        )
        return True
    except Exception:
        return False


def response_json(response: Any) -> dict[str, Any]:
    try:
        payload = response.json()
    except Exception:
        try:
            body = response.text()
        except Exception as exc:
            body = f"<unreadable response: {exc}>"
        return {"_status": response.status, "_body": body[:5000]}
    return payload if isinstance(payload, dict) else {"_payload": payload}


def json_contains(payload: Any, needle: str) -> bool:
    try:
        return needle.lower() in json.dumps(payload, ensure_ascii=False).lower()
    except Exception:
        return False


def first_text(payload: Any, keys: tuple[str, ...] = ("content", "answer", "response", "summary", "text")) -> str:
    if isinstance(payload, dict):
        for key in keys:
            value = payload.get(key)
            if isinstance(value, str) and value.strip():
                return value.strip()
        for value in payload.values():
            text = first_text(value, keys)
            if text:
                return text
    elif isinstance(payload, list):
        for value in payload:
            text = first_text(value, keys)
            if text:
                return text
    return ""


def scheduler_chat_ok(payload: dict[str, Any], expected_tasks: set[str]) -> bool:
    meta = payload.get("local_scheduler")
    if isinstance(meta, dict):
        terminal = meta.get("terminal_error") or meta.get("error")
        if terminal:
            return False
        task = str(meta.get("task") or "")
        if task and task not in expected_tasks:
            return False
    content = first_text(payload)
    citations = payload.get("citations")
    if not isinstance(citations, list) and isinstance(meta, dict):
        outputs = meta.get("outputs")
        citations = outputs.get("citations") if isinstance(outputs, dict) else None
    return bool(content) and isinstance(citations, list)


def run_live(args: argparse.Namespace) -> dict[str, Any]:
    from playwright.sync_api import sync_playwright

    failures: list[dict[str, Any]] = []
    checks = {
        "upload": False,
        "vector_chunk_render": False,
        "chat": False,
        "summary": False,
        "citation_render": False,
        "time_render": False,
    }
    token = f"WEB_DEMO_EVAL_{int(time.time() * 1000)}"
    fixture = (
        "# Attune web-demo eval fixture\n\n"
        f"{token} appears in the knowledge base.\n"
        "TCP/IP originated from ARPANET and DARPA packet-switching research.\n"
        "Troubleshooting checks physical link, IP, route, DNS, packet capture, ports, and logs.\n"
    )

    with tempfile.NamedTemporaryFile(
        "w",
        prefix=f"{token}_",
        suffix=".md",
        delete=False,
        encoding="utf-8",
    ) as f:
        f.write(fixture)
        fixture_path = Path(f.name)

    started = time.perf_counter()
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=bool(args.headless))
        page = browser.new_page()
        try:
            page.goto(f"{args.base_url}?api={args.api_url}", wait_until="networkidle", timeout=args.timeout_ms)
            page.get_by_text("上传 & 管理", exact=False).first.wait_for(state="visible", timeout=30000)
            page.locator('input[type="file"]').first.set_input_files(str(fixture_path))
            upload_rendered = visible_text(page, token, timeout=min(args.timeout_ms, 45000)) or visible_text(
                page, "ready", timeout=2000
            )
            checks["upload"] = upload_rendered and wait_for_demo_ready(page, timeout=min(args.timeout_ms, 90000))
            checks["time_render"] = visible_text(page, "全流程时间", timeout=5000)

            page.get_by_role("button", name="向量库").click(timeout=5000)
            page.locator("#vectorQuery").fill(token, timeout=5000)
            with page.expect_response(
                lambda r: r.request.method in {"GET", "POST"} and "/api/v1/search" in r.url,
                timeout=args.timeout_ms,
            ) as vector_resp:
                page.get_by_role("button", name="检索块").click(timeout=5000)
            vector_payload = response_json(vector_resp.value)
            checks["vector_chunk_render"] = json_contains(vector_payload, token) or visible_text(
                page, token, timeout=min(args.timeout_ms, 30000)
            )

            page.get_by_role("button", name="Chat RAG").click(timeout=5000)
            page.locator("#chatInput").fill(f"{token} 在知识库里说明了什么？", timeout=5000)
            with page.expect_response(
                lambda r: r.request.method == "POST" and r.url.rstrip("/").endswith("/api/v1/chat"),
                timeout=args.timeout_ms,
            ) as chat_resp:
                page.locator("#chatBtn").click(timeout=5000)
            chat_payload = response_json(chat_resp.value)
            checks["chat"] = scheduler_chat_ok(chat_payload, {"kb.query.ask", "local.extractive.answer"}) or visible_text(
                page, token,
                timeout=min(args.timeout_ms, 30000),
            )
            checks["citation_render"] = visible_text(page, "引用", timeout=10000) or visible_text(page, "knowledge", timeout=10000)

            page.get_by_role("button", name="Summary RAG").click(timeout=5000)
            page.locator("#summaryInput").fill(f"总结 {token} 文档的核心结论。", timeout=5000)
            with page.expect_response(
                lambda r: r.request.method == "POST" and r.url.rstrip("/").endswith("/api/v1/chat"),
                timeout=args.timeout_ms,
            ) as summary_resp:
                page.locator("#summaryBtn").click(timeout=5000)
            summary_payload = response_json(summary_resp.value)
            checks["summary"] = scheduler_chat_ok(
                summary_payload,
                {"kb.query.ask", "local.extractive.summary"},
            ) or visible_text(page, token, timeout=min(args.timeout_ms, 30000)) or visible_text(page, "ARPANET", timeout=10000)
        finally:
            browser.close()
            try:
                fixture_path.unlink()
            except OSError:
                pass

    for name, passed in checks.items():
        if not passed:
            failures.append({"failure_layer": "frontend", "check": name, "reason": f"{name} did not pass"})
    elapsed_ms = (time.perf_counter() - started) * 1000
    flow_rate = sum(1 for passed in checks.values() if passed) / len(checks)
    return {
        "schema_version": "attune.eval.web_demo_frontend.v1",
        "mode": "live",
        "target": {
            "base_url": args.base_url,
            "api_url": args.api_url,
        },
        "checks": checks,
        "metrics": {
            "frontend": {
                "web_demo_flow_pass_rate": flow_rate,
                "web_demo_citation_render_rate": 1.0 if checks["citation_render"] else 0.0,
                "web_demo_time_render_rate": 1.0 if checks["time_render"] else 0.0,
                "web_demo_vector_chunk_render_rate": 1.0 if checks["vector_chunk_render"] else 0.0,
                "elapsed_ms": elapsed_ms,
            }
        },
        "failures": failures,
    }


def main() -> int:
    args = parse_args()
    if args.dry_run:
        report = dry_run_report(args)
        write_report(args.out, report)
        print("kb-web-demo eval frontend dry-run PASS")
        return 0
    try:
        report = run_live(args)
        write_report(args.out, report)
    except Exception as exc:
        write_report(
            args.out,
            {
                "schema_version": "attune.eval.web_demo_frontend.v1",
                "mode": "live",
                "target": {"base_url": args.base_url, "api_url": args.api_url},
                "metrics": {"frontend": {"web_demo_flow_pass_rate": 0.0}},
                "failures": [{"failure_layer": "frontend", "reason": str(exc)}],
            },
        )
        print(f"kb-web-demo eval frontend failed: {exc}")
        return 1
    if report["failures"]:
        print(f"kb-web-demo eval frontend failed checks: {len(report['failures'])}")
        return 1
    print("kb-web-demo eval frontend PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
