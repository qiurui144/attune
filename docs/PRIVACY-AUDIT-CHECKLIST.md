# Privacy Audit Checklist (monthly)

> Run on the first Monday of each month. Owner: privacy maintainer.
> File the result as a GitHub issue tagged `privacy-audit` and link the
> output of `scripts/privacy-audit.sh` plus any policy diffs.
>
> 见 spec `docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md` §6.

---

## 1. Grep guards (CI also runs this; do a fresh manual pass)

- [ ] `bash scripts/privacy-audit.sh` exits 0 with `privacy-audit: PASS`.
- [ ] No hardcoded API keys anywhere:
      `grep -rnE '(sk-[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z-_]{35})' rust extension docs scripts`
- [ ] No telemetry call sites outside `attune-core/src/telemetry.rs`:
      `grep -rn 'telemetry::\|Telemetry::new\|TelemetryEvent' rust/crates/attune-*/src | grep -v 'telemetry.rs' | grep -v 'tests/'`

## 2. Provider policy diff

For each provider in `docs/PRIVACY.md` §5, fetch the current
data-retention policy URL and diff against the snapshot committed to
`docs/provider-policies/` (creating the snapshot the first time the
check is run).

- [ ] **OpenAI** — https://openai.com/policies/usage-policies
- [ ] **Anthropic** — https://www.anthropic.com/legal/aup
- [ ] **Google Gemini** — https://ai.google.dev/terms
- [ ] **DeepSeek** — https://deepseek.com/privacy
- [ ] **Attune Pro Gateway** — `cloud/docs/GATEWAY_PRIVACY.md` (internal)

If any policy weakens user protection (e.g. enabling training by default,
extending retention beyond what we advertise to users), file a
**RELEASE.md notice within 7 days** and update `docs/PRIVACY.md` §5.

## 3. Live install audit

- [ ] Install the latest GA on a clean Win + Linux machine.
- [ ] First launch: PrivacyTour modal renders (data-testid
      `privacy-tour-modal`) and dismisses to "never again".
- [ ] `tcpdump` during 60 s idle on unlocked vault with all five
      outbounds off → **zero outbound packets**.
- [ ] Toggle `web_search` on, run a Chat query, observe the request
      passes through `OutboundGate::enforce`. Then toggle off and run
      the same query → result must be `outbound-disabled`, not a silent
      success.
- [ ] Privacy dashboard `data-testid="vault-state"` reports `unlocked`,
      and clicking `vault-lock-now` transitions to `locked` and refuses
      subsequent outbound calls.

## 4. Recently changed code

- [ ] `git log --since="1 month ago" --diff-filter=A -- 'rust/crates/attune-*'`
      — any new `reqwest::get`, `tokio::TcpStream`, or `tonic` outbound
      call? If yes, confirm it's gated by `OutboundGate::enforce` and
      that `scripts/privacy-audit.sh` already lists the source file in
      its allow-list (and that the allow-list addition is justified in
      the PR description).

## 5. DSAR sanity

- [ ] `POST /api/v1/dsar/export` returns 200 and a JSON manifest.
- [ ] `POST /api/v1/dsar/delete` (against a throw-away account)
      removes the account from the cloud side; local vault is untouched.
- [ ] `GET /api/v1/audit/log?limit=10` shows entries with empty
      `redacted_meta` / no prompt content (assert via
      `tests/audit_log_redaction.rs`).

## 6. Documentation freshness

- [ ] `docs/PRIVACY.md` "Last reviewed" date is within 35 days of today.
- [ ] If the date is stale, update it as part of this audit; if the
      provider policy table needs changes, do those first.

---

## Pass criteria

All checkboxes above must be marked. If any item fails:
1. File a ticket in the `privacy` milestone.
2. Determine whether the failure blocks the current release (yes if it
   weakens a user-facing promise; no if it is internal documentation
   drift).
3. If yes, treat it as a v1.0.x hotfix candidate.
