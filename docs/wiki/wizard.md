---
sidebar_position: 3
---

# 配置向导（Setup Wizard）

首次启动 Attune 时，系统会自动打开四步配置向导。向导完成后可随时通过 **Settings → 重新运行向导** 修改配置。

## Step 1 — 创建 Vault 密码

Vault 是 Attune 的本地加密数据库，所有知识、配置和凭据都保存在其中。

- 密码使用 **Argon2id** 派生密钥，数据用 **AES-256-GCM** 加密
- 密码**不上传**到任何服务器，丢失密码无法找回（设计如此，保证数据只有你能访问）
- 建议使用 12 位以上随机密码，并备份到密码管理器

> 图示占位：`![Step 1 - 设置 Vault 密码](../../static/img/wizard-step1-vault.png)`

## Step 2 — 选择 Embedding 模型

Attune 根据当前机器的 RAM 和 GPU 自动推荐 Embedding 模型：

| 硬件 | 推荐模型 | 说明 |
|------|---------|------|
| ≥16 GB RAM + 独显/NPU | `bge-m3`（Ollama） | 最高精度，中英双语 |
| 8-16 GB RAM | `bge-base-zh`（ORT 本地） | 平衡速度与精度 |
| <8 GB RAM | `bge-small-zh`（ORT 本地） | 轻量，适合笔记本 |

点击"测试连接"会验证模型是否就绪。若使用 Ollama，Attune 会提示你先运行 `ollama pull bge-m3`。

## Step 3 — 配置 LLM

LLM 用于 Chat 问答和 AI Agents。Attune 推荐以下方案（按优先级排序）：

### ★ 推荐：Attune Pro Membership

登录 Attune Pro 账号即可使用云端 LLM 网关，无需管理 API Key。

```
Endpoint：https://gateway.engi-stack.com/v1
方式：登录账号获取 token
```

### BYOK（用你自己的 API Key）

如果你已有以下付费账号，对应 Plan 通常附带 API 额度：

| 账号类型 | API 地址 |
|---------|---------|
| OpenAI（ChatGPT Plus / Team） | `https://api.openai.com/v1` |
| Anthropic（Claude Pro） | `https://api.anthropic.com` |
| Google（Gemini Advanced） | `https://generativelanguage.googleapis.com` |
| DeepSeek / Qwen / 兼容 OpenAI | 服务商提供的 Base URL |

### 本地 Ollama（高级）

K3 一体机用户或配备独显的笔电用户，可选择 Ollama 本地 LLM：

```bash
# 先安装并启动 Ollama
ollama pull qwen2.5:3b   # 适合 8-16 GB RAM
ollama serve
```

然后在 Wizard Step 3 选择"本地 Ollama"，地址填 `http://localhost:11434`。

> 图示占位：`![Step 3 - LLM 配置](../../static/img/wizard-step3-llm.png)`

## Step 4 — 硬件感知底座

Attune 显示检测到的硬件信息，并告知哪些本地底座已就绪：

| 底座 | 用途 | 说明 |
|------|------|------|
| Embedding（bge） | 文档向量化 | 安装包已捆绑 ORT 模型 |
| Rerank（bge-reranker） | 搜索精排 | 安装包已捆绑 ORT 模型 |
| ASR（whisper.cpp） | 音频转写 | 安装包已捆绑，首次使用下载模型 |
| OCR（PP-OCRv5） | 图片 / 扫描 PDF 文字识别 | 安装包已捆绑 |

本步骤只需确认，无需手动操作。如需更换模型版本，后续在 **Settings → Models** 中调整。

> 图示占位：`![Step 4 - 硬件底座确认](../../static/img/wizard-step4-hardware.png)`

## 向导完成后

向导完成后，Attune 会打开主界面。建议按以下顺序开始：

1. **上传一个文件**（文件标签页 → 拖拽上传）
2. **等待 embedding 完成**（顶栏后台任务指示器变为绿色）
3. **打开 Chat 问答**（Chat 标签页 → 输入问题）

详细操作见 [快速开始](./quickstart.md)。
