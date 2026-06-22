# 端云协同调度 Model 1 — 容量信号协同（attune governor 接 k3-scheduler /capacity）

> 2026-06-22 · attune 侧实现。设计源 doc：`/data/company/project/attune-k3/docs/edge-cloud-scheduler-collaboration.md`（Model 1 全设计 + 能力图 + 缺口 + 契约）。
> 任务 #144。本 spec 落 attune **策略层（governor）** 的 Model 1 实现；k3-scheduler 侧（机制层）不在本 spec 范围。

---

## 1. 目标定位

**用户痛点**：K3 一体机形态下，attune 决定一次推理走「端侧（:8090 本地 A100/X100）还是云端」时**瞎决策** —— local/cloud 是静态二分（FormFactor 偏好），不知道本地此刻忙不忙。结果：本地排长队时该溢出到云却不溢；或反过来把本可本地秒回的任务推去云白花 token。

**对齐 positioning**：attune = "降低 token + 数据安全" 的私有 AI 知识伙伴。端云协同直接服务这两条北极星：
- **省 token**：本地空闲就本地跑（零云成本），只在本地忙/跑不动时才花云 token。
- **数据安全**：隐私 L0 任务**永不溢出云**，即使本地排队也等本地 —— 这是不可破的红线。

**职责分离（命脉）**：
- **attune = 策略层（Policy）**：隐私分级、脱敏、账户权益、准入。**隐私门（redaction + L0 永不出网）永远留 attune**。
- **k3-scheduler = 机制层（Mechanism）**：在 attune 给的隐私/账户约束内，按资源/能力回报「本地此刻忙不忙」（`/capacity` 容量信号）。scheduler **不做脱敏、不知账户**。

---

## 2. 范围边界

### 做（本 spec / 本次 impl）
- **能力图 SSOT**（capability map）：每能力（embedding/rerank/ocr/asr/chat-3b/7b/35b）→ {local 可行, cloud 可行, 默认偏好}。落 attune 数据（编进二进制 + 可被 catalog 覆盖路径预留）。
- **CapacitySignal 类型 + /capacity 客户端**：governor 提交推理前 `GET k3-scheduler /capacity?model=X` → `{state, eta_ms, mem_headroom_mb}`。
- **路由决策函数** `decide_route(capability, capacity_signal, privacy_class, account, cost)` → `RouteDecision{ Local | Cloud | QueueLocal | Reject }`。纯函数，可单测。
- **privacy_class 标注**：每推理任务标隐私级（复用 `PrivacyTier` L0/L1/L3）。L0 = local-only。
- **account quota/tier → 路由约束**：entitlement/member tier + 剩余配额 → 准入 + 降级（配额耗尽：本地兜底 / 拒 / 提示升级）。
- **telemetry**：路由决策落审计（复用 `UsageAggregator` + outbound audit），local/cloud 用量可回账户。
- **仅 K3 形态激活**：`FormFactor::K3Appliance` ∧ scheduler 可达时启用协同路由。**个人版（Laptop/Server）governor 行为 0 改动**（加 guard 测试）。

### 不做（明确写死，后续才做）
- ❌ **模型 2（统一调度器路由云）**：scheduler 加 cloud worker / policy admission / 统一计量 / cloud failover —— 本 spec 不碰，演进项。
- ❌ **k3-scheduler 侧任何代码**（`/capacity` 响应补字段、队列按 tier 加权）—— 那是 k3 仓的任务，本 spec 只消费现有契约。
- ❌ **真机 load-aware 验证**（本地忙 → 真溢出云）—— §7.3 标 PENDING（本机非测试环境 §1.6，需 K3 真设备）。本次只 mock `/capacity` 离线测。
- ❌ 改既有 `OutboundGate` / `governed_chat` 的契约 —— 协同路由在它们**之上**做决策，云分支仍必经 OutboundGate（不绕）。

---

## 3. 架构数据流

```text
                    ┌──────────────── attune 策略层（本 spec 实现） ────────────────┐
  推理请求 ─────────►│ EdgeCloudRouter::decide(task)                                 │
  (capability,      │   ① capability_map.lookup(capability)  → {local?, cloud?, 偏好} │
   privacy_class,   │   ② if K3 ∧ scheduler 可达:                                    │
   account)         │        CapacityClient.query(model) → CapacitySignal            │
                    │      else: signal = Unknown (静态二分回退)                      │
                    │   ③ decide_route(cap_entry, signal, privacy_class, account)     │
                    │        ↳ RouteDecision                                          │
                    └───────────────┬──────────────────────────────────────────────┘
                                    │
            ┌───────────────────────┼────────────────────────┬──────────────────┐
            ▼                       ▼                        ▼                  ▼
      RouteDecision::Local   RouteDecision::QueueLocal  RouteDecision::Cloud  Reject
            │                       │                        │              (配额耗尽
            ▼                       ▼                        ▼               + 无本地兜底)
      :8090 本地推理         等本地（不去云）          OutboundGate.enforce  → Err + 升级提示
      (K3 scheduler)        (L0 忙 / 强本地偏好)      (脱敏 + L0 二次拦截)
                                                            │
                                                            ▼
                                                      cloud_client → gateway
                                                            │
                                                            ▼
                                                    telemetry（local+cloud 回账户）
```

**隐私不变量（数据流红线）**：
- `privacy_class == L0` ⇒ `decide_route` 永不返回 `Cloud`（返回 `Local` 或 `QueueLocal`）。
- 任何 `Cloud` 分支 ⇒ 必经 `OutboundGate::enforce`（脱敏 + L0 二次拦截，defense-in-depth）。脱敏永在 attune。

**新增数据（无 DB schema 变更）**：
- `CapabilityMap`：编进二进制的静态表（`assets/capability-map.default.yaml` 或 Rust const）。
- `CapacitySignal`：内存值（每次查询，不落库）。
- telemetry 复用现有 `usage` / outbound audit 表，新增 `route_decision` 字段（kebab：`local`/`queue-local`/`cloud`/`reject`）进 audit meta。

---

## 4. 模块边界

| 模块 | 文件 | 职责 |
|:--|:--|:--|
| 能力图 SSOT | `attune-core/src/edge_cloud/capability.rs` + `assets/capability-map.default.yaml` | 每 capability → {local_capable, cloud_capable, preference} |
| 容量客户端 | `attune-core/src/edge_cloud/capacity.rs` | `CapacitySignal` 类型 + `CapacityClient`（HTTP `GET /capacity`）+ mock |
| 路由决策 | `attune-core/src/edge_cloud/router.rs` | `decide_route()` 纯函数 + `RouteDecision` enum + privacy/account 约束 |
| 模块根 | `attune-core/src/edge_cloud/mod.rs` | 导出 + `EdgeCloudRouter`（编排：lookup → query → decide）+ FormFactor guard |
| 接线 | `attune-core/src/governor.rs`（薄接入点）/ 复用 | governor 在 LLM 提交前调 `EdgeCloudRouter`；个人版短路 |
| 复用（不改契约） | `outbound_gate.rs` / `member_session.rs` / `platform/mod.rs` / `usage.rs` | OutboundGate 脱敏门 / tier+quota / FormFactor / telemetry |

新增 crate-level：`attune-core/src/lib.rs` 加 `pub mod edge_cloud;`。

---

## 5. API 契约

### 内部 Rust API（attune-core）

```rust
// capability.rs
pub enum Capability { Embedding, Rerank, Ocr, Asr, ChatLlm3b, ChatLlm7b, ChatLlm35b }
pub enum Preference { LocalPreferred, LocalStrong, CloudPreferred } // 默认偏好
pub struct CapabilityEntry { pub local_capable: bool, pub cloud_capable: bool, pub preference: Preference }
pub struct CapabilityMap { /* BTreeMap<Capability, CapabilityEntry> */ }
impl CapabilityMap { pub fn builtin() -> Self; pub fn lookup(&self, c: Capability) -> CapabilityEntry; }

// capacity.rs
pub enum CapacityState { ReadyFast, Queued, ReadySlow, Unavailable, Unknown } // Unknown = 查询失败/非K3
pub struct CapacitySignal { pub state: CapacityState, pub eta_ms: u32, pub mem_headroom_mb: u32 }
pub trait CapacityProbe: Send + Sync { fn query(&self, model: &str) -> CapacitySignal; } // 失败→Unknown(降级)
pub struct HttpCapacityClient { /* base_url=http://127.0.0.1:8090, timeout */ }
pub struct MockCapacityProbe { /* 预置 signal，离线测 */ }

// router.rs
pub struct PrivacyClass(pub PrivacyTier);            // L0 = local-only
pub struct AccountContext { pub tier: AccountTier, pub llm_quota_remaining: u64 } // 从 MemberState 派生
pub enum AccountTier { LoggedOut, Free, Paid }
pub enum RouteDecision { Local, QueueLocal, Cloud, Reject { reason: RejectReason } }
pub enum RejectReason { QuotaExhaustedNoLocal, CloudDisabledNoLocal, NotCapableAnywhere }
pub fn decide_route(
    entry: CapabilityEntry, signal: CapacitySignal,
    privacy: PrivacyClass, account: &AccountContext,
) -> RouteDecision;

// mod.rs
pub struct EdgeCloudRouter { map: CapabilityMap, probe: Box<dyn CapacityProbe>, form_factor: FormFactor }
impl EdgeCloudRouter {
    pub fn route(&self, cap: Capability, model: &str, privacy: PrivacyClass, account: &AccountContext) -> RouteDecision;
    // 个人版（非 K3）→ 一律走 cloud-preferred 静态路径（0 行为变化 guard）
}
```

### 外部 HTTP 契约（消费 k3-scheduler，不实现）

```
GET http://127.0.0.1:8090/capacity?model=<model>
→ 200 { "state": "READY_FAST|QUEUED|READY_SLOW|UNAVAILABLE", "eta_ms": <u32>, "mem_headroom_mb": <u32> }
失败（超时/连接拒/非 200/解析失败）→ CapacityState::Unknown（降级，不崩）
```

---

## 6. 扩展点 / 插件接口

- **新 capability**：`Capability` enum 加 variant + `capability-map.default.yaml` 加一行 → 自动纳入路由。
- **catalog 覆盖**：预留 `capability-map.yaml` 经 S8 download_with_failover 远程覆盖（复用 catalog 信任链机制；本次只编内置 baseline，远程覆盖为后续）。
- **演进到模型 2**：`RouteDecision::Cloud` 当前 attune 自己经 OutboundGate→cloud_client；未来可改为提交带 policy 的 `/infer` 让 scheduler 统一计量（脱敏仍预先在 attune 做）。decide_route 接口不变。
- **probe 替换**：`CapacityProbe` trait → 可换实现（HTTP / mock / 未来 gRPC）。

---

## 7. 错误 + 边界 case

| 场景 | 行为 | 错误码（kebab） |
|:--|:--|:--|
| `/capacity` 超时/连接拒/非 200 | `CapacityState::Unknown` → 退回静态二分（按 preference + capability），**不崩** | `capacity-probe-unreachable`（log warn，不上抛）|
| `/capacity` 返回畸形 JSON | 同上 Unknown 降级 | `capacity-parse-failed` |
| L0 + 本地忙（Queued/ReadySlow） | `QueueLocal`（绝不 Cloud） | — |
| L0 + 本地 Unavailable + 唯一能力在云 | `Reject{NotCapableAnywhere}`（L0 不可云，宁拒不泄）| `l0-no-local-capacity` |
| 配额耗尽（quota=0）+ 任务可本地 | 降级 `Local`/`QueueLocal`（本地兜底）| — |
| 配额耗尽 + 任务仅云可行（如 35b 本地跑不了）| `Reject{QuotaExhaustedNoLocal}` + UI 提示升级 | `quota-exhausted` |
| cloud 能力 disabled（用户关）+ 仅云可行 | `Reject{CloudDisabledNoLocal}` | `cloud-disabled` |
| 非 K3 形态（Laptop/Server）| **不查 /capacity**，走静态 cloud-preferred（=现状）| — |

**graceful degradation 总原则**：probe 不可达 → 静态二分（≈ 现状），绝不因协同层失败而 block 推理。

---

## 8. 成本契约

| 决策 | 归属层 | UI 显示 |
|:--|:--|:--|
| `Local` / `QueueLocal` | ⚡ 本地算力（K3 :8090，零云成本）| `~本地 · <eta>s`（QueueLocal 显示排队 eta）|
| `Cloud` | 💰 时间/金钱（云 token，扣账户配额）| `~<tok> tok · $<cost>` + tier |
| `Reject{QuotaExhausted}` | — | "本月配额已用完，升级会员 / 等下月重置" |

- 路由决策**不后台偷跑 LLM**：协同层只决定「在哪跑」，跑不跑仍遵成本契约（chat/分析仍需用户显式触发）。
- telemetry：每次路由的 (capability × route × cost) 落 `UsageAggregator`，local/cloud 用量回账户用于成本展示 + 审计。

---

## 9. 测试矩阵

| 类型 | case | 工具 |
|:--|:--|:--|
| **happy** | ReadyFast+local_capable → Local；Unavailable+cloud_capable+有配额 → Cloud | `#[test]` |
| **edge** | Queued/ReadySlow 边界；mem_headroom=0；eta 极大；preference 三档 | `#[test]` |
| **error** | probe 超时→Unknown 降级；畸形 JSON→Unknown；非200 | `MockCapacityProbe::failing()` |
| **adversarial（隐私红线）** | **L0+本地忙 → QueueLocal 永不 Cloud**（断言）；L0+Unavailable+仅云 → Reject（不泄）；Cloud 分支必经 OutboundGate（L0 二次拦截断言）| `#[test]` + 复用 outbound_gate L0 测 |
| **个人版 0 回退** | Laptop/Server 形态 → route 不查 /capacity + 决策恒等于静态 cloud-preferred（guard 测试，probe 调用计数=0）| `#[test]` + spy probe |
| **配额** | quota=0+可本地→本地兜底；quota=0+仅云→Reject+升级；Paid 充足→Cloud | `#[test]` |
| **proptest** | 任意 (state, privacy, quota) → L0 永不 Cloud 不变量；probe 失败永不 panic | `proptest` ≥3 |
| **集成** | mock scheduler `/capacity` HTTP 端点（wiremock/本地 axum）→ HttpCapacityClient 真解析 | `tests/edge_cloud_capacity.rs` |

**通过判据**：deterministic 路由 pass rate = 1.00；隐私不变量 proptest 0 反例；个人版 probe 调用计数恒 0。

---

## 10. 向后兼容

- **新增模块，无 DB schema 变更**：`edge_cloud` 是新 path，老部署不触发（仅 K3 形态激活）。
- **个人版（Laptop/Server）字节级 0 行为变化**：route 在非 K3 短路成静态 cloud-preferred；governor 既有 `governed_chat` 契约不变。
- **OutboundGate / cloud_client 契约不变**：协同层在其上做决策，云分支仍调原 enforce。
- **capability-map 远程覆盖**（后续）：无远程文件 → 内置 baseline = 当前静态偏好 freeze。

---

## 11. 风险登记

| # | 风险 | 缓解 |
|:--|:--|:--|
| R1 | **L0 隐私泄漏**（协同层 bug 把 L0 路由到云）| decide_route 硬断言 + proptest 不变量 + OutboundGate L0 二次拦截（defense-in-depth，脱敏永在 attune）|
| R2 | probe 不可达拖垮请求路径 | 短超时（默认 1.5s）+ 失败立即 Unknown 降级；probe 在决策前一次性查，不进推理热路径 |
| R3 | 个人版被误激活协同 | FormFactor::K3Appliance 单一 gate + guard 测试（probe 调用计数=0）|
| R4 | 配额竞态（并发耗尽）| 路由按读时快照决策；真实扣费仍由 cloud gateway 权威，attune 侧仅准入 hint，超扣由 gateway 拒（既有）|
| R5 | lock ordering | edge_cloud 不持有 vault/vectors/fulltext 锁；probe HTTP 在锁外；决策纯函数无锁 |
| R6 | 跨平台（probe HTTP）| reqwest+rustls 纯 Rust；mock 离线测；真机 §7.3 PENDING |

---

## 切片

| 切片 | 内容 | commit |
|:--|:--|:--|
| S1 | 能力图 SSOT（capability.rs + yaml + golden 测）| feat(edge-cloud): capability map SSOT |
| S2 | CapacitySignal + Probe trait + Mock + HttpCapacityClient | feat(edge-cloud): /capacity client + mock |
| S3 | decide_route 纯函数 + RouteDecision + 隐私不变量测 | feat(edge-cloud): routing decision + L0 invariant |
| S4 | EdgeCloudRouter 编排 + FormFactor guard + 个人版 0 回退测 | feat(edge-cloud): K3-only router + personal guard |
| S5 | 集成测（mock scheduler HTTP）+ telemetry 接线 | test(edge-cloud): capacity integration + telemetry |
