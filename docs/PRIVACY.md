# Attune Privacy

> _Local-first. Outbound-minimal. PII-redacted. User-sovereign._
>
> Last reviewed: 2026-06-05 (v1.0.6 GA). For audit history see
> [`docs/PRIVACY-AUDIT-CHECKLIST.md`](./PRIVACY-AUDIT-CHECKLIST.md).
>
> 本文档为 Attune 的隐私承诺与操作手册。中文摘要见文末「中文摘要」节。

---

## 1. The four promises

1. **Local-first.** All knowledge content (uploaded files, chat history,
   annotations, embeddings, vector index, full-text index) stays in your
   local encrypted vault by default. No outbound connection happens until
   you explicitly enable it in **Settings → Privacy**.
2. **Outbound-minimal.** There are exactly **five** classes of outbound
   connection the application can make, listed below. They are all
   **default-off**.
3. **PII-redacted.** Before any prompt or query crosses the network it is
   passed through `OutboundGate::enforce`, which applies the L1 regex
   redactor (12 builtin patterns: phone, national ID, email, address,
   IBAN, IP, etc.). L2 ONNX NER and L3 LLM-judgement redaction layers
   are available as opt-in.
4. **User-sovereign.** You can lock the vault at any moment, wipe the
   cloud session token with one click, export all your data (DSAR), or
   delete the account entirely.

---

## 2. The five outbound points

The Privacy dashboard at **Settings → Privacy** (top-level "Privacy" tab
on the sidebar, icon 🔐) shows the status of all five. They map 1:1 to
`OutboundKind` in `attune-core/src/outbound_gate.rs`:

| # | Kind | Default | What it sends | Where to disable |
|---|------|---------|---------------|------------------|
| 1 | **LLM** (`llm`) | off | PII-redacted prompts to chat/analysis provider (OpenAI / Anthropic / Gemini / DeepSeek / Ollama). | Privacy → LLM toggle. Disabling forces "no LLM" mode; chat still works against local memory but no synthesis. |
| 2 | **Attune Cloud** (`cloud_saas`) | off | Account ID + session token to `gateway.engi-stack.com` for Pro membership token gateway and quota sync. **No vault contents.** | Privacy → Attune Cloud toggle. The "Wipe cloud session" button below immediately revokes the token. |
| 3 | **WebDAV** (`webdav`) | off | Encrypted vault blocks (ciphertext) to your own WebDAV server. Remote sees only ciphertext. | Privacy → WebDAV toggle. |
| 4 | **Web search** (`web_search`) | off | Query string only, via headless browser to Bing / Google. Results are fetched then injected as Chat context. | Privacy → Web search toggle. |
| 5 | **Telemetry** (`telemetry`) | off | **Nothing in v1.0.6.** The send path returns `SendOutcome::SkippedNotImplemented`; the queue stub exists so future opt-in releases have a guarded path. | Privacy → Telemetry toggle. |

Every code path that performs network I/O is wrapped by
`OutboundGate::enforce(&policy, payload)`. The audit script
`scripts/privacy-audit.sh` greps the source tree to enforce this
invariant in CI.

---

## 3. Vault encryption boundary

- Master password → `Argon2id` (cost-tuned to ≥ 250 ms on first install) →
  per-vault Key Encryption Key (KEK).
- KEK wraps three Data Encryption Keys (DEKs): `dek_db` (rusqlite blobs),
  `dek_idx` (tantivy index), `dek_vec` (usearch HNSW vectors). All three
  are `AES-256-GCM` with random nonces.
- DEKs are decrypted in memory on unlock and zeroized on lock
  (`zeroize::Zeroize` + Drop guards).
- **Locked state is the off-switch.** While locked, every outbound point
  refuses to fire (the `OutboundGate` checks vault state first).

A "Lock now" button is available both on the sidebar (top right) and on
the Privacy dashboard.

---

## 4. DSAR (Data Subject Access Request)

Under **GDPR (EU)** and **PIPL (China)** you may request:

| Action | Endpoint | UI shortcut |
|--------|----------|-------------|
| Export all my data | `POST /api/v1/dsar/export` | Privacy → "Export my data" |
| Delete account + all cloud data | `POST /api/v1/dsar/delete` | Privacy → "Delete account & data" |
| View audit log | `GET /api/v1/audit/log` | Settings → Privacy → "Audit log" |

Local vault data is **not** auto-deleted by `dsar/delete` — that endpoint
removes cloud-side artifacts (membership account, gateway token usage
counters). To wipe local data, uninstall and delete `~/.local/share/attune`
(or `%APPDATA%\attune` on Windows).

Audit log records contain `(route, category, kind, redacted_count,
original_len, timestamp)`. They never contain prompts, responses, API
keys, or passwords. This invariant is asserted by
`tests/audit_log_redaction.rs`.

---

## 5. Third-party LLM provider retention summary

When you enable LLM outbound, the request is forwarded to the provider
you have configured (Wizard step 3 or Settings → AI Stack). Their data
retention is **not under Attune's control**; you should read their
policies. This snapshot is current as of 2026-06-05:

| Provider | Default training opt-out | Logging retention | Notes |
|----------|--------------------------|-------------------|-------|
| **OpenAI** API (gpt-4o, gpt-4o-mini, …) | Yes for API tier (vs ChatGPT consumer) | 30 days (abuse review only) | https://openai.com/policies/usage-policies |
| **Anthropic** Claude API | No training on API inputs | 30 days | https://www.anthropic.com/legal/aup |
| **Google Gemini** API | No training on paid API | 24 hours | https://ai.google.dev/terms |
| **DeepSeek** API | No training opt-out flag (verify before use) | Unspecified | https://deepseek.com/privacy |
| **Attune Pro Gateway** (`gateway.engi-stack.com`) | No training. Routes to one of the above. | 7-day error log only | See `cloud/docs/GATEWAY_PRIVACY.md`. |
| **Ollama** (local) | N/A | N/A | Runs in-process; no outbound. |

The monthly audit checklist (§3 of
[`PRIVACY-AUDIT-CHECKLIST.md`](./PRIVACY-AUDIT-CHECKLIST.md)) diffs these
provider policy URLs against committed snapshots and files a RELEASE
notice if anything weakens user protection.

---

## 6. How to verify outbound traffic locally

The strongest assurance is observation. With Attune running on your
machine and the privacy dashboard set to "all off":

```bash
# Linux: 60-second idle capture, should be ZERO outbound packets.
sudo tcpdump -i any -nn 'host gateway.engi-stack.com or port 443' \
    -G 60 -W 1 -w /tmp/attune-idle.pcap
tcpdump -nn -r /tmp/attune-idle.pcap | wc -l   # expect: 0
```

Then enable Web search, run one query, disable it again, and run the
query a second time. The second attempt MUST fail with a structured
`outbound-disabled` error — not silently retry.

---

## 7. 中文摘要

Attune 的隐私承诺概括为四条:

1. **本地优先** — 知识库、对话、批注、向量索引默认全部存本地加密 vault。
   任何出网行为必须用户在 **设置 → 隐私** 显式开启。
2. **出网最小化** — 全应用只有 5 条出网路径(LLM / Attune Cloud /
   WebDAV / 网络搜索 / 遥测),默认全部关闭。
3. **PII 脱敏** — 出网前必经 `OutboundGate::enforce`，内置 L1 正则
   脱敏 12 类(手机号 / 身份证 / 邮箱 / IP / 地址 / IBAN 等)。
4. **用户主权** — 任意时刻锁定 vault、一键清除云端会话凭证、
   导出全部数据(DSAR)、彻底删除账号。

操作入口:侧栏 🔐 「隐私」,首次启动会弹一次性引导(可点 "我知道了"
关闭,后续不再出现)。

DSAR 申请:`POST /api/v1/dsar/export`、`POST /api/v1/dsar/delete`,
或 UI 按钮(隐私页底部)。

第三方 LLM 厂商的留存策略请见上文 §5,Attune 在月度审计中比对这些
政策的快照(详见 `PRIVACY-AUDIT-CHECKLIST.md`)。

---

_Spec_: [`docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md`](./superpowers/specs/2026-05-28-privacy-logic-strategy.md)
_Audit script_: [`scripts/privacy-audit.sh`](../scripts/privacy-audit.sh)
_Test reproducers_: `crates/attune-core/tests/outbound_gate.rs`,
`crates/attune-server/tests/privacy_audit_log.rs`
