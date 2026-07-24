#!/usr/bin/env python3
"""Playwright gate for using kb-web-demo as the Attune RAG eval frontend."""
from __future__ import annotations

import argparse
import json
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


class FrontendEvalError(RuntimeError):
    def __init__(self, stage: str, reason: str) -> None:
        super().__init__(reason)
        self.stage = stage


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="http://127.0.0.1:8890")
    parser.add_argument("--api-url", default="http://127.0.0.1:8889")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--headless", type=int, default=1)
    parser.add_argument("--timeout-ms", type=int, default=120000)
    parser.add_argument("--profile", choices=("smoke", "deep"), default="smoke")
    parser.add_argument("--dry-run", action="store_true")
    return parser.parse_args()


def write_report(path: Path, report: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def dry_run_report(args: argparse.Namespace) -> dict[str, Any]:
    chat_cases = web_demo_chat_cases("WEB_DEMO_DRY_RUN") if args.profile == "deep" else []
    return {
        "schema_version": "attune.eval.web_demo_frontend.v1",
        "mode": "dry_run",
        "profile": args.profile,
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
                "web_demo_complex_chat_pass_rate": 1.0,
            }
        },
        "artifacts": {
            "chat_cases": [
                {
                    "case_id": case["case_id"],
                    "kind": case["kind"],
                    "required_terms": case["required_terms"],
                    "expected_tasks": sorted(case["expected_tasks"]),
                }
                for case in chat_cases
            ]
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


def api_post_json(base_url: str, path: str, payload: dict[str, Any], timeout_ms: int) -> dict[str, Any]:
    url = f"{base_url.rstrip('/')}{path}"
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=body,
        method="POST",
        headers={"content-type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=max(timeout_ms / 1000, 1)) as response:
            text = response.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as exc:
        text = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"POST {path} HTTP {exc.code}: {text[:1000]}") from exc
    data = json.loads(text) if text else {}
    return data if isinstance(data, dict) else {"_payload": data}


def json_contains(payload: Any, needle: str) -> bool:
    try:
        return needle.lower() in json.dumps(payload, ensure_ascii=False).lower()
    except Exception:
        return False


def folded_text(value: Any) -> str:
    if isinstance(value, str):
        return value.casefold()
    try:
        return json.dumps(value, ensure_ascii=False).casefold()
    except Exception:
        return str(value).casefold()


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


def citations_from_payload(payload: dict[str, Any]) -> list[Any]:
    citations = payload.get("citations")
    if isinstance(citations, list):
        return citations
    meta = payload.get("local_scheduler")
    if isinstance(meta, dict):
        outputs = meta.get("outputs")
        citations = outputs.get("citations") if isinstance(outputs, dict) else None
    return citations if isinstance(citations, list) else []


def web_demo_fixture(token: str) -> str:
    return (
        "# Attune web-demo deep RAG fixture\n\n"
        f"Marker: {token} appears in every expected answer.\n\n"
        "## TCP/IP origin\n"
        "TCP/IP originated from ARPANET and DARPA packet-switching research. "
        "The protocol suite separates TCP reliability from IP addressing and routing.\n\n"
        "## TCP/IP troubleshooting runbook\n"
        "Troubleshooting checks physical link, NIC status, IP address, subnet mask, default gateway, "
        "routing table, DNS resolution, listening ports, firewall rules, packet capture, application timeout, "
        "topology, change timeline, and logs. Do not invent operational conclusions without cited evidence.\n\n"
        "## Decision evidence\n"
        "Before recommending remediation, collect packet captures, route tables, DNS query results, "
        "firewall policy, service logs, topology diagrams, change approvals, and user symptom timelines. "
        "If audit logs or packet capture evidence are missing, the issue cannot be directly determined.\n\n"
        "## Boundary\n"
        "Zero trust segmentation is not directly covered by this manual. It may be discussed as industry-general "
        "guidance, but it must not be presented as a manual conclusion or source conclusion when evidence is insufficient.\n\n"
        "## Mechanical design analogy\n"
        "Airplane mechanical design requires load-path analysis, fatigue assessment, fastener selection, material trade-off, "
        "aerodynamic load validation, vibration review, manufacturability checks, and maintenance access planning.\n"
    )


def web_demo_chat_cases(token: str) -> list[dict[str, Any]]:
    return [
        {
            "case_id": "fact_origin",
            "kind": "chat",
            "prompt": f"{token} 对 TCP/IP 起源给出了什么证据？",
            "required_terms": [token, "TCP/IP", "ARPANET", "DARPA"],
            "expected_tasks": {"kb.query.ask", "local.extractive.answer"},
        },
        {
            "case_id": "operation_troubleshooting",
            "kind": "chat",
            "prompt": f"基于 {token}，如何排查 TCP/IP 连接异常？必须包含日志、抓包、路由和不要编造。",
            "required_terms": ["日志", "抓包", "路由", "不要编造"],
            "expected_tasks": {"kb.query.ask"},
        },
        {
            "case_id": "multi_intent_decision",
            "kind": "chat",
            "prompt": f"基于 {token}，同时判断应该先收集哪些证据、如何排序排查步骤，并说明缺少 packet capture 时的结论边界。",
            "required_terms": ["packet capture", "证据", "先", "边界"],
            "expected_tasks": {"kb.query.ask"},
        },
        {
            "case_id": "negative_evidence_boundary",
            "kind": "chat",
            "prompt": f"如果 {token} 中缺少审计日志或 packet capture，能否直接判定根因？",
            "required_terms": ["不能直接", "证据不足"],
            "expected_tasks": {"kb.query.ask"},
        },
        {
            "case_id": "out_of_manual_industry_general",
            "kind": "chat",
            "prompt": f"如果 {token} 手册未直接覆盖 zero trust segmentation，但客户问能否作为整改建议，应该如何回答？",
            "required_terms": ["知识库未直接覆盖", "行业通用", "不能当作手册结论"],
            "expected_tasks": {"kb.query.ask", "local.boundary.industry_general"},
        },
        {
            "case_id": "summary_citation_coverage",
            "kind": "summary",
            "prompt": f"总结 {token} 中 TCP/IP 起源、排查证据、结论边界和 airplane mechanical design 类比。",
            "required_terms": ["TCP/IP", "证据", "边界", "airplane"],
            "expected_tasks": {"kb.query.ask", "local.extractive.summary"},
        },
    ]


def case_terms_present(text: str, required_terms: list[str]) -> list[str]:
    folded = text.casefold()
    aliases = {
        "日志": ["日志", "logs", "log"],
        "抓包": ["抓包", "packet capture", "pcap", "tcpdump"],
        "packet capture": ["packet capture", "数据包捕获", "抓包", "pcap", "tcpdump"],
        "路由": ["路由", "routing", "routing table", "route table", "default gateway"],
        "不要编造": ["不要编造", "不能编造", "not invent", "unsupported"],
        "证据": ["证据", "evidence", "source", "citation"],
        "边界": ["边界", "boundary", "scope"],
        "行业通用": ["行业通用", "industry-general", "industry general"],
        "不能当作手册结论": ["不能当作手册结论", "不能作为手册结论", "not a manual conclusion"],
        "知识库未直接覆盖": ["知识库未直接覆盖", "未直接覆盖", "not directly covered"],
    }
    missing = []
    for term in required_terms:
        candidates = aliases.get(term, [term])
        if not any(candidate.casefold() in folded for candidate in candidates):
            missing.append(term)
    return missing


def evaluate_chat_case(payload: dict[str, Any], case: dict[str, Any]) -> dict[str, Any]:
    text = first_text(payload)
    citations = citations_from_payload(payload)
    missing_terms = case_terms_present(text, case["required_terms"])
    meta = payload.get("local_scheduler") if isinstance(payload.get("local_scheduler"), dict) else {}
    task = str(meta.get("task") or "")
    task_ok = not task or task in case["expected_tasks"]
    return {
        "case_id": case["case_id"],
        "kind": case["kind"],
        "pass": bool(text) and not missing_terms and bool(citations) and task_ok,
        "missing_terms": missing_terms,
        "citation_count": len(citations),
        "scheduler_task": task or None,
        "latency_ms": latency_from_chat_payload(payload),
        "content_excerpt": text[:500],
    }


def latency_from_chat_payload(payload: dict[str, Any]) -> float | None:
    candidates = [
        payload.get("latency_ms"),
        (payload.get("cost") or {}).get("latency_ms") if isinstance(payload.get("cost"), dict) else None,
        (payload.get("local_scheduler") or {}).get("latency_ms")
        if isinstance(payload.get("local_scheduler"), dict)
        else None,
    ]
    for value in candidates:
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            return float(value)
    return None


def run_live(args: argparse.Namespace) -> dict[str, Any]:
    from playwright.sync_api import sync_playwright

    failures: list[dict[str, Any]] = []
    stage = "init"
    checks = {
        "upload": False,
        "vector_chunk_render": False,
        "chat": False,
        "summary": False,
        "citation_render": False,
        "time_render": False,
    }
    if args.profile == "deep":
        checks["complex_chat"] = False
    token = f"WEB_DEMO_EVAL_{int(time.time() * 1000)}"
    fixture = web_demo_fixture(token)
    chat_case_results: list[dict[str, Any]] = []

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
            stage = "open_page"
            page.goto(f"{args.base_url}?api={args.api_url}", wait_until="networkidle", timeout=args.timeout_ms)
            page.get_by_text("上传 & 管理", exact=False).first.wait_for(state="visible", timeout=30000)
            stage = "upload_fixture"
            page.locator('input[type="file"]').first.set_input_files(str(fixture_path))
            upload_rendered = visible_text(page, token, timeout=min(args.timeout_ms, 45000)) or visible_text(
                page, "ready", timeout=2000
            )
            checks["upload"] = upload_rendered and wait_for_demo_ready(page, timeout=min(args.timeout_ms, 90000))
            checks["time_render"] = visible_text(page, "全流程时间", timeout=5000)

            stage = "vector_search"
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

            stage = "smoke_chat"
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

            stage = "smoke_summary"
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

            if args.profile == "deep":
                for case in web_demo_chat_cases(token):
                    stage = f"deep_case_api:{case['case_id']}"
                    try:
                        case_payload = api_post_json(
                            args.api_url,
                            "/api/v1/chat",
                            {
                                "message": case["prompt"],
                                "history": [],
                                "session_id": f"web-demo-eval-{token}-{case['case_id']}",
                            },
                            args.timeout_ms,
                        )
                        case_result = evaluate_chat_case(case_payload, case)
                        case_result["execution"] = "api_proxy"
                    except Exception as exc:
                        case_result = {
                            "case_id": case["case_id"],
                            "kind": case["kind"],
                            "pass": False,
                            "missing_terms": case["required_terms"],
                            "citation_count": 0,
                            "scheduler_task": None,
                            "latency_ms": None,
                            "execution": "api_proxy",
                            "error": str(exc),
                        }
                    chat_case_results.append(case_result)
                checks["complex_chat"] = bool(chat_case_results) and all(
                    case["pass"] for case in chat_case_results
                )
        except Exception as exc:
            raise FrontendEvalError(stage, str(exc)) from exc
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
    complex_pass_rate = (
        sum(1 for case in chat_case_results if case["pass"]) / len(chat_case_results)
        if chat_case_results
        else (1.0 if args.profile != "deep" else 0.0)
    )
    return {
        "schema_version": "attune.eval.web_demo_frontend.v1",
        "mode": "live",
        "profile": args.profile,
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
                "web_demo_complex_chat_pass_rate": complex_pass_rate,
                "elapsed_ms": elapsed_ms,
            }
        },
        "artifacts": {
            "chat_cases": chat_case_results,
            "last_stage": stage,
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
        stage = getattr(exc, "stage", None)
        write_report(
            args.out,
            {
                "schema_version": "attune.eval.web_demo_frontend.v1",
                "mode": "live",
                "profile": args.profile,
                "target": {"base_url": args.base_url, "api_url": args.api_url},
                "metrics": {"frontend": {"web_demo_flow_pass_rate": 0.0}},
                "artifacts": {"chat_cases": [], "last_stage": stage},
                "failures": [{"failure_layer": "frontend", "stage": stage, "reason": str(exc)}],
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
