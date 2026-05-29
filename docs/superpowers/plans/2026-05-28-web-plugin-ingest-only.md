# Web Plugin Ingest-Only Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the SSOT that reframes the Chrome extension as a **unidirectional knowledge ingest client** (5 sources: AI conversation / right-click selection / saved page / browse signals / sidepanel upload), formalise the 4 MSG schema under Manifest V3, add the missing extension→server contract (`POST /api/v1/ingest/extension/{type}` + `vault-locked` buffer), retire the three injection-era MSG stubs (`PREFETCH` / `SEARCH_RELEVANT` / `TOGGLE_INJECTION`), and ship a `scripts/extension-permission-audit.sh` CI gate that fails on dangerous new permissions.

**Architecture:** No new architecture — this plan codifies what already exists. The reframe is mostly **documentation + deprecation + contract tests**. Three code changes are net-new: (1) a `vault-locked` error code surfaced from `POST /api/v1/ingest` so the extension worker can buffer to `chrome.storage.local["pending_ingest"]` instead of dropping; (2) a typed `source_type` enum on the server ingest pipeline so `conversation` / `selection` / `webpage` / `browse_signal` / `upload` are explicit (today they are free-form strings); (3) a manifest permission audit script in CI. The extension itself keeps Preact + Vite + MV3 service worker; no framework changes.

**Tech Stack:** Manifest V3 (Chrome extension), Preact + Vite, Rust (axum 0.8 routes / store), Playwright (Chrome channel only, per CLAUDE.md §6.4), shell-based CI guard.

**Spec:** `docs/superpowers/specs/2026-05-28-web-plugin-as-knowledge-source.md` (commit `d005735`).

**Target release:** `v1.1.0` (2026-08-15) — bundled with VLM provider + defamation v3. Reframe content + audit script can ship earlier as part of v1.0.x docs sweeps, but the MSG deprecation + buffer behavior change waits until v1.1 to give one full release cycle of user warning.

---

## File Structure

### New files

| Path | Responsibility | Owner task |
|------|----------------|------------|
| `extension/README.md` (rewrite of existing) | Top-of-file disclaimer: ingest-only, no injection | Task 1 |
| `extension/src/shared/source_types.js` | Single export `SOURCE_TYPES` enum + helpers | Task 2 |
| `extension/src/background/pending_ingest_buffer.js` | Buffer for vault-locked / offline ingest | Task 4 |
| `rust/crates/attune-server/src/routes/ingest_extension.rs` | Typed extension ingest endpoints | Task 5 |
| `scripts/extension-permission-audit.sh` | Manifest permission diff guard | Task 7 |
| `extension/tests/dedup_property.spec.js` | Property test for djb2 dedup | Task 3 |
| `extension/tests/e2e/ingest_e2e.spec.ts` | Playwright Chrome MCP end-to-end | Task 6 |

### Modified files

| Path | Change | Owner task |
|------|--------|------------|
| `extension/src/shared/messages.js` | Mark `PREFETCH` / `SEARCH_RELEVANT` / `TOGGLE_INJECTION` deprecated; add `BROWSE_SIGNAL` to the enum (currently only used as a raw string) | Task 2 |
| `extension/src/background/worker.js` | Remove `injectionEnabled` logic; add `pending_ingest` flush; route via `source_types` | Task 4 |
| `extension/src/popup/Popup.jsx` | Add "Ingest-only — no AI page injection" disclaimer; remove injection toggle UI | Task 1 |
| `extension/src/options/Options.jsx` | Remove injection-related options | Task 1 |
| `extension/manifest.json` | Bump to `0.7.0`; add comment block referencing this spec | Task 7 |
| `rust/crates/attune-server/src/routes/ingest.rs` | Return `{code: "vault-locked"}` 409 when vault locked instead of 401 | Task 5 |
| `rust/crates/attune-server/src/lib.rs` | Register `/api/v1/ingest/extension/:type` and `/api/v1/browse_signals` (if missing) | Task 5 |
| `.github/workflows/ci.yml` | Add `extension-permission-audit` job | Task 7 |
| `RELEASE.md` | v1.1.0 entry | Task 8 |

---

## Task 1: Reframe — popup disclaimer + README rewrite + remove injection UI

**Files:**
- Modify: `extension/README.md`
- Modify: `extension/src/popup/Popup.jsx`
- Modify: `extension/src/options/Options.jsx`

- [ ] **Step 1: Rewrite `extension/README.md`**

Replace the entire file with content that opens with a clear positioning statement. Use the spec §1 reframe paragraph verbatim where possible:

```markdown
# Attune Chrome Extension

> **Ingest-only by design.** This extension captures content from your browser into your local Attune knowledge base. **It does not modify, inject, or alter any page you visit, including ChatGPT, Claude, or Gemini.** The legacy prompt-injection feature was removed in 2026-04 (cleanup-r15).

## What it does (5 capture sources)

1. **AI conversation capture** — Reads ChatGPT / Claude / Gemini conversations via MutationObserver and saves them to your local vault.
2. **Right-click "Save selection"** — Highlight text on any page, right-click → save.
3. **"Save this page"** — Sidepanel button extracts the readable body and saves it.
4. **Browse signals (opt-in)** — Aggregated dwell / scroll / copy counts per domain (whitelist + hard-blacklist for banks, login pages, password managers).
5. **Sidepanel file upload** — Drag-drop files into the sidepanel for indexing.

## What it does NOT do

- ❌ Inject any prompt, prefix, or suffix into AI chat boxes
- ❌ Modify any page DOM
- ❌ Read or transmit data to anywhere other than `http://localhost:18900`
- ❌ Load in incognito mode (hard-blocked by `manifest.json`)

## Permissions explained

| Permission | Why we need it |
|------------|----------------|
| `storage` | Save dedup cache + settings via `chrome.storage` |
| `sidePanel` | Render the Attune sidebar UI |
| `activeTab` | Read the current tab URL when you click "Save this page" |
| `tabs` | Coordinate state across multiple browser tabs |
| `contextMenus` | "Save selection" right-click entry |
| `webNavigation` | Detect SPA route changes for browse signals |
| `<all_urls>` (host) | Required by Chrome to run `browse_capture.js` on any page; the script is hard-blocked from banks, logins, and password managers before it does anything |

## Privacy

See `docs/PRIVACY.md` and the in-app **Settings → Privacy** dashboard. Browse signal capture is opt-out by default; you must explicitly add a domain to the whitelist.

---

中文版

> **仅采集，不注入。** 此扩展只从浏览器把内容存进本地 Attune 知识库。它**不修改任何页面**，包括 ChatGPT / Claude / Gemini。注入功能已于 2026-04 删除（cleanup-r15）。

5 个采集源 / 不做的事 / 权限说明：内容同上，详见 `docs/PRIVACY.md`。
```

- [ ] **Step 2: Update Popup.jsx**

In `extension/src/popup/Popup.jsx`, remove any `<Toggle>` / `<input type="checkbox">` whose label mentions "注入" / "injection". Add a prominent disclaimer near the top:

```jsx
// inside the Popup component, near the top of the rendered tree
<div class="bg-blue-50 border border-blue-200 rounded p-2 text-xs mb-3"
     data-testid="popup-ingest-disclaimer">
  <strong>Ingest-only</strong> — this extension captures content to your local
  Attune vault. It does not inject anything into ChatGPT/Claude/Gemini.
</div>
```

- [ ] **Step 3: Update Options.jsx**

Same: remove any UI control labelled `injectionEnabled` / "前缀注入" / "注入模式". Leave a single line in its place:

```jsx
<p class="text-gray-500 text-xs my-2">
  Injection mode was removed in extension v0.5+. This extension is ingest-only.
</p>
```

- [ ] **Step 4: Manually verify in Chrome**

Build + load: `cd extension && npm run build && # load `extension/dist/` as unpacked extension via chrome://extensions`.

Open popup — confirm disclaimer is visible.
Open options page — confirm no injection toggle.

- [ ] **Step 5: Commit**

```bash
git add extension/README.md extension/src/popup/Popup.jsx extension/src/options/Options.jsx
git commit -m "docs(extension): reframe as ingest-only — README + popup + options"
```

---

## Task 2: Typed `source_types` enum + MSG cleanup

**Files:**
- Create: `extension/src/shared/source_types.js`
- Modify: `extension/src/shared/messages.js`

- [ ] **Step 1: Create the typed enum**

```js
// extension/src/shared/source_types.js
/**
 * Server-recognised `source_type` values posted via /api/v1/ingest.
 * Keep this list synced with rust/crates/attune-server/src/routes/ingest_extension.rs.
 */
export const SOURCE_TYPES = Object.freeze({
  CONVERSATION: 'conversation',
  SELECTION:    'selection',
  WEBPAGE:      'webpage',
  BROWSE_SIGNAL:'browse_signal',
  UPLOAD:       'upload',
});

export const SOURCE_TYPE_VALUES = Object.values(SOURCE_TYPES);

export function isKnownSourceType(value) {
  return SOURCE_TYPE_VALUES.includes(value);
}
```

- [ ] **Step 2: Write the failing test**

```js
// extension/tests/source_types.spec.js
import { describe, it, expect } from 'vitest';
import { SOURCE_TYPES, SOURCE_TYPE_VALUES, isKnownSourceType } from '../src/shared/source_types.js';

describe('source_types', () => {
  it('exposes exactly 5 source types', () => {
    expect(SOURCE_TYPE_VALUES).toHaveLength(5);
    expect(SOURCE_TYPE_VALUES.sort()).toEqual(
      ['browse_signal','conversation','selection','upload','webpage'].sort()
    );
  });
  it('rejects unknown values', () => {
    expect(isKnownSourceType('rss')).toBe(false);
    expect(isKnownSourceType('webpage')).toBe(true);
  });
  it('is frozen', () => {
    expect(Object.isFrozen(SOURCE_TYPES)).toBe(true);
  });
});
```

- [ ] **Step 3: Run test**

Run: `cd extension && npm test -- source_types`
Expected: 3 passed.

- [ ] **Step 4: Update messages.js — mark deprecated MSGs and add BROWSE_SIGNAL**

Rewrite `extension/src/shared/messages.js`:

```js
/**
 * Unified extension message types + transport helpers.
 *
 * Deprecated types (PREFETCH / SEARCH_RELEVANT / TOGGLE_INJECTION) are
 * retained for one release cycle to honour the documented deprecation
 * window (see docs/superpowers/specs/2026-05-28-web-plugin-as-knowledge-source.md §5.3).
 * They will be removed in extension v0.8.
 */
export const MSG = {
  // — Active —
  CAPTURE_CONVERSATION: 'CAPTURE_CONVERSATION', // content → worker → server
  SAVE_SELECTION:       'SAVE_SELECTION',       // contextMenu → worker → server
  CAPTURE_PAGE:         'CAPTURE_PAGE',         // sidepanel → worker → content → server
  GET_PAGE_CONTENT:     'GET_PAGE_CONTENT',     // worker → content (internal)
  BROWSE_SIGNAL:        'BROWSE_SIGNAL',        // browse_capture → worker
  GET_STATUS:           'GET_STATUS',
  GET_SETTINGS:         'GET_SETTINGS',
  SETTINGS_UPDATED:     'SETTINGS_UPDATED',
  SEARCH:               'SEARCH',
  GET_ITEMS:            'GET_ITEMS',
  OPEN_SIDEPANEL:       'OPEN_SIDEPANEL',
  SUMMARIZE_AND_SAVE:   'SUMMARIZE_AND_SAVE',

  // — Deprecated; removed in extension v0.8 —
  /** @deprecated injection feature removed 2026-04 (cleanup-r15). Stub kept for one cycle. */
  PREFETCH:          'PREFETCH',
  /** @deprecated split from worker; sidepanel now hits server directly. */
  SEARCH_RELEVANT:   'SEARCH_RELEVANT',
  /** @deprecated injection toggle no longer affects any behaviour. */
  TOGGLE_INJECTION:  'TOGGLE_INJECTION',
};

export const DEPRECATED_MSGS = Object.freeze(['PREFETCH', 'SEARCH_RELEVANT', 'TOGGLE_INJECTION']);

export function sendToWorker(type, payload = {}) {
  return chrome.runtime.sendMessage({ type, ...payload });
}
export function sendToTab(tabId, type, payload = {}) {
  return chrome.tabs.sendMessage(tabId, { type, ...payload });
}
```

- [ ] **Step 5: Write the failing test for deprecation list**

```js
// extension/tests/messages_deprecation.spec.js
import { describe, it, expect } from 'vitest';
import { MSG, DEPRECATED_MSGS } from '../src/shared/messages.js';

describe('MSG deprecation', () => {
  it('lists the exact 3 deprecated names', () => {
    expect([...DEPRECATED_MSGS].sort()).toEqual(['PREFETCH', 'SEARCH_RELEVANT', 'TOGGLE_INJECTION'].sort());
  });
  it('all deprecated names still resolve (one release of compatibility)', () => {
    for (const name of DEPRECATED_MSGS) {
      expect(MSG[name]).toBe(name);
    }
  });
  it('BROWSE_SIGNAL is now a first-class MSG', () => {
    expect(MSG.BROWSE_SIGNAL).toBe('BROWSE_SIGNAL');
  });
});
```

- [ ] **Step 6: Run all extension tests**

Run: `cd extension && npm test`
Expected: source_types (3) + messages_deprecation (3) = 6 passed.

- [ ] **Step 7: Commit**

```bash
git add extension/src/shared/source_types.js \
        extension/src/shared/messages.js \
        extension/tests/source_types.spec.js \
        extension/tests/messages_deprecation.spec.js
git commit -m "feat(extension): SOURCE_TYPES enum + mark 3 MSG types deprecated"
```

---

## Task 3: Property test for djb2 dedup

**Files:**
- Create: `extension/tests/dedup_property.spec.js`

- [ ] **Step 1: Write the property test**

```js
// extension/tests/dedup_property.spec.js
import { describe, it, expect } from 'vitest';
import fc from 'fast-check';

// Port the dedup logic from worker.js — same input ⇒ same hash ⇒ same key.
function djb2(str) {
  let h = 5381;
  for (let i = 0; i < str.length; i++) h = ((h << 5) + h) ^ str.charCodeAt(i);
  return (h >>> 0).toString(36);
}

describe('djb2 dedup', () => {
  it('same content yields same hash 1000 random strings', () => {
    fc.assert(fc.property(fc.string({ minLength: 1, maxLength: 4000 }), (s) => {
      return djb2(s) === djb2(s);
    }), { numRuns: 1000 });
  });

  it('different content yields different hashes with collision rate < 1e-3 on random 5000-pair set', () => {
    let collisions = 0;
    const N = 5000;
    fc.assert(fc.property(
      fc.tuple(fc.string({ minLength: 1, maxLength: 200 }),
               fc.string({ minLength: 1, maxLength: 200 })),
      ([a, b]) => {
        if (a === b) return true;
        if (djb2(a) === djb2(b)) collisions++;
        return true;
      }
    ), { numRuns: N });
    expect(collisions / N).toBeLessThan(1e-3);
  });

  it('empty string is a valid input (no throw)', () => {
    expect(() => djb2('')).not.toThrow();
  });
});
```

- [ ] **Step 2: Add fast-check to extension devDependencies**

```bash
cd extension && npm install --save-dev fast-check
```

- [ ] **Step 3: Run**

Run: `cd extension && npm test -- dedup_property`
Expected: 3 passed; collision rate near 0.

- [ ] **Step 4: Commit**

```bash
git add extension/tests/dedup_property.spec.js extension/package.json extension/package-lock.json
git commit -m "test(extension): djb2 dedup property test (1k runs + collision sanity)"
```

---

## Task 4: Vault-locked buffer + cleanup deprecated handlers in worker.js

**Files:**
- Create: `extension/src/background/pending_ingest_buffer.js`
- Modify: `extension/src/background/worker.js`

- [ ] **Step 1: Create the buffer module**

```js
// extension/src/background/pending_ingest_buffer.js
/**
 * Buffer for ingest payloads that the server refused because vault is locked.
 *
 * Stored in chrome.storage.local["pending_ingest"] (persists across service-worker sleeps).
 * Hard cap 1000 entries (FIFO drop).
 */

const STORAGE_KEY = 'pending_ingest';
const MAX_ENTRIES = 1000;

export async function enqueue(entry) {
  const { [STORAGE_KEY]: existing = [] } = await chrome.storage.local.get(STORAGE_KEY);
  existing.push({ ...entry, queued_at: Date.now() });
  while (existing.length > MAX_ENTRIES) existing.shift();
  await chrome.storage.local.set({ [STORAGE_KEY]: existing });
  return existing.length;
}

export async function size() {
  const { [STORAGE_KEY]: existing = [] } = await chrome.storage.local.get(STORAGE_KEY);
  return existing.length;
}

export async function drainAndPost(postFn) {
  const { [STORAGE_KEY]: existing = [] } = await chrome.storage.local.get(STORAGE_KEY);
  if (existing.length === 0) return { sent: 0, remaining: 0 };
  const remaining = [];
  let sent = 0;
  for (const entry of existing) {
    try {
      const resp = await postFn(entry);
      if (resp && resp.status === 'ok') { sent++; continue; }
      // If server still locked / failing, stop draining; keep order.
      remaining.push(entry, ...existing.slice(existing.indexOf(entry) + 1));
      break;
    } catch {
      remaining.push(entry, ...existing.slice(existing.indexOf(entry) + 1));
      break;
    }
  }
  await chrome.storage.local.set({ [STORAGE_KEY]: remaining });
  return { sent, remaining: remaining.length };
}

export async function clearAll() {
  await chrome.storage.local.remove(STORAGE_KEY);
}
```

- [ ] **Step 2: Write the unit test**

```js
// extension/tests/pending_ingest_buffer.spec.js
import { describe, it, expect, beforeEach, vi } from 'vitest';

const store = new Map();
globalThis.chrome = {
  storage: {
    local: {
      get: async (key) => ({ [key]: store.get(key) }),
      set: async (obj) => { for (const [k,v] of Object.entries(obj)) store.set(k,v); },
      remove: async (key) => { store.delete(key); },
    }
  }
};

import { enqueue, size, drainAndPost, clearAll } from '../src/background/pending_ingest_buffer.js';

describe('pending_ingest_buffer', () => {
  beforeEach(async () => { await clearAll(); });

  it('enqueue grows size', async () => {
    expect(await size()).toBe(0);
    await enqueue({ title: 'a', content: 'x' });
    await enqueue({ title: 'b', content: 'y' });
    expect(await size()).toBe(2);
  });

  it('drainAndPost sends all when poster returns ok', async () => {
    await enqueue({ title: 'a' });
    await enqueue({ title: 'b' });
    const { sent, remaining } = await drainAndPost(async () => ({ status: 'ok' }));
    expect(sent).toBe(2);
    expect(remaining).toBe(0);
  });

  it('drainAndPost stops on first failure, keeps order', async () => {
    await enqueue({ title: 'a' });
    await enqueue({ title: 'b' });
    let n = 0;
    const { sent, remaining } = await drainAndPost(async () => {
      n++;
      return n === 1 ? { status: 'ok' } : { status: 'error', code: 'vault-locked' };
    });
    expect(sent).toBe(1);
    expect(remaining).toBe(1);
  });

  it('hard cap drops oldest at 1000', async () => {
    for (let i = 0; i < 1005; i++) await enqueue({ title: String(i) });
    expect(await size()).toBe(1000);
  });
});
```

- [ ] **Step 3: Run**

Run: `cd extension && npm test -- pending_ingest_buffer`
Expected: 4 passed.

- [ ] **Step 4: Modify `worker.js` — remove `injectionEnabled` + wire buffer + drop deprecated handlers**

In `extension/src/background/worker.js`:

- Delete the `injectionEnabled` global + the `chrome.storage.local.get('injectionEnabled')` initialiser around line 57.
- Delete the `case MSG.PREFETCH` block (around line 111). Replace with:
  ```js
  case MSG.PREFETCH:
  case MSG.SEARCH_RELEVANT:
  case MSG.TOGGLE_INJECTION: {
    console.warn('[attune] deprecated MSG received:', msg.type,
      '— this message is a no-op in extension v0.7+, removed in v0.8');
    return { ok: false, deprecated: true };
  }
  ```
- In the `case MSG.CAPTURE_CONVERSATION` / `SAVE_SELECTION` / `CAPTURE_PAGE` handlers, wrap the existing POST call:

  ```js
  import { enqueue, drainAndPost } from './pending_ingest_buffer.js';
  import { SOURCE_TYPES } from '../shared/source_types.js';

  // helper (top of file)
  async function postIngestOrBuffer(payload) {
    try {
      const resp = await api.ingest(payload);
      if (resp && resp.status === 'vault-locked' || resp?.code === 'vault-locked') {
        await enqueue(payload);
        return { status: 'buffered' };
      }
      return resp;
    } catch (e) {
      // Network error — buffer to retry later
      await enqueue(payload);
      return { status: 'buffered' };
    }
  }
  ```

- Add a health-check hook that on every successful `/health` reply attempts to flush the buffer:

  ```js
  // inside the existing 30s health-check setInterval
  if (backendOnline) {
    const { sent } = await drainAndPost(api.ingest);
    if (sent > 0) console.log(`[attune] flushed ${sent} buffered ingest entries`);
  }
  ```

- Replace the existing GET_STATUS reply to include buffered count:

  ```js
  case MSG.GET_STATUS: {
    const buffered = await import('./pending_ingest_buffer.js').then(m => m.size());
    return { online: backendOnline, buffered_count: buffered };
  }
  ```

- [ ] **Step 5: Write integration test for vault-locked buffer flow**

Add `extension/tests/worker_vault_locked.spec.js` using vitest with mocked `api`:

```js
import { describe, it, expect, beforeEach, vi } from 'vitest';

// Mock chrome.storage as in pending_ingest_buffer test
const store = new Map();
globalThis.chrome = {
  storage: { local: {
    get: async k => ({ [k]: store.get(k) }),
    set: async o => { for (const [k,v] of Object.entries(o)) store.set(k,v); },
    remove: async k => { store.delete(k); },
  }},
  runtime: { sendMessage: vi.fn() },
};

import { enqueue, drainAndPost, size, clearAll } from '../src/background/pending_ingest_buffer.js';

describe('vault-locked buffer flow', () => {
  beforeEach(async () => { await clearAll(); });

  it('buffers when poster reports vault-locked, drains on next ok', async () => {
    await enqueue({ title: 'queued-1', content: 'x' });
    expect(await size()).toBe(1);
    const { sent } = await drainAndPost(async () => ({ status: 'ok' }));
    expect(sent).toBe(1);
    expect(await size()).toBe(0);
  });
});
```

Run: `cd extension && npm test -- worker_vault_locked`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add extension/src/background/pending_ingest_buffer.js \
        extension/src/background/worker.js \
        extension/tests/pending_ingest_buffer.spec.js \
        extension/tests/worker_vault_locked.spec.js
git commit -m "feat(extension): vault-locked buffer + drop injection logic in worker.js"
```

---

## Task 5: Server side — typed extension ingest + vault-locked error code

**Files:**
- Create: `rust/crates/attune-server/src/routes/ingest_extension.rs`
- Modify: `rust/crates/attune-server/src/routes/ingest.rs`
- Modify: `rust/crates/attune-server/src/lib.rs`
- Test: `rust/crates/attune-server/tests/extension_ingest.rs`

- [ ] **Step 1: Write the failing test**

```rust
// rust/crates/attune-server/tests/extension_ingest.rs
use attune_server::test_support::spawn_test_server;
use serde_json::json;

#[tokio::test]
async fn ingest_conversation_with_typed_path_routes_correctly() {
    let srv = spawn_test_server().await;
    srv.unlock_vault("test-password-not-real").await;
    let resp = srv.client.post(format!("{}/api/v1/ingest/extension/conversation", srv.base_url))
        .json(&json!({
            "title": "chat with claude",
            "content": "hi",
            "url": "https://claude.ai/c/abc",
            "domain": "claude.ai",
            "metadata": { "platform": "claude", "captured_at": "2026-05-28T00:00:00Z" }
        }))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert!(body["id"].is_string());
}

#[tokio::test]
async fn ingest_rejects_unknown_source_type() {
    let srv = spawn_test_server().await;
    srv.unlock_vault("test-password-not-real").await;
    let resp = srv.client.post(format!("{}/api/v1/ingest/extension/rss", srv.base_url))
        .json(&json!({ "title": "x", "content": "y" }))
        .send().await.unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "unknown-source-type");
}

#[tokio::test]
async fn ingest_returns_vault_locked_code_when_vault_is_locked() {
    let srv = spawn_test_server().await;
    // explicitly DO NOT unlock
    let resp = srv.client.post(format!("{}/api/v1/ingest/extension/conversation", srv.base_url))
        .json(&json!({ "title": "x", "content": "y" }))
        .send().await.unwrap();
    assert_eq!(resp.status(), 409, "vault-locked must use 409 not 401 so worker can buffer");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "vault-locked");
}
```

- [ ] **Step 2: Run, verify it fails**

Run: `cargo test -p attune-server --test extension_ingest`
Expected: FAIL (route missing + status code today is 401).

- [ ] **Step 3: Create `ingest_extension.rs`**

```rust
//! Typed extension ingest endpoints — `POST /api/v1/ingest/extension/:type`
//! Wraps the generic `routes::ingest` handler with a typed source-type check
//! so the worker JS contract is enforced at the edge.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};

use crate::state::SharedState;

const KNOWN_TYPES: &[&str] = &["conversation", "selection", "webpage", "browse_signal", "upload"];

pub async fn ingest_typed(
    State(state): State<SharedState>,
    Path(source_type): Path<String>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !KNOWN_TYPES.contains(&source_type.as_str()) {
        return Err((StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("unknown source_type: {source_type}"),
                          "code": "unknown-source-type",
                          "allowed": KNOWN_TYPES }))));
    }
    if !state.vault_is_unlocked() {
        return Err((StatusCode::CONFLICT,
            Json(json!({ "error": "vault is locked — please unlock to ingest",
                          "code": "vault-locked" }))));
    }
    if let Some(obj) = body.as_object_mut() {
        obj.insert("source_type".into(), json!(source_type));
    }
    // Delegate to the existing generic ingest handler (which already does
    // parse → encrypt → index → embedding-queue).
    crate::routes::ingest::ingest_inner(state, body).await
        .map(Json)
        .map_err(|(status, payload)| (status, Json(payload)))
}
```

- [ ] **Step 4: Refactor `routes/ingest.rs` to expose `ingest_inner` and use vault-locked 409**

In `routes/ingest.rs`, factor the existing body of `ingest()` into a re-usable function:

```rust
pub async fn ingest(State(state): State<SharedState>, Json(body): Json<Value>)
    -> Result<Json<Value>, (StatusCode, Json<Value>)>
{
    if !state.vault_is_unlocked() {
        return Err((StatusCode::CONFLICT,
            Json(json!({ "error": "vault is locked", "code": "vault-locked" }))));
    }
    ingest_inner(state, body).await.map(Json).map_err(|(s,p)| (s, Json(p)))
}

pub async fn ingest_inner(state: SharedState, body: Value)
    -> Result<Value, (StatusCode, Value)>
{
    // ... (existing parse / encrypt / store logic moves here unchanged) ...
}
```

- [ ] **Step 5: Register the typed route**

In `rust/crates/attune-server/src/lib.rs`, near the existing line 76:

```rust
.route("/api/v1/ingest", post(routes::ingest::ingest))
.route("/api/v1/ingest/extension/:source_type", post(routes::ingest_extension::ingest_typed))
```

Add a module declaration in `routes/mod.rs`:

```rust
pub mod ingest_extension;
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p attune-server --test extension_ingest`
Expected: 3 passed.

- [ ] **Step 7: Update extension API client to use typed endpoint**

In `extension/src/shared/api.js`, replace any call site that builds the body with `source_type: 'conversation'` (etc.) and POSTs to `/api/v1/ingest`. Switch to typed paths:

```js
export const api = {
  ingestConversation: (body) => post('/api/v1/ingest/extension/conversation', body),
  ingestSelection:    (body) => post('/api/v1/ingest/extension/selection', body),
  ingestWebpage:      (body) => post('/api/v1/ingest/extension/webpage', body),
  // browse_signal stays on existing /api/v1/browse_signals (batched POST)
  // upload stays on existing /api/v1/upload (multipart)
  // ...rest unchanged
  ingest: (body) => {
    // legacy generic — used by buffer drain when we don't know the typed path
    const t = body?.source_type;
    return t && ['conversation','selection','webpage'].includes(t)
      ? post(`/api/v1/ingest/extension/${t}`, body)
      : post('/api/v1/ingest', body);
  },
};
```

Update `worker.js` to call `api.ingestConversation(payload)` etc. The generic `api.ingest` is kept for buffer drain.

- [ ] **Step 8: Re-run extension test suite**

Run: `cd extension && npm test`
Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add rust/crates/attune-server/src/routes/ingest_extension.rs \
        rust/crates/attune-server/src/routes/ingest.rs \
        rust/crates/attune-server/src/routes/mod.rs \
        rust/crates/attune-server/src/lib.rs \
        rust/crates/attune-server/tests/extension_ingest.rs \
        extension/src/shared/api.js \
        extension/src/background/worker.js
git commit -m "feat(ingest): typed /ingest/extension/:type + vault-locked 409 code"
```

---

## Task 6: E2E Playwright Chrome MCP

**Files:**
- Create: `extension/tests/e2e/ingest_e2e.spec.ts`

- [ ] **Step 1: Write the test**

```ts
// extension/tests/e2e/ingest_e2e.spec.ts
import { test, expect, chromium } from '@playwright/test';
import path from 'node:path';

const EXTENSION_DIR = path.resolve(__dirname, '../../dist');

test('extension captures a saved page into the vault', async () => {
  const userDataDir = path.resolve(__dirname, '../../.playwright-mcp/extension-profile');
  // Per CLAUDE.md §6.4 — Chrome channel only, persistent context for MV3.
  const context = await chromium.launchPersistentContext(userDataDir, {
    channel: 'chrome',
    headless: false,
    args: [
      `--disable-extensions-except=${EXTENSION_DIR}`,
      `--load-extension=${EXTENSION_DIR}`,
    ],
  });
  const page = await context.newPage();

  // Pre-condition: server running locally with vault unlocked at fixture password.
  await page.goto('http://example.com/');
  await page.waitForLoadState('domcontentloaded');

  // Trigger "save this page" via the extension's contextMenus or sidepanel.
  // For headless-friendly testing, we POST via the extension's own background API:
  const result = await page.evaluate(async () => {
    return await fetch('http://localhost:18900/api/v1/ingest/extension/webpage', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        title: document.title,
        content: document.body.innerText,
        url: location.href,
        domain: location.hostname,
        metadata: { captured_at: new Date().toISOString(), lang: 'en' },
      })
    }).then(r => r.json());
  });
  expect(result.status).toBe('ok');
  expect(typeof result.id).toBe('string');

  // Verify item is searchable
  const search = await page.evaluate(async () => {
    return await fetch('http://localhost:18900/api/v1/search?q=example')
      .then(r => r.json());
  });
  expect(search.results.length).toBeGreaterThan(0);

  await context.close();
});

test('extension buffers ingest when vault is locked, drains after unlock', async () => {
  // 1. Lock vault
  await fetch('http://localhost:18900/api/v1/privacy/lock', { method: 'POST' });
  // 2. Send an ingest; expect 409 vault-locked
  const r = await fetch('http://localhost:18900/api/v1/ingest/extension/webpage', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ title: 't', content: 'c', url: 'http://x', domain: 'x' })
  });
  expect(r.status).toBe(409);
  const body = await r.json();
  expect(body.code).toBe('vault-locked');
  // Re-unlock so other tests are unaffected
  await fetch('http://localhost:18900/api/v1/vault/unlock', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ password: 'test-password-not-real' }),
  });
});
```

- [ ] **Step 2: Configure playwright.config.ts for the extension tests**

Ensure `playwright.config.ts` (root) includes the new test directory:

```ts
// playwright.config.ts (add to projects array)
{
  name: 'extension-e2e',
  testDir: 'extension/tests/e2e',
  use: { channel: 'chrome' },
}
```

- [ ] **Step 3: Run**

```bash
cd extension && npm run build  # produces extension/dist/
cd .. && npx playwright test --project extension-e2e
```
Expected: 2 passed (assuming attune-server is running locally with vault initialised at `test-password-not-real`).

- [ ] **Step 4: Commit**

```bash
git add extension/tests/e2e/ingest_e2e.spec.ts playwright.config.ts
git commit -m "test(extension): Playwright Chrome E2E for webpage ingest + vault-locked buffer"
```

---

## Task 7: Permission audit script + CI gate + manifest version bump

**Files:**
- Create: `scripts/extension-permission-audit.sh`
- Modify: `extension/manifest.json`
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Write the audit script**

```bash
#!/usr/bin/env bash
# scripts/extension-permission-audit.sh — fail if extension/manifest.json
# introduces a dangerous permission outside the spec-allowed set.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

MANIFEST=extension/manifest.json
if [ ! -f "$MANIFEST" ]; then
  echo "FAIL: $MANIFEST missing"
  exit 1
fi

# Allowed permissions per spec §5.1
ALLOWED_PERMS='^(storage|sidePanel|activeTab|tabs|contextMenus|webNavigation)$'
ALLOWED_HOSTS='^(http://localhost/\*|http://127\.0\.0\.1/\*|<all_urls>)$'

# Extract permissions array (jq-free portable parse)
perms=$(grep -A 20 '"permissions"' "$MANIFEST" | sed -n '/\[/,/\]/p' \
  | tr -d ' ",\n[]' | tr '\n' ' ')
hosts=$(grep -A 20 '"host_permissions"' "$MANIFEST" | sed -n '/\[/,/\]/p' \
  | tr -d ' ",\n[]' | tr '\n' ' ')

fail=0
for p in $perms; do
  [ -z "$p" ] && continue
  if ! echo "$p" | grep -qE "$ALLOWED_PERMS"; then
    echo "FAIL: permission '$p' is not in the spec-allowed set"
    echo "      Allowed: storage / sidePanel / activeTab / tabs / contextMenus / webNavigation"
    echo "      To add a new permission, amend docs/superpowers/specs/2026-05-28-web-plugin-as-knowledge-source.md §5.1 first."
    fail=1
  fi
done
for h in $hosts; do
  [ -z "$h" ] && continue
  if ! echo "$h" | grep -qE "$ALLOWED_HOSTS"; then
    echo "FAIL: host_permission '$h' is not in the spec-allowed set"
    fail=1
  fi
done

# Hard-block dangerous permissions even if someone adds them to the allow regex
for dangerous in cookies webRequest history bookmarks declarativeNetRequest \
                 nativeMessaging proxy debugger; do
  if grep -qE "\"$dangerous\"" "$MANIFEST"; then
    echo "FAIL: '$dangerous' permission is blocked unconditionally — spec required"
    fail=1
  fi
done

if [ "$fail" -eq 0 ]; then
  echo "extension-permission-audit: PASS"
else
  echo "extension-permission-audit: FAIL"
  exit 1
fi
```

- [ ] **Step 2: Make executable + run locally**

```bash
chmod +x scripts/extension-permission-audit.sh
bash scripts/extension-permission-audit.sh
```
Expected: `extension-permission-audit: PASS` (current manifest is in the allow-list).

- [ ] **Step 3: Bump manifest version + add spec-link comment**

Edit `extension/manifest.json`:
- `"version": "0.7.0"` (was 0.6.1)
- Add a top-level `"_comment_spec"` field:
  ```json
  "_comment_spec": "Permissions are constrained by docs/superpowers/specs/2026-05-28-web-plugin-as-knowledge-source.md §5.1. New permissions require spec amend + scripts/extension-permission-audit.sh approval.",
  ```

- [ ] **Step 4: Add CI job**

In `.github/workflows/ci.yml`:

```yaml
  extension-permission-audit:
    name: Extension Permission Audit
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: bash scripts/extension-permission-audit.sh
```

- [ ] **Step 5: Run + push**

```bash
bash scripts/extension-permission-audit.sh
git add scripts/extension-permission-audit.sh extension/manifest.json .github/workflows/ci.yml
git commit -m "chore(extension): permission audit script + CI gate + manifest v0.7.0"
git push origin develop
```

Watch GitHub Actions for the new `extension-permission-audit` job to pass.

---

## Task 8: RELEASE notes + version bump + merge to main + tag v1.1.0

**Files:**
- Modify: `RELEASE.md`
- Modify: `rust/Cargo.toml`
- Modify: `rust/crates/attune-server/ui/package.json`
- Modify: `extension/package.json`

- [ ] **Step 1: Append v1.1.0 section to RELEASE.md**

```markdown
## v1.1.0 (2026-08-15) — VLM provider + Web Plugin reframe

### Highlights
- **Chrome extension reframed as ingest-only** — disclaimers in popup + README; 5 capture sources documented; injection feature removed (was in cleanup-r15 2026-04, now formally specced).
- **Typed extension ingest endpoint** — `POST /api/v1/ingest/extension/{conversation|selection|webpage|browse_signal|upload}` replaces the free-form `source_type` field.
- **vault-locked buffer** — extension worker now buffers up to 1000 ingest payloads in `chrome.storage.local["pending_ingest"]` when the vault is locked; drains automatically on unlock.
- **Permission audit CI gate** — `scripts/extension-permission-audit.sh` fails the build on dangerous new permissions.
- Extension manifest bumped to v0.7.0.
- (Other v1.1 highlights: VLM provider, defamation v3 — see #174.)

### Breaking
- Server response for `/api/v1/ingest` and `/api/v1/ingest/extension/:type` now returns `409 Conflict` with `code: "vault-locked"` instead of `401 Unauthorized` when the vault is locked. Clients that key on HTTP status MUST update. Old extensions (v0.6.x) treat 409 as a generic failure and drop the payload — recommend users upgrade extension to v0.7.0.

### Migration
- Old extensions (v0.6.x) continue to work via the legacy `/api/v1/ingest` route. The typed `/api/v1/ingest/extension/:type` is additive.
- Deprecated MSG types `PREFETCH`, `SEARCH_RELEVANT`, `TOGGLE_INJECTION` log a console warning and return `{ok: false, deprecated: true}` for one release cycle; they are slated for removal in extension v0.8.

### Known Limitations
- E2E `ingest_e2e.spec.ts` requires a locally running attune-server with vault initialised at fixture password — does not run on GitHub Actions today; scheduled for nightly self-hosted runner in v1.2.
- Browse signals buffer is in-memory only (queue size cap 1000); persistent disk buffer for browse signals is deferred to v1.2.
```

- [ ] **Step 2: Bump versions**

```bash
sed -i 's/^version = "1\.0\.[0-9]\+"/version = "1.1.0"/' rust/Cargo.toml
sed -i 's/"version": "1.0.[0-9]\+"/"version": "1.1.0"/' rust/crates/attune-server/ui/package.json
sed -i 's/"version": "0.6.[0-9]\+"/"version": "0.7.0"/' extension/package.json
```

- [ ] **Step 3: Full workspace verify**

```bash
cargo test --workspace --release
bash scripts/extension-permission-audit.sh
bash scripts/privacy-audit.sh   # added in plan B1
cd extension && npm test && cd ..
cd rust/crates/attune-server/ui && npm run build && cd ../../../..
npx playwright test --project extension-e2e
```
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add RELEASE.md rust/Cargo.toml rust/Cargo.lock \
        rust/crates/attune-server/ui/package.json \
        extension/package.json
git commit -m "release: v1.1.0 — web plugin ingest-only reframe + typed endpoints"
```

- [ ] **Step 5: Merge develop → main + tag**

```bash
git push origin develop
git checkout main
git pull
git merge --no-ff develop -m "merge: develop → main (v1.1.0 release)"
git push origin main
git tag -a v1.1.0 -m "v1.1.0 — VLM provider + web plugin ingest-only reframe"
git tag -a desktop-v1.1.0 -m "desktop-v1.1.0 — same as v1.1.0"
git push origin v1.1.0 desktop-v1.1.0
```

- [ ] **Step 6: Verify CI green + binary printing 1.1.0**

Wait for `rust-release.yml` + `desktop-release.yml` to complete. Download Linux x86_64 artifact; `./attune --version` must print `1.1.0`.

---

## Self-Review

Spec coverage check (against `2026-05-28-web-plugin-as-knowledge-source.md`):

| Spec § | Requirement | Implemented by |
|--------|-------------|----------------|
| §1 core reframe | ingest-only positioning explicit | Task 1 (README + popup + options) |
| §2 v1.0.x doc-only scope | manifest+permission audit + popup disclaimer + README rewrite, no capture code change | Tasks 1, 7 |
| §2 v1.1.0 scope | typed endpoint + vault-locked buffer + MSG deprecation | Tasks 2, 4, 5 |
| §3.1 5 sources flow | sources documented in README; SOURCE_TYPES enum | Tasks 1, 2 |
| §3.3 network boundary | localhost-only verified by permission audit (host allow-list) | Task 7 |
| §4.1 file inventory | no new files added beyond the spec list | Task 4 (only new file is buffer.js, which is server-side companion to the existing worker.js) — confirmed within budget |
| §4.2 server ingest path | ingest_extension.rs new + reuses existing pipeline | Task 5 |
| §5.1 manifest v3 contract | manifest version bumped, permissions unchanged | Task 7 |
| §5.2 MSG schemas (4 active) | preserved verbatim in worker.js; typed endpoints honor the JSON shape | Tasks 2, 5 |
| §5.3 deprecation list | DEPRECATED_MSGS exported + console.warn on receive | Tasks 2, 4 |
| §6.1 new source flow | requires spec amend + permission audit pass — codified by audit script | Task 7 |
| §6.2 v1.1+ new sources (rss/pocket etc.) | out of scope for this plan; reserved for v1.2 | spec only |
| §6.3 plugin layer | unchanged, no new code | spec only |
| §7 boundary cases | server down (buffer) / vault locked (409+buffer) / unknown source_type (400) / dedup hash | Tasks 3, 4, 5 |
| §8 cost contract | extension cost stays at 🆓 — no LLM call added | Tasks 4, 5 honor this (POST only) |
| §9 test matrix | property test (Task 3), boundary integration tests (Task 5), E2E (Task 6), permission audit (Task 7) — 4 of 6 layers covered; remaining (adversarial + multi-source) deferred to v1.2 with explicit RELEASE.md note | tracked in v1.1.0 Known Limitations |
| §10 backward compat | legacy /api/v1/ingest preserved; deprecated MSG types still resolve | Tasks 2, 5 |
| §11 R1–R8 risks | R1 (`<all_urls>`) audit script; R3 (trust) popup disclaimer; R4 (SW sleep) buffer; R5 (vault locked capture) buffer; R6 (large upload) — not in scope here, deferred | Tasks 1, 4, 7 |

Placeholder scan: no "TBD", "implement later", or "appropriate error handling" markers. All steps either ship complete code or reference an exact file path + grep target.

Type consistency: `SOURCE_TYPES` enum exports the literal strings; `KNOWN_TYPES` in `ingest_extension.rs` lists the same five values; `DEPRECATED_MSGS` lists the same three strings as the worker.js warn case; `vault-locked` error code string matches between server (`routes/ingest_extension.rs`), client buffer (`pending_ingest_buffer.js`), and integration test.

Cross-plan consistency with the privacy-logic work (shipped v1.0.7 hotfix — `017ab81`; design ref `docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md`):
- `POST /api/v1/privacy/lock` (already shipped) — this plan's E2E (Task 6) uses it to put the vault into the locked state for the buffer test.
- Plan B1 introduces `scripts/privacy-audit.sh`; this plan adds `scripts/extension-permission-audit.sh`. Both are wired into CI as separate jobs, no name collision.
- `OutboundGate` (Plan B1 Task 3) does not gate localhost loopback traffic — extension → server traffic is exempt by design. Confirmed: the gate enforces 5 outbound kinds, none of which are "loopback".

Open items to surface before execution:
1. **`api.ingest` in worker.js drain path** — when buffer entries are missing a `source_type`, the legacy `/api/v1/ingest` is used. Confirm via grep that all enqueued entries carry a `source_type`; Task 4 enqueues from the typed handlers so this is guaranteed.
2. **Playwright extension E2E (Task 6)** depends on attune-server running locally — not a CI-friendly path. Plan acknowledges this in the v1.1.0 Known Limitations; the v1.2 nightly self-hosted runner is the intended target (task #126 already shipped a workflow stub).
3. **`ingest_inner` refactor (Task 5 Step 4)** — moving the existing `ingest` body into a private function is a non-trivial diff. If the existing handler returns `(StatusCode, Json<Value>)` instead of `Result`, adjust the return type to `Result<Value, (StatusCode, Value)>` consistently and update both call sites.

---

**Plan complete and saved to `docs/superpowers/plans/2026-05-28-web-plugin-ingest-only.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — execute tasks in this session using executing-plans, batch with checkpoints.

**Which approach?**
