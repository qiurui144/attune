# Attune — 用户形态功能表

> 从**用户视角**列功能. 与 `FEATURES.md` (代码模块视角) 互补.
> 产品 = **Tauri 桌面应用窗口** (Windows / Linux / 未来 macOS).
> Web UI 仅用于服务器端 API 调试, **不是产品 UI**.

## 1. 产品定义

### 1.1 应用窗口

- **Tauri 桌面 GUI** (跨平台 Windows MSI / Linux AppImage+deb / macOS dmg 未来)
- 用户**所有配置**都在桌面应用窗口内完成
- Web UI demo 仅服务器端 API 调试 (`/data/company/project/attune/rust/crates/attune-server/ui-demo/`)

### 1.2 用户定位

- 面向**非专业用户** (非应用开发者)
- 默认开箱即用, 不需要任何技术配置
- 唯一暴露给用户的"配置"是 plugin (开源标准 MCP / skill / agents)

### 1.3 默认底座

#### 随二进制打包 (全配高精度, hidden 不暴露)

| 能力 | 默认引擎 | 用户可改? |
|------|---------|---------|
| Embedding | bge-m3 | ❌ |
| Reranker | bge-reranker-v2-m3 | ❌ |
| OCR | PP-OCRv5 | ❌ (但可选**场景预设**, 不暴露引擎) |
| ASR | whisper-large-v3-turbo (中文 WER 5-7%) | ❌ |
| 数据目录 | `~/.local/share/attune` (Linux) / `%APPDATA%\attune` (Win) | ❌ |

#### LLM 大模型 — 主云端 + 统一 OpenAI 兼容协议

**所有 LLM 调用统一走 OpenAI 兼容协议** (`POST /v1/chat/completions`),
不论后端是云端 OpenAI / DeepSeek / 智谱 / 通义 / Anthropic 兼容代理 / Ollama 本地.
attune 不为每个 provider 写独立 SDK — 一个 OpenAI client 走天下.

**默认不打包本地 LLM**. attune 大模型能力**主要使用云版本**:
- 普通免费用户: 自己配云端大模型 API key (在应用窗口设置面板)
- 付费用户: 云端 gateway 自动下发 (用户不持 raw key)
- 本地 LLM (可选): Ollama 自行装 (`docs/local-llm-setup.md`), 同样走 OpenAI 兼容
  endpoint `http://127.0.0.1:11434/v1`

**多模态支持** (per OpenAI Vision API):
- 文件 (PDF / DOCX / TXT / 代码): attune 自动 OCR/解析 → 拼到 user message 文本
- 图片 (PNG / JPG / WEBP): 走 OpenAI vision `content array` (`{type: image_url, image_url: {url}}`)
  · 支持 base64 data URI 或 https URL
  · 模型需支持 vision (gpt-4o / claude-3.5-sonnet / qwen-vl-max / ...)
  · 非 vision 模型: 图片自动 drop + log warning (代码 fallback 行为)

用户拍板: 默认配置全配高精度, 不降级. LLM 主云端 + OpenAI 统一协议.

## 2. 用户形态

| 形态 | 标识 | 网络要求 | LLM 来源 |
|------|------|---------|---------|
| **离线 self-host** | LoggedOut | 永不联网仅 RAG / 搜索可用; LLM Chat 需配自己的云端 API 或自装本地 LLM (Ollama) | 自配 (云 / 本地) |
| **免费会员** | Free (云端账号) | 注册/登录 + Chat 时联网 | **自己配云端大模型 API key** (默认) |
| **付费会员** | Paid (云端 license) | Chat 时联网; 30 天 license 离线缓存 | **云端 gateway 自动** (Pro 高级模型, 用户不持 raw key) |

**LLM 主云端定位** — attune 不假设用户机器能跑本地大模型. 本地 LLM 是可选高级能力, 由
用户按 `docs/local-llm-setup.md` 自行部署 Ollama.

## 3. 用户可配置项 (应用窗口暴露的仅 6 项)

| 项 | 离线 | 免费 | 付费 |
|----|:----:|:----:|:----:|
| **vault 主密码** (改密码) | ✏️ | ✏️ | ✏️ |
| **本地知识库目录关联** (隐私自管) | ✏️ | ✏️ | ✏️ |
| **云端大模型** (普通用户自己 API key) | ✏️ | ✏️ | 🔒 (云端 gateway 下发) |
| **plugin 装载** (社区 / 开发者本地) | ✏️ | ✏️ | 🔒 (云端按 license 自动 sync) |
| **plugin 卸载** | ✏️ | ✏️ | 🔒 (防误删 pro plugin) |
| **OCR 场景预设** (合同/票据/截图...) | ✏️ | ✏️ | ✏️ |

**OCR 场景预设说明**: 不同场景 (合同 / 票据 / 截图 / 古籍) 可用不同 OCR profile. 用户在 UI 看到的是**场景名称**, 不是引擎/模型/DPI 等技术参数. 应用窗口可同时配置**多个** profile.

## 4. 形态切换路径

### 4.1 离线 → 免费会员

应用窗口 → 我的账号 → 登录/注册:
- 写 `~/.config/npu-vault/license.json` (free license code)
- 设备绑定生效 (1:2 自动)
- 可配自己的云端大模型 API key (可选, 默认仍走本地)

### 4.2 免费 → 付费

应用窗口 → 我的账号 → 升级 → 跳到 accounts.attune.ai 付款:
- 付款后云端 admin 生成 paid license + 关联 entitled plugins
- 客户端自动拉新 license, 自动 sync pro plugins
- 云端大模型自动接入 (用户不持 raw API key)

## 5. plugin (开源标准 MCP / skill / agents)

| plugin 类型 | 离线 / 免费可用 | 付费可用 |
|------------|:----:|:----:|
| **社区 plugin** (公开 MCP / skill / agents) | ✅ 用户手动装 | ✅ 同时云端 sync |
| **开发者本地 plugin** (用 attune plugin-keygen 签名) | ✅ 手动 install | ✅ 手动 install |
| **官方 pro plugin** (law-pro / patent-pro 等) | ❌ 需 license | ✅ 自动 sync |

## 6. 设备绑定 1:2 (per attune-plugin-protocol §10)

后台自动, 用户不感知 (除非超 2 设备 → 弹窗选择踢下线某台).

## 7. 一键安装

| 平台 | 包格式 | 内含 |
|------|-------|------|
| Linux | AppImage (单文件) + deb (apt 包) | attune binary + 底座模型 (embedding/reranker/OCR/ASR) + poppler-utils |
| Windows | MSI installer | 同上 + Windows runtime |
| macOS (未来) | dmg + brew tap | 同上 |

**包含**: PP-OCRv5 / whisper.cpp / bge-m3 / bge-reranker-v2-m3 等底座模型 + poppler-utils + 依赖 runtime.

**不包含**: Ollama 本地 LLM (用户需用时自行装, 见 `docs/local-llm-setup.md`).
默认 attune 走云端大模型, 无需 Ollama 也能完整使用.

## 8. 日志 / 监控 (用开源高可用库)

- **日志**: `tracing` + `tracing-subscriber` (Rust 标准, 结构化日志)
- **错误**: `thiserror` (类型化错误)
- **HTTP**: `axum` + `tower-http` (Tokio 官方)
- **加密**: `argon2` + `aes-gcm` + `ed25519-dalek` (audited cryptography)
- **DB**: `rusqlite` (bundled SQLite, 跨平台)
- **全文搜索**: `tantivy` + `tantivy-jieba`
- **向量**: `usearch` (HNSW)

## 9. 与代码模块视角 (FEATURES.md) 的对应

| 用户行为 (本文) | 代码模块 (FEATURES.md) |
|----------------|---------------------|
| 应用启动 | `state.rs` + `vault::Vault` |
| 改主密码 | `vault::change_password` |
| 装 plugin | `attune-cli plugin-install` + `plugin_loader` |
| 登录云端 | `cloud_client::login` |
| 自动同步 pro plugin | `plugin_sync::sync_plugins` |
| 关联本地目录 | `attune-cli link-folder` + `/api/v1/folder-links` GET |
| 改云端大模型 (免费用户) | `routes/settings.rs::update_settings` + `SettingsLocks::cloud_llm` |
| 设备绑定 | `device_binding` + `accounts_client` |

## 10. 跨平台兼容矩阵

| 能力 | Linux x86_64 | Linux aarch64 (K3) | Windows x86_64 | macOS (未来) |
|------|:---:|:---:|:---:|:---:|
| 二进制可编译 | ✅ | ✅ (交叉) | ✅ (待 CI 验证) | ⏳ |
| AppImage / MSI / dmg | AppImage+deb | (K3 镜像) | MSI | dmg + brew |
| 默认底座模型预装 | ✅ | ✅ | ✅ | ⏳ |
| 一键安装无需额外依赖 | ✅ | ✅ | ✅ | ⏳ |
