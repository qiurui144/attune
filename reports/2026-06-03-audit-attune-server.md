# Audit: attune-server (Rust)

- **Area**: `/data/company/project/attune/rust/crates/attune-server`
- **Scope**: 15,468 LOC / 57 `.rs` files (src). routes 错误处理 / silent failure / 锁顺序 重点。
- **Method**: Glob/Grep 摸结构 + ctx_execute 分析锁序列/silent failure/复杂度 + 热点文件摘录读 (error.rs / state.rs / search.rs / items.rs / chat.rs)。read-only,不改代码。
- **Date**: 2026-06-03

## Scorecard (1 差 – 5 优)

| 维度 | 分 | 依据 |
|------|----|------|
| code_quality | 3 | lock poison 统一走 `unwrap_or_else(\|e\| e.into_inner())`(好);非 test `unwrap/expect` 仅限 lib.rs 启动期(可接受);但错误处理两套并存 + 一个 ABBA 锁序倒置 |
| complexity | 2 | `chat::chat()` = **1212 行单函数 / ~10 层嵌套 / 7 把锁**;`state.rs` 2028 行;`settings::update_settings` 172 行 |
| simplification_potential | 2 | 错误处理迁移半途(5 文件新 / 26 旧)、77 处 inline error JSON、40 处 "lock poisoned" 字面量、4 份 `err_500`/`err` 重复、`vault+dek` 样板散布 → 估可删/合 ~250-400 LOC |
| doc_accuracy | 3 | CLAUDE.md 锁序文档与代码漂移(top finding);测试数 "27 attune-server" 实际 252;错误处理"渐进 migration"仍 26/31 文件未迁 |

---

## 分维度 Findings

### (1) 正确性 / silent failure

| Sev | 位置 | 一句话 |
|-----|------|--------|
| **HIGH** | `routes/search.rs:176-178` + `routes/chat.rs:376-382` vs `routes/items.rs:122,143-144` | **ABBA 锁序倒置死锁风险**:search/chat 同时持 `fulltext→vectors→vault`(vault 最后);items.rs `update_item` 持 `vault` 后再取 `vectors+fulltext`(vault 最先)。两序相反 → 并发 update_item 与 search/chat 可互等死锁 |
| HIGH | CLAUDE.md「Lock ordering」节 | 文档声明序 `vault→vectors→fulltext→embedding`,**与代码真实序不符**:多锁同持的热点(search/chat/state.rs:765-766/items.rs:143-144)实际是 `fulltext→vectors→vault`。文档应被修正为真实序,且 update_item 应改为与热点一致(先 vault 后 vectors/fulltext 是反例) |
| MED | `routes/items.rs:148-150` | reindex 失败仅 `tracing::warn` 后吞掉(search 短暂 stale),无 enqueue_reindex 兜底重试 → 若该 path 永久失败则索引静默 drift。注释承认"下次 update 重试",但仅靠用户再次 update,无后台兜底 |
| LOW | `ingest_git.rs:128/174/180/191/193`、`ingest_webdav.rs:75`、`ingest_rss.rs:153` | `let _ = store.delete_item / record_signal_event / update_*` 静默吞错。signal/cursor 失败可接受,但 `delete_item` 失败被吞会留孤儿索引行 |
| LOW | `routes/privacy.rs:166` | `let _ = vault.store().audit_log_record(...)` — 审计日志写失败被静默吞。合规路径(DSAR/privacy)的审计落库失败应至少 warn |
| INFO | `routes/chat.rs:128/161`、`upload.rs:245/320` | `let _ = recommendation_tx.send()` — broadcast 无订阅者时返回 Err 属预期,吞错正确(无需改) |

### (2) 复杂度热点

| Sev | 位置 | 一句话 |
|-----|------|--------|
| **HIGH** | `routes/chat.rs:~60-1320` `pub async fn chat()` | **1212 行单函数**,~10 层缩进。混合:消息校验 / F-Pro domain detect / keyword trigger / skills router / 5-锁检索块 / 三阶段 spawn_blocking 压缩 / LLM call+retry / 信号写回。应拆 6-8 个 helper |
| MED | `src/state.rs` (2028 行) | AppState 巨结构(~30 字段) + 多个后台 worker `start_*` 内联。worker 启动逻辑可拆 `state/workers.rs` |
| MED | `routes/settings.rs:update_settings` (172) + `default_settings` (119) + `validate_settings_fields` (75) | settings 三函数加起来 366 行 + 810 行文件,字段校验/默认值可表驱动化 |
| LOW | `routes/office.rs:post_ocr` (159) / `post_transcribe` (117) | 长 handler,multipart 解析 + job 注册 + 错误分支多 |

### (3) dead code / 未用

- `#[allow(dead_code)]` 仅 2 处(`search.rs:308`、`office.rs:219`),office 一处有注释说明(backend 暂不暴露上限)。**整体很干净**。
- Cargo deps:25 个,`proptest` 唯一未被 src 引用 → **dev-dependency(tests/ 使用),非冗余**,无需动。
- 无 `TODO/FIXME/XXX/HACK/unimplemented!/todo!()`(0 处)。

### (4) 安全 (§1.4)

- 硬编码 secret 扫描:**0 命中**(无 api_key/token/password 字面量)。`eval.rs:382-385` 的 `x-attune-eval-seed: "42"` 等为测试 header,非 secret。
- 启动期 `expect()` (`lib.rs:346/400/414`):SIGTERM handler / ctrl_c / log directive — boot-time fail-fast,可接受。

### (5) doc-drift 清单

| 文档 | 声明 | 实际 |
|------|------|------|
| CLAUDE.md「Lock ordering」 | `vault→vectors→fulltext→embedding` | 热点真实序 `fulltext→vectors→vault`(见 search.rs/chat.rs);文档与代码冲突,且 items.rs 与热点又互相冲突 |
| CLAUDE.md「237+ tests(210 core + 27 server)」 | 27 attune-server | 实测 `#[test]`+`#[tokio::test]` = **252**(含 tests/ 37 文件)。数字陈旧 |
| `error.rs:23` + CLAUDE.md「渐进 migration」 | 旧 route 渐进迁 AppError | 31 route 文件中仅 **5** 用 AppError,**26** 仍 `(StatusCode, Json)` tuple。迁移基本停滞,客户端错误 shape 仍不统一(部分无 `code` 字段) |

### (7) 依赖冗余

- 无冗余(25 deps 全部被引用;proptest 为 dev-dep)。

---

## 简化 / 压缩建议(估 LOC)

| 建议 | 做法 | 估 LOC |
|------|------|--------|
| **统一错误处理(最大机会)** | 完成 `AppError` 迁移剩余 26 文件;77 处 inline `(StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":...})))` → `?` + `AppError`;删 `search.rs`/`chat_sessions.rs`/`office.rs`/`dsar.rs` 各自 `err_500`/`err` helper(4 份重复) | **-150~-250** |
| **lock-poison 样板** | 40 处 `.lock().map_err(\|_\| err_500("... lock poisoned"))?` 与 `.lock().unwrap_or_else(\|e\| e.into_inner())` 二选一统一;加 `trait LockExt { fn lock_recover(&self) -> Guard }` 扩展方法 | **-60~-100** |
| **vault+dek 取用样板** | `let vault = ...lock(); let dek = vault.dek_db()?` 跨 ~15 文件重复(dek_db 命中 10+ 文件) → `state.vault_dek()` helper 返回 `(Guard, Dek)` 或闭包封装 | **-40~-80** |
| **拆 `chat()` god-fn** | 1212 行拆为 `validate_input` / `detect_domain` / `run_search` / `compress_context` / `call_llm` / `emit_signals` 等 helper(不一定减净 LOC 但 -复杂度,可测性大增) | reorg ~400 |
| **state.rs worker 拆分** | `start_*_worker` 系列移 `state/workers.rs` | reorg ~300 |

**最大单点简化机会**:错误处理统一(AppError 迁移收尾 + helper 去重)→ **净删约 250-400 LOC** 且统一客户端错误契约(消除部分 route 缺 `code` 字段)。

---

## 优先级建议

1. **P0 修锁序倒置**(search.rs/chat.rs vs items.rs ABBA)— 真实死锁风险,且与 CLAUDE.md 文档冲突。选定唯一顺序(建议沿用热点 `fulltext→vectors→vault` 或反过来统一,改 items.rs),同步修文档。
2. **P1 完成 AppError 迁移**(26 文件)+ helper 去重 → 净删 ~250-400 LOC + 统一错误契约。
3. **P2 拆 `chat()`** 降复杂度。
4. **P3** items.rs reindex 失败加 `enqueue_reindex` 兜底;privacy.rs 审计写失败改 warn。
5. 修文档:测试数 27→252;锁序节。
