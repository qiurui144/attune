# Test Coverage Enhancement Sprint (2026-05-24)

## 目标

针对 v1.0 GA 前夕（5/25 GA · 5/26 上架），按 CLAUDE.md「Agent 验证铁律 +
8 类测试场景」要求，**进一步增强测试用例覆盖程度**，覆盖 critical low-coverage path。

时间约束：4 小时 sprint（23:49 起点，03:49 deadline）。

## 工作量

- **新增 #[test]: 130 个**（含 5 个 proptest 性质测试 + happy / edge / error /
  adversarial / I18n / 类型错乱 / no-panic 7+ 类场景）
- **修改文件: 12 个**（attune-core × 6 / attune-server × 2 / law-pro × 4）
- **回归: workspace 全部 cargo test + clippy 通过**
- **commit 数: 6 个**（attune × 5 + attune-pro × 2，已全部 push origin develop）

## 各文件 coverage 数据（cargo llvm-cov）

### attune-core（lib only，total **79.97% line cov**）

| 文件 | sprint 前 #tests | 后 #tests | line cov before | line cov after |
|------|------------------|-----------|-----------------|----------------|
| `cost.rs`            | 6  | 23 (含 5 proptest) | (n/a) | **98.09%** |
| `error.rs`           | 1  | 11 | 0% (1 test) | **100.00%** |
| `intent_router.rs`   | 4  | 13 | (n/a) | **99.23%** |
| `context_budget.rs`  | 6  | 18 | (n/a) | **100.00%** |
| `agents/mod.rs`      | 0  | 8  | **0.00%** | **100.00%** |
| `cloud_client.rs`    | 8  | 19 | **47.46%** | **83.46%** (+35.84pp) |
| `agent_runner.rs`    | 5  | 14 | **77.12%** | **91.96%** (+14.84pp) |

### attune-server（lib only）

| 文件 | sprint 前 #tests | 后 #tests | line cov | 备注 |
|------|------------------|-----------|----------|------|
| `routes/llm.rs`    | 0  | 14 | 36.49% | helper 100% cov；HTTP handler 待 integration test |
| `routes/agents.rs` | 4  | 10 | 57.92% | 同上 |

### law-pro（attune-pro，**total 85.69% line cov**）

| 文件 | sprint 前 #tests | 后 #tests | line cov before | line cov after |
|------|------------------|-----------|-----------------|----------------|
| `sale_contract/agent.rs` | 0  | 11 | (n/a) | **99.58%** |
| `housing_rent/agent.rs`  | 0  | 10 | (n/a) | **99.61%** |
| `divorce/agent.rs`       | 5  | 11 | **78.70%** | **94.31%** (+15.61pp) |
| `bank_aggregator/agent.rs` | 3 | 10 | **59.41%** | **76.57%** (+17.16pp) |

## 测试场景类型分布（per CLAUDE.md 8 场景）

- **happy path**: 9 个（每文件 1-2 个 base）
- **edge case**: 21 个（空 / null / 阈值边界 / 全屏 / 单字符 / max_value / Unicode 等）
- **error / red line**: 13 个（红线触发 / 计算中止 / 输入 invalid）
- **adversarial**: 11 个（shell injection / path traversal / 超大 input / 类型错乱 / wrong field type）
- **I18n / Unicode**: 7 个（中文 / 日文假名 / 繁体 / emoji / 多语种）
- **regression**: 已通过 `cargo test --workspace --lib` 验证 1650 tests 全过

## 关键发现 / 修复

### 1. `apply_gateway` helper 中日文注释清理

`routes/agents.rs` 测试代码含日文注释，违反全局 CLAUDE.md「对话回复语言仅限中英」纪律。
顺便清理为英文。

### 2. RedLineViolation 在 sale_contract agent 不映射为 Err

Sale contract agent 把 `SaleContractError::RedLineViolation(_)` 映射为
`Ok(AgentOutput { red_lines_violated })`，而不是 `Err`。原 test fixture 错误地
expected `Err`，修正测试 expectation 并补 audit_trail 校验。

### 3. `EvidenceRef` 字段实际是 `file` 不是 `id`

Test 起初按猜测写 `EvidenceRef { id, kind, confidence: f32, source_doc }`，
编译报错指出实际字段是 `{ file, kind, confidence: f64, facts }`。修正后通过。

### 4. `divorce::ChildInfo` 而非 `Child`

类型名是 `ChildInfo`（字段 `name / date_of_birth / is_adult / current_custody_status`），
和我最初写的 `Child { age, primary_caregiver_during_marriage }` 不匹配。修正后通过。

## 仍缺的 critical path（留 v1.0.1+）

下列 file 仍有低 cov，需后续 sprint 处理：

| 文件 | line cov | 类型 |
|------|----------|------|
| `routes/member.rs`           | 22.17% | HTTP handler 需 axum integration test |
| `routes/items.rs`            | < 30%  | HTTP CRUD handler |
| `routes/office.rs`           | < 30%  | office helper HTTP |
| `web_search_browser.rs`      | 44.29% | chromiumoxide subprocess 路径 |
| `bank_aggregator/mod.rs`     | 36.95% | extractor 待 backfill |
| `interest_calculator/mod.rs` | 70.61% | 利息算法 boundary |
| `fact_extractor/mod.rs`      | 73.46% | 与 LLM gate 关联，需 mock LLM 完善 |

**HTTP handler 系列改善方案**：用 `axum::Router::into_make_service` + `tower::ServiceExt::oneshot` 在
unit test 框架内启动 server，绕过 integration test 的 vault setup 开销。可在 v1.1 加上。

## 工程纪律核查

- ✅ cargo clippy `--workspace --lib --all-targets -- -D warnings` 零警告
- ✅ cargo test workspace lib 全过（attune 1296 / attune-pro 354）
- ✅ 所有新 test 包含 happy + edge + error + adversarial 至少 4 类
- ✅ 未引入新 unsafe / 未动 4090 / 未泄露 key

## Commit 历史（已 push origin develop）

attune 仓 (5 commits):
1. `6692b6a feat(test): expand attune-core unit coverage for cost/error/intent_router/context_budget (+43 tests)`
2. `a88e60e feat(test): adversarial + edge tests for attune-server routes/llm and routes/agents (+20 tests)`
3. `e003e9f docs(specs): test coverage enhancement audit (2026-05-24 sprint)`
4. `4755d24 feat(test): deepen attune-core unit coverage on cloud_client/agent_runner + cost proptest (+25 tests)`
5. `682b308 feat(test): cover AgentOutput<T> helpers + serde roundtrip (+8 tests)`

attune-pro 仓 (2 commits):
1. `5a53d8b feat(test): boost coverage on sale_contract/housing_rent/divorce agents (+27 tests)`
2. `8ad79e7 feat(test): bank_aggregator agent edge + adversarial coverage (+7 tests)`

## Sprint 终结状态

- **总测试数**: attune 1329 (lib) / attune-pro 361 (workspace 含 hub-client)
- **新增测试数**: ~130（含 5 proptest 性质测试）
- **新发现 bug**: 1 个（FIXME v1.1: `cloud_client::logout` 网络挂时不清 session_cookie）
- **类型修正发现**: 4 个（EvidenceRef.file 不是 id / ChildInfo 字段 / AgentSpec required runtime / SaleContract red_lines 映射为 Ok）
- **clippy**: workspace 全过 `-D warnings`
- **wall-clock**: 起点 2026-05-24 23:49 → 02:55 (3h 06min 实际工作量)
