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

## Step 2 — 配置知识库执行路径

Attune 不再在 wizard 内要求用户选择具体 embedding worker。知识库向量化和重排默认走以下路径：

| 场景 | 推荐 |
|------|------|
| 普通桌面 / 笔电 | 先使用全文检索 + 云端/BYOK LLM，按需配置远端 embedding provider |
| 高性能本机 / 边缘设备 | 填入 edge scheduler endpoint，由 scheduler 管理 embedding/rerank/OCR/ASR/LLM |
| 企业或团队 | 使用统一部署的 scheduler 或云网关 |

点击"测试连接"会验证 cloud/BYOK 或 scheduler endpoint 是否就绪。

## Step 3 — 配置 LLM

LLM 用于 Chat 问答和 AI Agents。Attune 推荐以下方案（按优先级排序）：

### ★ 推荐：Attune Pro Membership

登录 Attune Pro 账号即可使用云端 LLM 网关，无需管理 API Key。

```
Endpoint：https://gateway.engi-stack.com/v1
方式：登录账号获取 token
```

### BYOK（用你自己的 API Key）

填写开发者 API 账号签发的 API Key；网页会员订阅不等同于 API 额度。BYOK endpoint 必须兼容 OpenAI chat 协议。

| 账号类型 | API 地址 |
|---------|---------|
| OpenAI API（独立 API Key 与计费） | `https://api.openai.com/v1` |
| Google AI Studio（Gemini OpenAI compatibility） | `https://generativelanguage.googleapis.com/v1beta/openai` |
| DeepSeek / Qwen / 兼容 OpenAI | 服务商提供的 Base URL |

Attune 当前不把 Anthropic 原生 Messages API 当作 OpenAI-compatible endpoint；需要 Anthropic 时使用 Attune Pro gateway 或自有兼容网关。

### Edge scheduler（本地/边缘高性能）

本地高性能平台或边缘设备用户选择 edge scheduler，地址填 `http://127.0.0.1:8090` 或局域网 scheduler 地址。Attune 只调用 scheduler 统一接口，不直连具体本地模型 worker。

> 图示占位：`![Step 3 - LLM 配置](../../static/img/wizard-step3-llm.png)`

## Step 4 — 硬件与 Scheduler 状态

Attune 显示检测到的硬件信息，并告知当前 AI 执行路径：

| 状态 | 说明 |
|------|------|
| Cloud / BYOK 已配置 | Chat 使用云端或用户自己的 OpenAI-compatible endpoint |
| Edge scheduler 已配置 | Embedding/rerank/OCR/ASR/LLM 由 scheduler 统一提供 |
| 未配置 | 基础本地全文检索可用；Chat 需要补充 cloud/BYOK 或 scheduler |

本步骤只需确认，无需手动操作。如需更换 endpoint，后续在 **Settings → AI** 中调整。

> 图示占位：`![Step 4 - 硬件底座确认](../../static/img/wizard-step4-hardware.png)`

## 向导完成后

向导完成后，Attune 会打开主界面。建议按以下顺序开始：

1. **上传一个文件**（文件标签页 → 拖拽上传）
2. **等待 embedding 完成**（顶栏后台任务指示器变为绿色）
3. **打开 Chat 问答**（Chat 标签页 → 输入问题）

详细操作见 [快速开始](./quickstart.md)。
