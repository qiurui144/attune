---
sidebar_position: 7
---

# LLM 配置推荐

Attune 支持多种 LLM 提供商。本页说明各方案的优缺点和配置步骤。

## 三种接入方式

### ★ 方式 1：Attune Pro Membership（推荐）

登录 Attune Pro 账号后，Attune 自动配置云端 LLM 网关，**无需管理 API Key**。

- Endpoint：`https://gateway.attune.ai/v1`
- 路由后端：OpenAI / Anthropic / Gemini（对用户透明）
- 用量计入会员配额，超额后可选择 BYOK 或升级计划

配置步骤：**Settings → LLM → "使用 Attune Pro 账号"→ 登录**

### 方式 2：BYOK（自带 API Key）

如果你已有以下付费账号，对应 Plan 通常附带 API 额度：

#### OpenAI（ChatGPT Plus / Team）

```
Provider：OpenAI
API Key：sk-...
Base URL：https://api.openai.com/v1（默认，无需填写）
模型：gpt-4o-mini（推荐，低成本） / gpt-4o（高精度）
```

从 [platform.openai.com/api-keys](https://platform.openai.com/api-keys) 生成 Key。

#### Anthropic（Claude Pro）

```
Provider：Anthropic
API Key：sk-ant-...
模型：claude-haiku-3-5（推荐，低成本） / claude-sonnet-4（高精度）
```

从 [console.anthropic.com/keys](https://console.anthropic.com/keys) 生成 Key。

#### Google（Gemini Advanced）

```
Provider：Gemini
API Key：AIza...
Base URL：https://generativelanguage.googleapis.com（默认）
模型：gemini-2.0-flash（推荐）
```

从 [aistudio.google.com/app/apikey](https://aistudio.google.com/app/apikey) 生成 Key。

#### 兼容 OpenAI 格式的提供商

Attune 支持任何 OpenAI 兼容接口：

```
Provider：OpenAI 兼容
Base URL：https://api.deepseek.com/v1
API Key：sk-...
模型：deepseek-chat
```

常见国内提供商：DeepSeek、Qwen（阿里云 DashScope）、Moonshot、Baichuan 等。

### 方式 3：本地 Ollama

适合：K3 一体机 / 配备独显的工作站 / 对隐私要求极高的用户。

```bash
# 1. 安装 Ollama
curl -fsSL https://ollama.com/install.sh | sh

# 2. 拉取模型（按 RAM 选择）
ollama pull qwen2.5:3b    # 8 GB RAM
ollama pull qwen2.5:7b    # 16 GB RAM

# 3. 启动服务
ollama serve
```

Attune 配置：

```
Provider：Ollama
Base URL：http://localhost:11434
模型：qwen2.5:3b（或已 pull 的模型名）
```

## 按硬件 Tier 的推荐配置

| 设备 | 推荐 LLM 方案 | 预估 Chat 延迟 |
|------|-------------|--------------|
| 普通笔电（≤16 GB RAM） | Attune Pro 网关 / BYOK | 2-5 秒（网络决定） |
| 高配工作站（≥32 GB + 独显） | Ollama qwen2.5:7b 本地 | 1-3 秒 |
| K3 一体机 | Ollama qwen2.5:3b 本地 | 3-5 秒 |

## 成本对比

| 方案 | 1000 次问答估算 | 隐私 |
|------|--------------|------|
| Attune Pro 网关 | 订阅内包含 | L1 脱敏后出网 |
| GPT-4o-mini BYOK | ~$0.5-$2 | L1 脱敏后出网 |
| Gemini Flash BYOK | ~$0.1-$0.5 | L1 脱敏后出网 |
| Ollama 本地 | 仅电费 | 全本地，不出网 |

> **提示**：无论哪种方案，Attune 在发送给 LLM 之前都会对 chunk 内容进行 L1 PII 脱敏（手机号、邮箱、身份证等替换为 placeholder），LLM 响应后自动还原。详见 [隐私模型](./privacy.md)。

## 切换 LLM

Settings → LLM → 选择新方案，填写配置后点"保存并测试"。切换立即生效，不影响已有对话历史。
