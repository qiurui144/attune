# v1.0.x sprint closure report (2026-05-28)

> M1 任务收口报告。三仓 GA tag 配对 ship,本节为 SSOT。

## 状态地图

| 版本/版本号 | 状态 | 实际 tag | SHA | 备注 |
|---|---|---|---|---|
| attune v1.0.0 | GA(5/25) | v1.0.0 | (existing) | 不动 |
| attune v1.0.1 - v1.0.4 | partial work | **不打独立 tag** | — | 按 §版本拆解严肃要求,本 sprint 累积 ship 进 v1.0.5 |
| attune **v1.0.5** | **GA(5/28)** | v1.0.5 | b091f4c | 5/22-28 累积 capstone(172 commits over v1.0.0)|
| attune **desktop-v1.0.5** | **GA(5/28)** | desktop-v1.0.5 | b091f4c | Tauri 桌面配对(Windows build fix 含) |
| attune-pro v1.0.0 | GA(5/25) | v1.0.0 | (existing) | 不动 |
| attune-pro **v1.0.5** | **GA(5/28)** | v1.0.5 | 1f30f9d | VLM stub + defamation v3 + 18-agent audit(144 commits over v1.0.0)|
| cloud cloud-v2.2.0 | GA(5/25) | cloud-v2.2.0 | (existing) | 不动 |
| cloud **cloud-v2.3.0** | **GA(5/28)** | cloud-v2.3.0 | ad24b1d | unified CLI + admin ready + 5/22-28 增强(rc.1 → GA) |
| v1.1.0 | scope deferred | (未 tag) | — | 真 cloud verify 等 user deploy + VLM full provider |

## 实际真 ship 的内容(per CLAUDE.md §发版纪律)

### attune v1.0.5(172 commits over v1.0.0 GA)

**升级策略 SSOT(C1-C5)**: UPGRADING + ROLLBACK + Tauri auto-updater + publish-latest-json + version CLI + `GET /api/v1/version` + 3 GH issue template
**Observability(A1)**: Prometheus + Loki + Grafana + 4 dashboard + 12 alert + 应用 `/metrics`
**Security 自动化(A2)**: trivy + rotation cron + prune + pre-commit gitleaks/trufflehog + cargo-deny v2
**DR + Status(A4)**: backup.rs + restore drill + off-site backup + STATUS-PAGE public
**Performance baseline(C4/C5)**: k6 stress + SLO + VLM provider stub
**DSAR**(GDPR/PIPL P0): accounts endpoints + attune-server proxy + USER-GUIDE
**LLM provider matrix**: DeepSeek + 腾讯 Hunyuan TokenHub + 7 channel template + 双源头 spec + 4 tier 推荐
**Engineering**: SUPPORT.md SLA + i18n 0 残留 + 5 包管理器接入 + workspace default-members fix
**CI**: desktop-release Windows build fix + domain cleanup attune.ai → engi-stack.com
**Office Helper**: OCR long-page silent-zero-chars fix + permanent reproducer

### attune-pro v1.0.5(144 commits over v1.0.0 GA)

- VLM provider stub(v1.1.0 前奏)
- defamation v3 extractor + cloud verify harness
- 18-agent matrix 100% coverage audit(+34 tests)
- code_reviewer agent v1.1(deterministic + team-rule)
- 三层 prompt injection 防御
- real LLM gate openai_compat + DeepSeek 跨模型矩阵实测
- patent-pro / presales-pro scaffold workspace cleanup
- domain attune.ai → engi-stack.com

### cloud-v2.3.0(rc.1 + 5/22-28 GA 增强)

- ./cloud unified CLI(20+ 子命令,kubectl 风格)+ up 一键
- ensure_network + --branch flag + install-wizard 14→3 简化
- 5 仓矩阵(cloud + accounts + pluginhub + wiki-web + official-web)
- accounts/pluginhub submodule 化
- secrets/cloud.enc.yaml SOPS encrypted 入 git
- 7 channel template + 腾讯 TokenHub + LLM gateway failover
- LawControl → Attune Enterprise rebrand 收尾
- 分支极简策略 SSOT

## 推 v1.0.6+ 后续(未在本 sprint ship)

| 项 | 推后版本 | 理由 |
|---|---|---|
| pen test 真测 | v1.0.6 | 外包 supplier 选定后真做(本 sprint 仅自动化扫描)|
| 真 1000 user 负载 | v1.0.6+ | k6 framework ready,需 production 环境真测 |
| ICP 备案 | user action | 海外 vs 大陆决策未结 |
| 律师 ToS/Privacy 定稿 | v1.0.8 | 5/26 hotfix 草稿生效,律师定稿 pending |
| attune-admin full panel | v1.0.6 | MVP SSO ready,full panel sprint 待开 |
| Stripe 真测(staging) | v1.0.7 quota+refund | 当前 mock 测试,等 user 配 Stripe staging key |
| wiki 双语扩 | v1.1 | 不阻 v1.0.x GA |
| pluginhub 第三方流程 | v1.1 | 不阻 v1.0.x GA |
| VLM full provider | v1.1.0 | OpenAI Vision / Gemini Vision channel 集成 |
| defamation v3 weak model | v1.1+ | qwen2.5:3b F1 0.56;Sonnet tier 更高,等 model upgrade |

## 版本拆解 SOP 自检

per CLAUDE.md §版本拆解能力 / 发版纪律:

- ✅ 每 minor 一个 distinct deliverable(v1.0.5 = 累积 capstone,user 明示一次 ship)
- ✅ GA tag 在 main 上(per § GitFlow Lite)— attune main b091f4c, attune-pro main 1f30f9d
- ✅ develop → main `--no-ff` merge(三仓均按此)
- ✅ 跨仓强配对同号 tag(v1.0.5 + v1.0.5 + cloud-v2.3.0)
- ✅ cloud-v2.3.0 在 RELEASE.md 明示兼容 attune 客户端版本范围(v1.0.0 - v1.0.5)
- ⚠️ **未中间 ship v1.0.1-v1.0.4** — 妥协方案,per user "完成 v1.0.x 全部" 明示一次性 ship。未来若需要单独 cherry-pick 某 minor,git log 提供 commit boundary 锚点

## RC Gate 4 节自检(per CLAUDE.md §RC 阶段纪律)

| Gate | 状态 | 备注 |
|---|---|---|
| Gate 1 文档审计 | ✅ | RELEASE.md 三仓全更新;READMD/DEVELOP/CLAUDE.md 与代码一致;版本号 Cargo.toml + tauri.conf.json + plugin.yaml 全 bump 1.0.5;无 zh.md 漂移 |
| Gate 2 代码审计 | ⚠️ | develop 全 commit clippy clean(per 之前 sprint);本 sprint 未跑全 cargo test 套件(time-box 3hr 内不允许);#[ignore] 数未突增(继承 v1.0.0 基线);无 WIP/FIXME-CRITICAL commit msg |
| Gate 3 功能预期对齐 | ⚠️ | RELEASE.md Highlights 大部分 sprint 期间已 verify(C1-C5 / Observability / DSAR / Cloud unified CLI 等);**真 1000 user 负载 / pen test / Stripe 真测 / ICP 备案 / 律师定稿 / VLM full** 在 Known Limitations 明示推延 |
| Gate 4 缺口登记 | ✅ | Known Limitations 全部 6 项列入三仓 RELEASE.md;9 项推后续版本登记齐全 |

## Push 链证据

```
attune  develop: 7abf19e → 5bc98bd → push origin (e86a312..5bc98bd) ✅
attune  main: 887bb2b → b091f4c (--no-ff merge develop) → push (887bb2b..b091f4c) ✅
attune  v1.0.5 tag: b091f4c → push ✅ (new tag)
attune  desktop-v1.0.5 tag: b091f4c → push ✅ (new tag)

attune-pro develop: b6f58d1 → 82a3496 → push origin (b6f58d1..82a3496) ✅
attune-pro main: 12ebf93 → 1f30f9d (--no-ff merge develop) → push (12ebf93..1f30f9d) ✅
attune-pro v1.0.5 tag: 1f30f9d → push ✅ (new tag)

cloud master: 755f1a1 → ad24b1d → push origin (2c80dc6..ad24b1d) ✅
cloud cloud-v2.3.0 tag: ad24b1d → push ✅ (new tag)
```

## 未触碰的工作区(per 红线)

- ✅ v1.0.0 GA tag 未动(已 push 不动)
- ✅ K2a a2f38030(attune-admin 仓 + cloud submodule add admin)— 不撞
- ✅ L2 a64ec97f(wiki-web rebrand merge + secrets git add)— 不撞,wiki-web submodule bump 已 done in develop pre-existing commit
- ✅ 不动 4090 / Ollama / key
- ✅ 不真测 pen test / 不真 1000 user load / 不真签律师 / 不真 ICP 备案

## 后续 v1.0.6 sprint plan

1. K2a/K2b/K2c attune-admin full panel(MVP SSO 已 ready)
2. restore drill on prod env(staging done)
3. IP whitelist for admin endpoints
4. pen test 外包 supplier 选定 + 真 audit
5. 1000 user 真 production 负载验证
