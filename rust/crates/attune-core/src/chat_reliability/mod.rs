//! Chat-reliability agent — post-hoc evaluation of LLM chat responses.
//!
//! Activates a missing safety layer in the chat path: today, the LLM's answer
//! is shown to the user with **zero programmatic check** that the answer is
//! actually supported by the retrieved RAG chunks. Hallucinated dates, names,
//! and numbers reach the user surface unflagged.
//!
//! This agent runs **after** the LLM returned (background / non-blocking) and
//! emits a [`ChatReliabilityReport`] — a structured set of citation /
//! contradiction / hallucination signals + an aggregate `overall_confidence`
//! score in `[0, 1]`. The agent **does not** modify the chat response; UI /
//! downstream consumers decide whether to show a warning, gate a follow-up,
//! or log the signal for skill evolution.
//!
//! ## Cost contract (per `attune/CLAUDE.md` §"成本感知与触发契约")
//!
//! Strictly tier 🆓 / Layer 1: pure CPU heuristics over response text +
//! retrieved chunk text. **No LLM call is made.** The agent therefore must
//! never accept an [`crate::llm::LlmProvider`] in its public API and must
//! complete in milliseconds for typical inputs (response ≤ 8 KB, ≤ 20 chunks).
//!
//! ## Three evaluation dimensions
//!
//! 1. **Citation grounding** — every `item_id` referenced inline in the
//!    response (as `[item:...]` / `[source:...]` marker) is checked against
//!    the retrieved chunk list: present? content overlap with response?
//! 2. **Factual consistency** — response sentences that contain a concrete
//!    claim (number / date / proper noun) are checked against the union of
//!    retrieved chunk text. A directly contradicting chunk (same entity,
//!    different value) → [`Contradiction`].
//! 3. **Hallucination flag** — claim tokens in the response (specific
//!    numbers, ISO dates, organization-suffix tokens) that **do not appear
//!    in any retrieved chunk** → [`HallucinationFlag`]. Heuristic only; no
//!    "is / is not hallucination" verdict — caller post-processes the
//!    `severity` field.
//!
//! ## Agent verification doctrine (per `attune-pro/docs/agent-skill-training-methodology.md`)
//!
//! - **Independent ground truth**: golden YAMLs carry hand-derived
//!   `expected_grounded` / `expected_contradictions` / `expected_flags` plus
//!   a `# DERIVATION:` comment showing how the values were computed without
//!   calling [`evaluate_response`].
//! - **6-class coverage floor** (enforced by `chat_reliability_golden_gate.rs`):
//!     - ≥ 10 real golden cases — derived from real RAG corpora
//!       (rust-book, cs-notes, openai-cookbook excerpts).
//!     - ≥ 3 error cases (empty response / no chunks / unicode-malformed).
//!     - ≥ 5 boundary `#[test]`s in `tests/chat_reliability_boundary.rs`.
//!     - ≥ 3 proptest invariants (confidence ∈ [0,1] / monotone / idempotent).
//!     - ≥ 1 integration test (end-to-end through a fresh `Store`).
//! - **Deterministic**: no RNG, no system clock dependency, no LLM call.
//!   Re-running on the same `(response, chunks, query)` triple yields the
//!   identical report (asserted by proptest invariant).
//! - **ENFORCE mode 0 violations**: the gate `#[test]` panics on any
//!   golden case mismatch.

pub mod agent;

pub use agent::{
    confidence_from_signals, evaluate_response, normalize_text, ChatReliabilityConfig,
    ChatReliabilityReport, CitationCheck, CitationStatus, Contradiction, ContradictionKind,
    HallucinationFlag, HallucinationKind, HallucinationSeverity, RetrievedChunk,
};
