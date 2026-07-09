# Spec: Privacy Layer Enhancement (kvm-info-privacy + kvm-privacy-gateway 借鉴)

> Status: DRAFT (待评审) · Date: 2026-06-20 · Owner: privacy 层增强 agent (INT-2)
> 参考源 (read-only): `qiurui144/kvm-info-privacy`, `qiurui144/kvm-privacy-gateway`
> 关联: `docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md` (v1.0.6 OutboundGate SSOT)
> 关联 task: #124 (INT-2)

---

## 0. TL;DR — 两库能力 vs attune 现状对照表

| 能力 | kvm-info-privacy | kvm-privacy-gateway | attune 现状 | 判定 |
|------|:---:|:---:|------|------|
| 格式化 PII 正则检测 (id/phone/email/bank/...) | ✅ 8 类 | — | ✅ 12 类 + checksum + 边界 | **attune 更强**（避免重复） |
| 校验位 (GB11643 / Luhn / 号段) | ✅ | — | ✅ id checksum + Luhn + IPv6 parse | **平手** |
| 多检测器置信度融合 | ✅ +0.1 fuse | — | ❌ 单层去 overlap，无 confidence 模型 | △ 可借鉴（低优先） |
| **文档文件级检测**（PDF/DOCX/XLSX 解析→bbox） | ✅ | — | ❌ 仅 OCR 提文本，无 entity bbox/redact | **差距 (G1)** |
| **文件字节级脱敏**（PDF 内容流 / OOXML XML 替换） | ✅ text_replace | — | ❌ 只在 LLM 出网点 redact 文本 prompt | **差距 (G2)** |
| **图像黑框遮罩 / 人脸检测** | ✅ image_mask + RKNN face | — | ❌ 无（VLM 仅理解，不脱敏） | **差距 (G3)** |
| **保密关键词文档拦截**（fail-closed block） | ✅ Aho-Corasick + 403 | — | ❌ 无"机密文档禁止出网"短路 | **差距 (G4，最高价值)** |
| **信息分级 / classification** | ✅ normal/sensitive_partial/classified | mode off/audit/redact | △ L0/L1/L3 PrivacyTier (per-event 标签，非文档分级) | **差距 (G5)** |
| 可逆 placeholder redact + restore | ❌ (不可逆遮罩) | — | ✅ L1/L2/L3 全可逆 + 同值同标签 | **attune 独有** |
| 统一出网 gate (6 出网点) | ❌ | △ proxy 拦 AI 域名 | ✅ OutboundGate (llm/cloud/webdav/search/telemetry/embedding) | **attune 更强** |
| **出网透明代理 (MITM 拦浏览器上传)** | — | ✅ hudsucker + CA | ❌ attune 内置 Chat，不拦浏览器 | △ 借鉴/暂不做 (见 §2) |
| **出网审计日志** | — | ✅ MariaDB + KVM event | ✅ outbound_audit (本地 SQLite, 0 原文) | **attune 更强（隐私优先）** |
| L4.5 弱模型兜底 (schema/retry/few-shot) | ❌ | △ RKLLM verifier (实验) | ✅ llm_chat_redacted_hardened | **attune 独有** |

**结论一句话**：attune 的**出网文本脱敏链 + 统一 gate + 审计**已优于 KVM；KVM 的杠杆在 attune **完全缺失的"文档文件级隐私"**——把一份 PDF/DOCX/图片在**入库**与**导出/出网**前做 entity 级检测、字节级遮罩、机密文档拦截、文档分级。本 spec 以**移植 kvm-info-privacy 的检测+遮罩模块（Rust 同栈直接搬）**为主线，**借鉴 gateway 的 mode/分级/审计 schema 设计**，**不引入 MITM 代理**（与 attune"内置 Chat 不注入浏览器"产品决策冲突）。

---

## 1. 目标定位

**用户痛点**：attune 今天只在「文本 prompt 出网到 LLM」这一刻做 PII 脱敏。但用户的真实资产是**文档文件**——合同 PDF、客户名单 XLSX、含身份证照片的扫描件。这些文件：
1. **入库**时原文（含 PII）落 vault（加密，OK），但**导出 / WebDAV 同步 / 分享**时是**原始文件字节**出网，绕过了文本 redactor。
2. **机密文档**（标"绝密/内部/Confidential"）今天没有任何"禁止出网"短路——L0 tier 只在 per-chunk 标签上拦，文档整体无 block。
3. **图片中的 PII**（身份证照片、含人脸的截图）今天 VLM 只"理解"不"脱敏"，原图照样可被导出。

**与产品 positioning 对齐**（CLAUDE.md §成本契约 + 隐私优先）：attune = "降低 token + **数据安全**"。文档级隐私是"数据安全"叙事的最后一块拼图——让用户能**安全导出/分享一份脱敏后的文档**，而非只保护 chat。零行业绑定（OSS 通用），律师/医生/HR 等行业增强走 attune-pro 注册 `PiiExtractor`（已有扩展点）。

**北极星自查**：(a) 服务北极星？✅ 数据安全。(b) 追学术指标？否——复用现成 KVM 代码。(c) 偏离硬件约束？否——纯 CPU 检测/遮罩可跑（RKNN face 是可选 feature，local scheduler/RK3588 才启用）。

---

## 2. 范围边界

### 做 (v1.x)
- **S1 文档解析 + entity 级检测**：移植 kvm-info-privacy `parsers/` (PDF/DOCX/XLSX/image-text) + `models.rs` (Entity/bbox/DetectionReport)，产出文件内 PII entity 列表（含 bbox / page / layer）。
- **S2 文件字节级脱敏**：移植 `redactors/text.rs` (PDF 内容流等长替换 + OOXML `<w:t>`/sharedStrings 替换)；产出脱敏后文件。复用 attune 已有可逆 placeholder（**改进 KVM**：KVM 是不可逆黑块，attune 走 `[KIND_N]` 可逆 token，导出场景需要可逆性）。
- **S3 机密文档拦截 (G4)**：移植 `classifier.rs` Aho-Corasick 关键词 + 文档分级 (normal/sensitive/classified)，接入 OutboundGate——`Classified` 文档在导出/出网点 **fail-closed block**。
- **S4 文档分级 → OutboundGate**：classification 映射到现有 PrivacyTier 决策（classified → 视同 L0 永不出网 / sensitive → 强制脱敏）。
- **S5 图像脱敏 (G3，可选 feature)**：移植 `redactors/image_red.rs` 黑框遮罩 + `face_det.rs`（`#[cfg(feature="rknn")]`，local scheduler/RK3588 才编）。x86 降级为 regex-only（无图像 PII）。
- **S6 检测置信度融合 (借鉴)**：把 KVM 的 `fuse_confidence` 思路并入 attune `dedupe_overlaps`（低优先，nice-to-have）。

### 不做 (写死，scope creep 禁止)
- ❌ **MITM HTTPS 透明代理**（kvm-privacy-gateway 全部）。理由：attune 产品决策"内置 Chat，不向浏览器注入 / 不装 CA / 不拦 web 流量"（CLAUDE.md 产品决策记录）。gateway 的 mode/audit-schema **设计**可借鉴，**进程形态不引入**。
- ❌ 新增独立微服务进程 / 新端口（KVM 是 `:8000` 独立 axum 服务）。attune 走 **进程内 crate**（`attune-core::doc_privacy` 模块），不起新服务。
- ❌ MariaDB 审计（KVM gateway 用）。attune 用现有 `outbound_audit` SQLite 表。
- ❌ 中国大陆专属 PII 之外的新语言 NER（KVM 仅中文姓名/地址词典）；多语言留 v.next。
- ❌ 自动后台对全 vault 批量脱敏（违反成本契约第二层——分析阶段等用户开口）。

### 后续 (v.next)
- 视觉 PII（人脸/证件框）在 x86 上的纯 CPU 检测（现仅 RKNN）。
- 文档分级的 LLM 辅助判定（现仅关键词）。

---

## 3. 架构数据流

```
                    ┌─────────────── 入库路径 (建库阶段, 零成本~本地算力) ───────────────┐
 文件 upload ──► parser (PDF/DOCX/XLSX/img) ──► DocPrivacyScanner
                                                   │  ├─ RegexDetector (复用 pii::patterns, 12 类)
                                                   │  ├─ NerDetector   (复用 pii::ner 中文姓名/地址)
                                                   │  ├─ [feat rknn] FaceDetector (local scheduler/RK3588)
                                                   │  └─ Classifier (Aho-Corasick 机密词)
                                                   ▼
                                          DetectionReport { classification, entities[bbox/page/layer], blocked }
                                                   │
                            存 vault (原文加密) + 存 doc_privacy_meta(item_id → report, 0 原文)
                                                   │
        ┌──────────────────────── 出网/导出路径 (用户显式触发) ──────────────────────────┐
        ▼                                                                                  ▼
   导出文件 / WebDAV 同步                                                          chat / doc-intel LLM
        │                                                                                  │
   DocRedactor.redact(file, report, tier)                                    (已有) RedactingLlmProvider
        │  ├─ classification==Classified ─► OutboundGate block (L0CloudBlocked 类比)        │ 文本 placeholder redact
        │  ├─ text_replace: PDF 内容流 / OOXML 字节替换 → [KIND_N] 可逆 token               │
        │  └─ [feat] image_mask: 黑框 / 人脸遮罩                                            ▼
        ▼                                                                          OutboundGate.enforce
   脱敏后文件 (出网) ──────────────────► OutboundGate.enforce (kind=Webdav/CloudSaas)
                                              └─ 复用现有 6-clause 契约 + 写 outbound_audit
```

**DB tables**：
- 新增 `doc_privacy_meta(item_id INTEGER PK, classification TEXT, entity_count INTEGER, blocked INTEGER, report_json BLOB, scanned_ts_ms INTEGER)` —— `report_json` 存 DetectionReport **去原文版**（entity 只存 kind/bbox/page/count，**不存 value**，per §1.4 + audit.rs 0-原文铁律）。vault 加密表。
- 复用 `outbound_audit`（文档导出/拦截事件落此，privacy_tier 映射 classification）。

**cache**：DetectionReport 按 `content_hash` 缓存（复用 v0.7 content_hash 短路约定）——文件未变不重扫。

---

## 4. 模块边界

| 层 | crate / module | 改动 | 来源 |
|----|------|------|------|
| 检测/遮罩核心 | `attune-core/src/doc_privacy/` (新) | 新建：`mod.rs` / `parser.rs` / `detector.rs` / `classifier.rs` / `redactor.rs` / `models.rs` | **移植** kvm-info-privacy `src/{parsers,detectors,classifier,redactors,models}` |
| 复用文本检测 | `attune-core/src/pii/{patterns,ner}.rs` | 0 改动，被 doc_privacy detector 复用 | 现有 |
| 出网拦截 | `attune-core/src/outbound_gate.rs` | +1 变体：classification→block 决策（薄接线，不改 6-clause 核心） | 现有 |
| 持久化 | `attune-core/src/store/` | +`doc_privacy_meta` 表 + CRUD | 现有 pattern |
| 路由 | `attune-server/src/routes/privacy.rs` | +`GET /doc-privacy/report/:item_id` + `POST /doc-privacy/scan` | 现有 + 借鉴 KVM `/analyze` |
| 导出接线 | `attune-server/src/routes/{documents,ingest_webdav}.rs` | 导出前调 DocRedactor + gate | 现有 |
| 图像 (可选) | `attune-core/src/doc_privacy/image.rs` `#[cfg(feature="rknn")]` | 移植 `image_red.rs`+`face_det.rs` | KVM rknn feature |

**跨仓边界**：attune-pro 行业插件通过现有 `PiiExtractor` trait + (新) `register_doc_classifier_keywords()` 注入行业机密词（律所"案卷密"/医院"病历"），doc_privacy 不含任何行业绑定（OSS 边界）。

---

## 5. API 契约

### REST (attune-server)
```
POST /api/v1/doc-privacy/scan            # multipart file → DetectionReport (借鉴 KVM /analyze)
  resp: { classification, blocked, block_reason?, entities:[{kind,page,bbox,layer,count}], summary }
        # 注意：entities 不返回 value（隐私优先）；UI 只显示"第3页 2 处手机号"

POST /api/v1/doc-privacy/redact          # multipart file + {redact_kinds?, reversible:bool} → 脱敏文件
  resp: 二进制下载 + header x-redaction-report: {redacted:{phone:2}, strategy, reversible}
        # fail-closed: redact_kinds 空 → 脱敏全部已知类（沿用 KVM fail-closed 默认）
        # classified 文档 → 403 (沿用 KVM)

GET  /api/v1/doc-privacy/report/:item_id # 已入库文档的缓存 report
```

### Rust API (attune-core)
```rust
pub struct DocPrivacyScanner { /* regex + ner + classifier (+ rknn) */ }
impl DocPrivacyScanner {
    pub fn analyze(&self, path: &Path) -> Result<DetectionReport>;
}
pub struct DocRedactor;
impl DocRedactor {
    /// reversible=true → [KIND_N] 可逆 token (attune 改进, 导出可还原)；
    /// false → 黑块不可逆 (分享场景, 类 KVM)
    pub fn redact(&self, path:&Path, report:&DetectionReport, kinds:&[PiiKind], reversible:bool, out:&Path) -> Result<RedactionReport>;
}
```
**契约不变性**：`classification` enum 值 `normal|sensitive_partial|classified` 沿用 KVM snake_case（便于复用其测试 fixture）；`entities[].bbox = [f64;4]` 顺序固定。

---

## 6. 扩展点 / 插件接口

- **行业机密词**：attune-pro 插件 `plugin.yaml` 增 `confidential_keywords: [...]`，`PluginRegistry` 聚合注入 `Classifier`（复用现有 `all_pii_patterns` 聚合模式）。
- **行业 PII extractor**：已有 `PiiExtractor` trait 直接被 doc_privacy detector 复用（律所案号、病历号）。
- **新文件格式**：`parser.rs` 的 `Parser::for_file` dispatch 表加分支（复用 KVM 结构）。
- **图像后端**：`#[cfg(feature="rknn")]` 边界让 local scheduler/RK3588 启用 NPU face/OCR，x86/CI 默认关闭——**禁止设 default**（per KVM CLAUDE.md 踩坑）。

---

## 7. 错误 + 边界 case

| 场景 | 行为 | 错误码 (kebab) |
|------|------|------|
| classified 文档请求 redact/export | fail-closed block | `403 doc-classified` |
| redact_kinds 为空 | 脱敏**全部**已知类（fail-closed，非 pass-through） | — (沿用 KVM 修复) |
| PDF 加密/损坏 | 解析错误，不静默放行 | `422 doc-parse-failed` |
| rknn 模型缺失 (x86) | regex-only 降级 + `warn` log（不崩溃） | — (沿用 KVM 三态降级) |
| 文件超限 (>N MB) | 413 拒绝（借鉴 gateway `max_upload_size`） | `413 file-too-large` |
| bbox 越界 / 0 entity | redact 退化为复制原文（KVM `copy_file` 语义） | — |
| 出网点 classification 未知 | fail-closed 视同 classified | — |

**Graceful degradation**：检测器任一层失败（NER 词典缺/rknn 缺）→ 降级到可用层 + warn，**绝不**因单层失败而放行未脱敏文件。

---

## 8. 成本契约

| 阶段 | 层级 | 触发 |
|------|------|------|
| 入库扫描 (parser + regex + ner + classifier) | 🆓 零成本 (CPU 毫秒~秒级) | 建库阶段自动跑（顶栏"暂停后台任务"可停），结果缓存 |
| 图像/人脸检测 (rknn) | ⚡ 本地算力 (NPU 秒级) | 仅图像文件 + local scheduler/RK3588，建库自动 |
| 文档分级 LLM 辅助 (v.next) | 💰 时间/金钱 | **不做**（v1.x 仅关键词，零 LLM） |

**绝不**把文档脱敏升级到 LLM 层后台偷跑（per 成本契约第二层）。UI：导出对话框显示"本文档检测到 N 处 PII / 分级=机密，导出将脱敏"——本地、零费用、即时。

---

## 9. 测试矩阵

| 类 | 下限 | 内容 |
|----|------|------|
| Golden | ≥10 fixture | 移植 KVM 测试 fixture（fake PII 文档）+ attune 真实语料（CS-Notes PDF 等） |
| 属性 | ≥3 proptest | redact 后回喂 scan → entity 为空（KVM Layer-1 三重验证）；可逆 round-trip；任意 bytes 不 panic |
| 边界 | ≥5 | 加密 PDF / 空文件 / 超大 / 0-entity / bbox 越界 |
| 异常 | ≥3 | classified block / parse-fail / kinds 空=全脱 |
| 集成 E2E | ≥1 | upload → scan → 缓存 → export → gate block (classified) |
| 回归 | 每 bug +1 | KVM "MD5 不同 ≠ 脱敏正确" 反模式必测：脱敏文件回喂 analyze entity 必空 |
| adversarial | — | ZIP 炸弹 / OOXML 路径穿越 / XML 实体（per 文档智能维度矩阵，office 格式对抗面 P0 安全） |

**核心不变量（移植 KVM 三重验证）**：脱敏输出回喂 `analyze`，`entities` 必为空；MD5 不同不作为脱敏证据。

---

## 10. 向后兼容

- **新表 `doc_privacy_meta`**：纯增量，老 vault 无此表 → lazy create（复用现有 migration pattern）。老文档 report 缺失 → 首次导出时 on-demand scan。
- **OutboundGate**：classification 决策是**新增分支**，`contains_l0` / 现有 6-clause 行为不变；未标 classification 的 payload 走老路径（regression-safe）。已有 `egress_guard.rs` / `pii_chat_path_redact_test.rs` 等 0 改动应仍 PASS。
- **API**：全新端点，无既有 client 依赖。
- **feature flag**：`rknn` 默认关，x86/CI 编译矩阵不变。

---

## 11. 风险登记

| 风险 | 等级 | 缓解 |
|------|------|------|
| 移植 KVM 代码引入其依赖 (lopdf/calamine/zip/quick-xml) 增大二进制 + 跨平台风险 | 中 | 这些是纯 Rust（calamine/zip/quick-xml）；lopdf 纯 Rust；attune 已有 zip 依赖（OOXML 导出）。CI 跨平台矩阵验证 |
| KVM redact 是**不可逆黑块**，attune 导出需**可逆**——直接搬会丢可逆性 | 高 | DocRedactor 默认走 attune `[KIND_N]` 可逆 token（复用 pii::Redactor）；不可逆黑块仅"对外分享"显式选项 |
| PDF 内容流字节级等长替换可能破坏复杂 PDF (字体/编码) | 中 | 沿用 KVM `change_page_content` + 回喂验证；失败则降级"整页遮罩"或拒绝导出（fail-closed），不输出半脱敏文件 |
| classification 关键词误判（"内部资料"普通文档被 block） | 中 | 关键词可配置 + UI 给"我确认非机密，仍导出"二次确认（非静默 block）；行业词走 pro 插件不污染 OSS |
| OutboundGate 接线遗漏某导出点（如分享链接）→ 绕过 | 高 | 复用"所有出网点必经 gate"铁律 + `egress_guard.rs` 全出网点测试扩展覆盖文件导出 |
| rknn feature 误设 default 致 x86 CI 编译失败 | 低 | per KVM CLAUDE.md 踩坑，明确 `default=[]` + CI 断言 |
| 图像 PII 在 x86 无 NPU 完全检测不到（漏脱敏假象） | 中 | UI 明示"当前硬件不检测图像内 PII"（不给用户安全假象，per §4.5 telemetry 思路） |

---

## 落地切片 (per §7.1 版本拆解)

| 切片 | 主题 | 关键交付 | tag 位置 | blockedBy |
|------|------|---------|---------|-----------|
| **DP.1** | 检测核心移植 | `doc_privacy/{models,parser,detector,classifier}` + 复用 pii::patterns/ner + golden | develop | — |
| **DP.2** | 字节级脱敏 (可逆) | `doc_privacy/redactor.rs` (PDF/OOXML) + 三重验证 proptest | develop | DP.1 |
| **DP.3** | 机密拦截 + 分级→gate (G4 最高价值) | classifier→OutboundGate block + `doc_privacy_meta` 表 + 出网点接线 | develop | DP.2 |
| **DP.4** | REST + UI | `/doc-privacy/{scan,redact,report}` + 导出对话框成本提示 | develop | DP.3 |
| **DP.5** (可选) | 图像脱敏 | `#[cfg(feature=rknn)]` image_mask + face (local scheduler/RK3588 实测) | develop | DP.2 |

**复用方式决策**：**移植为主**（kvm-info-privacy 的 parser/detector/classifier/redactor/models 五模块直接搬进 `attune-core/src/doc_privacy/`，Rust 同栈，~1500 行）+ **借鉴设计**（gateway 的 mode/分级/fail-closed/审计 schema 思想）+ **不直接依赖**（不加 git submodule / 不 crate-depend——KVM 是独立服务仓，attune 走进程内模块；且 attune 要改 KVM 的不可逆遮罩为可逆 token，需 fork-and-adapt 而非依赖）。
