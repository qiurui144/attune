#!/usr/bin/env python3
"""Strict NAS Web API contract probe for installed Attune server packages.

This probe is intentionally HTTP-only from the runner perspective. Any path
submitted to /api/v1/index/bind must already exist on the Attune server host;
the K3 release wrapper creates that server-side fixture over SSH before calling
this script.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any


CORE_READ_ENDPOINTS = [
    ("status", "/api/v1/status", {"state", "items", "pending_embeddings"}),
    ("diagnostics", "/api/v1/status/diagnostics", {"vault_state", "ai_status", "scheduler"}),
    ("ai_stack", "/api/v1/ai-stack", set()),
    ("index_status", "/api/v1/index/status", set()),
    ("items", "/api/v1/items", set()),
    ("background_status", "/api/v1/background/status", set()),
    ("member_state", "/api/v1/member/state", set()),
    ("member_locks", "/api/v1/member/locks", set()),
    ("privacy_status", "/api/v1/privacy/status", set()),
    ("privacy_tier", "/api/v1/privacy/tier", set()),
    ("plugins", "/api/v1/plugins", set()),
    ("marketplace_plugins", "/api/v1/marketplace/plugins", set()),
    ("skills", "/api/v1/skills", set()),
    ("skill_runtime_skills", "/api/v1/skill-runtime/skills", set()),
    ("projects", "/api/v1/projects", set()),
    ("tags", "/api/v1/tags", set()),
    ("clusters", "/api/v1/clusters", set()),
    ("jobs", "/api/v1/jobs", set()),
    ("folder_links", "/api/v1/folder-links", set()),
    ("audit_outbound", "/api/v1/audit/outbound", set()),
    ("audit_log", "/api/v1/audit/log", set()),
    ("web_search_cache", "/api/v1/web-search-cache", set()),
    ("suggestions", "/api/v1/suggestions", set()),
    ("accounts", "/api/v1/accounts", set()),
    ("scenarios", "/api/v1/scenarios", set()),
    ("diagnostics_capabilities", "/api/v1/diagnostics/capabilities", set()),
]

TERMINAL_JOB_STATUSES = {"done", "completed", "complete", "success", "succeeded", "failed", "error", "cancelled", "canceled", "expired"}
FAILED_JOB_STATUSES = {"failed", "error", "cancelled", "canceled", "expired"}


class ProbeError(RuntimeError):
    pass


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--password", default=os.environ.get("ATTUNE_E2E_PASSWORD", os.environ.get("ATTUNE_VAULT_PW", "")))
    parser.add_argument("--token", default="")
    parser.add_argument("--bind-dir", default="")
    parser.add_argument("--scheduler-url", default="")
    parser.add_argument("--server-scheduler-base", default="http://127.0.0.1:8090")
    parser.add_argument("--scheduler-chat-model", default="llm-chat")
    parser.add_argument("--timeout", type=float, default=60.0)
    parser.add_argument("--job-timeout", type=float, default=90.0)
    parser.add_argument("--require-scheduler-chat", action="store_true")
    parser.add_argument("--out", type=Path, default=None)
    parser.add_argument("--dry-run", action="store_true")
    return parser.parse_args()


def normalize_base(raw: str) -> str:
    base = raw.strip().rstrip("/")
    if not base:
        raise ProbeError("base URL is empty")
    return base


def normalize_scheduler_base(raw: str) -> str:
    base = raw.strip().rstrip("/")
    if base.endswith("/v1"):
        base = base[:-3].rstrip("/")
    if not base:
        raise ProbeError("server scheduler base is empty")
    return base


def auth_headers(token: str = "", content_type: bool = False) -> dict[str, str]:
    headers: dict[str, str] = {}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if content_type:
        headers["Content-Type"] = "application/json"
    return headers


def parse_json(raw: bytes) -> Any:
    text = raw.decode(errors="replace")
    if not text:
        return {}
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return {"raw": text[:1000]}


def request_json(
    base_url: str,
    method: str,
    path: str,
    *,
    body: dict[str, Any] | None = None,
    token: str = "",
    timeout: float = 60.0,
    allow_statuses: set[int] | None = None,
) -> tuple[int, Any]:
    allow_statuses = allow_statuses or set()
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        f"{base_url}{path}",
        data=data,
        headers=auth_headers(token, content_type=body is not None),
        method=method,
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, parse_json(resp.read())
    except urllib.error.HTTPError as exc:
        payload = parse_json(exc.read())
        if exc.code in allow_statuses:
            return exc.code, payload
        raise ProbeError(f"{method} {path} failed HTTP {exc.code}: {payload}") from exc
    except Exception as exc:  # noqa: BLE001
        raise ProbeError(f"{method} {path} failed: {exc}") from exc


def request_raw(
    base_url: str,
    method: str,
    path: str,
    *,
    body: dict[str, Any] | None = None,
    token: str = "",
    timeout: float = 60.0,
) -> tuple[int, dict[str, str], bytes]:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        f"{base_url}{path}",
        data=data,
        headers=auth_headers(token, content_type=body is not None),
        method=method,
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, {k.lower(): v for k, v in resp.headers.items()}, resp.read()
    except urllib.error.HTTPError as exc:
        raise ProbeError(f"{method} {path} failed HTTP {exc.code}: {exc.read().decode(errors='replace')[:500]}") from exc


def request_multipart_upload(
    base_url: str,
    token: str,
    filename: str,
    content: str,
    timeout: float,
) -> tuple[int, Any]:
    boundary = f"attune-nas-web-contract-{int(time.time() * 1000)}"
    payload = (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="{filename}"\r\n'
        "Content-Type: text/markdown\r\n\r\n"
        f"{content}\r\n"
        f"--{boundary}--\r\n"
    ).encode()
    req = urllib.request.Request(
        f"{base_url}/api/v1/upload",
        data=payload,
        headers={
            **auth_headers(token),
            "Content-Type": f"multipart/form-data; boundary={boundary}",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, parse_json(resp.read())
    except urllib.error.HTTPError as exc:
        raise ProbeError(f"POST /api/v1/upload failed HTTP {exc.code}: {parse_json(exc.read())}") from exc


def request_multipart_voice_upload(
    base_url: str,
    token: str,
    filename: str,
    content: bytes,
    timeout: float,
    allow_statuses: set[int] | None = None,
) -> tuple[int, Any]:
    allow_statuses = allow_statuses or set()
    boundary = f"attune-voice-contract-{int(time.time() * 1000)}"
    payload = b"".join(
        [
            f"--{boundary}\r\n".encode(),
            f'Content-Disposition: form-data; name="file"; filename="{filename}"\r\n'.encode(),
            b"Content-Type: audio/wav\r\n\r\n",
            content,
            b"\r\n",
            f"--{boundary}\r\n".encode(),
            b'Content-Disposition: form-data; name="language"\r\n\r\n',
            b"auto\r\n",
            f"--{boundary}--\r\n".encode(),
        ]
    )
    req = urllib.request.Request(
        f"{base_url}/api/v1/voice/transcribe-file",
        data=payload,
        headers={
            **auth_headers(token),
            "Content-Type": f"multipart/form-data; boundary={boundary}",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, parse_json(resp.read())
    except urllib.error.HTTPError as exc:
        payload = parse_json(exc.read())
        if exc.code in allow_statuses:
            return exc.code, payload
        raise ProbeError(f"POST /api/v1/voice/transcribe-file failed HTTP {exc.code}: {payload}") from exc


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ProbeError(message)


def require_json_object(value: Any, label: str) -> dict[str, Any]:
    require(isinstance(value, dict), f"{label} must return a JSON object")
    return value


def require_keys(value: dict[str, Any], keys: set[str], label: str) -> None:
    missing = sorted(k for k in keys if k not in value)
    require(not missing, f"{label} missing keys: {missing}")


def int_value(value: Any) -> int | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return value
    if isinstance(value, float) and value.is_integer():
        return int(value)
    return None


def ensure_token(args: argparse.Namespace, base_url: str) -> str:
    if args.token:
        return args.token
    request_json(
        base_url,
        "POST",
        "/api/v1/vault/setup",
        body={"password": args.password},
        timeout=args.timeout,
        allow_statuses={400, 409},
    )
    _, unlocked = request_json(
        base_url,
        "POST",
        "/api/v1/vault/unlock",
        body={"password": args.password},
        timeout=args.timeout,
    )
    token = require_json_object(unlocked, "vault unlock").get("token")
    require(isinstance(token, str) and bool(token), "vault unlock did not return token")
    return token


def scheduler_patch(server_scheduler_base: str, chat_model: str) -> dict[str, Any]:
    return {
        "llm": {
            "provider": "local_scheduler",
            "endpoint": server_scheduler_base,
            "model": chat_model,
            "api_key": "local-scheduler",
        },
        "embedding": {
            "provider": "local_scheduler",
            "endpoint": server_scheduler_base,
            "model": "embedding-int8",
            "task": "kb.query.embed",
            "dims": 512,
        },
    }


def poll_scheduler_job(args: argparse.Namespace, base_url: str, token: str, job_id: str) -> dict[str, Any]:
    encoded = urllib.parse.quote(job_id, safe="")
    candidates = [
        (base_url, f"/api/v1/chat/edge-scheduler/jobs/{encoded}", token),
        (base_url, f"/api/v1/chat/local-scheduler/jobs/{encoded}", token),
    ]
    if args.scheduler_url:
        candidates.append((normalize_base(args.scheduler_url), f"/jobs/{encoded}", ""))

    deadline = time.monotonic() + args.job_timeout
    last: Any = None
    while time.monotonic() < deadline:
        for candidate_base, path, candidate_token in candidates:
            try:
                _, payload = request_json(candidate_base, "GET", path, token=candidate_token, timeout=min(args.timeout, 30))
            except ProbeError as exc:
                last = str(exc)
                continue
            if isinstance(payload, dict) and isinstance(payload.get("job"), dict):
                job = payload["job"]
            else:
                job = payload if isinstance(payload, dict) else {}
            status = str(job.get("status") or job.get("phase") or "").lower()
            last = job
            if status in TERMINAL_JOB_STATUSES:
                return job
        time.sleep(1)
    raise ProbeError(f"scheduler job {job_id} did not finish within {args.job_timeout}s: {last}")


def gate_result(name: str, fn) -> dict[str, Any]:
    started = time.perf_counter()
    try:
        detail = fn()
        return {
            "name": name,
            "pass": True,
            "latency_ms": round((time.perf_counter() - started) * 1000, 3),
            "detail": detail if detail is not None else {},
        }
    except Exception as exc:  # noqa: BLE001 - probe should report all gates.
        return {
            "name": name,
            "pass": False,
            "latency_ms": round((time.perf_counter() - started) * 1000, 3),
            "error": str(exc),
        }


def scheduler_observations_from_gates(results: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Surface scheduler instability observed while probing Attune's contract."""
    observations: list[dict[str, Any]] = []
    by_name = {str(item.get("name")): item for item in results}

    scheduler_probe = by_name.get("scheduler_probe")
    if scheduler_probe:
        if not scheduler_probe.get("pass"):
            observations.append({
                "severity": "error",
                "source": "scheduler_probe",
                "message": "scheduler instability: Attune could not validate scheduler discovery",
                "error": scheduler_probe.get("error"),
            })
        else:
            detail = scheduler_probe.get("detail") if isinstance(scheduler_probe.get("detail"), dict) else {}
            if detail.get("found") is False:
                observations.append({
                    "severity": "warning",
                    "source": "scheduler_probe",
                    "message": "scheduler instability: scheduler discovery returned found=false",
                    "checked": detail.get("checked"),
                })

    chat_scheduler = by_name.get("chat_scheduler")
    if chat_scheduler:
        if not chat_scheduler.get("pass"):
            observations.append({
                "severity": "error",
                "source": "chat_scheduler",
                "message": "scheduler instability: scheduler-backed chat gate failed",
                "error": chat_scheduler.get("error"),
            })
        else:
            detail = chat_scheduler.get("detail") if isinstance(chat_scheduler.get("detail"), dict) else {}
            job = detail.get("job") if isinstance(detail.get("job"), dict) else None
            if job:
                observations.append({
                    "severity": "info",
                    "source": "chat_scheduler",
                    "message": "scheduler job telemetry observed by Attune",
                    "job_id": job.get("job_id"),
                    "status": job.get("status"),
                    "model": job.get("model"),
                    "latency_ms": job.get("latency_ms"),
                    "queue_wait_ms": job.get("queue_wait_ms"),
                })
                if job.get("latency_ms") is None or job.get("queue_wait_ms") is None:
                    observations.append({
                        "severity": "warning",
                        "source": "chat_scheduler",
                        "message": "scheduler instability: scheduler job omitted latency or queue telemetry",
                        "job_id": job.get("job_id"),
                    })
            elif detail.get("scheduler_metadata") is False:
                observations.append({
                    "severity": "warning",
                    "source": "chat_scheduler",
                    "message": "scheduler instability: chat response did not expose scheduler metadata",
                })

    return observations


def run_live(args: argparse.Namespace) -> dict[str, Any]:
    base_url = normalize_base(args.base_url)
    server_scheduler_base = normalize_scheduler_base(args.server_scheduler_base)
    token_holder: dict[str, str] = {}
    cleanup_items: list[str] = []
    cleanup_dirs: list[str] = []
    local_bind: dict[str, str] = {}
    token_value = ""

    def token() -> str:
        if "token" not in token_holder:
            token_holder["token"] = ensure_token(args, base_url)
        return token_holder["token"]

    def health_gate() -> dict[str, Any]:
        _, root_health = request_json(base_url, "GET", "/health", timeout=args.timeout)
        _, api_health = request_json(base_url, "GET", "/api/v1/status/health", timeout=args.timeout)
        require_json_object(root_health, "/health")
        require_json_object(api_health, "/api/v1/status/health")
        return {"root": root_health, "api": api_health}

    def ui_gate() -> dict[str, Any]:
        status, headers, body = request_raw(base_url, "GET", "/", timeout=args.timeout)
        require(200 <= status < 300, "GET / must return 2xx")
        require(len(body) > 0, "GET / returned empty body")
        content_type = headers.get("content-type", "")
        return {"status": status, "content_type": content_type, "bytes": len(body)}

    def vault_gate() -> dict[str, Any]:
        _, status = request_json(base_url, "GET", "/api/v1/vault/status", timeout=args.timeout)
        require_json_object(status, "vault status")
        got = token()
        return {"vault_state": status.get("state"), "token_len": len(got)}

    def settings_scheduler_gate() -> dict[str, Any]:
        _, data = request_json(
            base_url,
            "PATCH",
            "/api/v1/settings",
            body=scheduler_patch(server_scheduler_base, args.scheduler_chat_model),
            token=token(),
            timeout=args.timeout,
        )
        settings = require_json_object(data, "settings patch")
        llm = settings.get("llm") if isinstance(settings.get("llm"), dict) else {}
        embedding = settings.get("embedding") if isinstance(settings.get("embedding"), dict) else {}
        require(llm.get("provider") == "local_scheduler", "llm.provider did not persist local_scheduler")
        require(llm.get("endpoint") == server_scheduler_base, "llm.endpoint did not match server scheduler base")
        require(llm.get("model") == args.scheduler_chat_model, "llm.model did not match scheduler chat model")
        require(embedding.get("provider") == "local_scheduler", "embedding.provider did not persist local_scheduler")
        _, readback = request_json(base_url, "GET", "/api/v1/settings", token=token(), timeout=args.timeout)
        readback_settings = require_json_object(readback, "settings get")
        return {
            "llm_provider": llm.get("provider"),
            "llm_endpoint": llm.get("endpoint"),
            "embedding_provider": embedding.get("provider"),
            "settings_keys": sorted(readback_settings.keys()),
        }

    def scheduler_probe_gate() -> dict[str, Any]:
        _, data = request_json(
            base_url,
            "POST",
            "/api/v1/llm/probe-edge-scheduler",
            body={"endpoints": [server_scheduler_base]},
            token=token(),
            timeout=args.timeout,
        )
        probe = require_json_object(data, "probe-edge-scheduler")
        require("found" in probe and "checked" in probe, "probe-edge-scheduler missing found/checked")
        if args.require_scheduler_chat:
            require(probe.get("found") is True, f"probe-edge-scheduler did not find scheduler-native endpoint: {probe}")
        return probe

    def model_capability_gate() -> dict[str, Any]:
        _, data = request_json(base_url, "GET", "/api/v1/ai-stack", token=token(), timeout=args.timeout)
        payload = require_json_object(data, "ai-stack model capability")
        rows = payload.get("model_capabilities")
        if rows is None and isinstance(payload.get("scheduler"), dict):
            rows = payload["scheduler"].get("model_capabilities")
        require(isinstance(rows, list), "ai-stack missing model_capabilities list")
        normalized: list[dict[str, Any]] = []
        for row in rows:
            require(isinstance(row, dict), f"model_capabilities rows must be objects: {row}")
            name = row.get("name")
            capabilities = row.get("capabilities")
            require(isinstance(name, str) and name.strip(), f"model_capability row missing model name: {row}")
            require(isinstance(capabilities, list), f"model_capability row missing capabilities: {row}")
            normalized.append({"name": name, "capabilities": [str(item) for item in capabilities], "ready": row.get("ready")})
        all_capabilities = {cap for row in normalized for cap in row["capabilities"]}
        require("chat" in all_capabilities, f"model_capability did not expose chat capability: {normalized}")
        require("summary" in all_capabilities, f"model_capability did not expose summary capability: {normalized}")
        return {"models": normalized, "capabilities": sorted(all_capabilities)}

    def core_reads_gate() -> dict[str, Any]:
        ok: list[str] = []
        for label, path, keys in CORE_READ_ENDPOINTS:
            _, data = request_json(base_url, "GET", path, token=token(), timeout=args.timeout)
            if isinstance(data, dict):
                require_keys(data, keys, label)
            elif keys:
                raise ProbeError(f"{label} must return object with keys {sorted(keys)}")
            ok.append(label)
        return {"count": len(ok), "labels": ok}

    def upload_gate() -> dict[str, Any]:
        marker = f"attune-nas-web-api-upload-{int(time.time())}"
        status, data = request_multipart_upload(
            base_url,
            token(),
            "nas-web-api-contract.md",
            f"# NAS Web API Contract\n\n{marker}\n",
            args.timeout,
        )
        require(200 <= status < 300, "upload did not return 2xx")
        payload = require_json_object(data, "upload")
        item_id = payload.get("id")
        if isinstance(item_id, str) and item_id:
            cleanup_items.append(item_id)
        require(payload.get("status") in {"processing", "duplicate", "degraded", "staged"} or item_id, f"unexpected upload response: {payload}")
        return {"status": payload.get("status"), "item_id": item_id, "marker": marker}

    def index_bind_gate() -> dict[str, Any]:
        if not args.bind_dir:
            raise ProbeError("--bind-dir is required for the strict index_bind gate")
        marker = "attune-nas-web-api-bind-token"
        _, data = request_json(
            base_url,
            "POST",
            "/api/v1/index/bind",
            body={
                "path": args.bind_dir,
                "recursive": True,
                "file_types": ["md", "txt"],
                "corpus_domain": "nas-web-contract",
            },
            token=token(),
            timeout=args.timeout,
        )
        payload = require_json_object(data, "index bind")
        dir_id = payload.get("dir_id")
        if isinstance(dir_id, str) and dir_id:
            cleanup_dirs.append(dir_id)
            local_bind["dir_id"] = dir_id
        require(payload.get("status") in {"ok", "accepted"}, f"unexpected bind status: {payload}")
        scan = require_json_object(payload.get("scan", {}), "index bind scan")
        require("deleted" in scan, f"index bind scan missing deleted count: {scan}")
        _, search = request_json(
            base_url,
            "GET",
            f"/api/v1/search?{urllib.parse.urlencode({'q': marker, 'top_k': 10})}",
            token=token(),
            timeout=args.timeout,
        )
        result = require_json_object(search, "search after bind")
        results = result.get("results", [])
        require(isinstance(results, list), "search results must be a list")
        result_text = json.dumps(results, ensure_ascii=False)
        require(marker in result_text, "server-side bind content was not searchable")
        return {"dir_id": dir_id, "search_results": len(results), "scan_deleted": scan.get("deleted")}

    def index_rescan_gate() -> dict[str, Any]:
        dir_id = local_bind.get("dir_id")
        require(dir_id, "index_rescan requires a successful index_bind dir_id")
        _, data = request_json(
            base_url,
            "POST",
            "/api/v1/index/rescan",
            body={"dir_id": dir_id},
            token=token(),
            timeout=args.timeout,
        )
        payload = require_json_object(data, "index rescan")
        require(payload.get("status") == "ok", f"unexpected rescan status: {payload}")
        scan = require_json_object(payload.get("scan", {}), "index rescan scan")
        require("deleted" in scan, f"index rescan scan missing deleted count: {scan}")
        return {
            "dir_id": dir_id,
            "total": scan.get("total"),
            "skipped": scan.get("skipped"),
            "deleted": scan.get("deleted"),
            "degraded": scan.get("degraded"),
        }

    def vector_indexing_snapshot() -> dict[str, Any]:
        _, status = request_json(base_url, "GET", "/api/v1/status", token=token(), timeout=args.timeout)
        _, index_status = request_json(base_url, "GET", "/api/v1/index/status", token=token(), timeout=args.timeout)
        status_obj = require_json_object(status, "status")
        index_obj = require_json_object(index_status, "index status")
        status_pending = int_value(status_obj.get("pending_embeddings"))
        index_pending = int_value(index_obj.get("pending_embeddings"))
        pending_values = [value for value in (status_pending, index_pending) if value is not None]
        require(pending_values, "status/index status did not expose pending_embeddings")
        return {
            "status_pending_embeddings": status_pending,
            "index_pending_embeddings": index_pending,
            "pending_embeddings": max(pending_values),
            "vector_ready": status_obj.get("vector_index", status_obj.get("vector_ready")),
            "fulltext_ready": status_obj.get("fulltext_index", status_obj.get("fulltext_ready")),
        }

    def vector_indexing_gate() -> dict[str, Any]:
        started = time.perf_counter()
        deadline = time.monotonic() + args.job_timeout
        polls = 1
        first = vector_indexing_snapshot()
        last = first
        while int_value(last.get("pending_embeddings")) != 0:
            if time.monotonic() >= deadline:
                raise ProbeError(f"embedding/vector queue did not drain within {args.job_timeout}s: {last}")
            time.sleep(0.5)
            polls += 1
            last = vector_indexing_snapshot()
        return {
            "initial": first,
            "final": last,
            "polls": polls,
            "drain_elapsed_ms": round((time.perf_counter() - started) * 1000, 3),
        }

    def export_gate() -> dict[str, Any]:
        artifact = {
            "type": "table",
            "data": {
                "title": "nas-web-api-contract",
                "headers": ["key", "value"],
                "rows": [["contract", "ok"]],
                "aligns": ["left", "left"],
            },
        }
        status, headers, body = request_raw(
            base_url,
            "POST",
            "/api/v1/export",
            body={"artifact": artifact, "format": "csv", "filename": "nas-web-api-contract"},
            token=token(),
            timeout=args.timeout,
        )
        require(200 <= status < 300, "export did not return 2xx")
        require(b"contract" in body, "export body did not contain expected CSV content")
        return {"content_type": headers.get("content-type", ""), "bytes": len(body)}

    def chat_scheduler_gate() -> dict[str, Any]:
        _, data = request_json(
            base_url,
            "POST",
            "/api/v1/chat",
            body={"message": "用一句话说明 attune-nas-web-api-bind-token 是否在知识库里。", "history": []},
            token=token(),
            timeout=max(args.timeout, 180),
        )
        payload = require_json_object(data, "chat")
        answer = payload.get("answer") or payload.get("content") or ""
        require(isinstance(answer, str) and bool(answer.strip()), "chat returned empty answer/content")
        meta = payload.get("local_scheduler")
        if args.require_scheduler_chat:
            require(isinstance(meta, dict), "chat did not return local_scheduler metadata")
            task = str(meta.get("task") or "")
            require(task in {"kb.query.ask", "local.extractive.answer", "local.safety.refusal"}, f"unexpected scheduler task {task}")
            job_id = meta.get("job_id")
            job_summary: dict[str, Any] | None = None
            if isinstance(job_id, str) and job_id:
                job = poll_scheduler_job(args, base_url, token(), job_id)
                status = str(job.get("status") or job.get("phase") or "").lower()
                require(status not in FAILED_JOB_STATUSES, f"scheduler chat job failed: {job}")
                require(status in {"done", "completed", "complete", "success", "succeeded"}, f"scheduler chat job did not complete: {job}")
                job_summary = {
                    "job_id": job_id,
                    "status": status,
                    "model": job.get("model"),
                    "latency_ms": job.get("latency_ms"),
                    "queue_wait_ms": job.get("queue_wait_ms"),
                }
            return {"task": task, "job": job_summary}
        return {"scheduler_metadata": isinstance(meta, dict)}

    def summary_workflow_gate() -> dict[str, Any]:
        _, data = request_json(
            base_url,
            "POST",
            "/api/v1/summary/workflow",
            body={
                "scenario": "risk",
                "detail": "attune-nas-web-api-bind-token",
                "model": args.scheduler_chat_model,
                "top_k": 6,
            },
            token=token(),
            timeout=args.timeout,
        )
        payload = require_json_object(data, "summary workflow")
        content = payload.get("content") or payload.get("summary") or ""
        require(isinstance(content, str) and content.strip(), "summary workflow returned empty content")
        sections = payload.get("summary_sections")
        require(isinstance(sections, dict), "summary workflow missing summary_sections object")
        require("core_conclusions" in sections, "summary_sections missing core_conclusions")
        require("risks_or_gaps" in sections, "summary_sections missing risks_or_gaps")
        citations = payload.get("citations")
        require(isinstance(citations, list), "summary workflow missing citations list")
        workflow = payload.get("summary_workflow")
        require(isinstance(workflow, dict), "summary workflow missing summary_workflow metadata")
        stages = workflow.get("stages") or payload.get("workflow_stages")
        require(isinstance(stages, list), "summary workflow missing multi-stage stages list")
        stage_names = [stage.get("name") for stage in stages if isinstance(stage, dict)]
        for required_stage in ("select", "map", "synthesize", "audit"):
            require(required_stage in stage_names, f"summary workflow missing stage {required_stage}: {stage_names}")
        return {
            "scenario": payload.get("scenario"),
            "model": payload.get("model"),
            "knowledge_count": payload.get("knowledge_count"),
            "citations": len(citations),
            "stages": stage_names,
        }

    def voice_scheduler_gate() -> dict[str, Any]:
        _, status_payload = request_json(
            base_url,
            "GET",
            "/api/v1/voice/status",
            token=token(),
            timeout=args.timeout,
        )
        status = require_json_object(status_payload, "voice status")
        require(status.get("schema_version") == "attune.voice.v1", "voice status schema mismatch")
        routes = require_json_object(status.get("routes", {}), "voice routes")
        require("synthesize" not in routes, "voice routes must not expose server-side synthesize packaging")
        require(routes.get("transcribe") == "/api/v1/voice/transcribe", "voice transcribe route missing")
        require(routes.get("transcribe_file") == "/api/v1/voice/transcribe-file", "voice transcribe-file route missing")
        require("tts" not in status, "voice status must stay scoped to ASR/audio input")
        asr = require_json_object(status.get("asr", {}), "voice asr")
        require(asr.get("task") == "kb.meeting.asr_frontend", "voice asr task mismatch")
        transcribe_status, transcribe_error = request_json(
            base_url,
            "POST",
            "/api/v1/voice/transcribe",
            body={"file_path": "/tmp/attune-nas-web-api-contract-missing.wav"},
            token=token(),
            timeout=args.timeout,
            allow_statuses={400, 503},
        )
        transcribe_error = require_json_object(transcribe_error, "voice transcribe validation")
        transcribe_code = transcribe_error.get("code")
        require(
            (transcribe_status == 400 and transcribe_code == "invalid-input")
            or (transcribe_status == 503 and transcribe_code == "voice-asr-not-ready"),
            f"voice transcribe did not expose Attune ASR validation/gate: HTTP {transcribe_status} {transcribe_error}",
        )
        upload_status, upload_payload = request_multipart_voice_upload(
            base_url,
            token(),
            "attune-voice-contract.wav",
            b"RIFF....WAVEfmt ",
            args.timeout,
            allow_statuses={503},
        )
        upload_payload = require_json_object(upload_payload, "voice transcribe-file")
        require(
            (upload_status == 202 and isinstance(upload_payload.get("job_id"), str) and upload_payload.get("job_id"))
            or (upload_status == 503 and upload_payload.get("code") == "voice-asr-not-ready"),
            f"voice transcribe-file did not expose Attune audio upload gate: HTTP {upload_status} {upload_payload}",
        )
        job_id = upload_payload.get("job_id") if upload_status == 202 else None
        if isinstance(job_id, str) and job_id:
            request_json(
                base_url,
                "DELETE",
                f"/api/v1/office/jobs/{urllib.parse.quote(job_id, safe='')}",
                token=token(),
                timeout=args.timeout,
                allow_statuses={404},
            )
        return {
            "status_route": "/api/v1/voice/status",
            "transcribe_route": routes.get("transcribe"),
            "transcribe_file_route": routes.get("transcribe_file"),
            "transcribe_file_status": upload_status,
            "asr_available": asr.get("available"),
        }

    def cleanup_gate() -> dict[str, Any]:
        deleted_items = 0
        deleted_dirs = 0
        for item_id in cleanup_items:
            encoded = urllib.parse.quote(item_id, safe="")
            request_json(base_url, "DELETE", f"/api/v1/items/{encoded}", token=token(), timeout=args.timeout, allow_statuses={404})
            deleted_items += 1
        for dir_id in cleanup_dirs:
            encoded = urllib.parse.quote(dir_id, safe="")
            request_json(base_url, "DELETE", f"/api/v1/index/unbind?dir_id={encoded}", token=token(), timeout=args.timeout, allow_statuses={404})
            deleted_dirs += 1
        return {"items": deleted_items, "directories": deleted_dirs}

    gates = [
        ("health", health_gate),
        ("ui_shell", ui_gate),
        ("vault", vault_gate),
        ("settings_scheduler", settings_scheduler_gate),
        ("scheduler_probe", scheduler_probe_gate),
        ("model_capability", model_capability_gate),
        ("core_reads", core_reads_gate),
        ("upload", upload_gate),
        ("index_bind", index_bind_gate),
        ("index_rescan", index_rescan_gate),
        ("vector_indexing", vector_indexing_gate),
        ("export", export_gate),
        ("summary_workflow", summary_workflow_gate),
        ("voice_scheduler", voice_scheduler_gate),
        ("chat_scheduler", chat_scheduler_gate),
    ]
    results = [gate_result(name, fn) for name, fn in gates]
    token_value = token_holder.get("token", "")
    results.append(gate_result("cleanup", cleanup_gate))
    failures = [item for item in results if not item["pass"]]
    scheduler_observations = scheduler_observations_from_gates(results)
    return {
        "mode": "api_contract",
        "dry_run": False,
        "base_url": base_url,
        "scheduler_url": args.scheduler_url,
        "server_scheduler_base": server_scheduler_base,
        "token_len": len(token_value),
        "gates": results,
        "scheduler_observations": scheduler_observations,
        "pass": not failures,
        "failures": failures,
    }


def dry_run(args: argparse.Namespace) -> dict[str, Any]:
    return {
        "mode": "api_contract",
        "dry_run": True,
        "base_url": normalize_base(args.base_url),
        "scheduler_url": args.scheduler_url,
        "server_scheduler_base": normalize_scheduler_base(args.server_scheduler_base),
        "bind_dir": args.bind_dir,
        "gates": [
            "health",
            "ui_shell",
            "vault",
            "settings_scheduler",
            "scheduler_probe",
            "model_capability",
            "core_reads",
            "upload",
            "index_bind",
            "vector_indexing",
            "export",
            "summary_workflow",
            "voice_scheduler",
            "chat_scheduler",
            "cleanup",
        ],
        "notes": [
            "bind_dir must be a server-side path visible to the Attune server",
            "server_scheduler_base is persisted into Attune settings and is evaluated from the NAS host",
            "scheduler_url is used only by the CI runner for scheduler probes or job polling",
            "scheduler instability observations are emitted from scheduler_probe, chat_scheduler, and job telemetry",
            "voice_scheduler validates Attune /api/v1/voice/status, /api/v1/voice/transcribe, and /api/v1/voice/transcribe-file; clients must not call scheduler audio endpoints directly",
        ],
        "scheduler_observations": [],
        "pass": True,
    }


def main() -> int:
    args = parse_args()
    if not args.dry_run and not args.password and not args.token:
        raise ProbeError("--password, --token, ATTUNE_E2E_PASSWORD, or ATTUNE_VAULT_PW is required")
    result = dry_run(args) if args.dry_run else run_live(args)
    text = json.dumps(result, ensure_ascii=False, indent=2, sort_keys=True)
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(text + "\n", encoding="utf-8")
    print(text)
    return 0 if result.get("pass") else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ProbeError as exc:
        print(json.dumps({"mode": "api_contract", "pass": False, "error": str(exc)}, ensure_ascii=False, indent=2), file=sys.stderr)
        raise SystemExit(2)
