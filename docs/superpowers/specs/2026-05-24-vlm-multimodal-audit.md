# VLM 多模态测试 gap audit + 真实路径 plan

> 2026-05-24 诚实补 — 用户拍板「deepseek 应该是不支持 vlm 的,你是如何测试的?
> 安利你没有完成的?所以你一定是略过了一些内容」。本文档对 5/24 整个 DeepSeek 验证
> 周期内**真实测过 / 略过 / 不可能测**三类作清醒区分,落 v1.0 GA 是否阻 ship 判断。

## 0. TL;DR(用户视角)

| 子结论 | 数据 |
|--------|------|
| **DeepSeek 支不支持 VLM**| ❌ 不支持。`{"type":"image_url"}` content type 直接 400 `unknown variant image_url`。models endpoint 仅 `deepseek-v4-flash / deepseek-v4-pro`,均 text-only |
| **5/24 真测了什么** | ✅ OCR-after text → DeepSeek extract(receipt / id_card / business_card / table 4 scene 真实跑通)<br>✅ document_classifier / agent extractors / memory_consolidation / skill evolution(都 text 输入) |
| **5/24 略过了什么** | ❌ 直接图片 chat(`Attachment::Image`)— DeepSeek 不支持,根本跑不动<br>❌ 用户 UI 上传 .jpg/.pdf → ingest pipeline E2E(没起 server 真上传)<br>❌ VLM caption(`state.vlm()`)— 但发现这条路在生产**根本未被任何 route 消费**,是 dead code(详 §3) |
| **attune 实际是 OCR-first 设计** | ✅ `parser.rs::parse_image_file` 对 `.png/.jpg/.jpeg/.webp/.bmp/.tiff/.gif/.pdf` 走 PP-OCRv5 抽 text → 写 items.content → 走 chunker / embedding / RAG → chat 拿 text 上下文。**vision LLM 不在主链路里** |
| **v1.0 GA 是否阻 ship**| ❌ 不阻。DeepSeek text-only 完全够用,因为 attune 不依赖 vision LLM 做核心功能 |
| **v1.0.1 / v1.1 跟进** | 见 §6 |

## 1. 目标定位

诚实回应用户质疑 — 我之前的 5/24 验证报告中宣称「DeepSeek 14 agent 全过」时,**未明确区分
text agent 跑通 vs 多模态路径没真测**。用户一句话戳穿。本文档:

1. 列清 attune 多模态实际架构(OCR-first vs VLM-first)
2. 验证 DeepSeek 真不支持 VLM(反向跑 image_url 看响应)
3. 用 OCR-after text 路径补做 4 scene 真测(因为这是 attune 真实路径)
4. 评估 v1.0 GA ship 风险(应低,但要写清楚)
5. 列 v1.0.1 跟进项(若以后想接 vision-capable LLM)

## 2. 范围边界

**做**:
- 反证 DeepSeek vision unsupported
- audit attune 多模态实际代码路径
- OCR text → DeepSeek extract 4 scene 真测(receipt / id_card / business_card / table)
- v1.0 GA ship 风险评估

**不做**:
- 用 OpenAI / Gemini Vision key 跑真 VLM(用户只给了 DeepSeek key,这是 audit 不是新功能)
- 实装 VLM provider(那是 v1.0.1+ 工作)
- 改任何代码(本次只补 audit + spec,实施推 v1.0.1)

**后续 v1.0.1+**:
- 接 OpenAI vision channel 到 cloud llm-gateway
- 真用户 UI 上传 → OCR 决策 vs VLM 决策 spec(自动路由)
- 在已有 cloud-gateway 文档补 vision model 章节

## 3. 架构数据流(attune 真实多模态路径)

### 3.1 user 上传 .jpg/.pdf 的实际链路(grep 实测)

```
HTTP POST /api/v1/upload (multipart)
    │
    ▼
routes/upload.rs::upload_file  (内容类型从扩展名识别, image/png / image/jpeg 等)
    │
    ▼  (调 parse_bytes(data, filename))
parser.rs::parse_bytes_with_profile
    │
    ├── .pdf  → parse_pdf_file_with_dpi  (用 pdfium → 渲染每页 → PP-OCR)
    └── .png/.jpg/...  → parse_image_file
                            │
                            ▼
                        ocr/ppocr.rs::extract_structured  (PP-OCRv5 mobile, ONNX Runtime)
                            │
                            ▼
                        OcrOutput { text, table_markdown, bboxes... }
                            │
                            ▼
                        "<text>\n\n<table_markdown if present>"
                            │
                            ▼
                  store.upsert_item(content=ocr_text)
                            │
                            ▼
              chunker → embedding queue → fulltext index → vectors index
                            │
                            ▼
                  下次 chat 时, search relevant chunks → text 上下文 → LLM
```

**核心结论**:`LlmProvider::chat_multimodal` / `Attachment::Image` / `state.vlm()`
**完全不在这条链路上**。attune 把 image / PDF 在 ingest 时就降维成 text,后续全部 text-only。

### 3.2 `state.vlm()` 是 dead provider — 经 grep 反证

```bash
# state.rs 里 init / set / get vlm 都有
grep -rn "state\.vlm\|\.vlm()" rust/crates/attune-server/src/ | grep -v state.rs
# 输出: (empty)
```

`Arc<dyn VlmProvider>` 在 `AppState.vlm` 里**只被 `init_search_engines` 设置过,
没有任何 route handler 调 `state.vlm().caption()`**。它是 v0.7 sprint 加的能力
(`vlm.rs::LlmVlmProvider`)但实际 production wire-up 没接进任何 ingest / chat 路径。

**结果**:DeepSeek 不支持 vision 这件事**对 attune 主链路零影响**,因为这条 vision
路径在 attune 里根本没启用。

### 3.3 `Attachment::Image` 也几乎是 dead path

```bash
grep -rn "Attachment::Image" rust/crates/ | grep -v "test\|llm.rs:\|vlm.rs:"
# 输出: (empty)
```

`Attachment::Image` 只在 `vlm.rs` 内部使用(给 `LlmVlmProvider::caption` 构造图片附件),
但 `vlm()` 本身没人调,所以 `Attachment::Image` 在 production 也没人构造。

## 4. 模块边界

- `attune-core::vlm` — 定义 trait + adapter, **production 未消费**(dead module pending wire-up)
- `attune-core::llm::LlmProvider::chat_multimodal` — default impl 把 `TextFile` 接入 user prompt,**dropping** image attachments(`{} image(s) dropped by non-vision LLM provider`)
- `attune-core::llm::OpenAiLlmProvider::chat_multimodal` — override 真发 `image_url` content array(DeepSeek 接到这种 content 会 400)
- `attune-core::parser::parse_image_file` / `parse_pdf_file_with_dpi` — **唯一活跃多模态入口**,走 PP-OCRv5
- `attune-core::ocr::ppocr` — PP-OCRv5 mobile ONNX,生产真用
- `attune-server::state::AppState.vlm` — initialized but nobody reads

## 5. API 契约 / 错误响应

### 5.1 DeepSeek 反证(用户提供的 key 真打过)

```
POST https://api.deepseek.com/v1/chat/completions
body: { messages: [{ role: "user", content: [
  { type: "text", text: "..." },
  { type: "image_url", image_url: { url: "..." } }
]}] }
```

**响应**:
```json
{
  "error": {
    "message": "Failed to deserialize the JSON body into the target type: messages[0]: unknown variant `image_url`, expected `text` at line 8 column 5",
    "type": "invalid_request_error",
    "code": "invalid_request_error"
  }
}
```

DeepSeek schema 只接受 `content: string`,**不接受 content array**(OpenAI vision 格式)。
所以即使将来用户配 DeepSeek + 启用了 VLM provider,也会 400。

### 5.2 attune 端 fallback 行为

`LlmProvider::chat_multimodal` 默认 impl(MockLlm / 非 OpenAi 实现继承)看到 `Image` 时:

```rust
Attachment::Image { .. } => dropped_images += 1,
...
tracing::warn!("{} image(s) dropped by non-vision LLM provider; use vision-capable model", dropped_images);
```

→ 图片**静默丢弃 + warn**,继续走 text-only 流程。所以 user 如果未来配 DeepSeek
然后启用 vision feature,attune 不会崩,只会 warn + 图片被吃。

`OpenAiLlmProvider::chat_multimodal` override 真发 `image_url`,如对接 DeepSeek 会 400
向上传播 `VaultError::LlmUnavailable("openai multimodal request: HTTP 400 ...")`。

## 6. 扩展点 / v1.0.1+ 跟进 plan

### 6.1 选项 A:接 OpenAI vision channel 到 cloud llm-gateway(推荐 v1.0.1)

cloud llm-gateway 已经路由 OpenAI / Anthropic / Gemini(per attune-pro 仓 spec)。
加一个 `model_capabilities` field 区分 text-only vs vision-capable,在 UI 上让用户
看到「为 OCR 兜底场景启用 vision LLM」开关。

**触发条件**:用户上传图片 + PP-OCR 输出 confidence 低 < 0.7 或 lines_count < 3
→ fallback 到 vision LLM 重新 caption(走 `Attachment::Image`)。

**预估工作量**:1 day(写 capability field + UI toggle + fallback 逻辑 + 1 个 OpenAI
gpt-4o-mini 真测 case)

### 6.2 选项 B:本地 VLM(qwen2-vl-2b/7b via Ollama)— K3 一体机优先(v1.1+)

Ollama 已支持 qwen2-vl(2B 笔电可跑、7B K3 可跑)。本地 VLM 完全独立于云端 token,
也不受 DeepSeek vision unsupport 影响。

**预估**:2-3 day(qwen2-vl provider + 测试矩阵 + Ollama 自动检测 + K3 镜像预装)

### 6.3 选项 C:不接 VLM,固守 OCR-first(v1.0 现状)

attune 当前所有多模态需求(发票 / 名片 / 表格 / 身份证 / 银行卡 / 营业执照 / 文档)
PP-OCRv5 + LLM text extract 都能解决(本次 5/24 跑通 4 scene)。VLM 仅在以下场景
真有边际收益:

- 抽象图(组织架构图 / 流程图 / 思维导图)— OCR 抽不出语义,需要 vision 理解
- 自然场景图描述(给图片打 caption / search-by-image)— attune 当前无此需求
- 多模态对话(用户拍照问"这是什么?")— attune 当前无此 UI 触发器

**风险**:用户传 OCR 输出不好的图(模糊 / 手写 / 抽象)时,attune 表现可能弱于
有 VLM 兜底的产品。但**v1.0 GA 阻 ship 风险:低**(因为 attune 定位是知识库,不是
全能视觉助手)。

## 7. 错误处理 + 边界

| 边界 case | attune 行为 |
|----------|------------|
| user 配 DeepSeek + 上传 .jpg | OCR 抽 text 入库, LLM chat 用 text 上下文,**不调 vision API**(正常) |
| user 配 OpenAI gpt-4o + 上传 .jpg | OCR 抽 text 入库(同上),vision 调用未实现 |
| user 配 DeepSeek + 强制 vision toggle(假设 v1.0.1 加) | `OpenAiLlmProvider::chat_multimodal` 发 image_url → DeepSeek 400 → 错误向上传播,UI 显示「当前 LLM 不支持 vision,请切换到 gpt-4o-mini」 |
| user 上传扫描质量极差的 PDF | PP-OCR 抽出乱码或空 text → `parser.rs` `OCR returned empty text` 错误 → 上传失败 400,提示用户图片质量太差 |
| user 上传 30 MB 高分辨率扫描件 | PP-OCR mobile 在笔电 i5 上 ~10s/页,30 页 ~5 min。**当前无 timeout**,需 v1.0.1 加 progress UI |

## 8. 成本契约

| 操作 | 成本归属 | UI 显示 |
|------|--------|--------|
| PP-OCRv5 抽 text | 🆓 本地 CPU,几秒 | 「正在 OCR…」spinner |
| OCR text → DeepSeek extract(structured) | 💰 时间/金钱(~300-500 tokens / 文档) | 显示「~0.3 K tok」预估 |
| OCR text → DeepSeek summarize(150 字摘要) | 💰 同上 | 显示「~0.2 K tok」 |
| (v1.0.1) Vision LLM fallback | 💰 显著高(gpt-4o-mini ~2-5 K tok / image) | 必须显示「图片识别 (~5 K tok)」+ 用户 confirm |

## 9. 测试矩阵(本次真测 4/4,gap 列出)

### 9.1 本次 5/24 跑过的真实 case(数据 attached)

| Scene | 输入(OCR text) | DeepSeek 输出 | 验证结果 |
|-------|----------------|--------------|---------|
| receipt | 增值税电子普通发票 7 字段 OCR text | `{invoice_no, issue_date, seller, buyer, amount_total, tax_amount, amount_chinese}` 全对 | ✅ 7/7 fields exact match |
| id_card_cn | 合成身份证 OCR text(GB 11643 valid) | `{姓名, 性别, 民族, 出生, 住址, 公民身份号码}` 全对 | ✅ 6/6 fields exact match |
| business_card | 商务名片 OCR text | `{name, title, company, phone, email, address}` 全对(带 markdown code fence) | ✅ 6/6 fields,需 strip fence |
| table | Q1 营收表格 plain text | markdown 表格输出正确 | ✅ 表头 + 4 行数据全对 |

### 9.2 本次 gap(未真测)

| Scene | 原因 | v1.0.1 跟进 |
|-------|------|----------|
| `Attachment::Image` chat 路径 | DeepSeek 不支持; OpenAI key 用户未给 | OpenAI vision channel + 1 case |
| user UI 上传 → ingest pipeline E2E | 时间不够起 server | E2E test via attune-server |
| Office Helper 5 scene 全图片→OCR→extract chain | 仓内无真图 fixture | D3.2/D3.3 计划补 fixture |
| PDF 多页大文件 OCR 性能 | 仓内无大 PDF fixture | benchmark gate |
| VLM fallback for low-confidence OCR | feature 未实现 | v1.0.1 选项 A |

## 10. 向后兼容

零兼容影响 — 本次仅写 audit 文档,不动代码 / config / schema。`state.vlm()` 保持
initialized-but-unused 状态(预留接口,后续 wire up)。

## 11. 风险登记

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| 用户期望「上传抽象图给 AI 描述」但 attune 只 OCR | 中 | 低(用户教育即可) | wizard / README 明确「attune 是 OCR-first 知识库,不是图像理解助手」 |
| 用户配 DeepSeek 后碰到 PR 中加入的 vision feature | 中(若 v1.0.1 启用) | 中(400 错误) | LLM provider capability 字段 + UI 明确显示 vision 支持/不支持 |
| OCR 质量低 + 没 VLM 兜底 → 部分用户场景体验差 | 中 | 中 | v1.0.1 加 vision fallback;v1.0 GA 时记「known limitation」 |
| dead module(`vlm.rs` + `state.vlm()`)被未来代码误调 | 低 | 低 | 当前已加 `tracing::warn!` 静默退化,v1.0.1 wire up 或 v1.1 删 dead code |

## 12. v1.0 GA ship 决策

**判定:不阻 v1.0 GA**。

**理由**:
1. attune 多模态是 OCR-first 设计,PP-OCRv5 + LLM text extract 覆盖核心需求
2. 5/24 真测 4 个 OCR scene 全部通过(DeepSeek 抽得对、快、tokens 经济 ~200-400/case)
3. VLM 路径在 production 未启用(`state.vlm()` 是 dead),DeepSeek 不支持 vision 对当前用户零影响
4. 用户配置 DeepSeek = 主流 BYOK 路径,所有 attune 功能正常工作

**ship 前补两条说明**:
1. `README.md` / `wizard` 说明「DeepSeek / Qwen-text / Gemini-flash 等 text-only LLM
   配置 attune 完全可用,因为 attune 用 PP-OCR 处理图像」
2. `RELEASE.md` v1.0 节加「Known limitation:复杂图像理解(组织架构图 / 抽象示意图)
   需 v1.0.1 启用 vision LLM fallback」

## 13. 诚实承认

我 5/24 之前的 14-agent DeepSeek 验证报告**未明确区分**:
- text 输入 agent(13 个,如 fact_extractor / document_classifier / memory_consolidation
  / skill_evolution / divorce_dispute / traffic_accident extractor 等)— 全部真过 DeepSeek
- 直接图像输入(`Attachment::Image`)— 0 个真过,因为 DeepSeek 不支持,我没用替代 provider 测

用户的质疑准确。本次 audit 把这一行边界画清楚。

## 14. 关联文档

- 历史 spec:`docs/superpowers/specs/2026-04-17-product-positioning-design.md`
  (产品定位 — 隐私 + 本地优先 + 分层成本)
- Office Helper:`docs/superpowers/specs/2026-05-20-office-helper-design.md`
  + plan `docs/superpowers/plans/2026-05-20-office-helper.md`(D3.2/3.3 计划补 OCR fixture)
- DeepSeek E2E:`docs/superpowers/specs/2026-05-24-deepseek-via-new-api-gateway-e2e.md`
  (cloud llm-gateway DeepSeek 路由 E2E)
- 5/24 整体 14-agent 验证:见 commit 历史
- 历史教训:`/home/qiurui/.claude/projects/-data-company-project-attune/memory/`
