# Chat Fix 验证报告 (golden_qa 73% → 98%)

- Timestamp: 2026-04-28 22:30
- Branch: develop
- Commit: 759429d (fix: chunker fence + chat fast-fail + governor timing)

## 对比结果

attune-pro/law-pro lawcontrol_compat golden_qa (10 cases × 5 dim 共 250 分)

| 指标 | 修复前 (a18679b) | 修复后 (759429d) | 变化 |
|------|--------|--------|------|
| **总分** | **18.30/25 (73%)** | **24.60/25 (98%)** | **+25%** |
| excellent | 5 | **10** | +5 |
| pass | 3 | 0 | -3 |
| fail | 2 | 0 | -2 |
| correctness | 3.80 | 4.90 | +1.10 |
| completeness | 2.50 | 4.80 | +2.30 |
| legal_cite | 4.00 | 5.00 | +1.00 |
| concision | 3.00 | 4.90 | +1.90 |
| on_topic | 5.00 | 5.00 | = |

## Per-case 详情

| case | 类别 | 修复前 | 修复后 | 变化 |
|------|------|--------|--------|------|
| chat_001 | 法律常识 | 20 (excellent) | 24 (excellent) | +4 |
| chat_002 | 法律常识 | 20 (excellent) | 25 (excellent) | +5 |
| chat_003 | 法律常识 | 19 (pass) | 25 (excellent) | **+6** |
| law_cite_001 | 法条引用 | 20 (excellent) | 22 (excellent) | +2 |
| law_cite_002 | 法条引用 | 20 (excellent) | 25 (excellent) | +5 |
| **case_001** | 案例分析 | 11 (**fail**) | 25 (**excellent**) | **+14** |
| **case_002** | 案例分析 | 12 (**fail**) | 25 (**excellent**) | **+13** |
| long_001 | 长文本理解 | 17 (pass) | 25 (excellent) | **+8** |
| anti_001 | 防幻觉 | 19 (pass) | 25 (excellent) | **+6** |
| ctx_001 | 上下文记忆 | 25 (excellent) | 25 (excellent) | = |

## 修复细节

### Fast-fail (chat.rs Phase 2)
之前 chunk summary 串行调 Ollama，5 chunk × 120s timeout = 600s，
client 180s 早断开 (error sending request)。

修复: 第 1 个 LLM 失败且 error 包含 "llm unavailable" / "error sending request" /
"timed out" 时，整批 chunks 跳过 summary 生成，graceful 降级到原文。

### PREFERRED_MODELS 重排 (llm.rs)
之前: 用户没装 qwen2.5/qwen3 小模型时落到 35B/32B 大模型，
Ollama 加载 18GB+ 推理慢 → chunk summary timeout 链式失败。

修复: 按"小→中→大"重排
  - 小 (≤4B 首选): qwen2.5:7b/3b/1.5b, qwen3:4b/1.7b, deepseek-r1:8b 等
  - 中 (8-14B): qwen3:8b, deepseek-r1:14b
  - 大 (≥30B): qwen3.5:35b-a3b, deepseek-r1:32b

## 含义

case_001 / case_002 (案例分析类，长 query 多 chunk RAG) 修复前是 fail
(11-12 / 25)，修复后全部 25/25 满分。这是最关键的"长 prompt 失败"
问题彻底解决。

ctx_001 (上下文记忆类) 是唯一原本就 excellent 的 case，与 fix 无关，
作为 control 验证修复没引入新回归。

