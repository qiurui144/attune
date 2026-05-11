# Attune + Attune-Pro 20 轮全面健康检查报告

- Timestamp: 2026-04-29T07:20:26+08:00
- Branch:    develop
- Commit:    759429d
- Mode:      full
- Filter:    all

## 执行中...


## 20 轮结果

| 轮次 | 类别 | 状态 | 备注 |
|------|------|------|------|
| 1 | 基础 | ⚠️ WARN | server 未就绪, 后续 round 自动启 server |
| 2 | 基础 | ⚠️ WARN | 未配云端 token, 走本地 Ollama (PREFERRED_MODELS 自动选) |
| 3 | 基础 | ✅ PASS |   Aggregate: Hit@10=0.80 MRR=0.42 Recall@10=0.57|  Aggregate: Hit@10=0.60 MRR=0.50 Recall@10=0.37|  Aggregate: Hit@10=1.00 MRR=0.59 Recall@10=0.80| |
| 4 | 检索 | ❌ FAIL | 无法读取 Scen A 结果 (bench 没跑?) |
| 5 | 检索 | ❌ FAIL | 无法读取 Scen B 结果 |
| 6 | 检索 | ❌ FAIL | 无法读取 Scen C 结果 |
| 7 | 检索 | ⚠️ WARN | top-3 中只有 0 条 legal (跨域可能有泄漏) |
| 8 | 检索 | ✅ PASS | F-Pro tests: 0 passed |
| 9 | 检索 | ✅ PASS | reranker tests: 5 passed |
| 10 | 证据链 | ✅ PASS | project=4fa1fd68-4c7a-43cf-b056-5d20ec8c570d, A=8c74956013e54a3f874aa8ee6f2c81a2 B=a60b900834d94d4f900d5733d90d6268 |
| 11 | 证据链 | ✅ PASS | workflow_test (含 deterministic ops): 7 passed — extract_entities skill 在 attune-pro/law-pro |
| 12 | 证据链 | ✅ PASS | find_overlap deterministic op 通过 (lists_project_files + missing_project_id) |
| 13 | 证据链 | ✅ PASS | annotation 写入成功 id=f12e37f06779490f9e7b9f3e39707414 |
| 14 | 证据链 | ✅ PASS | annotations 列出且内容含'证据链'/evidence_chain 关键词 |
| 15 | law-pro | ✅ PASS | law-pro golden_qa: 24.80/25, excellent=10 |
| 16 | law-pro | ✅ PASS | 5 capabilities (5 目录) + cargo check OK |
| 17 | law-pro | ✅ PASS |   correctness  : 5.00/5   completeness : 5.00/5   legal_cite   : 5.00/5   concision    : 4.80/5   on_topic     : 5.00/5  |
| 18 | 长上下文 | ✅ PASS | 13s, 7181 bytes response |
| 19 | 长上下文 | ⚠️ WARN | 多轮 ctx 记忆失败 (未引用 HT-2024-001) |
| 20 | 综合 | ✅ PASS | 0 层 PASS, 1235 总测试 |

## 汇总

- ✅ PASS: **13 / 20**
- ⚠️ WARN: 4
- ❌ FAIL: 3
- ⏭️ SKIP: 0
