# attune + attune-pro 插件全流程 E2E 验证 — 2026-05-24

**Status**: ✅ Done — 核心链路验通, 4 个 P0/P1 issue 待 v1.0 GA 前修
**Trigger**: 用户原话「attune+attune-pro 插件的全流程验证也需要进行」
**Tester**: AI (Claude Opus 4.7, 自动化 bash + python urllib + DeepSeek API)
**关联**:
- `attune-pro/CLAUDE.md` § Agent 验证铁律
- `attune/docs/superpowers/specs/2026-05-22-attune-attune-pro-integration-smoke.md` (上次 integration smoke)

## 1. 目标定位

5/25 v1.0 GA / 5/26 上架前最后一道 plugin pipeline 验证 — 把 plugin **从源代码到客户端 agent 触发** 整链跑通:
1. plugin pack 构造 (attune-pro/plugins/law-pro/)
2. CLI keygen / sign / verify-sig / install
3. attune-server-headless 启动加载 plugin registry
4. HTTP API `/api/v1/plugins` 显示 law-pro 14 agent
5. POST `/api/v1/agents/<id>/run` 触发 deterministic agent → 拿到完整 audit_trail
6. PATCH `/api/v1/settings` 配 DeepSeek → 触发 LLM extractor agent
7. POST `/api/v1/chat` 命中 chat_trigger → routing 提示 form 路径

## 2. 范围边界

**做的**:
- 8 步 happy-path E2E
- 5 个 agent invocation (civil_loan, limitation, fact_extractor, interest_calculator, traffic_accident)
- DeepSeek LLM 真接入 (35-char key, model=deepseek-chat)
- plugin sig 签 / 验 / 装 全链
- Web search 浏览器 fallback (chat 路径)

**没做(可后续)**:
- pluginhub pytest (venv 缺,网络 403,降级 curl smoke)
- 多次 server 重启 / lock-unlock cycle (上一次 v2 跑挂在 token=空, v3 改成 setup-once 解决)
- 8 个未编译 agent (per Finding-1)
- attune-pro 私有仓 pluginhub 上传链 (该链 cloud/pluginhub 仓覆盖)

## 3. 架构数据流

```
[attune-pro/plugins/law-pro/]
   ├── plugin.yaml (483 行,14 agent + 13 forms + chat_trigger)
   ├── bin/ (5 编译好的 rust binary)
   └── forms/ (13 yaml schema)
        │
        │ attune plugin-keygen → ed25519 keypair
        │ attune plugin-sign --priv-file → plugin.sig (88 bytes)
        │ attune plugin-install --pubkey → 装入 $XDG_DATA_HOME/attune/plugins/law-pro
        ▼
[attune-server-headless]
   ├── boot: PluginRegistry::load_all → "loaded 2 plugins"
   ├── POST /api/v1/vault/setup → token (Argon2id, 159s 首次派生)
   ├── PATCH /api/v1/settings → 配 DeepSeek api_key
   ├── POST /api/v1/agents/<id>/run → agent_runner::run_agent_subprocess
   │      ├── stdin: {input:{...}} JSON
   │      ├── exit 0 → 200 {ok:true, output, audit_trail}
   │      ├── exit 2 → 200 {ok:false, red_lines_violated:true}
   │      ├── exit 3 → 400 (input parse error — caller 的错)
   │      └── exit 4 → 500 (LLM_ENDPOINT not set,这是 BUG-2)
   └── POST /api/v1/chat → chat_trigger detect → web_search → LLM (DeepSeek) → reply
```

## 4. 模块边界

涉及的 crate / 模块:
- `attune-cli` (rust/crates/attune-cli/src/main.rs) — keygen / sign / install / list
- `attune-core::plugin_sig` (ed25519-dalek) — sign + verify
- `attune-core::plugin_registry` — load_all, default_plugins_dir
- `attune-core::agent_runner::run_agent_subprocess` — stdin pipe + exit code 映射
- `attune-server::routes::agents` — `/api/v1/agents/<id>/run`
- `attune-server::routes::chat` — `/api/v1/chat` + plugin chat_trigger detect
- `attune-server::routes::settings` — `/api/v1/settings` PATCH
- `attune-server::routes::forms` — `/api/v1/forms/<plugin>/<form>/submit`
- `attune-pro/plugins/law-pro/src/*` — 14 agent rust 源码 (但 bin/ 只 5 个)

## 5. API 契约

| 端点 | Method | Body shape | 验证状态 |
|------|--------|-----------|---------|
| `/health` | GET | - | ✅ |
| `/api/v1/vault/status` | GET | - | ✅ sealed / unlocked |
| `/api/v1/vault/setup` | POST | `{"password":"..."}` | ✅ 返 `{token, state, recovery_key}` |
| `/api/v1/vault/unlock` | POST | `{"password":"..."}` | ⚠️ v2 测试过 token=空 (env 问题,v3 绕开) |
| `/api/v1/plugins` | GET | - | ✅ 返 plugins[].agents[] |
| `/api/v1/plugins/agents` | GET | - | ❌ **404 — 端点不存在但 doc 提到过** |
| `/api/v1/agents/<id>/run` | POST | `{"input":{...}}` | ✅ |
| `/api/v1/forms/<plugin>/<form>/submit` | POST | (任意,acks) | ✅ |
| `/api/v1/settings` | PATCH | `{"llm":{...}}` | ✅ api_key_set:true 隐 key |
| `/api/v1/chat` | POST | `{"message":"...","history":[]}` | ✅ |

## 6. 扩展点

- 新 plugin: 跟 law-pro 一样布局 `plugin.yaml + forms/ + bin/`, sign + install
- 新 agent: 加 `agents:` list + bin/ 二进制 + forms/<id>.yaml schema
- 新 LLM provider: PATCH `/api/v1/settings` `{"llm":{"provider":"openai_compat", endpoint, model, api_key}}`

## 7. 错误处理 + 边界 case

实测错误码映射(per agents.rs L93+):
- exit 0 → HTTP 200 `{ok:true}` ✅ civil_loan / limitation 验证
- exit 2 → HTTP 200 `{ok:false, red_lines_violated:true}` (未触发,无样本)
- exit 3 → HTTP 400 "agent '<id>' rejected input: <reason>" ✅ 用错 schema 时验证
- exit 4 → HTTP 500 "internal server error" + server log "LLM_ENDPOINT not set" ❌ **BUG-2**
- binary not found → HTTP 500 "agent '<id>' binary not found" ❌ **BUG-1** (8 个 agent 缺)
- timeout → HTTP 503 (未触发)

## 8. 成本契约

实测 LLM 成本(per CLAUDE.md M2 + Cost Trigger Contract):
- chat 一次 (含 web_search): tokens_in=264, tokens_out=81, cost_usd=0.00005964 (DeepSeek)
- chat 简单回答: tokens_in=50, tokens_out=63, cost_usd=0.00002464 (DeepSeek)
- deterministic agent (civil_loan): 0 LLM token, 50ms 本地计算
- LLM extractor (fact_extractor): **未成功调通** (BUG-2)

## 9. 测试矩阵 — 8 步 status

| Step | 内容 | Status | 备注 |
|------|------|--------|------|
| 1 | plugin pack 审查 (14 agent + 13 forms) | ✅ | counts 达标 |
| 2 | plugin-keygen + sign + verify-sig | ✅ | ed25519 64-hex pubkey, 88B sig |
| 2.1 | plugin-verify (paid + Unsigned) | ✅ | 正确拒绝 (设计行为) |
| 2.2 | plugin-verify --trust Official | ✅ | "agents: 14, case_kinds: 8, trust verified: Official" |
| 3 | plugin-install --pubkey + plugin-list | ✅ | trust=Trusted, 装到 sandbox/data/attune/plugins/law-pro |
| 4 | attune-server-headless boot + load plugin | ✅ | "loaded 2 plugins" log line |
| 4.1 | vault/setup → token | ✅ | 110-char token, state=unlocked |
| 4.2 | /api/v1/plugins 看 law-pro | ✅ | API 返 plugins[].agents[] 完整 14 个 |
| 5 | civil_loan_agent 计算 ¥100k×5%×1y | ✅ | I=5000, balance=105000, audit_trail + missing_evidence + followups |
| 5.1 | limitation_agent 5.98 年 > 3 年 | ✅ | LikelyExpired, 引《民法典》第188条 |
| 5.2 | form submit ack | ✅ | next_step 正确 routing |
| 6 | PATCH settings + DeepSeek LLM | ✅ 配置 | api_key_set:true |
| 6.1 | fact_extractor_agent (LLM) | ❌ | **BUG-2: exit 4 LLM_ENDPOINT not set** |
| 6.2 | interest_calculator | ⚠️ | by-design library, 不可直接 dispatch |
| 6.3 | traffic_accident_agent | ❌ | **BUG-1: binary not found** |
| 7 | chat (RAG + chat_trigger) | ✅ | 命中 law-pro chat_trigger, web_search 找到 3 个 citation, DeepSeek 答正确 |
| 8 | pluginhub pytest smoke | ⚠️ | venv 缺 + pip 403, 降级 curl :18810 (服务未跑) |

## 10. 向后兼容

无 schema 变更。v1.0 当前 plugin.yaml schema 是 SSOT。

## 11. 风险登记 / v1.0 GA 阻塞评估

### Finding-1 (P0 GA blocker): **8 个 agent binary 未编译进 plugin pack**

| Agent | plugin.yaml 声明 | bin/ 实际 |
|-------|---|---|
| civil_loan_agent | bin/agent_civil_loan | ✅ |
| bank_aggregator_agent | bin/agent_bank_aggregate | ✅ |
| fact_extractor_agent | bin/agent_fact_extract | ✅ |
| limitation_agent | bin/agent_limitation_check | ✅ |
| evidence_chain_agent | bin/agent_evidence_chain | ✅ |
| **labor_dispute_agent** | bin/agent_labor_dispute | ❌ |
| **traffic_accident_agent** | bin/agent_traffic_accident | ❌ |
| **sale_contract_agent** | bin/agent_sale_contract | ❌ |
| **housing_rent_agent** | bin/agent_housing_rent | ❌ |
| **inheritance_agent** | bin/agent_inheritance | ❌ |
| **defamation_agent** | bin/agent_defamation | ❌ |
| **divorce_extractor_agent** | bin/agent_divorce | ❌ |
| **defamation_extractor_agent** | bin/agent_defamation | ❌ |
| interest_calculator | (library) | n/a |

**8 个**新 agent 的 rust 源码在 `attune-pro/plugins/law-pro/src/`(根据 commit `948d6ff` "14 agent availability — 11 deterministic 178/178")已写完,但 `bin/` 目录缺产出的二进制。

**修复**: `cd attune-pro/plugins/law-pro && cargo build --release` 把所有 agent target 编译到 `target/release/agent_*` → copy / hard-link 到 `bin/`. 这是发布 packaging 任务,通常走 CI 的 `package-plugin.sh`。

**v1.0 GA 影响**: **直接阻塞 4 个 v0.9.0 新 agent 投产** (traffic / divorce / sale / housing per CLAUDE.md v0.9.0 计划)。其他 4 个 (labor / inheritance / defamation / defamation_extractor) 也阻塞同一 release。

### Finding-2 (P1 LLM agent 阻塞): **agent_runner 未传 LLM 配置给子进程**

`fact_extractor_agent` 子进程报 exit 4 `LLM_ENDPOINT not set`。server 已配 DeepSeek (settings.llm.api_key_set:true 验证),但子进程 spawn 时**没注入 `LLM_ENDPOINT` / `LLM_API_KEY` / `LLM_MODEL` env**。

**修复点**: `attune-core::agent_runner::run_agent_subprocess` 在 spawn 前需读 vault settings, **打包 LLM 配置为环境变量** (`LLM_ENDPOINT`, `LLM_API_KEY`, `LLM_MODEL`) 喂给 child process。

**v1.0 GA 影响**: 阻塞 3 个 LLM extractor (fact / divorce / defamation extractor)。即使 binary 编译好了,LLM 配置没传同样不可用。

### Finding-3 (P2 UX 小瑕疵): `plugin-verify-sig` CLI 实际签名为 positional 参数,help/doc 中提到 `--pubkey` 暗示不一致

```
attune plugin-verify-sig --pubkey 65...   # 报错 unexpected argument '--pubkey'
attune plugin-verify-sig <dir> 65...      # 正确,positional
```

**修复**: 要么把 pubkey 改 `--pubkey` flag (与 plugin-install 一致),要么文档统一为 positional。建议前者(API consistency)。

### Finding-4 (P3 minor): `GET /api/v1/plugins/agents` → 404

某些前端代码或 doc 引用这个端点,但 server lib.rs 路由表里**不存在**。正确路径是 `/api/v1/plugins`(返 plugins[].agents[]) 或单 plugin 的 `/api/v1/plugins/{id}/agents`。

**修复**: 删除 doc 中 `/api/v1/plugins/agents` 引用,或补该路由。

### Finding-5 (info, non-bug): vault setup → 110-char token,Argon2id 派生 159 秒

setup 走完 Argon2id (m=64MB, t=3, p=4) + DEK 生成耗时 159 秒(实测 K3 形态),用户首次启动会等。这是设计行为(per `attune-core::crypto::derive_master_key` Argon2id 强度),不算 bug。但若 UX 体验考虑,setup wizard 应有 progress bar / 提示 "首次最长 3 分钟"。

## Follow-up Tasks (v1.0 / v1.0.1)

| Task | 优先级 | Where | Owner |
|------|--------|-------|-------|
| 编译 8 个缺失 agent binary 到 plugin pack bin/ | **P0 GA blocker** | attune-pro/plugins/law-pro CI | 5/25 前 |
| `agent_runner` 传 LLM env vars 给子进程 | **P1 v1.0** | attune-core::agent_runner | 5/25 前 (LLM extractor 才能用) |
| `plugin-verify-sig` 改 --pubkey flag | P2 v1.0.1 | attune-cli | post-GA |
| 删除 `/api/v1/plugins/agents` doc 引用 | P3 v1.0.1 | attune-server doc | post-GA |
| `plugin-list` 在 plugins 目录不存在时返回 0 数组而非字符串 "No plugins installed" | P3 v1.0.1 | attune-cli | post-GA |
| pluginhub venv 自带 / docker compose 跑 test | P3 v1.0.1 | cloud/pluginhub CI | post-GA |

## 关键验证证据

**chat 端到端 (DeepSeek + chat_trigger + web_search)**:
- 提问: "帮我算 10 万借款一年利息,年利率 5%,我代理原告,案件相关"
- 命中: law-pro chat_trigger (keyword "案件" priority 5)
- 返: "🔌 检测到此问题适合 **law-pro** 处理 (律师业务相关查询触发) ... 表单地址: /api/v1/forms/law-pro/civil_loan"
- citations 3 个: ailegal.baidu.com / lawtip.cn / calculator.io
- HTTP 200, 5292ms, $0.00005964

**civil_loan_agent 计算 + audit_trail**:
- input: 张三 vs 李四,10万本金,5% 年利率,2024 全年,代理原告
- output.computation: `{computed_interest: 5000, remaining_balance: 105000, formula_used: "simple_interest_year"}`
- output.missing_evidence: ["案件库未自动识别借条/借款合同", "案件库未自动识别银行流水"]
- output.followups: ["请确认借条是否已上传到案件库", "可补充银行流水做交叉验证"]
- HTTP 200, 50ms

**limitation_agent 时效检查 + 法条引用**:
- input: 受损 2020-06-01, 拟诉 2026-05-24
- output: `elapsed_years: 5.976, status: "likely_expired"`, 引《民法典》第188条
- 提醒律师核中断/中止事由
- HTTP 200, 50ms

## Sandbox 隔离方案 (供后续 E2E 复用)

```bash
TMP=/tmp/e2e-plugin-verify/sandbox
mkdir -p $TMP/{data,config,keys}
export HOME=$TMP XDG_DATA_HOME=$TMP/data XDG_CONFIG_HOME=$TMP/config
# vault.db 落 $XDG_DATA_HOME/attune/, device.key 落 $XDG_CONFIG_HOME/attune/
# plugin 装到 $XDG_DATA_HOME/attune/plugins/<id>/
```

**绝不污染 prod**: 全程 sandbox env,server 用 `--port 18901`/`18902`/`18903`(避开默认 18900),原 plugin.sig 备份 + 恢复。
