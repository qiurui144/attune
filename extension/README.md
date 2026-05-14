# attune Chrome 扩展

Attune 私有 AI 知识伙伴的浏览器伴侣 — Manifest V3 + Preact + Vite。

## 功能

- **AI 对话捕获**：ChatGPT / Claude / Gemini 三大平台对话自动入库（MutationObserver + 2s debounce + djb2 去重）
- **浏览状态信号**：除 AI/登录/密码管理域名外，G1 通用浏览摄取（`<all_urls>` 配置在 manifest）
- **侧栏（Side Panel）**：搜索 / 时间线 / 文件 / 状态
- **状态指示器**：4 态（disabled / processing / captured / offline）
- **后端联动**：心跳 + 会话感知加权 + 文件拖拽上传

> v0.6.0 之前提供向 AI 网站注入前缀的功能，cleanup-r15 删除（产品方向改为内置 Chat + RAG，不再向 AI 网站 DOM 注入修改对话）。捕获能力保留。

## 安装（用户）

1. 下载主仓 [Releases](https://github.com/qiurui144/attune/releases) 中的 `attune-extension-vX.Y.Z.zip`
2. Chrome `chrome://extensions` → 开启「开发者模式」→ 「加载已解压的扩展」选择解压目录
3. 后端需先启动：`attune-server` 或 [Tauri 桌面应用](https://github.com/qiurui144/attune/releases?q=desktop)

支持的浏览器：Chrome / Edge / Chromium 内核浏览器。Firefox / Safari 不支持（MV3 实现差异）。

## 开发

```bash
cd extension
npm install
npm run build       # 输出到 dist/
npm run watch       # 开发模式 + 热重载
```

加载未打包扩展：在 `chrome://extensions` 选「加载已解压」→ 选 `extension/`（包含 manifest.json）。

## 测试

E2E 测试在 [`../python/tests/test_extension.py`](../python/tests/test_extension.py)（Playwright + 真 Chrome，遵守 CLAUDE.md `channel="chrome"` 约束）：

```bash
cd python
pytest tests/test_extension.py -v
```

CI 默认 `--ignore` 该测试（GHA runner 无系统 Chrome）。本地手工跑。

## 目录结构

```
extension/
├── manifest.json        # MV3 配置
├── package.json         # npm + Vite
├── vite.config.js       # 多入口构建
├── src/
│   ├── background/      # service worker (消息路由 + 健康检查)
│   ├── content/         # 平台适配器 + capture + indicator
│   ├── popup/           # 工具栏图标弹窗
│   ├── sidepanel/       # 侧栏 (搜索 / 时间线 / 文件 / 状态)
│   ├── options/         # 选项页 (后端地址 / 注入模式)
│   └── shared/          # messages + api 封装
└── assets/icons/        # 16/48/128 png
```

## 权限说明

- `storage / sidePanel / activeTab / tabs / contextMenus / webNavigation` — 常规扩展能力
- `host_permissions: <all_urls>` — G1 通用浏览状态摄取需要（排除 login/signin/密码管理域名 + 3 AI 网站避免双注入）
- `incognito: not_allowed` — 隐身模式强制禁用（per R04 P1-4：防御 content script JS 检查被绕过）

Chrome 商店审查时会要求对 `<all_urls>` 提供 justification — 文案位于 store-listing。

## 版本同步

扩展 `manifest.version` 跟随主仓 `rust/RELEASE.md` 的 server 版本号（不绑定 desktop 版本）。每次主仓 GA 发版后同步 bump。

## 文档索引

- [../README.md](../README.md) — 仓库总览
- [../CLAUDE.md](../CLAUDE.md) — Chrome 限制 + Playwright 测试规则
- [../python/tests/test_extension.py](../python/tests/test_extension.py) — E2E 测试入口
