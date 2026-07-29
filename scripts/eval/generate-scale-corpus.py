#!/usr/bin/env python3
"""Generate deterministic multi-domain Markdown corpora for scale gates."""
from __future__ import annotations

import argparse
from pathlib import Path


TOPICS = {
    "networking": [
        "TCP/IP routing",
        "DNS resolution",
        "firewall policy",
        "packet capture",
        "service port timeout",
    ],
    "security": [
        "access control",
        "incident response",
        "asset inventory",
        "risk assessment",
        "audit evidence",
    ],
    "product": [
        "account setup",
        "API integration",
        "billing workflow",
        "support escalation",
        "release notes",
    ],
    "finance": [
        "filing risk",
        "revenue change",
        "operating expense",
        "cash flow",
        "management discussion",
    ],
    "mechanical": [
        "gear transmission",
        "bearing selection",
        "shaft strength",
        "hydraulic control",
        "maintenance access",
    ],
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--documents", type=int, required=True)
    parser.add_argument("--domains", required=True, help="Comma-separated domain list.")
    parser.add_argument("--out", type=Path, required=True)
    return parser.parse_args()


def parse_domains(raw: str) -> list[str]:
    domains = [part.strip() for part in raw.split(",") if part.strip()]
    if not domains:
        raise SystemExit("at least one domain is required")
    unknown = [domain for domain in domains if domain not in TOPICS]
    if unknown:
        raise SystemExit(f"unknown domains: {', '.join(unknown)}")
    return domains


def slugify(value: str) -> str:
    out = []
    previous_dash = False
    for ch in value.lower():
        if ch.isalnum():
            out.append(ch)
            previous_dash = False
        elif not previous_dash:
            out.append("-")
            previous_dash = True
    return "".join(out).strip("-") or "topic"


def render_doc(index: int, domain: str) -> str:
    topic = TOPICS[domain][index % len(TOPICS[domain])]
    source_key = f"{domain}::{slugify(topic)}"
    near_duplicate = index % 10 == 0
    support_workflow = index % 7 == 0
    lines = [
        f"# {source_key} - Attune Scale Document {index:05d}",
        "",
        f"ATTUNE_SCALE_DOC_ID=scale-{index:05d}",
        f"ATTUNE_SCALE_DOMAIN={domain}",
        f"ATTUNE_SCALE_TOPIC={topic}",
        "",
    ]
    if domain == "security":
        lines.extend(
            [
                f"Security anchor: security / 安全知识库 / {topic}.",
                "Coverage boundary anchor: if a question asks for an industry-general practice not directly covered by this manual / 知识库未直接覆盖, cite adjacent security evidence, say evidence is insufficient / 证据不足 for a manual conclusion, and separate industry-general / 行业通用 guidance from source-grounded facts.",
            ]
        )
        if topic == "incident response":
            lines.extend(
                [
                    "Incident response runbook anchor: confirm user symptom / 用户症状, request logs / 日志, and do not invent / 不要编造 unsupported operational conclusions.",
                    "Required evidence includes screenshots, timeline, alert id, affected asset, topology, configuration, diagnosis, troubleshoot, check, procedure, steps, workflow, 验证, 排查, 诊断, 流程.",
                ]
            )
        if topic == "audit evidence":
            lines.extend(
                [
                    "Audit evidence anchor: audit logs / 审计日志 are required for compliance; cannot directly mark compliant / 不能直接判定合规 when logs are missing.",
                    "Evidence gap anchor: evidence is insufficient / 证据不足; continue requesting / 继续索取 audit logs, control records, retention proof, asset owner approval, and change history.",
                    "Decision bridge anchor: audit evidence / 审计证据 supports risk assessment / 风险评估 by proving control operation, change history, and accountability before recommendations.",
                ]
            )
        if topic == "risk assessment":
            lines.extend(
                [
                    "Risk assessment anchor: risk assessment / 风险评估 must collect evidence first / 先收集证据 before mitigation or acceptance advice.",
                    "Decision support evidence includes risk factors, affected controls, likelihood, impact, and residual risk.",
                    "Decision bridge anchor: risk assessment / 风险评估 must be paired with audit evidence / 审计证据 such as audit logs, control records, retention proof, approvals, and change history.",
                ]
            )
        if topic == "access control":
            lines.extend(
                [
                    "Access control anchor: access control / 访问控制 evidence includes authorized users, roles, policy scope, approval records, and access review results.",
                    "Source-grounded response must preserve the security domain and cite access control evidence.",
                ]
            )
        lines.append("")
    lines.extend(
        [
        f"This document belongs to the {domain} knowledge base and describes {topic}.",
        "It is generated deterministically for retrieval, citation, and support workflow regression.",
        "",
        "Evidence section:",
        f"- Primary fact: {topic} requires source-grounded analysis before an answer is accepted.",
        f"- Retrieval key: {source_key}",
        "- Citation expectation: answers must cite this document id when it is the selected source.",
        ]
    )
    if domain == "security":
        lines.extend(
            [
                "",
                "Security bilingual anchors:",
                f"- Domain label: security / 安全知识库.",
                f"- Topic label: {topic}.",
                "- Boundary rule: for a specific security method not directly covered by the manual / 知识库未直接覆盖, cite related security evidence, state evidence is insufficient / 证据不足, and do not treat industry-general / 行业通用 advice as a manual conclusion / 不能当作手册结论.",
            ]
        )
        if topic == "incident response":
            lines.extend(
                [
                    "- Operation guidance: incident response triage starts by confirming the user symptom / 用户症状.",
                    "- Evidence to request: logs / 日志, screenshots, timeline, alert id, affected asset, topology, and configuration.",
                    "- Safety rule: do not invent operational conclusions / 不要编造 without cited evidence.",
                    "- Runbook signals: diagnosis, troubleshoot, check, procedure, steps, workflow, 验证, 排查, 诊断, 流程.",
                ]
            )
        if topic == "audit evidence":
            lines.extend(
                [
                    "- Audit evidence includes audit logs / 审计日志, control records, retention proof, asset owner approval, and change history.",
                    "- Compliance decision rule: cannot directly mark compliant / 不能直接判定合规 when audit logs are missing.",
                    "- Evidence gap rule: evidence is insufficient / 证据不足; continue requesting / 继续索取 audit logs and supporting records.",
                    "- Decision bridge: audit evidence / 审计证据 is required before risk assessment / 风险评估 recommendations.",
                ]
            )
        if topic == "risk assessment":
            lines.extend(
                [
                    "- Decision support: risk assessment / 风险评估 should identify risk factors, affected controls, likelihood, impact, and residual risk.",
                    "- Recommendation gate: collect evidence first / 先收集证据 before approving mitigation or acceptance.",
                    "- Decision bridge: pair risk assessment / 风险评估 with audit evidence / 审计证据 including audit logs, control records, approvals, and change history.",
                ]
            )
        if topic == "access control":
            lines.extend(
                [
                    "- Access control / 访问控制 evidence includes authorized users, roles, policy scope, approval records, and access review results.",
                    "- Source-grounded response must preserve the security domain and cite access control evidence.",
                ]
            )
    if near_duplicate:
        lines.extend(
            [
                "",
                "Near duplicate section:",
                "This is a near-duplicate distractor used to test wrong-source resistance.",
                "near-duplicate distractor",
            ]
        )
    if support_workflow:
        lines.extend(
            [
                "",
                "Support workflow:",
                "support workflow",
                "1. Confirm the user symptom.",
                "2. Retrieve the matching evidence section.",
                "3. Ask for logs, screenshots, topology, or configuration when evidence is insufficient.",
                "4. Do not invent operational conclusions without cited support.",
            ]
        )
    return "\n".join(lines) + "\n"


def main() -> int:
    args = parse_args()
    if args.documents < 1:
        raise SystemExit("--documents must be >= 1")
    domains = parse_domains(args.domains)
    args.out.mkdir(parents=True, exist_ok=True)
    for index in range(args.documents):
        domain = domains[index % len(domains)]
        topic = TOPICS[domain][index % len(TOPICS[domain])]
        topic_slug = slugify(topic)
        subdir = args.out / domain
        subdir.mkdir(parents=True, exist_ok=True)
        path = subdir / f"scale-{index:05d}-{domain}-{topic_slug}.md"
        path.write_text(render_doc(index, domain), encoding="utf-8")
    print(f"generated {args.documents} docs in {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
