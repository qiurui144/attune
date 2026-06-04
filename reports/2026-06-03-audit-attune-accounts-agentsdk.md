# Audit: attune-accounts + attune-agent-sdk (attune, Rust)

**Date**: 2026-06-03  **Auditor**: code-audit agent  **Scope**: `rust/crates/attune-accounts` (1334 LOC) + `rust/crates/attune-agent-sdk` (342 LOC) — read in full (both small enough for non-sampled review).
**Mode**: read-only. No secrets echoed (per §1.4).

## Scorecard

| 维度 | 分 (1差–5优) | 说明 |
|------|:---:|------|
| code_quality | 4 | 干净、一致的 axum handler + 纯 Rust 密码学；poison-safe lock (`unwrap_or_else(\|e\| e.into_inner())`) 全覆盖；test 覆盖扎实。少量 `.unwrap()` on serde + 一处幂等/防重放语义缺口扣分。 |
| complexity | 5 | 无超长函数（最长 ~70 行 `get_llm_endpoint`，线性 5 步早返回）；无深嵌套；最大文件 832 行但 350 行是 test。agent-sdk 是教科书级 leaf crate。 |
| simplification_potential | 4 | 主要是 reference 性质的 test-only 死代码（`RegisterResponse` enum + `DeviceFingerprint::collect/signature` 全家桶），约 90-110 LOC 可删/可标注；生产 handler 本身已紧凑。 |
| doc_accuracy | 3 | `docs/plugin-protocol.md §8` 与实现有 3 处字段/endpoint 漂移（verify endpoint 未列、`license_token` vs 全 `DeviceLicense`、`existing_devices` vs `existing`）。 |

---

## 分维度 Findings

### (1) 正确性 / silent failure

- **[MED] license 激活无签名校验 — 防重放/防伪靠"能解码"而非"签名有效"** `lib.rs:328-378` (`activate_license`) + `lib.rs:395-468` (`get_llm_endpoint`)：两个 handler 都只调 `SignedLicense::from_code()`（注释明示"不校验签名, 仅 parse"）+ `is_expired()`，**从不调 `signed.verify(pubkey, now)`**。任何人可本地构造任意 `LicenseClaims`（任意 tier / quota / max_devices）→ base64 编码 → `activate` 通过 → `get_llm_endpoint` 发放 gateway token。签名机制 (`license_protocol::verify`) 存在且测试充分，但 server 端从未在准入路径上调用它。reference impl 也应演示正确校验，否则"reference"会被照抄成漏洞。**建议**：activate / endpoint 路径加 `signed.verify(server_pubkey, now)`，pubkey 由 `set_signing_key` 的私钥派生缓存。
- **[LOW] `get_llm_endpoint` 的 quota 永远不递增** `lib.rs:432-435`：`remaining = quota.saturating_sub(act.used_tokens_this_month)`，但 `used_tokens_this_month` 仅在 `activate` 时置 0，**无任何路径 +=**。即每次请求 `remaining` 恒等于满额，`monthly_quota_exhausted`(429) 分支 (`lib.rs:436`) 是 dead branch。注释 `lib.rs:464` 也只是"简化"。reference 语义可接受，但应在 doc/注释明示"quota 计量需生产实装"，当前读代码会误以为已计量。
- **[LOW] `activate` / `get_llm_endpoint` 之间 license_id 防重放仅幂等不防伪** `lib.rs:349`：`activated.contains_key(license_id)` 只防同 id 二次激活，配合上面 [MED] 签名缺失，攻击者改 `license_id` 即得新 slot。根因同 [MED]。
- **[INFO] `register_device` 续期分支重置 `issued_at`+token，但 `list_devices` 把 `issued_at` 当 `last_seen_at` 返回** `lib.rs:135` / `lib.rs:218`：字段语义混用（issued_at ≠ last_seen），与 protocol doc SQL schema 的 `last_seen_at` 列不对应。reference 可接受，标注即可。
- **[INFO] `serde_json::to_value(...).unwrap()`** `lib.rs:125,164,467`：对自有 `Serialize` struct 不会 panic（除非 map key 非 string / float NaN，此处无），可接受；若想零 unwrap 可用 `Json(value)` 直接构造。

### (2) 复杂度热点

- 无。最长函数 `get_llm_endpoint` ~70 行但为线性 5-step + 早返回，可读性好。`next_month_start` (`lib.rs:470-483`) 用 `Datelike` 正确处理 12 月跨年，边界 OK。无嵌套 > 2 层。

### (3) Dead code / 未用

- **[MED] `device_binding::RegisterResponse` enum 完全 test-only** `device_binding.rs:106-113`：生产 `register_device` handler 走 ad-hoc `serde_json::json!`（`lib.rs:139-145, 150-164`），从不构造 `RegisterResponse`。仅 `register_response_serde_ok` 测试引用。跨 crate grep 确认 attune-core / attune-server 零引用。~10 LOC + 其测试。
- **[MED] `DeviceFingerprint::collect()` + `signature()` + `hostname_or_default()` + `cpu_brand_or_default()` 生产零调用** `device_binding.rs:39-82`：仅 fingerprint 测试触发。signature() 的 sha256 设备指纹在任何准入路径都没用上（server 用 `device_id == token` 比对，不用 signature）。~45 LOC（含 helper）纯 reference scaffolding。
- **[LOW] `ActivatedLicense` 字段已 `#[allow(dead_code)]` 标注** `lib.rs:49-55`：`license` / `activated_at` / `used_tokens_this_month` 实际只 `used_tokens_this_month` 被读（且恒 0，见上）。已有 allow 注释说明"待 GET /licenses/{id}/status"，可接受但该 endpoint 未在 roadmap，建议要么实装要么删字段。
- **[LOW] `LlmGatewayConfig.upstream_endpoint` 写不读** `lib.rs:60`：configure 时存入，`get_llm_endpoint` 只用 `gateway_endpoint`/`default_model`，`upstream_endpoint` + `upstream_api_key` 从不被本 crate 读取（设计上 upstream 由真实 gateway 用，此处只是存档）。reference 性质可接受，标注即可。

### (4) 简化 / 压缩机会

| 项 | 位置 | 估省 LOC | 动作 |
|----|------|:---:|------|
| 删 `RegisterResponse` enum + 测试 | `device_binding.rs:106-113,170-184` | ~20 | reference 流程实际走 ad-hoc json，enum 无消费者 |
| 删 `DeviceFingerprint::collect/signature` + 2 helper + 测试 | `device_binding.rs:39-82,127-148` | ~55 | 准入路径不用 signature；如保留作 reference 则加模块级注释明示"演示用，未接入准入" |
| `register_device` 续期 / 新建两分支构造 `DeviceLicense` 重复 | `lib.rs:118-124,150-156` | ~8 | 抽 `fn issue_license(device_id, account_id, now)` |
| 三处 `summaries` 构造（register 409 / list）重复 map→DeviceSummary | `lib.rs:130-138,213-222` | ~6 | 抽 `fn to_summary(&StoredDevice) -> DeviceSummary` |
| **合计** | | **~89 LOC** | |

注：~75 LOC（前两项）是有意的 OSS reference scaffolding（2026-05-20 从 attune-core quarantine 来）。若产品决定 reference 只演示"真正用到的"流程，可删；若要演示 fingerprint 防伪能力，则应**接入准入路径**（同时修 [MED] 签名缺失），不要留成 test-only。

### (5) doc-drift 清单

| # | doc | 实际代码 | 漂移 |
|---|-----|---------|------|
| D1 | `plugin-protocol.md:374-386` API endpoints 列表只有 register/deactivate/list | `lib.rs:81-93` router 多了 `/devices/verify`、`/admin/licenses/generate`、`/licenses/activate`、`/admin/llm/configure`、`/llm/endpoint` 共 5 个 endpoint | doc 缺 5 个 endpoint |
| D2 | `plugin-protocol.md:379` register 200 返回 `{ device_id, license_token }` | `lib.rs:150-156` 返回完整 `DeviceLicense`（device_id/account_id/token/issued_at/expires_at） | 字段名 `license_token` vs `token` + 缺字段 |
| D3 | `plugin-protocol.md:380` 409 体 key 为 `existing_devices` | `lib.rs:142-144` 实际 key 为 `existing` | key 名漂移（客户端 parse 会 miss） |
| D4 | `plugin-protocol.md:360-372` SQL schema 含 `last_seen_at` 列 | `lib.rs` StoredDevice 无 last_seen，list 用 `issued_at` 顶替 | 语义漂移（reference 可接受，标注） |

D3 是会真正影响客户端的（key 名不匹配 → 客户端拿不到候选设备清单）；D1/D2/D4 是文档不全/简化。

### (6) 安全 (§1.4 secrets / 注入)

- **无硬编码真实 secret**。`main.rs:17-31` signing key 走 `ATTUNE_LICENSE_SIGN_KEY` env，未配置时优雅禁用 generate 并提示。测试用 `"sk-secret"` / `"sk-..."` 是明示 fake fixture（§1.4 允许），且有反向断言 `llm_endpoint_after_activation_returns_info` (`lib.rs:771-772`) 确认响应体**不泄露** `sk-secret` upstream key — 良好实践。
- **[MED] 见上 (1)：reference 签名校验缺失**是本审计最大安全项（reference 被照抄成生产即漏洞）。
- 无 SQL（in-memory HashMap）→ 无注入面；无路径拼接；axum typed extractor 防 body 注入。
- `gateway_token = format!("attune-gw-{license_id}")` (`lib.rs:461`) 是可预测 token，但注释明示是 reference；生产应发不可猜随机 token —— 建议注释里加一句。

### (7) 依赖冗余

- **attune-accounts**：`ed25519-dalek` / `base64` / `sha2` 注释明示 2026-05-20 从 attune-core quarantine（合理 — 把 license/fingerprint 死代码隔离到唯一消费者）。`hex` 用于 main.rs key decode + protocol verify，用到。`sha2` 仅 `DeviceFingerprint::signature()` 用 → 若按 (4) 删 signature，则 `sha2` 可一并移除（省 1 dep）。其余均用到。
- **attune-agent-sdk**：仅 `serde` + `thiserror`（+ dev `serde_json`/`proptest`）。**零冗余，零 native dep** — 与 leaf crate 的 wasm 不变量完全一致。CI deny-list 守卫（注释引 `ci.yml` "WASM leaf build guard"）+ `From<AgentError> for VaultError` 在 core 侧单向 (`error.rs:72`)，已验证无环。模范实现。

---

## agent-sdk 专项（wasm leaf 不变量）

- 跨 crate 验证通过：`attune-core/Cargo.toml:14` 依赖本 leaf，`agents/mod.rs:19` re-export `Agent/AgentOutput/AgentError/AgentResult`（保持 `attune_core::agents::*` 路径），`error.rs:72-` 的 `From<AgentError>` 用 `#[non_exhaustive]` catch-all 兜底未来变体（`AgentError` 已标 non_exhaustive，`lib.rs:78`）。方向 core→leaf 单一，无循环依赖。
- JSON wire byte-exact 测试 (`lib.rs:181-197`) 锁死 6 字段顺序/命名 → 防 subprocess/wasm agent 契约漂移。测试矩阵齐：8 golden + 5 boundary + 3 error + 3 proptest，达 §6.1 下限。
- 唯一吹毛求疵：`prop_confidence_any_finite_no_panic` (`lib.rs:335-340`) 对 NaN/Inf 只断言"不 panic"，未断言 `to_string` 必 Err；可接受（注释已说明两种结果都 OK）。

---

## 最大简化机会

**~89 LOC** 可删/重构（其中 ~75 LOC 是 test-only reference scaffolding：`RegisterResponse` enum + `DeviceFingerprint::collect/signature`；~14 LOC 是 register/list 内重复的 license/summary 构造可抽函数）。若同时删 `signature()` 则 `sha2` 依赖也可移除。

## 优先修复建议（按 severity）

1. **[MED 安全]** reference activate/endpoint 路径加 `SignedLicense::verify()` 签名校验 —— 否则 reference 被照抄即伪造 license 漏洞。
2. **[MED doc]** `plugin-protocol.md §8` 修 D3（`existing` vs `existing_devices`，会影响真客户端）+ 补 D1 的 5 个 endpoint。
3. **[MED 清理]** 删或标注 test-only 死代码（`RegisterResponse` / fingerprint collect+signature），~75 LOC。
4. **[LOW]** quota 计量未实装（`used_tokens_this_month` 恒 0，429 dead branch）—— 注释/doc 明示"生产需实装"。
