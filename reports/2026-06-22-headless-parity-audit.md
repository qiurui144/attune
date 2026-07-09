# G6 headless↔桌面功能 parity audit + 补差

- 日期: 2026-06-22
- 任务: #145 G6 (local scheduler spec `docs/superpowers/specs/2026-06-22-local-scheduler-integration.md` 中 G6「headless 二等公民 / 首次开箱纯 Web」标 ⚠️ 部分,完整 headless↔桌面对齐 audit 随 local scheduler v0.2 — 本轮兑现)
- base: develop @ c186067(防 stale:`git fetch + reset --hard origin/develop` 已做)
- 分支: 见 git log

> 注:prompt 引用的 `2026-06-10-local-scheduler-integration-gaps.md` 仓内不存在;实际 SSOT 是
> `2026-06-22-local-scheduler-integration.md`(其 G1-G8 对齐表逐字引用同一 gaps 文档命名)。
> G6 定义与 prompt 完全一致,据此执行。

---

## 0. 架构前提(决定 audit 范围)

attune 桌面 = **Tauri 壳 spawn 内嵌 attune-server(:18900),webview 直接加载同一份
嵌入式 Web UI**(`apps/attune-desktop/src/main.rs` + `embedded_server.rs`)。
**桌面没有任何独立的功能 UI** —— 它和 headless Web 用户看到的是**同一套 React/Preact
单页**(`crates/attune-server/ui/`,`include_str!` 进 server 二进制)。

因此「桌面有/Web 缺」的真实面 = **桌面壳 (Tauri native) 额外提供、而纯 Web 无法触达的
那几个 OS-级能力点**:
1. 系统托盘(show/quit)— `tray.rs`
2. 自动更新器(check/download/restart)— `main.rs` `check_for_update_now` / `restart_for_update`
3. OS 文件拖拽 → 上传 — `main.rs` `upload_dropped_paths` + 前端 `attune-file-drop` 监听
4. 原生目录选择器(`@tauri-apps/plugin-dialog`)— 3 处调用

UI 内**唯一**被 Tauri 门控的代码点(grep `__TAURI_INTERNALS__` / `canPickFolder` / `isTauri`):
`App.tsx`、`wizard/Step5Data.tsx`、`views/RemoteView.tsx`、`views/SettingsView.tsx`。
逐点核对见下表。

所有 REST `/api/v1/*` 业务能力(vault init/unlock、LLM wizard、采集源 bind(local/
webdav/git/email/rss)、search、chat、设置、导出、插件安装/marketplace、备份/DSAR、
锁定/解锁、organize)**全部经嵌入式 Web UI 暴露,与桌面共用同一套页面 → 天然 0 落差**。

---

## 1. 落差清单(桌面壳有 / 纯 Web headless 缺或弱 + 严重度)

| # | 能力 | 桌面壳 | 纯 Web headless | 严重度 | 处置 |
|---|------|--------|------------------|--------|------|
| L1 | **wizard 首次开箱「关联文件夹」** | 原生目录弹窗 picker | **完全卡死**:`canPickFolder=false` → 按钮 disabled,仅一句「请在桌面版用弹窗」toast。首次开箱**无法绑定任何监听目录**,只能 import-profile 或 skip | **P0(阻断 headless 全流程)** | **已补**:加手填绝对路径输入框 + 添加按钮(`!canPickFolder` 时渲染),与 RemoteView/SettingsView 同 pattern,统一走 `/index/bind` |
| L2 | 设置→数据 文件夹管理新增目录 | 原生 picker(multiple) | **已有手填回退**:`onAddFolder` 在 Web 弹手填路径 modal(`showAddModal`)。**无落差** | — | 既有,无需动 |
| L3 | 远程目录(WebDAV/本地)绑定 | 原生 picker(可选) | **已有手填回退**:`RemoteView.LocalForm` 常驻路径输入 + browse 仅桌面显示。**无落差** | — | 既有,无需动 |
| L4 | OS 文件拖拽上传 | 拖文件进窗口 → `upload_dropped_paths` | Web 用标准 `<input type=file>` / 拖拽区上传(各 View 的 upload 入口)。功能等价,仅缺「拖进 OS 窗口」这一交互糖 | **P2(交互糖,功能不缺)** | 不补:Web 标准文件上传已覆盖「把文件入库」目标;OS-窗口级拖拽是壳特性,headless 无显示窗口本就无意义 |
| L5 | 应用自动更新器 | 30s 被动检查 + 手动 check/download/restart(SettingsView「应用更新」块) | `isTauri=false` → 整块 `return null`,Web 无更新入口 | **P2(设计如此,非缺口)** | 不补:headless/local scheduler 更新走 **apt/镜像重建/包管理器**(per CLAUDE.md 瘦包 + runtime-fetch 模型),不应也不能由 webview 内 in-app updater 负责。属正确的形态差异 |
| L6 | 系统托盘(show/quit) | 有 | headless 无 GUI,托盘无意义;停服走 `systemctl stop` / 进程管理 | **P2(N/A headless)** | 不补:headless 24h 常驻由 systemd/容器管理生命周期,托盘是桌面 GUI 概念 |

**结论**:真正阻断 headless 全流程的 **P0 落差仅 L1 一项**;L2/L3 早已有手填回退(此前
sprint 修过,L1 被遗漏);L4/L5/L6 是「桌面 GUI/OS 特性」与「headless 形态」的**正当形态
差异**,非功能缺口,补进 Web 既无意义也违背瘦包/包管理器更新模型。

---

## 2. 补了哪些(P0/P1)

### L1 (P0) — wizard Step5「关联文件夹」headless 手填回退

文件:`rust/crates/attune-server/ui/src/wizard/Step5Data.tsx`

- `canPickFolder`(Tauri 存在)→ 保留原生目录弹窗;
- `!canPickFolder`(纯 Web / local scheduler 一体机)→ 渲染**手填绝对路径输入框 + 「添加文件夹」按钮**
  (Enter 也可提交),push 进同一 `folderPaths` 状态;
- 两条路径(picker / 手填)统一走既有 `api.post('/index/bind', {path, recursive:true})`
  —— 复用 server 侧 `bind_directory` + `validate_bind_path`(canonicalize 已拒绝不存在/
  越界路径,错误友好脱敏),**0 业务代码改动,0 新 endpoint**;
- 描述文案从「请在桌面版」改为「纯网页/一体机请手填服务器可访问的绝对路径」;
- `pickFolder` 抽出共享 `addFolderPath`(去重)+ `submitManualPath`。

i18n:新增 `wizard.data.folder.{desc_manual,toast_manual_hint,manual_label,manual_placeholder}`
(zh+en 同步),移除已无引用的 `desc_browser_only` / `toast_browser_only`。zh/en key 集合 diff=0。

**个人版桌面 0 回退**:`canPickFolder=true` 分支逐字保留原生 picker 行为(仅把 disabled 态
判断从 `folderPicking || !canPickFolder` 简化为 `folderPicking`,因该分支恒 canPick);桌面
用户体验不变。

### P1 — 本轮无新增 P1 补丁

audit 未发现「桌面有 / Web 弱(非阻断)且应补」的 P1:L2/L3 已具回退;L4/L5/L6 属正当形态差异。

---

## 3. headless 全流程纯 Web 可达性结论

开机 → 纯 Web(:18900)可达性逐站核对(REST + UI 均共用页面):

| 阶段 | 纯 Web 可达? | 备注 |
|------|--------------|------|
| vault init(设密码) | ✅ | wizard Step2,无 Tauri 门控 |
| LLM wizard(会员网关 / BYOK / 本地 / local scheduler :8090) | ✅ | wizard Step3,local scheduler profile 已 ship(local-scheduler-S3) |
| 硬件探测 | ✅ | wizard Step4,纯 Web |
| **首次开箱关联文件夹** | ✅ **(本轮修复前 ❌)** | L1 手填回退补齐后纯 Web 可绑定监听目录 |
| 后续加监听目录(设置→数据) | ✅ | L2 既有手填 modal |
| 采集源(WebDAV/git/email/rss) | ✅ | bind-remote/git/email REST + RemoteView/各表单,纯 Web |
| 搜索 / chat | ✅ | ChatView/KnowledgeView,纯 Web |
| 设置 / 锁定·解锁 | ✅ | 顶栏锁定 + unlock REST,纯 Web |
| 导出(交付物 xlsx/csv/md/docx/pdf) | ✅ | ExportButton,浏览器下载 |
| 插件安装 / marketplace | ✅ | MarketplaceView,纯 Web |
| 备份 / DSAR | ✅ | 设置内,REST |

**结论:补齐 L1 后,local scheduler headless「开机 → 纯 Web 完成 vault init + LLM/scheduler 配置 +
采集(含本地目录)+ 搜索/chat + 导出 + 插件」全流程纯 Web 可达,无需桌面。** 唯三需桌面的
能力(OS 拖拽糖 / in-app 更新器 / 系统托盘)是正当形态差异,headless 各有等价替代(标准
文件上传 / 包管理器或镜像更新 / systemd 生命周期)。

---

## 4. 桌面 0 回退确认

- 个人版桌面(Tauri)代码 **未改一行**(`apps/attune-desktop/*` 无 diff)。
- UI 改动仅在 `!canPickFolder`(纯 Web)分支新增;`canPickFolder`(桌面)分支保留原生 picker
  全部行为。
- diff 范围:`Step5Data.tsx` + 2 i18n + 重建的 `ui/dist/index.html` + `Cargo.lock`(无逻辑)。

---

## 5. 六类测试 / 质量门

| 维度 | 结果 |
|------|------|
| happy(桌面 picker) | 保留原生 picker 分支,逻辑未变,既有路径 |
| happy(headless 手填) | 手填 → push folderPaths → `/index/bind`(server 已测) |
| 边界(空路径) | `addFolderPath` trim 后空串拒绝;按钮 `disabled={!manualPath.trim()}` |
| 边界(重复路径) | `addFolderPath` 去重(`includes` 检查) |
| 异常(不存在/越界路径) | server `validate_bind_path` canonicalize 拒绝 + 友好脱敏错误(既有) |
| 回归 | `cargo build -p attune-core -p attune-server` 通过;`cargo clippy --all-targets -D warnings` 0 警告;`cargo test -p attune-server --lib`(member/bind 相关)通过 |
| UI 构建 | `npm run build`(tsc --noEmit + vite)通过,dist 重新嵌入 |
| i18n 双守卫 | Guard1(硬编码中文)CLEAN;Guard2(zh/en key diff)CLEAN |

注:本改动为 UI(TSX)+ i18n,server 侧 `/index/bind` 路径未触碰,既有 server 测试即覆盖;
手填路径与原生 picker 路径汇入**同一** REST 端点。纯 Web headless E2E(真起 :18900 + 真
Playwright Chrome 跑 wizard 绑定目录)= §7.3 真机/真服 PENDING(本轮 worktree 内做了
build/clippy/test/i18n 门,未起真 server + Chrome;留给 RC/真机验收)。

---

## 6. 残留

- **需桌面的(正当形态差异,不补)**:OS 窗口文件拖拽糖(L4)、in-app 自动更新器(L5)、
  系统托盘(L6)。headless 各有等价替代,补进 Web 无意义。
- **留 v.next / 真机**:纯 Web headless 全流程 **真起服 + Playwright Chrome E2E**(§6.4 +
  §7.3)未在本 worktree 执行 —— 属真机/真服验收范畴,建议随 local scheduler 真机(LOCAL_SCHEDULER_IP :8090)
  或 GA 验收 §7.2 Gate3 一并跑(本轮已备齐纯 Web 可达性的代码面)。
- **跨平台**:改动纯 TS/i18n,无 arch 特异代码,跨平台无新债。
