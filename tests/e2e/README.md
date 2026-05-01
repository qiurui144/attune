# Attune E2E Tests

Layer 4 of the test pyramid (`docs/TESTING.md` §1.1) — Playwright Chrome golden flows.

## Constraint (per CLAUDE.md)

**Chrome only**. Do NOT switch to Chromium / Firefox / WebKit. The
`playwright.config.ts` is locked to `channel: 'chrome'`.

## Setup

```bash
cd tests/e2e
npm install
npx playwright install chrome  # one-time browser install
```

Build the server binary first:

```bash
cd rust && cargo build --release --bin attune-server-headless
```

## Running

```bash
# All specs (default Chrome project, 1 worker, headless)
npm test

# With UI (helpful for debugging visual changes)
npm run test:headed

# Step-through debugger
npm run test:debug

# View HTML report after test run
npm run report
```

## Structure

```
tests/e2e/
├── package.json              ← npm + Playwright config
├── playwright.config.ts      ← Chrome-only project, single worker
├── tsconfig.json             ← strict TS
├── helpers/
│   └── server.ts             ← spawnAttuneServer() fixture
└── specs/
    ├── smoke.spec.ts         ← C.2 framework smoke (this baseline)
    ├── wizard.spec.ts        ← C.3 #1 wizard 5-step golden flow (planned)
    ├── reader.spec.ts        ← C.3 #2 annotation → RAG boost (planned)
    └── chat.spec.ts          ← C.3 #3 citation jump → Reader (planned)
```

## Capability coverage map

Each spec MUST cite the FEATURES.md capability ID(s) it covers:

```typescript
test('vault setup → unlocked state', async ({ page }) => {
  // covers: F-01-VAULT, F-09-FORMFACTOR
});
```

Current matrix (per `docs/FEATURES.md` §4):

| Spec | Covers | Status |
|------|--------|--------|
| smoke.spec.ts | F-16-DISTRIBUTION (binary + UI loads), F-08-BROWSEEXT (CORS for extension) | ✅ baseline |
| wizard.spec.ts | F-01-VAULT, F-09-FORMFACTOR, F-16-DISTRIBUTION | 🚧 C.3 |
| reader.spec.ts | F-04-READER, F-02-RAG (annotation-weighted boost) | 🚧 C.3 |
| chat.spec.ts | F-03-CHAT, F-02-RAG (citation jump) | 🚧 C.3 |

## Server fixture lifecycle

Each `spec` file uses `spawnAttuneServer({ port })` in `beforeAll`. The fixture:

- Creates a tempdir for HOME / XDG_DATA_HOME / XDG_CONFIG_HOME → fresh sealed vault
- Spawns `rust/target/release/attune-server-headless --no-auth`
- Polls `/api/v1/status/health` until 200 (timeout 30s)
- Returns `{ baseUrl, proc, tmpDir, cleanup }`

`afterAll` calls `cleanup()` — SIGTERM + 2s grace + SIGKILL fallback + tempdir rm.

This means: each spec file gets one server. Tests **within** the spec share that
server (so vault state persists across tests in the same file). Cross-spec
isolation: separate ports + separate tempdirs.

## Anti-patterns to avoid

Per CLAUDE.md "Playwright E2E 测试规则":

- ❌ Don't mix Bash commands inside a Playwright test (e.g., `docker exec`, `curl`)
- ❌ Environment setup MUST happen before `test()` runs
- ❌ Don't use `chromium` / `firefox` / `webkit` channels — Chrome only

## CI integration (planned)

`scripts/test-pyramid.sh --with-e2e` already includes a hook for E2E. The
desktop-release.yml workflow may invoke a subset on macOS for native shell tests
(future work — not v0.6.1).
