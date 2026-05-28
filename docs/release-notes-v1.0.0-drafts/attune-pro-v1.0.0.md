# attune-pro v1.0.0 — law-pro Plugin Pack GA (2026-05-25)

> **attune-pro = industry capability plugin packs for Attune OSS.**
> v1.0.0 ships the **law-pro** pack: 11 deterministic agents + 3 LLM extractors covering the most common individual lawyer case types in Chinese civil law.
> Requires: attune (OSS) >= 1.0.0.

---

## Highlights

- **11 production-ready deterministic agents** — zero LLM token per invocation, pure formula/rule computation; covering civil loan, labor dispute, limitation period, evidence chain, bank statement aggregation, traffic accident, sale contract, housing rent, inheritance, interest calculator, and defamation
- **3 LLM extractors** — `fact_extractor` (F1 = 0.9828), `divorce_extractor` (F1 = 0.9710), `defamation_extractor` (Beta, F1 = 0.72); schema-guided array output + `chat_with_retry` fallback
- **Reliability Framework: 3-phase, 6-class floor** — per CLAUDE.md "Agent Verification Iron Rule"; 100% pass rate on `agent_golden_gate` harness
- **Signed `.attunepkg`** — Argon2id + AES-GCM encrypted YAML + Ed25519 signed; license key decrypts; install via `attune plugin install law-pro-v1.0.0.attunepkg` or Marketplace UI
- **Independent LLM GT re-audit** — 22 stress cases × 11 deterministic agents; confirmed 0 LLM dependency pollution in deterministic path

---

## Install

```bash
# Option A: via Marketplace tab in Attune desktop (recommended)
# Log in to Attune Cloud → Settings → Member → sync-plugins
# If you have a law-pro entitlement, the plugin auto-installs.

# Option B: manual file install
attune plugin install law-pro-v1.0.0.attunepkg

# Verify install
attune plugin list
# expected: law-pro  v1.0.0  active
```

Requires **attune >= 1.0.0** (server or desktop). Plugin will refuse to load on older attune versions.

---

## Agent Matrix

### Deterministic Agents (11) — Zero LLM Token

| Agent | Legal Basis | Capability |
|-------|-------------|-----------|
| `civil_loan_agent` | §680 + LPR | 民间借贷利息上限计算（LPR-capped） |
| `limitation_agent` | §188 | 诉讼时效判断（起算 / 中断 / 届满） |
| `labor_dispute_agent` | §47 / §87 | 劳动争议赔偿（N / 2N 算法） |
| `evidence_chain_agent` | — | 证据链关系分析与完整性评估 |
| `bank_aggregator_agent` | — | 银行流水聚合 + LLM hallucination guard（报告层） |
| `traffic_accident_agent` | 《道路交通安全法》 | 交通事故责任划分 + 赔偿估算 |
| `sale_contract_agent` | 《合同法》 | 买卖合同审查要点 + 风险标注 |
| `housing_rent_agent` | 《民法典》租赁章节 | 房屋租赁纠纷要素分析 |
| `inheritance_agent` | §1127 / §1130 / §1131 | 法定继承份额计算 |
| `defamation_agent` | 《民法典》名誉权 | 名誉权 / 一般侵权构成要件判断 |
| `interest_calculator` | — | LPR 利率引擎（civil_loan 内置，可独立调用） |

### LLM Extractors (3) — grounded, schema-guided

| Extractor | Status | F1 (qwen2.5:3b) | Output |
|-----------|--------|-----------------|--------|
| `fact_extractor_agent` | Production | **0.9828** | grounded 事实抽取，带原文依据槽位；ungrounded → 律师核实标记 |
| `divorce_extractor_agent` | Production | **0.9710** | 离婚案情要素抽取（财产 / 子女抚养 / 过错等），schema array |
| `defamation_extractor` | Beta | 0.72 (goal ≥ 0.75) | 名誉侵权 LLM 提取；`chat_with_retry` Lever 2；v1.0.1 调优 |

Token cost per invocation: ~1,500–2,500 tokens (displayed in Attune UI before confirmation).

---

## Reliability Framework

Three phases, all complete for v1.0.0 GA:

| Phase | Coverage | Status |
|-------|----------|--------|
| **Phase 1** — per-agent 6-class gate | Golden (≥10 real) / proptest (≥3) / boundary (≥5) / error fixture (≥3) / E2E subprocess (≥1) / regression fixture | ✅ All 11 det + 3 extractor pass |
| **Phase 2** — cross-agent stress | 22 stress cases × 11 deterministic agents; 0 LLM dependency confirmed | ✅ |
| **Phase 3** — true LLM gate | Real qwen2.5:3b holdout runs; F1 measured & locked | ✅ fact / divorce Production; defamation Beta |

`agent_golden_gate` CI harness: **100% pass rate** on develop branch.

---

## What's New (v0.7.0 → v1.0.0)

- **traffic_accident_agent** — new in v0.9.x sprint; full 6-class ENFORCE gate
- **sale_contract_agent** — new in v0.9.x sprint; full 6-class ENFORCE gate
- **housing_rent_agent** — new in v0.9.x sprint; full 6-class ENFORCE gate
- **inheritance_agent** — new in v0.9.x sprint; §1127/§1130/§1131 share calculation; proptest container tolerance fix
- **defamation_agent** + **defamation_extractor** — new in v1.0.0 rc; deterministic 构成要件 + LLM extractor; F1 0.56 → 0.72 via `chat_with_retry`
- **Robust LLM infra** — `fact_extractor` + `divorce_extractor` migrated to schema-guided array output; `chat_with_retry` Lever 1/2 retry with exponential backoff
- **22 stress-case sweep** — independent GT re-audit across all 11 deterministic agents
- **Plugin signing** — `.attunepkg` format with Ed25519 signature; `attune_core::plugin_sync::install_plugin_package` verifies sig before atomic install
- `plugin.yaml` updated: all 14 agents registered (8 Production agents were previously missing from registry — GA blocker fixed in `8c20267`)
- tech-pro / presales-pro / patent-pro plugin framework scaffolded (not yet published to pluginhub — v1.1.x roadmap)

---

## Breaking Changes (v0.x → v1.0)

- Plugin pack format: `.attunepkg` v1 (signed + encrypted). v0.x `.yaml` raw plugin directories are no longer loaded in production mode.
- `attune_min_version` field in `plugin.yaml` is now enforced server-side: packs declaring `attune_min_version: 1.0.0` will not load on attune < 1.0.0.

---

## Migration (v0.x → v1.0)

```bash
# 1. Upgrade attune OSS to v1.0.0 first (required)
#    see: https://github.com/qiurui144/attune/releases/tag/v1.0.0

# 2. If you had manually-placed v0.x plugin directories, remove them:
rm -rf ~/.config/npu-vault/plugins/law-pro-dev/

# 3. Install the signed pack:
attune plugin install law-pro-v1.0.0.attunepkg
# OR log in to Attune Cloud and use `attune sync-plugins`

# 4. Restart attune-server (if running headless):
sudo systemctl restart attune
```

No vault data migration required. Agent computation is stateless.

---

## Known Limitations

- `defamation_extractor` F1 = 0.72 (goal ≥ 0.75 for "Production") — v1.0.1 prompt tuning + golden set expansion
- `qwen2.5:7b` reaches F1 = 0.81 for defamation — recommended if accuracy is critical before v1.0.1
- Weak model matrix (gemma:2b / phi3:mini) — not yet tested; deferred to v1.0.1
- tech-pro / presales-pro / patent-pro packs: framework in repo, not yet published to pluginhub — v1.1.x
- `DEVELOP.md` for attune-pro pending documentation sprint — v1.0.1
- All agents operate on user-provided case facts; outputs are **calculation assistance only, not legal advice**. When uncertain, the UI prompts: "Please verify with your supervising attorney."

---

## Documentation

- law-pro user guide: https://wiki.engi-stack.com/attune-pro/law-pro
- Agent methodology: `docs/agent-skill-training-methodology.md` (866 lines, in attune-pro repo)
- Plugin development guide: https://github.com/qiurui144/attune/blob/main/docs/plugin-development.md
- Source (private): https://github.com/qiurui144/attune-pro
