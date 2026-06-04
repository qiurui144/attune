# Spec: attune-server AppError 收尾迁移 (B4)

> SDLC sprint B4 (T-B 轨)。2026-06-04 起草。
> 前置 grounding: 直接 grep 实测 (本 spec §3/§7 数据均来自 `rust/crates/attune-server/src/routes/` 实扫)。
> 关联: [[project_sdlc_governance_tracks]] · audit `reports/2026-06-03-fullaudit-consolidated.md` (attune-server = 最弱 crate 2.5)。

## 1. 目标定位

attune-server 45 个 route 文件中只有 4 个 (email/rss/git/status) 用了统一 `AppError`，其余 30 个文件共 **181 处**仍手写 `Err((StatusCode::X, Json(json!({"error": msg}))))` tuple。问题:

- **错误 JSON shape 不一致**:旧 tuple = `{"error": msg}`(无 `code`);新 AppError = `{"error": msg, "code": "kebab"}`。客户端(Chrome 扩展 / attune-pro / Tauri webview)无法靠稳定 `code` 字段定向处理错误,只能 sniff message 文本。
- **样板冗余**:每处 tuple ~1-3 行 `Json(serde_json::json!({...}))`,可由 `?` + `From` impl 收敛。
- **维护性**:错误映射散落 30 文件,改一处错误语义要 grep 全仓。

**用户痛点对齐**:用户 2026-06-03 指令"attune 全量排查 + 代码压缩";本 sprint 是审计认定的最大单点压缩(attune-server 最弱 crate)+ 客户端错误契约统一。

## 2. 范围边界

**做**:
- 把 30 个 route 文件里 **错误路径** 的 tuple `Err((StatusCode, Json))` 迁移到 `AppError` + `?`/`return Err(AppError::X(..))`。
- handler 签名 `Result<T, (StatusCode, Json<Value>)>` → `AppResult<T>`。
- error.rs 新增 `TooManyRequests` variant(覆盖现有 429 site)。
- error.rs 调整 `IntoResponse`,使 wire message **不带类别前缀**(见 §10),消除 UX 回归。
- 每文件迁移后 status 保持核验 + `cargo test -p attune-server` + clippy 干净。

**不做(写死,禁止 scope creep)**:
- ❌ 不动**成功响应** tuple(`Ok((StatusCode::CREATED, Json(..)))` 等 2xx)——它们不是错误,保持原样(projects.rs:94/163 等)。
- ❌ 不改错误的 HTTP status 语义(FORBIDDEN 仍 403,不"顺手改成更合理的 401")。改 status = 行为变更,本 sprint 严格 status-preserving。
- ❌ 不重构 route 业务逻辑 / 不改 handler 签名之外的类型。
- ❌ 不碰 attune-core 的 `VaultError`(其 `From` 映射已存在于 error.rs)。
- ❌ 不动 attune-cli / attune-accounts / attune-pro 的错误处理(后续独立 sprint)。

## 3. 架构数据流

```
route handler (旧)
  └─ Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))
                                   │  迁移
                                   ▼
route handler (新)  -> AppResult<Json<T>>
  └─ something().map_err(|e| AppError::Internal(e.to_string()))?      // 显式映射
  └─ store.get(id).map_err(AppError::from)?                            // From<VaultError> 自动
  └─ return Err(AppError::BadRequest(format!("...")))                  // 直接构造
                                   │
                                   ▼  IntoResponse (error.rs, §10 修订)
        HTTP status (variant→status) + Json({"error": <raw msg>, "code": <kebab>})
```

**StatusCode → AppError variant 映射表**(grounding 实测分布,181 处):

| 旧 StatusCode | 次数 | → AppError variant | code |
|---|---|---|---|
| INTERNAL_SERVER_ERROR | 84 | `Internal` | internal |
| FORBIDDEN | 41 | `Forbidden` | forbidden |
| BAD_REQUEST | 22 | `BadRequest` | bad-request |
| NOT_FOUND | 16 | `NotFound` | not-found |
| UNAUTHORIZED | 5 | `Unauthorized` | unauthorized |
| BAD_GATEWAY | 4 | `BadGateway` | bad-gateway |
| SERVICE_UNAVAILABLE | 3 | `ServiceUnavailable` | service-unavailable |
| PAYLOAD_TOO_LARGE | 2 | `PayloadTooLarge` | payload-too-large |
| UNPROCESSABLE_ENTITY | 1 | `Unprocessable` | unprocessable |
| **TOO_MANY_REQUESTS** | 1 | **`TooManyRequests` (新增)** | too-many-requests |
| CREATED (201) | 2 | **不迁移**(成功响应,§7) | — |

无 DB tables / cache 变更。

## 4. 模块边界

- crate: `attune-server` (单 crate,不跨仓)
- 核心改动文件: `src/error.rs`(新增 variant + IntoResponse 修订 + 测试)
- 迁移文件(30): `src/routes/*.rs` —— per-file 清单见 §9 表。
- 不涉及: attune-core / attune-cli / attune-accounts / extension(客户端读 `error` 字段不变,§6 验证)。

## 5. API 契约

**对外 HTTP 错误契约(迁移后统一)**:

```json
{ "error": "<人类可读 message, 无类别前缀>", "code": "<kebab-case 稳定标签>" }
```

- HTTP status: 与迁移前**逐一致**(status-preserving,§2 硬约束)。
- `error` 字段: 与迁移前 message 文本**一致**(靠 §10 的 IntoResponse 改造保证不加前缀)。
- `code` 字段: **新增**(加性变更,旧客户端忽略未知字段不受影响)。

**无新增 endpoint / 无 path 变更 / 无 request schema 变更**。

## 6. 扩展点 / 兼容客户端

- 客户端读错误(实测 §grounding 点 6): extension `App.jsx:31 setMsg(result?.error || '保存失败')` —— **只显示 `error` 字符串,不 match 文本、不读 `code`**。→ 加 `code` 安全;message 保持原文(§10)避免 UX 回归。
- 未来新增错误类别 = error.rs 加 variant + `parts()` 一行 + 必要的 `From` impl,route 侧 `?` 自动生效(扩展点不变)。
- 后续可让 extension/attune-pro 改读 `code` 做定向处理(本 sprint 不强制客户端改动,只解锁能力)。

## 7. 错误 + 边界 case

| 边界 | 处理 |
|---|---|
| `Ok((StatusCode::CREATED, Json(..)))` 成功 tuple (projects.rs:94/163 等 2 处) | **排除**,不迁移。识别: 在 `Ok(...)` 位置 / 2xx status。 |
| `TOO_MANY_REQUESTS` (annotations.rs:127) | error.rs 新增 `TooManyRequests(String)` variant → 429 / "too-many-requests"。 |
| 多行 `json!({...})` 仅含 `"error"` key (grounding 实测 chat:77 / annotations:127 等均只 error key) | 正常迁移,无额外字段丢失。 |
| tuple 带 `"error"` 之外额外字段 (若迁移中发现) | **停下,单列**: 该 site 保留 tuple 或扩 AppError 携带 detail;不静默丢字段。迁移前每文件先 grep `Json(.*json!` 确认 shape。 |
| `.map_err(|_| (...))` 丢弃原 error (如 "vault lock poisoned") | 迁移为 `.map_err(|_| AppError::Internal("vault lock poisoned".into()))`,保留原文案。 |
| 同一文件混合已迁移 + 未迁移 (git.rs 用 AppResult 但仍 9 处 tuple) | 该文件**收尾全部** tuple,签名统一 AppResult。 |

## 8. 成本契约

零运行时成本(纯错误构造路径,编译期 `?` 展开)。无 LLM / 无本地算力 / 无新依赖(thiserror/serde_json 已在用)。开发成本: 本 sprint。

## 9. 测试矩阵

**回归守卫现状(grounding 实测)**: 全 crate 仅 ~5 处真断言 HTTP status → **现有测试无法自动 catch status 漂移**。本 sprint 的守卫策略:

| 类型 | 下限 | 手段 |
|---|---|---|
| error.rs 单元 | 每个 variant(含新 TooManyRequests)1 个 IntoResponse 测试,断言 status + code | `#[tokio::test]` in error.rs |
| message 保持 | 1 个测试断言 IntoResponse wire message **无类别前缀**(§10 核心) | error.rs 测试 |
| status 保持核验(per-file) | 每文件迁移后,grep 对比迁移前后每 site 的"原 StatusCode ↔ 选用 variant 的 status"一一对应 | `git diff` + 映射表人工核 + §3 表 |
| 全量回归 | `cargo test -p attune-server` 全 bin 0 failed(基线: 当前全绿) | 主循环 + background Bash(per 看门狗教训) |
| lint | `cargo clippy -p attune-server --all-targets -D warnings` 干净 | 同上 |
| 编译 | `cargo build -p attune-server` 0 error | 增量编译每文件后 |

**新增 HTTP 级回归(可选增强,若时间允许)**: 对 top-3 文件(items/annotations/chat)的关键错误路径加 1-2 个 `tower::ServiceExt::oneshot` 测试断言 status —— 把薄守卫补厚。列入 §11 风险缓解。

## 10. 向后兼容 (关键设计决策)

**问题**: AppError 当前 `#[error("bad request: {0}")]` 等给 Display 加类别前缀。迁移后 wire message 从 `"foo"` 变 `"bad request: foo"` → 客户端显示文案 UX 回归(extension 直接 `setMsg(result.error)`)。

**决策(提请 G1 裁决,推荐 Option B)**:

- **Option A**: 接受前缀(沿用 4 个已迁移文件的 precedent),靠 `code` 区分类别。
  - ✗ message 文本变更(非纯加性);UX 回归(用户看到 "bad request: " 噪声)。
- **★ Option B (推荐)**: 改 `IntoResponse`,wire `error` 字段用**原始内层 message**(不含类别前缀),类别由 `code` 承载。实现: variant `#[error]` 去掉前缀(如 `#[error("{0}")]`),或 IntoResponse 取内层 string。
  - ✓ 迁移成为**纯加性**契约变更(message 不变 + 仅加 code)。
  - ✓ 同时把 4 个已迁移文件也归正(去前缀),全 crate 一致。
  - 影响: error.rs Display 不再带前缀(日志若需类别,记 `code`);error.rs 现有测试用 `contains` 仍通过。

**migration path**: 无 DB schema / 无 client 强制改动。`code` 字段加性引入,老 client 忽略。Tag 时 RELEASE.md 记 "错误响应新增 `code` 字段(加性);错误 message 不变"。

## 11. 风险登记

| 风险 | 缓解 |
|---|---|
| **status 漂移**(选错 variant,403→500 等) | §3 映射表逐一对照;per-file `git diff` 核验;§9 status 保持核验步骤;映射机械(同 status 唯一 variant)降低判断空间。 |
| **回归守卫薄**(现有测试 catch 不到) | §9 补 error.rs 全 variant IntoResponse 测试;top-3 文件可选加 HTTP 级 oneshot 测试;每文件 cargo test + clippy。 |
| **隐藏额外字段丢失**(tuple JSON 带 error 之外字段) | §7: 每文件迁移前 grep `json!` shape;发现额外字段 → 停下单列,不静默丢。 |
| **message 前缀 UX 回归** | §10 Option B 根治(wire 用原始 message)。 |
| **大改动量**(30 文件 181 site)出错 | 按 §9 表 per-file 提交(1 文件 1 commit),易回滚 + review 粒度小;主循环驱动(不用 subagent,避免 agent 空转 per 本 sprint code-explorer 教训)。 |
| **CREATED 误迁**(成功当错误) | §7 排除;迁移时只动 `Err(...)` 位置,`Ok(...)` 不碰。 |
| **并发/性能** | 无(纯错误路径,无锁/无 IO 新增)。 |

---

**G1 提请裁决点**: (1) §10 Option A vs B(推荐 B);(2) §9 是否要求 top-3 文件补 HTTP 级 oneshot 测试(推荐"是",把薄守卫补厚);(3) 范围是否同意排除成功 tuple + 不改 status 语义(推荐"是")。

---

## ⚠ 实施发现 (2026-06-04, §3.1 回头改 spec)

**Task 0 已完成并提交** (error.rs: `TooManyRequests` variant + Option B 去前缀, 6 测试过, commit f97183a) —— 该基础设施独立有价值, 保留。

**Task 1 (per-handler 读盘) 推翻了"181 site 多为 uniform"假设**:

- **粒度是 per-handler 不是 per-site**:handler 签名 `Result<T, (StatusCode, Json)>` → `AppResult<T>` 是 all-or-nothing。handler 内**任一** site 是 rich-error(带 `error` 之外字段)→ 整个 handler 无法迁(否则丢字段, §7 红线)。
- **rich-error handler 比预期多**:实读前 3 个"最小最干净"候选 **全是 rich-error**:
  - `ingest.rs::ingest` + `upload.rs::upload_file`:backpressure 错误带 `pending_embeddings` + `retry_after_seconds`(客户端 retry 信号)。
  - `agents.rs::run_agent`:`{"error","code","message","agent_id","runtime"}` 5 字段结构化错误(已用自有 `RouteError` 别名)。
  - 已知还有 `classify`/`clusters`(hint)、`settings:291`/`git`(code)。
- **自动扫描不可靠**:多行 `format!` / 嵌套 `json!` 干扰括号匹配(awk 漏了 ingest backpressure)→ **必须逐 handler 人读**才能安全区分 uniform vs rich。
- **结论**:可机械迁移的 **uniform `{"error"}`-only handler 子集 < 181 site**, audit 的 "-250~400 LOC" 估值偏高。准确收益需读完 30 文件后定。

**重定范围(提请用户/G1 裁决)**:
- **Option 1(推荐, 安全)**:只迁 uniform-only handler 子集(逐 handler 读筛),rich-error handler 保留 tuple。收益较小但零契约破坏。
- **Option 2(更大设计)**:给 AppError 加 `Detailed { status, code, extra: Map }` variant 吸收 rich-error,全面统一。设计/测试成本更高。
- **Option 3**:B4 收益小于预期 → 降级,资源投其他 backlog(B3/B5)。

Task 0 基础设施(429 + Option B)无论选哪个都已落地, 不浪费。
