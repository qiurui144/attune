# 本地模型一键化就绪 UX — 实施报告

**日期**: 2026-06-01
**分支**: `worktree-agent-a0a4070ea959e956f`(隔离 worktree,base = attune develop)
**Commit**: `091ce99` feat(local-model): one-click Ollama/LM Studio readiness + summary mode (zero-terminal UX)

## 最高产品原则

attune 面向非技术用户:所有非本软件但必需的第三方依赖(Ollama 运行时 / 底座模型 / LLM 模型)
必须应用内一键拉取部署,绝不让用户去终端敲 `ollama pull` 或手动装 Ollama。所有"缺失"态
配一键修复按钮,不是文字提示。

## 设计决策(§3.1-lite)

### 三态归一(core `ollama_setup.rs`)
`check_readiness(daemon_reachable, available, configured_model) -> OllamaReadiness`:
- 🔴 `DaemonDown` — `/api/tags` 不可达 → 一键安装 Ollama
- 🟡 `ModelMissing { configured, available }` — daemon 在但配置模型未下载 → 一键拉取
- 🟢 `Ready { resolved }` — 就绪(resolved 是经 `:latest` 归一匹配到的实际 tag)

`match_model` 做 `:latest` 双向归一(`qwen2.5` ⇔ `qwen2.5:latest`),且不做前缀误匹配
(`qwen2` 不命中 `qwen2.5:3b`)。纯逻辑,脱离网络可单测。

### 一键安装平台差异(`install_plan(os)`)
| 平台 | 方式 | 行为 |
|------|------|------|
| linux | `Script { curl … install.sh \| sh }` | 后台 `sh -c` 执行 + best-effort `ollama serve`;UI 轮询 readiness |
| windows | `Installer { OllamaSetup.exe }` | 当前阶段返回 manual(交 UI 弹下载;静默装留 desktop Tauri sidecar) |
| macos / 未知 | `ManualDownload` | graceful 降级给官网 https://ollama.com/download |

并发守卫:`INSTALL_IN_FLIGHT` compare_exchange,同时只跑 1 个安装(busy 态返回)。
所有失败走 tracing::warn + 用户友好 message,**不 panic**(§4.5)。

### summary 模式 off/local/cloud
新 settings 字段 `summary`,枚举校验进 `update_settings`:
- `off` — 纯检索,跳过上下文摘要(chat.rs 中等价 Raw passthrough,零 LLM 成本、无需本地模型)
- `local` — 用 `summary_llm`(本地 Ollama)
- `cloud` — 复用主 chat LLM(远端 token),避免要求笔电先装 Ollama 才能用摘要

**形态分裂默认**:K3 一体机(预装本地模型)= `local`;Laptop/Server/Unknown(LLM 默认远端)= `cloud`。
弱机用户可在 Settings 改 `off`。chat.rs 在 strategy gate 读 summary:off → 注入原文;
cloud → `llm_arc` 优先;local → `summary_llm` 优先(两者互相兜底,graceful)。

### LM Studio detect 预设
`GET /lmstudio/probe` 探本机 `:1234/v1/models`(OpenAI 兼容)。探到 → 返回可一键填的
openai_compat endpoint + 模型列表;探不到 → 给官网 https://lmstudio.ai 下载链接
(LM Studio 是 GUI,装不了一键 install,只能探测 + 引导)。

### 底座模型一键 ensure
`POST /ai-stack/ensure` 后台拉缺失底座:OCR(PP-OCRv5 ~16MB,`ppocr::ensure_models_downloaded`)
+ ASR ggml(按硬件 tier 选,`asr::ensure_whisper_model`)。Embedding/Rerank 在 vault 解锁 +
首次检索时自动加载,不在此单独拉。UI 轮询 `/ai_stack` 直到 available 翻绿。

## 阶段交付

所有阶段在单 commit `091ce99` 落地(隔离 worktree,未 push、未动 develop)。

| 阶段 | 内容 | 测试结果 |
|------|------|---------|
| 1 | Ollama 三态 + 一键 install/pull(core+server+UI) | core 17 pass;server lib 103 pass |
| 2 | summary off/local/cloud + 枚举校验 + chat.rs 接入 | summary_default 2 新测过 |
| 3 | 底座模型一键 ensure(OCR+ASR)+ About tab 按钮 | 编译 + lib 测过 |
| 4 | LM Studio detect 预设卡片 | 编译 + typecheck 过 |
| 5 | 测试 + build dist + 报告 | 见下 |

## 文件清单

新增:
- `rust/crates/attune-core/src/ollama_setup.rs`(三态 + install_plan + 17 单测)
- `rust/crates/attune-server/ui/src/components/LocalModelReadiness.tsx`(三态 UI + 一键 + 轮询)

修改:
- `rust/crates/attune-core/src/lib.rs`(注册 ollama_setup 模块)
- `rust/crates/attune-server/src/routes/llm.rs`(ollama_readiness / install_ollama / lmstudio_probe)
- `rust/crates/attune-server/src/routes/ai_stack.rs`(ensure 端点)
- `rust/crates/attune-server/src/routes/settings.rs`(summary 枚举 + 默认 + 2 新测)
- `rust/crates/attune-server/src/routes/chat.rs`(summary 模式接入压缩 gate)
- `rust/crates/attune-server/src/lib.rs`(挂载 4 新路由)
- `rust/crates/attune-server/ui/src/wizard/Step3LLM.tsx`(Ollama 卡一键化 + LM Studio 卡)
- `rust/crates/attune-server/ui/src/views/SettingsView.tsx`(summary 选择 + Ollama 就绪 + 底座一键)
- `rust/crates/attune-server/ui/src/components/index.ts`(导出)
- `rust/crates/attune-server/ui/src/i18n/{zh,en}.ts`(新 key 同步)
- `rust/crates/attune-server/ui/dist/index.html`(vite 重建产物)

## 验证证据

- `cargo test -p attune-core --lib ollama_setup` → **17 passed**(三态 / tag 归一 / 平台计划)
- `cargo test -p attune-server --lib` → **103 passed, 0 failed**(含 summary_default 2 新测)
- `cargo clippy -p attune-server -p attune-core --lib` → 无 warning / error
- `npm run typecheck` → 通过(修了 1 个 unused `scanning` var)
- `npm run build`(tsc + vite singlefile) → 通过,dist/index.html 326kB 重建
- i18n 守卫:zh/en key 集合 diff = 0;硬编码中文 grep 0 残留

## §1.3 合规

全程**未**真跑 `ollama pull` / 真启 ollama / 真装。所有测试用纯逻辑单测(mock 不需要,因为
core 逻辑不触网);install/pull/ensure 端点逻辑写出但真实拉取只在终端用户点按钮时发生。

## 阻塞项 / 后续

- Windows 应用内静默安装暂返回 manual(下载链接);desktop 端可接 Tauri sidecar 静默装(后续)。
- pull/install 进度目前靠 UI 轮询 readiness(4s × 75 上限 5min);真正的 WS 进度推送是 v.next
  增强(后端 pull 端点注释已标"进度推送由 WS 侧实现",当前未接 WS)。
- 本地模型一键 install/pull 的端到端真验证需在真实终端用户机器上做(本机 §1.3 不真跑)。
