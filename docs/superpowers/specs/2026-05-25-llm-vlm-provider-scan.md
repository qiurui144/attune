# LLM × VLM Provider 真测扫描 — 推荐 default tier 决策

- **Date**: 2026-05-25
- **Type**: Empirical verification spec（数据驱动,非架构设计；per `~/.claude/CLAUDE.md §Baseline 不轻易下结论 SOP`）
- **Status**: COMPLETED — 真测数据已落档
- **Scope**: attune-core LLM provider 选型；cloud llm-gateway channel templates
- **Owners**: attune-core / cloud llm-gateway maintainer
- **关联**:
  - `2026-05-24-llm-vlm-multi-provider-architecture.md`（v1.0.1 多 provider 架构 — 本 spec 提供 default tier 数据）
  - `2026-05-25-dual-source-llm-vlm-supply-verification.md`（dual-source 验证)
  - 用户原话: 「扫描一下,在 LLM 和 VLM 方面都进行测试,可以尝试使用 qwen 等,看哪个好用」
  - raw data: `reports/runs/2026-05-25-llm-vlm-scan/`
  - 新增 channel template: `/data/company/cloud/llm-gateway/docs/channel-config-templates/tencent-tokenhub.yaml`

---

## 目录 (Table of Contents)

- [1. 目标定位](#1-目标定位)
- [2. 范围边界](#2-范围边界)
- [3. 架构数据流](#3-架构数据流)
- [4. 模块边界](#4-模块边界)
- [5. API 契约](#5-api-契约)
- [6. 扩展点 / 插件接口](#6-扩展点--插件接口)
- [7. 错误处理 + 边界 case](#7-错误处理--边界-case)
- [8. 成本契约](#8-成本契约)
- [9. 测试矩阵](#9-测试矩阵)
- [10. 向后兼容](#10-向后兼容)
- [11. 风险登记](#11-风险登记)
- [附录 A. 完整真测数据矩阵](#附录-a-完整真测数据矩阵)
- [附录 B. 4 tier 推荐表](#附录-b-4-tier-推荐表)
- [附录 C. 探测过程 + 死路径登记](#附录-c-探测过程--死路径登记)

---

## 1. 目标定位

### 1.1 用户痛点

per `2026-05-24-llm-vlm-multi-provider-architecture.md` §1.1,v1.0 单 provider 设计已不够用。v1.0.1 改造需要回答的关键问题:

- **default LLM**:wizard 推 attune Pro Membership Gateway 时,网关后端应该路由到哪个 model?
- **default VLM**:VLM 接入哪个 provider 性价比最好?
- **cost-conscious tier**:用户自己用 BYOK,我们应该建议哪个最便宜?
- **quality-first tier**:用户付 token 不嫌贵,选哪个最准?

直觉判断("v4-pro 应该比 v4-flash 强"/"hunyuan-pro 应该比 turbo 强")**无据**。需要真测数据替直觉。

### 1.2 与既有规则的对齐

- 严守 `~/.claude/CLAUDE.md §Baseline 不轻易下结论 SOP`:每个结论必须 cite raw log
- 严守 `~/.claude/CLAUDE.md §Secrets 严禁硬编码`:key 通过 env var 注入 `/tmp/secrets-*/key.env`,0 key 进 spec / commit / log
- 严守 `~/.claude/CLAUDE.md §测试方案规范`:SSOT markdown(本 spec)+ test code(`/tmp/llm-vlm-scan-work/run_scan.py`)+ raw data(`reports/runs/`)三 artifact 齐备

### 1.3 北极星

attune 是隐私优先、本地优先的私有知识库。云端 LLM 是辅助路径(per v1.0 GA Roadmap),所以 default provider 选择标准是**结构化输出 fidelity > latency > cost**,不是单纯 benchmark score。

---

## 2. 范围边界

### 2.1 做什么

- 选定 LLM provider × model 共 3 组,跑 5 真实法律 extraction case 各一份
- 落档 raw API output + 时延 + token usage 到 `reports/runs/`
- 据此输出 4 tier 推荐(default / cost-conscious / quality-first / VLM)
- 新增 `tencent-tokenhub.yaml` channel template 到 cloud llm-gateway

### 2.2 不做什么(明示 scope 边界)

- ❌ **不**测 Qwen / GLM / Anthropic / OpenAI / Gemini(user 当前没提供这些 key)
- ❌ **不**做 multi-seed 评估(本次是 single-seed 真测,5 case × 3 model = 15 runs;依据 `~/.claude/CLAUDE.md §调研/算法项目` 多 seed 要求,本次仅作 **default tier 选型 baseline**,正式上线前需要 3-seed 复跑)
- ❌ **不**做 jailbreak / prompt injection / adversarial 测试(per #154 v1.0.1 spec 覆盖)
- ❌ **不**做 VLM 完整真测(user 当前 TokenHub plan 无 VLM 可用 — 探测结论登记在附录 C)

### 2.3 后续(写死,不允许本次扩 scope)

- v1.0.1 上线前:加 ≥ 3 seed 复跑 default tier(per multi-seed 规则)
- v1.0.1 上线前:补 Qwen / GLM provider 真测对照(user 需提供 key 或买 TokenHub 扩展套餐)
- v1.0.1 上线前:VLM 真测(等 user 提供 Qwen-VL / Gemini Vision / GPT-4o key,或买 Hunyuan-Vision TokenHub 套餐)

---

## 3. 架构数据流

```
┌─────────────────────────────────────────────────────────────┐
│  Test driver: /tmp/llm-vlm-scan-work/run_scan.py            │
│                                                              │
│   5 cases (defamation / traffic / divorce / sale / housing) │
│        │                                                     │
│        ▼ for each (provider × model)                         │
│                                                              │
│   call_deepseek()  ──► api.deepseek.com/v1/chat/completions │
│   call_tokenhub()  ──► tokenhub.tencentmaas.com/v1/chat...  │
│        │                                                     │
│        ▼                                                     │
│   evaluate_extraction(raw, expected_keys, expected_values)  │
│        │                                                     │
│        ▼                                                     │
│   落档 reports/runs/2026-05-25-llm-vlm-scan/<p>-<m>-<c>.json│
└─────────────────────────────────────────────────────────────┘

DB tables 涉及:无(纯外部 API 真测,无本地 DB 写入)
Cache layers:无(每次 fresh call,避免缓存导致 latency 失真)
```

---

## 4. 模块边界

| 模块 | 涉及文件 | 角色 |
|------|---------|------|
| Test driver | `/tmp/llm-vlm-scan-work/run_scan.py` | 真测脚本(执行后归档到 commit 不用 — 是一次性产物) |
| Raw outputs | `reports/runs/2026-05-25-llm-vlm-scan/<provider>-<model>-<case>.json` × 15 + `_SUMMARY.json` + `_VLM_PROBE.json` | 数据 SSOT |
| Channel template | `/data/company/cloud/llm-gateway/docs/channel-config-templates/tencent-tokenhub.yaml` | 新增,与现有 `deepseek.yaml` / `qwen.yaml` 平级 |
| Spec | 本文件 | 真测决策 SSOT |

跨仓边界:**attune 仓**(本 spec + raw data)+ **cloud 仓**(tencent-tokenhub.yaml)。

---

## 5. API 契约

### 5.1 DeepSeek 直连(已知,无变更)

- Base URL: `https://api.deepseek.com/v1`
- Auth: `Authorization: Bearer <DEEPSEEK_API_KEY>`
- 实测 active models(per `/v1/models` 探测 2026-05-25):
  - `deepseek-v4-flash` ← **本次推荐 default**
  - `deepseek-v4-pro`(quality-first 候选,但本次数据反对)

### 5.2 腾讯云 TokenHub 网关(本次发现 + 接入)

- Base URL: `https://tokenhub.tencentmaas.com/v1` (per Tencent 官方文档,WebSearch 2026-05-25)
- Auth: `Authorization: Bearer <sk-xxx>` (单 API key,OpenAI 兼容 — **不是** TC3-HMAC-SHA256 签名)
- Endpoint: `/chat/completions`(OpenAI compat)
- User 当前 plan active model(per 真测 2026-05-25):
  - `deepseek-v3-0324` — 唯一 active(其它 candidate 全 `400004 model not found` 或 `401007 NO_FREE_PACKAGE`)
- 注:`ak-xxx` SecretId 字段是 TokenHub console 标识符,API 调用**只用 `sk-xxx`** 作 Bearer

### 5.3 评估契约

每 case 评估 4 维:
- `parsed_ok`: JSON parse 成功(strip markdown fence 后)
- `keys_hit_ratio`: 期望字段命中率
- `value_match_ratio`: 期望值匹配率(int 严格 / string contains)
- `elapsed_s`: 端到端时延

详见 `run_scan.py::evaluate_extraction`。

---

## 6. 扩展点 / 插件接口

本 spec 是真测数据,不直接定义接口。但本次新增的 `tencent-tokenhub.yaml` channel template **复用** cloud llm-gateway 现有 channel 接口:

- new-api channel type = `openai-compatible`
- base_url + key 通过 `<USER_INPUT>` 占位,install-wizard 走 sops 加密注入
- 与现有 deepseek / qwen / glm channel 平级,管理后台一键添加

后续扩展(per #154):
- v1.0.1 加 channel:`tencent-hunyuan-direct.yaml`(TC3 签名,用户提供腾讯云 CAM 主 key 走)
- v1.0.1 加 channel:`qwen-aliyun.yaml`(Aliyun DashScope OpenAI compat)

---

## 7. 错误处理 + 边界 case

### 7.1 本次真测发现的错误形态

| 错误 | 触发条件 | 处理 |
|------|---------|------|
| `401007 NO_FREE_PACKAGE` | TokenHub model 未购买套餐 | 客户端层报 user-friendly 错:「该模型未启用,请到腾讯云控制台购买套餐」 |
| `400004 model not found` | TokenHub model 名拼错 / 未上架 | 客户端层校验:对照 channel template 内 `models:` 白名单 |
| **silent image drop** | text-only model(deepseek-v3-0324) 接收 image input | client 必须**预检**目标 model 是否有 vision capability,no则 reject(per `2026-05-24-vlm-multimodal-audit.md`) |
| **markdown wrapping** | model 输出 ```json...``` 包装 | parser 必须 strip code fence(已在 `evaluate_extraction` 实现,client 端 LLM service 也需要) |
| **unit not normalized** | model 输出 "50万元" 字符串而非 `500000` | extractor prompt 需明确「金额字段返回 int,以元为单位,不要文字」 |

### 7.2 错误码 kebab(client 端)

- `provider-package-inactive`(对应 TokenHub 401007)
- `model-not-found-in-channel`(对应 400004)
- `vision-not-supported`(model = text-only 但传 image)
- `json-parse-failed`(LLM 输出无法 parse)

---

## 8. 成本契约

per `~/.claude/CLAUDE.md §三层成本` + attune CLAUDE.md §成本感知与触发契约:

本次真测 cost 数据(input + output token,via `/v1/chat/completions` usage 字段):

| Provider/Model | total_input_tokens | total_output_tokens | est. cost (per 5 case) | tier 归属 |
|----------------|--------------------|---------------------|------------------------|-----------|
| deepseek-direct/v4-flash | 394 | 826 | ~$0.000287 (input $0.14/M + output $0.28/M) | 🆓 **default / cost-conscious** |
| deepseek-direct/v4-pro | 394 | 539 | ~$0.000699 (input $0.27/M + output $1.10/M) | 💰 quality(本次数据**否决**该归属,见 §9) |
| tencent-tokenhub/deepseek-v3-0324 | 334 | 211 | (TokenHub 套餐内,免单次计费) | 🇨🇳 国内合规 / 备份 channel |

**结论**:v4-flash 既最便宜又最准 → default tier 双优。

---

## 9. 测试矩阵

### 9.1 case 设计(覆盖 attune-pro 5 个 law agent extraction)

| case_id | 域 | 期望字段 | 期望值约束 |
|---------|-----|---------|------------|
| case01-defamation-extract | 名誉权 | defendant / harm / amount_cny | defendant=张某, amount_cny=50000 |
| case02-traffic-accident | 交通事故 | parties / injury_level / repair_cost_cny | injury_level=轻伤, repair_cost_cny=12000 |
| case03-divorce-asset | 离婚财产 | house_value_cny / has_child / child_age | house_value_cny=2800000, has_child=true, child_age=7 |
| case04-sale-contract | 买卖合同 | seller / buyer / total_cny / dispute_type | total_cny=500000 |
| case05-housing-rent | 房屋租赁 | monthly_rent_cny / months_overdue / total_overdue_cny | monthly_rent=5000, months=3, total=15000 |

### 9.2 真测 raw 数据汇总

raw outputs(每 case 一份 JSON)归档于 `reports/runs/2026-05-25-llm-vlm-scan/`:

```
deepseek-direct-deepseek-v4-flash-case01-defamation-extract.json
...
deepseek-direct-deepseek-v4-pro-case05-housing-rent.json
tencent-tokenhub-deepseek-v3-0324-case01-defamation-extract.json
...
_SUMMARY.json       ← 三 provider 统计汇总
_VLM_PROBE.json     ← VLM model 探测过程登记
```

### 9.3 真测结果矩阵

per `_SUMMARY.json` 2026-05-25 23:34:

| Model | API ok | parse ok | mean keys_hit | mean value_match | p50 latency (s) | p95 latency (s) |
|-------|--------|----------|---------------|------------------|------------------|------------------|
| **deepseek-v4-flash** | 5/5 | **5/5** | **1.000** | **1.000** | **1.446** | 4.431 |
| deepseek-v4-pro | 5/5 | 5/5 | 1.000 | 0.700 | 2.503 | 3.157 |
| deepseek-v3-0324 (tokenhub) | 5/5 | 5/5 | 1.000 | 0.933 | 2.021 | **2.172** |

**核心结论(数据驱动)**:

1. **v4-flash 是最佳 extractor**:value_match 满分 1.000,**反直觉**(直觉以为 v4-pro > v4-flash)
2. **v4-pro 输出"50 万元"而非 500000**:更"自然语言",但 extractor 任务下扣分严重(case01 amount_cny="5万元" / case04 total_cny="50万元"→0 分)。raw evidence: `deepseek-direct-deepseek-v4-pro-case04-sale-contract.json::api_result.raw_response`
3. **v3-0324 (tokenhub) 输出 markdown code block wrap**:parser 需 strip,且 case03 输出"280"误省"万"单位 → 抠掉 1/3 分。raw evidence: `tencent-tokenhub-deepseek-v3-0324-case03-divorce-asset.json::api_result.raw_response`
4. **latency**:v4-flash p50 最快(1.45s)但 p95 4.43s(单 case 偶发),tokenhub v3-0324 p50/p95 最稳(2.02-2.17s,差距小)

### 9.4 与既有 v1.0 spec 衔接

per `2026-05-24-llm-vlm-multi-provider-architecture.md` §3 数据流,v1.0.1 default LLM provider 选型本次提供**数据**:

```
Wizard 推荐 LLM provider 顺序(v1.0.1 实装):
  ★ attune Pro Membership Gateway → 后端实际路由 deepseek-v4-flash (本次推荐)
  ☆ BYOK DeepSeek → 直连,client settings template 默认 model=deepseek-v4-flash
  ☆ BYOK Tencent TokenHub → 走 tencent-tokenhub.yaml channel,默认 model=deepseek-v3-0324
  本地 Ollama → unchanged
```

---

## 10. 向后兼容

### 10.1 channel template 命名

`tencent-tokenhub.yaml` 是**新增**,与现有 `deepseek.yaml` / `qwen.yaml` 平级,**不动**老 channel,零 breaking。

### 10.2 client 端 default model

attune v1.0 现行 default model 字段(`app_settings.llm.model`)为 `deepseek-chat`(legacy alias)。本次推荐改为 `deepseek-v4-flash`,但**仅在 wizard fresh setup 生效**;老用户的 settings 不动(per Schema versioning rule)。

### 10.3 spec 衔接

本 spec 是 `2026-05-24-llm-vlm-multi-provider-architecture.md` 的**数据补强**,不替代之。v1.0.1 实装时 dual-cite。

---

## 11. 风险登记

| 风险 | 概率 | 影响 | 缓解 |
|------|-----|------|------|
| **single-seed 偏差**:本次每 model 仅跑 5 case × 1 seed,v4-pro "扣分"可能是 prompt 微小变化导致 | 高 | 错误 default 选型 | v1.0.1 上线前 3-seed 复跑,效应 < 2σ 则**收回** v4-flash > v4-pro 的结论 |
| **TokenHub plan 变化**:user 套餐 active model 可能变,本 spec 的 deepseek-v3-0324 路径可能失效 | 中 | TokenHub channel 报 401007 | 客户端报 user-friendly 错(per §7);install-wizard 注册时 health check 当前 plan |
| **VLM 真测覆盖 0**:本次无 VLM provider 可测 | 高 | v1.0.1 VLM 选型无数据驱动,仍是直觉 | 不 ship VLM 默认 enable,v1.0.1 wizard 显示「VLM 需用户自配 — 推荐 Gemini Vision / GPT-4o / Qwen-VL」+ 用户提供 key 后再补真测 |
| **prompt overfitting**:5 case 都是法律领域,extractor 普通用户场景(笔记摘要 / 邮件分类 / 通用问答)未测 | 中 | 通用任务表现可能不同 | v1.0.1 前补 ≥ 3 通用 case(笔记结构化 / 摘要 / Q&A) |
| **TokenHub 自由域名变更**:腾讯可能改 `tokenhub.tencentmaas.com` | 低 | channel template 域名 hardcoded | 在 channel template 注释里 cite 官方文档 URL,变更时同步更新 |

---

## 附录 A. 完整真测数据矩阵

逐 case 数据(per `reports/runs/2026-05-25-llm-vlm-scan/*.json`):

| case | v4-flash key_hit / val_match / lat | v4-pro key_hit / val_match / lat | v3-0324 key_hit / val_match / lat |
|------|------------------------------------|----------------------------------|------------------------------------|
| case01-defamation | 1.0 / 1.0 / 1.45s | 1.0 / 0.5 / 2.50s | 1.0 / 1.0 / 2.05s |
| case02-traffic | 1.0 / 1.0 / 4.43s | 1.0 / 1.0 / 3.16s | 1.0 / 1.0 / 2.02s |
| case03-divorce | 1.0 / 1.0 / 2.05s | 1.0 / 1.0 / 2.43s | 1.0 / 0.667 / 1.76s |
| case04-sale | 1.0 / 1.0 / 1.37s | 1.0 / 0.0 / 2.86s | 1.0 / 1.0 / 1.81s |
| case05-housing | 1.0 / 1.0 / 1.34s | 1.0 / 1.0 / 2.05s | 1.0 / 1.0 / 2.17s |

**异常点 deep dive**:

- `v4-pro / case01 val_match=0.5`:raw output `"amount_cny": "5万元"` 而非 `50000`。模型输出文字数字,扣 1/2 分
- `v4-pro / case04 val_match=0.0`:raw output `"total_cny": "50万元"`,期望 `500000`,完全错配
- `v3-0324 / case03 val_match=0.667`:raw output `"house_value_cny": 280`,模型把"280 万"理解为"280"忽略单位

参考 raw files:`deepseek-direct-deepseek-v4-pro-case01-defamation-extract.json::api_result.raw_response`、`deepseek-direct-deepseek-v4-pro-case04-sale-contract.json::api_result.raw_response`、`tencent-tokenhub-deepseek-v3-0324-case03-divorce-asset.json::api_result.raw_response`

---

## 附录 B. 4 tier 推荐表

per 9.3 数据驱动:

| Tier | 用户群 | 推荐 provider / model | 数据依据 |
|------|--------|------------------------|---------|
| 🟢 **default**(attune Pro Membership Gateway 后端) | 标准用户 | **deepseek-direct / deepseek-v4-flash** | val_match 1.000 + cost 最低 + p50 1.45s 最快 |
| 💰 **cost-conscious**(BYOK 自掏腰包) | 价格敏感用户 | **deepseek-direct / deepseek-v4-flash** | 同 default — 已经最便宜 |
| 🏆 **quality-first**(性价比无关) | 关注准确性用户 | **deepseek-direct / deepseek-v4-flash**(同 default,因本次 5 case 数据下 v4-flash > v4-pro);后续 3-seed 复跑后可重评 | 现有数据下 flash 已满分,pro 反扣分 |
| 🇨🇳 **国内合规 / 备份** | 数据出境敏感用户 / DeepSeek 主线 down 时 | **tencent-tokenhub / deepseek-v3-0324** | val_match 0.933 + 国内域名 + 套餐计费 |
| 👁 **VLM**(图像理解) | 截图分析 / OCR fallback | ⚠️ **本次无数据**,需用户提供 Qwen-VL / Gemini Vision / GPT-4o key 后补测 | 见 §11 风险 + `_VLM_PROBE.json` |

**重要注**:`quality-first` 与 `default` 相同**仅在本次 5 case 数据下成立**。v1.0.1 上线前必须 3-seed 复跑 + 通用 case 补强。

---

## 附录 C. 探测过程 + 死路径登记

per `~/.claude/CLAUDE.md §Baseline 不轻易下结论 SOP` §3(多路径 fallback),登记本次"X 不可用"前真试过的路径:

### C.1 腾讯云 LLM API 探测(2026-05-25)

| 路径 | 结果 | 死因 |
|------|------|------|
| `hunyuan.tencentcloudapi.com` + TC3 签名 | `AuthFailure.SecretIdNotFound` | user 提供的 `ak-xxx` 不是腾讯云 CAM SecretId(那是 `AKID...` 格式),是 TokenHub 网关的 console identifier |
| `api.lkeap.cloud.tencent.com/v1` + Bearer | `401 not_authorized` | LKEAP 是腾讯云知识引擎,不接受 TokenHub key |
| `api.hunyuan.cloud.tencent.com/v1` + Bearer | `401 Incorrect API key` | 是混元官方 OpenAI 兼容端点,但不接受 TokenHub 的 key |
| `tokenhub.tencentcloudapi.com` + TC3 签名 | `MissingParameter X-TC-Action` | 是腾讯云 OpenAPI gateway,需 TC3 + Action,但 user key 不是 SecretId |
| **`tokenhub.tencentmaas.com/v1` + Bearer** | ✅ **work** | 真正 TokenHub OpenAI 兼容入口,key=sk-xxx |

WebSearch query: `腾讯云 TokenHub API 调用 endpoint ak-xxx sk-xxx OpenAI compatible base_url` 命中官方文档 `cloud.tencent.com/document/product/1823/130081`。

### C.2 TokenHub model 探测

跑 ≥ 30 个候选 model 名,只有 `deepseek-v3-0324` 在 user 当前 plan active。其它(qwen2.5-vl/qwen-vl-max/hunyuan-vision/hunyuan-large-vision/glm-4/gpt-4o/etc)全部 `400004 model not found`,`deepseek-r1-0528` 触发 `401007 NO_FREE_PACKAGE`(说明 model 存在但未购买套餐)。

### C.3 VLM 真测放弃

每个候选 VLM model 全部 `400004 model not found`。`deepseek-v3-0324` 接收 image input 时**静默丢图**返回"我看不到图"。结论:user 当前 key set 无 VLM 可真测,真测无法进行,v1.0.1 必须等 user 提供新 VLM provider key 后才能补全。

---

## Sources

- WebSearch 2026-05-25: [腾讯云 TokenHub 视频生成](https://cloud.tencent.com/document/product/1823/130081)
- WebSearch 2026-05-25: [腾讯混元 OpenAI 兼容接口](https://cloud.tencent.com/document/product/1729/111007)
- WebSearch 2026-05-25: [知识引擎原子能力 OpenAI 兼容](https://cloud.tencent.com/document/product/1772/130551)
