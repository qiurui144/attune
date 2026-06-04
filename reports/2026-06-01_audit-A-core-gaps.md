# Audit A — 核心功能缺失 / 未闭环 / Stub (read-only)

**Date**: 2026-06-01 · **Lens**: 核心功能缺失 / 未闭环 / stub · **Method**: 静态代码审计 (无 build / 无改码)
**Scope**: attune-OSS 核心闭环 `vault unlock → ingest → search(RRF) → chat+RAG → classify/cluster` + 4 已知线索 + release-clean 3 gap 核实

---

## 0. 结论速览

核心闭环 **全环真端到端 wire,无意外断裂**。所有"声称可用"的承诺均有 entry point → 实现 → 真跑通路径。
线索中的 stub 经核实 **全部是故意 stub**(注释明示 v1.1/v2 + graceful degrade),不是意外断裂。
release-clean 三个已知 gap(file-drop / updater / perf baseline)中 **两个已被 commit `3d215e3` 修复**,第三个属误归类。

- **P0 (核心闭环断)**: 0
- **P1 (重要功能 stub / 生产风险)**: 1 — desktop file-drop 上传的 auth token 来源在生产可能为空
- **P2 (边角 / 故意 stub / 架构 cruft)**: 4

---

## 1. 核心闭环 Wire 图 (全环真端到端,无断裂)

```
[vault unlock]  routes/vault.rs:91 vault_unlock → attune_core::vault.unlock(pwd)
     │            ↳ 颁发 token + init_search_engines() + reload_llm  ✅ 真
     ▼
[ingest]        routes/ingest.rs:34 ingest / routes/upload.rs:25 upload_file
     │            ↳ attune_core::ingest::ingest_document_with_profile
     │            ↳ content_hash 短路 + Inserted/Duplicate/Updated/Skipped 分支齐
     │            ↳ 即时 FTS add_document + enqueue embed(后台 worker)  ✅ 真
     │            ↳ backpressure 503 (pending embed > limit, OSS-S15)    ✅ 真
     ▼
[search RRF]    routes/search.rs:80 search / :236 search_relevant
     │            ↳ fulltext(tantivy) + vectors(usearch) + reranker
     │              全部 lock+as_ref 注入 attune-core HybridSearch        ✅ 真
     │            ↳ score cutoff(OSS-S17) + top_k≤100(OSS-S14 DoS guard) ✅ 真
     ▼
[chat + RAG]    routes/chat.rs chat_with_options
     │            ↳ knowledge 检索注入 prompt(:862 for k in knowledge)
     │            ↳ PII redact_batch 出网前 + restore 响应(:307/:377)    ✅ 真
     │            ↳ context budget / tier / force-local-for-evidence     ✅ 真
     │            ↳ LLM unavailable → graceful 原文 fallback(:761)       ✅ 真
     ▼
[classify]      routes/classify.rs:7 classify_one / :60 rebuild / :87 drain
     │            ↳ LLM classifier.classify_one → items.tags + TagIndex  ✅ 真
[cluster]       routes/clusters.rs:56 rebuild
                  ↳ attune_core::clusterer hdbscan(min_items=10 防 panic) ✅ 真
```

每一环都有 route handler → attune-core 实现 → 真索引/真 LLM/真加密路径。无 `todo!()` / `unimplemented!()` / silent NotFound 在主路径上。

---

## 2. 承诺 vs 实装 Gap 清单

| # | 承诺 (来源) | 实装状态 | file:line | 严重度 | 故意 stub / 意外断 | 根因 (一句话) |
|---|------------|---------|-----------|--------|------------------|--------------|
| A1 | desktop 拖拽文件即上传 (README/UI 拖拽体验) | **已修复 + 功能完整** | `apps/attune-desktop/src/main.rs:87` upload_dropped_paths + `:191` DragDrop::Drop emit; UI `ui/src/App.tsx:89` listen | — | 已闭环 (commit 3d215e3) | release-clean 报的"活 listener 只 alert"是旧死代码,#240 已用真 multipart POST 替换 |
| **A2** | desktop 拖拽上传带鉴权可达 embedded server | **生产 auth 风险** | `apps/attune-desktop/src/main.rs:89` `ATTUNE_DEV_TOKEN` (全仓唯一引用处) + embedded server `no_auth:false` | **P1** | 意外断 (潜在) | 上传 token 只从 `ATTUNE_DEV_TOKEN` env 读;若生产桌面未设该 env → bearer 为空 → vault 需鉴权时拖拽上传 401。需确认桌面进程是否注入该 env(本审计未见注入点) |
| A3 | desktop 自动更新 (RELEASE auto-updater) | **已修复 + 功能完整** | `apps/attune-desktop/src/main.rs:18` check_for_update_now(真 download_and_install) + `:79` restart_for_update; UI `SettingsView.tsx:652` UpdaterRow | — | 已闭环 (commit 3d215e3 / ed151e1) | release-clean 报的"updater 前端零接入"已修;30s 被动 check + 手动 check + 进度 emit 全有 |
| A4 | perf baseline 防回归 (docs/TESTING.md §perf) | **已实装,非 overwrite** | `attune-core/tests/perf_reindex_bench.rs` / `perf_chunker_bench.rs` / `rag_perf_audit.rs` + docs/reports/performance-baseline-v1-0-5.md | P2 | 误归类 | baseline 阈值 hardcode 在 test 源码 + 提交在 docs/reports;release-clean 的"perf-test 覆写基线"实为 **plugin 升级 overwrite `plugins/<id>/`**(见 2026-05-29-self-iteration-preservation-audit.md:67),与 perf baseline 无关 |
| A5 | python_subprocess capability runtime | 故意 stub (graceful Err) | `attune-core/src/capability_dispatch.rs:197` | P2 | **故意 stub** | runtime enum 不含 python_subprocess;parse 时返回 `unsupported-runtime` Err(非 silent NotFound),设计明示未实现 |
| A6 | telemetry HTTP send (RELEASE privacy) | 故意 stub (v1.1 gated) | `attune-core/src/telemetry.rs:4`(模块注释) `:88` SkippedNotImplemented | P2 | **故意 stub** | v1.0.6 只 ship queue+default-off 持久化;HTTP 后端 v1.1;default 永不 auto-opt-in,enabled 时返回 SkippedNotImplemented(spec §4.2 #⑤) |
| A7 | cloud logout 网络失败清 session | 真 bug,已测锁定 | `attune-core/src/cloud_client.rs` FIXME(v1.1) (test `logout_returns_err_on_unreachable_keeps_session_current_behavior`) | P2 | **故意 stub (有 test 锁定当前行为)** | logout 用 `?` 提前 return,网络挂时不清本地 session_cookie;test 明示锁当前行为,v1.1 应改无条件本地清 |
| A8 | query_rewrite "stub" model | 测试专用,非生产 | `attune-core/src/query_rewrite.rs:90` `#[cfg(test)]` 下 StubLlm | — | 非 gap (test fixture) | StubLlm 全部在 `#[cfg(test)] mod tests` 内;生产 `rewrite_query` 走真 LlmProvider + LLM 失败 fallback 原 query |
| **A9** | MCP client (产品决策: MCP ≥v0.7 不做) | **premature / dead code** | `attune-core/src/mcp_client.rs`(401 行) + `lib.rs:114 pub mod mcp_client` | P2 | 意外 (架构 cruft) | 401 行完整 stdio JSON-RPC client,但全仓 **零 instantiation**(无 McpClient::new / McpConfig::new 在 .rs 中、AppState 无 mcp 字段、router 无 mcp 路由)。与产品决策"MCP ≥v0.7 不做"矛盾,属提前落地未接线的死代码 |

---

## 3. 每条根因 + 处置建议

- **A2 (P1)** — desktop 拖拽上传 auth: 唯一需要后续动作的真风险。`ATTUNE_DEV_TOKEN` 是全仓唯一 token 来源,但审计未发现桌面进程在何处注入它。若 vault 鉴权开启 + env 未注入 → 拖拽上传静默 401(UI 会 toast `upload_fail`,但用户不知是 auth 问题)。**建议**: 确认桌面是否在 unlock 后把 session token 写入 `ATTUNE_DEV_TOKEN`,或改 upload_dropped_paths 走 unlock 后的 in-process token 颁发路径。
- **A9 (P2)** — mcp_client 死代码: 与产品决策直接矛盾(MCP ≥v0.7 不做),401 行未接线。**建议**: 要么标 `#[allow(dead_code)]` + 模块注释明示"v0.7 预留未接线",要么删除直到真正接线时再加(per CLAUDE.md §4.2.2 不为将来预留)。
- **A5/A6/A7 (P2)** — 三个故意 stub 均合规: 有注释明示 future version + graceful degrade(Err 而非 silent / default-off / test 锁定)。无需动作,符合 §4.5 LLM agent 兜底与隐私 spec。
- **A8** — 非 gap, test fixture 误入线索。
- **A4** — 误归类, perf baseline 真实存在且防回归机制健全; release-clean 原意是 plugin 升级 overwrite(独立问题)。

---

## 4. 审计边界声明

- 纯静态 + git log 核实,未 build / 未真跑 desktop / 未真触发拖拽上传。A2 的 401 风险是**代码路径推断**(token 来源唯一 + 未见注入点),需真机 desktop drop 验证才能从"潜在"升为"确认"。
- 闭环各环判"真 wire"依据: route handler 调 attune-core 真实现 + 无 todo!/unimplemented!/silent-success。已逐环 Read/grep 核实 file:line。
