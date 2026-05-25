# Office 办公助理入口 — 设计文档

**Status**: Approved (brainstorming → spec → writing-plans 下一步)
**Target Release**: v0.7.1 (deadline 2026-05-25)
**Owner**: attune main repo (open-source line, Rust 商用线)
**Scope**: 把 attune-core 已实现的 OCR + ASR 后端能力首次暴露成产品化入口

## 1. 背景与目标

### 1.1 现状

attune-core 已实现：
- **OCR**：PP-OCRv5 mobile（DBNet+CRNN+CLS, ~21 MB ONNX, 中文准确率 94-96%）+ 7 个场景预设
- **ASR**：whisper.cpp subprocess（small Q8 默认, 中文 WER < 20%）+ 说话人分离（pyannote.audio subprocess）

但这些能力**只在 upload pipeline 自动触发**——用户拖文件入 vault 时被动调用。
**没有任何主动入口**让用户把 OCR/ASR 当作"工具"使用（结构化抽取 / 转写后不入库）。

attune-server 只暴露了 `/api/v1/ocr/profiles`（场景预设 CRUD），**没有 OCR/ASR 主动调用 API**。

### 1.2 目标

定位为**办公助理**入口：
- 结构化 OCR（5 个常用场景 schema）
- 会议语音转写（转写 + 时间戳 + 说话人分离）
- **稳定性极高**（L1 准入门 + L2 加强测试）
- 结果**不自动入 vault**（用户显式 Save 才入），保持工具属性

模型已默认部署（v0.6.x 起 PP-OCRv5 + whisper.cpp 已捆绑），本次工作**不引入新模型**，只暴露入口 + 加结构化抽取 + 加稳定性保障。

### 1.3 边界（不做的事）

- ❌ 引入 PP-Structure 或更重的 OCR 模型（避免 200 MB+ 额外占用）
- ❌ 用 LLM 做字段抽取（违反 CLAUDE.md "成本感知契约"——OCR 必须在零成本/本地算力档）
- ❌ 会议章节切割（chapter detection）→ v0.8
- ❌ 关键决议 / Action Item 抽取 → 依赖 LLM，v0.8 或更晚
- ❌ 24h soak test / OOM 注入 / 灾难注入（L3）→ 下一阶段任务
- ❌ macOS 适配（per CLAUDE.md 平台优先级 Windows P0 + Linux P1, macOS 暂不做）

## 2. 架构总览

```
┌─ Chrome 扩展（不变）
│
├─ Web UI (attune-server embedded SPA)
│   └─ OfficeView (新 tab)
│       ├─ 📷 OCR Panel：拖文件 → 选 profile → 显示结构化结果 + bbox 叠加
│       └─ 🎙️ Transcribe Panel：拖音频 → 选模型 → WS 实时进度 → 转写文本 + 说话人色块
│
├─ REST API + WebSocket (attune-server)
│   ├─ POST /api/v1/office/ocr        (sync, multipart, 不限大小, 软警告)
│   ├─ POST /api/v1/office/transcribe (async, 返 job_id)
│   ├─ GET  /api/v1/office/jobs/{id}  (轮询备份)
│   ├─ DELETE /api/v1/office/jobs/{id} (取消)
│   └─ WS   /api/v1/office/jobs/ws    (实时进度推送)
│
├─ Core (attune-core)
│   ├─ ocr/structured/ (新增)
│   │   ├─ mod.rs (SceneExtractor trait + 公共辅助)
│   │   ├─ scene_document.rs
│   │   ├─ scene_receipt.rs
│   │   ├─ scene_table.rs
│   │   ├─ scene_card.rs
│   │   ├─ scene_id_card.rs (3 子类型: id_card_cn / bank_card / business_license)
│   │   └─ normalize.rs (date / amount / Luhn / GB11643)
│   ├─ ocr (已实现) — 复用 extract_text_from_pdf / PP-OCRv5 line + bbox 输出
│   ├─ asr (已实现) — 复用 transcribe_with_diarization
│   └─ office_job_queue.rs (新增) — 内存 in-flight job state machine
│
└─ CLI (attune-cli)
    ├─ attune ocr <file> [--profile receipt] [--json]
    └─ attune transcribe <audio> [--model small|medium|large-v3-turbo] [--wait]
```

**核心原则**：
- 所有调用零 LLM（字段抽取走规则 + 正则 + bbox 邻近 + 校验函数）
- 异步 job 状态在内存（不持久化）—— 服务重启所有 in-flight job 标 cancelled
- 结果不入 vault（除非用户显式 Save）

## 3. 数据契约（REST + WS）

### 3.1 OCR 同步端点

**`POST /api/v1/office/ocr`** (multipart/form-data)

请求字段：
| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file` | binary | yes | 文件，不限大小（>50 MB 软警告） |
| `profile` | string | yes | `document` \| `receipt` \| `table` \| `card` \| `id_card` \| `screenshot` \| `ancient` \| `form` \| `contract` |
| `id_card_subtype` | string | profile=id_card 时必填 | `id_card_cn` \| `bank_card` \| `business_license` |
| `return_bbox` | bool | optional, default true | false 时省 bbox 节省载荷 |

成功响应 `200 OK`：

```json
{
  "envelope_version": "1",
  "profile": "receipt",
  "elapsed_ms": 1843,
  "engine": "ppocrv5-mobile",
  "lines": [
    {"text": "增值税电子普通发票", "bbox": [120, 30, 480, 60], "confidence": 0.98}
  ],
  "structured": {
    "schema": "receipt_v1",
    "fields": {
      "invoice_no":     {"value": "12345678", "confidence": 0.96, "bbox": [500, 80, 700, 110], "source_line_idx": 2},
      "issue_date":     {"value": "2026-05-18", "confidence": 0.94, "bbox": [500, 120, 700, 150], "source_line_idx": 3},
      "seller":         {"value": "ABC 公司", "confidence": 0.91, "bbox": [...], "source_line_idx": 5},
      "buyer":          {"value": null, "confidence": 0.0, "bbox": null, "source_line_idx": null},
      "amount_total":   {"value": "1234.56", "confidence": 0.97, "bbox": [...], "source_line_idx": 11},
      "tax_amount":     {"value": "111.11", "confidence": 0.93, "bbox": [...], "source_line_idx": 10},
      "amount_chinese": {"value": "壹仟贰佰叁拾肆元伍角陆分", "confidence": 0.89, "bbox": [...], "source_line_idx": 12}
    },
    "unrecognized_fields": ["buyer"],
    "validation_warnings": []
  }
}
```

错误响应 `4xx/5xx`：
```json
{"error": "PDF 受密码保护", "code": "pdf-parse-failed"}
```

错误码枚举（kebab）：
- `invalid-input` (400) / `unsupported-format` (400) / `empty-file` (400) / `id-card-subtype-required` (400) / `profile-not-found` (404)
- `pdf-parse-failed` (500) / `ocr-engine-failed` (500) / `internal-error` (500)

### 3.2 ASR 异步端点

**`POST /api/v1/office/transcribe`**

请求（multipart 或 JSON）：
```json
{
  "file_path": "/path/to/audio.mp3",
  "language": "auto",
  "model": "small",
  "diarization": true,
  "max_speakers": 4
}
```

立即响应 `202 Accepted`：
```json
{"job_id": "ocr-job-7f3a2c", "ws_url": "/api/v1/office/jobs/ws?job_id=ocr-job-7f3a2c"}
```

**`GET /api/v1/office/jobs/{job_id}`**：
```json
{
  "job_id": "ocr-job-7f3a2c",
  "state": "running",
  "queue_position": 0,
  "progress": 0.42,
  "stage": "transcribing",
  "elapsed_ms": 18432,
  "eta_ms": 25600,
  "result": null,
  "error": null,
  "warnings": []
}
```

`state` enum: `queued` / `running` / `done` / `failed` / `cancelled`
`stage` enum: `queued` / `loading_model` / `transcribing` / `diarizing` / `postprocess`

完成时 `result`：
```json
{
  "model": "small",
  "language_detected": "zh",
  "duration_sec": 1843.5,
  "segments": [
    {"start_sec": 0.0, "end_sec": 4.32, "text": "大家下午好...", "speaker": "SPEAKER_00", "confidence": 0.91}
  ],
  "speakers": [
    {"id": "SPEAKER_00", "total_sec": 612.4, "segment_count": 47}
  ],
  "full_text": "...",
  "diarization_used": true
}
```

**`DELETE /api/v1/office/jobs/{job_id}`** — SIGTERM 子进程 + 清临时文件 → 204
- 已 done → 409 `job-already-completed`
- 已 cancelled → 409 `job-already-cancelled`

### 3.3 WebSocket 进度推送

**`WS /api/v1/office/jobs/ws?job_id=<id>`** (JSON Lines)

Server → Client：
```json
{"type": "progress", "job_id": "...", "state": "queued", "queue_position": 3, "stage": "queued"}
{"type": "progress", "job_id": "...", "state": "running", "queue_position": 0, "stage": "transcribing", "progress": 0.42, "elapsed_ms": 18432}
{"type": "done",     "job_id": "...", "result": {...}}
{"type": "failed",   "job_id": "...", "error": {"message": "...", "code": "asr-engine-failed"}}
{"type": "cancelled","job_id": "..."}
```

Client → Server (取消)：
```json
{"type": "cancel", "job_id": "..."}
```

### 3.4 资源 / 排队语义（个人助手）

**不限并发，不 reject，排队处理**：

| 资源 | 策略 | 行为 |
|------|------|------|
| OCR 文件大小 | 不限，>50 MB 软警告 | 进队列照常处理 |
| ASR 文件大小 | 不限，>500 MB 软警告 | 进队列照常处理 |
| 并发 OCR | 全局信号量 = CPU 核数 × 0.5 | 超过 → state=queued FIFO 出队 |
| 并发 ASR | 全局信号量 = 2（whisper-cli 已多线程吃满）| 超过 → state=queued |
| 单 job 超时 | 不限 | 让它跑完 |
| 内存峰值 | 软警告 | 不 kill |

仅保留的硬约束：格式 whitelist、profile 存在性、必填字段。

## 4. 字段抽取规则（B 档每 scene）

### 4.1 通用框架

```rust
pub struct RawLine { pub text: String, pub bbox: BBox, pub confidence: f32 }
pub struct BBox { pub x: u32, pub y: u32, pub w: u32, pub h: u32 }

pub struct FieldValue {
    pub value: Option<String>,
    pub confidence: f32,
    pub bbox: Option<BBox>,
    pub source_line_idx: Option<usize>,
}

pub trait SceneExtractor {
    fn schema_name(&self) -> &'static str;
    fn extract(&self, lines: &[RawLine]) -> StructuredFields;
}
```

公共辅助：
- `find_value_after_anchor(lines, anchor_re, max_lines=2)`
- `find_value_in_same_row(lines, anchor_bbox, x_tolerance=20)`
- `normalize_date(s)` — `2026/05/18` / `2026年5月18日` / `26-5-18` → ISO `2026-05-18`
- `normalize_amount(s)` — 去千分位/全角/货币符号 → `1234.56`
- `validate_phone_cn` / `validate_email` / `validate_id_card_cn_gb11643` / `luhn`

**confidence 计算**：
```
field.confidence = min(
  ocr_line.confidence,
  anchor_match_score,           // exact=1.0 / fuzzy=0.7
  validation_pass ? 1.0 : 0.5
)
```

`confidence < 0.6` → UI 黄色高亮提示手填。

### 4.2 各 scene schema

#### `document_v1`
```typescript
{ schema: 'document_v1', fields: {
  title?: FieldValue,
  blocks: { value: BlockItem[], confidence, ... }
}}
type BlockItem = { type: 'title'|'paragraph'|'list'|'figure_caption'|'footer', text, bbox, order }
```
算法：y 聚类成段落 → x 直方图双栏检测 → 启发式标 block 类型 → 双栏按 left/right 列重排 order。

#### `receipt_v1`
字段：`invoice_no, issue_date, seller, buyer, amount_total, tax_amount, amount_chinese`
锚点：
| 字段 | anchor regex |
|------|--------------|
| invoice_no | `发票号码[:：]?` / `号码[:：]` |
| issue_date | `开票日期[:：]?` |
| seller | `销售方` 区块下的"名称"行 |
| buyer | `购买方` 区块下的"名称"行 |
| amount_total | `价税合计` / `(小写)` / `合计金额` |
| tax_amount | `税额` (排除"税率") |
| amount_chinese | `价税合计(大写)` / 大写金额行（含元/角/分 + 壹贰...） |

校验：`issue_date` parse ISO / `amount_total` ≥ 0 / `tax_amount ≤ amount_total` / `amount_chinese` 解析后 ≈ amount_total (误差 < 0.1)。

#### `table_v1`
字段：`headers, rows, row_count, column_count`
算法：
1. y 聚类成逻辑行（y 重叠 ≥ 50%）
2. 行内按 x 排序
3. k-means 聚类所有 cell x-center，k = cell 数中位数 → 列数 N
4. headers = 第一行（字体粗 / 全非数字 / 在最上方）

限制：合并 cell 不支持（PP-OCRv5 mobile 不输出合并标记）。

#### `card_v1`
字段：`name, company, job_title, phone, email, address`
启发式：
- `phone`: 正则 `1[3-9]\d{9}` / 带分隔符 `\d{3,4}[-\s]?\d{7,8}`
- `email`: 标准 regex + 校验
- `job_title`: 关键词 `(CEO|CTO|总监|经理|主任|engineer|manager|director|主管)`
- `name`: 字号最大 + 卡片上半部 + 2-4 汉字 / 2-3 英文词
- `company`: 含 `(有限公司|股份有限公司|集团|Ltd|Inc|Corp|科技|信息)`
- `address`: 含 `(路|街|号|室|楼|区|市|省|Road|Street|Floor)`

#### `id_card_cn_v1` / `bank_card_v1` / `business_license_v1`
用户必须显式指定 `id_card_subtype`（不让 OCR 猜）。
- `id_card_cn_v1`: `name, gender, nationality, birth_date, address, id_number` + GB 11643 校验位
- `bank_card_v1`: `card_number, bank_name, card_type, valid_thru` + Luhn 校验 + BIN 表查银行
- `business_license_v1`: `registration_no, company_name, legal_rep, registered_capital, established_date, scope` + GB 32100-2015 校验位

### 4.3 抽取失败兜底

- 抽不出 → `value: null, confidence: 0.0` + 加入 `unrecognized_fields`
- 校验失败 → `value: "<raw>", confidence × 0.5` + 加入 `validation_warnings`
- OCR 引擎失败 → `structured: null`（A 档 lines 仍返）+ error code `ocr-engine-failed`
- **绝不返编造的 placeholder**（不返 "未识别" / "N/A" 当作 value）

### 4.4 Schema 演进规则（tagged union 路径 Y）

Rust 实现：
```rust
#[derive(Serialize, Deserialize)]
#[serde(tag = "schema", rename_all = "snake_case")]
pub enum StructuredFields {
    DocumentV1        { fields: DocumentFields,        unrecognized_fields: Vec<String>, validation_warnings: Vec<String> },
    ReceiptV1         { fields: ReceiptFields,         unrecognized_fields: Vec<String>, validation_warnings: Vec<String> },
    TableV1           { fields: TableFields,           unrecognized_fields: Vec<String>, validation_warnings: Vec<String> },
    CardV1            { fields: CardFields,            unrecognized_fields: Vec<String>, validation_warnings: Vec<String> },
    IdCardCnV1        { fields: IdCardCnFields,        unrecognized_fields: Vec<String>, validation_warnings: Vec<String> },
    BankCardV1        { fields: BankCardFields,        unrecognized_fields: Vec<String>, validation_warnings: Vec<String> },
    BusinessLicenseV1 { fields: BusinessLicenseFields, unrecognized_fields: Vec<String>, validation_warnings: Vec<String> },
}
```

演进规则：
- 加新字段 (optional) → schema 版本不动（前向兼容）
- 改语义 / 删字段 → `*_v2`，老版本至少保留 2 release
- Client 见未知 `schema` → fallback A 档 (`lines + bbox`)
- 新 scene → 加 enum 变体；老 client 不识别 → fallback A 档

## 5. 准确度 / 速度红线（L1 准入门）

| 指标 | 红线 | 测试集 |
|------|------|--------|
| **OCR document** 字符级准确率 | ≥ 92% | 10 张（双栏论文 / 单栏教程 / 法律文书 / 公司报告 / 学术 PDF）|
| **OCR document** p50 速度 | A4 单页 ≤ 3s | 同上 |
| **OCR receipt** 字段级准确率 | ≥ 92%（10 张 × 7 字段 = 70 个字段 ≥ 65 对）| 增值税普票/专票/电子票/收据/餐饮/滴滴/京东/火车票 |
| **OCR receipt** p50 速度 | ≤ 2s | 同上 |
| **OCR table** cell 级准确率 | ≥ 92% | 财务报表 / Excel 打印 / Word 表格 / 课程表 |
| **OCR table** p50 速度 | A4 单页 ≤ 4s | 同上 |
| **OCR card** 字段级准确率 | ≥ 92%（10 张 × 6 字段 = 60 ≥ 55 对）| 商务/设计师/工程师/中英双语/艺术字体 |
| **OCR card** p50 速度 | ≤ 1.5s | 同上 |
| **OCR id_card_cn** 字段级准确率 | ≥ 95% | 5 张脱敏 |
| **OCR bank_card** 字段级准确率 | ≥ 95% | 5 张脱敏（工/招/建/中信/借记 vs 信用）|
| **OCR business_license** 字段级准确率 | ≥ 95% | 5 张脱敏 |
| **OCR cards** p50 速度 | ≤ 2s | 同上 |
| **OCR p95 速度** | ≤ 1.5 × p50 红线 | 同上 |
| **ASR 中文 WER** | ≤ 15% | AISHELL-3 抽 20 段 |
| **ASR 英文 WER** | ≤ 10% | LibriSpeech test-clean 抽 20 段 |
| **ASR 中英混说 WER** | ≤ 18% | 内部 5 段中英混说技术分享 |
| **ASR DER** | ≤ 25% | 内部 10 段会议录音 (2-4 人, 5-15 分钟) |
| **ASR RTF (small Q8 CPU)** | ≤ 0.5 p50 | 上述全集 |

## 6. 测试矩阵 (L1 + L2)

### 6.1 测试金字塔

```
L2 (商用稳定性):
  · 并发集成 (5 OCR + 2 ASR 同跑)
  · 取消测试 (SIGTERM 子进程 + pgrep 验残留 = 0)
  · 失败恢复 (corrupt PDF / 0 字节 / SIGKILL whisper)
  · 多语种 (中英混说 WER ≤ 18%)

L1 (release gate):
  · 准确度 golden 门 (每 scene ≥ 92% / 卡证 ≥ 95% / WER ≤ 15% / DER ≤ 25%)
  · 速度 golden 门 (p50 + p95)
  · 错误码契约门
  · schema 兼容门

单元测试:
  · 抽取规则 (≥ 5 / scene)
  · normalize_* / luhn / gb11643 各 ≥ 5 edge case

proptest:
  · 5 个不变量 (no-panic / confidence ∈ [0,1] / schema 稳定)
```

### 6.2 Golden 数据集

放 `rust/crates/attune-server/tests/golden/office/`：

```
office/
├── ocr/
│   ├── document/  (10 张 + expected.yaml)
│   ├── receipt/   (10)
│   ├── table/     (10)
│   ├── card/      (10)
│   ├── id_card_cn/    (5)
│   ├── bank_card/     (5)
│   └── business_license/ (5)
├── asr/
│   ├── zh_aishell/  (20 段)
│   ├── en_libri/    (20)
│   ├── zh_en_mixed/ (5)
│   └── meeting/     (10)
└── BASELINE_ENV.md
```

每个 OCR sample `expected.yaml`：
```yaml
id: doc-receipt-01
profile: receipt
schema_version: receipt_v1
expected_fields:
  invoice_no: "12345678"
  ...
expected_lines_count_min: 12
max_elapsed_ms: 2000
reviewer:
  name: REAL_INVOICE_ANONYMIZED
  approved: true
```

### 6.3 测试文件清单

`rust/crates/attune-server/tests/`：
- `office_ocr_golden_gate.rs` (L1 准确度 + 速度)
- `office_asr_golden_gate.rs` (L1 WER + DER + RTF)
- `office_error_contract.rs` (L1 错误码契约)
- `office_schema_compat.rs` (L1 schema 兼容)
- `office_concurrent_test.rs` (L2 并发)
- `office_cancel_test.rs` (L2 取消)
- `office_failure_recovery_test.rs` (L2 失败恢复)
- `office_prop_tests.rs` (L2 proptest)

### 6.4 ENFORCE mode

per CLAUDE.md 验证铁律 + sale_contract 经验，开 `ATTUNE_ENFORCE_OFFICE_FLOOR=1` gate：

| 类别 | 红线 |
|------|------|
| Golden (real approved) | OCR 每 scene ≥ 5；ASR ≥ 10 段累计 |
| Error cases | ≥ 3 |
| Proptest invariants | ≥ 3 |
| Boundary tests | ≥ 5 per scene |
| Integration subprocess | ≥ 1 per scene |
| Concurrent/cancel | ≥ 1 各 |

ENFORCE=1 时跑六类全集，0 violations 才能打 release tag。

## 7. 边界 case 行为

| 边界 | 行为 |
|------|------|
| 0 字节文件 | 400 `empty-file`（不进引擎）|
| 超大文件 | 软警告 + 接受（个人助手）|
| 空 audio（全静音） | 返 `segments: [], full_text: "", warnings: [...]`，不 fail |
| 全黑图 / 全白图 | A 档 `lines: []`，B 档 fields 全 null，不 fail |
| PDF 加密 | 400 `pdf-parse-failed` + message "PDF 受密码保护" |
| PDF 100+ 页 | A 档照常，WS 推页级进度 |
| 音频含 video 流 | whisper.cpp 自动 demux audio（需 ffmpeg）|
| 非 16 kHz 采样率 | whisper.cpp 内部 resample |
| WS client 断开 | server 不取消 job，重连可恢复订阅 |
| Server 重启 | in-flight job 全标 cancelled + warning |
| 同文件并发 5 次 | 各自独立 job，不去重 |
| whisper OOM (exit 137) | failed + message "可能内存不足，请尝试更小模型" |
| id_card subtype 不符（如 subtype=id_card_cn 但实际营业执照） | fields 全返但 confidence 低 + `validation_warnings: ["可能不是身份证，请确认 subtype"]` |
| bank_card Luhn fail | confidence × 0.5 + warning "卡号校验位错误"（不阻塞）|
| table 0 行 | `rows: [], unrecognized_fields: ["table_structure"]` |
| GIF 多帧 | 只 OCR 第一帧 + warning |

## 8. 兜底原则

per CLAUDE.md：

1. **绝不编造字段值**——抽不出 → `value: null`，不返 "未识别" / "N/A"
2. **绝不静默忽略错误**——显式错误码或显式 warning
3. **绝不下"是否正确"的法律 / 业务结论**——Luhn fail 只说"校验位错"不说"假卡"
4. **WS 断开 ≠ 取消**——client 可重连恢复进度
5. **个人助手语义**——宁等不拒，宁返低 confidence 让用户判断也不返 "识别失败"

## 9. 实施计划（6 天）

> wall-clock 诚实声明（per CLAUDE.md）：今天 2026-05-20，离 5/25 release **6 天 wall-clock**。
> 下面"天"指日历日（不是 8h 工时）。

| Day | 日期 | Phase | 交付 |
|-----|------|-------|------|
| **D1** | 5/20 (Tue) | REST 骨架 + A 档输出 | `/office/ocr` + `/office/transcribe` 暴露，所有 scene 先返 A 档（structured=null）。Job queue + WS 框架。CLI 桩。集成测试 happy path。 |
| **D2** | 5/21 (Wed) | B 档抽取 ×5 scene | 5 个 `scene_*.rs` 实现 + 单元测试 ≥ 5 boundary/scene。 |
| **D3** | 5/22 (Thu) | Golden 数据集 + L1 准入门 | 50 OCR + 55 ASR sample 采集 + expected.yaml。golden gate 代码。**第一次完整跑 L1，识别 fail 项**。 |
| **D4** | 5/23 (Fri) | 准确度迭代 + UI | D3 fail 逐个 fix（多半名片 / 双栏）。OfficeView UI 完成。 |
| **D5** | 5/24 (Sat) | L2 + schema 兼容 + 文档 | 并发 / 取消 / 失败恢复 / proptest。schema 兼容。文档。ENFORCE mode 跑通。 |
| **D6** | 5/25 (Sun) | RC → GA | 全链路联调。L1 + L2 全绿确认。develop → main merge。tag `v0.7.1` + `desktop-v0.7.1`。 |

### 9.1 兜底链（任一天卡壳触发）

1. D2 抽取规则做不完 → 名片降级 P1（A 档 only），document 双栏退到全局 y 排序
2. D3 准确度未达 92% → RELEASE.md 显式声明实测值 + target 在 v0.7.2，不延期
3. D4 UI 来不及完整 → 最小 UI：拖拽 + JSON 显示，bbox 叠加放 v0.7.2
4. D5 L2 来不及全做 → 必做并发 + 取消 + proptest no-panic；失败恢复放 v0.7.2
5. D6 任何 gate fail → 发 v0.7.1-rc.1 让用户验，正式 GA 滑下周

**deadline 硬约束（CLAUDE.md 时长约束），scope 可降——不接受为赶 release 关准确度门**。

### 9.2 文件清单

**attune-core (新增)**：
- `src/ocr/structured/mod.rs` (~150 LoC)
- `src/ocr/structured/scene_document.rs` (~250)
- `src/ocr/structured/scene_receipt.rs` (~200)
- `src/ocr/structured/scene_table.rs` (~200)
- `src/ocr/structured/scene_card.rs` (~250)
- `src/ocr/structured/scene_id_card.rs` (~300)
- `src/ocr/structured/normalize.rs` (~200)
- `src/office_job_queue.rs` (~400)

**attune-server (新增)**：
- `src/routes/office.rs` (~500)
- `src/office_state.rs` (~150)

**attune-server (修改)**：
- `src/lib.rs` 注册路由
- `src/state.rs` AppState 加 `office_jobs`
- `ui/src/views/OfficeView.tsx` (新增 ~400)
- `ui/src/views/index.ts` export
- `ui/src/AppShell.tsx` (或导航) 注册 tab
- `ui/src/i18n/zh.ts` + `en.ts` 加 office.* key (per i18n 铁律两边同步)
- `ui/src/hooks/useOfficeJob.ts` (新增) REST + WS 订阅

**attune-cli (新增)**：
- 2 个子命令注册 (`Ocr` / `Transcribe`)

**tests (新增)**：
- 8 个测试文件
- 7 个 OCR scene 目录 + 4 个 ASR 语言/会议目录 (50 OCR + 55 ASR sample)
- `BASELINE_ENV.md`

**docs (修改)**：
- `rust/README.md` Office helper 段
- `rust/DEVELOP.md` OCR/ASR API + 测试运行指南
- `rust/RELEASE.md` v0.7.1 changelog
- 本 spec 文档

### 9.3 风险登记

| 风险 | 概率 | 缓解 |
|------|------|------|
| 名片 92% 字段准确度上不去 | 中-高 | D4 dedicated iterate；D5 早晨仍 fail 触发兜底② |
| document 双栏阅读顺序难 | 中 | A 档准确度只看字符级（OCR 引擎本身），block order 不进 92% 门；P1 best-effort |
| Golden 样本收集慢 | 中 | 公开集自动化 (AISHELL/LibriSpeech/SROIE/FUNSD)；OCR 用公开 + 5-10 内部脱敏 |
| whisper-cli 跨 macOS/Windows | 低 | 重点测 Linux x86_64 (P0)，Windows 软测 |
| UI 来不及完整 | 中 | 兜底③ minimum UI |

### 9.4 GA 验收清单 (D6)

- [ ] `cargo test -p attune-server` 全绿，含 office_* 8 文件
- [ ] `ATTUNE_ENFORCE_OFFICE_FLOOR=1 cargo test -p attune-server` 0 violations
- [ ] L1 所有红线达成
- [ ] L2 并发 5 OCR + 2 ASR 实测 wall-clock 无 panic
- [ ] CLI `attune ocr <real-invoice.pdf>` 输出正确 JSON
- [ ] CLI `attune transcribe <real-meeting.mp3>` 跑通 diarization
- [ ] Web UI 拖拽上传 5 scene 均能渲染
- [ ] WS 进度推送 ≥ 30s 无断连
- [ ] curl 端到端验证（非单元 mock）
- [ ] schema_compat: mock 老 client 收新 server response 不崩
- [ ] README / DEVELOP / RELEASE 文档与代码一致
- [ ] develop → main merge commit 含 v0.7.1 rationale
- [ ] tag `v0.7.1` + `desktop-v0.7.1` 同时打在 main
- [ ] `git push origin develop main v0.7.1 desktop-v0.7.1`

## 10. 后续阶段（v0.8+ 不在本次 scope）

- L3 稳定性：24h soak / OOM 注入 / 灾难注入
- 会议章节切割（chapter detection）
- ASR 关键决议 / Action Item 抽取（依赖 LLM，需先评估成本契约）
- PP-Structure 升级支持表格合并 cell
- 名片 P0 高标杆持续优化
- 多语种 OCR（日语 / 韩语）
- macOS 适配

## 11. 引用

- CLAUDE.md (project) — 双产品线 / Rust 商用线约定 / i18n 铁律 / 文档生命周期协调 / 错误处理 / Lock ordering / async-safe fs
- CLAUDE.md (user global) — 验证优先于编码 / 时间表述诚信 / Playwright E2E 规则 / 描述-only 铁律
- `attune-core/src/asr.rs` 991 LoC — 已实现 ASR backend + diarization
- `attune-core/src/ocr/` 1797 LoC — 已实现 PP-OCRv5 OCR
- `attune-server/src/routes/ocr_profiles.rs` — 现有场景预设 CRUD（不动）
- `docs/superpowers/specs/2026-04-25-industry-attune-design.md` §6.2 — ASR 集成方案历史背景
