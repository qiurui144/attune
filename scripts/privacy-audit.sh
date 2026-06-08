#!/usr/bin/env bash
# scripts/privacy-audit.sh
#
# v1.0.6 Privacy Logic SSOT audit gate. Returns 0 only when the working
# tree satisfies four invariants:
#
#   1. Every outbound HTTP client (reqwest::Client / reqwest::get / .post)
#      lives in a file from the allow-list. New outbound sites MUST add
#      themselves to this list in the same PR that introduces them so the
#      review is forced.
#   2. No hardcoded API keys (OpenAI sk-…, AWS AKIA…, Google AIza…).
#   3. No telemetry call sites outside `attune-core/src/telemetry.rs`.
#   4. The privacy default block in `routes/settings.rs` keeps every one
#      of the five outbound keys set to `false`.
#
# Spec: docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md §6.
# See also docs/PRIVACY-AUDIT-CHECKLIST.md (monthly manual audit).

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

fail=0

# ──────────────────────────────────────────────────────────────────────
# Allow-list — files that legitimately call reqwest. Anything else MUST
# route through `attune_core::OutboundGate::enforce` first (per spec
# §4.2). New entries here require a privacy-maintainer review.
# ──────────────────────────────────────────────────────────────────────
allow_files='rust/crates/attune-core/src/(outbound_gate|chat|cloud_client|telemetry|web_search_browser|web_search|web_search_engines|llm|embed|asr|mcp_client)\.rs|rust/crates/attune-core/src/(sync/webdav|infer/embedding|ocr/.*)\.rs|rust/crates/attune-server/src/routes/(llm|status|version)\.rs|rust/crates/attune-server/src/test_support\.rs'

echo "==> 1. Outbound HTTP clients must route through OutboundGate"
hits=$(grep -rnE 'reqwest::(Client|get|post|Request)\b' rust/crates/attune-*/src 2>/dev/null \
  | grep -vE "$allow_files" \
  | grep -vE '^[^:]+:[0-9]+:\s*//' \
  || true)
if [ -n "$hits" ]; then
  echo "FAIL: outbound HTTP outside the allow-list (must wrap with OutboundGate::enforce):"
  echo "$hits"
  fail=1
else
  echo "  ok — all reqwest sites are allow-listed"
fi

echo "==> 2. Hardcoded API keys (OpenAI sk-… / AWS AKIA… / Google AIza…)"
# Exclusions:
#   - Comment lines (# / // / *)
#   - The PII detector source (`pii/`) — its very job is to match these
#     patterns; the strings there are test fixtures, not real secrets.
#   - Any test file (`tests/` directory or `_test.rs` suffix) — same
#     reason: fixtures that exercise the redactor.
#   - This audit script itself (the patterns above match its own regex).
hits=$(grep -rnE '(sk-[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{35})' \
        rust extension docs scripts 2>/dev/null \
        | grep -vE '^[^:]+:[0-9]+:\s*(\*|//|#)' \
        | grep -vE 'scripts/privacy-audit\.sh' \
        | grep -vE 'rust/crates/attune-core/src/pii/' \
        | grep -vE '/tests/|_test\.rs:|_tests\.rs:' \
        | grep -vE 'AKIAIOSFODNN7EXAMPLE' \
        || true)
if [ -n "$hits" ]; then
  echo "FAIL: hardcoded API-key candidates:"
  echo "$hits"
  fail=1
else
  echo "  ok — no hardcoded keys"
fi

echo "==> 3. Telemetry call sites outside telemetry.rs"
hits=$(grep -rnE 'telemetry::|Telemetry::new|TelemetryEvent' \
        rust/crates/attune-*/src 2>/dev/null \
        | grep -v 'rust/crates/attune-core/src/telemetry.rs' \
        | grep -vE '^[^:]+:[0-9]+:\s*(\*|//)' \
        || true)
if [ -n "$hits" ]; then
  echo "WARN: telemetry references outside telemetry.rs (review if intentional):"
  echo "$hits"
fi

echo "==> 4. Privacy default block in routes/settings.rs is all-false"
# Restrict to production code (skip the #[cfg(test)] tail). awk extracts
# every line of the file up to but not including the first `#[cfg(test)]`
# attribute, then matches the `"privacy": { … }` block within that prefix.
default_block=$(awk '/^#\[cfg\(test\)\]/{exit} {print}' \
                  rust/crates/attune-server/src/routes/settings.rs \
                | awk '/"privacy": \{/,/^[[:space:]]*\},/' \
                | grep -E '"(llm|cloud_saas|webdav|web_search|telemetry)"' \
                | grep -v ': false' \
                || true)
if [ -n "$default_block" ]; then
  echo "FAIL: privacy default block contains non-false values for an outbound key:"
  echo "$default_block"
  fail=1
else
  echo "  ok — 5 outbound keys all default false"
fi

# ──────────────────────────────────────────────────────────────────────
# 5. OutboundGate result must NOT be discarded (no-op enforcement).
#
# History (F-17 G1, 2026-06-08): four egress sites called
# `let _ = OutboundGate::enforce(... enabled:true, vault_unlocked:true ...)`
# as a *no-op audit marker* ("wired in Task 7", never done). Check #1 keys on
# the *presence* of `OutboundGate::enforce` and was fooled — the call existed
# but its Result (Disabled / VaultLocked / L0CloudBlocked / RedactorRequired)
# was thrown away, so disabling an egress or locking the vault did NOT stop it.
#
# This check FAILS on any `let _ = ... OutboundGate::enforce` (result discarded)
# and on the tell-tale `// wired in Task 7` no-op comment. The ONE legitimate
# discard is CloudClient::wipe_session (DSAR-adjacent always-allow) — explicitly
# allow-listed below.
# ──────────────────────────────────────────────────────────────────────
echo "==> 5. OutboundGate Result must be honored (no discarded enforce / no-op markers)"
gate_allow='rust/crates/attune-core/src/cloud_client\.rs' # wipe_session: intentional always-allow
# 5a. `let _ = ... OutboundGate::enforce` — discarded fail-closed Result.
noop_hits=$(grep -rnE 'let[[:space:]]+_[[:space:]]*=.*OutboundGate::enforce' \
              rust/crates/attune-*/src 2>/dev/null \
            | grep -vE "$gate_allow" \
            | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' \
            || true)
# 5b. The historical no-op marker comment (a stub that never wired real state).
marker_hits=$(grep -rnE 'wired in Task 7|non-rejecting call site marker|hardcoded.*OutboundPolicy' \
                rust/crates/attune-*/src 2>/dev/null || true)
if [ -n "$noop_hits" ] || [ -n "$marker_hits" ]; then
  echo "FAIL: OutboundGate enforcement is a no-op at one or more egress points:"
  [ -n "$noop_hits" ] && { echo "  -- discarded Result (let _ = enforce):"; echo "$noop_hits"; }
  [ -n "$marker_hits" ] && { echo "  -- no-op marker comments:"; echo "$marker_hits"; }
  echo "  Fix: propagate the Err (use ? / match) and pass REAL enabled +"
  echo "       vault_unlocked + redactor (see web_search_browser.rs / scanner_webdav.rs)."
  fail=1
else
  echo "  ok — every OutboundGate::enforce honors its Result (wipe_session allow-listed)"
fi

if [ "$fail" -eq 0 ]; then
  echo "privacy-audit: PASS"
  exit 0
else
  echo "privacy-audit: FAIL"
  exit 1
fi
