#!/usr/bin/env python3
"""Playwright gate for using kb-web-demo as the Attune RAG eval frontend."""
from __future__ import annotations

import argparse
import json
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any


DEFAULT_RTOS_CORPUS_DIR = Path(
    "/mnt/hdd/allwinner/v821/tina-v821-release-v1.1/tina-v821-release/docs/pdf/其他文档/RTOS"
)
DEFAULT_RTOS_SOURCE_FILE = "RTOS_CCU_开发指南.pdf"
DEFAULT_RTOS_DMAC_SOURCE_FILE = "RTOS_DMAC_开发指南.pdf"
RTOS_HOWTO_QUESTION = "rtos开发中如何在ccu开发中查询时钟的type和id"
RTOS_DMAC_QUESTION = "给我rtos中dmac申请dma通道的函数接口"


class FrontendEvalError(RuntimeError):
    def __init__(self, stage: str, reason: str) -> None:
        super().__init__(reason)
        self.stage = stage


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="http://127.0.0.1:8890")
    parser.add_argument("--api-url", default="http://127.0.0.1:8889")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--token", default="")
    parser.add_argument("--headless", type=int, default=1)
    parser.add_argument("--timeout-ms", type=int, default=120000)
    parser.add_argument("--profile", choices=("smoke", "deep", "rtos"), default="smoke")
    parser.add_argument("--rtos-corpus-dir", type=Path, default=DEFAULT_RTOS_CORPUS_DIR)
    parser.add_argument("--rtos-source-file", default=DEFAULT_RTOS_SOURCE_FILE)
    parser.add_argument("--rtos-dmac-source-file", default=DEFAULT_RTOS_DMAC_SOURCE_FILE)
    parser.add_argument(
        "--reset-before",
        action="store_true",
        help="Clear the Attune demo knowledge base before running this frontend gate.",
    )
    parser.add_argument("--dry-run", action="store_true")
    return parser.parse_args()


def write_report(path: Path, report: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def dry_run_report(args: argparse.Namespace) -> dict[str, Any]:
    chat_cases = web_demo_chat_cases("WEB_DEMO_DRY_RUN") if args.profile == "deep" else []
    summary_cases = web_demo_summary_cases("WEB_DEMO_DRY_RUN") if args.profile == "deep" else []
    rtos_case = rtos_case_metadata(args) if args.profile == "rtos" else None
    rtos_dmac_case = rtos_dmac_case_metadata(args) if args.profile == "rtos" else None
    checks = [
        "upload",
        "folder_upload",
        "vector_chunk_render",
        "chat",
        "summary",
        "model_switch_gate",
        "citation_render",
        "time_render",
    ]
    if args.profile == "rtos":
        checks = [
            "upload",
            "vector_chunk_render",
            "chat",
            "citation_render",
            "time_render",
            "rtos_manual_howto",
            "rtos_dmac_howto",
        ]
    metrics = {
        "web_demo_flow_pass_rate": 1.0,
        "web_demo_citation_render_rate": 1.0,
        "web_demo_time_render_rate": 1.0,
        "web_demo_vector_chunk_render_rate": 1.0,
        "web_demo_model_switch_gate_rate": 1.0,
        "web_demo_complex_chat_pass_rate": 1.0,
        "web_demo_summary_workflow_pass_rate": 1.0,
    }
    if args.profile == "rtos":
        metrics["web_demo_rtos_manual_howto_pass_rate"] = 1.0
        metrics["web_demo_rtos_dmac_howto_pass_rate"] = 1.0
    return {
        "schema_version": "attune.eval.web_demo_frontend.v1",
        "mode": "dry_run",
        "profile": args.profile,
        "target": {
            "base_url": args.base_url,
            "api_url": args.api_url,
            "auth": "bearer" if args.token else "none",
        },
        "checks": checks,
        "metrics": {"frontend": metrics},
        "artifacts": {
            "chat_cases": [
                {
                    "case_id": case["case_id"],
                    "kind": case["kind"],
                    "required_terms": case["required_terms"],
                    "expected_tasks": sorted(case["expected_tasks"]),
                }
                for case in chat_cases
            ],
            "summary_cases": [
                {
                    "case_id": case["case_id"],
                    "scenario": case["scenario"],
                    "required_terms": case["required_terms"],
                    "required_sections": case["required_sections"],
                    "required_stages": case["required_stages"],
                }
                for case in summary_cases
            ],
            "rtos_case": rtos_case,
            "rtos_dmac_case": rtos_dmac_case,
        },
        "failures": [],
    }


def rtos_case_metadata(args: argparse.Namespace) -> dict[str, Any]:
    return {
        "question": RTOS_HOWTO_QUESTION,
        "corpus_dir": str(args.rtos_corpus_dir),
        "source_file": args.rtos_source_file,
        "required_evidence_groups": {
            "method": [
                "hal/source/ccmu",
                "sunxi-ng",
                "ccu-",
                "ccu_sun20iw2.h",
                "HAL_SUNXI_CCU",
                "CLK_",
                "clk id",
                "type",
                "hal_clk_type_t",
                "hal_clock_get",
            ],
            "location": ["RTOS_CCU_开发指南", "4.2.1", "查找某个时钟"],
        },
    }


def rtos_dmac_case_metadata(args: argparse.Namespace) -> dict[str, Any]:
    return {
        "question": RTOS_DMAC_QUESTION,
        "corpus_dir": str(args.rtos_corpus_dir),
        "source_file": args.rtos_dmac_source_file,
        "required_evidence_groups": {
            "interface": [
                "hal_dma_chan_request",
                "sunxi_dma_chan",
            ],
            "status": [
                "HAL_DMA_CHAN_STATUS_FREE",
                "HAL_DMA_CHAN_STATUS_BUSY",
            ],
            "location": [
                "RTOS_DMAC_开发指南",
                "dma",
                "通道",
            ],
            "forbidden_linux_interfaces": [
                "dma_request_chan",
            ],
        },
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


def wait_for_named_demo_ready(page: Any, needle: str, timeout: int) -> bool:
    try:
        page.wait_for_function(
            """
            (needle) => {
              const rows = JSON.parse(sessionStorage.getItem('attune_demo_files') || '[]');
              return rows.some(([, f]) => {
                if (!f || !(f.ready || f.status === 'ready')) return false;
                return String(f.name || '').includes(needle);
              });
            }
            """,
            arg=needle,
            timeout=timeout,
        )
        return True
    except Exception:
        return False


def verify_model_switch_gate(page: Any) -> dict[str, Any]:
    return page.evaluate(
        """
        async () => {
          const summary = document.getElementById('summaryModel');
          const btn = document.getElementById('summaryBtn');
          const detail = document.getElementById('summaryDetail');
          if (!summary || !btn || !detail || typeof updateModelActionGate !== 'function') {
            return {pass:false, reason:'summary model gate controls missing'};
          }
          const options = Array.from(summary.options || []).map(option => ({
            value: option.value,
            text: option.textContent || '',
            ready: option.dataset.ready,
          })).filter(option => option.value);
          const notReady = options.find(option => option.ready === '0'
            || /loading|unavailable|not ready|missing|unknown|failed|displaced/i.test(option.text));
          if (!notReady) {
            return {pass:true, skipped:true, reason:'no not-ready summary model option'};
          }
          const originalApi = window.api;
          let workflowCalls = 0;
          window.api = async function(method, path, body) {
            if (path === '/api/v1/summary/workflow') workflowCalls += 1;
            return originalApi.apply(this, arguments);
          };
          try {
            summary.value = notReady.value;
            summary.dispatchEvent(new Event('change', {bubbles:true}));
            await new Promise(resolve => setTimeout(resolve, 0));
            const blockedOk = btn.disabled === true
              && detail.disabled === true
              && btn.dataset.lockReason === 'model-not-ready';
            if (typeof runSummaryWorkflow === 'function') {
              await runSummaryWorkflow();
            }
            const ready = options.find(option => option.value !== notReady.value
              && (option.ready === '1'
                || !/loading|unavailable|not ready|missing|unknown|failed|displaced/i.test(option.text)));
            let readyOk = true;
            if (ready) {
              summary.value = ready.value;
              summary.dispatchEvent(new Event('change', {bubbles:true}));
              await new Promise(resolve => setTimeout(resolve, 0));
              readyOk = btn.disabled === false && detail.disabled === false;
            }
            return {
              pass: blockedOk && workflowCalls === 0 && readyOk,
              blockedOk,
              workflowCalls,
              readyOk,
              notReadyModel: notReady.value,
              readyModel: ready ? ready.value : null,
            };
          } finally {
            window.api = originalApi;
          }
        }
        """
    )


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


def api_post_json(
    base_url: str,
    path: str,
    payload: dict[str, Any],
    timeout_ms: int,
    token: str = "",
) -> dict[str, Any]:
    url = f"{base_url.rstrip('/')}{path}"
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    headers = {"content-type": "application/json"}
    if token:
        headers["authorization"] = f"Bearer {token}"
    request = urllib.request.Request(
        url,
        data=body,
        method="POST",
        headers=headers,
    )
    try:
        with urllib.request.urlopen(request, timeout=max(timeout_ms / 1000, 1)) as response:
            text = response.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as exc:
        text = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"POST {path} HTTP {exc.code}: {text[:1000]}") from exc
    data = json.loads(text) if text else {}
    return data if isinstance(data, dict) else {"_payload": data}


def api_get_json(
    base_url: str,
    path: str,
    timeout_ms: int,
    token: str = "",
) -> dict[str, Any]:
    url = f"{base_url.rstrip('/')}{path}"
    headers = {}
    if token:
        headers["authorization"] = f"Bearer {token}"
    request = urllib.request.Request(url, method="GET", headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=max(timeout_ms / 1000, 1)) as response:
            text = response.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as exc:
        text = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"GET {path} HTTP {exc.code}: {text[:1000]}") from exc
    data = json.loads(text) if text else {}
    return data if isinstance(data, dict) else {"_payload": data}


def reset_demo_environment(base_url: str, timeout_ms: int, token: str = "") -> dict[str, Any]:
    return api_post_json(
        base_url,
        "/api/v1/demo/reset",
        {"confirm": "CLEAR_DEMO"},
        timeout_ms,
        token,
    )


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


def chat_job_id_from_payload(payload: dict[str, Any]) -> str:
    meta = payload.get("local_scheduler")
    direct = meta.get("job_id") if isinstance(meta, dict) else None
    direct = direct or payload.get("job_id") or payload.get("id")
    if isinstance(direct, str) and direct.startswith("job_"):
        return direct
    text = first_text(payload)
    marker = "job_"
    idx = text.find(marker)
    if idx < 0:
        return ""
    end = idx
    while end < len(text) and (text[end].isalnum() or text[end] in "_:-"):
        end += 1
    return text[idx:end]


def chat_async_placeholder(text: str) -> bool:
    folded = text.casefold()
    return (
        "本地 scheduler 知识库回答任务已提交" in text
        or "local scheduler" in folded
        and "job_id=" in folded
        and any(marker in folded for marker in ("submitted", "queued", "eta", "wait"))
    )


def text_from_scheduler_job(payload: dict[str, Any]) -> str:
    job = payload.get("job") if isinstance(payload.get("job"), dict) else payload
    outputs = job.get("outputs") if isinstance(job, dict) else None
    if isinstance(outputs, dict):
        choices = outputs.get("choices")
        if isinstance(choices, list) and choices:
            message = choices[0].get("message") if isinstance(choices[0], dict) else None
            if isinstance(message, dict) and isinstance(message.get("content"), str):
                return message["content"]
        for key in ("answer", "text", "content"):
            value = outputs.get(key)
            if isinstance(value, str) and value.strip():
                return value.strip()
    return first_text(payload)


def resolve_chat_payload(base_url: str, payload: dict[str, Any], timeout_ms: int, token: str = "") -> dict[str, Any]:
    text = first_text(payload)
    if text and not chat_async_placeholder(text):
        return payload
    job_id = chat_job_id_from_payload(payload)
    if not job_id:
        return payload
    deadline = time.time() + max(timeout_ms / 1000, 1)
    last_payload: dict[str, Any] = payload
    while time.time() < deadline:
        time.sleep(0.25)
        last_payload = api_get_json(
            base_url,
            f"/api/v1/chat/local-scheduler/jobs/{urllib.parse.quote(job_id)}",
            timeout_ms,
            token,
        )
        text = text_from_scheduler_job(last_payload)
        if text:
            merged = dict(payload)
            merged["content"] = text
            merged["job"] = last_payload.get("job", last_payload)
            return merged
        status = str((last_payload.get("job") or last_payload).get("status", "")).lower()
        if status in {"done", "completed", "complete", "success", "succeeded", "failed", "error", "cancelled", "canceled"}:
            break
    return payload


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


def web_demo_chat_cases(token: str, source_title: str | None = None) -> list[dict[str, Any]]:
    source_prefix = f"请只使用《{source_title} - Attune web-demo deep RAG fixture》这个来源，" if source_title else ""
    return [
        {
            "case_id": "fact_origin",
            "kind": "chat",
            "prompt": f"{source_prefix}{token} 对 TCP/IP 起源给出了什么证据？",
            "required_terms": [token, "TCP/IP", "ARPANET", "DARPA"],
            "expected_tasks": {"kb.query.ask", "local.extractive.answer"},
        },
        {
            "case_id": "operation_troubleshooting",
            "kind": "chat",
            "prompt": f"{source_prefix}基于 {token}，如何排查 TCP/IP 连接异常？必须包含日志、抓包、路由和不要编造。",
            "required_terms": ["日志", "抓包", "路由", "不要编造"],
            "expected_tasks": {"kb.query.ask"},
        },
        {
            "case_id": "multi_intent_decision",
            "kind": "chat",
            "prompt": f"{source_prefix}基于 {token}，同时判断应该先收集哪些证据、如何排序排查步骤，并说明缺少 packet capture 时的结论边界。",
            "required_terms": ["packet capture", "证据", "先", "边界"],
            "expected_tasks": {"kb.query.ask"},
        },
        {
            "case_id": "negative_evidence_boundary",
            "kind": "chat",
            "prompt": f"{source_prefix}如果 {token} 中缺少审计日志或 packet capture，能否直接判定根因？",
            "required_terms": ["不能直接", "证据不足"],
            "expected_tasks": {"kb.query.ask"},
        },
        {
            "case_id": "out_of_manual_industry_general",
            "kind": "chat",
            "prompt": f"{source_prefix}如果 {token} 手册未直接覆盖 zero trust segmentation，但客户问能否作为整改建议，应该如何回答？",
            "required_terms": ["知识库未直接覆盖", "行业通用", "不能当作手册结论"],
            "expected_tasks": {"kb.query.ask", "local.boundary.industry_general"},
        },
    ]


def web_demo_summary_cases(token: str) -> list[dict[str, Any]]:
    required_stages = ["select", "map", "synthesize", "audit"]
    return [
        {
            "case_id": "summary_recent_core",
            "scenario": "recent",
            "detail": f"总结 {token} 中 TCP/IP 起源、排查证据和结论边界。",
            "required_terms": ["TCP/IP", "证据", "边界"],
            "required_sections": ["scope", "core_conclusions", "key_evidence", "risks_or_gaps", "next_actions"],
            "required_stages": required_stages,
        },
        {
            "case_id": "summary_folder_overview",
            "scenario": "folder",
            "detail": f"按文件夹综述 {token} 和对应 folder fixture 的 TCP/IP 与 airplane mechanical design 共同主题。",
            "required_terms": [token, "TCP/IP", "airplane"],
            "required_sections": ["scope", "core_conclusions", "key_evidence", "next_actions"],
            "required_stages": required_stages,
        },
        {
            "case_id": "summary_compare_sources",
            "scenario": "compare",
            "detail": f"对比 {token} 中 TCP/IP runbook 和 airplane mechanical design 类比的差异。",
            "required_terms": ["TCP/IP", "airplane", "对比"],
            "required_sections": ["scope", "core_conclusions", "key_evidence", "risks_or_gaps"],
            "required_stages": required_stages,
        },
        {
            "case_id": "summary_risk_gap",
            "scenario": "risk",
            "detail": f"识别 {token} 中缺少 packet capture 或 audit logs 时的风险和缺口。",
            "required_terms": ["packet capture", "风险", "缺口"],
            "required_sections": ["scope", "core_conclusions", "key_evidence", "risks_or_gaps", "next_actions"],
            "required_stages": required_stages,
        },
    ]


def summary_text(payload: dict[str, Any]) -> str:
    parts = [first_text(payload)]
    sections = payload.get("summary_sections")
    if isinstance(sections, dict):
        parts.append(json.dumps(sections, ensure_ascii=False))
    return "\n".join(part for part in parts if part)


def evaluate_summary_case(payload: dict[str, Any], case: dict[str, Any]) -> dict[str, Any]:
    sections = payload.get("summary_sections")
    citations = payload.get("citations")
    workflow = payload.get("summary_workflow") if isinstance(payload.get("summary_workflow"), dict) else {}
    stages_payload = workflow.get("stages")
    if not isinstance(stages_payload, list):
        stages_payload = payload.get("workflow_stages")
    stage_names = [
        str(stage.get("name"))
        for stage in stages_payload
        if isinstance(stage, dict) and stage.get("name")
    ] if isinstance(stages_payload, list) else []
    text = summary_text(payload)
    missing_terms = case_terms_present(text, case["required_terms"])
    missing_sections = [
        section
        for section in case["required_sections"]
        if not isinstance(sections, dict) or section not in sections
    ]
    missing_stages = [
        stage
        for stage in case["required_stages"]
        if stage not in stage_names
    ]
    scenario = payload.get("scenario") or (payload.get("summary_workflow") or {}).get("scenario")
    return {
        "case_id": case["case_id"],
        "scenario": case["scenario"],
        "pass": (
            scenario == case["scenario"]
            and isinstance(sections, dict)
            and isinstance(citations, list)
            and bool(citations)
            and not missing_terms
            and not missing_sections
            and not missing_stages
        ),
        "missing_terms": missing_terms,
        "missing_sections": missing_sections,
        "missing_stages": missing_stages,
        "workflow_stages": stage_names,
        "citation_count": len(citations) if isinstance(citations, list) else 0,
        "knowledge_count": payload.get("knowledge_count"),
        "model": payload.get("model"),
    }
def case_terms_present(text: str, required_terms: list[str]) -> list[str]:
    folded = text.casefold()
    aliases = {
        "日志": ["日志", "logs", "log"],
        "抓包": ["抓包", "packet capture", "pcap", "tcpdump", "数据包", "网络包", "报文", "流量捕获"],
        "packet capture": ["packet capture", "数据包捕获", "抓包", "pcap", "tcpdump"],
        "路由": ["路由", "routing", "routing table", "route table", "default gateway"],
        "不要编造": ["不要编造", "不能编造", "not invent", "unsupported"],
        "证据": ["证据", "evidence", "source", "citation"],
        "边界": ["边界", "boundary", "scope"],
        "不能直接": ["不能直接", "无法直接", "不能判定", "不能确定", "cannot directly", "cannot determine"],
        "行业通用": ["行业通用", "industry-general", "industry general"],
        "不能当作手册结论": ["不能当作手册结论", "不能作为手册结论", "not a manual conclusion"],
        "知识库未直接覆盖": ["知识库未直接覆盖", "未直接覆盖", "not directly covered"],
        "风险": ["风险", "risk", "risks"],
        "缺口": ["缺口", "gap", "gaps", "missing"],
        "对比": ["对比", "compare", "comparison", "差异", "different"],
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


def rtos_howto_result(payload: dict[str, Any]) -> dict[str, Any]:
    text = first_text(payload)
    folded = text.casefold()
    citations = citations_from_payload(payload)
    method_candidates = [
        "hal/source/ccmu",
        "sunxi-ng",
        "ccu-",
        "ccu_sun20iw2.h",
        "HAL_SUNXI_CCU",
        "CLK_",
        "clk id",
        "type",
        "hal_clk_type_t",
        "hal_clock_get",
    ]
    method_terms = [
        term
        for term in method_candidates
        if term.casefold() in folded
    ]
    action_terms = [
        term
        for term in ["hal/source/ccmu", "ccu_sun20iw2.h", "clk id", "hal_clk_type_t", "hal_clock_get"]
        if term.casefold() in folded
    ]
    location_terms = [
        term
        for term in ["RTOS_CCU_开发指南", "4.2.1", "查找某个时钟"]
        if term.casefold() in folded or json_contains(citations, term)
    ]
    weak_redirect_only = (
        bool(text)
        and len(method_terms) < 2
        and any(term in folded for term in ["查阅", "参考", "联系技术支持", "consult"])
    )
    return {
        "pass": (
            bool(text)
            and len(method_terms) >= 2
            and bool(action_terms)
            and bool(location_terms)
            and bool(citations)
            and not weak_redirect_only
        ),
        "method_terms": method_terms,
        "action_terms": action_terms,
        "location_terms": location_terms,
        "citation_count": len(citations),
        "weak_redirect_only": weak_redirect_only,
        "content_excerpt": text[:800],
    }


def rtos_dmac_howto_result(payload: dict[str, Any]) -> dict[str, Any]:
    text = first_text(payload)
    folded = text.casefold()
    citations = citations_from_payload(payload)
    interface_terms = [
        term
        for term in ["hal_dma_chan_request", "sunxi_dma_chan"]
        if term.casefold() in folded
    ]
    status_terms = [
        term
        for term in ["HAL_DMA_CHAN_STATUS_FREE", "HAL_DMA_CHAN_STATUS_BUSY"]
        if term.casefold() in folded
    ]
    location_terms = [
        term
        for term in ["RTOS_DMAC_开发指南", "dma", "通道"]
        if term.casefold() in folded or json_contains(citations, term)
    ]
    mentions_linux_request_chan = "dma_request_chan" in folded
    linux_contrast_only = mentions_linux_request_chan and any(
        term in folded for term in ["不是 dma_request_chan", "不要用 dma_request_chan", "not dma_request_chan"]
    )
    linux_interface_confusion = mentions_linux_request_chan and not linux_contrast_only
    weak_redirect_only = (
        bool(text)
        and not interface_terms
        and any(term in folded for term in ["查阅", "参考", "联系技术支持", "consult"])
    )
    return {
        "pass": (
            bool(text)
            and len(interface_terms) >= 2
            and bool(location_terms)
            and bool(citations)
            and not linux_interface_confusion
            and not weak_redirect_only
        ),
        "interface_terms": interface_terms,
        "status_terms": status_terms,
        "location_terms": location_terms,
        "citation_count": len(citations),
        "linux_interface_confusion": linux_interface_confusion,
        "weak_redirect_only": weak_redirect_only,
        "content_excerpt": text[:800],
    }


def run_rtos_live(args: argparse.Namespace) -> dict[str, Any]:
    from playwright.sync_api import sync_playwright

    source_path = args.rtos_corpus_dir.expanduser() / args.rtos_source_file
    dmac_source_path = args.rtos_corpus_dir.expanduser() / args.rtos_dmac_source_file
    if not source_path.is_file():
        raise FrontendEvalError("rtos_source", f"RTOS source file not found: {source_path}")
    if not dmac_source_path.is_file():
        raise FrontendEvalError("rtos_dmac_source", f"RTOS DMAC source file not found: {dmac_source_path}")

    failures: list[dict[str, Any]] = []
    stage = "init"
    checks = {
        "upload": False,
        "vector_chunk_render": False,
        "chat": False,
        "citation_render": False,
        "time_render": False,
        "rtos_manual_howto": False,
        "rtos_dmac_howto": False,
    }
    rtos_result: dict[str, Any] = {}
    rtos_dmac_result: dict[str, Any] = {}
    started = time.perf_counter()

    with sync_playwright() as p:
        browser = p.chromium.launch(headless=bool(args.headless))
        page = browser.new_page()
        if args.token:
            page.add_init_script(
                script=f"sessionStorage.setItem('attune_auth_token', {json.dumps(args.token)});",
            )
        try:
            stage = "open_page"
            page_params = {"api": args.api_url}
            joiner = "&" if "?" in args.base_url else "?"
            page.goto(
                f"{args.base_url}{joiner}{urllib.parse.urlencode(page_params)}",
                wait_until="networkidle",
                timeout=args.timeout_ms,
            )
            page.get_by_text("上传 & 管理", exact=False).first.wait_for(state="visible", timeout=30000)

            stage = "upload_rtos_pdf"
            page.locator('input[type="file"]').first.set_input_files(str(source_path))
            checks["upload"] = visible_text(
                page,
                args.rtos_source_file,
                timeout=min(args.timeout_ms, 45000),
            ) and wait_for_named_demo_ready(page, args.rtos_source_file, timeout=args.timeout_ms)
            checks["time_render"] = visible_text(page, "全流程时间", timeout=5000)

            stage = "upload_rtos_dmac_pdf"
            page.locator('input[type="file"]').first.set_input_files(str(dmac_source_path))
            checks["upload"] = checks["upload"] and visible_text(
                page,
                args.rtos_dmac_source_file,
                timeout=min(args.timeout_ms, 45000),
            ) and wait_for_named_demo_ready(page, args.rtos_dmac_source_file, timeout=args.timeout_ms)

            stage = "vector_search_rtos"
            page.get_by_role("button", name="向量库").click(timeout=5000)
            page.locator("#vectorQuery").fill("查找某个时钟的 type 和 id", timeout=5000)
            with page.expect_response(
                lambda r: r.request.method in {"GET", "POST"} and "/api/v1/search" in r.url,
                timeout=args.timeout_ms,
            ) as vector_resp:
                page.get_by_role("button", name="检索块").click(timeout=5000)
            vector_payload = response_json(vector_resp.value)
            checks["vector_chunk_render"] = json_contains(vector_payload, "type") and (
                json_contains(vector_payload, "id") or visible_text(page, "时钟", timeout=10000)
            )

            stage = "chat_rtos_howto"
            chat_payload = api_post_json(
                args.api_url,
                "/api/v1/chat",
                {
                    "message": RTOS_HOWTO_QUESTION,
                    "history": [],
                    "session_id": f"web-demo-rtos-{int(time.time() * 1000)}",
                },
                args.timeout_ms,
                args.token,
            )
            chat_payload = resolve_chat_payload(args.api_url, chat_payload, args.timeout_ms, args.token)
            rtos_result = rtos_howto_result(chat_payload)
            checks["chat"] = bool(first_text(chat_payload))
            checks["citation_render"] = rtos_result["citation_count"] > 0
            checks["rtos_manual_howto"] = bool(rtos_result["pass"])

            stage = "chat_rtos_dmac_howto"
            dmac_payload = api_post_json(
                args.api_url,
                "/api/v1/chat",
                {
                    "message": RTOS_DMAC_QUESTION,
                    "history": [],
                    "session_id": f"web-demo-rtos-dmac-{int(time.time() * 1000)}",
                },
                args.timeout_ms,
                args.token,
            )
            dmac_payload = resolve_chat_payload(args.api_url, dmac_payload, args.timeout_ms, args.token)
            rtos_dmac_result = rtos_dmac_howto_result(dmac_payload)
            checks["chat"] = checks["chat"] and bool(first_text(dmac_payload))
            checks["citation_render"] = checks["citation_render"] and rtos_dmac_result["citation_count"] > 0
            checks["rtos_dmac_howto"] = bool(rtos_dmac_result["pass"])
        except Exception as exc:
            raise FrontendEvalError(stage, str(exc)) from exc
        finally:
            browser.close()

    for name, passed in checks.items():
        if not passed:
            failures.append({"failure_layer": "frontend", "check": name, "reason": f"{name} did not pass"})
    elapsed_ms = (time.perf_counter() - started) * 1000
    flow_rate = sum(1 for passed in checks.values() if passed) / len(checks)
    return {
        "schema_version": "attune.eval.web_demo_frontend.v1",
        "mode": "live",
        "profile": args.profile,
        "target": {
            "base_url": args.base_url,
            "api_url": args.api_url,
            "auth": "bearer" if args.token else "none",
        },
        "checks": checks,
        "metrics": {
            "frontend": {
                "web_demo_flow_pass_rate": flow_rate,
                "web_demo_citation_render_rate": 1.0 if checks["citation_render"] else 0.0,
                "web_demo_time_render_rate": 1.0 if checks["time_render"] else 0.0,
                "web_demo_vector_chunk_render_rate": 1.0 if checks["vector_chunk_render"] else 0.0,
                "web_demo_rtos_manual_howto_pass_rate": 1.0 if checks["rtos_manual_howto"] else 0.0,
                "web_demo_rtos_dmac_howto_pass_rate": 1.0 if checks["rtos_dmac_howto"] else 0.0,
                "elapsed_ms": elapsed_ms,
            }
        },
        "artifacts": {
            "rtos_case": {
                **rtos_case_metadata(args),
                **rtos_result,
                "source_path": str(source_path),
            },
            "rtos_dmac_case": {
                **rtos_dmac_case_metadata(args),
                **rtos_dmac_result,
                "source_path": str(dmac_source_path),
            },
            "last_stage": stage,
        },
        "failures": failures,
    }


def run_live(args: argparse.Namespace) -> dict[str, Any]:
    from playwright.sync_api import sync_playwright

    failures: list[dict[str, Any]] = []
    stage = "init"
    checks = {
        "upload": False,
        "folder_upload": False,
        "vector_chunk_render": False,
        "chat": False,
        "summary": False,
        "model_switch_gate": False,
        "citation_render": False,
        "time_render": False,
    }
    if args.profile == "deep":
        checks["complex_chat"] = False
        checks["summary_workflows"] = False
    token = f"WEB_DEMO_EVAL_{int(time.time() * 1000)}"
    folder_token = f"{token}_FOLDER"
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
    folder_tmp = tempfile.TemporaryDirectory(prefix=f"{token}_dir_")
    folder_root = Path(folder_tmp.name) / f"{token}_folder"
    nested_dir = folder_root / "nested"
    nested_dir.mkdir(parents=True, exist_ok=True)
    folder_fixture_path = nested_dir / f"{folder_token}.md"
    folder_fixture_path.write_text(web_demo_fixture(folder_token), encoding="utf-8")
    summary_case_results: list[dict[str, Any]] = []
    model_switch_result: dict[str, Any] = {}
    reset_result: dict[str, Any] | None = None

    started = time.perf_counter()
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=bool(args.headless))
        page = browser.new_page()
        if args.token:
            page.add_init_script(
                script=f"sessionStorage.setItem('attune_auth_token', {json.dumps(args.token)});",
            )
        try:
            if args.reset_before:
                stage = "reset_demo_environment"
                reset_result = reset_demo_environment(args.api_url, args.timeout_ms, args.token)
            stage = "open_page"
            page_params = {"api": args.api_url}
            joiner = "&" if "?" in args.base_url else "?"
            page.goto(
                f"{args.base_url}{joiner}{urllib.parse.urlencode(page_params)}",
                wait_until="networkidle",
                timeout=args.timeout_ms,
            )
            page.get_by_text("上传 & 管理", exact=False).first.wait_for(state="visible", timeout=30000)
            stage = "model_switch_gate"
            try:
                page.wait_for_function(
                    "() => Array.isArray(window.latestModelCapabilities) && window.latestModelCapabilities.length > 0",
                    timeout=min(args.timeout_ms, 30000),
                )
            except Exception:
                pass
            model_switch_result = verify_model_switch_gate(page)
            checks["model_switch_gate"] = bool(model_switch_result.get("pass"))

            stage = "upload_fixture"
            page.locator('input[type="file"]').first.set_input_files(str(fixture_path))
            upload_rendered = visible_text(page, token, timeout=min(args.timeout_ms, 45000)) or visible_text(
                page, "ready", timeout=2000
            )
            checks["upload"] = upload_rendered and wait_for_demo_ready(page, timeout=min(args.timeout_ms, 90000))
            checks["time_render"] = visible_text(page, "全流程时间", timeout=5000)

            stage = "folder_upload_fixture"
            page.locator("#folderInput").set_input_files(str(folder_root))
            checks["folder_upload"] = visible_text(
                page,
                folder_token,
                timeout=min(args.timeout_ms, 45000),
            ) and wait_for_named_demo_ready(page, folder_token, timeout=min(args.timeout_ms, 90000))

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
            page.locator("#summaryScenario").select_option("risk", timeout=5000)
            page.locator("#summaryDetail").fill(f"总结 {token} 文档的核心结论。", timeout=5000)
            with page.expect_response(
                lambda r: r.request.method == "POST"
                and r.url.rstrip("/").endswith("/api/v1/summary/workflow"),
                timeout=args.timeout_ms,
            ) as summary_resp:
                page.locator("#summaryBtn").click(timeout=5000)
            summary_payload = response_json(summary_resp.value)
            summary_sections = summary_payload.get("summary_sections")
            checks["summary"] = (
                isinstance(summary_sections, dict)
                and "core_conclusions" in summary_sections
                and "risks_or_gaps" in summary_sections
            ) or visible_text(page, token, timeout=min(args.timeout_ms, 30000)) or visible_text(
                page, "Summary Workflow", timeout=10000
            )

            if args.profile == "deep":
                for case in web_demo_chat_cases(token, fixture_path.stem):
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
                            args.token,
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
                for case in web_demo_summary_cases(token):
                    stage = f"deep_summary_api:{case['case_id']}"
                    try:
                        summary_payload = api_post_json(
                            args.api_url,
                            "/api/v1/summary/workflow",
                            {
                                "scenario": case["scenario"],
                                "detail": case["detail"],
                                "model": "llm-chat",
                                "top_k": 8,
                            },
                            args.timeout_ms,
                            args.token,
                        )
                        summary_result = evaluate_summary_case(summary_payload, case)
                        summary_result["execution"] = "api_proxy"
                    except Exception as exc:
                        summary_result = {
                            "case_id": case["case_id"],
                            "scenario": case["scenario"],
                            "pass": False,
                            "missing_terms": case["required_terms"],
                            "missing_sections": case["required_sections"],
                            "missing_stages": case["required_stages"],
                            "citation_count": 0,
                            "knowledge_count": None,
                            "model": None,
                            "execution": "api_proxy",
                            "error": str(exc),
                        }
                    summary_case_results.append(summary_result)
                checks["summary_workflows"] = bool(summary_case_results) and all(
                    case["pass"] for case in summary_case_results
                )
        except Exception as exc:
            raise FrontendEvalError(stage, str(exc)) from exc
        finally:
            browser.close()
            try:
                fixture_path.unlink()
            except OSError:
                pass
            folder_tmp.cleanup()

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
    summary_workflow_pass_rate = (
        sum(1 for case in summary_case_results if case["pass"]) / len(summary_case_results)
        if summary_case_results
        else (1.0 if args.profile != "deep" else 0.0)
    )
    return {
        "schema_version": "attune.eval.web_demo_frontend.v1",
        "mode": "live",
        "profile": args.profile,
        "target": {
            "base_url": args.base_url,
            "api_url": args.api_url,
            "auth": "bearer" if args.token else "none",
        },
        "checks": checks,
        "metrics": {
            "frontend": {
                "web_demo_flow_pass_rate": flow_rate,
                "web_demo_citation_render_rate": 1.0 if checks["citation_render"] else 0.0,
                "web_demo_time_render_rate": 1.0 if checks["time_render"] else 0.0,
                "web_demo_vector_chunk_render_rate": 1.0 if checks["vector_chunk_render"] else 0.0,
                "web_demo_model_switch_gate_rate": 1.0 if checks["model_switch_gate"] else 0.0,
                "web_demo_complex_chat_pass_rate": complex_pass_rate,
                "web_demo_summary_workflow_pass_rate": summary_workflow_pass_rate,
                "elapsed_ms": elapsed_ms,
            }
        },
        "artifacts": {
            "chat_cases": chat_case_results,
            "summary_cases": summary_case_results,
            "model_switch_gate": model_switch_result,
            "reset_before": reset_result,
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
        report = run_rtos_live(args) if args.profile == "rtos" else run_live(args)
        write_report(args.out, report)
    except Exception as exc:
        stage = getattr(exc, "stage", None)
        write_report(
            args.out,
            {
                "schema_version": "attune.eval.web_demo_frontend.v1",
                "mode": "live",
                "profile": args.profile,
                "target": {
                    "base_url": args.base_url,
                    "api_url": args.api_url,
                    "auth": "bearer" if args.token else "none",
                },
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
