---
sidebar_position: 7
---

# LLM 配置推荐

Attune 支持多种 LLM 提供商。本页说明各方案的优缺点和配置步骤。

## 三种接入方式

### ★ 方式 1：Attune Pro Membership（推荐）

登录 Attune Pro 账号后，Attune 自动配置云端 LLM 网关，**无需管理 API Key**。

- Endpoint：`https://gateway.engi-stack.com/v1`
- 路由后端：OpenAI / Anthropic / Gemini（对用户透明）
- 用量计入会员配额，超额后可选择 BYOK 或升级计划

配置步骤：**Settings → LLM → "使用 Attune Pro 账号"→ 登录**

### 方式 2：BYOK（自带 API Key）

如果你有开发者 API 账号和 API Key，可配置 OpenAI-compatible endpoint。消费级网页订阅不应被当作 API 额度。

#### OpenAI API（独立 API Key 与计费）

ChatGPT Plus、Business/Team 等 ChatGPT 订阅不包含 API 用量；需要在 OpenAI API
平台单独开通计费并创建 API key。

```
Provider：OpenAI
API Key：sk-...
Base URL：https://api.openai.com/v1（默认，无需填写）
模型：gpt-4o-mini（推荐，低成本） / gpt-4o（高精度）
```

从 [platform.openai.com/api-keys](https://platform.openai.com/api-keys) 生成 Key。

#### Google Gemini OpenAI compatibility

```
Provider：Gemini
API Key：AIza...
Base URL：https://generativelanguage.googleapis.com/v1beta/openai
模型：gemini-2.0-flash（推荐）
```

从 [aistudio.google.com/app/apikey](https://aistudio.google.com/app/apikey) 生成 Key。

> Attune 当前的 BYOK transport 使用 OpenAI-compatible chat 协议，不直连 Anthropic 原生 Messages API。需要 Anthropic 时请使用 Attune Pro gateway 或提供 OpenAI-compatible facade 的自有网关。

#### 兼容 OpenAI 格式的提供商

Attune 支持任何 OpenAI 兼容接口：

```
Provider：OpenAI 兼容
Base URL：https://api.deepseek.com/v1
API Key：sk-...
模型：deepseek-chat
```

常见国内提供商：DeepSeek、Qwen（阿里云 DashScope）、Moonshot、Baichuan 等。

### 方式 3：Edge scheduler

适合：本地高性能平台、边缘设备、Windows/Linux 工作站统一调度、本地知识库快速问答。

Attune 只需要 scheduler base URL，例如 `http://127.0.0.1:8090`。模型下载、RVV/AVX/CUDA/DirectML/ROCm、prompt cache、拒答模板和队列调度由 scheduler 管理。

Attune 配置：

```
Provider：Edge scheduler
Base URL：http://127.0.0.1:8090
模型：由 scheduler contract / models 接口声明
```

## 按硬件 Tier 的推荐配置

| 设备 | 推荐 LLM 方案 | 预估 Chat 延迟 |
|------|-------------|--------------|
| 普通笔电（≤16 GB RAM） | Attune Pro 网关 / BYOK | 2-5 秒（网络决定） |
| 高配工作站（≥32 GB + 独显） | Edge scheduler / BYOK | 取决于 scheduler worker 与排队状态 |
| 边缘调度器设备 | Edge scheduler | 简单知识库查询目标 10 秒内 |

## 成本对比

| 方案 | 1000 次问答估算 | 隐私 |
|------|--------------|------|
| Attune Pro 网关 | 订阅内包含 | L1 脱敏后出网 |
| GPT-4o-mini BYOK | ~$0.5-$2 | L1 脱敏后出网 |
| Gemini Flash BYOK | ~$0.1-$0.5 | L1 脱敏后出网 |
| Edge scheduler | 本地设备成本 | 可全本地，不出网 |

> **提示**：无论哪种方案，Attune 在发送给 LLM 之前都会对 chunk 内容进行 L1 PII 脱敏（手机号、邮箱、身份证等替换为 placeholder），LLM 响应后自动还原。详见 [隐私模型](./privacy.md)。

## 切换 LLM

Settings → LLM → 选择新方案，填写配置后点"保存并测试"。切换立即生效，不影响已有对话历史。
