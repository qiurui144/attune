# INT-1 浏览器登入器 attune 侧接入 — 实施报告

**Date**: 2026-06-21  **Task**: #133 (INT-1 impl)  **Branch**: develop (worktree)
**Tip**: `826c6c0` (本报告写完后会再 +1 commit 含本 .md)
**Spec**: `docs/superpowers/specs/2026-06-20-browser-autologin-integration.md`
**源工具**: `community-browser-automation` @ `212c957` (`/data/tmp/refs/...`,MIT,G1-G6 契约已验)

诚实声明:本 slice 落地 **SidecarController（契约对接核心）+ 登入协助能力的安全/存储/校验
底座 + 全 6 类测试 + 真工具 smoke**。REST 端点 + UI tab(spec §5/§4.1 列为 "if scoped")
**未在本 slice 落地**(见「未做/待办」),控制器 + 能力底座是其依赖,已就绪可接线。

---

## 1. SidecarController 设计（契约对接）

`attune-core/src/browser_login/sidecar.rs`。定位 + spawn `community-browser` CLI,
绑定其已验证的 **JSON-over-CLI 契约**(源工具 `cli.py` @ 212c957):

| 契约项 | attune 侧落点 |
|--------|--------------|
| **定位**(跨平台 Win P0) | `locate()`:① env `ATTUNE_BROWSER_TOOL` → ② PATH `community-browser`(`which` crate)→ ③ `python -m community_browser_automation.cli`。找不到 → `SidecarError::ToolNotFound`(kebab `browser-tool-not-found`),**不 panic**,能力优雅 disable |
| **G1 stdout 单 JSON** | `RunResult`(serde):`schema_version`(首键 fast-check)/`status`/`url`/`records`/`error`/`error_code`。`parse_run_result` 要求**恰好一个 JSON 值**(子进程往 stdout 打日志 = 违约 → `BadOutput`) |
| **G2 退出码分流** | `RunOutcome::from_exit_code`:0=Success / 10=NeedsHuman / 11=Restricted / 12=SessionExpired / 2=Usage / 其余=Internal。**attune 侧自持映射表**,即便子进程 status 文本错标也按退出码路由 |
| **G3 凭据 stdin** | `SidecarCommand::Auto` → `--credentials-stdin` flag + 凭据 JSON 写 child stdin 后关管道。**绝不进 argv / env / log**(§1.4 L-3)。`Credentials` 实现 `Zeroize`-on-drop + redacted `Debug`;写完 stdin 立即 `zeroize` 堆缓冲 |
| **G4 `done\n` resume** | login(人在回路)路径预留(注释 + 非 Auto 命令关闭 stdin 让 child 见 EOF 不阻塞);本 slice 用 `--wait-seconds` 兜底,`done\n` 主动 resume 留接线位 |
| 超时 + kill + 清理 | `with_timeout`(默认 120s)bounded poll(25ms)→ 超时 `kill`+`wait`→ `Timeout`。`run()` 无论成败都 `cleanup_state_file`:temp `--state` 文件 zeroize-on-disk + unlink(会话工作副本不留盘) |
| schema fast-check | major version != "1" → `SchemaMismatch`(契约漂移防护) |

跨平台:`std::process::Command` + `PathBuf` + `which` crate;无 shell 依赖;chrome 探测交给工具(`E2E_BROWSER_EXECUTABLE`,接线位)。

## 2. 登入协助能力 + 保险柜

- **recipe**(`browser_login/recipe.rs`):`LoginRecipe`(env-name-only 凭据,镜像源工具
  `CredentialSpec`)+ `to_json()` 断言不含凭据值(`looks_like_secret_value` 守卫)。
- **会话保险柜**:复用 `third_party_accounts`,新增 provider `browser_login`
  (`KNOWN_PROVIDERS`)。会话 `storage_state` JSON → AES-256-GCM(dek)落 `secret_enc`。
  secret 上限按 provider 分层:`browser_login` = 256KB(会话 JSON 大);其余仍 8KB。
  **超限报错不截断**(截断会破坏 JSON → 会话失效)。
- **SourceKind::LoginAssist**(additive,`as_str="login_assist"`):入库走既有 connector 抽象。
- 凭据归属:与 WebDAV/IMAP/Git PAT 同表同加密同 vault-locked 语义;`username` 存
  用户可读源名(非密码)。

## 3. 安全

| 约束 | 落地 |
|------|------|
| **allowlist + SSRF (L-7)** | `validate_entry_url` 复用 `net::url_guard::validate_outbound_url`:http(s) only / 拒裸 IP / 拒 loopback·private·link-local·metadata / **host allowlist(仅用户批准源)** / DNS-rebind 缓解。attune 侧独立校验(不依赖 sidecar 自检,defense-in-depth) |
| **auto 默认关 (L-5)** | 控制器默认路径 = scan/login(人在回路)。`SidecarCommand::Auto` 仅显式构造时携带凭据;能力层不默认走 auto |
| **凭据 stdin (L-3)** | 仅 `--credentials-stdin`;argv 审计测试实测断言凭据不入 argv;`Credentials` Debug redacted;env 中 `ATTUNE_BROWSER_*` 被 `env_remove` 清掉 |
| **会话加密** | secret_enc BLOB 实测不含明文(单测 + proptest);list view 编译期无 secret 字段 |
| **出网受控** | `OutboundKind::BrowserCrawl`(additive,`browser_crawl`);disabled / vault-locked 实测拒绝 |
| **边界** | crawl-INTO-vault(入站采集),**不**注入 web AI(守 cleanup-r15) |

## 4. 真工具 smoke 结果 ✅ — JSON-over-CLI 对接成立(实证)

`pip install -e .` 在本机 venv 失败(tsinghua mirror 403 + venv 缺 setuptools),
改走**源码直跑**(= attune fallback#3 的等价路径,§1.6 纯离线 smoke 例外):
`PYTHONPATH=src python3 -m community_browser_automation.cli scan https://example.com`。

**真 Playwright 启动浏览器**导航 example.com,输出真 G1 JSON(exit 0):
```json
{ "schema_version": "1", "status": "ok", "url": "https://example.com/",
  "title": "Example Domain", "elapsed_ms": 2882, "records": [],
  "error": null, "error_code": null }
```

**真工具 → 真 Rust SidecarController 端到端**(`browser_login_real_smoke.rs`,`#[ignore]`):
真 CLI stdout → 真 `RunResult` 解析 → `RunOutcome::Success`。两条定位路径均通过:
- env override(`ATTUNE_BROWSER_TOOL`)→ `REAL SMOKE OK: status=ok url=example.com schema=1`
- `python -m ...cli` fallback → 同上

**→ JSON-over-CLI 契约对接成立(非 mock,真工具真浏览器)。**

### 真工具 smoke 暴露的真 bug(已修)
`python -m community_browser_automation`(裸 package)**失败** —— package 无
`__main__.py`("cannot be directly executed")。SidecarController fallback#3 原写裸
package,**已修**为 `community_browser_automation.cli` 子模块(cli 有
`if __name__=='__main__'` block,实测 exit 0)。修后经 `locate()` fallback 路径
再 smoke 通过。commit `826c6c0`。

## 5. 六类测试覆盖（§6.1 / spec §9）

| 类型 | 用例 | 结果 |
|------|------|------|
| Golden/happy | `browser_login_subprocess.rs`:scan→ok;全链 scan(needs-login)→login(写+清会话)→run(爬 records) | ✅ |
| 集成 E2E 真子进程 | 同上 9 个(真 spawn 假 sidecar 脚本驱动真 spawn/stdin/parse/exit/cleanup 路径)+ 真工具 smoke 1 | ✅ |
| 边界 | bad-json→BadOutput / schema-2→SchemaMismatch / 空 stdout / 256KB 会话边界(refuse 不截断)/ webdav 仍 8KB | ✅ |
| 异常/错误 | timeout→kill / 缺二进制→SpawnFailed / tool-not-found / vault-locked(BrowserCrawl) / allowlist 拒 | ✅ |
| 对抗/安全 | **凭据 stdin 不入 argv**(argv-dump 旁证实测)+ 会话密文无明文 + L-7 拒非批准/内网/file:// + auto-off | ✅ |
| 属性测试 | 会话 round-trip 不变 / 密文永不含明文 marker / entry_url 校验不 panic+allowlist 单调(3 proptest) | ✅ |
| 回归 | 既有 third_party(14)+ outbound_gate 测试不回退 | ✅ |

**统计**:新增 ~40 测试。专项跑:subprocess 9 ✅ + session_vault 15 ✅ + inline(sidecar/recipe)
+ 真 smoke 1 ✅。**全 lib 套件 2459 passed / 0 failed / 2 ignored**(无回归)。
通过判据(deterministic)= 1.00。

## 6. clippy / build

- `cargo clippy -p attune-core --all-targets -- -D warnings` 干净。
- `cargo build -p attune-core -p attune-server` 通过(server 无改动,仅确认不破)。

## 7. 未做 / 待办

- **REST 端点 + UI tab**(spec §5/§4.1,"if scoped"):未落地。SidecarController + 能力
  底座(recipe/会话/allowlist/OutboundKind)是其依赖,已就绪;接线 = 加
  `routes/login_assist.rs` + UI tab(i18n zh/en parity)。
- **`login_assist_recipes` 元数据表**(spec §3.2):本 slice 用既有 third_party_accounts
  存会话;recipe 元数据(consent/TTL/rate-limit/entry_url)表未建,REST 接线时补。
- **打包**(Python runtime bundling,spec §4.4 / R3):
  - desktop 安装包需捆瘦 Python runtime(嵌入式 CPython / PyInstaller onedir)+
    Playwright wheel + `community_browser_automation`(MIT,需进 `ACKNOWLEDGMENTS.md` +
    保留 NOTICE)。体积预算 +40-60MB(spec 列 plan 首 gate PoC 实测)。
  - Playwright **浏览器二进制不捆**(用系统 Chrome / 首次运行 fetch,对齐 attune
    thin-deb + runtime-fetch 决策)。
  - **真工具 smoke 已证 CLI 契约对接成立**;打包是分发问题,不阻塞契约/能力正确性。
- **`auto` LLM 表单识别兜底**(§4.5.A-G):auto 默认关,启用前需过 3-tier 矩阵 + F1≥0.85
  gate + vision 改 qwen-3.6/3.7(源工具默认 qwen-vl 已下架)。本 slice 仅落控制器
  `Auto` 命令构造 + 凭据 stdin 安全;LLM 质量门留启用前。

---

## 附录:真工具 smoke 复现
```bash
PYTHONPATH=/data/tmp/refs/community-browser-automation/src \
ATTUNE_BROWSER_TOOL=<wrapper-or-community-browser> \
ATTUNE_SMOKE_RECIPE=/data/tmp/smoke-recipe.json \
  cargo test -p attune-core --test browser_login_real_smoke -- --ignored --nocapture
# → REAL SMOKE OK: status=ok url=https://example.com/ schema=1
```
