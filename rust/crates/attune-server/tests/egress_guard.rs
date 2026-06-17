//! R1.1b anti-recurrence guard — every outbound HTTP client construction in
//! `attune-server` / `attune-core` src must be REGISTERED in the allowlist
//! below, with a rationale (destination class + OutboundGate status).
//!
//! Why a text-scan test: the OutboundGate audit (2026-06-08 coverage scan +
//! R1.1b) found egress points added without gate wiring (version.rs GitHub
//! fetch). A clippy lint can't carry per-callsite rationale; this test makes
//! "new un-registered egress point" a hard test failure, forcing the author to
//! (a) classify the destination, (b) wire/justify the gate, (c) register here.
//!
//! Scan pattern: `reqwest::Client::builder|new(` / `reqwest::blocking::Client::
//! builder|new(` / `reqwest::get(` / `reqwest::blocking::get(`, comment lines
//! skipped. Allowlist keys on per-file occurrence COUNT (line numbers churn too
//! much; a count change = an egress point was added/removed → re-audit).

use std::path::{Path, PathBuf};

/// (relative path from rust/crates/, expected count, rationale: destination + gate status)
const ALLOWLIST: &[(&str, usize, &str)] = &[
    (
        "attune-server/src/routes/version.rs",
        1,
        "GitHub releases API (update check). Gated: OutboundGate kind=Telemetry, \
         honors settings.privacy.telemetry, fail-closed default-off (R1.1b).",
    ),
    (
        "attune-server/src/routes/status.rs",
        2,
        "Ollama localhost:11434 probes (/api/tags + /api/ps). Local destination \
         (hardcoded loopback) — no egress, gate not required.",
    ),
    (
        "attune-server/src/routes/llm.rs",
        3,
        "(1) test_llm: user-initiated BYOK endpoint test — explicit user action on \
         user-supplied endpoint+key, wizard/Settings 'Test connection' button; \
         payload is the literal 'ping', no vault data. (2) probe_k3: loopback + \
         RFC1918 subnet scan local; user-supplied non-local candidates gated via \
         OutboundGate kind=Llm (R1.1b). (3) lmstudio_probe: compile-time \
         localhost:1234 constant — local, no gate.",
    ),
    (
        "attune-server/src/test_support.rs",
        5,
        "Test harness only (#[doc(hidden)]) — all requests target the in-process \
         eval server on 127.0.0.1. Never compiled into a production call path.",
    ),
    (
        "attune-core/src/embed.rs",
        2,
        "Embedding providers (Ollama / OpenAI-compat). Endpoint from user \
         settings; default is local Ollama/ONNX (local_destination=true). \
         Cloud endpoints are user-configured BYOK; gated via OutboundKind::Embedding \
         in state.rs::start_queue_worker (L0 item filter) and embed_pending_memories. \
         AppState::embedding_is_local flag drives gate enforcement (#82 P0 fix).",
    ),
    (
        "attune-core/src/llm.rs",
        2,
        "LLM providers (Ollama / OpenAI-compat). Egress is enforced at call \
         sites: chat.rs F-17 gate + RedactingLlmProvider, documents.rs I1/I2 \
         privacy-gate; provider itself is the transport.",
    ),
    (
        "attune-core/src/cloud_client.rs",
        1,
        "Attune Cloud SaaS (accounts/billing/DSAR). kind=CloudSaas; user-initiated \
         login/DSAR flows; wipe-cloud-session provides the privacy off-switch.",
    ),
    (
        "attune-core/src/plugin_sync.rs",
        1,
        "PluginHub download on member login (Bearer-auth'd, signature-verified \
         packages). kind=CloudSaas; runs only on explicit user login.",
    ),
    (
        "attune-core/src/plugin_hub.rs",
        1,
        "HttpPluginHubProvider — marketplace browsing/install, only active after \
         user configures settings.pluginhub.url (default MockPluginHubProvider).",
    ),
    (
        "attune-core/src/web_search_browser.rs",
        1,
        "Web search HTTP fallback. Gated: chat.rs F-17 G1 checks \
         settings.privacy.web_search before any search; provider also carries \
         with_outbound_policy defense-in-depth.",
    ),
    (
        "attune-core/src/ingest/rss.rs",
        1,
        "RSS feed fetch — user explicitly configured feed URLs (data source the \
         user asked to ingest). Worker-side; runs only for user-added feeds.",
    ),
    (
        "attune-core/src/ocr/ppocr.rs",
        1,
        "PP-OCR model download (one-time asset fetch from release mirror), not \
         user-content egress. No vault data on the wire.",
    ),
    // layout.rs egress removed (S8 refactor routed layout downloads through
    // download_hf_file_from in model_store.rs; entry removed per egress_guard stale check).
    (
        "attune-core/src/infer/model_source.rs",
        1,
        "S8 dynamic model source: probe_source_with — health/latency probe sent to \
         builtin model mirror candidates (company-mirror / ModelScope / HF) ONLY during \
         pre-flight or explicit /api/v1/ai-stack/refresh trigger; never on request path. \
         Destination = model asset mirrors (no vault data). No OutboundGate needed: \
         (a) probes carry no PII/vault content; (b) triggered only by AI-stack init or \
         explicit user action; (c) equivalent to ppocr.rs pattern (one-time asset infra).",
    ),
    (
        "attune-core/src/infer/model_store.rs",
        1,
        "S8 dynamic model download: download_hf_file_from — downloads a model binary \
         from the S8-selected best source (company-mirror first, HF fallback). Called only \
         when a model file is absent/stale; no vault data on the wire. Same destination \
         class as ppocr.rs (one-time asset fetch from release mirror); no OutboundGate \
         needed for asset downloads that carry no user content.",
    ),
];

const PATTERNS: &[&str] = &[
    "reqwest::Client::builder(",
    "reqwest::Client::new(",
    "reqwest::blocking::Client::builder(",
    "reqwest::blocking::Client::new(",
    "reqwest::get(",
    "reqwest::blocking::get(",
];

fn crates_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = .../rust/crates/attune-server
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .to_path_buf()
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let p = entry.path();
        if p.is_dir() {
            collect_rs_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// Count egress-client constructions in a file, skipping `//` comment lines.
fn count_egress_points(path: &Path) -> usize {
    let src = std::fs::read_to_string(path).expect("read source");
    src.lines()
        .filter(|line| {
            let t = line.trim_start();
            !t.starts_with("//") && !t.starts_with('*')
        })
        .map(|line| PATTERNS.iter().filter(|pat| line.contains(*pat)).count())
        .sum()
}

#[test]
fn every_outbound_http_client_is_registered() {
    let root = crates_root();
    let mut files = Vec::new();
    for crate_dir in ["attune-server/src", "attune-core/src"] {
        collect_rs_files(&root.join(crate_dir), &mut files);
    }

    let mut violations = Vec::new();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for file in &files {
        let count = count_egress_points(file);
        if count == 0 {
            continue;
        }
        let rel = file
            .strip_prefix(&root)
            .expect("under crates root")
            .to_string_lossy()
            .replace('\\', "/");
        seen.insert(rel.clone(), count);
        match ALLOWLIST.iter().find(|(p, _, _)| *p == rel) {
            None => violations.push(format!(
                "UNREGISTERED egress point(s) in {rel} ({count} occurrence(s)). \
                 Classify the destination, wire (or justify skipping) OutboundGate, \
                 then register it in tests/egress_guard.rs ALLOWLIST with a rationale."
            )),
            Some((_, expected, _)) if *expected != count => violations.push(format!(
                "{rel}: egress-point count changed (allowlist {expected}, found {count}). \
                 An outbound client was added/removed — re-audit the file and update \
                 tests/egress_guard.rs ALLOWLIST."
            )),
            Some(_) => {}
        }
    }

    // Stale allowlist entries (file deleted / egress removed) must be pruned too,
    // so the allowlist stays an accurate egress inventory.
    for (path, _, _) in ALLOWLIST {
        if !seen.contains_key(*path) {
            violations.push(format!(
                "STALE allowlist entry: {path} no longer contains egress points — \
                 remove it from tests/egress_guard.rs ALLOWLIST."
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "egress guard failed:\n{}",
        violations.join("\n")
    );
}
