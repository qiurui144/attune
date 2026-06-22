# 原生 MCP server + Agent Scoped Token + 审计(G1 + G2)

> **状态**: DRAFT(spec-first,§3.1 11 节)
> **日期**: 2026-06-22
> **作者**: G1+G2 实现 agent(原生 MCP + scoped-token/审计)
> **前置**: `docs/superpowers/specs/2026-06-22-k3-scheduler-integration.md`(K3 :8090 收口)/ attune-k3 G1-G8 缺口(`2026-06-10-k3-integration-gaps.md`,attune-k3 仓回填) / 现有 `attune-mcp-bridge`(K3 仓,过渡桥)
> **用户拍板**: K3 一体机要接 Hermes / Claude 等 agent host;过渡用 attune-mcp-bridge(MCP server 包装 attune REST)。G1 = attune **原生 MCP**,工具契约 **兼容桥** → 桥退役,K3 零迁移。G2 = MCP 工具粒度权限(scoped token + 审计),同期做。

---

## 1. 目标定位

attune-k3 一体机(个人智算存一体机,Law Edition 首发)是 24h 常驻的 headless 服务,需要被外部 **agent host**(Hermes / Claude Desktop / 其它 MCP client)当作「私有知识库 + 记忆」工具调用。MCP(Model Context Protocol)是 agent host 的事实标准工具接口。

当前 attune 仅暴露私有 REST(`/api/v1/*`)+ WS,agent host 无法直接消费。过渡方案 = `attune-mcp-bridge`(K3 仓的独立 MCP server 进程,把 MCP 工具调用翻译成 attune REST 调用)。该桥是一层多余进程 + 一份需要同步维护的契约。

**G1 目标**:attune-server **原生**暴露 MCP(JSON-RPC over HTTP/SSE,headless 24h 常驻),6 个工具 **工具名 + 参数完全兼容 attune-mcp-bridge 契约** → K3 侧把 agent host 的 endpoint 从桥换成 attune 原生 MCP,**零工具调用代码迁移**;桥进程随后退役。

**G2 目标**:MCP 是「外部 agent 调我的私有知识库」的入口,**必须**比内部 REST 更收紧权限。引入 **scoped token**(settings 签发/吊销,最小权限 `search`/`chat`/`ingest` 三权)+ **高危动作硬黑名单**(export/delete/settings 对 scoped token 永久拒绝,不论权限位)+ **请求级 `agent_source` 审计**(谁调、哪工具、何时,0 敏感内容落本地审计表)。

**用户痛点**:
- K3 命脉 = 数据不出门(隐私优先,Law Edition 处理敏感卷宗)。外部 agent 接入是最大的「数据出门」风险面 → 必须 scoped + 审计 + 高危黑名单。
- 桥是技术债:多一个进程、多一份契约、多一次部署。原生 MCP 让 K3 部署面收窄。

**与产品 positioning 对齐**:
- **1Password 式私密**:scoped token = 最小权限授权,高危黑名单 = 即使授权也挡不可逆操作。
- **成本契约(§成本感知)**:MCP `vault_search`/`ingest` = 零成本/本地算力层;`vault_chat`/`agent_invoke` = 时间/金钱层(经既有成本路径,scoped token `chat` 权限门控)。

## 2. 范围边界

**做(本 spec,G1 + G2)**:
- **G1 原生 MCP server**:attune-server 新增 MCP transport(JSON-RPC 2.0,**HTTP/SSE 优先**,stdio 可选后置)。`tools/list`(discovery)+ `tools/call`(dispatch)+ `initialize` 握手 + MCP error 映射。
- **6 工具**,每个 **包装现有 REST handler 的业务函数**(不重写业务):`vault_search` / `vault_chat` / `ingest` / `annotate` / `agent_invoke` / `job_status`。工具名 + 参数兼容 attune-mcp-bridge 契约(§5 契约表,桥须对齐)。
- **G2 scoped token**:settings 可**签发 / 吊销** scoped token;权限最小集 `search` / `chat` / `ingest` 三权(token 带权限位 + 可过期 + 可吊销)。
- **权限校验**:MCP `tools/call` 校验 token 权限位 — 无对应权限 → 拒(MCP error)。
- **高危永久拒**:export / delete / settings 类动作对 scoped token **永久拒绝**(硬编码黑名单,不论 token 权限位);scoped token **不能**调任何高危工具(本 spec 6 工具中无高危工具暴露,黑名单同时锁未来扩展 + 锁 `ingest` 不退化成 delete)。
- **agent_source 审计**:每次 MCP 工具调用落本地审计表(复用 outbound_audit 风格):`agent_source`(谁,token label) / `tool`(哪工具) / `ts_ms`(何时) / `decision`(allow/deny) / `deny_reason`。**0 敏感内容**(不存 query 原文 / 不存结果)。
- **§6.1 六类测试** + 安全对抗(scoped token 试调 export/delete 必拒;无 chat 权限调 vault_chat 必拒)。

**不做(后续 / 他仓)**:
- stdio transport 完整实现(本 spec HTTP/SSE 优先,stdio 留扩展点 §6,headless 24h 场景 HTTP/SSE 足够)。
- MCP `resources/*` / `prompts/*`(只做 `tools/*`,资源/提示后置)。
- attune-mcp-bridge 仓的退役 PR(归 K3 仓,本 spec 只保证契约兼容使其可退役)。
- scoped token 的 UI 面板美化(本 spec 出 REST 签发/吊销 endpoint + 最小 settings 接线;wizard/精致 UI 后置)。
- 真 K3 设备 + 真 agent host(Hermes)端到端(§7.3,标 **PENDING-真机**)。
- G3 vault locked-mode / G5 durable queue / G7 并发基线 — 另任务(#141 / #142 / K3 v0.5)。

**写死(scope creep 防线)**:
- **6 工具 + 3 权限 + 高危黑名单是封闭集**。新增工具/权限 = 新 spec。
- **不改既有 REST handler 业务逻辑**。MCP 工具是 handler 业务核心的**第二消费者**,经薄包装层调用。若实现中发现必须改 handler 签名才能复用 → 抽 `pub(crate)` core 函数(纯加性,REST handler 也调它),不改 REST 行为。

## 3. 架构数据流

```text
┌─────────── 外部 agent host (Hermes / Claude Desktop / MCP client) ───────────┐
│   MCP JSON-RPC 2.0 over HTTP/SSE                                              │
│   Authorization: Bearer <scoped-token>   ── initialize / tools/list / tools/call
└───────────────────────────────────┬──────────────────────────────────────────┘
                                     ▼
┌──────────────────── attune-server-headless (24h 常驻) ─────────────────────────┐
│  POST /mcp  (JSON-RPC over HTTP)   +   GET /mcp/sse  (server→client events)    │
│       │                                                                        │
│       ▼  mcp::dispatch(request, token_ctx)                                     │
│  ┌──────────────────────────────────────────────────────────────────────┐    │
│  │ G2 GATE (每次 tools/call,handler 业务前)                              │    │
│  │  1. verify scoped token (HMAC + 过期 + nonce 吊销, 复用 vault 信任根)  │    │
│  │  2. high-risk denylist? (export/delete/settings) → 永久拒 (硬编码)     │    │
│  │  3. tool→required scope 映射, token 权限位含? 否 → 拒                    │    │
│  │  4. audit: record_agent_call{agent_source, tool, ts, decision, reason} │    │
│  └───────────────────────────────┬──────────────────────────────────────┘    │
│                                   ▼ (allow)                                     │
│  ┌──────────── tool → 既有 REST 业务核心 (薄包装, 不重写) ─────────────┐      │
│  │ vault_search → search::search_core(state, q, top_k)                   │      │
│  │ vault_chat   → chat::chat_core(state, msg, history, session_id)       │      │
│  │ ingest       → ingest::ingest_core(state, IngestRequest)              │      │
│  │ annotate     → annotations::create_core(state, CreateAnnotationReq)   │      │
│  │ agent_invoke → agents::run_agent_core(state, agent_id, input)         │      │
│  │ job_status   → jobs::job_status_core(state, job_id)                   │      │
│  └───────────────────────────────┬──────────────────────────────────────┘      │
│                                   ▼                                              │
│         既有 vault / search / chat / store (0 业务逻辑改动)                       │
│         (chat/agent_invoke 内部经既有 OutboundGate→Redact→doc_privacy 出网门)     │
└────────────────────────────────────────────────────────────────────────────────┘
```

**信任不变量(硬保证)**:
1. **scoped token 与 vault session token 共信任根**(HMAC over master_key),但**独立命名空间** + **独立权限位**。scoped token 即使被偷,只能在签发的权限子集内动作,且永远碰不到高危黑名单动作。
2. **高危黑名单先于权限位检查**(顺序:denylist → scope)。即使有人错配了一个含 `delete` 字样的权限位,export/delete/settings 仍被硬编码挡掉。
3. **审计先落盘后返回**:allow 与 deny 都落审计(deny 也要留痕,这是入侵检测面)。审计失败**不**放行(fail-closed:审计写不进 → 拒绝调用,防「无痕调用」)。
4. **MCP 出网仍走既有门**:`vault_chat`/`agent_invoke` 经既有 OutboundGate + RedactingLlmProvider + doc_privacy(MCP 不新开出网点)。

**DB tables**:
- **新增 `scoped_tokens`**(scoped token 元数据,token 本身不落库,只落 `token_id` + 权限位 + label + 过期 + 吊销标志)。
- **新增 `agent_audit`**(MCP 调用审计,0 敏感内容)。
- 复用 `sessions`(vault session)/ `vault_meta`(nonce 信任根)。

**cache layers**:scoped token 校验是 HMAC + DB 读(O(1) keyed),不缓存(吊销要即时生效);审计直接 INSERT。

## 4. 模块边界

| crate / 文件 | 改动性质 | 说明 |
|---|---|---|
| `attune-server/src/mcp/mod.rs` | **新增** | MCP JSON-RPC 2.0 协议层:`initialize` / `tools/list` / `tools/call` / error 映射 |
| `attune-server/src/mcp/tools.rs` | **新增** | 6 工具定义(name + JSON schema + required_scope + is_high_risk)+ tool→core 派发 |
| `attune-server/src/mcp/transport.rs` | **新增** | HTTP `POST /mcp`(JSON-RPC)+ SSE `GET /mcp/sse`(axum sse) |
| `attune-server/src/mcp/gate.rs` | **新增** | G2 gate:token verify → denylist → scope → audit(纯函数,可单测) |
| `attune-server/src/routes/scoped_tokens.rs` | **新增** | settings REST:`POST /api/v1/scoped-tokens`(签发)/ `GET`(列)/ `DELETE /{id}`(吊销) |
| `attune-server/src/routes/{search,chat,ingest,annotations,agents,jobs}.rs` | **抽 core(纯加性)** | 把 handler 业务核心抽成 `pub(crate) fn *_core(...)`,REST handler + MCP tool 共用;REST 行为不变 |
| `attune-server/src/lib.rs` | **路由挂载** | 挂 `/mcp` + `/mcp/sse` + `/api/v1/scoped-tokens*`;MCP 路由走专用 gate(不走普通 bearer_auth_guard) |
| `attune-core/src/store/scoped_token.rs` | **新增** | `scoped_tokens` 表 CRUD + `ScopedTokenMeta` 类型 + HMAC 签发/校验(复用 vault crypto) |
| `attune-core/src/store/agent_audit.rs` | **新增** | `agent_audit` 表 CRUD + `AgentAuditEvent` 类型(镜像 outbound_audit 风格) |
| `attune-core/src/store/mod.rs` | **SCHEMA_SQL 加表** | `scoped_tokens` + `agent_audit` 两表 `CREATE TABLE IF NOT EXISTS`(additive,老 vault 自动获空表) |
| `attune-core/src/vault.rs` | **加 scoped 签发/校验**(纯加性) | `issue_scoped_token(scopes, ttl, label)` / `verify_scoped_token(token) -> ScopedClaims`;复用 master_key HMAC + nonce 吊销根 |

**判定**:既有 REST handler 的**业务逻辑 0 改动**(只抽 `*_core` 纯加性,handler 仍调同一函数,wire 行为字节级不变)。vault / search / chat 核心 / 插件 / skill — 0 业务改动。

## 5. API 契约

### 5.1 MCP 协议(G1,被 agent host 消费)

JSON-RPC 2.0 over HTTP `POST /mcp` + SSE `GET /mcp/sse`。`Authorization: Bearer <scoped-token>`。

| method | 说明 | 响应 |
|---|---|---|
| `initialize` | MCP 握手 | `{protocolVersion, capabilities:{tools:{}}, serverInfo:{name:"attune", version}}` |
| `tools/list` | discovery | `{tools: [{name, description, inputSchema}, ...]}`(只列 token 有权限的工具) |
| `tools/call` | 调用 | `{content:[{type:"text", text}], isError}` 或 MCP error |

### 5.2 6 工具契约(兼容 attune-mcp-bridge,桥须对齐)

> **桥对齐要求**:工具名 + 参数 key 必须逐字一致。若本地 `attune-mcp-bridge` 源可见以其为准;不可见时按下表定契约,**桥须对齐本表**(在 K3 仓 spec 标注)。

| 工具名 | 参数(input schema) | 包装的 REST core | required_scope | high_risk |
|---|---|---|---|---|
| `vault_search` | `{query: string, top_k?: int}` | `search::search_core(state, query, top_k)` | `search` | 否 |
| `vault_chat` | `{message: string, session_id?: string, history?: [{role,content}]}` | `chat::chat_core(state, ChatRequest)` | `chat` | 否 |
| `ingest` | `{title: string, content: string, source_type?: string, url?: string, tags?: [string]}` | `ingest::ingest_core(state, IngestRequest)` | `ingest` | 否 |
| `annotate` | `{item_id: string, body: string, anchor?: object}` | `annotations::create_core(state, CreateAnnotationRequest)` | `ingest` | 否 |
| `agent_invoke` | `{agent_id: string, input: object}` | `agents::run_agent_core(state, agent_id, input)` | `chat` | 否 |
| `job_status` | `{job_id: string}` | `jobs::job_status_core(state, job_id)` | `search` | 否 |

> **scope 映射理由**:`annotate` 是写入(归 `ingest` 写权限);`agent_invoke` 触发 LLM 计算(归 `chat` 算力权限);`job_status` 是只读(归 `search` 读权限)。三权覆盖 6 工具,无第四权。

### 5.3 高危黑名单(G2,永久拒绝 scoped token)

硬编码常量 `HIGH_RISK_TOOLS: &[&str] = &["export", "delete", "settings", ...]`(及任何含这些动作语义的工具)。scoped token 调用命中黑名单 → 永久 `forbidden`(MCP error code `-32003`),**不论权限位**。本 spec 6 工具均不在黑名单(MCP 不暴露 export/delete/settings 工具);黑名单同时锁:(a) 未来工具扩展不得绕过;(b) gate 层对任何 method/tool 名做黑名单前置匹配。

### 5.4 scoped token 管理(G2,settings REST,需 vault unlock + session bearer)

| method + path | body | 响应 |
|---|---|---|
| `POST /api/v1/scoped-tokens` | `{label: string, scopes: ["search","chat","ingest"], ttl_secs?: int}` | `{token: "<once-only>", token_id, label, scopes, expires_at}`(token 明文**仅此一次**返回) |
| `GET /api/v1/scoped-tokens` | — | `{items: [{token_id, label, scopes, expires_at, revoked, created_at}]}`(不含 token 明文) |
| `DELETE /api/v1/scoped-tokens/{token_id}` | — | `{revoked: true}` |

**校验**:scopes 必须 ⊆ `{search,chat,ingest}`(非法 scope → 400);ttl 上限/默认见 §7。

## 6. 扩展点 / 插件接口

- **新 MCP 工具**:`mcp/tools.rs` 的工具表加 entry(name + schema + required_scope + is_high_risk + core 派发臂)。新工具若是写/不可逆 → 加进 `HIGH_RISK_TOOLS`。
- **新权限位**:`Scope` enum 加 variant + scope→tool 映射;**需新 spec**(三权是封闭集,扩展是架构决策)。
- **stdio transport**:`mcp/transport.rs` 留 `mcp::dispatch(request, token_ctx)` 为传输无关核心,stdio loop 后续接同一 `dispatch`(本 spec HTTP/SSE 优先)。
- **审计后端**:`agent_audit` 表是本地 SSOT;未来导出/转发(SIEM)经既有 CSV 导出风格扩展,不改 gate。

## 7. 错误 + 边界 case

| case | 行为 | 错误码 |
|---|---|---|
| 无 Authorization / token 格式错 | MCP error,拒 | `-32001` unauthorized |
| scoped token 过期 | 拒 + 审计 deny(reason=`expired`) | `-32001` |
| scoped token 已吊销(nonce 不匹配) | 拒 + 审计 deny(reason=`revoked`) | `-32001` |
| token 无对应 scope(无 chat 权限调 vault_chat) | 拒 + 审计 deny(reason=`scope-missing`) | `-32002` forbidden |
| 高危工具(export/delete/settings) | **永久拒** + 审计 deny(reason=`high-risk-denied`) | `-32003` forbidden |
| 未知工具名 | MCP error,审计 deny(reason=`unknown-tool`) | `-32601` method not found |
| 审计写入失败 | **fail-closed**:拒绝调用(不放无痕调用) | `-32603` internal |
| 工具参数 schema 不符 | 拒,业务前校验 | `-32602` invalid params |
| 业务核心返回 Err(vault locked / backpressure 等) | 映射 MCP error,审计仍记 allow(gate 通过,业务失败) | `-32000` 应用错误 + 原 code |
| scopes 含非 `{search,chat,ingest}` | 签发拒 | 400 `invalid-scope` |
| ttl_secs 超上限(默认 30d,上限 90d) | clamp 到上限 + warn | — |
| vault locked 时签发/吊销 | 拒(需 unlock) | 401 vault-locked |

graceful degradation 全程:任何失败 → MCP error + 审计留痕,**绝不 panic、不 silent swallow**。

## 8. 成本契约

| MCP 工具 | 成本层 | 触发 | scope 门 |
|---|---|---|---|
| `vault_search` / `job_status` | 🆓 零成本(CPU) | agent host 自由调 | `search` |
| `ingest` / `annotate` | ⚡ 本地算力(embedding 后台) | agent host 写入 | `ingest` |
| `vault_chat` / `agent_invoke` | 💰 时间/金钱(LLM,云端或本地) | agent host 显式调 | `chat` |

scoped token 的 `chat` 权限 = 「授权这个 agent 花我的 token/算力」。审计表记每次调用 → 用户可在 settings 看「哪个 agent 花了多少次 chat」。成本路径复用既有(MCP 不新增计费路径)。

## 9. 测试矩阵(§6.1 六类)

| 类型 | 用例 | 工具 |
|---|---|---|
| **happy** | `initialize` 握手;`tools/list` 返 6 工具;每个工具 `tools/call`(full-scope token)→ 包装的 core 被调、返回正确 shape | `#[test]` mcp/* + tools |
| **edge** | `tools/list` 用部分权限 token 只列有权工具;空 history chat;ttl clamp;scopes 子集 | `#[test]` |
| **error** | 未知工具 → -32601;bad params → -32602;业务 Err(vault locked)→ 映射;审计写失败 → fail-closed 拒 | `#[test]` + mock |
| **adversarial / 安全** | (a) scoped token 试调 `export`/`delete`/`settings` → **必拒** `-32003`;(b) 无 `chat` 权限 token 调 `vault_chat`/`agent_invoke` → **必拒** `-32002`;(c) 过期 token 拒;(d) 吊销后立即拒(nonce);(e) 篡改 token HMAC 拒;(f) 高危黑名单先于 scope(含 delete 字样权限位仍挡 delete) | `#[test]` gate + vault |
| **concurrent** | 多 token 并发 `tools/call`(SharedState Arc/Mutex);吊销与调用 race → 吊销后调用拒 | `#[test]` |
| **resource** | 大量审计 INSERT 不阻塞;token 列表分页;审计表只增不爆(0 内容) | `#[test]` |
| **agent_source 审计落表** | allow 调用落 `agent_audit`(agent_source/tool/ts/decision=allow);deny 调用落(decision=deny + reason);0 敏感内容(grep 审计行无 query/content) | `#[test]` agent_audit |
| **真机 §7.3** | K3 真 :8090 + 真 agent host(Hermes)`initialize`→`tools/call` 端到端 | **PENDING-真机** |

通过判据:MCP 协议 / gate / scoped token / 高危拒 / 审计 deterministic PASS rate = 1.00;clippy `-D warnings` 干净;安全对抗 6 项全 PASS(高危永久拒 + scope 拒为硬门)。

## 10. 向后兼容

- `scoped_tokens` + `agent_audit` 是**新增表**(`CREATE TABLE IF NOT EXISTS`,additive)→ 老 vault 下次 open 自动获空表,零 migration。
- MCP 是**新增 transport**(新路由 `/mcp` + `/api/v1/scoped-tokens`),既有 REST `/api/v1/*` + WS **0 改动** → 老 client(Chrome 扩展 / Tauri / attune-pro)零影响。
- 既有 REST handler 抽 `*_core` 是**纯加性重构**(handler 仍调同函数)→ wire 行为字节级不变,既有测试不回退。
- `attune-mcp-bridge`(K3 仓桥):本 spec 保证原生 MCP 工具契约兼容桥 → K3 把 agent host endpoint 从桥切到 attune 原生 MCP **零工具调用代码迁移**,桥退役。桥仍可作 fallback 共存一段(两者契约同)。
- schema_version 不变(新表用既有惯例,无 schema bump)。

## 11. 风险登记

| 风险 | 缓解 |
|---|---|
| **scoped token 提权**:误把高危动作放行 | denylist **先于** scope 检查 + 硬编码常量 + 6 工具均非高危 + adversarial 测试钉死「含 delete 权限位仍挡 delete」 |
| **token 泄漏**:scoped token 被偷 | 权限最小集(只 3 权) + 高危永远碰不到 + 可吊销(nonce 即时生效) + 过期 + 审计可追溯(哪 token 何时调啥) |
| **无痕调用**:审计被绕过 | 审计 fail-closed(写不进 → 拒调用);deny 也落审计(入侵检测面) |
| **桥契约漂移**:原生 MCP 与桥工具名/参数不一致 → K3 迁移要改代码 | §5.2 契约表为 SSOT;桥源可见以源为准,不可见时桥须对齐本表(K3 仓 spec 标注);契约测试钉死工具名+参数 |
| **被迫改业务代码** | 范围写死「0 业务逻辑改动,只抽 `*_core`」;触发即停报告 |
| **MCP 出网新口**:vault_chat/agent_invoke 绕过 OutboundGate | MCP 工具调既有 core,core 内部经既有出网门;MCP 不新开出网点(测试断言出网仍经 gate) |
| **共信任根耦合**:scoped token 与 session 同 master_key | 独立命名空间(payload 含 `scoped:` 前缀)+ 独立权限位 + verify_scoped_token 不接受 session token 反之亦然(测试钉死) |

---

## 切片表(§7.1.4)

| 切片 | 主题 | 关键交付 | 改动层 | 状态 |
|---|---|---|---|---|
| MCP-S1 | core 抽取 | 6 handler `*_core`(纯加性,REST 行为不变) | 重构(加性) | 本 spec |
| MCP-S2 | scoped token 信任根 | vault `issue/verify_scoped_token` + `scoped_tokens` 表 + store CRUD | core/crypto/schema | 本 spec |
| MCP-S3 | agent_audit | `agent_audit` 表 + `AgentAuditEvent` + record/list | core/schema | 本 spec |
| MCP-S4 | G2 gate | token verify → denylist → scope → audit(纯函数) | server/gate | 本 spec |
| MCP-S5 | MCP 协议 + 6 工具 | initialize/tools.list/tools.call + 工具表 + transport(HTTP/SSE) | server/mcp | 本 spec |
| MCP-S6 | scoped-token REST | 签发/列/吊销 endpoint + lib.rs 挂载 | server/routes | 本 spec |
| MCP-真机 | §7.3 端到端 | K3 :8090 + Hermes agent host | — | **PENDING-真机** |

---

## 对齐 G1-G8(`2026-06-10-k3-integration-gaps.md`)

| G | 缺口 | 本 spec 覆盖? |
|---|---|---|
| **G1** | 原生 MCP server(替代 attune-mcp-bridge) | ✅ **本 spec 覆盖**:6 工具兼容桥契约,HTTP/SSE transport,桥退役就绪 |
| **G2** | agent scoped token + 工具粒度权限 | ✅ **本 spec 覆盖**:3 权最小集 + 高危黑名单 + agent_source 审计 |
| G3 | vault locked-mode | ➖ 任务 #141 另做 |
| G5 | durable job queue | ➖ 任务 #142 另做(本 spec `job_status` 工具只读消费其结果) |
| G7 | 多终端并发基线 | ➖ K3 v0.5 实测定级(本 spec concurrent 测试覆盖正确性,非性能基线) |
