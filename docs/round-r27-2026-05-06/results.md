# Attune OSS — Round 27 OSS-S19 fix (chat 笔电形态硬性 cloud config)

**Started**: 2026-05-06 05:15

**目标**: 修 chat.rs 让笔电形态 + 无 cloud config 时**明确 reject** 而非 silent fallback 到本地 Ollama。

## ⭐ OSS-S19 fix 编写 + 部署 + 验收 (commit `af782a8`)

### 修复
- state.rs:316-321 LLM 第 3 级 (Ollama auto-detect) 加 `form_factor.prefers_local_llm()` 判断
- 仅 K3Appliance (一体机) 允许本地 fallback
- 笔电/服务器/Unknown + 无 cloud config → None → chat 返回 503

### 验证
- v1 cloud config 在: chat 200 "Hello!..." (cloud OK)
- v2 cloud cleared 后 restart: chat 503 "AI 后端不可用"
- 测试套件 612/0 全过

### 7 个 OSS-S* bug 全部修复完成

| Bug | Commit | 状态 |
|-----|--------|------|
| S12 chat hallucination | b867df8 | ✅ |
| S13 IndexReader leak | 4d083ae | ✅ |
| S14 top_k DoS | 4d083ae | ✅ |
| S15 ingest backpressure | 20decfb | ✅ |
| S16 WS auth | 1e87c50 | ✅ |
| S17 corpus cutoff | c9441ff | ✅ |
| S19 chat fallback | af782a8 | ✅ NEW |

OSS-S20 (corpus 信噪比崩塌) 已记录, 暂不修 (设计层维护工具范畴)。
