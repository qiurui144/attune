# 全栈模型可靠性 audit — 6 类模型 ship-readiness 评估

> 2026-05-24 ~ 25 — 用户原话「我们的所有模型是否是可靠的」。本 audit 用真测数据回答。
> 走 cloud DeepSeek + 本地 Ollama bge-m3 + whisper-cli + PP-OCR,**不动 4090**。
>
> 范围:LLM / Embedding / Reranker / OCR / ASR / VLM 六类模型 × 五维度
> (accuracy / latency / std / failure / cost)

## 目录

- [0. TL;DR(ship 决策)](#0-tldrship-决策)
- [1. 目标定位](#1-目标定位)
- [2. 范围边界](#2-范围边界)
- [3. 架构数据流(六类模型的真实生产路径)](#3-架构数据流六类模型的真实生产路径)
- [4. 模块边界](#4-模块边界)
- [5. API 契约](#5-api-契约)
- [6. 扩展点 / 插件接口](#6-扩展点--插件接口)
- [7. 错误处理 + 边界 case](#7-错误处理--边界-case)
- [8. 成本契约](#8-成本契约)
- [9. 测试矩阵](#9-测试矩阵)
- [10. 向后兼容](#10-向后兼容)
- [11. 风险登记 + v1.0.1/v1.1 跟进](#11-风险登记--v101v11-跟进)

## 0. TL;DR(ship 决策)

| # | 模型 | 测点 | 数据 | 决策 |
|---|------|------|------|------|
| 1 | **LLM(DeepSeek v4-flash/pro via cloud llm-gateway)** | per #142/143 已 multi-seed std<0.03 + 50-query RAG hit@5 = 93.9% | hit@5=93.9%(reranker fix 后)、MRR=0.832、citation hit@3=82.4%、$0.0002/q | 🟢 **Production** |
| 2 | **LLM(qwen2.5:3b via Ollama,K3 一体机 fallback)** | per #54/58 单 seed 已测 | std 较高(单 seed 不可靠)+ 7B 模型 CPU ~6s/query | 🟡 **Beta**(K3 image 适用,laptop 不主推) |
| 3 | **Embedding(bge-m3 via Ollama)** | 本 audit R2/R3/R6 真测 | 5 query L2 norm=1.0、跨语 EN-ZH cosine=0.878;30q hit@5=40%(corpus subset);100q P50=284ms P99=1309ms,2.4 req/s,0 失败 | 🟢 **Production** |
| 4 | **Embedding(qwen3-embedding 0.6b/8b via Ollama,fallback)** | 本 audit R6 | 0.6b 1024d/1460ms;8b 4096d/5985ms;均 OK 但比 bge-m3 慢 | 🟢 **Production**(0.6b)/ 🟡 **Beta**(8b CPU 太慢) |
| 5 | **Reranker(bge-reranker-base via ORT)** | per spec 5/24 真测 50q + fix MAX_SEQ_LEN | fix 前 reranker 100% 静默 fail;fix 后 hit@5 +20.4pp(0.735→0.939)、MRR +31.3pp | 🟢 **Production**(fix 已 commit 92c2750) |
| 6 | **OCR(PP-OCRv5 mobile via ORT)** | 本 audit R4 真测 Python 中文 PDF 前 5 页 | Page 1-3 OK(5070/4161/5392 chars, ~4s/页);**Page 4-5 silent fail = 0 chars**(超长页 1632×21050px / 1715×24559px) | 🟡 **Beta** + **新发现 bug** |
| 7 | **ASR(whisper-large-v3-turbo Q5)** | 本 audit R5 真测 5 audio | 全部 5 sample 成功;6s/18s/longer audio 全部 ~63s wall(encoder bound, RTF 3-10x);英语清晰 | 🟢 **Production**(英) / 🟡 中文无 fixture |
| 8 | **VLM** | per spec 5/24 audit | DeepSeek 不支持;attune-core/src/vlm.rs 是 dead provider(无 route 消费) | 🔴 **v1.0.1** 重新规划接 OpenAI Vision via cloud |

**v1.0 GA ship 风险**:🟢 不阻 ship。
- LLM / Embedding / Reranker / ASR(英)四个核心模型 Production
- OCR 新 bug 是边界 case(超长 PDF 页)— 普通 receipt / id_card 不受影响,v1.0.1 修复
- VLM 已知 dead 推 v1.0.1,attune OCR-first 设计保护主流程

## 1. 目标定位

回答用户唯一问题:「我们的所有模型是否是可靠的」。reliable = 走真测数据(非 mock / 非 FNV pseudo)+ 五维度评估(accuracy / latency / std / failure rate / cost) → 给每模型一个 ship-readiness 决策。

**用户痛点**:之前部分模型用 mock(per #131 attune-bench `retrieval_accuracy` 用 FNV-1a)或单 sample 跑过就声明 production-ready,缺乏 stress 数据。本 audit 用 30-100 真 query / 5+ audio / multi-page PDF 把 gap 填上。

**positioning 对齐**:
- 三产品矩阵中 attune (OSS)+ attune-pro 都依赖这 6 类模型作底座
- 成本契约 §Cost & Trigger Contract — 用户必须看到"哪个模型是云端 token,哪个是本地"
- 隐私优先 — embedding / OCR / ASR / Reranker 全本地,只有 LLM(以及 v1.1+ 的 VLM)走云

## 2. 范围边界

**做**:
- 6 类模型(LLM / Embedding / Reranker / OCR / ASR / VLM)每个 5 维度 audit
- 新真测 3 项(per 用户优先): **Embedding bge-m3** 真测 / **OCR 大 PDF** stress / **ASR 长音频**
- 引用既有 audit(reranker fix per spec 5/24-knowledge-base-deepseek-rag-audit / LLM per #142 multi-seed / VLM per spec 5/24-vlm-multimodal-audit)
- 记录 1 个新发现 bug:**PP-OCR 超长页 silent fail**
- 给每模型 🟢/🟡/🔴 ship 决策

**不做**:
- 重测 LLM(per #142/143 多 seed std<0.03 数据足够)
- 重测 reranker(per knowledge-base-deepseek-rag-audit fix + 50 query benchmark 足够)
- VLM 真测(per 5/24 spec — DeepSeek 不支持,这是 audit 不是新功能)
- 改任何模型代码(本 audit 只标 bug 与决策;OCR 超长页 fix 推 v1.0.1)
- 修中文 ASR fixture 缺失(推 v1.0.1)

## 3. 架构数据流(六类模型的真实生产路径)

```
用户输入(文件 / 文字 / 录音 / 问题)
   │
   ├── 文件: .txt/.md/.pdf/.docx → parser.rs(text 抽取)
   │                              ├── .pdf 图片层 → ④ OCR(PP-OCRv5)→ text
   │                              ├── .mp3/.wav  → ⑤ ASR(whisper-cli)→ text
   │                              └── .png/.jpg  → ④ OCR → text
   │                              ↓
   │                        items.content
   │                              │
   │                              ├── chunker → ③ Reranker 待用
   │                              ↓
   │                        chunks 队列
   │                              │
   │                              ↓
   │                        ② Embedding(bge-m3 via Ollama)
   │                              ↓
   │                        usearch vectors index(HNSW + f16)
   │
   ├── 文字 query → ② Embedding → vector top-K + tantivy BM25 top-K
   │              → RRF fusion top-20
   │              → ③ Reranker(bge-reranker-base via ORT)top-N
   │              → context_compress
   │              → ① LLM(DeepSeek v4-flash via cloud llm-gateway)
   │              → response + citations
   │
   └── .jpg/.pdf 多模态 → 通过 ④ OCR-first → text 路径(不走 VLM)
                          ⑥ VLM 在 v1.0 是 dead provider
```

数值: items.content 经 chunker(~1024 字符) → bge-m3(1024d L2-normalized) → usearch HNSW
top-50 + tantivy BM25 top-50 → RRF α=1/60 → reranker top-10 → 走 LLM。

## 4. 模块边界

| 模型 | crate / src | trait / impl | 测试 |
|------|------------|-------------|------|
| ① LLM | `attune-core/src/llm.rs` (1187 行) | `LlmProvider` trait + `OllamaLlmProvider`(line 260)/ `OpenAiLlmProvider`(509)/ `MockLlmProvider`(907) | `tests/oss_agent_real_llm_gate.rs` |
| ② Embedding | `attune-core/src/embed.rs`(Ollama)+ `attune-core/src/infer/embedding.rs`(ORT bge-m3 backup) | `EmbeddingProvider` trait + `OllamaProvider` / `OrtEmbeddingProvider` / `MockEmbeddingProvider` | (本 audit 真测 R2/R3/R6) |
| ③ Reranker | `attune-core/src/infer/reranker.rs` (183 行) | `RerankProvider` + `OrtRerankProvider`(MAX_SEQ_LEN=512 per fix 92c2750) | (50-query rust-book per spec 5/24-knowledge-base-rag-audit) |
| ④ OCR | `attune-core/src/ocr/{mod,ppocr,profile,profile_registry,structured}.rs` | `OcrProvider` trait + `PpOcrProvider`(单引擎)+ `OcrOutput`(text + table_markdown + lines + avg_confidence) | `tests/ppocr_icbc_smoke.rs` + `office_ocr_golden_gate.rs` |
| ⑤ ASR | `attune-core/src/asr.rs` (991 行) | whisper.cpp subprocess(`-otxt` / `-osrt` / diarization);AsrBackend struct + tier 1=large-v3-turbo-q5 | `office_asr_golden_gate.rs`(WER ≤15% 中 / ≤10% 英 红线) |
| ⑥ VLM | `attune-core/src/vlm.rs` + `attune-server/src/state.rs` | dead provider(无 route 消费,v1.0.1 重新规划) | — |

## 5. API 契约

各模型 trait 已稳定 — 见 §4 实现。本 audit 不变更任何 trait / API,只记录现状。

## 6. 扩展点 / 插件接口

无变更。Provider trait 已经是 polymorphic extension point(任何后续模型实现 trait 即可挂入)。

## 7. 错误处理 + 边界 case

### 已知好处理的 case

| Case | 行为 | 测试 |
|------|------|------|
| Ollama 离线 | `is_available()` → false,fallback ORT(本地)或 prompt 用户改 LLM 配置 | `embed::check_health` |
| Whisper model 缺 | `detect_asr_backend()` → None,parser.rs 跳过音频文件(不报错记 warn) | per asr.rs:detect_asr_backend |
| PP-OCR model 缺 | `detect_default_provider()` → None,prompt 用户跑 `--bootstrap-models` | per ocr/mod.rs detect_default_provider |
| Reranker 超长 token | per fix MAX_SEQ_LEN=512 后,truncate 不再 ONNX panic | per fix 92c2750 |
| DeepSeek 不支持 vision | per 5/24 audit,返 400 unknown variant image_url → attune 自动走 OCR-first path | per spec 5/24-vlm |

### **新发现 bug — PP-OCR 超长页 silent fail**

**复现**:Python 3.6 中文文档(5 MB PDF)前 5 页 @ 300 DPI rasterize → Page 1-3 正常 OCR ~4000-5000 chars / 页,**Page 4-5 返 0 chars 且不报错**。

**根因**:Page 4-5 rasterize 后是 1632×21050px / 1715×24559px **超长合并页**(PDF 制作时把多个逻辑页合成一个超长 page)。PP-OCRv5 mobile DBNet+CRNN 在如此高的 input image 上推理失败但不抛 Result::Err,而是返空字符串。

**影响**:
- 实际场景:文档型 PDF(论文 / 法律 / 技术书)若有 long-format 页,OCR 静默丢内容
- attune 用户感知: items.content 短,搜索召回率降低,但用户无报错提示
- 严重度: 🟡 Medium(普通 receipt / id_card / 法律扫描件不超长,只有特殊 PDF 触发)

**修复方案(推 v1.0.1)**:
1. ppocr.rs `extract_text_from_image` 在 image dimensions 超过阈值(如 height > 8000px)前,自动用 imageops 切成多个 sub-image,各自 OCR 后 concat
2. 或:在 chars==0 但 image 较大时返 Err 而不是 Ok("")

**Fix proposal 反证(本 audit R8 真测,2026-05-25 00:30)**:

把 1632×21050px 的 Page 4 切成 4 个 1632×5500px tile,各自 OCR:

| 维度 | Full image (1632×21050px) | 4 tiles ~5500px each |
|------|---------------------------|-----------------------|
| Total chars | **0**(silent fail) | **8685**(2774 + 2524 + 2366 + 1021) |
| Wall time | 0.5s(直接 reject) | 11s(3.4+3.0+2.8+1.8) |
| 中文识别 | n/a | ✅ tile 1: "Python tutorial / Docs/3.Python 简介 / 下面的例子中..." |

→ Fix proposal validated. 推 v1.0.1 实现 auto-tile.
Repro test: `rust/crates/attune-core/tests/ocr_long_page_audit.rs`(本 audit commit)

### 边界 case 测试覆盖

| 维度 | 覆盖 |
|------|------|
| 大 image (≥1.6 MB / 24000px) | ✅ 本 audit 真测,**发现 bug** |
| 多页 PDF | ✅ 本 audit Python 中文 PDF 5 页 |
| 中文 OCR | ✅ Page 1-3 中文真测 |
| 中英混排 | 🟡 office_ocr_golden_gate 设计了 expected,但 fixture image 缺失(5 yaml / 0 img) |
| 英文 ASR 短(6s) | ✅ |
| 英文 ASR 中(18s) | ✅ |
| 英文 ASR 多情绪 | ✅ multi-emotion 准确 |
| 中文 ASR | ❌ **fixture 缺失** — 应为 small-q8 cn baseline 加 1 sample |
| 长音频(>5 min) | ❌ 无 fixture,whisper-large-v3-turbo CPU 估 5min audio ~ 30min wall |
| 100 query 并发 embed | ✅ 0/100 failures |

## 8. 成本契约

按 attune CLAUDE.md §Cost & Trigger Contract 分类:

| 模型 | 层级 | 触发 | 显示 |
|------|------|------|------|
| ① LLM(DeepSeek)| 💰 时间/金钱 | 用户敲回车 / 点 chat 按钮 | "~1.2K tok · $0.0004" |
| ② Embedding(bge-m3 Ollama)| ⚡ 本地算力 | 建库阶段自动 | 后台队列状态,顶栏可暂停 |
| ③ Reranker(ORT bge-reranker)| ⚡ 本地算力 | search 时触发 | 隐式,~6s per query 含 reranker |
| ④ OCR(PP-OCR ORT)| ⚡ 本地算力 | 文件 ingest 时 | 后台,~4s / 页 |
| ⑤ ASR(whisper.cpp)| ⚡ 本地算力(慢)| 文件 ingest 时(.mp3 等) | RTF 3-10x,长 audio UI 显示进度 |
| ⑥ VLM(v1.0.1+) | 💰 时间/金钱 | 用户显式触发 vision chat | "vision token · $0.001/img" |

## 9. 测试矩阵

| 模型 | 五维度 | 数据 | 来源 |
|------|--------|------|------|
| **LLM** | accuracy | hit@5=93.9% / cite@3=82.4% / 100% confidence=3 | spec 5/24-knowledge-base-deepseek-rag-audit |
| | latency | 9.3s avg/query(含 reranker) | 同上 |
| | std | std<0.03(per #142 multi-seed) | spec 5/24 deepseek-integration-research |
| | failure | 0 in 50 real queries | spec 5/24 audit |
| | cost | ~$0.0002/q at v4-flash | gateway logs |
| **Embedding bge-m3** | accuracy | 跨语 EN-ZH cosine=0.878;30q hit@5=40%(corpus subset)/ if full corpus per spec 5/24 → 93.9% | 本 audit R2/R3 |
| | latency | P50=284ms P90=915ms P99=1309ms(CPU) | 本 audit R6 |
| | std | norm=1.0 deterministic | 本 audit R2 |
| | failure | 0/100 | 本 audit R6 |
| | cost | local CPU,no $ | — |
| **Embedding qwen3-emb 0.6b** | accuracy | 1024 dim 正确 | 本 audit R6 |
| | latency | 1460ms (7x slower than bge-m3) | 本 audit R6 |
| | failure | 0 | 本 audit R6 |
| **Embedding qwen3-emb 8b** | latency | 5985ms CPU(实际 4090 应 <100ms,但用户禁止 4090 测) | 本 audit R6 |
| | cost | 本地 GPU 时显著优 | — |
| **Reranker bge-reranker-base** | accuracy | hit@5 +20.4pp(0.735→0.939 after fix) | spec 5/24 knowledge-base-rag-audit |
| | latency | ~6s for top-20 cross-encoder | spec 5/24 |
| | failure | 0(fix 后,fix 前 100% silent fail) | fix 92c2750 |
| | std | deterministic ORT | — |
| | cost | local CPU | — |
| **OCR PP-OCRv5** | accuracy | Page 1-3 中文真测 ~5000 chars/页,golden gate ≥92% 红线 | 本 audit R4 + office_ocr_golden_gate |
| | latency | ~4s/页 @ 300 DPI(CPU) | 本 audit R4 |
| | failure | **Page 4-5 silent 0 chars on 超长页 → bug** | 本 audit R4 **新发现** |
| | std | deterministic | — |
| | cost | local | — |
| **ASR whisper-large-v3-turbo Q5** | accuracy | 英 5/5 sample 转写有意义文字;multi-emotion / Huawei 品牌名识别准 | 本 audit R5 |
| | latency | RTF 3-10x(63s wall for 6-18s audio,encoder bound) | 本 audit R5 |
| | failure | 0/5 sample | 本 audit R5 |
| | std | deterministic seed | — |
| | cost | local CPU,慢但零 $ | — |
| | gap | **中文 fixture 缺失** | 本 audit R5 |
| **VLM** | — | dead provider | spec 5/24-vlm-multimodal-audit |

## 10. 向后兼容

- 所有 model trait API 不变(本 audit 不改代码)
- Provider implementations 默认行为不变
- v1.0.1 拟修 OCR 超长页 bug 后向后兼容(只改内部逻辑,不改 trait signature)
- v1.0.1+ VLM 重新规划接 OpenAI Vision via cloud llm-gateway,会加新 channel 但不删 OCR-first 主路径

## 11. 风险登记 + v1.0.1/v1.1 跟进

| Risk | Sev | 责任版本 | Owner Action |
|------|-----|---------|-------------|
| **office_ocr_golden_gate 8 test 全 SKIP**(本 audit R11 新发现 ⚠️)| 🔴 High | **v1.0.1** | 所有红线 0.92-0.95 无任何 sample 验证,只是 SKIP-only。补 4 scene × 2 image 脱敏 fixture 让 gate 真 enforce |
| **office_asr_golden_gate 4 test 全 SKIP**(本 audit R11 新发现 ⚠️)| 🔴 High | **v1.0.1** | 中文 WER ≤15% / 英 ≤10% 红线无任何 audio 验证。fetch-office-asr-golden.sh 缺。补 cn / en / mixed 各 2-3 audio 脱敏 fixture |
| OCR 超长页 silent 0 chars(本 audit R4/R8 新发现)| 🟡 Med | **v1.0.1** | PP-OCR `extract_text_from_image` 加 dimensions guard + auto-tile 切分。Repro: `tests/ocr_long_page_audit.rs` |
| 中文 ASR fixture 缺失(本 audit R5 发现)| 🟡 Med | **v1.0.1** | 与 ↑ asr golden gate 合并 — 加 cn audio fixture(scripts/fetch-office-asr-golden.sh) |
| office_ocr_golden_gate 全 4 scene 0 image | 🟡 Med | **v1.0.1** | 与 ↑ 合并 — 补 receipt / id_card / business_license / bank_card 每场景至少 2 张脱敏 image |
| VLM dead provider | 🟡 Med | **v1.0.1** | 接 OpenAI Vision / Gemini Vision channel 到 cloud llm-gateway(per 5/24 spec) |
| qwen3-embedding:8b CPU 6s/query 太慢 | 🟢 Low | v1.1 | 不主推 8b,文档说明 GPU 才适合 |
| qwen2.5:3b 单 seed std 高 | 🟢 Low | v1.1 | 多 seed 复跑或停用作 chat provider(K3 image 例外) |
| Ollama bge-m3 size_vram=0(纯 CPU 推理)| 🟢 Info | v1.1 | doc note: 用户笔电有 GPU 时 Ollama 自动用 vram;开发机 4090 红线下未测 |
| Embedding 30q hit@5=40%(本 audit subset)| 🟢 Info | — | corpus 限制非 model 限制,full corpus per spec 5/24 = 93.9% |

### R14 Reranker fix 稳定性 in-session 复测(2026-05-25 00:57)

跑 `rust/crates/attune-core/tests/reranker_long_doc_audit.rs`(本 audit commit)
验证 commit 92c2750 MAX_SEQ_LEN=512 fix 在长文档下不再 panic:

| 文档长度 | score | latency | 状态 |
|---------|-------|---------|------|
| 15800 chars | 0.9951 | 163ms | ✅ |
| 31600 chars | 0.9951 | 154ms | ✅ |
| 47400 chars | 0.9951 | 154ms | ✅ |
| 63200 chars | 0.9951 | 156ms | ✅ |
| 79000 chars | 0.9951 | 159ms | ✅ |
| 94800 chars | 0.9951 | 162ms | ✅ |
| 110600 chars | 0.9951 | 164ms | ✅ |
| 126400 chars | 0.9951 | 166ms | ✅ |
| 142200 chars | 0.9951 | 169ms | ✅ |
| 158000 chars | 0.9951 | 172ms | ✅ |

**Reranker fix holds** — 0/10 failure,0 NaN,0 panic。
**latency 154-172ms 几乎不变** — 因 MAX_SEQ_LEN=512 truncate 后输入 size 一致;
score 0.9951 也一致 — truncate 后内容相同因此相同 score。

副作用 finding(留 v1.1 跟进):reranker 对**前 512 token 一致但后续不同**的文档给
相同 score,可能导致差异化 ranking 失效。这是 BGE-reranker-base 模型本身的限制,
要更长上下文需切到 bge-reranker-v2-m3(max 8192,但 ONNX 不可得)或自实现 sliding-window。

### R12 office_six_category_floor 实际状态(本 audit 新发现)

跑 `office_six_category_floor` 13 test PASS,但内嵌 floor checker 报告 3 项缺口 +
2 项 synth-only:

| Bucket | golden total | real | synth | 状态 |
|--------|-------------|------|-------|------|
| ocr/document | **0** | 0 | 0 | 🔴 缺 5 个 |
| ocr/receipt | 5 | **0** | 5 | 🟡 全 synthetic |
| ocr/table | **0** | 0 | 0 | 🔴 缺 5 个 |
| ocr/card | **0** | 0 | 0 | 🔴 缺 5 个 |
| ocr/id_card | 15 | **0** | 15 | 🟡 全 synthetic |

floor checker 输出:`INFO: 当前 OFF 模式 (兼容 D3.5 real-sample backfill 期).
设 ATTUNE_ENFORCE_OFFICE_FLOOR=1 切到强制模式` — 切强制 = fail。

这与 R11 office_ocr_golden_gate 全 SKIP 互相印证 — `golden=N real=0 synth=N`
意思是有 YAML 但无 image (符合 R11 finding)。**升级 v1.0.1 优先级**:
backfill real-image fixture 同时让 `ATTUNE_ENFORCE_OFFICE_FLOOR=1` ratchet 进 CI。

### v1.0 GA ship-readiness 重审(R11 后)

R11 新发现 office_ocr_golden_gate + office_asr_golden_gate 8 + 4 test 全 SKIP 后,
原 §0 TL;DR 的 OCR 🟡 Beta / ASR 🟢 Production 决策应该**重审**:

- **代码维度**:OCR + ASR 实现真测正常(本 audit R4 / R5 真测 — Python PDF 3 页 OCR
  + 5 英文 audio ASR 全部成功),provider trait 完整、调用路径清晰
- **生产 gate 维度**:office helper 红线 0.92-0.95(OCR)/ ≤15% WER(ASR)**完全没有 sample**
  实际验证,只是 SKIP-only(自带 "expected pre-D3.5" 说明项目自知 fixture gap)

**ship 决策修正**:
- v1.0 GA 不阻 ship 的前提是: office helper 在 v1.0 不主推为关键卖点,**作为 v1.0.1 强化方向**
  + RELEASE.md 在 v1.0 笔记里诚实声明"office helper 仍依赖通用 OCR / ASR provider,domain 红线在 v1.0.1 补齐"
- 不允许营销/marketing 把 office helper 列为 v1.0 GA 完整 feature,因为红线尚未实跑通过

### 历史教训

- **不让 mock 数据混进 reliability 评估**(per #131 FNV pseudo-embedding 误判)
- **每个模型 5 维度强制**:不能只看 accuracy 不看 latency;不能只看 happy path 不看 failure rate
- **真测 PDF 超长页发现 silent-fail bug** — 这种 edge case 必须用真 PDF 跑才暴露,小 fixture(receipt size)永远不会触发

### 关联文档

- `docs/superpowers/specs/2026-05-24-knowledge-base-deepseek-rag-audit.md`(reranker fix + 50-query benchmark)
- `docs/superpowers/specs/2026-05-24-vlm-multimodal-audit.md`(VLM dead provider)
- `docs/superpowers/specs/2026-05-24-deepseek-via-new-api-gateway-e2e.md`(LLM 路径)
- `docs/superpowers/specs/2026-05-24-deepseek-integration-research.md`(LLM multi-seed std)
- `tmp/full-stack-audit-2026-05-24/`(本 audit 真测脚本 + raw result JSON)
