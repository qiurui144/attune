# Wizard 5 步首启流程

attune 桌面/服务端首次启动走 5 步 wizard, 完成后进入主 UI. 本文档覆盖标准 path
+ 失败/回退分支.

> 本文档对应 v0.6.3 GA. UI 截图由 Playwright MCP 在 attune-server-headless +
> Chrome 上抓取 (E2E-2 验证, 2026-05-14).

## 入口

冷启动 attune-desktop 或 attune-server-headless → 浏览器访问 `:18900/` →

**首次** (无 vault.db): 自动进入 wizard
**非首次锁定**: lock screen, 输入主密码; 重置后回 wizard

## Step 1 — 欢迎

`Step 1: 欢迎` — 介绍 attune 定位 (私有 AI 知识伙伴, 本地决定 + 全网增强).

按钮:
- **开始设置** → Step 2 (新用户路径)
- **我有备份, 直接导入** → 跳到 import .vault-profile (老设备迁移)

## Step 2 — 主密码

`Step 2: 密码` — 设置本地 vault 加密主密码.

字段:
- **主密码** (≥12 字符, 字母+数字, 强度指示弱/中/强)
- **再次输入** (校验匹配)
- **显示/隐藏** 切换
- **高级选项** 折叠 (Device Secret 导出多设备同步)

校验:
- < 12 字符: "密码至少需要 12 个字符"
- 缺字母/数字: "密码需包含字母和数字"
- 两次不一致: "两次输入不一致"

提交后:
- 自动生成 recovery key 下载文件 `attune-recovery-key.txt` (用户务必保存,
  忘记密码靠它重置且保留数据)
- Argon2id 派生 KEK → 解 DEK (随机 32 字节) → 写入 vault.db
- 跳 Step 3

## Step 3 — AI 大脑

`Step 3: AI` — 选 LLM provider.

选项:
- **★ 云端 API** — 默认推荐. 下拉选 vendor (Attune Pro Membership Gateway /
  OpenAI / Anthropic / Gemini / DeepSeek / Qwen / 自定义 OpenAI 兼容).
  填账号 → 测试连接 → 验证登录 → 使用云端
- **暂不配置 (演示模式)** — 跳过 LLM, chat 禁用, 可浏览所有界面 + 索引文件

**FormFactor 形态感知** (per [ADR 0002](adr/0002-formfactor-llm-split.md)):
- Laptop / Server / Unknown → 默认呈现"云端 API"卡片优先
- K3 一体机 → 额外显示"本地 Ollama" 自动检测卡片

## Step 4 — 硬件

`Step 4: 硬件` — 自动检测 + 推荐 ML 模型配置.

显示:
- CPU 型号 / GPU (NVIDIA/AMD/Intel 检测) / 内存
- 推荐 embedding 模型 (bge-m3 / bge-base / bge-small 按 RAM tier)
- 推荐 ASR (whisper-large-v3-turbo-q5 / whisper-medium-q5 按 GPU tier)

按钮 **应用推荐 →** 跳 Step 5

## Step 5 — 数据源

`Step 5: 数据` — 从哪里开始积累知识库.

三选项:
- **📂 绑定文件夹** — Tauri 桌面版用文件夹弹窗选目录, 自动监听+入库
- **📥 导入 .vault-profile** — 从老设备备份恢复 (vault + 索引 + plugin 全套)
- **→ 跳过, 先看看** — 之后随时在设置中添加

按 **完成 · 进入 Attune →** 进主 UI.

## 主 UI 入口

进入后 sidebar 7 个 tab:
- 新对话 / 条目 / 项目 / 远程目录 / 知识全景 / 技能 / 插件市场 / 设置

设置 → 6 子 tab: 通用 / AI 大脑 / 数据 / **会员** / 隐私 / 关于

会员 tab 新加: **高级 · 自部署 cloud 后端** 折叠区 (v0.6.3 FEAT-1) — 自部署
attune-cloud-* 容器时填 3 URL: accounts (会员登录) / gateway (LLM token) /
pluginhub (plugin 市场). 默认走 attune.ai 公共云.

## 失败 / 回退路径

| 现象 | 原因 | 处理 |
|------|------|------|
| Step 2 提交后 500 | vault.db 写权限不足 (~/.local/share/attune/) | 检查目录权限 + 用户身份 |
| Step 3 测试连接失败 | API key 无效 / 网络问题 / endpoint 不对 | 选"暂不配置"继续, 进 UI 后 Settings → AI 大脑 重试 |
| Step 4 GPU 没检测到 | NVIDIA 驱动未装 / Intel iGPU 内核版本低 | 选 "应用推荐" 走 CPU; v0.7 wizard 加 "驱动一键安装" link |
| Step 5 跳过后无知识库 | 用户选了 "跳过, 先看看" | 进主 UI 后 设置 → 数据 → 本地知识库目录 → 选目录 |

## 完整重置

忘记主密码 + 没 recovery key → 锁屏 "无恢复密钥? 清空并重置本地知识库" 按钮
→ 二次确认对话框 "请输入 RESET 确认" → 全清 vault.db + 重启 wizard.

⚠ **不可恢复**: 已加密 item 全失. recovery key 是唯一兜底.
