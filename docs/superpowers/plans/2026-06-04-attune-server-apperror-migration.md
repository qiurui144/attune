# attune-server AppError 收尾迁移 Implementation Plan (B4)

> **For agentic workers:** 本 plan 由主循环驱动执行(**不用 subagent** —— 本 sprint code-explorer agent 空转 253K token 教训 + cargo 看门狗教训);cargo 走主循环 + background Bash。Steps 用 checkbox 跟踪。

**Goal:** 把 attune-server 30 个 route 文件里 181 处 `Err((StatusCode, Json))` tuple 迁移到统一 `AppError` + `?`,status 逐一保持,错误响应统一加 `code` 字段,message 文本不变。

**Architecture:** error.rs 先扩(新 variant + Option B wire-message 改造 + 全 variant 测试),再按"易→难"逐文件迁移,每文件 1 commit + cargo build/test/clippy 守卫。

**Tech Stack:** Rust / Axum 0.8 / thiserror / serde_json。

---

## 执行约束(全程)

- **TMPDIR=/data/tmp-sdlc**(护 /);cargo 只 `-p attune-server`,**禁 `--workspace`**。
- 每文件迁移后:`cargo build -p attune-server` 必过;每批(3-5 文件)后 `cargo test -p attune-server`(background Bash)+ clippy。
- **status-preserving 铁律**:每 site 用 §映射表选 variant,选完 `git diff` 核对原 StatusCode = 新 variant 的 status。
- 1 文件 = 1 commit,msg `refactor(server): migrate <file> to AppError (B4 N/30)`。
- **只动 `Err(...)` 位置**;`Ok((StatusCode::CREATED, ..))` 等成功 tuple 不碰。

## 映射表(贴每个 task 用)

| 旧 StatusCode | AppError variant |
|---|---|
| INTERNAL_SERVER_ERROR | `AppError::Internal(msg)` |
| FORBIDDEN | `AppError::Forbidden(msg)` |
| BAD_REQUEST | `AppError::BadRequest(msg)` |
| NOT_FOUND | `AppError::NotFound(msg)` |
| UNAUTHORIZED | `AppError::Unauthorized(msg)` |
| BAD_GATEWAY | `AppError::BadGateway(msg)` |
| SERVICE_UNAVAILABLE | `AppError::ServiceUnavailable(msg)` |
| PAYLOAD_TOO_LARGE | `AppError::PayloadTooLarge(msg)` |
| UNPROCESSABLE_ENTITY | `AppError::Unprocessable(msg)` |
| TOO_MANY_REQUESTS | `AppError::TooManyRequests(msg)` (Task 0 新增) |
| CREATED / 2xx | **不迁移** |

## 迁移 Recipe(标准模式,每文件套用)

```rust
// 旧
pub async fn h(...) -> Result<Json<T>, (StatusCode, Json<serde_json::Value>)> {
    let x = foo().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": e.to_string()}))))?;
    if bad { return Err((StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": "msg".into()})))); }
    Ok(Json(...))
}
// 新
use crate::error::{AppError, AppResult};
pub async fn h(...) -> AppResult<Json<T>> {
    let x = foo().map_err(|e| AppError::Internal(e.to_string()))?;     // 显式
    // 或: let x = foo().map_err(AppError::from)?;   // 若有 From<该类型>
    if bad { return Err(AppError::BadRequest("msg".into())); }
    Ok(Json(...))
}
```

签名替换: `Result<X, (StatusCode, Json<serde_json::Value>)>` → `AppResult<X>`。移除该文件不再用的 `use axum::http::StatusCode;` / `use axum::Json;`(若仅错误路径用)—— 编译警告会提示。

---

### Task 0: error.rs 扩展(新 variant + Option B + 测试)

**Files:** Modify `src/error.rs`

- [ ] **Step 1: 写失败测试** — 新增 variant 测试 + message 无前缀测试

```rust
#[tokio::test]
async fn too_many_requests_maps_to_429() {
    let resp = AppError::TooManyRequests("rate".into()).into_response();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], "too-many-requests");
    // Option B: wire message 无类别前缀
    assert_eq!(v["error"], "rate");
}
```

- [ ] **Step 2: 跑测试确认 fail** — `cargo test -p attune-server --lib error:: 2>&1 | tail` → FAIL(variant 不存在 / message 带前缀)
- [ ] **Step 3: 实现**
  - 加 variant: `#[error("{0}")] TooManyRequests(String),`(注意:Option B 所有 variant 的 `#[error(..)]` 去类别前缀,改成 `#[error("{0}")]`)。
  - `parts()` 加 `AppError::TooManyRequests(_) => (StatusCode::TOO_MANY_REQUESTS, "too-many-requests"),`。
  - 现有 9 个 variant 的 `#[error("xxx: {0}")]` 全改 `#[error("{0}")]`(Option B,前缀去掉,类别由 `code` 承载)。
  - 评估加 `From<anyhow::Error> for AppError`(若 routes 大量在 anyhow 上 `?`,Task 1 扫描决定;→ `AppError::Internal(e.to_string())`)。
- [ ] **Step 4: 跑测试确认 pass** — `cargo test -p attune-server --lib error::` → PASS(含原有 5 测试,`contains` 仍通过)
- [ ] **Step 5: commit** — `refactor(server): AppError add TooManyRequests + drop category prefix from wire message (Option B, B4 0/30)`

### Task 1: 全量 JSON-shape 扫描(暴露边界 case)

**目的:** 迁移前确认 181 site 是否有 tuple 带 `"error"` 之外字段(§7 风险),+ 统计 `?`-on-anyhow 决定是否加 `From<anyhow::Error>`。

- [ ] **Step 1: 扫额外字段** — `grep -rnA3 'StatusCode::[A-Z_]*, Json' src/routes/ | grep -E '"[a-z_]+":' | grep -v '"error"'` → 列出所有非 error key 的字段。
- [ ] **Step 2: 分类** — 每个额外字段 site 标注:(a) 可丢弃(冗余信息)→ 正常迁移;(b) 客户端可能依赖 → 该 site **保留 tuple** 或扩 AppError 携带 detail(单列,不静默丢)。结论写进本 plan Task 1 备注。
- [ ] **Step 3: anyhow 统计** — `grep -rn '?;' src/routes/ | wc -l` 粗估;若 routes 普遍 anyhow,Task 0 补 `From<anyhow::Error>`。
- [ ] **Step 4: 无 commit**(纯调查,结论指导后续 task)。

### Task 2-N: per-file 迁移(易→难)

**批次顺序**(按 tuple 计数升序 = 易→难,每文件独立 commit):

**Batch A(小, ≤3 site/文件,~15 文件)**: feedback.rs(3) / ingest.rs(3) / ocr_profiles.rs(3) / 其余 1-2 site 文件。
**Batch B(中, 5-9 site)**: chat_sessions.rs(5) / dsar.rs(5) / profile.rs(6) / behavior.rs(9) / git.rs(9 收尾) / tags.rs。
**Batch C(大, 11-13 site)**: vault.rs(11) / chat.rs(13) / clusters.rs(13) / settings.rs(13)。
**Batch D(最大, 18-38 site)**: classify.rs(18) / annotations.rs(22, 含 429) / items.rs(38)。

每文件统一 5 步:

- [ ] **Step 1: 读文件 + grep 该文件 shape** — `grep -nA2 'StatusCode::' src/routes/<f>.rs`,确认无额外字段(Task 1 已标则跳)。
- [ ] **Step 2: 套 Recipe 迁移** — 改签名 → `AppResult<T>`;每 tuple 按映射表换 variant;清理 unused import;**跳过 Ok() 成功 tuple**。
- [ ] **Step 3: build** — `cargo build -p attune-server 2>&1 | tail` → 0 error。
- [ ] **Step 4: status 核验** — `git diff src/routes/<f>.rs`,逐 site 核对原 StatusCode ↔ 新 variant status 一致;确认无 Ok() 被误改。
- [ ] **Step 5: commit** — `refactor(server): migrate <f> to AppError (B4 N/30)`。

每批(A/B/C/D)结束:`cargo test -p attune-server`(background Bash)0 failed + `cargo clippy -p attune-server --all-targets -- -D warnings` 干净。

### Task N+1: top-3 HTTP 级回归测试(补薄守卫)

**Files:** `tests/apperror_http_regression.rs`(新建)

- [ ] **Step 1: 写测试** — 对 items / annotations / chat 各 1-2 个错误路径,`tower::ServiceExt::oneshot` 打 router,断言 status + `code` 字段。(参考现有 tests/*.rs 的 router 构造 helper。)
- [ ] **Step 2: 跑 → PASS**
- [ ] **Step 3: commit** — `test(server): HTTP-level status regression for migrated error paths (B4)`

### Task N+2: 收尾验证 + 文档

- [ ] **Step 1: 全量** — `cargo test -p attune-server`(background)全 bin 0 failed + clippy 干净 + `grep -rcE '\(StatusCode::[A-Z_]+, Json' src/routes/ | grep -v ':0'` 只剩成功 tuple(CREATED)+ Task 1 单列保留项。
- [ ] **Step 2: RELEASE.md** — 记"错误响应统一加 `code` 字段(加性);错误 message 去类别前缀(message 文本对旧 tuple 路径不变,对 4 个已迁移文件去前缀)"。
- [ ] **Step 3: 删本 plan**(per §3.2 实施完成后立即删)+ commit。
- [ ] **Step 4: push develop**(per push memory: temp-unset insteadOf + store helper)。

---

## Self-Review

- **Spec 覆盖**: §2 范围(迁移错误 tuple / 排除成功 tuple / 不改 status 语义)→ Task 0-N 全覆盖;§10 Option B → Task 0 Step 3;§9 测试 → Task 0 测试 + Task N+1 HTTP 测试;§7 边界 → Task 1 扫描 + 每文件 Step 1。✓
- **Placeholder**: 无 TBD;Recipe 给完整前后代码;映射表显式。✓
- **类型一致**: `AppResult<T>` / `AppError::X(String)` 全 plan 一致;variant 名与 error.rs §3 表一致。✓
- **风险**: status 漂移(Step 4 核验)/ 额外字段(Task 1)/ 大改动(per-file commit 易回滚)均有缓解。✓
