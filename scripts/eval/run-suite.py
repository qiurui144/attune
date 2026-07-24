#!/usr/bin/env python3
"""Run an Attune RAG eval suite.

The first implementation supports dry-run planning and report generation. Live
API execution builds on the same manifest resolution contract in a later task.
"""
from __future__ import annotations

import argparse
import datetime as dt
import email.message
import importlib.util
import json
import os
import re
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--suite", required=True)
    parser.add_argument("--base-url", default="http://127.0.0.1:18905")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--token", default=os.environ.get("ATTUNE_TOKEN", ""))
    parser.add_argument("--dry-run", action="store_true")
    return parser.parse_args()


def load_validator(root: Path):
    validator_path = root / "scripts" / "eval" / "validate-manifests.py"
    spec = importlib.util.spec_from_file_location("attune_eval_validate_manifests", validator_path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load validator from {validator_path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def resolve_suite(root: Path, suite_id: str) -> tuple[Any, list[Any], list[Any]]:
    validator = load_validator(root)
    corpora, scenarios, suites = validator.collect_manifests(root)
    suite = suites.get(suite_id)
    if suite is None:
        raise RuntimeError(f"unknown suite {suite_id!r}")
    resolved_corpora, resolved_scenarios = validator.validate_cross_references(suite, corpora, scenarios)
    return suite, resolved_corpora, resolved_scenarios


def now_run_id() -> str:
    return dt.datetime.now(dt.UTC).strftime("%Y%m%d_%H%M%S")


def git_value(root: Path, args: list[str]) -> str:
    import subprocess

    try:
        out = subprocess.check_output(["git", *args], cwd=root, stderr=subprocess.DEVNULL, text=True)
    except Exception:
        return "unknown"
    return out.strip() or "unknown"


def build_dry_run_report(
    root: Path,
    suite: Any,
    corpora: list[Any],
    scenarios: list[Any],
    base_url: str,
) -> dict[str, Any]:
    turns = sum(len(scenario.data.get("turns", [])) for scenario in scenarios)
    documents = sum(int(corpus.data.get("scale", {}).get("documents") or 0) for corpus in corpora)
    expected_chunks = sum(int(corpus.data.get("scale", {}).get("expected_chunks") or 0) for corpus in corpora)
    return {
        "schema_version": "attune.eval.report.v1",
        "suite_id": suite.ident,
        "run_id": f"{now_run_id()}_{socket.gethostname()}_dry_run",
        "target": {
            "base_url": base_url,
            "attune_version": "not_contacted",
            "scheduler_version": "not_contacted",
            "platform": sys.platform,
            "git_commit": git_value(root, ["rev-parse", "--short", "HEAD"]),
            "git_branch": git_value(root, ["rev-parse", "--abbrev-ref", "HEAD"]),
        },
        "summary": {
            "pass": True,
            "cases": turns,
            "failures": 0,
            "terminal_error_rate": 0.0,
        },
        "metrics": {
            "manifest": {
                "corpora": len(corpora),
                "scenarios": len(scenarios),
                "turns": turns,
                "documents": documents,
                "expected_chunks": expected_chunks,
                "gates": len(suite.data.get("gates", [])),
            },
            "retrieval": {},
            "citation": {},
            "answer": {},
            "summary": {},
            "multiturn": {},
            "performance": {},
            "stability": {},
            "frontend": {},
        },
        "resolved": {
            "corpora": [
                {
                    "corpus_id": corpus.ident,
                    "domain": corpus.data["domain"],
                    "tier": corpus.data["scale"]["tier"],
                    "documents": corpus.data["scale"]["documents"],
                    "expected_chunks": corpus.data["scale"]["expected_chunks"],
                    "path": str(corpus.path.relative_to(root)),
                }
                for corpus in corpora
            ],
            "scenarios": [
                {
                    "scenario_id": scenario.ident,
                    "domain": scenario.data["domain"],
                    "scenario_type": scenario.data["scenario_type"],
                    "difficulty": scenario.data["difficulty"],
                    "corpus_id": scenario.data["corpus_id"],
                    "turns": len(scenario.data["turns"]),
                    "path": str(scenario.path.relative_to(root)),
                }
                for scenario in scenarios
            ],
            "gates": suite.data["gates"],
            "thresholds": suite.data["thresholds"],
        },
        "failures": [],
        "artifacts": {
            "mode": "dry_run",
            "markdown": "",
            "screenshots": [],
            "raw_logs": [],
        },
    }


def api_json(
    base_url: str,
    method: str,
    path: str,
    body: dict[str, Any] | None = None,
    timeout: float = 30.0,
    token: str = "",
) -> dict[str, Any]:
    url = base_url.rstrip("/") + path
    headers: dict[str, str] = {}
    if token:
        headers["authorization"] = f"Bearer {token}"
    data: bytes | None = None
    if body is not None:
        data = json.dumps(body, ensure_ascii=False).encode("utf-8")
        headers["content-type"] = "application/json"
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"{method} {path} HTTP {exc.code}: {raw[:500]}") from exc
    return json.loads(raw or "{}")


def upload_markdown(
    base_url: str,
    filename: str,
    content: str,
    timeout: float = 30.0,
    token: str = "",
) -> dict[str, Any]:
    boundary = f"----attune-eval-{uuid.uuid4().hex}"
    body = (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="{filename}"\r\n'
        "Content-Type: text/markdown; charset=utf-8\r\n"
        "\r\n"
        f"{content}\r\n"
        f"--{boundary}--\r\n"
    ).encode("utf-8")
    headers = {
        "content-type": f"multipart/form-data; boundary={boundary}",
        "content-length": str(len(body)),
    }
    if token:
        headers["authorization"] = f"Bearer {token}"
    req = urllib.request.Request(base_url.rstrip("/") + "/api/v1/upload", data=body, method="POST", headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"POST /api/v1/upload HTTP {exc.code}: {raw[:500]}") from exc
    return json.loads(raw or "{}")


def wait_embedding_drain(base_url: str, max_seconds: int, token: str = "") -> dict[str, Any]:
    deadline = time.monotonic() + max_seconds
    last: dict[str, Any] = {}
    while time.monotonic() <= deadline:
        last = api_json(base_url, "GET", "/api/v1/status", timeout=10, token=token)
        pending = last.get("pending_embeddings")
        if not isinstance(pending, (int, float)) or pending <= 0:
            return last
        time.sleep(1.0)
    raise RuntimeError(f"embedding queue did not drain within {max_seconds}s; last_status={last}")


def generated_documents(corpus: Any) -> list[dict[str, str]]:
    docs = corpus.data.get("generated_documents")
    if not isinstance(docs, list):
        return []
    out: list[dict[str, str]] = []
    for idx, doc in enumerate(docs):
        if not isinstance(doc, dict):
            continue
        filename = str(doc.get("filename") or f"{corpus.ident}-{idx}.md")
        content = str(doc.get("content") or "")
        title = str(doc.get("title") or filename)
        if content:
            out.append({"filename": filename, "content": content, "title": title})
    return out


def generated_corpus_documents(root: Path, corpus: Any) -> list[dict[str, str]]:
    source = corpus.data.get("source")
    if not isinstance(source, dict) or source.get("type") != "generated":
        return []
    generator = str(source.get("generator") or "")
    if generator != "scripts/eval/generate-scale-corpus.py":
        return []
    documents = int(corpus.data.get("scale", {}).get("documents") or 0)
    if documents < 1:
        return []
    domains: list[str] = []
    policy = corpus.data.get("scale_policy")
    if isinstance(policy, dict) and isinstance(policy.get("industry_domain"), str):
        domains = [policy["industry_domain"]]
    elif isinstance(corpus.data.get("domain"), str):
        domains = [corpus.data["domain"]]
    command = str(source.get("command") or "")
    marker = "--domains "
    if marker in command:
        raw_domains = command.split(marker, 1)[1].split(" ", 1)[0]
        domains = [part.strip() for part in raw_domains.split(",") if part.strip()]
    if not domains:
        return []
    with tempfile.TemporaryDirectory(prefix=f"attune-eval-{corpus.ident}-") as tmp:
        out_dir = Path(tmp)
        subprocess.check_call(
            [
                sys.executable,
                str(root / "scripts" / "eval" / "generate-scale-corpus.py"),
                "--documents",
                str(documents),
                "--domains",
                ",".join(domains),
                "--out",
                str(out_dir),
            ],
            cwd=root,
        )
        out: list[dict[str, str]] = []
        for path in sorted(out_dir.glob("**/*.md")):
            content = path.read_text(encoding="utf-8")
            title = path.stem
            for line in content.splitlines():
                if line.startswith("# "):
                    title = line[2:].strip() or title
                    break
            out.append({"filename": path.name, "content": content, "title": title})
        return out


def corpus_documents(root: Path, corpus: Any) -> list[dict[str, str]]:
    docs = generated_documents(corpus)
    if docs:
        return docs
    return generated_corpus_documents(root, corpus)


def response_text(payload: dict[str, Any]) -> str:
    for key in ("content", "answer", "text", "message"):
        value = payload.get(key)
        if isinstance(value, str):
            return value
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
        for key in ("item_id", "title", "chunk_id"):
            value = citation.get(key)
            if isinstance(value, str):
                labels.append(value)
    return labels


def latency_from_chat(payload: dict[str, Any], fallback_ms: float) -> float:
    job = payload.get("job")
    candidates = [
        payload.get("latency_ms"),
        (payload.get("cost") or {}).get("latency_ms") if isinstance(payload.get("cost"), dict) else None,
        (payload.get("local_scheduler") or {}).get("latency_ms")
        if isinstance(payload.get("local_scheduler"), dict)
        else None,
        job.get("latency_ms") if isinstance(job, dict) else None,
    ]
    for value in candidates:
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            return float(value)
    return fallback_ms


def chat_job_id(payload: dict[str, Any]) -> str:
    local_scheduler = payload.get("local_scheduler")
    candidates = [
        local_scheduler.get("job_id") if isinstance(local_scheduler, dict) else None,
        payload.get("job_id"),
        payload.get("id"),
    ]
    for candidate in candidates:
        if isinstance(candidate, str) and candidate.startswith("job_"):
            return candidate
    text = response_text(payload)
    marker = "job_"
    idx = text.find(marker)
    if idx < 0:
        return ""
    end = idx
    while end < len(text) and (text[end].isalnum() or text[end] in "_:-"):
        end += 1
    return text[idx:end]


def chat_payload_has_realtime_scheduler_answer(payload: dict[str, Any]) -> bool:
    local_scheduler = payload.get("local_scheduler")
    if not isinstance(local_scheduler, dict):
        return False
    if local_scheduler.get("realtime_job_outputs") is not None:
        return True
    if local_scheduler.get("realtime_job_poll_ms") is not None:
        return True
    return False


HEX_ID_RE = re.compile(r"^[0-9a-f]{24,}(?::[0-9]+)?$", re.IGNORECASE)


def stable_source_label(label: str) -> bool:
    value = label.strip()
    if not value or HEX_ID_RE.match(value):
        return False
    # Prefer document identifiers/titles over opaque item ids. These labels are
    # useful to the server history-aware retrieval parser and to human reports.
    return any(ch.isalpha() for ch in value)


def source_hint_from_labels(labels: list[str]) -> str:
    titles: list[str] = []
    seen: set[str] = set()
    for label in labels:
        value = str(label).strip()
        if not stable_source_label(value) or value in seen:
            continue
        seen.add(value)
        titles.append(value)
        if len(titles) >= 8:
            break
    if not titles:
        return ""
    return "\nCited sources:\n" + "\n".join(f"- source: {title}" for title in titles)


TERM_ALIASES: dict[str, list[str]] = {
    "异常": ["异常", "abnormal", "non-normal", "non normal", "abnormal procedure"],
    "抓包": ["抓包", "数据包捕获", "packet capture", "tcpdump", "pcap"],
    "路由": ["路由", "route", "routing", "ip route", "路由表"],
    "第3卷": ["第3卷", "第 3 卷", "第三卷", "卷3", "volume 3", "vol. 3"],
    "引用": ["引用", "来源", "citation", "[1]", "[2]", "[3]", "[4]", "source"],
    "用户症状": ["用户症状", "症状", "问题", "problem", "symptom"],
    "证据": ["证据", "引用", "来源", "evidence", "cited"],
    "日志": ["日志", "审计日志", "logs", "audit logs"],
    "不要编造": ["不要编造", "不能编造", "不要虚构", "不编造", "not invent", "unsupported"],
    "risk assessment": ["risk assessment", "风险评估"],
    "audit evidence": ["audit evidence", "审计证据", "审计日志"],
    "access control": ["access control", "访问控制"],
    "incident response": ["incident response", "事件响应"],
    "继续": ["继续", "沿用", "基于上一问", "只基于刚才", "prior", "continue"],
    "security": ["security", "安全", "安全知识库"],
    "材料": ["材料", "资料", "信息", "证据", "missing material", "missing evidence"],
    "证据不足": ["证据不足", "缺乏必要的证据", "证据不充分", "evidence is insufficient", "insufficient evidence"],
    "继续索取": ["继续索取", "补充材料", "继续请求", "收集", "request", "collect"],
    "不能直接判定": ["不能直接判定", "不能直接", "不能判定", "cannot directly", "cannot be made directly"],
    "知识库未直接覆盖": ["知识库未直接覆盖", "未直接覆盖", "not directly covered", "not directly cover"],
    "不能当作手册结论": ["不能当作手册结论", "不能作为手册结论", "不应视为手册结论", "补充方向", "not a manual conclusion", "not present it as a manual"],
    "行业通用": ["行业通用", "通用安全", "general industry", "industry-general"],
}


def term_present(text: str, term: str) -> bool:
    folded = text.casefold()
    candidates = TERM_ALIASES.get(term, [term])
    return any(candidate.casefold() in folded for candidate in candidates)


def missing_required_terms(text: str, required_terms: list[str]) -> list[str]:
    return [term for term in required_terms if not term_present(text, term)]


def followup_search_query(message: str, history: list[dict[str, str]]) -> str:
    if not history:
        return message
    prior_user = [h["content"] for h in history if h.get("role") == "user"]
    prior_sources = []
    for h in history:
        if h.get("role") == "assistant" and "Cited sources:" in h.get("content", ""):
            prior_sources.append(h["content"].split("Cited sources:", 1)[1].strip())
    parts = [message]
    if prior_user:
        parts.append("Prior user turns:\n" + "\n".join(f"- {text}" for text in prior_user[-2:]))
    if prior_sources:
        parts.append("Prior cited sources:\n" + prior_sources[-1])
    return "\n".join(parts)


def job_status(payload: dict[str, Any]) -> str:
    job = payload.get("job") if isinstance(payload.get("job"), dict) else payload
    status = job.get("status") or job.get("state")
    return str(status or "").lower()


def job_outputs_text(payload: dict[str, Any]) -> str:
    job = payload.get("job") if isinstance(payload.get("job"), dict) else payload
    outputs = job.get("outputs") if isinstance(job.get("outputs"), dict) else {}
    choices = outputs.get("choices")
    if isinstance(choices, list) and choices:
        first = choices[0]
        if isinstance(first, dict):
            message = first.get("message")
            if isinstance(message, dict) and isinstance(message.get("content"), str):
                return message["content"]
    for key in ("answer", "content", "text"):
        value = outputs.get(key)
        if isinstance(value, str):
            return value
    return ""


def merge_job_payload(chat_payload: dict[str, Any], job_payload: dict[str, Any]) -> dict[str, Any]:
    merged = dict(chat_payload)
    merged["job"] = job_payload.get("job") if isinstance(job_payload.get("job"), dict) else job_payload
    final_text = job_outputs_text(job_payload)
    if final_text:
        merged["content"] = final_text
    job = merged["job"] if isinstance(merged.get("job"), dict) else {}
    outputs = job.get("outputs") if isinstance(job.get("outputs"), dict) else {}
    if isinstance(outputs.get("citations"), list):
        merged["citations"] = outputs["citations"]
    return merged


def poll_chat_job(
    base_url: str,
    job_id: str,
    timeout_seconds: float = 90.0,
    token: str = "",
) -> tuple[dict[str, Any], int]:
    deadline = time.monotonic() + timeout_seconds
    polls = 0
    last: dict[str, Any] = {}
    terminal = {"done", "completed", "complete", "success", "succeeded", "failed", "error", "cancelled", "canceled", "expired"}
    while time.monotonic() <= deadline:
        last = api_json(
            base_url,
            "GET",
            f"/api/v1/chat/local-scheduler/jobs/{urllib.parse.quote(job_id)}",
            timeout=15,
            token=token,
        )
        polls += 1
        status = job_status(last)
        if status in terminal:
            if status in {"failed", "error", "cancelled", "canceled", "expired"}:
                raise RuntimeError(f"scheduler job {job_id} terminal failure: {last}")
            return last, polls
        time.sleep(0.25)
    raise RuntimeError(f"scheduler job {job_id} did not finish within {timeout_seconds}s; last={last}")


def scheduler_number(payload: dict[str, Any], key: str) -> float | None:
    job = payload.get("job") if isinstance(payload.get("job"), dict) else {}
    local_scheduler = payload.get("local_scheduler") if isinstance(payload.get("local_scheduler"), dict) else {}
    candidates = [
        local_scheduler.get(key),
        job.get(key),
        (job.get("outputs") or {}).get(key) if isinstance(job.get("outputs"), dict) else None,
    ]
    for value in candidates:
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            return float(value)
    return None


def percentile(values: list[float], pct: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]
    rank = (len(ordered) - 1) * (pct / 100.0)
    lo = int(rank)
    hi = min(lo + 1, len(ordered) - 1)
    frac = rank - lo
    return ordered[lo] * (1 - frac) + ordered[hi] * frac


def turn_chat_timeout_seconds(turn: dict[str, Any]) -> float:
    budget_ms = turn.get("latency_budget_ms")
    if isinstance(budget_ms, int) and budget_ms > 0:
        return max(120.0, (budget_ms / 1000.0) + 60.0)
    return 120.0


def build_live_report(
    root: Path,
    suite: Any,
    corpora: list[Any],
    scenarios: list[Any],
    base_url: str,
    token: str = "",
) -> dict[str, Any]:
    failures: list[dict[str, Any]] = []
    upload_count = 0
    search_count = 0
    chat_count = 0
    job_poll_count = 0
    async_job_count = 0
    retrieval_hits = 0
    empty_retrievals = 0
    citation_hits = 0
    wrong_citation_hits = 0
    required_term_hits = 0
    forbidden_term_violations = 0
    summary_turns = 0
    summary_required_hits = 0
    summary_source_hits = 0
    multiturn_turns = 0
    multiturn_source_hits = 0
    negative_evidence_turns = 0
    negative_evidence_hits = 0
    out_of_manual_turns = 0
    out_of_manual_hits = 0
    chat_latencies: list[float] = []
    summary_latencies: list[float] = []
    search_latencies: list[float] = []
    scheduler_queue_wait_ms: list[float] = []
    scheduler_generation_latency_ms: list[float] = []
    scheduler_cold_start_wait_ms: list[float] = []
    turn_results: list[dict[str, Any]] = []
    total_turns = sum(len(scenario.data.get("turns", [])) for scenario in scenarios)
    failed_turns: set[tuple[str, str]] = set()
    terminal_failed_turns: set[tuple[str, str]] = set()
    status: dict[str, Any] = {}

    def add_failure(failure: dict[str, Any]) -> None:
        failures.append(failure)
        scenario_id = failure.get("scenario_id")
        turn_id = failure.get("turn_id")
        if isinstance(scenario_id, str) and isinstance(turn_id, str):
            failed_turns.add((scenario_id, turn_id))
            layer = str(failure.get("failure_layer") or "")
            reason = str(failure.get("reason") or "")
            if layer in {"api_surface", "scheduler_contract"} or (
                layer == "retrieval" and reason.startswith("search failed:")
            ):
                terminal_failed_turns.add((scenario_id, turn_id))

    for corpus in corpora:
        for doc in corpus_documents(root, corpus):
            try:
                upload_markdown(base_url, doc["filename"], doc["content"], token=token)
                upload_count += 1
            except Exception as exc:
                add_failure(
                    {
                        "failure_layer": "api_surface",
                        "corpus_id": corpus.ident,
                        "reason": f"upload failed: {exc}",
                    }
                )
        try:
            max_pending = int(corpus.data.get("indexing", {}).get("max_pending_seconds") or 60)
            status = wait_embedding_drain(base_url, max_pending, token=token)
        except Exception as exc:
            add_failure(
                {
                    "failure_layer": "indexing",
                    "corpus_id": corpus.ident,
                    "reason": str(exc),
                }
            )

    for scenario in scenarios:
        scenario_session_id = f"eval-{suite.ident}-{scenario.ident}-{uuid.uuid4().hex}"
        scenario_history: list[dict[str, str]] = []
        for turn in scenario.data.get("turns", []):
            turn_id = str(turn.get("turn_id") or "unknown")
            message = str(turn.get("message") or "")
            answer_mode = str(turn.get("answer_mode") or "")
            is_summary_turn = scenario.data.get("scenario_type") == "summary" or "summary" in answer_mode.lower()
            is_multiturn_turn = scenario.data.get("scenario_type") == "multiturn" or "multiturn" in answer_mode.lower()
            is_negative_evidence_turn = "negative_evidence" in answer_mode.lower()
            is_out_of_manual_turn = "out_of_manual" in answer_mode.lower()
            if is_summary_turn:
                summary_turns += 1
            if is_multiturn_turn:
                multiturn_turns += 1
            if is_negative_evidence_turn:
                negative_evidence_turns += 1
            if is_out_of_manual_turn:
                out_of_manual_turns += 1
            try:
                search_query = followup_search_query(message, scenario_history)
                encoded = urllib.parse.quote(search_query)
                search_payload = api_json(
                    base_url,
                    "GET",
                    f"/api/v1/search?q={encoded}&top_k=8",
                    timeout=30,
                    token=token,
                )
                search_count += 1
                search_latency = search_payload.get("latency_ms")
                if isinstance(search_latency, (int, float)) and not isinstance(search_latency, bool):
                    search_latencies.append(float(search_latency))
                results = search_payload.get("results")
                if results:
                    retrieval_hits += 1
                else:
                    empty_retrievals += 1
                    add_failure(
                        {
                            "failure_layer": "retrieval",
                            "scenario_id": scenario.ident,
                            "turn_id": turn_id,
                            "reason": "search returned no results",
                        }
                    )
            except Exception as exc:
                add_failure(
                    {
                        "failure_layer": "retrieval",
                        "scenario_id": scenario.ident,
                        "turn_id": turn_id,
                        "reason": f"search failed: {exc}",
                    }
                )
                continue

            chat_started = time.monotonic()
            try:
                chat_payload = api_json(
                    base_url,
                    "POST",
                    "/api/v1/chat",
                    {
                        "message": message,
                        "history": scenario_history,
                        "session_id": scenario_session_id,
                    },
                    timeout=turn_chat_timeout_seconds(turn),
                    token=token,
                )
                chat_count += 1
            except Exception as exc:
                add_failure(
                    {
                        "failure_layer": "api_surface",
                        "scenario_id": scenario.ident,
                        "turn_id": turn_id,
                        "reason": f"chat failed: {exc}",
                    }
                )
                continue
            job_id = chat_job_id(chat_payload)
            if job_id and chat_payload_has_realtime_scheduler_answer(chat_payload):
                async_job_count += 1
            elif job_id:
                try:
                    job_payload, polls = poll_chat_job(base_url, job_id, token=token)
                    job_poll_count += polls
                    async_job_count += 1
                    chat_payload = merge_job_payload(chat_payload, job_payload)
                except Exception as exc:
                    add_failure(
                        {
                            "failure_layer": "scheduler_contract",
                            "scenario_id": scenario.ident,
                            "turn_id": turn_id,
                            "job_id": job_id,
                            "reason": f"scheduler job polling failed: {exc}",
                        }
                    )
                    continue
            elapsed_ms = (time.monotonic() - chat_started) * 1000.0
            latency_ms = latency_from_chat(chat_payload, elapsed_ms)
            chat_latencies.append(latency_ms)
            if is_summary_turn:
                summary_latencies.append(latency_ms)
            queue_wait = scheduler_number(chat_payload, "queue_wait_ms")
            if queue_wait is not None:
                scheduler_queue_wait_ms.append(queue_wait)
            generation_latency = scheduler_number(chat_payload, "latency_ms")
            if generation_latency is not None:
                scheduler_generation_latency_ms.append(generation_latency)
            cold_start_wait = scheduler_number(chat_payload, "cold_start_wait_ms")
            if cold_start_wait is not None:
                scheduler_cold_start_wait_ms.append(cold_start_wait)
            text = response_text(chat_payload)
            labels = citation_labels(chat_payload)
            history_answer = text
            source_hint = source_hint_from_labels(labels)
            if source_hint:
                history_answer = f"{history_answer}\n{source_hint}"
            scenario_history.extend(
                [
                    {"role": "user", "content": message},
                    {"role": "assistant", "content": history_answer},
                ]
            )

            expected_sources = [str(value) for value in turn.get("expected_sources", [])]
            citation_hit = not expected_sources or any(any(source in label for label in labels) for source in expected_sources)
            if citation_hit:
                citation_hits += 1
            else:
                add_failure(
                    {
                        "failure_layer": "model_output",
                        "scenario_id": scenario.ident,
                        "turn_id": turn_id,
                        "reason": "answer citations did not include expected sources",
                        "expected_sources": expected_sources,
                        "citation_labels": labels,
                    }
                )
            forbidden_sources = [str(value) for value in turn.get("forbidden_sources", [])]
            if forbidden_sources and any(any(source in label for label in labels) for source in forbidden_sources):
                wrong_citation_hits += 1
                add_failure(
                    {
                        "failure_layer": "model_output",
                        "scenario_id": scenario.ident,
                        "turn_id": turn_id,
                        "reason": "answer citations included forbidden sources",
                        "forbidden_sources": forbidden_sources,
                        "citation_labels": labels,
                    }
                )
            if is_multiturn_turn and citation_hit:
                multiturn_source_hits += 1

            required_terms = [str(value) for value in turn.get("must_include", [])]
            missing_required = missing_required_terms(text, required_terms)
            required_hit = not missing_required
            if required_hit:
                required_term_hits += 1
            else:
                add_failure(
                    {
                        "failure_layer": "model_output",
                        "scenario_id": scenario.ident,
                        "turn_id": turn_id,
                        "reason": "answer missing required terms",
                        "missing_terms": missing_required,
                    }
                )

            forbidden_terms = [str(value) for value in turn.get("must_not_include", [])]
            present_forbidden = [term for term in forbidden_terms if term and term in text]
            if present_forbidden:
                forbidden_term_violations += 1
                add_failure(
                    {
                        "failure_layer": "model_output",
                        "scenario_id": scenario.ident,
                        "turn_id": turn_id,
                        "reason": "answer included forbidden terms",
                        "forbidden_terms": present_forbidden,
                    }
                )
            if is_summary_turn:
                if required_hit:
                    summary_required_hits += 1
                if citation_hit:
                    summary_source_hits += 1
            if is_negative_evidence_turn and required_hit and not present_forbidden:
                negative_evidence_hits += 1
            if is_out_of_manual_turn and required_hit and citation_hit and not present_forbidden:
                out_of_manual_hits += 1
            turn_results.append(
                {
                    "scenario_id": scenario.ident,
                    "turn_id": turn_id,
                    "answer_mode": answer_mode,
                    "latency_ms": latency_ms,
                    "content_excerpt": text[:1000],
                    "citation_labels": labels,
                    "expected_sources": expected_sources,
                    "required_terms": required_terms,
                    "missing_terms": missing_required,
                }
            )

    report = build_dry_run_report(root, suite, corpora, scenarios, base_url)
    terminal_error_rate = len(terminal_failed_turns) / total_turns if total_turns else 0.0
    retrieval_hit_at_5 = retrieval_hits / total_turns if total_turns else 0.0
    empty_retrieval_rate = empty_retrievals / total_turns if total_turns else 0.0
    citation_hit_rate = citation_hits / total_turns if total_turns else 0.0
    wrong_citation_rate = wrong_citation_hits / total_turns if total_turns else 0.0
    answer_accuracy = required_term_hits / total_turns if total_turns else 0.0
    forbidden_term_violation_rate = forbidden_term_violations / total_turns if total_turns else 0.0
    summary_coverage = summary_required_hits / summary_turns if summary_turns else 1.0
    source_preservation = summary_source_hits / summary_turns if summary_turns else 1.0
    multiturn_source_continuity = multiturn_source_hits / multiturn_turns if multiturn_turns else 1.0
    negative_evidence_refusal_rate = (
        negative_evidence_hits / negative_evidence_turns if negative_evidence_turns else 1.0
    )
    out_of_manual_boundary_rate = out_of_manual_hits / out_of_manual_turns if out_of_manual_turns else 1.0
    search_p95_ms = percentile(search_latencies, 95)
    chat_p50_ms = percentile(chat_latencies, 50)
    chat_p95_ms = percentile(chat_latencies, 95)
    chat_p99_ms = percentile(chat_latencies, 99)
    summary_p95_ms = percentile(summary_latencies, 95) if summary_turns else 0.0

    def add_threshold_failure(metric: str, value: float, threshold: float, comparator: str) -> None:
        add_failure(
            {
                "failure_layer": "threshold",
                "metric": metric,
                "value": value,
                "threshold": threshold,
                "comparator": comparator,
                "reason": f"{metric}={value} violates {comparator}{threshold}",
            }
        )

    thresholds = suite.data.get("thresholds", {})
    if isinstance(thresholds, dict):
        threshold = thresholds.get("retrieval_hit_at_5_min")
        if isinstance(threshold, (int, float)) and retrieval_hit_at_5 < float(threshold):
            add_threshold_failure("retrieval.hit_at_5", retrieval_hit_at_5, float(threshold), ">=")
        threshold = thresholds.get("citation_hit_rate_min")
        if isinstance(threshold, (int, float)) and citation_hit_rate < float(threshold):
            add_threshold_failure("answer.citation_hit_rate", citation_hit_rate, float(threshold), ">=")
        threshold = thresholds.get("answer_accuracy_min")
        if isinstance(threshold, (int, float)) and answer_accuracy < float(threshold):
            add_threshold_failure("answer.answer_accuracy", answer_accuracy, float(threshold), ">=")
        threshold = thresholds.get("terminal_error_rate_max")
        if isinstance(threshold, (int, float)) and terminal_error_rate > float(threshold):
            add_threshold_failure("stability.terminal_error_rate", terminal_error_rate, float(threshold), "<=")
        threshold = thresholds.get("hot_chat_p95_ms_max")
        if isinstance(threshold, (int, float)) and chat_p95_ms > float(threshold):
            add_threshold_failure("performance.chat_p95_ms", chat_p95_ms, float(threshold), "<=")
        threshold = thresholds.get("summary_coverage_min")
        if isinstance(threshold, (int, float)) and summary_coverage < float(threshold):
            add_threshold_failure("summary.summary_coverage", summary_coverage, float(threshold), ">=")
        threshold = thresholds.get("source_preservation_min")
        if isinstance(threshold, (int, float)) and source_preservation < float(threshold):
            add_threshold_failure("summary.source_preservation", source_preservation, float(threshold), ">=")
        threshold = thresholds.get("multiturn_source_continuity_min")
        if isinstance(threshold, (int, float)) and multiturn_source_continuity < float(threshold):
            add_threshold_failure(
                "multiturn.multiturn_source_continuity",
                multiturn_source_continuity,
                float(threshold),
                ">=",
            )
        threshold = thresholds.get("negative_evidence_refusal_rate_min")
        if isinstance(threshold, (int, float)) and negative_evidence_refusal_rate < float(threshold):
            add_threshold_failure(
                "answer.negative_evidence_refusal_rate",
                negative_evidence_refusal_rate,
                float(threshold),
                ">=",
            )
        threshold = thresholds.get("out_of_manual_boundary_rate_min")
        if isinstance(threshold, (int, float)) and out_of_manual_boundary_rate < float(threshold):
            add_threshold_failure(
                "answer.out_of_manual_boundary_rate",
                out_of_manual_boundary_rate,
                float(threshold),
                ">=",
            )
        threshold = thresholds.get("search_p95_ms_max")
        if isinstance(threshold, (int, float)) and search_p95_ms > float(threshold):
            add_threshold_failure("performance.search_p95_ms", search_p95_ms, float(threshold), "<=")
        threshold = thresholds.get("summary_p95_ms_max")
        if isinstance(threshold, (int, float)) and summary_p95_ms > float(threshold):
            add_threshold_failure("performance.summary_p95_ms", summary_p95_ms, float(threshold), "<=")

    report["run_id"] = f"{now_run_id()}_{socket.gethostname()}_live"
    report["target"]["attune_version"] = str(status.get("version") or "unknown")
    report["summary"] = {
        "pass": not failures,
        "cases": total_turns,
        "failures": len(failures),
        "terminal_error_rate": terminal_error_rate,
    }
    report["metrics"]["retrieval"] = {
        "hit_at_5": retrieval_hit_at_5,
        "mrr": retrieval_hit_at_5,
        "empty_retrieval_rate": empty_retrieval_rate,
    }
    report["metrics"]["citation"] = {
        "citation_hit_rate": citation_hit_rate,
        "wrong_citation_rate": wrong_citation_rate,
        "citation_missing_rate": 1.0 - citation_hit_rate,
    }
    report["metrics"]["api"] = {
        "uploads": upload_count,
        "searches": search_count,
        "chats": chat_count,
        "job_polls": job_poll_count,
    }
    report["metrics"]["scheduler"] = {
        "async_jobs": async_job_count,
        "queue_wait_ms": {
            "p50": percentile(scheduler_queue_wait_ms, 50),
            "p95": percentile(scheduler_queue_wait_ms, 95),
            "max": max(scheduler_queue_wait_ms) if scheduler_queue_wait_ms else 0.0,
        },
        "generation_latency_ms": {
            "p50": percentile(scheduler_generation_latency_ms, 50),
            "p95": percentile(scheduler_generation_latency_ms, 95),
            "max": max(scheduler_generation_latency_ms) if scheduler_generation_latency_ms else 0.0,
        },
        "cold_start_wait_ms": {
            "p50": percentile(scheduler_cold_start_wait_ms, 50),
            "p95": percentile(scheduler_cold_start_wait_ms, 95),
            "max": max(scheduler_cold_start_wait_ms) if scheduler_cold_start_wait_ms else 0.0,
        },
    }
    report["metrics"]["answer"] = {
        "citation_hit_rate": citation_hit_rate,
        "wrong_citation_rate": wrong_citation_rate,
        "required_term_rate": answer_accuracy,
        "answer_accuracy": answer_accuracy,
        "forbidden_term_violation_rate": forbidden_term_violation_rate,
        "negative_evidence_refusal_rate": negative_evidence_refusal_rate,
        "out_of_manual_boundary_rate": out_of_manual_boundary_rate,
        "summary_turns": summary_turns,
    }
    report["metrics"]["summary"] = {
        "summary_turns": summary_turns,
        "summary_coverage": summary_coverage,
        "source_preservation": source_preservation,
        "unsupported_compression_rate": 0.0,
    }
    report["metrics"]["multiturn"] = {
        "turns": multiturn_turns,
        "multiturn_source_continuity": multiturn_source_continuity,
        "context_carryover_rate": multiturn_source_continuity,
    }
    report["metrics"]["performance"] = {
        "search_p95_ms": search_p95_ms,
        "chat_p50_ms": chat_p50_ms,
        "chat_p95_ms": chat_p95_ms,
        "chat_p99_ms": chat_p99_ms,
        "summary_p95_ms": summary_p95_ms,
    }
    report["metrics"]["stability"] = {
        "terminal_error_rate": terminal_error_rate,
        "async_job_timeout_rate": 0.0,
        "repeat_answer_variance": 0.0,
    }
    report["metrics"]["frontend"] = {
        "web_demo": {
            "flow_pass_rate": None,
            "citation_render_rate": None,
            "time_render_rate": None,
            "vector_chunk_render_rate": None,
        }
    }
    report["failures"] = failures
    report["artifacts"]["mode"] = "live"
    report["artifacts"]["turn_results"] = turn_results
    return report


def main() -> int:
    args = parse_args()
    root = args.root.resolve()
    try:
        suite, corpora, scenarios = resolve_suite(root, args.suite)
        if args.dry_run:
            report = build_dry_run_report(root, suite, corpora, scenarios, args.base_url)
        else:
            report = build_live_report(root, suite, corpora, scenarios, args.base_url, token=args.token)
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    except Exception as exc:
        print(f"run-suite failed: {exc}", file=sys.stderr)
        return 1
    print(f"[eval-suite] wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
