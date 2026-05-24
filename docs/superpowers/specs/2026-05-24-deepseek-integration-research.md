# DeepSeek 接入调研 — 5/24 实测验证

> 用户提供 API key 后实测,**软件 100% stability ready,可启动 #71 cloud LLM gate verify**。

## 1. 实测验证(2026-05-24)

### 1.1 endpoint + key 验证

```bash
$ curl -sS -m 10 -H "Authorization: Bearer $DEEPSEEK_API_KEY" https://api.deepseek.com/v1/models
{"object":"list","data":[
  {"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},
  {"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}
]}
```

**4/4 verification 全过**:
1. ✅ key 有效
2. ✅ 模型 list — `deepseek-v4-flash` + `deepseek-v4-pro`(legacy `deepseek-chat` route to v4-flash,2026-07-24 retire)
3. ✅ chat completions — "PONG" 返回
4. ✅ `response_format: json_object` schema-guided — 纯 JSON 返回

System fingerprint: `fp_8b330d02d0_prod0820_fp8_kvcache_20260402`(FP8 KV cache)

### 1.2 cost 实测

- prompt 9 tok + completion 2 tok = 11 tok
- ~$0.000003 USD / call(几乎免费)
- 跑 30 case extractor verify 预估 < $0.10 USD

## 2. attune 软件兼容性

### 2.1 codebase 已 first-class 支持 DeepSeek

| 位置 | 现状 |
|------|------|
| `attune-core::llm::OpenAiLlmProvider` | ✅ OpenAI compat path(DeepSeek 协议兼容)|
| `attune-server::routes::settings.rs:229` | ✅ provider enum 含 `deepseek` |
| `attune-server::tests::system_wizard_full_flow_test.rs:143` | ✅ DeepSeek endpoint test fixture 已有 |
| `cloud/llm-gateway/README.md` | ✅ 5 provider 列表含 DeepSeek |
| `cloud/llm-gateway/docs/failover-policy.md:57` | ✅ deepseek-fallback channel 优先级 5 |
| `llm-gateway-hardening spec` | ✅ 已列 DeepSeek |
| Robust LLM infra(#69)`chat_with_format_json` / `chat_with_retry` | ✅ 完全兼容 DeepSeek schema-guided |

### 2.2 stability tests

| 仓 | tests | 状态 |
|----|-------|------|
| attune-core lib | 1150 pass / 1 ignored / 0 fail | ✅ |
| attune-pro law-pro lib | 286 pass / 0 fail | ✅ |
| attune-pro defamation v3 unit(#135)| 8/8 pass | ✅ MockLlm-as-Claude 模拟强 LLM 输出已 ready |

**结论**:**0 code change**即可接 DeepSeek。

## 3. cost 详细分析

per [DeepSeek pricing 2026](https://api-docs.deepseek.com/quick_start/pricing):

| Model | cache-miss input $/M | cached input $/M | output $/M |
|-------|---------------------|------------------|-----------|
| `deepseek-v4-flash` | $0.14 | $0.0028 | $0.28 |
| `deepseek-v4-pro` | $0.435 | $0.003625 | ~$0.87 |

**对比**:
- GPT-4o-mini: $0.15 input / $0.60 output
- Claude Haiku: $0.25 input / $1.25 output
- DeepSeek V4 Flash:**比 GPT-4o-mini 便宜 ~50%(output 端),input 持平**

cached input 极便宜(系统 prompt 重复多次时优势大):
- attune extractor 系统 prompt ~ 2KB,30 case 跑后 cached → 后续每 call < $0.00001

## 4. context window 优势

DeepSeek V4:**1M token context + 384K max output**

| 用途 | 适配 |
|------|------|
| attune chat 长文档 RAG context | ✅ 几乎不限制 |
| attune-pro fact_extractor 长证据 OCR text | ✅ 1M token 远超 attune-core chunk 上限 |
| 跨多 doc query | ✅ 注入 50+ chunks 仍游刃有余 |

## 5. 接入方案 — 推荐路径

### Path A:attune client 直接 BYOK(用户配 DeepSeek key)

适合个人用户。流程:
1. user 启 attune-server
2. settings UI → LLM provider 选 `openai_compat`
3. endpoint:`https://api.deepseek.com/v1`
4. model:`deepseek-v4-flash`(or `deepseek-chat` legacy alias)
5. api_key:用户 DeepSeek key
6. test connection → smoke ping

**0 code 改动需做**(per #2.1 codebase 现状)。

### Path B:cloud llm-gateway 配 DeepSeek channel

适合付费会员(走 attune-pro cloud gateway,用户不感知)。流程:
1. cloud llm-gateway(new-api)admin
2. 加 channel DeepSeek + base_url `https://api.deepseek.com/v1` + key
3. 优先级 / quota 配置 per `failover-policy.md`
4. attune-pro member login 后,gateway 自动路由 DeepSeek

**0 code 改动需做**(per `cloud/llm-gateway/docs/superpowers/specs/2026-05-22-llm-gateway-hardening.md` 5 provider 设计)。

## 6. #71 LLM gate verify 开发计划

软件 ready,可启动 #71:

### Phase 1(立即可做):attune-pro 3 LLM extractor 真 LLM gate(用 DeepSeek)

跑 `agent_golden_gate.rs llm_agent_golden_gate_real_llm`:
- fact_extractor:10 case → 预期 F1 from qwen 0.88 → DeepSeek **0.92+**(per #58 next-step 预期)
- divorce_extractor:10 case → 预期 F1 from qwen 0.80 → DeepSeek **0.90+**
- defamation_extractor:10 case → 预期 F1 from qwen 0.56 → DeepSeek **≥ 0.85**(关键!)

implementation:
```bash
cd /data/company/project/attune-pro
export ATTUNE_LLM_PROVIDER=openai_compat
export ATTUNE_LLM_ENDPOINT=https://api.deepseek.com/v1
export ATTUNE_LLM_MODEL=deepseek-v4-flash
source /tmp/secrets-deepseek/key.env
export ATTUNE_LLM_API_KEY=$DEEPSEEK_API_KEY
cargo test --release -p law-pro --test agent_golden_gate -- --ignored --nocapture llm_agent_golden_gate_real_llm
```

### Phase 2(v3 prompt verify):defamation v3 prompt 在 DeepSeek 上跑

per #135 spec,v3 prompt 单步(A+C)+ multi-step fallback。预期 single-step F1 ≥ 0.85。

### Phase 3:cross-model 矩阵(#71)

跑 fact / divorce / defamation × DeepSeek-V4-flash / DeepSeek-V4-pro / 现 qwen2.5:3b(对照基准)。

cost 估:5 agent × 3 model × 10 case ≈ 150 token-heavy calls ≈ **< $0.50 USD**

## 7. 风险登记

| R | 描述 | 缓解 |
|---|------|------|
| R1 | key 误进 git | ✅ 存 /tmp/secrets-deepseek/(chmod 600,gitignored)+ env var only |
| R2 | DeepSeek API rate limit | 实测预算 30 case 总 token 远低于 API rate 限制 |
| R3 | DeepSeek model 输出 schema 偏离 | ✅ response_format=json_object 已实测保证 valid JSON |
| R4 | v3 prompt 实测仍 < 0.85 | 后续多步 chain(per #135)兜底 |
| R5 | DeepSeek 服务下线 / 政策变化 | cloud-llm-gateway failover 5 provider 自动切换(per spec)|

## 8. user 决策点 — 开发是否启动?

### 启动 #71 cloud LLM gate verify 条件

- ✅ key 已收(本调研)
- ✅ endpoint 验通(实测 PONG)
- ✅ JSON schema-guided 实测 work
- ✅ 软件 stability 1150 + 286 tests pass
- ✅ 0 code change required
- ✅ cost 预算 < $0.50 USD for full cross-model matrix

**待用户批准 dispatch #71 后,可立即启动 Phase 1(attune-pro 3 LLM extractor real gate via DeepSeek)。**

## 9. 不动事项(per 红线)

- ❌ key 不进 git(stored /tmp/secrets-deepseek/key.env chmod 600)
- ❌ key 不 echo 进 conversation / logs
- ❌ key 不进 dispatch agent prompt(若 dispatch agent 跑,通过 env var 注入,prompt 仅含 path)
- ❌ 不动 Ollama 4090(本路径全 cloud token)

## 10. 历史 timeline

- 5/22 #58 prompt fix sprint:qwen2.5:3b 跑 6 iter,defamation 0.56 / divorce 0.80 / fact 0.88
- 5/22 #58 next-step 建议:cloud LLM 预期 ≥ 0.85
- 5/22 user 决策:defamation 推 v1.0.1 cloud verify
- 5/22-23 4090 use freeze(per global CLAUDE.md "不允许未经批准启用 4090 的 ollama")
- 5/24 user 提供 DeepSeek key + 调研要求
- 5/24 本调研验证 software ready
- 5/24+ pending user 批 #71 启动

## 11. next steps

| Step | owner | timing |
|------|-------|--------|
| 1. 本调研报告 review | user | 5/24 |
| 2. 批 #71 启动 | user | 5/24 |
| 3. dispatch #71 跑 Phase 1 | AI | 5/24-25 |
| 4. cross-model 矩阵 Phase 3 | AI | 5/25-26 |
| 5. v3 prompt cloud verify(#135) | AI | 5/27 |
| 6. RELEASE.md v1.0.1 标 defamation Production via cloud LLM | AI | 5/28+ |
