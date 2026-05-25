# Support & SLA

attune 的支持响应分级。OSS 主线 best-effort，Attune Pro 会员有更明确的响应承诺（见 attune-pro 仓 SUPPORT.md）。

本文档面向所有 attune 用户（OSS / Pro）。

## 目录

- [严重程度分级](#严重程度分级)
- [响应通道](#响应通道)
- [上报模板](#上报模板)
- [非支持范围](#非支持范围)

## 严重程度分级

按问题对用户的影响划分。响应时间为**首次 ack** 的 best-effort，非修复时长。

### P0 — 数据/安全级（24 小时响应）

触发条件（任一）:

- Vault 解锁失败、密码正确但 unlock loop / panic / 数据消失
- 数据丢失：item / annotation / project 入库后未在 UI 出现（且本地无 corruption）
- 加密失效：本地 vault 文件能被未经授权读出明文（Argon2 / AES-256-GCM 路径出现 bug）
- Secrets 泄露：发现 token / key / password 出现在 log / commit / crash report 中

**响应**：24 小时内由维护者 ack + 起草调查计划。critical 同时挂 GitHub Security Advisory。

**上报**：私下邮件 happyqiuqiu9604@gmail.com（**不要**开 public issue，避免漏洞被利用）。

### P1 — 核心功能不可用（48 小时响应）

触发条件（任一）:

- Chat 不工作（loading 永不结束 / 输出乱码 / LLM 路径全失败）
- 搜索不工作（query 返回 0 / 返回错误 / index 损坏）
- 文件上传 / 解析 fail（PDF / DOCX / MD 已知 happy-path 文件解析挂）
- Agent 报告异常（law-pro / 任何已发布 agent F1 在 golden set 上跌出 RELEASE.md 阈值）
- Tauri 桌面端启动崩溃 / wizard 卡住无法完成

**响应**：48 小时内 ack + 复现尝试。复现成功后 7 天内出修复 PR / 给出 workaround。

**上报**：[GitHub Issues](https://github.com/qiurui144/attune-core/issues) — 用 `bug` label。

### P2 — UI / 文档（1 周响应）

触发条件（任一）:

- UI 错位 / 按钮失效 / i18n 错语言
- 文档过时 / 漂移（README / DEVELOP / RELEASE 与代码不一致）
- 性能退化（chat / search 响应时间 > 上一 release 的 2x）
- Web UI 国际化 key 缺失 / 中英混杂

**响应**：1 周内 ack + triage。修复时机进下一个 minor。

**上报**：GitHub Issues — `ui` / `docs` / `i18n` / `performance` label。

### P3 — Feature Request（无 SLA）

包括:

- 新连接器 / 新 connector / 新平台支持
- 新 agent / 新 capability
- UX 改善建议
- 第三方集成请求

**响应**：路线图评审周期内（每个 minor 规划时）。可能被接收 / 推到 v.next / 标记 won't fix。

**上报**：GitHub Discussions（preferred）或 Issues 加 `enhancement` label。

## 响应通道

| 渠道 | 用途 | 链接 |
|------|------|------|
| GitHub Issues | P1 / P2 bug 上报 | https://github.com/qiurui144/attune-core/issues |
| GitHub Discussions | P3 feature / 使用问题 / 讨论 | https://github.com/qiurui144/attune-core/discussions |
| Email（私下） | P0 安全漏洞 / 数据安全 issue | happyqiuqiu9604@gmail.com |
| Discord（占位） | 实时讨论（待开启） | TBD |

## 上报模板

P0 / P1 上报必含:

1. **attune 版本**：`attune --version` 输出 + desktop 版本（Settings → About）
2. **OS 环境**：Linux 发行版 / Windows 版本号 / macOS（如有）
3. **复现步骤**：从 fresh install / 已有 vault 起算的完整步骤
4. **预期 vs 实际**：你期望看到什么 / 实际看到什么
5. **日志**：`~/.attune/logs/server.log` 末 200 行 + 浏览器 console（如涉及 UI）
6. **截图 / 录屏**：UI 问题强烈建议附

P2 / P3 至少 1 / 3 / 4。

## 非支持范围

以下场景**不在** attune OSS 支持范围内:

- **个性化部署咨询**（如 K3 一体机适配 / 企业 LDAP 接入）→ 联系 Attune Pro 商业支持
- **第三方 LLM provider 自身故障**（OpenAI / Anthropic / Gemini API 不可用） → 上报对应厂商
- **用户硬件故障**（GPU 驱动 / Ollama runtime 装错）→ 上游社区
- **超出 README 文档范围的二次开发**（魔改源码后异常）→ 自行排查

## 历史 incident

[RELEASE.md](../RELEASE.md) 各版本节会记录已确认的 known issues + workaround。
有疑似 incident 时优先查这里，可能已有处理建议。

## 内部 SLA 工具（维护者用）

- `gh issue list --label bug --state open` — 待处理 P1 队列
- 每周三 review 会议过一遍 P0 / P1 队列
- 季度回顾响应时间是否符合 SLA（公开数据进 RELEASE.md）
