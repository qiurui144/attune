# Spec: Browser Login-Assist + Secure Session Capture/Reuse (#66)

> **Status**: DRAFT — design review pending. **Spec only — NOT implemented (只规划不实现).**
> **Date**: 2026-06-17  **Author**: spec-analyst (AI draft, for user review per §3.1)
> **Supersedes**: `docs/superpowers/specs/2026-06-10-browser-login-assist-session-capture.md`
>   (同主题前稿；本稿在当前 develop 上核实现状 + 接入第三方账号管理 spec。合入后删旧稿，per §3.2 同主题最多 1 份。)
> **Source concept**: `/data/community` `AdaptiveBrowser`（`shared/playwright_adaptive*.py` + `briefing/src/platform/auto_login.py` + `briefing/src/api/routes/qrlogin.py`）。**clean-room PORT — 复用设计 IDEA，不复制 Python 源码**（§4.4）。
> **关联 spec**:
> - [[2026-06-17-suggestions-and-thirdparty-accounts]] — 第三方账号统一管理（能力 B）。**会话凭据是该统一凭据体系的一类**，本 spec 定义其专用捕获/复用机制；二者关系见 §1.4 + §4.2。
> - [[2026-05-28-privacy-logic-strategy]] — OutboundGate 出网受控（本 capability 新增 `BrowserCrawl` kind）。
> - CLAUDE.md §成本感知与触发契约（本 capability 第 8 节约束来源）。

---

## ⭐ 跨切面铁律 — ToS / 法律合规 (PROMINENT — 全 spec 第一约束)

> **这是本 capability 的 #1 风险，置顶不下沉。** 任何后续节（§2 / §8 / §11）与本节冲突，以本节为准。

自动化登录 + 抓取第三方会员/账号墙站点（微信公众号 / 搜狗 / 任意 membership-gated source）
**极可能违反这些站点的服务条款（ToS）与反爬条款** —— `INTERCEPT` 信号之所以存在，正是因为这些站点
*主动阻止*自动化访问。attune 必须把合规责任以产品契约的形式固化，**不允许默默开启**：

| # | 硬约束 | 落点 |
|---|--------|------|
| L-1 | **逐源用户同意门（per-source consent gate）**：用户添加任何 login-assist 源时，必须显式勾选 "我已阅读并对该源的 ToS 合规负责"。未勾选 → 该源不能创建、不能爬取。同意状态入库（`consent_at` 时间戳 + ToS-ack 版本）。 | §5 API / §7 错误 / §9 测试 |
| L-2 | **仅限用户自己的账号/会话**：捕获的会话必须是用户本人手动登录产生的；**永不**共享、转售、跨用户复用、跨设备自动同步。会话与 vault 绑死，随 vault 走。 | §1.4 / §2-OUT |
| L-3 | **保守默认速率限制**：默认 inter-request 间隔 ≥ 5s（比 web-search 的 2s 更保守），并发上限 = 1（单源串行），每源每日抓取上限可配。用户**不能**把速率调到 0。 | §8 / §11 |
| L-4 | **人在回路 = 用户本人的行为**：captcha / 登录由**用户手动**完成（可见本地 Chrome 窗口），attune 不自动破解 captcha、不自动注入凭据、不自动 2FA。这把 "自动规避" 的定性部分转移为 "用户本人操作其本人账号"。 | §2-OUT / §4 |
| L-5 | **明确 scope 声明 + 免责**：RELEASE.md / README / Settings UI 内嵌一段范围与免责声明："Login-assist 用于把你**有权访问**的会员内容采集进你**本地**的知识库；你需自行确认所添加来源的 ToS 合规；attune 不为违反第三方 ToS 的使用负责。" | §1 / §7.2 Gate-4 |
| L-6 | **保守反检测立场**：**不**把反 bot 指纹**伪造**当作 feature 移植。L3/L4（navigator/UA/WebGL）仅做"会话保真重放（fidelity replay）"——还原**用户真实登录那次**的环境以维持会话有效，**不**为了规避检测而主动伪造一个虚假身份。明确 OUT：`--disable-blink-features=AutomationControlled`、`navigator.webdriver` 主动 spoof 不作为卖点（详见 §2-OUT / §4）。 | §2 / §4 / §11 |

**反模式（违反即拒绝合入）**：默认开启爬取无同意门 / 速率可调到 0 / 自动破 captcha / 共享会话 /
把指纹伪造写进 RELEASE Highlights 当能力卖点。

---

## 0. 现状核实（§6.3 — 已有 vs 增量，全部引代码路径，未臆造）

| 机制 | 路径 | 状态 | 本 spec 关系 |
|------|------|------|-------------|
| chromiumoxide 浏览器自动化（zero-API web search） | `attune-core/src/web_search_browser.rs` | **已有** | 复用系统 Chrome 检测（`detect_system_browser` / `cached_browser_path` / 三段式 resolve）+ 速率限制范式 |
| 系统 Chrome / Chrome-for-Testing 检测 | `web_search_browser.rs:15,33` | **已有** | 复用 |
| OutboundGate 6-kind 出网受控（Llm/CloudSaas/Webdav/WebSearch/Telemetry/Embedding） | `outbound_gate.rs:36` | **已有** | **新增** 第 7 kind `BrowserCrawl`（additive） |
| 字段级 DEK 加密原语 `crypto::encrypt/decrypt(Key32)` | `attune-core/src/crypto.rs` | **已有** | 复用（会话 JSON 落 `session_enc` BLOB，与 `items.content` 同模式） |
| 凭据落库参照模板（WebDAV `password_enc` DEK BLOB） | `store/webdav_remotes.rs:43` | **已有** | DB 表设计照此（密文 BLOB，明文绝不落盘） |
| Git token 引用加密落盘 | `store/git_sources.rs:65`（`token_ref_enc`） | **已有** | 会话泄漏审计参照 `debug_raw_git_token_enc` 测试范式（§9） |
| 统一采集框架 `SourceConnector` / `SourceKind`（6 variant） | `ingest/connector.rs:7` | **已有** | **新增** 第 7 种源 `SourceKind::LoginAssist` |
| 唯一入库路径 `ingest_document` | `ingest/pipeline.rs` | **已有** | 复用（爬取结果统一经此入 vault） |
| 第三方账号统一凭据抽象（`credentials` 表 + provider adapter） | [[2026-06-17-suggestions-and-thirdparty-accounts]] §4（DRAFT，未实现） | **规划中** | 会话凭据是其一类（§4.2）；本 spec 不 block 它，亦不依赖其落地 |
| **AdaptiveBrowser 概念**（headless↔headed 切换 / 拦截检测 / 4 层会话 / 人在回路 resume） | `/data/community shared/playwright_adaptive*.py`（Python/Playwright，**非 attune 代码**） | **外部参照** | clean-room PORT 到 Rust（§4.4） |

**结论**：浏览器自动化底座（系统 Chrome 检测 + 速率范式）、DEK 加密、SourceConnector 入库管道、OutboundGate **全部已有**。
**缺的（增量）**：(1) headless 爬撞墙时的**拦截检测器**；(2) **可见本地 Chrome 人在回路登录循环**；(3) **4 层会话捕获 + DEK 加密落盘**；(4) **会话复用驱动 crawl→ingest**；(5) 逐源 consent gate + 保守速率 + TTL/clear；(6) REST + UI tab。
**增量 ≈ 一个新 SourceConnector（含状态机 + detector + session 模块）+ 新 DB 表 + OutboundKind/SourceKind 各 +1 variant + 新 REST/UI。零新增 LLM 自动调用（采集层永不升第三层，§8）。**

> chromiumoxide 当前**不在 Cargo.toml 硬依赖**（web-search 实际走 reqwest 抓 SERP）；headed 人在回路 + CDP 会话捕获**需先 PoC 落实 CDP 能力**（§11 R5，plan 首个 gate）。

---

## 1. 目标定位

### 1.1 用户痛点

attune 已有 chromiumoxide 浏览器自动化（zero-API web search）与统一采集框架（`SourceConnector`：
本地文件夹 / WebDAV / Email / RSS / CloudDrive / GitRepo），但**所有现有采集源都是开放/凭静态 token 即可的**。用户大量有价值的
知识沉淀在**登录/会员墙后**（微信公众号收藏、社区账号墙内容、需登录才能看的订阅源、SaaS 控制台、企业内网）——这些 attune 当前
**完全采集不到**：headless 爬一打就撞 401/403/captcha/`/login` 重定向。

痛点 = "我能用浏览器手动看到的会员内容，attune 却进不了我的知识库"。

### 1.2 本 capability 解决什么

**人在回路的会话捕获（human-in-the-loop session capture）**：attune headless 爬一个登录/会员墙源，
拦截到反爬信号时**弹出一个可见的本地 Chrome 窗口**，用户**手动登录 / 手动解 captcha**，点 "完成"，
attune 捕获已认证会话（4 层），**vault 加密**存储，并在后续对该源会员内容的爬取中**复用**该会话——直到过期再请用户登录一次。

### 1.3 与产品定位对齐

- **降低 token + 数据安全**（CLAUDE.md 产品定位）：会员内容采集进**本地** vault，不出网、不经云。
- **成本契约**（CLAUDE.md §成本与触发契约）：爬取 = 🆓/⚡（本地 Chrome / CPU，零 API 费用），归属 ingest-source connector 层（§8）。
- **"不注入 web AI / 不走 MCP" 产品决策不变**（§4）：本 capability 是
  **crawl-login-gated-content-INTO-vault**（把会员内容采集进 vault），**不是** inject-INTO-web-AI
  （向 ChatGPT.com 等注入 context）——后者依旧**不做**。

### 1.4 与第三方账号管理 spec 的关系（会话凭据 = 一类凭据）

[[2026-06-17-suggestions-and-thirdparty-accounts]] 能力 B 把**静态配置型凭据**（LLM BYOK key / WebDAV password / Git PAT / Email password）
收编进统一 `credentials` 表（`provider_type + secret_enc[DEK]`）。**浏览器会话凭据是凭据体系中一个性质不同的类**：

| 维度 | 静态凭据（spec B `credentials`） | 会话凭据（本 spec `login_assist_sources.session_enc`） |
|------|--------------------------------|----------------------------------------------------|
| 形态 | 用户手输的 key/password（单字段 secret） | 浏览器登录后捕获的 **4 层会话 JSON**（cookies+storage+IndexedDB+环境保真） |
| 获取 | 用户**输入**明文 → DEK 加密 | 用户**交互登录** → 程序捕获（attune **永不**持有用户密码，L-2/L-4） |
| 生命周期 | 长期，用户改才变 | **会过期**（TTL）→ 需重登 |
| 测试连接 | provider adapter `test()`（spec B §5） | 复用会话试抓一次入口页 → active/expired |

**边界裁决**：会话凭据**不并入** spec B 的 `credentials` 表（形态/生命周期差异大，强行统一会污染 spec B 的简单 secret 模型）。
但二者**共享同一加密原语**（`crypto::encrypt(dek, …)`）、**同一 vault-locked 失败语义**（锁屏不可读）、**同一出网受控点**（OutboundGate）。
统一账号管理 UI（spec B）可**列出**本 spec 的 login-assist 源作为一类账号条目（只读视图，链到本 spec 的源管理 tab），但**增删/捕获/复用**走本 spec 的专用 API。spec B 不实现也不阻塞本 spec。

---

## 2. 范围边界

### 2.1 IN（本版本做）

1. **Intercept-detectors（拦截信号检测器）**：clean-room PORT community 的信号集——HTTP `401/403/429/503`、
   DOM captcha 标记（`.geetest`/`.recaptcha`/`.nc-captcha-wrap`/`#wechatVerify` 等）、URL `/login //captcha //verify`、
   HTML 关键词（`环境异常`/`频繁操作`/`请验证`）。可逐源扩展（§6）。
2. **可见本地 Chrome 人在回路登录循环（visible-Chrome human-resume login loop）**：
   headless 默认 → 拦截 → 弹**系统可见 Chrome 窗口** → 用户手动登录/解 captcha → 点 "完成" → resume。
   （attune 是桌面应用，用真实可见 Chrome 窗口，**不用 noVNC sidecar** — §4。）
3. **4 层会话捕获（4-layer session capture）**:
   - **L1** storage_state 等价物（cookies + localStorage + sessionStorage）；
   - **L2** IndexedDB dump；
   - **L3** UA / navigator / platform / languages 还原（会话保真，非伪造——L-6）；
   - **L4** WebGL VENDOR+RENDERER 指纹钉定（会话保真，非伪造——L-6）。
4. **vault 加密的会话存储**：捕获的会话 JSON 经字段级 AES-256-GCM(dek) 加密落 DB BLOB，
   与 `items.content` / `git_sources.token_ref_enc` 同模式（§1.4 / §4）。
5. **会话复用驱动 crawl→ingest**：复用会话头无人值守爬取该源会员页 → 经 `SourceConnector` → `ingest_document` 入 vault。
6. **逐会话 clear / expire 控制**：每源 "清除会话 / 重新登录" UI 按钮 + 会话 TTL 自动过期（默认 30 天可配）。
7. **逐源 consent gate（L-1）** + **保守速率限制（L-3）**。

### 2.2 OUT（本版本不做 / 永不做）

| OUT 项 | 理由 |
|--------|------|
| **自动凭据/密码注入** | **人登录，永不存密码**（L-2 / §1.4）。attune 永不持有用户密码。 |
| **自动 2FA / TOTP / QR 自动应答** | 用户在可见 Chrome 里手动完成一次（L-4）。community 的 `qrlogin` 自动轮询不移植为无人值守能力。 |
| **激进反 bot 指纹伪造** | **不**移植 `--disable-blink-features=AutomationControlled` / `navigator.webdriver` 主动 spoof 当 feature（L-6 / ToS 保守）。L3/L4 仅做会话保真重放。 |
| **noVNC / headless-server 模式** | attune 桌面用真实可见 Chrome 窗口，无 noVNC 暴露面（community 的 `0.0.0.0:6080` 无认证风险整类消除 — §4）。 |
| **无逐源同意的爬取** | 违反 L-1 跨切面铁律。 |
| **向 web AI 注入 context** | "不注入 web AI / 不走 MCP" 决策不变（§1.3 / §4）。 |
| **共享/转售/跨用户/跨设备自动同步会话** | L-2。会话与 vault 绑死。 |
| **把会话凭据并入 spec B `credentials` 统一表** | 形态/生命周期差异大（§1.4）；会污染 spec B 简单 secret 模型。 |
| **Python / Playwright / docker / Vue 任务中心移植** | 大栈不匹配，clean-room Rust port 重写概念（§4）。 |
| **跨源会话共用同一 Chrome / 同一 BrowserContext** | community 单 Chrome + mutex 的跨源 cookie 污染是 open question；attune 逐源隔离（§11 R4）。 |

### 2.3 后续版本（写死，不允许 silent scope creep）

- v.next-1：逐源 detector adapter 插件化（§6） + 行业源 adapter（pro，§4.3）。
- v.next-2：会话健康度主动探测（过期前提示重登）。
- v.next-3：多源并行（在保守速率+逐源隔离前提下）。
- v.next-4：spec B 统一账号 UI 真正聚合 login-assist 源条目（只读卡）。

---

## 3. 架构数据流

### 3.1 数据流图

```
                        ┌──────────────── attune-core (Rust) ────────────────┐
 用户添加源             │                                                     │
 (+ consent gate L-1)   │   LoginAssistSource (实现 SourceConnector)          │
        │               │        │ fetch_documents(sink)                      │
        ▼               │        ▼                                            │
 [login_assist_sources] │   ┌─ AdaptiveCrawler 状态机 ─────────────────┐      │
 DB 表(consent/TTL/      │   │ 1. headless 爬 (系统 Chrome via CDP)       │      │
  session_enc BLOB)      │   │ 2. InterceptDetector.scan(resp/dom/url/kw) │      │
        │               │   │      ├─ 无拦截 → 抓页 → RawDocument → sink  │      │
        │ load session   │   │      └─ 拦截!  → 升级 headed ↓             │      │
        ▼ (decrypt dek)  │   │ 3. 弹可见本地 Chrome 窗口 (visible)        │      │
   SessionStore ─────────┼──▶│ 4. await 用户手动登录/解 captcha + "完成"  │◀── 用户手动操作
   (4-layer)             │   │ 5. SessionCapture.capture() → 4 层         │      │
        ▲ save session   │   │ 6. 回到 headless，复用会话续爬             │      │
        │ (encrypt dek)  │   └────────────────────────────────────────────┘      │
        │               │        │ RawDocument(s)                              │
        │               │        ▼                                            │
        │               │   OutboundGate::enforce(BrowserCrawl, …)  (fail closed)│
        │               │        │                                            │
        │               │        ▼                                            │
        │               │   ingest_document() → parse→dedup→insert→embed→classify│
        │               │   ┌─ vault (AES-256-GCM, items.content) ─┐          │
        └───────────────┼───┤ session_enc BLOB + 爬取的 item 正文   │          │
                        │   └───────────────────────────────────────┘          │
                        └─────────────────────────────────────────────────────┘
```

### 3.2 DB tables（新增）

`login_assist_sources`（模式参照 `git_sources` / `webdav_remotes`）:

| 列 | 类型 | 说明 |
|----|------|------|
| `source_id` | TEXT PK | 稳定 ID |
| `name` | TEXT | 用户可读名 |
| `entry_url` | TEXT | 会员墙入口 URL |
| `detector_profile` | TEXT | 用哪套 detector（`default` / 命名 adapter，§6） |
| `consent_at` | INTEGER | L-1 同意时间戳（NULL = 未同意 = 禁止爬） |
| `tos_ack_version` | TEXT | 同意时的 ToS-ack 文案版本 |
| `session_enc` | BLOB | **4 层会话 JSON 经 dek AES-256-GCM 加密**（NULL = 尚未登录） |
| `session_captured_at` | INTEGER | 会话捕获时刻 |
| `session_ttl_secs` | INTEGER | 过期窗口（默认 30 天） |
| `rate_limit_ms` | INTEGER | inter-request 间隔（默认 ≥5000，不可 < 下限） |
| `daily_fetch_cap` | INTEGER | 每日抓取上限 |
| `corpus_domain` | TEXT | 同 git_sources（pro 跨域防污染） |

> `session_enc` 永远是密文 BLOB；明文会话**绝不**落盘/进日志/回显（§1.4）。
> 测试可读原始密文字节验证不含明文（参照 `git_sources` 的 `debug_raw_git_token_enc`）。

### 3.3 cache layers

会话 = 唯一缓存层（decrypt-once-per-crawl，crawl 期间内存持有，结束 zeroize）。爬取的页正文走既有
`indexed_files` 增量（`modified_marker`）去重，不额外加 cache。

---

## 4. 模块边界

### 4.1 涉及 crate / module / file

| 模块 | 角色 | 新增/复用 |
|------|------|----------|
| `attune-core/src/ingest/login_assist.rs`（新） | `LoginAssistSource: SourceConnector` + `AdaptiveCrawler` 状态机 | **新** |
| `attune-core/src/ingest/login_assist/detector.rs`（新） | `InterceptDetector` + 信号集 + per-source registry | **新** |
| `attune-core/src/ingest/login_assist/session.rs`（新） | `SessionState`（4 层） + capture/restore + TTL/expire | **新** |
| `attune-core/src/web_search_browser.rs` | 系统 Chrome 检测（`detect_system_browser` / `resolve_browser`）+ 速率限制范式 | **复用** |
| `attune-core/src/ingest/connector.rs` | `SourceConnector` / `RawDocument` / `SourceKind`（新增 `LoginAssist` variant，as_str=`login_assist`） | **复用 + 扩** |
| `attune-core/src/ingest/pipeline.rs` | `ingest_document`（唯一入库） | **复用** |
| `attune-core/src/crypto.rs` | `encrypt`/`decrypt`（Argon2id+AES-256-GCM dek） | **复用** |
| `attune-core/src/store/login_assist_sources.rs`（新） | DB 表 CRUD + session_enc 加解密（参照 `store/git_sources.rs`） | **新** |
| `attune-core/src/outbound_gate.rs` | `OutboundKind::BrowserCrawl`（新 variant，as_str=`browser_crawl`） + enforce | **复用 + 扩** |
| `attune-server/src/routes/login_assist.rs`（新） | REST: 增删源 / consent / 触发登录 / clear-session / 状态 | **新** |
| `attune-server/ui`（新视图/tab） | 源管理 + consent 勾选 + "登录" / "清除会话" 按钮（i18n zh+en，零硬编码 per CLAUDE.md i18n 铁律） | **新** |

### 4.2 与现有 capability 的关系（关键边界 — §7 设计约束 7）

本 capability **扩展**三件既有设施：
1. **chromiumoxide web-search**（`web_search_browser.rs`）——复用系统 Chrome 检测 + 速率限制 + `OutboundGate` 范式，**新增**"headed 人在回路 + 4 层会话"维度。
2. **SourceConnector ingest 框架**（`ingest/`）——新增第 7 种源（`LoginAssist`），与 LocalFolder/WebDav/Email/Rss/CloudDrive/GitRepo 平级，复用 `ingest_document` 唯一入库路径。
3. **凭据体系**（[[2026-06-17-suggestions-and-thirdparty-accounts]] 能力 B）——会话凭据是其一类（§1.4）；共享加密原语 + vault-locked 语义 + OutboundGate，但用专用表/API（不并入统一 `credentials` 表）。

**必须清晰区分（不得混淆）**：
- ✅ 本 capability = **crawl-login-gated-content-INTO-vault**（把会员内容采集进本地 vault）。
- ❌ **不是** inject-INTO-web-AI（向 web AI 站点 DOM 注入 context）——该方向 cleanup-r15 已删，**保持不做**（§1.3）。
- 数据方向相反：本 capability 是**入站采集**，注入是**出站污染 web 页面**。

### 4.3 OSS / pro 边界（§7 设计约束 6）

按 `oss-pro-strategy.md` v2 §4.3 判据（"对任何领域个人通用用户都有价值 → OSS"）:

| 部件 | 归属 | 论证 |
|------|------|------|
| login-assist 通用机制（状态机 / 默认 detector / 4 层会话 / vault 加密 / consent gate / 速率限制） | **OSS-base** | 通用登录墙采集对**任何**个人用户都有价值，且是 attune 既有 OSS chromiumoxide + SourceConnector 的**自然延伸**。与 web-search 同属 OSS。 |
| 行业特定源 adapter（法律数据库 / 专利局 / 行业期刊等的 detector + 抽取规则） | **attune-pro** | 行业绑定（law/patent/sales/tech/medical）按 OSS 边界规则一律在 attune-pro，不进 OSS。通过 §6 detector adapter 扩展点接入。 |

**The call**：通用 login-assist = **OSS-base**（本 spec 范围）；行业源 adapter = **pro**（后续，经 §6 扩展点）。

### 4.4 License-clean 论证（clean-room PORT — §7 设计约束 1）

- **源 = `/data/community` AdaptiveBrowser（Python/Playwright）**。本 spec 选择 **clean-room PORT**：在 attune 既有 chromiumoxide 之上**用 Rust 重新实现概念**，**不复制 Python 源码**。
- **复用的是设计 IDEA**（非代码）：intercept-detector 信号集、4 层会话设计、人在回路 resume 循环、headless↔headed 切换。
- **丢弃**：Python / Playwright / `sync_api`/`async_api`、docker、noVNC sidecar（桌面用可见 Chrome 窗口）、Vue 任务中心、SQLite 跨进程 mutex（attune 单进程，用进程内锁）、`qrlogin` 自动轮询。
- **明确不移植**：`navigator.webdriver` 主动 spoof / `--disable-blink-features=AutomationControlled` 作为反检测 feature（L-6）。
- **实现前确认 community 源 license**（subproject `LICENSE`），若为 Apache-2.0 保留一句 courtesy NOTICE（即使纯 clean-room 概念复用亦可）。

---

## 5. API 契约

### 5.1 REST endpoints（kebab-case，前缀 `/api/v1/`）

| Method | Path | 说明 |
|--------|------|------|
| `POST` | `/api/v1/login-assist/sources` | 创建源。body 必含 `consent: true` + `tos_ack_version`，否则 `400 consent-required`（L-1）。 |
| `GET` | `/api/v1/login-assist/sources` | 列源 + 会话状态（`session: none/active/expired`），**永不返回会话明文**。 |
| `DELETE` | `/api/v1/login-assist/sources/{id}` | 删源（连带 clear 会话）。 |
| `POST` | `/api/v1/login-assist/sources/{id}/login` | 触发人在回路登录（弹可见 Chrome，await 用户 "完成"）。 |
| `POST` | `/api/v1/login-assist/sources/{id}/resume` | 用户点 "完成"，捕获并加密存会话。 |
| `POST` | `/api/v1/login-assist/sources/{id}/clear-session` | 清除该源会话（L-2 用户控制）。 |
| `POST` | `/api/v1/login-assist/sources/{id}/crawl` | 触发一次会员内容爬取（经速率限制 + OutboundGate）。 |

### 5.2 typed schema（核心结构）

```rust
pub struct LoginAssistSourceInput {  // 写入用（consent 必填）
    pub name: String,
    pub entry_url: String,
    pub detector_profile: String,     // "default" | <adapter-name>
    pub consent: bool,                // 必须 true，否则拒绝创建（L-1）
    pub tos_ack_version: String,
    pub rate_limit_ms: u64,           // clamp 到 >= MIN_RATE_LIMIT_MS（L-3）
    pub session_ttl_secs: u64,
    pub corpus_domain: Option<String>,
}
pub enum SessionStatus { None, Active { captured_at: i64 }, Expired }  // 对外永不含会话值

struct SessionState {                 // 内部，永不序列化进 API 响应/日志
    l1_storage_state: String,         // cookies+local/session storage JSON
    l2_indexed_db: String,
    l3_navigator: NavigatorProfile,   // UA/platform/languages（会话保真，非伪造）
    l4_webgl: WebglProfile,           // VENDOR+RENDERER（会话保真，非伪造）
    schema_version: u8,               // = 2；预留迁移（§10）
}
```

### 5.3 CLI（可选）

`attune login-assist add|list|login|clear|crawl <source>` —— 同 REST 语义，登录仍弹可见 Chrome。

---

## 6. 扩展点 / 插件接口

- **逐源 detector adapter**：`trait InterceptDetectorAdapter { fn signals(&self) -> DetectorSignals; }`，
  按 `detector_profile` 名注册（对应 community 的 per-source registry）。OSS 内置 `default`；
  行业 adapter（pro）经此接口接入而不改 core（§4.3）。
- **会话 schema 版本化**：`SessionState.schema_version` 预留升级位（§10）。
- **新会员源接入路径**：注册一个 `detector_profile` + 配 `entry_url` + 过 consent gate，无需改 `AdaptiveCrawler`。
- **未来 source adapter**（后续版本）：抽取规则按源定制（如把某站 DOM → `RawDocument.title/content`）。

---

## 7. 错误 + 边界 case

| 场景 | 行为 | 错误码（kebab） |
|------|------|------|
| 创建源未带 consent | 拒绝创建 | `400 consent-required`（L-1） |
| rate_limit_ms < 下限 | clamp 到下限（不报错，记日志） | —（L-3） |
| 爬取时 vault 锁定 | `OutboundGate` 拒绝（会话需解密）；不出网 | `401 vault-locked` |
| 会话过期（超 TTL） | 状态 = `Expired`，自动触发"请重新登录"，不静默用过期会话 | `409 session-expired` |
| 拦截后用户取消登录 | 中止本次 crawl，不入库部分数据，graceful 退出 | `crawl-aborted-by-user` |
| 系统无 Chrome | login-assist 整体 disable（同 web-search 的 `NeedsDownload`） | `503 browser-unavailable` |
| 会话捕获失败（页面崩 / CDP 断） | 不写半截会话；保留旧会话（若有）；记日志（不记会话值） | `session-capture-failed` |
| 单页抓取失败 | 吞掉记日志、继续下一页（per `SourceConnector` 约定） | — |
| 源级致命（连不上 / 鉴权墙变形） | 返回 Err 终止该源 | `crawl-source-fatal` |
| daily_fetch_cap 触顶 | 停止本日爬取，下次窗口续（L-3） | `daily-cap-reached` |

**graceful degradation**：任何会话/网络失败都**不阻塞**其它源；OutboundGate 失败 = 不出网（fail closed）。

---

## 8. 成本契约（§7 设计约束 5）

映射 CLAUDE.md §成本与触发契约三层:

| 阶段 | 层级 | 资源 | 触发 |
|------|------|------|------|
| headless 爬取 + detector 扫描 + 页解析 | 🆓 **零成本** | CPU，本地 Chrome，**零 API** | 建库阶段自动（同 RSS/WebDAV 周期采集） |
| 4 层会话捕获 / 重放 + embedding 入库 | ⚡ **本地算力** | 本地 Chrome / GPU embedding | 建库阶段自动；顶栏 "暂停后台任务" 可停 |
| **人在回路登录（弹可见 Chrome）** | ⚡ **本地 + 用户时间** | 本地 Chrome + 用户手动操作 | **必须用户显式触发**（点 "登录"）——属交互成本，UI 明示 "需你手动登录一次" |
| LLM 分析爬取到的内容（若用户后续在 chat 问） | 💰 **时间/金钱** | 既有 chat 路径，本 capability 不引入 | 用户在 chat 显式触发（不在本 capability 范围） |

**归属**：本 capability = **ingest-source connector**（与 WebDAV/RSS/Git 同层），喂 vault。爬取本身永远停在
🆓/⚡（零 API）——**绝不**在采集阶段升级到第三层 LLM（per §成本契约规则 1）。UI 在源卡片标注
"🆓 本地爬取 · 需手动登录一次"。

---

## 9. 测试矩阵（6 类下限对应）

| 类型 | 用例 | 工具 |
|------|------|------|
| **Golden / happy** | ≥10 真实拦截 HTML/响应 fixture（401/403/429 + geetest/recaptcha/wechatVerify DOM + /login 重定向 + 关键词） → detector 命中断言 | fixture YAML/HTML |
| **属性测试** | ≥3: detector 对随机 HTML 不 panic；会话 capture→encrypt→decrypt→restore round-trip 不变；rate_limit clamp 单调 | `proptest` |
| **边界 case** | ≥5 `#[test]`: 空 HTML / 超大页 / 无 detector 命中 / TTL=0 立即过期 / 会话 schema_version 边界 | inline `#[cfg(test)]` |
| **异常 / 错误** | ≥3: 未 consent 创建拒绝 / vault-locked 爬取被 OutboundGate 拒 / 会话过期不复用 | YAML `expected_error` |
| **集成 E2E** | ≥1 subprocess: 假登录墙 fixture server → 拦截 → (mock 人工 resume) → 捕获会话 → 复用爬第二页 → 入 vault | `tests/login_assist_subprocess.rs` |
| **回归 fixture** | 每修 1 bug 加 1 永久 fixture（detector 漏报 / 会话泄漏回归等） | golden set 永久 |

**对抗 / 安全用例（L-1..L-6 强制）**:
1. **会话泄漏审计**：grep 全代码路径，断言会话明文**绝不**出现在 日志 / API 响应 / 错误信息 / 序列化（参照 `git_sources` 的 `debug_raw_git_token_enc`：DB 里只有密文 BLOB）。
2. **log-scrub**：注入含会话值的 crawl，断言所有日志行不含 cookie/token 子串。
3. **consent gate 强制**：无 consent 的源**不能**被 `crawl` endpoint 触发（返回 `consent-required`）。
4. **vault 加密验证**：`session_enc` BLOB 字节不含任何明文会话字符串。
5. **速率下限**：尝试设 `rate_limit_ms=0` → 被 clamp 到下限，断言实际间隔 ≥ 下限。
6. **过期不复用**：超 TTL 会话**不被**当 active 用，强制走重登。
7. **SSRF / 越权**：`entry_url` 不得指向 `127.0.0.1`/内网/`file://`/非 http(s) scheme（防把 login-assist 当 SSRF 跳板抓本机服务）；crawl 仅在源 `entry_url` 同源/同站范围内（不跟外站重定向窃取无关 cookie）。

**通过判据**（deterministic 部分）：detector / 会话 round-trip / 加密 / consent / 速率 / 过期 / SSRF — pass rate = **1.00**。
（本 capability 无 LLM agent，无 F1≥0.85 档。）

---

## 10. 向后兼容

- **新可选源类型**：`SourceKind::LoginAssist` 是**新增** variant（as_str=`login_assist`），老库无该源 = 行为不变；不动现有 6 种源任何路径。
- **DB 迁移**：新增 `login_assist_sources` 表（additive migration），不改既有表 schema。
- **会话 schema 版本化**：`SessionState.schema_version = 2`，预留 `migrate_session_v1_to_v2` 升级位（本版本无 v1 历史数据，纯预留）。
- **老 client 行为**：不认识 `/api/v1/login-assist/*` 的老前端 → endpoint 不存在即 404，不影响既有 tab。
- **OutboundKind 扩展**：新增 `BrowserCrawl` variant 是 additive，既有 6 个 kind 的 enforce 行为不变（含 `as_str()` 与 §301 测试需同步加分支断言）。
- **与 spec B 解耦**：会话凭据用专用表，spec B 的 `credentials` 表落地与否、先后顺序均不影响本 spec（§1.4）。

---

## 11. 风险登记

| # | 风险 | 等级 | 缓解 |
|---|------|------|------|
| R1 | **ToS / 法律**（#1 风险）：自动登录+爬第三方会员站违反其 ToS | **高** | 跨切面铁律 L-1..L-6：逐源 consent gate / 仅用户自有会话 / 保守速率 / 人在回路=用户行为 / scope+免责声明 / 保守反检测。RELEASE Gate-4 明示 Known Limitation。 |
| R2 | **会话 = 在用凭据落盘**（§1.4） | **高** | vault 字段级 AES-256-GCM(dek)，与 `git_sources.token_ref_enc` 同模式；0600+vault；永不 log/commit/echo；clear+TTL 自动过期；测试断言密文。**优于 community 的仅 gitignore+chmod。** |
| R3 | **反检测被定性为"规避"** | **高** | L-6：**不**移植主动 spoof feature；L3/L4 仅会话保真重放；人手动解 captcha；RELEASE 明确"非反检测工具"。 |
| R4 | 跨源 cookie 污染（community open question） | 中 | attune **逐源隔离**会话（每源独立 `session_enc` + 独立 BrowserContext），不共享单 Chrome（§2-OUT）。 |
| R5 | **chromiumoxide CDP 能力未落实**（当前不在 Cargo.toml 硬依赖，web-search 实际走 reqwest 而非 CDP） | **中-高** | headed 人在回路 + 4 层捕获需真 CDP 驱动。**plan 首个 gate = CDP PoC**：验证 chromiumoxide 当前 Chrome 兼容性；若反序列化不兼容（web_search_browser.rs 注释记载的坑），退而用系统 Chrome 的 `--remote-debugging-port` + 直连 CDP。**实现前必须 PoC 验证，未过不进 impl。** |
| R6 | 会话过期导致静默爬空 | 中 | TTL 检测 → `Expired` 状态 → 主动提示重登，**不静默用过期会话**（§7）。 |
| R7 | 可见 Chrome 窗口被用户误关 | 低 | resume 超时 → `crawl-aborted-by-user`，保留旧会话（若有），graceful。 |
| R8 | 速率过快触发源封号（伤用户自己账号） | 中 | L-3 保守默认 ≥5s + 并发=1 + daily cap；不可调到 0。 |
| R9 | 并发/锁：爬取 worker 与 server 锁序 | 中 | 遵循 CLAUDE.md lock ordering（`fulltext→vectors→vault`）；crawl worker 经 `enqueue_reindex` 间接入库，不自取 vectors/fulltext 锁。 |
| R10 | **SSRF / 内网越权**（entry_url 指本机/内网） | 中 | §9 用例 7：entry_url scheme/host 校验，禁 localhost/内网/file://；crawl 限同站，不跟外站重定向。 |

---

## 附录 A — 评审流程（per §3.1）

1. 本 spec 落盘（本文件，docs/superpowers/ gitignored，git add -f 入库）。
2. **spec 评审（用户批准）** ← 当前停在此步。合入后删除 `2026-06-10-browser-login-assist-session-capture.md`（同主题，§3.2 单份）。
3. 批准后 `superpowers:writing-plans` 出 implementation plan（含 R5 chromiumoxide CDP PoC 作为首个 gate）。
4. plan 评审过 → implementation。
5. 三层（spec/plan/impl）任一变更上层同步。

## 附录 B — 与 community 源的差异速查（clean-room PORT 对照）

| community（Python / Playwright） | attune（clean-room Rust port） |
|------------------------------|------------------------------|
| Playwright `async_api`/`sync_api` | chromiumoxide / CDP（系统 Chrome） |
| noVNC + Xvfb + x11vnc docker sidecar | **可见本地 Chrome 窗口**（桌面应用，无 noVNC） |
| `storage_state_v2.json` chmod 0600 + gitignore | vault 字段级 **AES-256-GCM dek** 加密 BLOB |
| `AutoLoginManager` 存 storage_state 进 vault（已加密，正面参照） | 同向，扩展为 4 层会话 + per-source 表 + TTL |
| `qrlogin` 自动轮询扫码 | **不移植**（L-4 人在回路手动） |
| SQLite 跨进程 mutex + 心跳 | 进程内锁（attune 单进程） |
| Vue 任务中心 + briefing FastAPI | attune-server REST + 内嵌 Web UI tab |
| `navigator.webdriver` spoof / `--disable-blink-features` 作为能力 | **不移植**为 feature（L-6）；L3/L4 仅会话保真重放 |
| 无 consent gate | 逐源 consent gate（L-1） |
| 无会话过期/清除 | TTL 自动过期 + 用户 clear（L-2） |
