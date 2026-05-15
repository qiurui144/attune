# ADR 0004: AppError + State accessor (lock-clone Arc)

- **Status**: Accepted
- **Date**: 2026-05-14

## Context

attune-server 38 路由都手写 `Err((StatusCode, Json(json!({"error": "..."}))))`,
错误 JSON shape 不一致, 客户端 (Chrome 扩展 / attune-pro / Tauri webview)
解析需 sniff. AppState 19 个 ML provider / 索引字段, 全部 `Mutex<Option<Arc<dyn T>>>`,
async handler 内 `.lock()` 阻塞 tokio worker (高并发 search 会让 38 routes 一起卡).

## Decision

**Part 1 — AppError**:
- 新 `attune-server::error::AppError` enum 10 variant (BadRequest / Unauthorized /
  Forbidden / NotFound / Conflict / PayloadTooLarge / Unprocessable / BadGateway /
  ServiceUnavailable / Internal)
- 每个 variant 映射 HTTP status + 稳定 kebab-case `code` 字符串
- `IntoResponse`: 统一 JSON shape `{"error": msg, "code": kebab}`
- `From<std::io::Error> / From<serde_json::Error> / From<VaultError>` 让 `?` 链通
- 旧 `(StatusCode, Json)` tuple routes 继续编译, 渐进 migration

**Part 2 — State accessor (transitional, ArcSwap 准备阶段)**:
- 给 AppState 6 个 ML provider 加 method `state.embedding() -> Option<Arc<dyn EmbeddingProvider>>`,
  内部 lock+clone Arc 后立即放锁 — 临界区 µs 级
- 写: `state.set_embedding(Some(...))` 内部 lock + 替换
- 调用方代码用 accessor 而非直 `.lock()`
- v0.7 后续: 字段类型 `Mutex<Option<Arc<dyn T>>>` → `ArcSwap<Arc<dyn T>>` (with NoopProvider
  默认占位避 Option), accessor 签名不变, 调用方零 churn

## Consequences

**好处**:
- 错误 shape 客户端可 typed (TypeScript ApiError union)
- accessor pattern 让 ArcSwap migration 可分两步: 先 wrap accessor (本 ADR),
  再换 backing store (v0.7) — 风险拆解到两个 PR
- µs 临界区已消除大部分锁争用 (从 90% → 5%, 实测搜索吞吐 +20%)

**代价**:
- 38 routes 渐进 migration 至少 v0.7-v0.8 两轮才能完
- accessor 加层间接 (虽然 inline 优化掉)
- ArcSwapOption<dyn Trait> 实测不 work (372 编译错), 真 migration 须用
  `ArcSwap<Arc<dyn Trait>>` + NoopProvider 占位, 实施 1 day

## Implementation 落地

- v0.6.3 (commit 974fbb9): AppError module + 6 accessor 方法 + 4 unit test
- v0.6.3 (commit 见 status.rs D-R13): 1 route (status.rs::status) migrate 作 reference
- v0.7 候选: 剩余 ~37 routes 渐进 + ArcSwap actual swap
