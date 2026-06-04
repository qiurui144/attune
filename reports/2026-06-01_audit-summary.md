# attune OSS — 核心功能缺失 + 架构冗余排查 (consolidated scorecard)

> 2026-06-01 · read-only audit · 3 lens 并行 explorer · 触发: 用户 pivot(堆 RSS/云盘 connector 前先查地基)
> 子报告: [A core-gaps](2026-06-01_audit-A-core-gaps.md) · [B redundancy](2026-06-01_audit-B-redundancy.md) · [C oss-boundary](2026-06-01_audit-C-oss-boundary.md)

## 总判 (verdict)

**地基稳。核心闭环零断裂，冗余轻量(~400 LOC)，真正的债是 OSS 边界泄漏(行业代码回灌)。**

- 核心闭环 `vault→ingest→search(RRF)→chat+RAG→classify/cluster` **全环真端到端 wire，0 个 P0 断裂**。
- 不该在加新 connector 前推倒重来；该先做的是「边界归位 + 小幅减法」。

## 优先级行动清单

| 优先 | 类别 | 项 | 证据 | 建议 |
|---|---|---|---|---|
| 🔴 P0 | OSS 边界 | patent 全栈在 OSS(可执行,无 pro gating) | `routes/patent.rs` + `scanner_patent.rs`(153+L) | 迁 attune-pro/patent-pro + 从 OSS 删;与已发布 `oss-pro-strategy.md:80` 直接冲突 |
| 🔴 P0 | OSS 边界 | 18 个 paid 行业 agent 声明 + legal_defamation 流 + 律师触发词,且 OSS 测试耦合法律链 | `agents.registry.toml:118-361` · `agent_flows.toml:37-40`(名誉/诽谤/侮辱) · `case_metadata.rs`(原被告/案号) | 行业声明/流/测试迁出 OSS SSOT;OSS 测试门不应依赖法律链(CI 脆弱+能力清单泄漏) |
| 🟡 P1 | 核心功能 | desktop 拖拽上传仅 `ATTUNE_DEV_TOKEN` env bearer,生产未注入则 vault 鉴权开时静默 401 | `apps/attune-desktop/src/main.rs:89` | **真机验证** token 注入点;缺则补 |
| 🟢 P2 | 冗余(减法) | `capture/` 整目录死 scaffold(323L,0 生产 caller;真 IMAP 在 ingest/email.rs) | `capture/{email,telegram,mod}.rs` | 删整目录 |
| 🟢 P2 | 冗余 | `mcp_client.rs` 401L 完整但 0 instantiation,违产品决策「MCP ≥v0.7 不做」 | `attune-core/src/mcp_client.rs` | 删 or 明示预留 |
| 🟢 P2 | 冗余 | `html_to_text` 双实现 | `parser.rs:438`(私有) + `ingest/email.rs:54`(pub,rss 复用) | 合并保留 pub 版(先对齐两套测试) |
| 🟢 P2 | 冗余 | `plugin_sig::verify_strict` 死函数 0 caller | `plugin_sig.rs:121` | 删 or 文档化「PluginHub 上线激活」 |

**可回收 ≈ 350–420 LOC**(capture/ + verify_strict 纯减法风险低;html_to_text 合并需对齐测试)。

## 已证伪 / 无需动作 (honest negatives — 防误返工)

- ✅ `memory/`(L2 episodic) vs `memory_consolidation.rs`(L3 semantic) = **合理分层**,都有 state.rs caller,非 dup。
- ✅ 4-path ingest dup(upload/update/scanner/webdav) = **已在 v0.7 收口到 `ingest_document` 统一函数**,是正面案例。
- ✅ web_search ×4 / plugin ×5 / scanner / agent 家族 = 合理分层,非冗余。
- ✅ telemetry(v1.1 gated) / python_subprocess(graceful Err) / cloud logout(FIXME v1.1) = **故意 stub + graceful degrade**,合规。
- ✅ file-drop 上传 + updater 前端 UX = **已修(`3d215e3` #240)**,功能完整 → roadmap **S3(desktop-wiring) 基本作废**。
- ✅ 16 处 `#[allow(dead_code/unused)]`:仅 1 处真死(verify_strict),余为 serde/字段/test/feature-gate 合法。
- ✅ v0.6.0-rc.2 边界瘦身(CaseNo/extract_case_no/CHAT_TRIGGER_KEYWORDS/4 yaml)= 100% 完成;P0 违规是**瘦身之后**新 feature 回灌,非漏网。

## 对 roadmap 的影响

- **S3 desktop-wiring-fixes**:核心诉求(file-drop/updater)已由 #240 完成 → 建议**降级/作废**,仅留 perf-baseline 小项(且其实是 plugin 升级 overwrite 独立问题)。
- **新候选 S4 = OSS 边界归位**:P0 两条(patent 迁出 + 行业声明/流/测试出 OSS)是真正高价值、且触及已发布策略 → 走 spec-first SDLC(rename/迁移类,§3.1)。
- **快速清理(非 sprint)**:P2 减法 ~400 LOC 可直接 develop commit(§5.1 小改路径)。
