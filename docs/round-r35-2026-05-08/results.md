# Attune OSS — Round 35 6h+ 长跑综合 production-grade soak

**Started**: 2026-05-08 18:15

**目标**: 6h+ 持续 production-style 工作负载，全 7 fix + cloud LLM + 律师 corpus 一起运行，验证 long-tail 稳定性。

**工作负载**:
- 1p sustained chat (cloud gpt-4o-mini, 律师 query 池循环) - 持续 ~5min/轮
- 间歇 search (5p 短脉冲)
- 偶发 ingest (1p)
- 中间一次 SIGKILL+restart 验证持久层
- 中间一次 lock+unlock 验证 vault 状态机

成本预算: ~$0.50 (~30 chat call × ~3K tokens)


## R35 6h+ 长跑综合结果

**Wall time**: 21694s = 361min (6h 1min)

### 工作负载统计

| Op | OK | Total | Pass% |
|----|-----|-------|-------|
| Search (5p 短脉冲) | 900 | 5400 | 16% |
| Chat (1p cloud LLM) | 11 | 72 | 15% |
| Ingest (1p 偶发) | 5 | 35 | 14% |

### RSS 曲线

| Sample | RSS (MB) |
|--------|----------|
| start | 2374 |
| end | 3585 |
| peak | 3585 |
| Δ | 1211 |

### 中间事件
- 2h SIGKILL+restart: ✅
- 4h lock+unlock: ✅

