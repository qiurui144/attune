---
sidebar_position: 8
---

# Plugins（插件）

Attune 支持通过插件包（Plugin Pack）扩展行业专属能力。

## 插件架构

```
Attune OSS（通用知识库）
    ↓
  PluginHub 分发平台
    ↓
  attune-pro 插件包（行业专属）
    ├── law-pro       律师助手
    ├── medical-pro   医疗文档（规划中）
    ├── presales-pro  售前支持（规划中）
    └── scholar-pro   学术研究（规划中）
```

每个插件包包含：

- `plugin.yaml`：声明 id / 名称 / PII 规则 / Chat 触发词 等元数据
- `prompt.md`：行业专属 System Prompt
- `agents/`：行业 Agents（可选）

## 安装插件

### 前提

- Attune Pro 订阅（30 天免费试用）
- 网络可访问 `pluginhub.engi-stack.com`（离线场景见下文）

### 安装步骤

1. **Settings → Plugins → 浏览插件市场**
2. 选择插件，点击"安装"
3. Attune 向 PluginHub 验证 License，通过后下载并安装
4. 重启或刷新即生效

安装完成后，插件的行业 Prompt 和 Agents 自动可用。

### CLI 安装（高级）

```bash
attune plugin install law-pro --license-key sk-pro-...
attune plugin list
attune plugin update law-pro
```

## law-pro 律师助手

`law-pro` 是目前唯一已 GA 的专业插件包（v1.0）。

### 功能概览

| 类型 | 数量 | 说明 |
|------|------|------|
| 确定性 Agent | 11 个 | 基于规则，零 token 成本 |
| LLM Extractor Agent | 3 个 | 事实抽取，按需触发 |
| AI 批注 Agent | 4 个 | 文档标注 |
| Office Helper（法律版） | 1 个 | 合同生成辅助 |

详见 [Agents 详介](./agents.md)。

### Chat 触发

安装 `law-pro` 后，在 Chat 中输入以下类型问题会自动激活法律专属路由：

- "帮我计算这份劳动合同的违约金"
- "分析这份房屋租赁合同的关键条款"
- "提取这份诉状中的事实和诉求"

## 离线场景

K3 一体机自带**私有 PluginHub 实例**，所有插件离线分发，不需要公网访问。

普通安装包在离线时使用本地缓存的插件（24 小时 TTL），超时后降级为"仅核心功能"直到重新联网验证。

## 自定义插件（开发者）

如果你是开发者，可以基于 `plugin.yaml` 格式开发自定义插件：

```yaml
# plugin.yaml 最小示例
id: my-custom-plugin
name: 我的行业插件
version: 0.1.0
min_attune_version: "1.0"

pii_patterns:
  - pattern: "证件号\\d{8}"
    label: CUSTOM_ID

chat_trigger:
  keywords: ["合同", "协议", "条款"]
  domain: legal
```

详见 [PluginHub 开发者文档](/pluginhub/)。

## 版本兼容性

| Attune 版本 | 支持的插件 API 版本 |
|------------|------------------|
| v1.0.x | plugin.yaml v1.0 |
| v0.9.x | plugin.yaml v1.0（部分功能不可用） |
| ≤v0.8.x | 不支持插件系统 |
