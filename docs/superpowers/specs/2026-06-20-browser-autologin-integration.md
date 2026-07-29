# Spec: community-browser-automation 集成（INT-1：浏览器自动登入器接入第三方采集源）

> **Status**: DRAFT — design review pending. **Spec only — NOT implemented（只规划不实现）.**
> **Date**: 2026-06-20  **Author**: integration spec agent（AI draft, for user review per §3.1）  **Task**: #123 (INT-1)
> **Source tool**: `github.com/qiurui144/community-browser-automation`（本地镜像 `/data/tmp/refs/community-browser-automation/`，Python 3.9+ / Playwright，**MIT license**，v0.2.0，**已初步测试过**）。
> **关联 spec（不替代，互补）**:
> - [[2026-06-17-browser-login-assist-session-capture]]（#66）—— 同主题的**纯 clean-room Rust port** 路线（把 `/data/community` AdaptiveBrowser *概念* 重写进 attune-core）。**本 spec 是另一条集成路线**：复用 qiurui144 的**已测真工具**（subprocess sidecar），不是概念 port。§4.5 给出二者取舍裁决（本 spec 推荐 **sidecar 优先 + 长期可选 port**），二者**择一落地，不并行实现**；合入路线确定后删除落选稿（§3.2 同主题单份）。
> - [[2026-06-17-suggestions-and-thirdparty-accounts]]（能力 B：第三方账号统一凭据，`store/third_party_accounts.rs` **已落地**，AES-256-GCM）—— 会话/凭据存储复用其加密模式（§4.3）。
> - [[2026-05-28-privacy-logic-strategy]]（OutboundGate）—— 新增 `OutboundKind::BrowserCrawl`（additive）。
> - CLAUDE.md §成本与触发契约（§8 来源）；§4.5 LLM Agent 兜底原则（PageAnalyzer 经 attune 网关，§4.4）。

---

## ⭐ 跨切面铁律 — ToS / 法律合规 + 凭据安全（PROMINENT — 全 spec 第一约束）

> **本 capability 的 #1 与 #2 风险，置顶不下沉。** 任何后续节与本节冲突，以本节为准。

自动化登录 + 抓取第三方会员/账号墙站点**极可能违反站点 ToS 与反爬条款**；且 community-browser-automation
**默认会用 LLM 自动识别表单并填充凭据**（`auto` 命令）——这把"人在回路"变成了"程序自动登录"，**扩大了合规与凭据攻击面**。
attune 接入时必须把它**收窄回"用户本人授权 + 人在回路兜底"** 的姿态：

| # | 硬约束 | 落点 |
|---|--------|------|
| L-1 | **逐源用户同意门（per-source consent gate）**：添加任何 login-assist 源必须显式勾选"我已阅读并对该源 ToS 合规负责"。未勾选 → 不能创建、不能爬。`consent_at` + `tos_ack_version` 入库。 | §5 / §7 / §9 |
| L-2 | **仅限用户自己的账号/会话**：会话由用户授权登录产生；**永不**共享/转售/跨用户复用/跨设备自动同步；会话与 vault 绑死。 | §4.3 / §2-OUT |
| L-3 | **凭据绝不明文落库/落日志（§1.4）**：源工具 *env-var-only* 凭据模型（`models.py::CredentialSpec` 仅存 env 名）是正面起点，但 attune 不靠 env —— 凭据走 `third_party_accounts.secret_enc`（DEK AES-256-GCM），运行期注入 sidecar 走 **stdin / 短命 env**（§4.4），**不写 recipe、不写 argv、不写 log**。 | §4.4 / §9 |
| L-4 | **保守速率 + 并发=1**：默认 inter-request ≥ 5s，单源串行，每源日上限可配，**不可调到 0**（比 web-search 2s 更保守）。 | §8 / §11 |
| L-5 | **人在回路是用户本人行为，captcha/MFA 不自动破**：源工具的 `auto`（LLM 自动填表）**默认关**；attune 默认走 `login`（人在回路）/ `scan`。`auto` 仅在用户**逐源显式开启 + 配 LLM** 时可用，且 captcha/2FA/QR 一律 fall back `needs-human`（源工具已如此 — README "does not bypass CAPTCHA/MFA"）。**不移植**任何 `navigator.webdriver` spoof / `--disable-blink-features` 反检测姿态作卖点。 | §2 / §4.2 |
| L-6 | **明确 scope 声明 + 免责**：RELEASE/README/Settings 内嵌"用于采集你**有权访问**的会员内容进**本地**库；ToS 合规自负；attune 不为违反第三方 ToS 的使用负责；这**不是**反检测/绕墙工具"。 | §1 / §7.2 Gate-4 |
| L-7 | **SSRF / 内网越权防护**：源工具 `web.py` 已含 loopback/link-local 拒绝 + DNS 重绑检查 + CDP allowlist —— attune 侧**再独立校验一遍** `entry_url`（http(s) only / 禁 localhost / 禁内网 / 禁 `file://`），不依赖 sidecar 自检（defense-in-depth）。 | §7 / §9 |

**反模式（违反即拒绝合入）**：默认开 `auto` LLM 自动登录无 consent / 凭据进 recipe/argv/log / 速率可调 0 / 自动破 captcha / 共享会话 / sidecar 绑 `0.0.0.0` / 把反检测写进 Highlights。

---

## 0. 现状核实（§6.3 — 已有 vs 增量，引代码路径，未臆造）

### 0.1 community-browser-automation 实测能力（读源码，非臆测）

| 能力 | 源路径 | 状态 | 备注 |
|------|--------|------|------|
| `scan`：访问 start_url → 检测 login/captcha/restriction 信号 | `runner.py::detect_signals` / `session_manager.py::_cmd_scan` | ✅ 实装 | recipe-driven selector + restricted_text 关键词 |
| `login`：开页 → 等用户手动登录 → 抓 `storage_state` | `runner.py::capture_login` / `_cmd_login_wait` | ✅ 实装 | **人在回路**，attune 默认主路径 |
| `auto`：LLM 识别表单字段 → 填凭据 → 提交 → 验成功 | `runner.py::auto_login` + `llm_agent.py::PageAnalyzer.find_login_fields` | ✅ 实装 | 默认**关**（L-5）；known_selectors 优先于 LLM；多步登录(用户名→密码两段)已处理 |
| 凭据 env-var-only（recipe 仅存 env 名，绝不存值） | `models.py::CredentialSpec` | ✅ 实装 | 正面安全模型；LLM 只看 page text/form html，**不看凭据**（README Security） |
| 会话持久化 = Playwright `storage_state` JSON（cookies+localStorage） | `runner.py` 各处 `context.storage_state(path=...)` | ✅ 实装 | **单层**（cookies+web storage），无 IndexedDB/WebGL/navigator 还原 |
| recipe-driven 抽取（CSS selector + attr + many） | `runner.py::_extract` / `models.py::ExtractRule` | ✅ 实装 | result_container + 规则数组 |
| 多步 flow（navigate/scan/auto_login/wait_login/search/extract/wait） | `runner.py::run_flow` / `models.py::RecipeStep` | ✅ 实装 | 站内多页流程 |
| **CDP connect（连已起 headed Chromium）** | `runner.py::_with_page` `connect_over_cdp` / `E2E_CDP_ENDPOINT` | ✅ 实装 | **关键**：解决 #66 R5（attune chromiumoxide 反序列化坑）—— sidecar 用 Playwright 而非 chromiumoxide |
| captcha 填写中继 / QR 检测 / OTP 等待 / vision 截图分析 | `session_manager.py::_cmd_captcha/_cmd_qr_detect/_cmd_otp_wait/_cmd_vision_analyze` | ✅ 实装 | attune **不暴露** auto-captcha；QR/OTP 仅作"人在回路提示"用 |
| FastAPI web UI + 256-bit token auth + WS 流 + SSRF/CDP allowlist | `web.py` | ✅ 实装 | attune **不复用其 UI/HTTP**，只调 CLI（§4.4）；其 SSRF 校验作参照 |
| LLM 经 OpenAI-compat（`LLM_BASE_URL/_API_KEY/_MODEL`，默认 deepseek-v4 文本 / qwen-vl vision） | `llm_agent.py::LLMConfig/VisionConfig` | ✅ 实装 | attune 注入时改指 **attune 网关 / BYOK**（§4.4）；qwen-vl 已下架须改 qwen-3.6/3.7（CLAUDE §4.5.H） |

**与 #66 源（`/data/community` AdaptiveBrowser）差异**：本工具是**独立 MIT 真工具**（已测），技术栈 = Playwright + FastAPI + Vue；#66 源是无 license 的内部概念集。本工具**多了** LLM 自动表单识别 + recipe DSL + CDP-connect + QR/OTP/vision；**少了** 4 层会话保真（仅 storage_state 单层）。

### 0.2 attune 既有可复用设施

| 机制 | 路径 | 状态 | 本 spec 关系 |
|------|------|------|-------------|
| 第三方账号凭据保险柜（`secret_enc` DEK BLOB + provider 白名单 + 脱敏 View） | `store/third_party_accounts.rs` | **已落地** | **复用**：会话凭据存于此（新增 provider `login_assist`，§4.3） |
| OutboundGate 6-kind 出网受控 | `outbound_gate.rs:36` | **已有** | **新增** 第 7 kind `BrowserCrawl`（additive） |
| 字段级 DEK 加密 `crypto::encrypt/decrypt(Key32)` | `crypto.rs` | **已有** | 复用（会话 JSON 落 `secret_enc`） |
| 凭据落库参照（git PAT `token_ref_enc` / WebDAV `password_enc`） | `store/git_sources.rs` / `store/webdav_remotes.rs` | **已有** | 泄漏审计参照 `debug_raw_git_token_enc` 范式（§9） |
| SourceConnector / SourceKind（6 variant）+ 唯一入库 `ingest_document` | `ingest/connector.rs:7` / `ingest/pipeline.rs` | **已有** | **新增** 第 7 种 `SourceKind::LoginAssist`；爬取结果统一经 `ingest_document` |
| 系统 Chrome 检测 + web-search 速率范式 | `web_search_browser.rs:16,62` | **已有** | 复用 Chrome 检测；**注意** web-search 已**弃 chromiumoxide 改 reqwest**（CDP 反序列化坑，`web_search_browser.rs:229` 注释）→ 印证 sidecar(Playwright) 是更稳的 CDP 路线（§4.5 R决策依据） |
| 会员场景自动下插件 / 第三方账号建议卡 | task #107 / #100（已完成） | **已有** | onboarding 衔接点（§1.4） |

**结论**：凭据保险柜、DEK 加密、SourceConnector 入库、OutboundGate、Chrome 检测**全已有**。
**增量** = (1) **Python sidecar 进程管理**（spawn / 凭据经 stdin 注入 / JSON-line 解析 / 生命周期+清理）；(2) `LoginAssistSource: SourceConnector`（驱动 sidecar `scan`/`login`/`run` → `RawDocument` → `ingest_document`）；(3) recipe 生成/存储 + 会话 `storage_state` 经 DEK 加密落 `third_party_accounts.secret_enc`；(4) consent gate + 保守速率 + TTL/clear；(5) `OutboundKind::BrowserCrawl` + `SourceKind::LoginAssist` 各 +1 variant；(6) REST + UI tab；(7) **打包**：随 desktop 分发瘦 Python runtime + Playwright（§4.4 / §11 R5）。

---

## 1. 目标定位

### 1.1 用户痛点
用户大量知识沉淀在**登录/会员墙后**（公众号收藏、社区账号墙、需登录的订阅、学术库如 CNKI、SaaS 控制台）。attune 现有 6 种采集源全是开放/静态 token 的，**进不了**这些墙后内容。痛点 = "我浏览器能手动看到的会员内容，attune 进不了我的本地库"。

### 1.2 本 capability 解决什么
**接入已测的 community-browser-automation 作为"登录墙采集 sidecar"**：用户对一个会员源
①过 consent gate ②（默认）点"登录"弹**可见浏览器**手动登录/解 captcha ③attune 捕获 `storage_state` 会话并 **DEK 加密存 vault** ④后续复用会话**无人值守爬取**该源会员页 → 经 `ingest_document` 入本地库 → 过期再请用户登录一次。
进阶用户可**逐源显式开启** `auto`（LLM 自动识别表单填凭据，captcha/MFA 仍人工兜底）。

### 1.3 与产品定位对齐
- **降低 token + 数据安全**：会员内容采集进**本地** vault，正文不出网。
- **成本契约**：爬取/会话复用 = 🆓/⚡（本地浏览器/CPU，零 API）；仅 `auto` 的表单识别用 LLM（💰，用户显式开，§8）。
- **"不注入 web AI / 不走 MCP" 决策不变**：本 capability = **crawl-login-gated-content-INTO-vault**（入站采集），**不是** inject-INTO-web-AI（cleanup-r15 删的方向，保持不做）。**这条边界是 spec 必须厘清的产品红线**（§4.2）。

### 1.4 与 onboarding（会员自动下插件 + 账号建议卡）衔接
完整 onboarding 链：**会员登录（#107 自动下场景插件）→ 第三方账号建议卡（#100）提示"连接你的网盘/邮箱/会员源"→ 用户点某会员源 → 进入本 capability 的 consent + 登录流 → 会话入库 → 内容采集进库 → 该源在建议卡转"已连接"不再弹**。
- login-assist 源凭据/会话作为 `third_party_accounts`(provider=`login_assist`) 一条 → 建议卡规则引擎据此知"已连接"（复用 #100 既有 provider-稀缺判定，**新增 provider 入白名单即可**）。
- 统一账号管理 UI（spec [[...suggestions-and-thirdparty-accounts]]）**列出** login-assist 源为一类账号条目（只读视图，链到本 capability 源管理 tab）；增删/登录/采集走本 capability 专用 API。

---

## 2. 范围边界

### 2.1 IN（本版本做）
1. **Python sidecar 集成层**：随 desktop 打包 community-browser-automation + Python runtime + Playwright；Rust spawn 子进程，CLI 调 `scan`/`login`/`run`（默认）；凭据经 stdin/短命 env 注入；JSON 输出解析。
2. **人在回路登录主路径**：`login`（弹可见浏览器，等用户手动完成）→ 捕获 `storage_state`。
3. **会话 DEK 加密存储**：`storage_state` JSON 经 AES-256-GCM(dek) 落 `third_party_accounts.secret_enc`（provider=`login_assist`），**明文绝不落盘/日志**。
4. **会话复用驱动 crawl→ingest**：解密会话 → sidecar `run --state` 无人值守爬 → `RawDocument` → `OutboundGate(BrowserCrawl)` → `ingest_document` 入 vault。
5. **recipe 管理**：attune 侧生成/存储 recipe（start_url/login_url/signals/extract）；内置默认 + 用户可填 selector。
6. **逐源 consent gate（L-1）+ 保守速率（L-4）+ 会话 TTL/clear（L-2）**。
7. **可选 `auto` 模式**：逐源显式开启 + 配 attune 网关 LLM；captcha/MFA fall back 人在回路。
8. **REST + UI tab**：源增删 / consent 勾选 / "登录" / "清除会话" / "立即采集" / 状态。

### 2.2 OUT（本版本不做 / 永不做）
| OUT 项 | 理由 |
|--------|------|
| 默认开 `auto` LLM 自动登录 | L-5：默认人在回路；`auto` 须逐源显式开。 |
| 自动破 captcha / 自动 2FA / 自动 QR 扫码应答 | L-5；源工具本身也不破（README）。QR/OTP 检测仅作"提示用户手动处理"。 |
| 反检测指纹伪造作卖点（`navigator.webdriver` spoof 等） | L-5；源工具未强推此姿态，attune 不引入。 |
| 复用源工具的 FastAPI / Vue web UI / WS / token auth | attune 有自己的 REST+嵌入式 UI；只调 CLI（§4.4），减暴露面（不起第二个 HTTP 服务）。 |
| sidecar 绑 `0.0.0.0` / 暴露端口 | 只走 CLI stdin/stdout + 文件，**零网络监听**（消除源工具 web 模式的整类风险）。 |
| 向 web AI 注入 context | "不注入 / 不 MCP" 决策不变（§1.3）。 |
| 共享/转售/跨用户/跨设备自动同步会话 | L-2。 |
| 把会话凭据并入一个全新的会话专用表 | 复用既有 `third_party_accounts`（§4.3），不另起表（避免与 spec #66 的 `login_assist_sources` 表分叉；本 spec 用既有表 + 一张轻量 `login_assist_recipes` 元数据表）。 |
| 行业特定源 adapter（law/patent 数据库 detector+抽取规则） | OSS 边界：行业绑定 → attune-pro（§4.3）。 |

### 2.3 后续版本（写死，不允许 silent scope creep）
- v.next-1：4 层会话保真（IndexedDB/navigator/WebGL）—— 仅当单层 storage_state 实测会话失效率高时才做（数据驱动，§11 R7）。
- v.next-2：会话健康度主动探测（过期前提示重登）。
- v.next-3：多源并行（保守速率 + 逐源隔离前提）。
- v.next-4：行业源 adapter（pro，经 §6 扩展点）。
- v.next-5：长期评估"sidecar → 纯 Rust port"（#66 路线）替换，若 Python 打包负担/维护成本超阈值（§4.5）。

---

## 3. 架构数据流

### 3.1 数据流图
```
            ┌──────────────────── attune-core (Rust) ─────────────────────┐
 用户加源    │  LoginAssistSource (实现 SourceConnector)                    │
 +consent L-1│       │ fetch_documents(sink)                                │
      │      │       ▼                                                      │
 [login_assist_recipes] (元数据: entry_url/signals/extract/consent/TTL)     │
 [third_party_accounts] (provider=login_assist, secret_enc = 会话 storage_state[DEK]) │
      │      │   ┌─ SidecarController ───────────────────────────────────┐ │
      │ load │   │ spawn `python -m community_browser_automation ...`     │ │
      │ sess │   │  · scan   → 信号(needs-login/restricted/ok)            │ │
      ▼(dec) │   │  · login  → 弹可见浏览器, 等用户 → storage_state JSON  │◀── 用户手动登录/解 captcha
 SessionStore│──▶│  · run --state → 复用会话爬会员页 → records(JSON)      │ │
      ▲ save │   │ 凭据: stdin/短命 env (绝不进 argv/recipe/log, L-3)     │ │
      │(enc) │   │ 进程: timeout + kill + temp-state 即用即删(zeroize)    │ │
      │      │   └────────────────────────────────────────────────────────┘ │
      │      │       │ records → RawDocument(s)                              │
      │      │       ▼                                                       │
      │      │  OutboundGate::enforce(BrowserCrawl, …)  (fail closed)        │
      │      │       │                                                       │
      │      │       ▼  ingest_document() → parse→dedup→insert→embed→classify│
      └──────┼───────┤ vault (AES-256-GCM, items.content) + secret_enc 会话  │
            └─────────────────────────────────────────────────────────────┘
```

### 3.2 DB tables
**复用** `third_party_accounts`（已落地）：会话存为 provider=`login_assist` 一行，`secret_enc` = 加密的 `storage_state` JSON，`endpoint` = entry_url，`username` = 用户可读源名（**不含密码**），`status` ∈ `none|active|expired`。
> 需为 `KNOWN_PROVIDERS` 新增 `login_assist`（`store/third_party_accounts.rs:25`），并把 `secret` 长度上限从 8192 放宽到会话 JSON 实际需要（storage_state 可达数十 KB → 单列上限提至 256 KB，超限报错而非截断）。

**新增** 轻量 `login_assist_recipes`（recipe + 采集策略元数据，**无 secret**）:
| 列 | 类型 | 说明 |
|----|------|------|
| `source_id` | TEXT PK | 关联 `third_party_accounts.id` |
| `name` | TEXT | 用户可读名 |
| `entry_url` | TEXT | 会员墙入口（http(s) only，§7 校验） |
| `login_url` | TEXT | 登录页（可空，默认 entry_url） |
| `recipe_json` | TEXT | signals/extract/known_selectors（**无凭据**，源工具 recipe shape） |
| `auto_login_enabled` | INTEGER | 0=默认人在回路 / 1=用户显式开 LLM auto（L-5） |
| `consent_at` | INTEGER | L-1（NULL=未同意=禁爬） |
| `tos_ack_version` | TEXT | 同意文案版本 |
| `session_captured_at` | INTEGER | 会话捕获时刻 |
| `session_ttl_secs` | INTEGER | 默认 30 天 |
| `rate_limit_ms` | INTEGER | 默认 ≥5000，clamp 下限 |
| `daily_fetch_cap` | INTEGER | 每日抓取上限 |
| `corpus_domain` | TEXT | pro 跨域防污染 |

> 会话明文**绝不**进 `login_assist_recipes`（此表无 secret 列）；只在 `third_party_accounts.secret_enc` 以密文存。recipe_json 入库前断言不含凭据子串。

### 3.3 cache layers
会话 = 唯一缓存层（decrypt-once-per-crawl，写入 sidecar 用的临时 state 文件 → crawl 结束 `zeroize` + 删文件）。页正文走既有 `indexed_files` 增量去重。

---

## 4. 模块边界

### 4.1 涉及 crate / module / file
| 模块 | 角色 | 新增/复用 |
|------|------|----------|
| `attune-core/src/ingest/login_assist/mod.rs`（新） | `LoginAssistSource: SourceConnector` | **新** |
| `attune-core/src/ingest/login_assist/sidecar.rs`（新） | `SidecarController`：spawn / 凭据 stdin 注入 / JSON 解析 / timeout+kill / temp-state 清理 | **新** |
| `attune-core/src/ingest/login_assist/recipe.rs`（新） | recipe 生成/序列化 + entry_url 校验（L-7） | **新** |
| `attune-core/src/store/login_assist_recipes.rs`（新） | recipe 元数据 CRUD（无 secret） | **新** |
| `attune-core/src/store/third_party_accounts.rs` | 会话 secret_enc 存取 + 新增 provider `login_assist` + 上限放宽 | **复用 + 扩** |
| `attune-core/src/ingest/connector.rs` | `SourceKind::LoginAssist`（as_str=`login_assist`） | **复用 + 扩** |
| `attune-core/src/ingest/pipeline.rs::ingest_document` | 唯一入库 | **复用** |
| `attune-core/src/crypto.rs` | encrypt/decrypt(dek) | **复用** |
| `attune-core/src/outbound_gate.rs` | `OutboundKind::BrowserCrawl`（as_str=`browser_crawl`）+ enforce | **复用 + 扩** |
| `attune-core/src/web_search_browser.rs` | 系统 Chrome/Chromium 检测（供 sidecar `E2E_BROWSER_EXECUTABLE`） | **复用** |
| `attune-server/src/routes/login_assist.rs`（新） | REST | **新** |
| `attune-server/ui`（新 tab） | 源管理 + consent + 登录/清除/采集（i18n zh+en，零硬编码） | **新** |
| `packaging/`（desktop deb/msi/AppImage） | 捆绑 Python runtime + Playwright + community_browser_automation | **新**（§4.4） |

### 4.2 与现有 capability 的关系（产品红线 — 必须厘清）
- ✅ 本 capability = **crawl-login-gated-content-INTO-vault**（把用户有权访问的会员内容采集进**本地** vault）。
- ❌ **不是** inject-INTO-web-AI（向 ChatGPT.com 等 DOM 注入 context）——cleanup-r15 已删，**保持不做**。
- 数据方向相反：本 = **入站采集**；注入 = 出站污染页面。源工具 `human_input`/`vision`/`qr`/`captcha` 等"操控页面"能力 attune **只用于"协助用户登录+捕获会话"**，**不**用于把 attune 内容推到第三方页面。
- 与 web-search（`web_search_browser.rs`）平级：都是浏览器驱动的零-API 出网采集；web-search 抓公开 SERP，本 capability 抓登录墙后内容。复用 Chrome 检测 + 速率范式 + OutboundGate。

### 4.3 OSS / pro 边界 + 凭据归属
按 `oss-pro-strategy.md` v2 §4.3（"对任何领域个人通用用户都有价值 → OSS"）:
| 部件 | 归属 | 论证 |
|------|------|------|
| 通用 login-assist（sidecar 集成 / 人在回路 / 会话加密 / consent / 速率 / 默认 recipe） | **OSS-base** | 通用登录墙采集对任何个人用户有价值，是 web-search + SourceConnector 自然延伸。 |
| 行业源 adapter（法律/专利/期刊库 detector+抽取规则） | **attune-pro** | 行业绑定一律在 pro，经 §6 detector/recipe adapter 扩展接入，不改 core。 |

**凭据归属**：会话凭据存既有 `third_party_accounts`(provider=`login_assist`)，与 WebDAV/IMAP/Git PAT **同表同加密同 vault-locked 语义**；用户登录密码**永不**经 attune（人在回路时密码只进浏览器；`auto` 时密码经 DEK 解出 → stdin 注入 sidecar → 用完 zeroize，**不落 recipe/argv/log**）。

### 4.4 集成方式决策 — **Python sidecar（subprocess）**（见 §4.5 取舍）
- **形态**：community-browser-automation 作为**捆绑的 Python sidecar**，attune Rust 经 `std::process::Command` spawn，调其 **CLI**（`scan`/`login`/`run`），**不**起它的 FastAPI web 服务（零网络监听，OUT §2.2）。
- **凭据注入**：`auto` 模式密码经 DEK 解密 → 写 sidecar **stdin**（或单次性 env，进程退出即失效），**绝不**进 recipe JSON / argv / 日志（L-3）。源工具已是 env-var-only（`models.py`），attune 用**专属随机 env 名 + 单进程生命周期**收窄。
- **LLM 指向**：sidecar `LLM_BASE_URL/_API_KEY/_MODEL` 注入 **attune 网关 / 用户 BYOK**（默认 deepseek-v4 文本；vision 用 qwen-3.6/3.7，**改掉源工具默认的已下架 qwen-vl**，per CLAUDE §4.5.H）。LLM 出网经 attune 网关即受 OutboundGate 约束。
- **浏览器**：sidecar 经 `E2E_BROWSER_EXECUTABLE` 指向 attune 检测到的系统 Chrome（`web_search_browser.rs::detect_system_browser`），或 `E2E_CDP_ENDPOINT` 连 attune 起的 headed 浏览器（local scheduler 一体机模式参照源工具 RK3566 用法）。
- **打包（P0 Win / P1 Linux，§11 R5）**：desktop 安装包捆绑**瘦 Python runtime**（嵌入式 CPython / PyInstaller onedir）+ Playwright + community_browser_automation。Playwright 浏览器二进制**不捆**（用系统 Chrome / 首次运行按需，对齐 attune"thin-deb + runtime-fetch"决策）。体积预算：Python+Playwright wheel ≈ +40-60MB（**plan 首个 gate 实测**）。
- **License**：源工具 **MIT** → 集成无虞；在 attune `ACKNOWLEDGMENTS.md` + sidecar 目录保留其 MIT LICENSE 与 NOTICE。

### 4.5 集成方式取舍（sidecar vs Rust port vs 打包 runtime）
| 维度 | **A. Python sidecar（本 spec 选）** | B. clean-room Rust port（#66 路线） | C. 嵌入 Python 解释器(pyo3) |
|------|-----------------------------------|-----------------------------------|---------------------------|
| 复用已测代码 | ✅ 直接用 v0.2.0 已测工具 | ❌ 全重写（detector/会话/状态机） | ✅ 复用，但需 pyo3 胶水 |
| CDP 稳定性 | ✅ Playwright 成熟（源工具已验 CDP-connect）；规避 attune chromiumoxide 反序列化坑（`web_search_browser.rs:229`） | ⚠️ 需自己解决 CDP（#66 R5 未决） | ✅ 同 A |
| 跨平台打包(Win P0) | ⚠️ 需捆 Python+Playwright（+40-60MB，最大成本） | ✅ 纯 Rust 单二进制 | ❌ pyo3 跨平台/打包最复杂 |
| 维护 | ⚠️ 双语言边界 + 子进程协议 | ✅ 单语言单仓 | ❌ 最重 |
| 上手速度 | ✅ 最快（接 CLI 即可） | ❌ 最慢 | ⚠️ 中 |
| 进程隔离/崩溃容错 | ✅ 子进程崩不拖垮 attune | ✅ 同进程需 panic 边界 | ❌ pyo3 panic 风险高 |

**The call**：**选 A（sidecar）**。理由：(1) 复用**已初步测试过的真工具**，最快交付且 CDP 路线已被源工具验证（直接化解 #66 最大风险 R5）；(2) 子进程隔离比 pyo3/同进程更稳（浏览器自动化易崩）；(3) 唯一显著代价 = Python+Playwright 打包体积（+40-60MB），可接受且对齐 attune 已有"按需 fetch"哲学。**长期**（v.next-5）若 Python 打包/维护负担实测超阈值，再评估 B（port）替换 —— 但 B 须先独立解决 CDP 稳定性，不在本 spec 范围。**否决 C**（pyo3 跨平台 + panic 风险最高，收益不及 A）。

---

## 5. API 契约

### 5.1 REST endpoints（kebab-case，前缀 `/api/v1/`）
| Method | Path | 说明 |
|--------|------|------|
| `POST` | `/api/v1/login-assist/sources` | 创建源。body 必含 `consent:true` + `tos_ack_version`，否则 `400 consent-required`（L-1）。 |
| `GET` | `/api/v1/login-assist/sources` | 列源 + 会话状态（`none/active/expired`），**永不返回会话明文**。 |
| `DELETE` | `/api/v1/login-assist/sources/{id}` | 删源（连带 clear 会话）。 |
| `POST` | `/api/v1/login-assist/sources/{id}/scan` | sidecar `scan`：探测当前是否需登录/受限。 |
| `POST` | `/api/v1/login-assist/sources/{id}/login` | 人在回路登录（弹可见浏览器，等用户"完成"）→ 捕获并加密存会话。 |
| `POST` | `/api/v1/login-assist/sources/{id}/auto-login` | （仅 `auto_login_enabled=1`）LLM 自动登录；captcha/MFA fall back needs-human。 |
| `POST` | `/api/v1/login-assist/sources/{id}/clear-session` | 清除会话（L-2）。 |
| `POST` | `/api/v1/login-assist/sources/{id}/crawl` | 复用会话爬一次（经速率 + OutboundGate → ingest）。 |

### 5.2 typed schema（核心）
```rust
pub struct LoginAssistSourceInput {   // consent 必填
    pub name: String,
    pub entry_url: String,            // http(s) only, 非内网 (L-7 校验)
    pub login_url: Option<String>,
    pub recipe_json: String,          // signals/extract/known_selectors (无凭据)
    pub auto_login_enabled: bool,     // 默认 false (L-5)
    pub consent: bool,                // 必须 true 否则拒 (L-1)
    pub tos_ack_version: String,
    pub rate_limit_ms: u64,           // clamp 到 >= MIN_RATE_LIMIT_MS (L-4)
    pub session_ttl_secs: u64,
    pub corpus_domain: Option<String>,
}
pub enum SessionStatus { None, Active { captured_at: i64 }, Expired } // 对外永不含会话值

// sidecar 输出 (community-browser RunResult JSON) → attune 内部映射，永不序列化进 API/log:
struct SidecarRunResult { status: String, url: String, records: Vec<serde_json::Value>, error: Option<String> }
```

### 5.3 CLI（可选）
`attune login-assist add|list|scan|login|clear|crawl <source>` —— 同 REST 语义；登录仍弹可见浏览器。

---

## 6. 扩展点 / 插件接口
- **recipe/detector adapter**（pro 行业源）：`trait LoginAssistRecipeAdapter { fn recipe(&self) -> RecipeJson; }` 按名注册；OSS 内置 `default`，行业 adapter 经此接入不改 core（§4.3）。
- **sidecar 协议版本化**：`SidecarController` 记录 community-browser-automation 版本（pin 版本号），输出 schema 变更走兼容分支。
- **新会员源接入路径**：填 recipe（entry_url/signals/extract）+ 过 consent + 登录一次，无需改集成层。
- **会话保真升级位**（v.next-1）：会话存储抽象预留"单层 storage_state → 4 层"迁移点。

---

## 7. 错误 + 边界 case
| 场景 | 行为 | 错误码（kebab） |
|------|------|------|
| 创建源未带 consent | 拒绝 | `400 consent-required`（L-1） |
| entry_url 非 http(s)/指内网/file:// | 拒绝创建 | `400 invalid-entry-url`（L-7） |
| rate_limit_ms < 下限 | clamp 到下限（记日志不报错） | —（L-4） |
| 采集时 vault 锁定（会话需解密） | OutboundGate 拒，不出网 | `401 vault-locked` |
| 会话超 TTL | 状态=Expired，提示重登，不静默用过期会话 | `409 session-expired` |
| sidecar 缺失/Python 不可用 | login-assist 整体 disable（同 web-search NeedsDownload） | `503 sidecar-unavailable` |
| sidecar 进程超时/崩溃 | timeout→kill→清临时 state；保留旧会话（若有）；记日志（**不记会话/凭据值**） | `sidecar-failed` |
| sidecar 返回 needs-human（captcha/MFA/无字段） | 转人在回路，提示用户手动登录 | `needs-human` |
| 用户取消登录 | 中止本次，不入半截数据 | `crawl-aborted-by-user` |
| `auto` 但未配 LLM / 未开 auto_login_enabled | 拒绝走 auto，回落 login 提示 | `400 auto-login-disabled` |
| 单页抓取失败 | 吞掉记日志、继续（SourceConnector 约定） | — |
| daily_fetch_cap 触顶 | 停本日爬取 | `daily-cap-reached` |

**graceful degradation**：sidecar/网络/会话失败**不阻塞**其它源；OutboundGate 失败=不出网（fail closed）；sidecar 子进程崩溃不拖垮 attune（进程隔离，§4.5）。

---

## 8. 成本契约（§7 设计约束 5）
| 阶段 | 层级 | 资源 | 触发 |
|------|------|------|------|
| `scan` + `run`（会话复用爬取）+ 页解析 | 🆓 **零成本** | CPU/本地浏览器，**零 API** | 建库阶段自动（同 RSS/WebDAV 周期） |
| 会话捕获 + embedding 入库 | ⚡ **本地算力** | 本地浏览器/GPU embedding | 建库自动；顶栏"暂停后台"可停 |
| **人在回路登录（弹可见浏览器）** | ⚡ **本地+用户时间** | 浏览器+用户手动 | **用户显式点"登录"**；UI 明示"需你手动登录一次" |
| **`auto` 的 LLM 表单识别** | 💰 **时间/金钱** | attune 网关/BYOK LLM | **用户逐源显式开 auto 才发生**；UI 标"本地/云端+预估"，per §8 规则 |
| 后续 chat 分析采集内容 | 💰 | 既有 chat 路径 | 用户 chat 显式触发（不在本范围） |

**归属**：本 capability = ingest-source connector（与 WebDAV/RSS/Git 同层）。采集本身停在 🆓/⚡（零 API）；唯一升第三层的是用户主动开的 `auto` 表单识别，默认关。UI 源卡标"🆓 本地采集 · 需手动登录一次"（auto 开时加"⚡ LLM 自动登录已启用"）。

---

## 9. 测试矩阵（6 类下限对应）
| 类型 | 用例 | 工具 |
|------|------|------|
| **Golden / happy** | ≥10：mock 登录墙 fixture server（needs-login/restricted/ok 信号 + 登录成功页 + 会员结果页）→ sidecar scan/login/run 状态断言；recipe→records 映射 | fixture server + JSON |
| **属性测试** | ≥3：会话 encrypt→decrypt→restore round-trip 不变；rate_limit clamp 单调；recipe_json 序列化不含凭据 | `proptest` |
| **边界 case** | ≥5：空 records / 超大会话 JSON（>256KB 报错不截断）/ 无信号命中 / TTL=0 立即过期 / sidecar 输出非法 JSON | inline `#[cfg(test)]` |
| **异常/错误** | ≥3：未 consent 创建拒 / vault-locked 采集被 OutboundGate 拒 / 会话过期不复用 / sidecar 缺失 503 | YAML expected_error |
| **集成 E2E** | ≥1 subprocess：真 spawn sidecar（mock LLM + mock 站点）→ scan needs-login → (mock 人工 resume) → 捕获会话 → 复用 run 爬第二页 → 入 vault | `tests/login_assist_subprocess.rs` |
| **回归 fixture** | 每修 1 bug 加 1 永久 fixture（会话泄漏 / 凭据进 argv 回归等） | golden set 永久 |

**对抗/安全用例（L-1..L-7 强制）**:
1. **凭据/会话泄漏审计**：grep 全路径 + 实测 —— 会话明文/密码**绝不**出现在 日志/API 响应/错误/argv/recipe_json/序列化（参照 `git_sources::debug_raw_git_token_enc`：库里只密文 BLOB）。
2. **argv 审计**：spawn sidecar 时断言 `Command` 的 args **不含**任何凭据子串（凭据只走 stdin/短命 env）。
3. **log-scrub**：注入含会话/密码值的 crawl，断言所有日志行不含 cookie/token/password 子串。
4. **consent gate 强制**：无 consent 源**不能**被 crawl/auto-login 触发。
5. **会话加密验证**：`secret_enc` BLOB 字节不含明文会话字符串。
6. **速率下限**：设 `rate_limit_ms=0` → clamp 到下限，实测间隔 ≥ 下限。
7. **过期不复用**：超 TTL 会话不当 active，强制重登。
8. **SSRF/越权（L-7）**：entry_url 不得指 `127.0.0.1`/内网/`file://`/非 http(s)；attune 侧独立校验（不依赖 sidecar 自检）；crawl 限同站不跟外站重定向窃 cookie。
9. **auto 默认关**：新建源 `auto_login_enabled` 默认 false；未显式开时 auto-login endpoint 返 `400 auto-login-disabled`。
10. **sidecar 进程清理**：超时/崩溃后断言子进程被 kill + 临时 state 文件删除（no leftover）。

**通过判据**（deterministic）：会话 round-trip/加密/consent/速率/过期/SSRF/argv-审计/进程清理 = pass rate **1.00**。`auto` 的 LLM 表单识别属 LLM 路径 —— 若启用须过 §4.5.A-G 兜底（schema-guided/重试-验证/few-shot/3-tier 矩阵），real-LLM F1 ≥ 0.85 才在 RELEASE 标可用 tier；否则标"实验性，需 ≥ gpt-4o-mini"。

---

## 10. 向后兼容
- **新可选源类型** `SourceKind::LoginAssist`（as_str=`login_assist`）：老库无该源 = 行为不变；不动现有 6 种源路径。
- **DB 迁移**：新增 `login_assist_recipes` 表（additive）；`third_party_accounts` 仅新增 provider 白名单值 + 放宽列上限（不改 schema 结构）。
- **OutboundKind 扩展** `BrowserCrawl`（additive）：既有 6 kind 的 enforce 不变（`as_str()` + 测试同步加分支）。
- **sidecar 版本 pin**：捆绑 community-browser-automation 版本固定；升级走兼容分支 + 回归。
- **会话存储抽象**：单层 storage_state 留"→4 层"迁移位（v.next-1），schema_version 标记。
- **老 client**：不认 `/api/v1/login-assist/*` → 404，不影响既有 tab。
- **与 #66 解耦**：本 spec 用既有 `third_party_accounts` + 新 `login_assist_recipes`；#66 用 `login_assist_sources` 专用表 —— **二者择一**，不可同时落地（同主题单份，§3.2），合入前删落选稿。

---

## 11. 风险登记
| # | 风险 | 等级 | 缓解 |
|---|------|------|------|
| R1 | **ToS / 法律**（#1）：自动登录+爬第三方会员站违反 ToS；`auto` LLM 自动登录加重定性 | **高** | L-1..L-6：consent gate / 仅用户自有会话 / 保守速率 / `auto` 默认关且 captcha 人工 / scope+免责 / 不反检测。RELEASE Gate-4 列 Known Limitation。 |
| R2 | **凭据 + 会话落盘/泄漏**（§1.4）：会话=在用凭据；`auto` 时密码经进程 | **高** | DEK AES-256-GCM(dek) 存 `third_party_accounts.secret_enc`（同 git PAT 模式）；密码只走 stdin/短命 env，**不进 argv/recipe/log**；用完 zeroize；测试断言密文 + argv 审计 + log-scrub。优于源工具仅 env+本地文件。 |
| R3 | **Python sidecar 跨平台打包**（Win P0 + Linux）：体积 +40-60MB / Playwright 浏览器二进制 / Python runtime 嵌入 | **中-高** | **plan 首个 gate = 打包 PoC**：嵌入式 CPython/PyInstaller onedir + Playwright wheel，Win/Linux 各出一次真包测体积+启动；浏览器二进制用系统 Chrome 或按需 fetch（对齐 attune thin-deb 决策）。未过 PoC 不进 impl。 |
| R4 | **sidecar 子进程协议/崩溃**：JSON 输出漂移 / 进程挂死 / 僵尸 | 中 | 版本 pin + JSON schema 校验 + timeout+kill + 临时 state 即用即删 + 进程隔离（崩不拖垮 attune）；§9 用例 10。 |
| R5 | **CDP 稳定性**（attune chromiumoxide 已知反序列化坑） | **中→低** | **本 spec 选 sidecar(Playwright) 正是规避此坑** —— 源工具 `connect_over_cdp` 已验；attune 不再走 chromiumoxide CDP。降级为低风险。 |
| R6 | `auto` LLM 在弱模型上抽烂字段（参照 defamation_extractor F1=0.09 踩坑） | 中 | `auto` 默认关；启用须过 §4.5.A-G 兜底 + 3-tier 矩阵 + F1≥0.85 gate；vision 用 qwen-3.6/3.7（非已下架 qwen-vl）；失败 fall back 人在回路。 |
| R7 | **单层 storage_state 会话失效率**（无 IndexedDB/navigator/WebGL 保真，某些站会话短命） | 中 | 先用单层（源工具已验可用于 CNKI 等）；**实测失效率**驱动是否做 v.next-1 4 层保真（数据驱动，不预先过度工程）。 |
| R8 | 速率过快伤用户自己账号（封号） | 中 | L-4 保守 ≥5s + 并发=1 + daily cap，不可调 0。 |
| R9 | SSRF/内网越权（entry_url 指本机/内网） | 中 | L-7 + §9 用例 8：attune 侧独立校验 + 限同站。 |
| R10 | 并发/锁：crawl worker 与 server 锁序 | 中 | 遵 CLAUDE lock ordering（`fulltext→vectors→vault`）；crawl worker 经 `enqueue_reindex` 间接入库，不自取 vectors/fulltext 锁。 |
| R11 | **与 #66 路线分叉**（两条同主题 spec） | 中 | §4.5 裁决 + §10 解耦：二者择一，合入前删落选稿；本 spec 推荐 sidecar 优先（复用已测工具 + 化解 R5）。 |

---

## 附录 A — 评审流程（per §3.1）
1. 本 spec 落盘（docs/superpowers/，git add -f 入库）。
2. **spec 评审（用户批准）+ 与 #66 路线二选一裁决** ← 当前停在此。落选稿合入前删（§3.2 单份）。
3. 批准后 `superpowers:writing-plans` 出 plan（**首 gate = §11 R3 打包 PoC + R5 CDP 验证**）。
4. plan 评审过 → implementation。
5. spec/plan/impl 三层任一变更，上层同步。

## 附录 B — community-browser-automation 能力 → attune 映射速查
| community-browser-automation（Python/Playwright，MIT） | attune 集成（sidecar） |
|---------------------------------------------------|----------------------|
| CLI `scan`/`login`/`run` | attune REST/CLI 经 SidecarController 调（**主用**） |
| CLI `auto`（LLM 自动登录） | 逐源显式开启才用，默认关（L-5），LLM 走 attune 网关 |
| FastAPI web UI + token auth + WS + SSRF/CDP allowlist | **不复用**（attune 自有 REST+UI，零额外监听）；SSRF 校验作 L-7 参照 |
| `storage_state` 单层会话（cookies+web storage） | DEK 加密存 `third_party_accounts.secret_enc`（升级 4 层=v.next-1） |
| recipe JSON（signals/extract/known_selectors/steps） | 存 `login_assist_recipes.recipe_json`（无凭据） |
| `CredentialSpec` env-var-only | attune 凭据走 vault DEK → 运行期 stdin/短命 env 注入 sidecar |
| `LLM_*`/`VISION_*`（默认 deepseek-v4 / qwen-vl） | 注入 attune 网关/BYOK；vision 改 qwen-3.6/3.7（qwen-vl 已下架） |
| `connect_over_cdp` / `E2E_CDP_ENDPOINT`（RK3566 headed Chromium） | 复用：连 attune 起的 headed 浏览器 / local scheduler 一体机模式 |
| captcha 中继 / QR 检测 / OTP 等待 / vision 分析 | 仅作"人在回路提示用户手动处理"；**不**暴露自动破解 |
| 无 consent gate / 无会话过期 | 新增逐源 consent（L-1）+ TTL 自动过期 + 用户 clear（L-2） |
