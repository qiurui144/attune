# Web Plugin as Knowledge Source — attune SSOT

> 状态: Draft v1（2026-05-28 立案）
> 触发: 用户 v1.0 暴击 #9「我们的 web 插件作为知识库来源也需要」
> Spec 责任人: extension / ingest 子领域
> 关联代码: `extension/` + `rust/crates/attune-core/src/{ingest/, capture/, store/}` + `rust/crates/attune-server/src/routes/{ingest.rs, upload.rs}`
> 关联前置: G1 浏览信号 schema 见 `rust/crates/attune-core/src/capture/`（w3-batch-b 设计 spec 已实现并归档）+ `docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md`（隐私 SSOT）

---

## 1. 目标定位

**用户痛点 / Why now**

attune Chrome extension 历史曲折:

- v0.1-v0.4:**双向** — 既向 AI 网站(ChatGPT/Claude/Gemini)DOM **注入**前缀,又**捕获**对话回库
- 2026-04-12 cleanup-r15:产品方向转「**内置 Chat + RAG**」,**`content/injector.js` 已删**(per CLAUDE.md「已实现模块 → Chrome 扩展 → 注」)
- v0.5+ 至今:extension 沦为**单向输入**职责,但**没有 spec 明示** — 文档散在 manifest.json / worker.js / browse_capture.js,用户不清楚 extension 当前到底干什么

**核心 reframe(本 spec 明确)**:

> attune Chrome extension = **knowledge ingest 客户端**(输入源),不是 **inject 中间人**(输出注入)。它是 attune server 的多种 ingest source 之一(文件夹 watcher / paste / upload 之外的浏览器侧采集源)。

**产品 positioning 对齐**

- CLAUDE.md「三产品矩阵」:attune 是个人通用知识库 — extension 是**采集渠道**,无行业绑定
- CLAUDE.md「成本契约」:extension 捕获属于「建库阶段」— 必须停在「能被搜到 + 150 字摘要」,不允许触发 LLM 深度分析
- CLAUDE.md「隐私默认」:extension 默认 opt-out(per browse_capture.js HARD_BLACKLIST),全部对齐 Spec 1 (privacy logic strategy)

**为何重要**:

1. extension 是用户**最高频接触**的入口(每次开浏览器都见)— 不明示职责会带来「这玩意儿到底干什么」的信任崩塌
2. 现状代码已有 5 种 capture source(对话/选中/页面/浏览信号/sidepanel 上传),没有 SSOT 列清楚 — 后续加 RSS / bookmark / pocket 等会乱
3. v1.1+ 计划加更多 source(per CLAUDE.md「v1.1 spec next-wave」),先把契约钉死

---

## 2. 范围边界

**做(本 spec 责任)**:

- 明确 extension 的**单向输入**职责契约 — 列穷举的 capture source 与 message protocol
- 现有 5 种 source 行为 SSOT:对话捕获(AI 网站)/ 右键选中文本 / 主动保存当前页 / 浏览状态信号(G1) / sidepanel 上传
- Chrome ext → attune server 的 message format(Manifest V3 json schema)
- **明确 extension 不再向 AI 网站 DOM 注入**(per cleanup-r15 已删 injector.js)— 文档化避免 reviewer 误以为是 regression
- 后续 source 扩展点(RSS reader / Pocket import / bookmark sync / OCR-from-screenshot)— 留接口不实施
- 现状 manifest.json 权限的最小化 audit

**不做(留给其他 spec 或 v.x+)**:

- 重新引入 prompt injection(此为产品方向反向移动,本 spec 明确**禁止**)
- 自动 OCR 截图入库(v1.2+,需要 vision model 路径,与 VLM provider spec 关联)
- iOS / Android 移动端扩展(平台优先级 P3,v2.x)
- Safari / Firefox 扩展(平台优先级矩阵未列,v2.x)
- 浏览器历史全量同步(过度入侵,需用户主动 export)
- 实时多设备同步 extension settings(WebDAV 已是用户主权同步,extension 暂用 chrome.storage.sync)

**v1.0.x 实施口径**:

| v 版本 | 增量 |
|---|---|
| v1.0.x 本 spec | 仅文档 + manifest 权限 audit,不动 capture 代码 |
| v1.1.0 | RSS / Atom feed 入库(per CLAUDE.md「v1.1+」)|
| v1.2.0 | Pocket / Instapaper bookmark import |
| v2.x | 跨浏览器 / 移动端 |

---

## 3. 架构数据流

### 3.1 ASCII 数据流图(extension → attune server 单向)

```
┌────────────── Chrome Browser ──────────────┐         ┌── attune server (localhost:18900) ──┐
│                                              │         │                                       │
│  ① AI 网站对话捕获                            │         │                                       │
│     (chatgpt.com / claude.ai / gemini.*)    │         │                                       │
│     content/capture.js (MutationObserver)   │         │                                       │
│           │                                  │         │                                       │
│           │ MSG.CAPTURE_CONVERSATION         │         │                                       │
│           ▼                                  │         │                                       │
│  ② 右键选中文本                               │         │                                       │
│     contextMenus("npu-save-selection")      │         │                                       │
│           │                                  │         │                                       │
│           │ MSG.SAVE_SELECTION               │         │                                       │
│           ▼                                  │         │                                       │
│  ③ 主动保存当前页(action / contextmenu)       │         │                                       │
│     sidepanel "Save Page" 按钮 OR           │         │                                       │
│     content/index.js extractor              │         │                                       │
│           │                                  │         │                                       │
│           │ MSG.CAPTURE_PAGE                 │         │                                       │
│           ▼                                  │         │                                       │
│  ④ 浏览状态信号 (G1)                          │         │                                       │
│     content/browse_capture.js                │         │                                       │
│     (whitelist + HARD_BLACKLIST + opt-out)  │         │                                       │
│     dwell / scroll / copy / visit            │         │                                       │
│           │                                  │         │                                       │
│           │ "BROWSE_SIGNAL" → batched queue  │         │                                       │
│           ▼                                  │         │                                       │
│  ⑤ Sidepanel 上传                            │         │                                       │
│     sidepanel/pages/FilesPage.jsx           │         │                                       │
│     drag-drop / file picker                  │         │                                       │
│           │                                  │         │                                       │
│           │ FormData multipart               │         │                                       │
│           ▼                                  │         │                                       │
│     ┌─────────────────────────────────┐     │         │                                       │
│     │ background/worker.js (router)   │     │  HTTPS  │   ┌─────────────────────────────┐    │
│     │  + dedup (djb2, 1h TTL)          │─────┼─────────▶│ /api/v1/ingest               │    │
│     │  + browse_queue (30s flush)      │     │         │   /api/v1/upload              │    │
│     │  + health check (30s)            │     │         │   /api/v1/browse_signals      │    │
│     └─────────────────────────────────┘     │         │   /api/v1/status/health       │    │
│                                              │         │   └─────────────┬─────────────┘    │
│  ── 不存在的方向 ──                          │         │                 │ ingest::pipeline  │
│   ✗ 任何 attune → AI 网站 DOM 注入            │         │                 ▼                   │
│   ✗ 任何 attune → 用户其他网页内容修改         │         │      vault encrypt + index +       │
│   ✗ extension 不主动拉 attune server data    │         │      embedding + classify           │
│                                              │         │                                     │
└──────────────────────────────────────────────┘         └─────────────────────────────────────┘
```

**关键不变量**:

- **单向** — extension 永远向 server 推数据,**不读** server 数据(除 `health` / `status` 探活)
- **本地** — extension 只连 `http://localhost:18900` / `http://127.0.0.1:18900`,**禁止**连任何远端
- **去重** — worker djb2 hash + 1h TTL,避免重复入库
- **无 inject** — 不向任何用户访问的网页注入 prompt / 弹窗 / 修改内容

### 3.2 DB tables(server 端,extension 写入路径)

| Table | 来源 source | 通过 endpoint |
|---|---|---|
| `items` (source_type='conversation') | ① AI 对话 | POST /api/v1/ingest |
| `items` (source_type='selection') | ② 右键选中 | POST /api/v1/ingest |
| `items` (source_type='webpage') | ③ 主动保存页 | POST /api/v1/ingest |
| `items` (source_type='upload') | ⑤ sidepanel 上传 | POST /api/v1/upload (multipart) |
| `browse_signals` | ④ 浏览信号 | POST /api/v1/browse_signals |

**加密**:所有 table 走 dek_db AES-256-GCM(per Spec 1 隐私 SSOT §3.2)。

### 3.3 网络边界

| 链 | 协议 | 加密 |
|---|---|---|
| extension → server | HTTP localhost(loopback) | OS-level 进程隔离,**不需要** TLS — 但允许用户配 HTTPS 自签证书(企业场景) |
| extension → AI 网站 | 用户浏览器原生 HTTPS | 不在 attune 控制 |
| server → 外部 | 见 Spec 1 (privacy logic strategy) | 5 个出网点 |

---

## 4. 模块边界

### 4.1 extension 侧文件

| 文件 | 职责 | LOC(2026-05-28)|
|---|---|---|
| `extension/manifest.json` | Manifest V3 权限声明 + content_scripts 注册 | 67 |
| `extension/src/background/worker.js` | 消息路由 + dedup + browse queue + health check | 368 |
| `extension/src/content/index.js` | AI 网站 content script entry(detector + capture + indicator)| ~150 |
| `extension/src/content/capture.js` | AI 对话捕获(MutationObserver + 2s debounce + djb2 dedup) | 160 |
| `extension/src/content/detector.js` | 平台适配器(ChatGPT/Claude/Gemini 选择器)| ~80 |
| `extension/src/content/indicator.js` | 4 状态 UI 指示器 | ~120 |
| `extension/src/content/browse_capture.js` | G1 通用浏览状态采集(whitelist + 黑名单 + opt-out)| 141 |
| `extension/src/shared/messages.js` | 统一消息类型 MSG.* | 33 |
| `extension/src/shared/api.js` | 后端 API client | 92 |
| `extension/src/shared/storage.js` | chrome.storage 包装 | — |
| `extension/src/popup/Popup.jsx` | 工具栏 popup(连接状态 / 统计 / 总开关)| — |
| `extension/src/popup/Privacy.jsx` | popup 内嵌隐私简卡 | — |
| `extension/src/options/Options.jsx` | options 全页面(后端地址 / 排除域名 / 测试连接)| — |
| `extension/src/sidepanel/App.jsx` | sidepanel 入口(Search / Timeline / Files / Status) | — |
| `extension/src/sidepanel/pages/{Search,Timeline,Status,Files}Page.jsx` | sidepanel 四 tab | — |

**已删除文件(per cleanup-r15,2026-04-12)**:

- `extension/src/content/injector.js` — 前缀 inject 已弃用
- `extension/src/content/inject.js` / `injector_helper.js` — 相关 helper

**禁止再引入**:任何 `extension/src/content/inject*` / `dom_modify*` / `prompt_helper*` 形态的文件。

### 4.2 server 侧 ingest 路径

| crate / module / file | 职责 |
|---|---|
| `attune-server/src/routes/ingest.rs` | POST /ingest 端点 |
| `attune-server/src/routes/upload.rs` | POST /upload multipart |
| `attune-server/src/routes/browse_signals.rs` | POST/GET/DELETE /browse_signals |
| `attune-core/src/ingest/mod.rs` | pipeline: parse → chunk → encrypt → index → embedding queue |
| `attune-core/src/capture/` | conversation/selection/webpage 共用解析逻辑 |
| `attune-core/src/store/` | DB CRUD + signal 写入 |

---

## 5. API 契约

### 5.1 Chrome extension manifest v3

```json
{
  "manifest_version": 3,
  "name": "Attune 私有 AI 知识伙伴",
  "version": "0.6.x",
  "incognito": "not_allowed",            // 隐身窗口硬阻断(R04 P1-4)
  "permissions": [
    "storage",                            // chrome.storage.local / .sync
    "sidePanel",                          // 侧边栏 UI
    "activeTab",                          // 主动保存当前页时拿 tab.url
    "tabs",                               // toggle 状态广播 + 多 tab 协同
    "contextMenus",                       // 右键「保存到知识库」
    "webNavigation"                       // browse_capture 跨 SPA 路由检测
  ],
  "host_permissions": [
    "http://localhost/*",                 // attune server loopback
    "http://127.0.0.1/*",
    "<all_urls>"                          // browse_capture 通用页;受 HARD_BLACKLIST 防御
  ],
  "content_scripts": [
    { "matches": ["https://chatgpt.com/*", "https://claude.ai/*", "https://gemini.google.com/*"],
      "js": ["dist/content/index.js"], "run_at": "document_idle" },
    { "matches": ["<all_urls>"],
      "exclude_matches": ["*://chatgpt.com/*", "*://claude.ai/*", "*://gemini.google.com/*",
                          "*://*/login*", "*://*/signin*",
                          "*://*1password*/*", "*://*lastpass*/*", "*://*bitwarden*/*",
                          "*://accounts.google.com/*"],
      "js": ["dist/content/browse_capture.js"], "run_at": "document_idle" }
  ]
}
```

**权限最小化原则**:本 spec amend 后,任何新增 permission(`webRequest` / `cookies` / `history` / `bookmarks` 等)必须 spec amendment + user 评审。

### 5.2 extension → server 消息(JSON schema)

#### MSG.CAPTURE_CONVERSATION → POST /api/v1/ingest

```json
{
  "title": "string (≤200 char, 自动截断)",
  "content": "string (≤500KB)",
  "source_type": "conversation",
  "url": "https://chatgpt.com/c/abc",
  "domain": "chatgpt.com",
  "metadata": {
    "platform": "chatgpt | claude | gemini",
    "captured_at": "ISO8601",
    "conversation_id": "string",
    "message_role": "user | assistant",
    "message_index": 0
  }
}

// 响应
{ "status": "ok | duplicate", "id": "uuid" }
```

#### MSG.SAVE_SELECTION → POST /api/v1/ingest

```json
{
  "title": "string (selectionText 前 100 char)",
  "content": "string (selectionText 全量, ≤100KB)",
  "source_type": "selection",
  "url": "string",
  "domain": "string",
  "metadata": { "source": "context_menu", "captured_at": "ISO8601" }
}
```

#### MSG.CAPTURE_PAGE → POST /api/v1/ingest

```json
{
  "title": "string (document.title or URL fallback)",
  "content": "string (Readability 主体提取, ≤2MB)",
  "source_type": "webpage",
  "url": "string",
  "domain": "string",
  "metadata": {
    "captured_at": "ISO8601",
    "lang": "zh | en | ...",
    "word_count": 1234
  }
}
```

#### BROWSE_SIGNAL → batched → POST /api/v1/browse_signals

```json
{
  "signals": [
    {
      "url": "string",
      "domain": "string",
      "title": "string",
      "dwell_ms": 12345,
      "scroll_pct_max": 80,
      "copy_count": 2,
      "visit_count": 1,
      "captured_at": "ISO8601"
    }
  ]
}
```

(详细 schema 见 `rust/crates/attune-core/src/capture/` — G1 浏览信号已实现)

#### sidepanel files upload → POST /api/v1/upload (multipart)

```
Content-Type: multipart/form-data
fields:
  file: <binary>
  source_type: "upload"
  metadata: {"client": "extension-sidepanel", ...} (JSON string)

// 响应
{ "status": "ok | duplicate", "id": "uuid", "size_bytes": 12345 }
```

### 5.3 内部消息类型(extension 进程内)

per `extension/src/shared/messages.js`:

| 类型 | 方向 | 用途 |
|---|---|---|
| `CAPTURE_CONVERSATION` | content → worker | AI 对话入库 |
| `SAVE_SELECTION` | worker → worker(contextMenus) | 右键选中入库 |
| `CAPTURE_PAGE` | sidepanel → worker → content | 主动保存当前页 |
| `GET_PAGE_CONTENT` | worker → content | 取页面 readable content |
| `BROWSE_SIGNAL` | browse_capture → worker | 浏览信号入队 |
| `SUMMARIZE_AND_SAVE` | content → worker | 对话摘要后入库(legacy,v1.x 保留)|
| `GET_STATUS` | popup/sidepanel → worker | 后端连接状态查询 |
| `GET_SETTINGS` / `SETTINGS_UPDATED` | options → worker | 设置同步 |
| `OPEN_SIDEPANEL` | content → worker | 跳转 sidepanel |
| `SEARCH` / `GET_ITEMS` | sidepanel → worker → server | sidepanel 查询 |
| `PREFETCH` / `SEARCH_RELEVANT` | **deprecated**(injector 时代),保留 stub 给 API 兼容 | — |
| `TOGGLE_INJECTION` | **deprecated** 名义保留,实际无 inject 路径 | — |

**dep cleanup 计划**:`PREFETCH` / `SEARCH_RELEVANT` / `TOGGLE_INJECTION` 在 extension v0.8 删除(给一个 release 周期通知)。

---

## 6. 扩展点 / 插件接口

### 6.1 新 capture source 引入流程

任何新 source 必须:

1. 起名 `source_type` 字符串(snake_case,例:`rss` / `pocket` / `bookmark`)
2. 在 `extension/src/content/` 或 `src/sidepanel/` 加 source-specific JS
3. 复用现有 `MSG.*` 类型 OR 加新类型到 `shared/messages.js`
4. **必复用** POST /api/v1/ingest 端点(不允许新 endpoint),通过 `source_type` 区分
5. 加 `extension/src/options/Options.jsx` 开关 — **默认 false**
6. amend 本 spec §5.2 + §5.3
7. integration test 真跑(`tests/extension_<source>.spec.js`)

### 6.2 v1.1+ 计划 source

| source | 形态 | 默认 | 触发位置 |
|---|---|---|---|
| `rss` | RSS/Atom feed URL 列表 | opt-in | options page → "RSS Feeds" tab |
| `bookmark_import` | 一次性 import `chrome.bookmarks` | 用户按钮 | sidepanel → "Import Bookmarks" |
| `pocket` | Pocket OAuth → 文章 | opt-in(需 OAuth)| options page |
| `instapaper` | 同 Pocket | opt-in | options |
| `screenshot_ocr` | 截图 → 本地 OCR → 入库 | 用户右键 | contextMenus + browser action |

**触发 source 注册时同时改 server 端**:

- `attune-server/src/routes/ingest.rs` 已支持任意 `source_type` 透传(不需要改路由)
- `attune-core/src/ingest/mod.rs` 的 pipeline 需要识别新 source_type 决定 parser(`rss` → atom parser,`pocket` → markdown 等)

### 6.3 attune-pro plugin 接 web source

attune-pro 行业 plugin 可以**复用** OSS extension(不再开发独立 extension)。例:

- law-pro 用户访问中国裁判文书网 → 「保存到知识库」走标准 MSG.CAPTURE_PAGE → server 端 law-pro plugin 的 classifier 识别 source_type='webpage' + domain='wenshu.court.gov.cn' → 自动归 case_law project

→ plugin **不在 extension 侧加代码**,只在 server 端 classifier 上区分。保持 extension 通用。

---

## 7. 错误处理 + 边界 case

| 边界 | 行为 |
|---|---|
| server 不在(端口未启动) | worker `health()` 失败 → backendOnline=false → indicator 红色 → 捕获静默 buffer 到 `chrome.storage.session.dedup` 等下次 |
| dedup hash 冲突(djb2 假阳)| 假阳概率 ~ 2^-32;真冲突时丢一条,产品可接受 |
| content > 500KB(超 ingest 限) | server 返 413 → worker 记 error → 用户看 sidepanel status page 红条 |
| 截选文本 > 100KB | content/index.js 客户端裁剪 + UI 提示 |
| webpage 提取无 readable content | server 返 400 `page-no-readable-content` → 不重试 |
| browse_signals 队列爆(>10K)| 30s flush 间隔 + 单次 batch ≤50 + 队列上限 1000;超限 oldest-drop |
| extension 在 incognito 加载 | `manifest.json incognito=not_allowed` 阻断;`browse_capture.js` 二次 `chrome.extension.inIncognitoContext` 检查兜底 |
| AI 网站 DOM schema 变(选择器失效)| `detector.js` 平台适配器返 null → indicator 灰色「offline 平台不支持」→ 不 panic |
| 用户在 HARD_BLACKLIST 网站(银行 / 政府 / 密码管理器)| browse_capture.js 早 return,**0 网络 traffic** |
| Manifest V3 service worker 被 Chrome 休眠 | health check 30s 周期 + onStartup listener 复活 |
| user 切换 attune server 地址(LAN K3)| options page → `SETTINGS_UPDATED` → worker reload baseUrl,无需重启 extension |
| sidepanel 拖上传 100MB 大文件 | server `upload.rs` 流式接 + 写盘前算 hash;前端进度条 |
| dedup table 持久化失败(chrome.storage.session 满)| 失败时清 oldest 50%,记 console warn |
| Adversarial:恶意网页伪造 `MSG.CAPTURE_*` 给 worker | extension API 设计上,网页 JS **不能**直接 `chrome.runtime.sendMessage` 到自己以外的 extension;同源策略保护 |

---

## 8. 成本契约

| 操作 | 成本层 | 触发 |
|---|---|---|
| AI 对话 capture | 🆓 零成本(client side dedup + server side parse + encrypt) | 自动,无需 LLM |
| 选中保存 | 🆓 零成本 | 用户右键 |
| 主动保存当前页 | 🆓 零成本(Readability 本地解析) | 用户按钮 |
| Browse signals batch flush | 🆓 零成本(30s 一次 HTTP POST,字节级) | 自动 |
| Sidepanel files upload | 🆓 零成本 ingest;⚡ 后台 embedding 本地算力 | 用户拖拽 |
| Sidepanel search | 🆓 零成本(本地 BM25+HNSW) | 用户键入 |
| **不允许**:capture 时跑 LLM 摘要 | ❌ 违反「建库阶段不升级第三层」契约 |

**对应 CLAUDE.md「成本感知」节**:extension 全部捕获 source = 建库阶段,**永远 stay at 🆓+⚡ 层**,**禁止**触发 💰 token 出网。如需 AI 分析,等用户在 Chat tab 主动开口。

**UI 显示**:popup 内显示「今日捕获 N 条 / 0 token 花费」,reinforce 零金钱成本承诺。

---

## 9. 测试矩阵

| 类型 | 下限 | 例子 |
|---|---|---|
| **Golden case** | ≥10 真实 | chatgpt 对话 5 例 + claude 3 例 + gemini 2 例 → ingest hash 稳定 |
| **Property test**(去重) | proptest 1000 case | 同 content N 次 capture → server 仅 1 条 item |
| **Boundary** | ≥5 case | content 0 byte / 1 byte / 100K / 500K / 600K(超限) |
| **Error** | ≥8 case | server down / 401 / 413 / 500 / network DNS fail / TLS fail(企业)/ Chrome 限频 / quota 满 |
| **Adversarial** | ≥5 case | 银行域 hard block / login path hard block / incognito 0 capture / 恶意网页伪造 message 被同源拦 / data: URL 拒收 |
| **Integration (E2E)** | ≥3 真浏览器 | (1) Playwright Chrome MCP 打开 chatgpt.com → 发 prompt → 等响应 → 验 sidepanel 见捕获;(2) 右键选中文本 → 验入库;(3) 主动保存当前页 → 验 webpage source_type |
| **Multi-source 协同** | ≥1 case | 同 URL 既 browse_signal 又 webpage capture,server 端不冲突,signal 关联到 item |
| **Permission audit** | manifest.json grep | 0 个新增 dangerous permission(`webRequest` / `<all_urls>` 之外的);CI 自动 |

**Playwright 真 Chrome 必要**(per CLAUDE.md §6.4):

- `channel="chrome"` 强制
- 不允许 mock chrome.runtime
- E2E 在 `tests/extension_e2e/` 用 puppeteer-extra + chrome extension loading

**6 类下限**:happy / edge / error / adversarial / 多用户(每用户独立 chrome.storage) / 资源耗尽(队列爆) — 全部覆盖。

---

## 10. 向后兼容

| 场景 | 兼容 SOP |
|---|---|
| 老 extension(v0.4.x 含 injector)→ 新 server | server `/api/v1/ingest` 一直兼容;legacy injector 已删,不影响 |
| 新 extension(本 spec 后)→ 老 server(v0.6.x)| 新 source_type='webpage' 在 v0.6.0+ 已支持(per `routes/ingest.rs` 透传)→ 兼容 |
| 老用户 chrome.storage 含 `injectionEnabled` key | worker 静默忽略,UI 不展示 toggle;deprecation 周期后(extension v0.8)清 |
| 后端从 localhost 迁到 LAN K3 一体机 | options page 改 backendUrl → SETTINGS_UPDATED → 无 restart 即生效 |
| Manifest V3 → 未来 V4 | 现 V3 写法不直接兼容 V4(Google 历史),需做适配层 — 本 spec 不预先设计 |
| Chrome ext 数据迁移到其他浏览器(Firefox)| chrome.storage 不兼容 webextensions storage;v2.x 加 export/import 设置 JSON |
| 新增 source 导致 source_type 集合扩 | server 端 `source_type` 是 free-form string,扩展无 schema migration |

**禁止 breaking change**:任何缩减现有 capture 能力(例:删 `MSG.CAPTURE_PAGE`)= breaking,必须 RELEASE.md Breaking 节明示并提供 migration path。

---

## 11. 风险登记

### R1 — `<all_urls>` 权限广泛(中)

**风险**:browse_capture 的 `<all_urls>` content_script 注入到任意页 — 即便 HARD_BLACKLIST 也是「JS 已加载,然后判断 + return」,被攻击者 patch 后能撬开

**缓解**:

- 双层防御:`manifest.json` exclude_matches 在 Chrome 引擎层阻止注入(JS 根本不上)+ `browse_capture.js` 头部 HARD_BLACKLIST 兜底
- extension code 走 Chrome Web Store **审核 + signed** — 用户从 Store 安装的 extension 不可本地 patch(开发者模式 sideload 例外)
- Privacy Audit checklist 项:每月 grep `<all_urls>` 使用位点,确保仅一处

### R2 — AI 网站 DOM schema 变动(中)

**风险**:ChatGPT / Claude / Gemini 改前端 DOM,`detector.js` 选择器失效 → 静默 0 捕获

**缓解**:

- `detector.js` 选择器版本化(每个平台 ≥2 个 fallback selector)
- indicator UI 显示「检测失败」状态(不仅 offline)
- 失败时 worker 发 `browse_signal` 含 `error="detector_fail"` → server 端 aggregate,运营层每周看 dashboard
- Playwright E2E nightly 跑(per CLAUDE.md「v1.1 nightly real-LLM workflow」)— DOM 变会立刻被 CI 捕到

### R3 — 用户误以为 extension 注入(信任)(高)

**风险**:历史用户记忆 extension 会向 ChatGPT 注入 — 即使代码不再,popup / README 说明不清会损信任

**缓解**:

- **popup 显著位置**写「捕获模式 — 不向 AI 网站注入任何内容」
- `extension/README.md` 顶部双语 disclaimer
- options page 移除任何「注入」相关 toggle(per §5.3 deprecation 计划)
- 本 spec landed → docs/PRIVACY.md 同步说明

### R4 — Service Worker 休眠丢消息(中)

**风险**:Manifest V3 service worker 被 Chrome 30s 后休眠,正在处理的 message / 队列丢

**缓解**:

- `chrome.storage.session.dedup` 持久化 dedup map(per `worker.js initState`)
- browse_queue 入 `chrome.storage.local` 也持久化(待加 — 当前仅 in-memory,v1.0.x 补)
- onStartup + onInstalled 双重唤醒

### R5 — 用户 vault locked 时 extension 捕获(中)

**风险**:用户 lock vault 后,extension 继续捕获 → server 端 `routes/ingest.rs` 返 401 locked → 数据丢失(client side dedup map 持久,server 401 后 client 不重试)

**缓解**:

- server `/api/v1/ingest` 在 vault locked 时返 `{ code: "vault-locked" }` 而不是 401
- worker 收到 vault-locked → buffer 入 `chrome.storage.local["pending_ingest"]` 队列(上限 1000)
- vault unlock 后 worker 自动 flush
- UI indicator 显示「vault locked,N 条待入库」

### R6 — 大文件 sidepanel 上传 OOM(低-中)

**风险**:用户拖 1GB 文件到 sidepanel → service worker 内存爆

**缓解**:

- 客户端 client-side 50MB 上限 + UI 提示
- `Streams API` 流式上传(已实现?待核查;若未,v1.1 加)
- server upload.rs 流式写盘,不全加载到 RAM

### R7 — 浏览器多 profile 串数据(低)

**风险**:用户 Chrome 用多个 profile(work + personal),都装 extension → 都向同 localhost:18900 写

**缓解**:

- 当前不区分;server 端单 vault 单用户假设(per attune positioning B2C)
- v1.x 加 `extension/options` 的 profile_tag(可选字符串),写进 metadata.profile;sidepanel filter 可看不同 profile
- 文档说明:「多 profile 默认共享同一 vault — 这是 feature 不是 bug」

### R8 — RSS / Pocket 第三方 OAuth token 残留(v1.1 风险预登记)

**风险**(v1.1 RSS / Pocket 上线后):用户 OAuth → token 存哪里?

**缓解**(预登记,v1.1 实施时确认):

- OAuth token 存 vault `cloud_session` table(AES-256-GCM)
- vault locked 时 RSS poll 暂停
- 用户「断开 RSS」按钮立即吊销 token + 删 vault 行
- 符合 Spec 1 (privacy logic strategy) 出网点 ②(cloud SaaS)契约

---

## 实施 next steps(本 spec 之外)

1. **本 spec landed** → 评审通过后 invoke `superpowers:writing-plans` 出 implementation plan
2. **v1.0.x 实施 scope**:
   - extension/README.md 重写「unidirectional ingest only,no injection」
   - popup 加 disclaimer
   - `worker.js` 移除 deprecated MSG.PREFETCH 之类 stub(extension v0.8 release 时)
   - server `routes/ingest.rs` 加 `vault-locked` 错误码 + worker buffer 兜底(R5)
   - `scripts/extension-permission-audit.sh` CI gate(权限新增 = build fail)
3. **v1.1.0 scope**:RSS / bookmark import(扩展点 §6.2)
4. **docs/PRIVACY.md(Spec 1 同时 land)**:用户视角描述 extension 的输入侧职责

---

**Spec 完成**:11 节齐全。任何后续 extension 新 source / 新 permission / 新 MSG 类型必先 amend 本 spec → user 评审 → implementation。
