# Attune RAG Profile and Vector Conversation Readiness Requirements

Date: 2026-07-23

## Goal

Make Attune's own RAG layer robust enough that scheduler improvements are not masking product gaps.

The system must support both covered and uncovered user questions:

- If the knowledge base contains relevant evidence, retrieve enough of it for grounded chat or summary.
- If the knowledge base does not contain relevant evidence, refuse clearly instead of producing generic model knowledge.
- If the first retrieval pass is weak, run bounded recovery retrieval before answering.
- Chat and Summary must use distinct RAG profiles, prompts, budgets, and fallback semantics.

## Core Concern

Current E2E proved simple factual RAG works, for example:

- User asks: `TCP/IP 起源于哪里？`
- Knowledge base contains: origin, ARPA/DARPA, ARPANET, Vint Cerf, Bob Kahn.
- Retrieval finds evidence and Chat RAG answers correctly.

But real users often shift intent:

- `我们应该如何排查 TCP/IP？`
- `TCP/IP 连接失败时怎么定位？`
- `为什么 ping 通但业务不通？`
- `根据这份网络文档，总结排障步骤。`

These are not the same as origin lookup. If the KB only contains origin history, Attune must not turn that into troubleshooting advice. If the KB contains troubleshooting material but first retrieval only finds origin chunks, Attune must recover before answering.

## Failure Modes to Cover

### F1. Intent Shift

Same entity, different task:

- `TCP/IP 起源` = fact lookup.
- `TCP/IP 如何排查` = diagnostic/procedure.
- `TCP/IP 和 OSI 区别` = comparison.
- `总结 TCP/IP 文档` = summary/synthesis.

Required behavior:

- Detect the user intent class before final retrieval:
  - `lookup`
  - `diagnostic`
  - `procedure`
  - `comparison`
  - `summary`
  - `definition`
  - `source_lookup`
- Use different retrieval and answer budgets per intent.
- Do not answer a diagnostic/procedure question from pure history/definition evidence.

### F2. Semantic Coverage Gap

Vector search may miss relevant chunks when user wording differs from document wording.

Example:

- User: `如何排查 TCP/IP？`
- Document terms: `connectivity troubleshooting`, `packet loss`, `DNS`, `gateway`, `route table`, `firewall`.

Required behavior:

- Run multi-query retrieval for non-lookup intents:
  - original query;
  - entity-focused query: `TCP/IP troubleshooting`;
  - task-focused query: `network connectivity diagnosis steps`;
  - Chinese/English bilingual expansion when language mismatch is likely.
- Fuse results across BM25/vector/RRF.
- Keep a trace of retrieval queries in response metadata for debug.

### F3. Insufficient Evidence

Retrieval returns chunks, but they do not support the requested task.

Example:

- KB has TCP/IP origin history only.
- User asks for production troubleshooting steps.

Required behavior:

- Compute evidence coverage before answer generation.
- Required coverage depends on intent:
  - lookup: at least one directly relevant citation.
  - comparison: at least two comparable evidence groups.
  - summary: multiple chunks or one sufficiently long source.
  - diagnostic/procedure: evidence must contain actionable diagnostic concepts, not just entity mentions.
- If coverage fails, answer with refusal:
  - state what evidence was found;
  - state what is missing;
  - ask user to upload relevant troubleshooting/operation material.

### F4. Threshold Blind Spots

A fixed vector threshold can drop relevant results or admit weak results.

Current edge retrieval uses a vector min score around `0.65` for RAG. This can be too strict for paraphrased diagnostic questions and too loose for exact factual questions.

Required behavior:

- Threshold is profile-driven, not one global constant.
- For diagnostic/summary/comparison, allow a lower recovery threshold after first-pass failure.
- Keep exact-match/BM25 paths active for entity-heavy questions.
- Report whether retrieval used:
  - `first_pass`
  - `expanded_query`
  - `lower_threshold_recovery`
  - `lexical_fallback`

### F5. Summary Coverage Gap

Summary questions need document-level coverage, not just top raw chunks.

Required behavior:

- Summary RAG should prefer:
  - source-diverse chunks;
  - section headings;
  - beginning/end chunks;
  - L1/L2/L3 summaries when available;
  - raw cited chunks for final citation grounding.
- Extractive summary fallback must be labeled as fallback, not model summary.

## Required RAG Profiles

The `oss-rag-default` plugin must drive runtime behavior, not only declare metadata.

### `default_kb_chat`

Use for normal factual or short-form KB Q&A.

Profile requirements:

- Retrieval top_k: 5-8 final chunks.
- Context: short cited windows.
- Answer model: `llm-chat` when local scheduler is selected.
- Output: concise answer with citations.
- Refuse when evidence is missing.
- Recovery: one bounded expanded retrieval pass before refusal.

### `default_kb_diagnostic`

Use when the user asks how to troubleshoot, debug, diagnose, fix, or investigate.

Profile requirements:

- Retrieval includes task expansion:
  - `troubleshoot`
  - `diagnose`
  - `failure`
  - `symptom`
  - `root cause`
  - Chinese equivalents: `排查`, `定位`, `故障`, `原因`, `步骤`
- Evidence must contain diagnostic/procedure concepts.
- If not, refuse with missing-evidence explanation.
- Do not provide operational steps from generic model knowledge.

### `default_kb_summary`

Use for summaries and synthesis.

Profile requirements:

- Retrieval top_k: 8-12 final chunks.
- Prefer source-diverse and section-diverse evidence.
- Use model summary when configured and healthy.
- Use extractive fallback only as explicit degraded mode.
- Output must include:
  - core conclusion;
  - key evidence;
  - gaps/risks/todos.

## Response Metadata Requirements

Every `/api/v1/chat` RAG response must expose:

```json
{
  "rag_profile": "default_kb_chat",
  "intent": "lookup",
  "answer_mode": "llm-chat",
  "degraded": false,
  "degraded_reason": null,
  "retrieval": {
    "strategy": "hybrid_rrf",
    "passes": ["first_pass"],
    "queries": ["TCP/IP 起源于哪里"],
    "final_top_k": 5,
    "vector_results": 2,
    "bm25_results": 3,
    "coverage_score": 0.86,
    "coverage_status": "sufficient"
  },
  "citations_count": 2,
  "knowledge_count": 2
}
```

Allowed `answer_mode` values:

- `llm-chat`
- `llm-summary`
- `extractive-answer`
- `extractive-summary`
- `refusal-insufficient-evidence`
- `async-job`

## Acceptance Tests

### T1. Covered Lookup

Knowledge:

```text
TCP/IP 起源于 ARPA/DARPA 资助的 ARPANET 互联网络研究。
```

Question:

```text
TCP/IP 起源于哪里？
```

Expected:

- `intent=lookup`
- `coverage_status=sufficient`
- answer mentions ARPA/DARPA and ARPANET.
- citations_count >= 1.

### T2. Uncovered Diagnostic

Knowledge:

```text
TCP/IP 起源于 ARPA/DARPA 资助的 ARPANET 互联网络研究。
```

Question:

```text
我们应该如何排查 TCP/IP 连接失败？
```

Expected:

- `intent=diagnostic`
- retrieval may find the TCP/IP origin chunk.
- `coverage_status=insufficient`
- answer_mode=`refusal-insufficient-evidence`
- response says current KB lacks troubleshooting evidence.
- response must not invent ping/DNS/firewall steps.

### T3. Covered Diagnostic With Different Wording

Knowledge:

```text
Network connectivity diagnosis should verify link status, IP address, subnet mask, gateway route, DNS resolution, firewall policy, packet loss, and application port reachability.
```

Question:

```text
我们应该如何排查 TCP/IP 连接失败？
```

Expected:

- `intent=diagnostic`
- retrieval uses expanded query.
- `coverage_status=sufficient`
- answer includes only cited diagnostic steps from KB.
- citations_count >= 1.

### T4. Summary Coverage

Knowledge:

- RAG overview chunk.
- Retrieval chunk.
- Reranking chunk.
- Context window chunk.
- Safety checks chunk.

Question:

```text
总结这份 RAG 文档的检索、重排序、上下文窗口和安全检查建议。
```

Expected:

- `intent=summary`
- `rag_profile=default_kb_summary`
- evidence spans multiple chunks.
- answer includes core conclusion, key evidence, gaps/risks/todos.
- if model summary unavailable, response metadata says `answer_mode=extractive-summary`.

### T5. Weak Retrieval Recovery

Knowledge:

```text
Connectivity diagnosis includes route table inspection and DNS resolution checks.
```

Question:

```text
业务能 ping 通但访问不了，怎么定位？
```

Expected:

- first pass may be weak.
- recovery pass expands to connectivity/application reachability.
- if evidence found, answer grounded.
- if evidence not found, refuse.

## Implementation Direction

1. Load active RAG profile from plugin registry.
2. Add intent classifier before retrieval.
3. Build retrieval request from profile + intent.
4. Add bounded multi-query recovery.
5. Add evidence coverage scoring before answer generation.
6. Route answer mode:
   - sufficient + chat -> `llm-chat`;
   - sufficient + summary -> `llm-summary` or explicit extractive fallback;
   - insufficient -> refusal.
7. Return structured retrieval/coverage metadata.

## Release Gate

Attune RAG readiness is not complete until T1-T5 pass without relying on scheduler instability or model parametric knowledge.
