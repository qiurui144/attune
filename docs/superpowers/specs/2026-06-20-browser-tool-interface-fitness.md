# 接口适配性分析：community-browser-automation ↔ attune 集成诉求

> **Status**: ANALYSIS（只读分析，供用户优化上游用，**不改任何代码**）
> **Date**: 2026-06-20  **Task**: #123 (INT-1)  **Author**: 接口适配性分析 agent
> **被分析对象**: `github.com/qiurui144/community-browser-automation`，本地镜像 `/data/tmp/refs/community-browser-automation/`，**v0.2.0 / MIT / Python ≥3.9 / Playwright + FastAPI**（引代码路径，未臆测）。
> **attune 诉求来源**:
> - 集成 spec `docs/superpowers/specs/2026-06-20-browser-autologin-integration.md`（sidecar 路线，已选 A）
> - clean-room 路线 spec `2026-06-17-browser-login-assist-session-capture.md`（#66）
> - 凭据保险柜 `rust/crates/attune-core/src/store/third_party_accounts.rs`（已落地，AES-256-GCM）
> - 采集源接入 `ingest/connector.rs`（SourceConnector / SourceKind）
> **本文档目的**: 评估**现有上游接口**能否满足 attune 作为调用方的诉求；列缺口 + 给上游接口改动建议 + 推荐 attune↔tool 调用契约。**结论与建议都是给上游优化用的输入，attune 侧的合规收窄（consent/速率/auto 默认关）不在上游改动范围。**

---

## 1. attune 作为调用方的诉求清单

attune 计划把该工具作 **subprocess sidecar** 用（spec §4.4 选项 A：CLI over `std::process::Command`，不复用其 FastAPI/Vue/WS），从 Rust 驱动 `scan`/`login`/`run`，捕获 `storage_state` 进保险柜，复用会话采内容进 vault。由此推出以下诉求（D1–D12）：

| # | 诉求 | 为什么 attune 需要 |
|---|------|-------------------|
| D1 | **触发登入**（指定站点/采集源、指定 login 页） | 每个采集源是一个会员墙站点，需按 recipe 触发 |
| D2 | **人在回路模式**（弹可见浏览器，等用户手动登录/解 captcha） | L-5 默认主路径；captcha/MFA 不自动破 |
| D3 | **auto 模式**（LLM 自动识别表单填凭据） | 进阶用户逐源显式开启；captcha/MFA 仍人工兜底 |
| D4 | **凭据安全传入**（不进 argv / 不进 log / 不进 recipe 文件） | §1.4 secrets 铁律；凭据来自 vault DEK 解密 |
| D5 | **输出 storage_state / cookie** 给保险柜（可控路径 / 可回读） | 会话 DEK 加密存 `third_party_accounts.secret_enc` |
| D6 | **会话复用采内容进 vault**（`run --state` 无人值守爬 → 结构化 records） | crawl → RawDocument → ingest_document |
| D7 | **结构化进度 / 状态 / 错误码**（供 Rust 程序化判定，不靠 stderr 文本 scrape） | 区分 logged-in / needs-human / restricted / error 走不同分支 |
| D8 | **跨平台**（Windows P0 / Linux x86_64 P1），无 GUI 也能跑（headless + CDP attach） | attune 主平台矩阵；local scheduler/无头服务器场景 |
| D9 | **可被 Rust subprocess 稳定驱动**（确定性 stdin/stdout 协议，无交互式 prompt 阻塞） | sidecar 生命周期可控、可超时 kill |
| D10 | **退出码规范**（区分成功/需人工/错误，供 `ExitStatus` 判定） | Rust 不解析文本即可判结果 |
| D11 | **LLM 端点可注入**（指向 attune 网关 / BYOK，OpenAI-compat） | LLM 出网经 attune 网关受 OutboundGate 约束 |
| D12 | **打包/依赖瘦身**（降 Python+Playwright 体积，浏览器二进制不强捆） | desktop 安装包体积预算（spec R3，+40-60MB 是最大成本） |

---

## 2. 现有接口映射表（诉求 ↔ 满足度 ↔ 现有接口形态）

满足度图例：✅ 满足 / 🟡 部分满足 / ❌ 缺。**全部基于源码实读**（cli.py / web.py / runner.py / models.py / llm_agent.py / session_manager.py）。

| # | 诉求 | 满足 | 现有接口形态（引路径） |
|---|------|------|----------------------|
| D1 | 触发登入 | ✅ | CLI `auto`/`login` 子命令 + recipe `start_url`/`login_url`（`cli.py:23-55`，`models.py:84` login_url，`runner.py:99` 优先 login_url）|
| D2 | 人在回路 | ✅ | CLI `login --wait-seconds`（`cli.py:96`→`runner.py::capture_login:296`，轮询 success 信号或超时 needs-human）|
| D3 | auto 模式 | ✅ | CLI `auto`（`runner.py::auto_login:61`），known_selectors 优先于 LLM，已处理用户名→密码两段式多步登录（`runner.py:160-198`）|
| D4 | 凭据安全传入 | 🟡 **关键缺口** | **CLI 路径仅支持 env-var-only**：recipe 存 env 名，`cli.py:110` 从 `os.environ` 读值（**正面**：值不进 recipe/argv）。**但无 stdin 通道** —— 父进程仍须把凭据放进**子进程 env**（attune 可接受，但缺更强的 stdin 注入）。**web.py `AutoLoginRequest` 反而退化**：`username`/`password` 明文字段（`web.py:182-187`）—— 该路径 attune 不用，但是上游攻击面 |
| D5 | 输出 storage_state | ✅ | `--state <path>` 调用方完全控制路径（`cli.py:39`，`runner.py:106` `context.storage_state(path=...)`）；标准 Playwright storage_state JSON（cookies+localStorage 单层）|
| D6 | 会话复用采内容 | ✅ | CLI `run --state --query --out`（`cli.py:57-62`→`runner.py::run_search:325`），recipe-driven 抽取 records（`_extract:391`，上限 50 条）；`--out` 写文件 |
| D7 | 结构化状态/错误 | 🟡 | **stdout 打印完整 JSON**（`cli.py:135` `print(payload)`，`result_to_json:660` = `RunResult.__dict__`）—— status/url/title/records/signals/error 字段齐。**缺**：(a) JSON 与日志/告警混在同一 stdout（`PageAnalyzer` 等用 `logging`，但若 root logger 配到 stdout 会污染）；(b) **错误是自由文本字符串**（`error="login-error: 密码错误"` / `f"{type(exc).__name__}: {exc}"`，`runner.py:271/291`）非枚举码 |
| D8 | 跨平台 + 无头 | ✅(Linux) 🟡(Win) | headless：`runner.py:637` `launch(headless=not headed)`，CLI `auto`/`run` 默认 headed=False（无头）✅；CDP attach：`E2E_CDP_ENDPOINT`/`connect_over_cdp`（`runner.py:54,632`）✅；浏览器探测 `find_browser`（`runner.py:33`，chromium/chromium-browser/google-chrome + `E2E_BROWSER_EXECUTABLE`）。**Win 未在矩阵实测**（`find_browser` 未含 `chrome.exe` 路径探测）|
| D9 | 稳定 subprocess 驱动 | 🟡 | CLI 是**一发命令一进程**（fire-and-run，非长驻 daemon）—— 简单可控 ✅。**但**：`login --wait-seconds` 是**进程内轮询阻塞**（`runner.py:300-313`），attune 无法在"用户已登录完成"时**主动 resume**，只能等满 wait-seconds 或 success 信号自动命中 —— 缺一个"用户完成"的回传信号通道（web.py 的 WS `command_done` 有，CLI 无）|
| D10 | 退出码规范 | 🟡 | `main()` 返回 `0` 除非 status=="error" 才 `1`（`cli.py:136`）；`auto` 凭据缺失返 `2`（`cli.py:109,117`）。**缺**：`needs-human` / `restricted` / `session-expired` 等与成功**同为 exit 0**，Rust 必须再解析 JSON status 才能分流 |
| D11 | LLM 端点注入 | ✅ | `LLM_BASE_URL`/`LLM_API_KEY`/`LLM_MODEL`（OpenAI-compat，`llm_agent.py:87-102`），vision 走 `VISION_*` fallback `LLM_*`（`llm_agent.py:106-128`）；attune 经 env 注入网关即可。**注意**：vision 默认 `qwen2.5-vl-7b-instruct`（`llm_agent.py:123`）—— 与 spec 称"qwen-vl 已下架"需对齐选型 |
| D12 | 打包瘦身 | 🟡 | deps 4 个 runtime（`pyproject.toml`：playwright/fastapi/uvicorn/python-multipart）。**attune sidecar 只需 CLI** → fastapi/uvicorn/python-multipart **全是 web.py 专属，CLI 路径用不到** —— 当前是硬依赖，无法只装 CLI 子集 |

**汇总**：12 项中 **6 满足（D1/D2/D3/D5/D6/D11）、6 部分（D4/D7/D8/D9/D10/D12）、0 全缺**。工具的核心能力（登入/人在回路/auto/会话/抽取/LLM 注入）都已实装且 attune 可直接用；缺口集中在**程序化驱动的"契约硬度"**（错误枚举、退出码语义、stdout 纯净度、CLI 的人工完成回传、Win 浏览器探测、依赖可裁剪）。

---

## 3. 缺口 + 上游改动建议（给用户优化 community-browser-automation 用）

按对 attune 集成的价值排序（G1 最高）。每条标 **诉求关联** + **改动落点**。

### G1（top）— stdout 纯净的结构化 JSON 协议 + 错误枚举（D7/D9）
**问题**：`cli.py:135` 把结果 JSON `print()` 到 stdout，但 `logging`（PageAnalyzer/runner debug 行）若被 root logger 配到 stdout 会**污染同一流**，Rust `serde_json` 解析会炸；错误是自由文本（`runner.py:271/291`），Rust 无法稳定 match。
**建议**：
1. **stdout 只输出一行/一块结果 JSON；所有日志强制走 stderr**（`logging.StreamHandler(sys.stderr)`，CLI 入口显式配置）。给一个 `--json`（或默认）模式保证 stdout 是**单个 JSON 文档**，无前后缀。
2. **错误码枚举化**：`RunResult` 增 `error_code` 字段，取值固定小集（kebab）：`llm-no-fields` / `fill-failed-username` / `fill-failed-password` / `submit-failed` / `login-error` / `captcha-detected` / `login-not-confirmed` / `timeout` / `nav-failed` / `internal`。保留 `error`（人读详情）但 Rust 只 match `error_code`。现有字符串前缀（`runner.py:213/247/271/283`）已半结构化，抽成枚举即可。
3. **结果 schema 版本号**：`RunResult` 增 `schema_version: "1"`，输出契约变更时 bump（呼应 spec §6「sidecar 协议版本化」）。

### G2 — 退出码语义分层（D10）
**问题**：`needs-human`/`restricted`/成功同为 exit 0（`cli.py:136`），Rust 必须解析 JSON 才能分流，退出码形同虚设。
**建议**：定义稳定退出码表，让 Rust **不读 body 即可粗分流**：
- `0` = 终态成功（logged-in / ok / 有 records）
- `10` = needs-human（captcha/MFA/无字段，需人工）
- `11` = restricted（受限/封锁/频繁）
- `12` = session-expired / not-logged-in
- `2` = 用法错误（凭据未配 / recipe 非法）— 已有
- `1` = 内部错误（异常）— 已有
（退出码 + JSON `error_code` 双轨：码用于快速分流，JSON 用于细节。）

### G3 — 凭据 stdin 注入通道（D4）
**问题**：CLI 仅 env-var-only（`cli.py:110`）。attune 倾向**更强的 stdin 注入**（凭据写子进程 stdin，进程退出即灭，比 env 更短命、不被 `/proc/<pid>/environ` 窥见）。web.py `AutoLoginRequest` 还有明文 `username`/`password` 字段（`web.py:182`）= 攻击面。
**建议**：
1. CLI `auto` 增 `--credentials-stdin`：从 stdin 读一行 JSON `{"username":"...","password":"..."}`（或 `KEY=VALUE\n` 行），读毕立即清引用。**优先级高于 env**。argv 永不含凭据（保持现状）。
2. web.py `AutoLoginRequest` **移除明文 `username`/`password` 字段**，只留 `username_env`/`password_env`（与 CLI/recipe 一致，收窄攻击面）。
3. README Security 节补一句"凭据优先 stdin，env 次之，绝不入 argv/recipe/log"。

### G4 — CLI 人在回路的"用户已完成"回传（D9/D2）
**问题**：CLI `login` 只能轮询 success 信号或等满 `--wait-seconds`（`runner.py:300`）。若站点 success 信号 recipe 没配准，用户登录完了 sidecar 也不知道，只能干等超时。web.py 有 `/continue` + WS `command_done`，CLI 没有等价物。
**建议**（任一）：
1. **轻量**：CLI `login` 支持监听 stdin —— 用户在 attune UI 点"我已登录完成"，attune 往 sidecar stdin 写一行 `done\n`，sidecar 立即抓 storage_state 返回（比纯轮询 + 超时更跟手）。
2. 或：`login` 周期性向 **stderr** 发 NDJSON 进度事件（`{"event":"waiting","elapsed_ms":...}` / `{"event":"signal-hit"}`），attune 可据此驱动 UI；stdout 仍只在终态出结果 JSON。

### G5 — 依赖分层，CLI 路径免装 web 栈（D12）
**问题**：`pyproject.toml` 把 fastapi/uvicorn/python-multipart 列为硬 runtime dep，但**只有 web.py 用**；attune sidecar 只调 CLI，白背 web 栈体积。
**建议**：拆 extras —— `dependencies = ["playwright>=1.40"]`（CLI 唯一刚需），`[project.optional-dependencies] web = ["fastapi","uvicorn[standard]","python-multipart"]`。attune 装 `pip install community-browser-automation`（瘦），web UI 用户装 `[web]`。直接砍 spec R3 的打包体积。

### G6 — Windows 浏览器探测 + 跨平台明示（D8）
**问题**：`find_browser`（`runner.py:33`）只查 `chromium`/`chromium-browser`/`google-chrome`（POSIX 名），**无 Windows `chrome.exe` / Edge 默认安装路径探测**；attune Win 是 P0。
**建议**：`find_browser` 增 Windows 分支（`%ProgramFiles%\Google\Chrome\Application\chrome.exe`、`%LOCALAPPDATA%`、Edge `msedge.exe`），并文档化"优先用 `E2E_BROWSER_EXECUTABLE` 显式指定"（attune 会注入检测到的系统 Chrome 路径）。README 增一节跨平台浏览器解析顺序。

### G7（次要）— vision 默认模型 + 文本默认对齐（D11）
**问题**：vision 默认 `qwen2.5-vl-7b-instruct`（`llm_agent.py:123`）、文本默认 `gemini-3-flash-nothinking`（README）。attune 选型默认 deepseek-v4 文本 / qwen-3.6/3.7 多模态（CLAUDE §4.5.H），且 attune 总会 env 覆盖。
**建议**：保持 env 可覆盖（已满足）；上游若想对齐，把 vision 默认改成在架的 multimodal 模型，README 注明"默认值随 provider 在架情况调整，生产环境应显式设 `*_MODEL`"。**低优先级**——attune 注入即覆盖，不阻塞。

---

## 4. 推荐的 attune ↔ tool 调用契约

**形态：JSON over CLI（stdin 注入凭据 + stdout 单 JSON 结果 + stderr 日志/进度 + 退出码分流）。** 不走 HTTP（spec §2.2：sidecar 零网络监听）。这是把 §3 的 G1–G4 收敛成一份可照做的契约——**用户照此优化上游后，attune SidecarController 可直接对接**。

### 4.1 调用形态
```
# attune 注入 env：LLM 端点指向网关、浏览器指向系统 Chrome
LLM_BASE_URL=<gateway>  LLM_API_KEY=<short-lived>  LLM_MODEL=deepseek-v4
E2E_BROWSER_EXECUTABLE=<attune-detected-chrome>
python -m community_browser_automation <scan|login|auto|run> <recipe.json> [flags]
   < (凭据 JSON 经 stdin，仅 auto)         # G3
   1> 结果 JSON（单文档，无污染）          # G1
   2> 日志 + NDJSON 进度事件               # G1/G4
   exit code ∈ {0,1,2,10,11,12}            # G2
```

### 4.2 stdin（凭据，仅 auto；G3）
```json
{"username": "<resolved-from-vault-DEK>", "password": "<resolved>"}
```
读毕清引用；argv/recipe/log 永不含值。

### 4.3 stdout（终态结果，单 JSON；G1）
```json
{
  "schema_version": "1",
  "status": "logged-in",          // logged-in|ok|needs-human|restricted|needs-login|error
  "error_code": null,             // null 或 kebab 枚举（G1.2）
  "error": null,                  // 人读详情，可空
  "url": "https://...",
  "title": "...",
  "elapsed_ms": 1234,
  "signals": ["..."],
  "records": [ { /* recipe extract 结果 */ } ],
  "state_path": "/tmp/xxx.json"   // 写出会话的路径（login/auto 成功时；attune 即用即删 + DEK 加密）
}
```
（现有 `RunResult` 已含 status/url/title/elapsed_ms/records/signals/error；本契约= 加 `schema_version`/`error_code`/`state_path` 三字段。）

### 4.4 stderr（日志 + 进度 NDJSON；G1/G4）
- 所有 `logging` 输出 → stderr（不污染 stdout）。
- 可选 NDJSON 进度行：`{"event":"waiting","elapsed_ms":...}` / `{"event":"signal-hit"}` / `{"event":"needs-human","reason":"captcha"}`。

### 4.5 退出码（G2）
`0` 成功 · `10` needs-human · `11` restricted · `12` session-expired · `2` 用法错 · `1` 内部错。

### 4.6 人在回路 resume（G4）
`login` 读 stdin：attune UI"我已完成"→ 写 `done\n` → sidecar 立即抓 storage_state 出结果。（无 resume 信号则退回轮询 + 超时，向后兼容。）

---

## 5. 安全 / ToS 边界提醒（给上游 + attune 双方）

工具上游**已有**良性安全姿态，attune 集成时需保持并收窄：
- **auto 默认关**：上游 `auto` 是显式子命令（不默认触发）✅；attune 侧 `auto_login_enabled` 默认 false，captcha/MFA 一律 fall back `needs-human`（README 已声明"does not bypass CAPTCHA/SSO/MFA"✅）。
- **域名 allowlist / SSRF**：web.py 已有 loopback/link-local 拒绝 + DNS 重绑检查 + CDP allowlist（`web.py:60-164`）✅；**CLI 路径无此校验** → **attune 侧须独立校验 entry_url**（http(s) only / 禁内网 / 禁 file://，spec L-7），不依赖 sidecar 自检（defense-in-depth）。建议上游也把 URL 校验下沉到 runner（CLI navigate/goto 前），让两路径一致。
- **人在回路 captcha**：captcha/QR/OTP 检测仅作"提示用户手动处理"，**不暴露自动破解**（attune 不暴露 `_cmd_captcha` 自动应答给终端用户）。
- **凭据零落地**：env-var-only（recipe 不存值）是好起点；建议升级到 stdin（G3），并移除 web.py 明文凭据字段。
- **会话仅用户自有**：会话绑 vault、TTL 过期重登、不跨用户/设备同步（attune 侧 L-2，非上游职责，但上游 README 可加一句使用边界免责）。

---

## 附录 — 关键源码锚点（便于上游定位改动）
| 改动 | 文件:行 |
|------|--------|
| G1 stdout/stderr 分流 + 结果 print | `cli.py:132-136` |
| G1 错误码枚举 + schema_version | `runner.py:21-31`(RunResult) / `runner.py:144-291`(error 文本点) |
| G2 退出码 | `cli.py:136`（当前 0/1）+ `cli.py:109,117`（2） |
| G3 凭据 stdin | `cli.py:99-123`(auto 分支) / `web.py:182-187`(移除明文字段) |
| G4 login resume | `runner.py:296-323`(capture_login 轮询) |
| G5 依赖分层 | `pyproject.toml:7-12` |
| G6 Win 浏览器探测 | `runner.py:33-41`(find_browser) |
| G7 默认模型 | `llm_agent.py:95-97,119-123` |
