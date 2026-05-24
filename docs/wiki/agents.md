# Attune Agents（v1.0）

> v1.0 GA 投产：**20 个 agent**
> - law-pro：11 个确定性 agent + 3 个 LLM extractor = 14
> - OSS 内置：4 个 AI 批注 agent + document classifier = 5
> - Office helper：OCR + ASR = 1 套（2 个入口）

所有 agent 通过三阶段 [Reliability Framework](architecture.md#reliability-framework-v10)
强制门控（Phase 1 F1=1.00 / Phase 2 six-class floor / Phase 3 LLM F1 ≥ 0.85）。

---

## law-pro Agent（需装 Attune Pro / law-pro 插件包）

### 确定性 Agent（11 个）

纯规则 / 公式执行，零 LLM 调用，CPU ≤ 5s。

| # | ID | 描述 | 案件类型 |
|---|-----|------|---------|
| 1 | `civil_loan_agent` | 民事借贷本息合规计算 — 《民法典》利率上限公式严格执行 | civil-loan |
| 2 | `bank_aggregator_agent` | 银行流水聚合 — 结构化交易 + 跨证据交叉验证 | civil-loan |
| 3 | `limitation_agent` | 诉讼时效检查 — 日期算术，中断事由交律师判断 | civil-loan |
| 4 | `evidence_chain_agent` | 证据链关系分析 — 印证/矛盾/缺口，不下法律结论 | civil-loan |
| 5 | `labor_dispute_agent` | 劳动争议经济补偿金/赔偿金 — 《劳动合同法》§47/§87 公式 | labor-dispute |
| 6 | `evidence_classifier` | 证据分类器 — 借条/银行流水/微信记录/收据自动分类 | all |
| 7 | `inheritance_agent` | 法定继承份额计算 — 《民法典》§1127/§1130/§1131 | inheritance |
| 8 | `defamation_agent` | 名誉权/一般侵权损害赔偿计算 — 确定性计算部分 | defamation-tort |
| 9 | `traffic_accident_agent` | 交通事故赔偿 — 人身损害赔偿标准计算 | traffic-accident |
| 10 | `divorce_agent` | 婚姻财产分割 — 共同财产分割规则计算 | divorce |
| 11 | `sale_contract_agent` | 买卖合同违约金计算 — 违约金调整规则 | sale-contract |

### LLM Extractor Agent（3 个）

使用 LLM 从 OCR 文本中抽取结构化事实，带原文依据锚点。Phase 3 F1 = 0.9828。

| # | ID | 描述 | LLM tokens/次 |
|---|-----|------|--------------|
| 1 | `fact_extractor_agent` | grounded 事实抽取 — OCR 文本 → 本息事实（带原文依据） | ~2000 |
| 2 | `housing_rent_agent` | 房租/押金事实抽取 — 租赁合同关键条款提取 | ~1500 |
| 3 | `defamation_extractor_agent` | 名誉权事实抽取 — 侵权行为描述 + 损害后果提取 | ~2000 |

**成本声明**：LLM extractor 仅在用户**显式点击「运行 agent」**时触发，永不后台偷跑（per Attune
[成本感知契约](index.md)）。UI 上每个 agent 按钮旁显示预估 token 用量和费用。

---

## OSS 内置 Agent（需 Attune 基础版，免费）

### AI 批注 Agent（4 个）

对已入库的文档片段触发 AI 分析，结果写入 annotation 表。

| # | ID | 触发方式 | 描述 |
|---|-----|---------|------|
| 1 | `ai_annotation_highlights` | 用户在 Reader 中选中文本 | 提取核心要点，生成高亮批注 |
| 2 | `ai_annotation_questions` | 用户在 Reader 中选中文本 | 从内容生成学习/审阅问题 |
| 3 | `ai_annotation_risk` | 用户在 Reader 中选中文本 | 识别潜在风险点（合同/技术文档） |
| 4 | `ai_annotation_outdated` | 用户在 Reader 中选中文本 | 检测过期信息/日期/版本引用 |

所有批注 agent 遵循成本感知契约：单次调用 LLM，调用前显示 token 预估，结果按 `chunk_hash`
缓存避免重复消费。

### Document Classifier（1 个）

| ID | 触发方式 | 描述 |
|----|---------|------|
| `document_classifier` | 文档入库时自动 | 按 plugin 注册维度分类；OSS 分 general/tech，装了 law-pro 后增加 legal 维度 |

---

## Office Helper（OSS 内置，免费）

Office helper 不是单个 agent，而是两个离线处理管线，通过 `attune-cli` / UI 触发：

| 入口 | 底层引擎 | 描述 |
|------|---------|------|
| `attune ocr` / OCR 面板 | PP-OCRv5 mobile（ONNX Runtime） | 图片/扫描件 → 结构化文本；支持场景分类 + 2 列重排 |
| `attune transcribe` / ASR 面板 | whisper.cpp（medium Q5 ~ large-v3-turbo Q5）| 音频/视频 → 文字转录；中文 WER < 12% |

两者输出均可直接入库（命中 ingest pipeline），转录/识别结果作为新 item 索引到 vault。

---

## Agent 触发成本汇总

| 分类 | 触发策略 | LLM 成本 |
|------|---------|---------|
| 确定性 law-pro agent | 用户显式点击 / chat_trigger 关键词 | 零（纯规则） |
| LLM extractor agent | 用户显式点击 | ~1500-2000 tokens/次 |
| AI 批注 agent | 用户在 Reader 选中文本后点击 | ~500-1000 tokens/次 |
| Document classifier | 文档入库自动触发 | 零（本地模型） |
| Office helper | 用户显式触发 | 零（本地推理） |

详细成本契约：[成本感知与触发契约](https://github.com/qiurui144/attune/blob/develop/CLAUDE.md)
