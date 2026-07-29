#!/usr/bin/env python3
"""Run a no-OCR single-domain scale RAG probe against an Attune server."""
from __future__ import annotations

import argparse
import json
import os
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any


TERMINAL_OK = {"done", "completed", "complete", "success", "succeeded"}
TERMINAL_BAD = {"failed", "error", "cancelled", "canceled", "expired"}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="http://127.0.0.1:18907")
    parser.add_argument("--scheduler-base", default="http://127.0.0.1:8090")
    parser.add_argument("--bind-dir", required=True)
    parser.add_argument("--scenario", default="tests/eval/scenarios/security/security_scale_coverage.json")
    parser.add_argument("--report-out", required=True)
    parser.add_argument("--password", default=os.environ.get("ATTUNE_E2E_PASSWORD", os.environ.get("ATTUNE_VAULT_PW", "")))
    parser.add_argument("--wait-seconds", type=int, default=900)
    parser.add_argument("--chat-timeout", type=float, default=240.0)
    return parser.parse_args()


def request_json(
    base_url: str,
    method: str,
    path: str,
    body: Any | None = None,
    token: str = "",
    timeout: float = 60.0,
) -> tuple[int, dict[str, Any]]:
    data = json.dumps(body).encode("utf-8") if body is not None else None
    headers = {"Content-Type": "application/json"} if body is not None else {}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(
        base_url.rstrip("/") + path,
        data=data,
        headers=headers,
        method=method,
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode("utf-8")
            payload = json.loads(raw) if raw else {}
            if not isinstance(payload, dict):
                payload = {"value": payload}
            return resp.status, payload
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode("utf-8", errors="replace")
        try:
            payload = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            payload = {"raw": raw[:1000]}
        if not isinstance(payload, dict):
            payload = {"value": payload}
        payload["http_status"] = exc.code
        return exc.code, payload


def response_text(payload: dict[str, Any]) -> str:
    for key in ("answer", "content", "text", "message"):
        value = payload.get(key)
        if isinstance(value, str):
            return value
    local_scheduler = payload.get("local_scheduler")
    outputs = local_scheduler.get("outputs") if isinstance(local_scheduler, dict) else None
    choices = outputs.get("choices") if isinstance(outputs, dict) else None
    if isinstance(choices, list):
        for choice in choices:
            message = choice.get("message") if isinstance(choice, dict) else None
            content = message.get("content") if isinstance(message, dict) else None
            if isinstance(content, str) and content.strip():
                return content
    job = payload.get("job")
    outputs = job.get("outputs") if isinstance(job, dict) and isinstance(job.get("outputs"), dict) else None
    choices = outputs.get("choices") if isinstance(outputs, dict) else None
    if isinstance(choices, list):
        for choice in choices:
            message = choice.get("message") if isinstance(choice, dict) else None
            content = message.get("content") if isinstance(message, dict) else None
            if isinstance(content, str) and content.strip():
                return content
    return json.dumps(payload, ensure_ascii=False)


def citation_labels(payload: dict[str, Any]) -> list[str]:
    citations = payload.get("citations")
    if not isinstance(citations, list):
        job = payload.get("job")
        outputs = job.get("outputs") if isinstance(job, dict) and isinstance(job.get("outputs"), dict) else None
        citations = outputs.get("citations") if isinstance(outputs, dict) else None
    if not isinstance(citations, list):
        return []
    labels: list[str] = []
    for citation in citations:
        if not isinstance(citation, dict):
            continue
        for key in ("item_id", "title", "chunk_id", "source", "path"):
            value = citation.get(key)
            if isinstance(value, str):
                labels.append(value)
    return labels


def job_id(payload: dict[str, Any]) -> str:
    local_scheduler = payload.get("local_scheduler")
    candidates = [
        local_scheduler.get("job_id") if isinstance(local_scheduler, dict) else None,
        payload.get("job_id"),
        payload.get("id"),
    ]
    for candidate in candidates:
        if isinstance(candidate, str) and candidate.startswith("job_"):
            return candidate
    return ""


def job_status(payload: dict[str, Any]) -> str:
    job = payload.get("job") if isinstance(payload.get("job"), dict) else payload
    return str(job.get("status") or job.get("state") or job.get("phase") or "").lower()


def merge_job_payload(chat_payload: dict[str, Any], job_payload: dict[str, Any]) -> dict[str, Any]:
    merged = dict(chat_payload)
    job = job_payload.get("job") if isinstance(job_payload.get("job"), dict) else job_payload
    merged["job"] = job
    outputs = job.get("outputs") if isinstance(job, dict) and isinstance(job.get("outputs"), dict) else {}
    for key in ("answer", "content", "text"):
        value = outputs.get(key)
        if isinstance(value, str) and value.strip():
            merged["answer"] = value
            break
    if not isinstance(merged.get("answer"), str) or not merged["answer"].strip():
        choices = outputs.get("choices")
        if isinstance(choices, list):
            for choice in choices:
                message = choice.get("message") if isinstance(choice, dict) else None
                content = message.get("content") if isinstance(message, dict) else None
                if isinstance(content, str) and content.strip():
                    merged["answer"] = content
                    break
    if isinstance(outputs.get("citations"), list):
        merged["citations"] = outputs["citations"]
    local_scheduler = dict(merged.get("local_scheduler") or {})
    if isinstance(job, dict):
        for key in (
            "cache_hit",
            "cold_start_wait_ms",
            "device_used",
            "eta_ms",
            "latency_ms",
            "model",
            "outputs",
            "phase",
            "queue_wait_ms",
            "reason",
            "scheduled_as",
            "service_class",
            "startup_state",
            "startup_wait_ms",
            "status",
            "task",
            "worker_pid",
        ):
            if key in job:
                local_scheduler[key] = job.get(key)
    if local_scheduler:
        merged["local_scheduler"] = local_scheduler
    return merged


def poll_job(base_url: str, token: str, ident: str, timeout: float) -> tuple[dict[str, Any], int]:
    deadline = time.monotonic() + timeout
    polls = 0
    last: dict[str, Any] = {}
    path = f"/api/v1/chat/local-scheduler/jobs/{urllib.parse.quote(ident, safe='')}"
    while time.monotonic() <= deadline:
        _, last = request_json(base_url, "GET", path, token=token, timeout=20)
        polls += 1
        status = job_status(last)
        if status in TERMINAL_OK:
            return last, polls
        if status in TERMINAL_BAD:
            raise RuntimeError(f"scheduler job {ident} failed: {last}")
        time.sleep(0.5)
    raise RuntimeError(f"scheduler job {ident} did not finish within {timeout}s: {last}")


def pending_snapshot(base_url: str, token: str) -> dict[str, Any]:
    _, status = request_json(base_url, "GET", "/api/v1/status", token=token, timeout=20)
    _, index_status = request_json(base_url, "GET", "/api/v1/index/status", token=token, timeout=20)
    values = []
    for payload in (status, index_status):
        value = payload.get("pending_embeddings")
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            values.append(int(value))
    return {
        "status": status,
        "index_status": index_status,
        "pending_embeddings": max(values) if values else None,
    }


def wait_embedding_drain(base_url: str, token: str, wait_seconds: int) -> dict[str, Any]:
    deadline = time.monotonic() + wait_seconds
    first = pending_snapshot(base_url, token)
    last = first
    polls = 1
    while True:
        pending = last.get("pending_embeddings")
        if pending is None or pending <= 0:
            return {"first": first, "last": last, "polls": polls}
        if time.monotonic() >= deadline:
            raise RuntimeError(f"embedding queue did not drain within {wait_seconds}s: {last}")
        time.sleep(2.0)
        polls += 1
        last = pending_snapshot(base_url, token)


def ensure_token(base_url: str, password: str) -> str:
    request_json(base_url, "POST", "/api/v1/vault/setup", {"password": password}, timeout=30)
    status, payload = request_json(base_url, "POST", "/api/v1/vault/unlock", {"password": password}, timeout=30)
    if not (200 <= status < 300):
        raise RuntimeError(f"vault unlock failed status={status}: {payload}")
    token = payload.get("token")
    if not isinstance(token, str) or not token:
        raise RuntimeError(f"vault unlock did not return token: {payload}")
    return token


def configure_scheduler(base_url: str, token: str, scheduler_base: str) -> dict[str, Any]:
    body = {
        "llm": {
            "provider": "local_scheduler",
            "endpoint": scheduler_base.rstrip("/"),
            "model": "llm-chat",
            "api_key": "local-scheduler",
        },
        "embedding": {
            "provider": "local_scheduler",
            "endpoint": scheduler_base.rstrip("/"),
            "model": "embedding-int8",
            "task": "kb.query.embed",
            "dims": 512,
        },
        "rerank": {"enabled": True, "task": "kb.query.rerank"},
    }
    status, payload = request_json(base_url, "PATCH", "/api/v1/settings", body, token=token, timeout=60)
    if not (200 <= status < 300):
        raise RuntimeError(f"settings patch failed status={status}: {payload}")
    return payload


def bind_corpus(base_url: str, token: str, bind_dir: str) -> dict[str, Any]:
    body = {
        "path": bind_dir,
        "recursive": True,
        "file_types": ["md", "txt"],
        "corpus_domain": "security",
    }
    status, payload = request_json(base_url, "POST", "/api/v1/index/bind", body, token=token, timeout=180)
    if not (200 <= status < 300):
        raise RuntimeError(f"index bind failed status={status}: {payload}")
    return payload


def expected_source_hits(expected: list[str], text: str) -> list[str]:
    normalized = text.lower()
    hits: list[str] = []
    for source in expected:
        candidates = [source.lower()]
        if source.startswith("generated:security-support-workflow"):
            candidates.extend(["support workflow", "logs, screenshots, topology"])
        if any(candidate in normalized for candidate in candidates):
            hits.append(source)
    return hits


def required_term_present(term: str, answer: str, citation_labels: list[str]) -> bool:
    if term == "引用" and citation_labels:
        return True
    topic_terms = {"security", "access control", "incident response", "risk assessment", "audit evidence"}
    aliases = {
        "security": ["security", "安全", "安全知识库"],
        "access control": ["access control", "访问控制"],
        "incident response": ["incident response", "事件响应"],
        "risk assessment": ["risk assessment", "风险评估"],
        "audit evidence": ["audit evidence", "审计证据"],
        "用户症状": ["用户症状", "故障现象", "症状", "symptom"],
        "日志": ["日志", "审计日志", "logs", "log"],
        "不要编造": ["不要编造", "不编造", "不得编造", "不能编造", "避免编造", "未经支持", "do not invent"],
        "不能直接判定": ["不能直接判定", "不能直接", "无法直接判定", "cannot directly"],
        "证据不足": ["证据不足", "证据不充分", "缺乏", "缺少", "尚缺", "日志缺失", "insufficient"],
        "继续索取": ["继续索取", "补充信息", "补充", "进一步调查", "继续请求", "request"],
        "继续": ["继续", "上一问", "前述", "刚才", "尚缺", "缺少", "后续", "提供的参考资料", "prior"],
        "材料": ["材料", "资料", "信息", "实施细节", "验证方法", "流程说明", "处理流程", "records"],
        "先收集": ["先收集", "优先收集", "先", "before"],
    }
    haystack = answer.lower()
    if term in topic_terms:
        haystack = f"{haystack}\n{json.dumps(citation_labels, ensure_ascii=False).lower()}"
    candidates = aliases.get(term, [term])
    return any(candidate.lower() in haystack for candidate in candidates)


def validate_turn(
    base_url: str,
    token: str,
    turn: dict[str, Any],
    history: list[dict[str, str]],
    chat_timeout: float,
) -> dict[str, Any]:
    message = str(turn.get("message") or "")
    started = time.monotonic()
    search_status, search_payload = request_json(
        base_url,
        "GET",
        f"/api/v1/search?{urllib.parse.urlencode({'q': message, 'top_k': 8})}",
        token=token,
        timeout=60,
    )
    chat_status, chat_payload = request_json(
        base_url,
        "POST",
        "/api/v1/chat",
        {"message": message, "history": history},
        token=token,
        timeout=chat_timeout,
    )
    jid = job_id(chat_payload)
    polls = 0
    if jid:
        job_payload, polls = poll_job(base_url, token, jid, chat_timeout)
        chat_payload = merge_job_payload(chat_payload, job_payload)
    elapsed_ms = round((time.monotonic() - started) * 1000.0, 3)
    answer = response_text(chat_payload)
    labels = citation_labels(chat_payload)
    combined = json.dumps(search_payload, ensure_ascii=False) + "\n" + json.dumps(labels, ensure_ascii=False) + "\n" + answer
    expected = [str(x) for x in turn.get("expected_sources", [])]
    source_hits = expected_source_hits(expected, combined)
    results = search_payload.get("results")
    failures: list[str] = []
    if not (200 <= search_status < 300):
        failures.append(f"search status {search_status}")
    if not isinstance(results, list) or not results:
        failures.append("search returned no results")
    if not (200 <= chat_status < 300):
        failures.append(f"chat status {chat_status}")
    if not answer.strip():
        failures.append("chat returned empty answer")
    if turn.get("requires_citations") and not labels:
        failures.append("chat returned no citation labels")
    missing_sources = [source for source in expected if source not in source_hits]
    if missing_sources:
        failures.append(f"expected sources not found: {missing_sources}")
    for term in turn.get("must_include", []):
        term_s = str(term)
        if not required_term_present(term_s, answer, labels):
            failures.append(f"missing required term: {term_s}")
    answer_lower = answer.lower()
    for term in turn.get("must_not_include", []):
        term_s = str(term)
        if term_s.lower() in answer_lower:
            failures.append(f"forbidden term present: {term_s}")
    budget = turn.get("latency_budget_ms")
    if isinstance(budget, (int, float)) and elapsed_ms > float(budget):
        failures.append(f"latency {elapsed_ms}ms exceeds budget {budget}ms")
    local_scheduler = chat_payload.get("local_scheduler")
    if not isinstance(local_scheduler, dict):
        failures.append("missing local_scheduler metadata")
    elif local_scheduler.get("task") not in {"kb.query.ask", "local.extractive.answer", "local.safety.refusal"}:
        failures.append(f"unexpected local_scheduler task: {local_scheduler.get('task')}")
    history.append({"role": "user", "content": message})
    history.append({"role": "assistant", "content": answer})
    return {
        "turn_id": turn.get("turn_id"),
        "answer_mode": turn.get("answer_mode"),
        "pass": not failures,
        "failures": failures,
        "latency_ms": elapsed_ms,
        "search_results": len(results) if isinstance(results, list) else None,
        "citation_labels": labels[:10],
        "expected_source_hits": source_hits,
        "local_scheduler": local_scheduler,
        "job_id": jid or None,
        "job_polls": polls,
        "answer": answer,
        "answer_excerpt": answer[:600],
    }


def main() -> int:
    args = parse_args()
    if not args.password:
        raise SystemExit("--password, ATTUNE_E2E_PASSWORD, or ATTUNE_VAULT_PW is required")
    scenario = json.loads(Path(args.scenario).read_text(encoding="utf-8"))
    started = time.time()
    report: dict[str, Any] = {
        "schema_version": "attune.release.non_ocr_scale_e2e.v1",
        "started_at_epoch": started,
        "base_url": args.base_url,
        "scheduler_base": args.scheduler_base,
        "bind_dir": args.bind_dir,
        "scenario_id": scenario.get("scenario_id"),
        "domain": scenario.get("domain"),
        "ocr_enabled": False,
        "steps": {},
        "turns": [],
        "failures": [],
    }
    try:
        report["steps"]["health"] = request_json(args.base_url, "GET", "/api/v1/status/health", timeout=20)[1]
        token = ensure_token(args.base_url, args.password)
        report["steps"]["token"] = {"unlocked": True}
        report["steps"]["settings"] = configure_scheduler(args.base_url, token, args.scheduler_base)
        report["steps"]["bind"] = bind_corpus(args.base_url, token, args.bind_dir)
        report["steps"]["embedding_drain"] = wait_embedding_drain(args.base_url, token, args.wait_seconds)
        history: list[dict[str, str]] = []
        for turn in scenario.get("turns", []):
            if not isinstance(turn, dict):
                continue
            result = validate_turn(args.base_url, token, turn, history, args.chat_timeout)
            report["turns"].append(result)
            if not result["pass"]:
                report["failures"].append({"turn_id": result["turn_id"], "failures": result["failures"]})
    except Exception as exc:
        report["failures"].append({"layer": "probe", "reason": str(exc)})
    finally:
        report["finished_at_epoch"] = time.time()
        report["elapsed_ms"] = round((report["finished_at_epoch"] - started) * 1000.0, 3)
        report["pass"] = not report["failures"]
        Path(args.report_out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.report_out).write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"pass": report["pass"], "report": args.report_out, "failures": report["failures"]}, ensure_ascii=False, indent=2))
    return 0 if report["pass"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
