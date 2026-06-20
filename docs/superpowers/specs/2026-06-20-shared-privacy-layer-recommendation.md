# Recommendation: Shared Privacy Layer (`privacy-core`) — 多项目共用隐私层

> Status: RECOMMENDATION (优化建议，待用户评审决策) · Date: 2026-06-20 · Owner: 跨项目隐私层设计 agent (INT-2)
> 参考源 (read-only): `qiurui144/kvm-info-privacy`, `qiurui144/kvm-privacy-gateway`, `attune-core::{pii, outbound_gate, redacting_llm, dsar}`
> 关联: `docs/superpowers/specs/2026-06-20-privacy-layer-enhancement.md`(本仓单项目视角；本文将其升级为**跨项目共享**视角)
> 关联约束: `~/.claude/CLAUDE.md §1.4`(Secrets)、`§3.1`(spec-first)、attune 产品决策(不引 MITM / 要可逆脱敏)、GDPR/DSAR 合规

---

## 0. 目的

用户将进一步优化 KVM 的隐私处理层，目标是**多项目（attune / kvm / 未来 lawcontrol 等）共用一个隐私层**。本文给出落地建议：抽出一个**进程内 Rust crate `privacy-core`**，作为三方现有能力的并集与单一事实源（SSOT），各项目按 feature/trait 接入，统一**必须统一的部分**（PII 类别 / 分级 / 可逆脱敏算法 / 审计 schema / 出网决策接口），保留**项目特化的部分**（attune 不要 MITM、要可逆 token；kvm 要 MITM、文档黑块可不可逆）。

**最大设计张力（先说结论，详见 §3 / §11）**：
1. **MITM 代理 vs 进程内 gate** — kvm 拦浏览器/HTTP 流量需要 hudsucker+CA；attune 产品决策**不装 CA / 不拦浏览器**。→ MITM 作 **可选 crate feature `mitm-proxy`**，attune 不开。
2. **可逆 token vs 不可逆黑块** — attune 导出/出网需 `[KIND_N]`+restore 可还原；kvm 文档对外分享要永久黑块不可逆。→ `RedactMode` enum 统一两模式，调用方选。

---

## 1. 现状对照 — 三方强项 / 弱项 / 重叠

| 维度 | attune (`pii`/`outbound_gate`/`dsar`) | kvm-info-privacy | kvm-privacy-gateway | 重叠 |
|------|------|------|------|------|
| 文本 PII 正则检测 | ✅ **12 类**(id/phone/email/ipv4/ipv6/credit/bank/plate/apikey/url/mac/gps) + 边界 sandwich 检查 | ✅ 8 类(id/phone/bank/email/plate/name/address/face) | — (复用 info 的 `/analyze`) | **高度重叠** id/phone/email/bank/plate；attune 多 ip/apikey/url/mac/gps；kvm 多 name/address/face |
| 校验位 | ✅ GB11643 + Luhn + IPv6 parse + GPS range | ✅ GB11643 + Luhn + 号段白名单 + 生日范围 | — | **重叠**(算法等价；kvm 号段+生日更细，attune 边界检查更强) |
| NER (姓名/地址) | ❌ (有 `ner.rs` 占位，词典 NER 未深做) | ✅ 词典 NER(姓氏表 + 地址触发词正则) | — | kvm 独有可移植 |
| 多检测器置信度融合 | ❌ 单层去 overlap，无 confidence | ✅ `fuse_confidence` 同值多检测器 +0.1 | — | kvm 独有 |
| 文档文件级解析 (PDF/DOCX/XLSX/img→bbox) | ❌ (仅 OCR 提文本，无 entity bbox) | ✅ `parsers/` + Entity{bbox/page/layer} | — | **kvm 独有，attune 最大缺口** |
| 文件字节级脱敏 (PDF 内容流 / OOXML XML) | ❌ (仅文本 prompt redact) | ✅ `redactors/text.rs` 等长字节替换 | (转发 info 的 `/redact`) | **kvm 独有** |
| 图像黑框 / 人脸遮罩 | ❌ (VLM 仅理解不脱敏) | ✅ `redactors/image_red.rs` + RKNN `face_det`(feature) | — | kvm 独有(NPU 绑定) |
| 可逆 placeholder (`[KIND_N]`+restore) + 同值同标签 | ✅ **独有**(redact/restore/redact_batch 全局唯一索引) | ❌ 不可逆黑块 `████` | ❌ | **attune 独有** |
| 机密关键词 fail-closed 拦截 | △ (L0 per-chunk 标签拦，无文档整体 block) | ✅ Aho-Corasick + classified→block | — | kvm 文档级更强 |
| 信息分级 | △ `PrivacyTier` L0/L1/L3 (per-event 标签) | ✅ `Classification` normal/sensitive_partial/classified (文档级) | △ `PrivacyMode` off/audit/redact (网关模式) | **三方各一套，必须统一(见 §3)** |
| 统一出网 gate | ✅ **`OutboundGate`** 6 出网点 6-clause 契约(disabled/vault/L0/redactor/redact) | ❌ | △ proxy 拦 AI 域名(域名白名单) | **attune 独有，应作共享接口** |
| 出网透明代理 (MITM 拦浏览器上传) | ❌ (内置 Chat 不注入浏览器) | — | ✅ hudsucker + CA + multipart 重写 | **kvm 独有，attune 不要(冲突点)** |
| 审计日志 | ✅ `outbound_audit` 本地 SQLite，**0 原文**(by_kind/total) | △ DetectionReport(含 entity.value，本地) | ✅ MariaDB `privacy_audit_log` **0 原文**(只存 sha256+type count) + KVM event POST | **重叠**(都 0 原文 schema 相似；存储后端不同) |
| 弱模型 LLM 兜底 (schema/retry/few-shot) | ✅ **独有** `llm_chat_redacted_hardened` | ❌ | △ RKLLM `llm_verifier`(降误报，实验) | attune 独有 |
| DSAR / GDPR 合规 | ✅ `dsar.rs`(export/delete) | ❌ | ❌ | **attune 独有，应作共享接口** |

### 一句话总结
- **attune 强**：出网文本侧（12 类检测 + 可逆脱敏 + 全局唯一索引 + 统一 6-clause gate + 0-原文审计 + 弱模型兜底 + DSAR）。
- **kvm-info-privacy 强**：文档文件侧（解析 + entity bbox + 字节级遮罩 + 图像/人脸 + 多检测器融合 + 词典 NER + 文档分级拦截）。
- **kvm-privacy-gateway 强**：网络流量侧（MITM 拦浏览器上传 + 域名白名单 + multipart 重写 + 集中审计 + 远端 verifier）。
- **三方都有**：格式化 PII 正则 + 校验位（**重复实现 ≥ 2 套**，最该合并）；0-原文审计理念（schema 该统一）。

---

## 2. 共享隐私层架构 — `privacy-core` crate (核心)

### 2.1 设计原则
1. **进程内 crate，非微服务**：attune 走进程内（不起新端口）；kvm 可把它链进 info-privacy-rs 服务 + gateway。crate 不假设运行形态。
2. **核心零重型依赖**：检测 / 脱敏 / 分级 / 审计 schema 全部纯 Rust，无网络、无 NPU、无 DB。重型能力（PDF/OOXML 解析、图像、RKNN、MITM、SQL 审计）全部 **feature-gated**，默认 `default = ["text"]`。
3. **trait 边界分离机制与策略**：`PiiExtractor` / `OutboundDecision` / `AuditSink` / `DocParser` 都是 trait，项目注入自己的实现，core 不绑定任何存储 / 端点 / 硬件。
4. **fail-closed 优先**：检测器单层失败 → 降级到可用层 + warn，**绝不**放行未脱敏内容；分级未知 → 视同最高密级；redact_kinds 空 → 脱敏全部已知类（沿用 kvm 修复）。

### 2.2 模块边界 + crate 树

```
privacy-core/
├── Cargo.toml                 # default=["text"]; features: ner, doc(pdf/ooxml), image, rknn, mitm-proxy, audit-sql
├── src/
│   ├── lib.rs
│   ├── kind.rs                # ⭐统一 PiiKind 闭合枚举 (attune 12 + kvm name/address/face = 并集) + Custom/Plugin
│   ├── tier.rs                # ⭐统一信息分级 Classification L0..L3 (合并 attune Tier + kvm Classification + gateway Mode)
│   ├── detect/
│   │   ├── regex.rs           # 移植两边正则 + 校验位 (GB11643/Luhn/号段/生日/IPv6parse/GPS range) — 单一实现
│   │   ├── boundary.rs        # attune sandwich 边界检查 (减误命中)
│   │   ├── ner.rs             # [feat ner] kvm 词典 NER (姓名/地址) — 中文
│   │   ├── fuse.rs            # kvm 多检测器 confidence 融合 (+0.1 同值多命中)
│   │   └── mod.rs             # Detector: text -> Vec<Entity>(kind,span,page,bbox,confidence)
│   ├── redact/
│   │   ├── text.rs            # ⭐可逆 token [KIND_N]+restore (attune) + 全局唯一 redact_batch
│   │   ├── mask.rs            # 不可逆黑块 ████ (kvm)
│   │   ├── doc.rs             # [feat doc] PDF 内容流 / OOXML <w:t>/sharedStrings 字节级替换 (kvm) — 两 mode 都支持
│   │   ├── image.rs           # [feat image] 图像黑框；[feat rknn] 人脸框
│   │   └── mod.rs             # Redactor { mode: RedactMode } + RedactionResult{redacted, mappings, stats}
│   ├── classify.rs            # 机密关键词 Aho-Corasick fail-closed (kvm) + 高密度 entity 升级 (kvm)
│   ├── gate.rs                # ⭐OutboundGate (attune) — 6-clause 契约，泛化为 trait OutboundDecision
│   ├── audit.rs              # ⭐统一审计 schema AuditEvent (0 原文) + trait AuditSink (SQLite/MySQL/POST 各项目实现)
│   ├── compliance.rs          # [feat compliance] DSAR export/delete trait (attune dsar.rs 泛化)
│   ├── proxy.rs               # [feat mitm-proxy] hudsucker MITM + 域名白名单 + multipart 重写 (kvm gateway) — attune 不开
│   └── plugin.rs              # PiiExtractor trait + 行业机密词注册 (attune-pro / law vertical)
```

### 2.3 关键 trait 接口

```rust
// kind.rs — 统一 PII 类别 (并集，snake_case 序列化对齐两边 fixture)
pub enum PiiKind {
    // attune 12 类
    IdCard, Phone, Email, Ipv4, Ipv6, CreditCard, BankCard,
    PlateNumber, ApiKey, Url, MacAddress, Coordinate,
    // kvm 补充
    Name, Address, Face,
    // 扩展点
    Custom(String), PluginProvided(String),
}
impl PiiKind { pub fn placeholder_prefix(&self) -> &str; }

// tier.rs — 统一分级 (4 级，三方映射见 §3.2)
pub enum Classification { L0, L1, L2, L3 }   // L0=永不出网/机密 · L3=公开
impl Classification {
    pub fn from_kvm(c: KvmClassification) -> Self;     // classified→L0, sensitive_partial→L1, normal→L3
    pub fn must_stay_local(&self) -> bool;             // L0 → true (fail-closed)
}

// detect/mod.rs — 检测器 (文本/文档统一 Entity 模型)
pub struct Entity { pub kind: PiiKind, pub span: (usize,usize),
                    pub page: u32, pub bbox: [f64;4], pub layer: Layer, pub confidence: f32 }
pub trait Detector { fn detect(&self, blocks: &[TextBlock]) -> Vec<Entity>; }

// redact/mod.rs — ⭐两种脱敏模式统一 (解决可逆性张力)
pub enum RedactMode {
    Reversible,      // [KIND_N] + restore (attune 出网/导出，可还原)
    Irreversible,    // ████ 黑块 / 等长空格 (kvm 对外分享，永久脱敏)
}
pub struct Redactor { mode: RedactMode, /* dict + plugin */ }
impl Redactor {
    pub fn redact_text(&self, text:&str) -> RedactionResult;          // 文本
    pub fn redact_batch<S:AsRef<str>>(&self, segs:&[S]) -> (Vec<String>, Vec<PiiMatch>); // 全局唯一
    pub fn restore(&self, text:&str, m:&[PiiMatch]) -> String;        // 仅 Reversible 有意义
    #[cfg(feature="doc")]
    pub fn redact_file(&self, path:&Path, report:&DetectionReport, kinds:&[PiiKind], out:&Path) -> Result<RedactionReport>;
}

// gate.rs — ⭐出网决策接口 (attune OutboundGate 泛化)
pub trait OutboundDecision {
    fn is_enabled(&self, kind: OutboundKind) -> bool;
    fn vault_unlocked(&self) -> bool;
    fn is_local_destination(&self) -> bool;
}
pub struct OutboundGate;
impl OutboundGate {
    // 6-clause: disabled / vault / L0-cloud-blocked / classified-blocked / redactor-required / redact
    pub fn enforce(policy:&OutboundPolicy, payload:&str) -> Result<String, OutboundError>;
}

// audit.rs — ⭐统一审计 schema (0 原文铁律) + 可插拔后端
pub struct AuditEvent {
    pub ts_ms: i64, pub kind: OutboundKind, pub action: String, // off|audit|redact|block
    pub doc_sha256: Option<String>,           // hash，永不原文
    pub by_kind: HashMap<String, usize>,      // {phone:2, id_card:1}
    pub classification: Classification,
    pub model: Option<String>, pub destination_label: Option<String>,
}
pub trait AuditSink: Send + Sync { fn record(&self, ev: &AuditEvent); }  // attune=SQLite, kvm=MySQL/POST

// plugin.rs — 行业扩展 (OSS 边界保持)
pub trait PiiExtractor: Send + Sync { fn name(&self)->&str; fn extract(&self,text:&str)->Vec<(usize,usize)>; }
```

### 2.4 各项目如何接入

| 项目 | 接入形态 | features | 接入点 |
|------|---------|---------|--------|
| **attune** (lib 依赖) | `privacy-core = { path/git, features=["text","ner","doc","compliance"] }`；**不开** `mitm-proxy` / `audit-sql`(用本地 SQLite 实现 `AuditSink`) | text, ner, doc, compliance | `attune-core` 把现有 `pii`/`outbound_gate`/`dsar` 替换为 re-export `privacy-core`；`AuditSink` 实现写 `outbound_audit`；`RedactMode::Reversible` 默认 |
| **kvm-info-privacy** (lib 依赖) | `privacy-core = { features=["text","ner","doc","image","rknn","audit-sql"] }` | 全文档 + 图像 + NPU | 服务层只剩 axum 路由 + RKNN worker 桥接；检测/脱敏/分级全调 `privacy-core`；`AuditSink` 实现写 MySQL；`RedactMode::Irreversible` 默认 |
| **kvm-privacy-gateway** (lib 依赖 + 可选 feature) | `privacy-core = { features=["text","mitm-proxy","audit-sql"] }` | + mitm-proxy | hudsucker handler 调 `privacy-core::proxy` + `Redactor`(Irreversible) + `AuditSink`(MySQL+POST) |
| **未来 lawcontrol 等** | 按需 features | — | 复用同一 trait 体系 |

---

## 3. 统一 vs 项目特化

### 3.1 必须统一（SSOT，跨项目契约，漂移 = bug）

| 统一项 | 理由 | 落点 |
|--------|------|------|
| **PII 类别枚举 + placeholder 前缀** | 两边各一套（attune 12 / kvm 8），重叠 5 类。不统一 → 同一身份证在 attune 叫 `IdCard` 在 kvm 叫 `id_card`，跨项目数据/审计无法对齐 | `kind.rs` 并集枚举 + snake_case 序列化 |
| **校验位算法** (GB11643/Luhn/号段/生日/IPv6) | 当前 **≥ 2 套重复实现**，维护漂移风险（一边修了 bug 另一边没修） | `detect/regex.rs` 单一实现 |
| **可逆脱敏算法** (`[KIND_N]` 索引 + 同值同标签 + 全局唯一 `redact_batch` + 长度降序 restore) | attune 已验证过 fuzz/UTF-8/overlap，是最难写对的部分；kvm 若要可逆应直接复用 | `redact/text.rs` |
| **信息分级语义** (4 级 L0–L3 + `must_stay_local`) | 三方各一套（见 §3.2），不统一则 gate 决策无法跨项目一致 | `tier.rs` |
| **审计事件 schema** (`AuditEvent` 字段 + **0 原文铁律**) | 合规审计要跨项目可聚合；0 原文是 §1.4 硬约束，schema 不统一易出原文泄漏 | `audit.rs` |
| **出网决策 6-clause 契约** | attune 已立"所有出网点必经 gate"铁律，是隐私层的总闸；kvm 也应纳入 | `gate.rs` trait |
| **fail-closed 默认** (kinds 空=全脱 / 未知分级=最高密 / 单层失败不放行) | 隐私优先的安全默认必须跨项目一致 | 各模块 |

### 3.2 三套分级如何合并为一套（关键映射）

| 统一 `Classification` | attune `PrivacyTier` | kvm-info `Classification` | gateway `PrivacyMode` | gate 行为 |
|------|------|------|------|------|
| **L0** 机密/永不出网 | L0 | `classified` (Aho-Corasick 命中) | (redact 模式下命中机密词) | **fail-closed block**，即使脱敏也不出网 |
| **L1** 敏感/强制脱敏 | L1 | `sensitive_partial` (高密度 entity) | `redact` | 强制 `Reversible` 脱敏后才出网 |
| **L2** 一般/审计 | (新增中间档) | — | `audit` | 记审计 + 按需脱敏，放行 |
| **L3** 公开 | L3 | `normal` | `off` | 直接放行 |

> 注：gateway 的 `PrivacyMode`(off/audit/redact) 是**网关运行模式**而非内容分级，应保留为**网关侧配置**（`mitm-proxy` feature 内），映射到内容 `Classification` 决策时落到上表 gate 行为；不要把"模式"和"内容密级"混为一个枚举（当前三方混用是债务）。

### 3.3 必须项目特化（feature flag / trait 隔离）

| 特化项 | attune | kvm | 隔离方式 |
|--------|--------|-----|---------|
| **MITM 透明代理** | ❌ 不开（不装 CA / 不拦浏览器，产品决策） | ✅ 开 | `#[cfg(feature="mitm-proxy")]` — attune 编译时不含 hudsucker/CA |
| **脱敏可逆性** | `Reversible` 默认（出网可还原） | `Irreversible` 默认（对外分享永久遮罩） | 运行时 `RedactMode` 参数，调用方选 |
| **图像 / 人脸 NPU** | ❌（x86 无 NPU，UI 明示"不检测图像内 PII"） | ✅ RKNN（K3/RK3588） | `#[cfg(feature="rknn")]`，**禁设 default**（per kvm CLAUDE.md 踩坑） |
| **审计存储后端** | 本地 SQLite `outbound_audit` | MySQL `privacy_audit_log` + KVM event POST | `trait AuditSink` 各自实现 |
| **DSAR / 合规** | ✅ `compliance` feature | 暂不需要 | `#[cfg(feature="compliance")]` |
| **行业机密词 / PII** | attune-pro 插件注册 | kvm 自带配置 | `trait PiiExtractor` + `register_keywords()` |

---

## 4. 迁移路径（渐进，不大爆炸，向后兼容）

> 原则：先抽 leaf crate（无反向依赖），各项目逐个切换，每步独立可测可回滚。参考 attune-core wasm leaf-crate 抽取经验（task #2 已验证此模式可行）。

**Phase 0 — 抽 crate（不改任何项目行为）**
- 新建 `privacy-core` 仓 / monorepo crate，把 attune 的 `pii::{patterns,mod,dictionary,ner}` + `outbound_gate` 原样搬入（attune 是最成熟的文本侧实现，作迁移基线）。
- attune 侧 `attune-core::pii` / `outbound_gate` 改为 `pub use privacy_core::...` re-export。**API 签名不变**，现有 `pii_chat_path_redact_test.rs` / `outbound_gate.rs` 测试 0 改动应全 PASS（向后兼容验证门）。

**Phase 1 — 合并校验位 + 类别枚举**
- 把 kvm-info-privacy 的 `name/address/face` 类别 + 号段/生日校验合并进 `kind.rs` / `detect/regex.rs`（并集，不删 attune 现有）。
- kvm-info-privacy 改为依赖 `privacy-core`，删本仓 `detectors/regex_det.rs` 重复实现（保留服务层 + RKNN worker 桥接）。回归门：kvm 现有检测 fixture 全 PASS。

**Phase 2 — 文档侧能力进 core**
- 移植 kvm `parsers/` + `redactors/{text,image}` + `classifier.rs` + `fuse.rs` 进 `privacy-core` 的 `doc` / `image` / `rknn` feature。
- attune 开 `doc` feature，按本仓 `2026-06-20-privacy-layer-enhancement.md` 的 DP.1–DP.4 切片接入文档级隐私（此时直接用共享 crate，无需 fork）。

**Phase 3 — 审计 schema + gate 统一**
- 定义 `AuditEvent` + `AuditSink`；attune 用 SQLite 实现，kvm 用 MySQL 实现。两边旧审计表**保留**，新事件双写一个 release 周期后切换（migration-safe）。
- gateway 依赖 `privacy-core` + 开 `mitm-proxy` feature，调共享 `Redactor`/`gate`/`AuditSink`，删本仓重复的 scanner→info HTTP 往返（改为进程内调用，省一跳网络）。

**Phase 4 — 分级统一 + DSAR**
- 三套分级映射到 `Classification`（§3.2）；`PrivacyMode` 降为 gateway 侧配置。
- attune `dsar.rs` 泛化进 `compliance` feature。

**向后兼容硬约束**：
- 每 Phase 都保 API 签名兼容（re-export / 同名 trait）；旧 DB 表 lazy-keep，新表增量。
- 序列化用 snake_case 对齐 kvm fixture（`classification` enum 值 / `entities[].bbox=[f64;4]` 顺序固定），便于复用对方测试语料。
- feature 默认 `["text"]`，CI 跨平台编译矩阵不变（rknn/mitm 默认关）。

---

## 5. 治理

### 5.1 放哪
**推荐：独立仓 `qiurui144/privacy-core`，各项目 git-dependency pin commit**（不用 git submodule，参考 wiki vendored 经验 + §8.1 submodule commit-pin 铁律）。

| 方案 | 评估 |
|------|------|
| **独立仓 + git-dep pin**（✅ 推荐） | 三仓技术栈独立（attune Rust / kvm Rust / 服务），crate 是纯库无运行形态绑定；commit-pin 避免 follow-branch 漂移；版本独立演进（§1.1.8 插件版本独立） |
| monorepo crate | 三项目不在同一 repo，不适用 |
| git submodule | §8.1 要求 commit-pin（可行），但 submodule 对 Cargo 工作流不如 git-dep 顺手；不推荐 |
| 发布到 crates.io | 含中国 PII / 机密词，且仍在快速演进，过早公开发布不合适；私有 git-dep 更稳 |

### 5.2 版本策略
- `privacy-core` 走**独立 SemVer**（§1.1.8：包版本独立于项目 tag）；只在真有能力 delta 时 bump。
- 各项目 `Cargo.toml` pin 到具体 commit/tag（不 follow branch）。
- **跨项目契约变更**（PiiKind / Classification / AuditEvent schema）= **major bump** + 三项目协调升级（§7.1.5 强配对思路：契约层变更需同步）。

### 5.3 谁维护
- core 由**隐私层 owner**统一维护；各项目只提 feature/trait 实现（AuditSink / PiiExtractor），不 fork core 逻辑。
- 任何检测/脱敏/分级逻辑改动**只在 core 改一处**，禁止各项目本地重复实现（当前 ≥ 2 套校验位就是反模式）。

### 5.4 测试矩阵（跨项目契约硬门）

| 类 | 下限 | 内容 |
|----|------|------|
| **PII 检测 F1（多语言）** | 中文 + 英文 fixture，per-kind precision/recall；格式化 PII recall ≥ 0.99 / 0 幻觉 | 合并两边 golden + attune 真实语料(CS-Notes 等) |
| **脱敏可逆性** | round-trip：`Reversible` redact→restore == 原文；全局唯一 `redact_batch` 跨段不混淆 | attune 现有 redact 测试 + proptest |
| **脱敏不可逆性** | `Irreversible` 输出**回喂 detect → entity 必空**（kvm 三重验证铁律：MD5 不同 ≠ 脱敏正确） | kvm 反模式回归测试 |
| **机密拦截 fail-closed** | classified/L0 → block；kinds 空=全脱；单层失败不放行；未知分级=最高密 | 异常 case |
| **跨项目契约** | `PiiKind`/`Classification`/`AuditEvent` 序列化 round-trip + **0 原文断言**（audit 永不含 value） | schema 测试 |
| **adversarial** | ZIP 炸弹 / OOXML 路径穿越 / XML 实体 / 任意 bytes 不 panic / catastrophic regex（regex crate 线性时间）| `doc` feature 安全面（P0） |
| **跨平台编译** | `default`(x86) + `rknn`(aarch64 cross) + `mitm-proxy` 各编译矩阵；rknn 误设 default 的 CI 断言 | per kvm 踩坑 |

---

## 6. 关键约束（写死）

1. **MITM 是可选 feature**：`privacy-core` 默认**不含** MITM 代理 / CA 安装代码（在 `#[cfg(feature="mitm-proxy")]` 内）。attune 编译时不开此 feature，二进制不含 hudsucker/CA 依赖 — 守 attune"不装 CA / 不拦浏览器"产品决策。
2. **两种 redact 模式都支持**：`RedactMode::{Reversible, Irreversible}`。attune 出网/导出默认 `Reversible`(`[KIND_N]`+restore)；kvm 文档对外分享默认 `Irreversible`(黑块/等长空格)。调用方显式选，core 不预设单一模式。
3. **§1.4 密钥**：core 自身不含任何真实 key/token；审计 0 原文（只 sha256 + by_kind count）；API key 检测器命中后同走脱敏，不入 log。
4. **GDPR/DSAR 合规**：`compliance` feature 提供 DSAR export/delete trait（attune 现有 `dsar.rs` 泛化）；审计可聚合但永不含原文，满足"可被遗忘 + 可审计"双要求。
5. **fail-closed 跨项目一致**：分级未知→最高密；redact_kinds 空→全脱；检测器单层失败→降级+warn 不放行。

---

## 7. 落地切片建议（per §7.1 版本拆解）

| 切片 | 主题 | 关键交付 | blockedBy |
|------|------|---------|-----------|
| **PC.0** | 抽 crate（attune 文本侧作基线） | `privacy-core` re-export，attune 测试 0 改动 PASS | — |
| **PC.1** | 类别 + 校验位合并 | `kind.rs` 并集 + `detect/regex.rs` 单一实现；kvm-info 删重复 | PC.0 |
| **PC.2** | 文档侧进 core | `doc`/`image`/`rknn` feature 移植 kvm parsers/redactors/classifier | PC.1 |
| **PC.3** | 审计 schema + gate 统一 | `AuditEvent`+`AuditSink`；gateway 开 `mitm-proxy` 接入 | PC.2 |
| **PC.4** | 分级统一 + DSAR | `Classification` 三套映射 + `compliance` feature | PC.3 |

---

## 8. 总结

抽一个进程内 Rust crate `privacy-core` 作三方能力并集与 SSOT：**统一** PII 类别 / 校验位 / 可逆脱敏算法 / 4 级分级 / 0-原文审计 schema / 6-clause 出网 gate / fail-closed 默认；**特化** MITM(可选 feature，attune 不开) / 脱敏可逆性(RedactMode 双模) / NPU 图像(rknn feature) / 审计后端(AuditSink trait) / DSAR(compliance feature)。以 attune 成熟的文本侧实现为迁移基线、移植 kvm 的文档/图像侧能力、把 gateway 的 MITM 收进可选 feature，渐进迁移、保向后兼容，独立仓 git-dep commit-pin 治理，跨项目契约变更走 major bump 三项目协调。最大张力（MITM / 可逆性）已用 feature flag + RedactMode 双模解决，不强迫任一项目接受不需要的形态。
