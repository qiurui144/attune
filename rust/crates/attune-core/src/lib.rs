//! # attune-core
//!
//! attune 私有 AI 知识伙伴 — 核心库。
//!
//! 提供 vault (Argon2id + AES-256-GCM 加密存储) / store (SQLite + FTS5 全文索引) /
//! vectors (usearch HNSW + f16) / chunker / parser / embed / chat (含 PII redactor +
//! evidence chain) / classifier / clusterer / web search 等 ~50 个模块。
//!
//! ## 主要 facade
//!
//! - [`vault::Vault`] — 加密存储入口, unlock 后暴露 [`store::Store`]
//! - [`search`] — RRF 混合搜索 (vector + FTS) + 两阶段层级 + 注入预算
//! - [`chunker`] — 滑动窗口 + 章节切割, code fence balanced
//! - [`chat::ChatEngine`] — RAG 对话引擎, 含 PII redact + cite extraction
//! - [`embed::EmbeddingProvider`] — Ollama / ONNX 抽象 trait
//! - [`classifier::Classifier`] + [`taxonomy::Taxonomy`] — 自动分类
//! - [`async_fs`] — spawn_blocking 包装 fs ops, async handler 友好
//!
//! ## 加密策略
//!
//! - 主密码 → Argon2id 派生 KEK → 解 DEK (随机 32B)
//! - 字段级 AES-256-GCM (items.content / tags / embed_queue.chunk_text)
//! - title/url/created_at TEXT 明文 (per privacy trade-off, list 性能优先)
//!
//! ## 三档隐私 (per Phase A.5)
//!
//! - L0 🔒 chunk 永不出网 (强制本地 LLM)
//! - L1 默认 12 PII 类脱敏 → 云端 LLM
//! - L3 LLM 语义脱敏 (v0.7, K3 一体机)
//!
//! ## Stable public API for routing consumers (Plan A2 dependency anchor)
//!
//! The following types and functions are frozen as of Plan A1 Task M. Plan A2
//! (hybrid token routing) consumes this surface directly; bumping or renaming
//! anything below must coordinate with `docs/superpowers/plans/2026-05-28-
//! hybrid-token-routing.md`.
//!
//! ```
//! # use attune_core::{
//! #     TokenUsage, UsageEvent, UsageKind, CacheOutcome, CallOutcome, ErrorKind,
//! #     UsageRecorderGuard, UsageAggregator,
//! #     CacheBackend, CacheScope, CachedValue, cache_key,
//! # };
//! let _ = TokenUsage::empty("p", "m");
//! let _key: String = cache_key("gpt-4o-mini", "hello");
//! let _scope = CacheScope::Llm;
//! ```

pub mod ai_annotator;
pub mod annotation_weight;
// v1.0.6 Privacy Logic Strategy — single outbound enforcement entry-point.
// Every network egress (LLM / Cloud SaaS / WebDAV / Web Search / Telemetry)
// MUST be wrapped by OutboundGate::enforce so settings + PII redactor are
// consulted in one place. See docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md.
pub mod outbound_gate;
pub use outbound_gate::{OutboundError, OutboundGate, OutboundKind, OutboundPolicy};
// v1.0.6 Privacy Logic Strategy — default-off telemetry queue.
// Stub for v1.0.6: ships queue + default-false persistence, no HTTP send yet.
// Actual send gated behind future v1.1 toggle AND privacy.telemetry == true.
pub mod telemetry;
// async_fs: D3 review 引入 — async-safe fs helpers (spawn_blocking 包装).
// 新代码默认走 async_fs::*, 防止 future async handler 误调用 sync std::fs.
pub mod async_fs;
// usage / cache: Plan A1 — Cache/Context/Token standard API
// spec: docs/superpowers/specs/2026-05-28-cache-context-token-standard-api.md
// Public surface frozen at Task M for Plan A2 routing consumers.
pub mod cache;
pub mod usage;
// agent_quality: ACP-2 unified quality gate orchestration (workspace manifest SSOT).
// spec: docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md §3 ACP-2
pub mod agent_quality;

// ── Plan A1 Task M: frozen public API surface for Plan A2 routing consumers ──
//
// Any rename / removal / signature change to the items re-exported below is a
// **breaking change** for Plan A2 (hybrid token routing). Plan A2's
// `CapabilityRouter` imports these names directly from `attune_core::*` (NOT
// from `attune_core::usage::types::*` / `attune_core::cache::*`) — keeping the
// crate root stable means A2 can be developed against this commit without
// needing to track sub-module reshuffles.
//
// Spec anchor: docs/superpowers/specs/2026-05-28-cache-context-token-
// standard-api.md §8 ("Stable public API"). Plan A2 blockedBy = this commit.
pub use usage::{
    CacheOutcome, CallOutcome, ErrorKind, TokenUsage, UsageEvent, UsageKind,
    UsageRecorderGuard, UsageAggregator,
};
pub use cache::{CacheBackend, CacheScope, CachedValue, cache_key};
// chat 模块整体 pub(crate) — ChatEngine 只能内部构造（依赖 Vault/Store internal types）。
// 外部消费者（attune-server route）通过本 crate re-export 拿到 Citation / ChatResponse /
// parse_confidence / strip_confidence_marker 这些公开 API（per reviewer I3）。
pub(crate) mod chat;
pub use chat::{parse_confidence, strip_confidence_marker, Citation, ChatEngine, ChatResponse};
// chat_reliability — post-hoc deterministic evaluation agent for LLM chat
// responses (citation grounding + factual consistency + hallucination flag).
// Zero LLM cost, designed to run from a background tokio task after each
// chat turn. See module-level docs for cost contract + verification doctrine.
pub mod chat_reliability;
pub mod chunker;
pub mod context_compress;
pub mod context_budget;
pub mod plugin_hub;
pub mod plugin_loader;
pub mod plugin_registry;
pub mod capability_dispatch;
pub mod skills;
pub mod agents;
pub mod agent_telemetry;
pub mod feedback;
pub mod mcp_client;
pub mod case_metadata;
pub mod plugin_encryption;
pub mod ui_runtime;
pub mod agent_runner;
// 2026-05-20: license / license_cache / accounts_client / device_binding 模块
// 被移到 attune-accounts (OSS reference SaaS) — live cloud-Bearer-token path
// 不走 Ed25519 SignedLicense, 这些类型只有 attune-accounts 在用, 留在 attune-core
// 是 footgun. 删了它们, 同时把 LicenseCache 启动时的死代码也从 state.rs 删掉.
pub mod member_session;
pub mod cloud_client;
pub mod plugin_sync;
pub mod plugin_sig;
pub mod classifier;
pub mod clusterer;
pub mod crypto;
pub mod embed;
pub mod entities;
pub mod infer;
pub mod error;
pub mod index;
pub mod ingest;
pub mod intent_router;
pub mod governor;
pub mod llm;
pub mod llm_settings;
pub mod ocr;  // v0.6.0-rc.3: pub for ai_stack status API
pub mod asr;
pub mod office_job_queue;
pub mod parser;
pub mod pii;
pub mod platform;
pub mod memory_consolidation;
pub mod memory;
pub mod project_recommender;
pub mod queue;
pub mod backup;
pub mod reindex;
pub mod resource_governor;
pub mod scanner;
pub mod scanner_patent;
pub mod scanner_webdav;
pub mod search;
pub mod store;
pub mod tag_index;
pub mod taxonomy;
pub mod vault;
pub mod vectors;
pub mod skill_evolution;

// v0.7 sprint feature modules
pub mod cost;
pub mod tools;
pub mod demo;
pub mod query_rewrite;
pub mod entity_graph;
pub mod linker;
pub mod skill_eval;
pub mod report;
pub mod reader;
pub mod web_search;
pub mod web_search_browser;
pub(crate) mod web_search_engines;
pub mod workflow;
pub mod capture;
pub mod sync;
pub mod vlm;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
