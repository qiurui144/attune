# 致谢

[English](ACKNOWLEDGMENTS.md) · [简体中文](ACKNOWLEDGMENTS.zh.md)

Attune 站在巨大的开源社区肩膀上：项目、论文、技术博客都给了我们具体的设计养料。本文件列出我们采纳的**具体设计模式**及其来源。深表感谢。

依赖软件的完整 license 归属请见 `Cargo.lock`（Rust）和 `package.json`（Chrome 扩展 / 桌面）。本文档专注于**设计与算法层面的思想来源**，不重复列依赖传递。

---

## 基础灵感

| 层面 | 影响来源 | 为什么 |
|------|---------|-------|
| 加密 vault 隐喻 | **1Password**（仅设计语言） | "屏保即锁屏" + Argon2id 密钥派生 UX |
| Local-first PKM 体感 | [**Obsidian**](https://obsidian.md), [**Logseq**](https://github.com/logseq/logseq) | 反向链接、本地 Markdown 所有权、"笔记属于你"姿态 |
| 单二进制分发 | [**Caddy**](https://caddyserver.com), [**SQLite**](https://sqlite.org) | 零运行时依赖的 Rust 二进制理念 |

---

## 按 feature 归属

### H1 资源治理（commit `2bc558c`，2026-04-27）

| 来源 | 我们采纳的 |
|------|-----------|
| [GuillaumeGomez/sysinfo](https://github.com/GuillaumeGomez/sysinfo)（MIT） | 跨平台 CPU/RAM 采样原语 |
| Linux `nice(1)` / `ionice(1)` | "好公民"协作式调度哲学 — 系统忙时让让，不靠每任务配额 |
| [logseq/logseq](https://github.com/logseq/logseq)（反例） | 显式针对 Logseq/Obsidian 索引重建拖系统的常见吐槽设计 — 因此每任务 budget + 顶栏 Pause |

### A1 Memory Consolidation MVP（commit `71a714f`，2026-04-27）

| 来源 | 我们采纳的 |
|------|-----------|
| [mem0ai/mem0](https://github.com/mem0ai/mem0)（Apache-2.0） | "AI memory layer"叙事；episodic memory 作为 chunk 之上独立数据模型 |
| [skill_evolution.rs]（attune 自有，2026-03） | 三阶段锁释放（prepare → generate → apply）由 SkillEvolver 已建立；A1 直接镜像 |

### W3 Batch A — F1 / F2 / F4 / C1（2026-04-27）

| 来源 | 我们采纳的 |
|------|-----------|
| [吴师兄文章](https://mp.weixin.qq.com/s/YNcfSN0uv1c1LsLPzgB0jw) §6 高频 query 缓存 | C1 web_search_cache 表 — SHA-256(query) 主键 + DEK 加密 results + 30 天 TTL 模式 |
| [linkwarden/linkwarden](https://github.com/linkwarden/linkwarden)（AGPL-3.0） | "fetch 时快照"心智 — 一旦 query 入缓存，TTL 窗口内视为不变 |
| attune 自有 `chunk_summaries` 表 | F2 sidecar 模式 — 按 `(item_id, chunk_idx)` 主键的独立表，不扩 core schema（避免老 vault `.encbin` 迁移风险）。chunk_summaries 首创（W2 之前），chunk_breadcrumbs 复用（W3 batch A） |

### J 系列 — RAG 生产工程化（W2-W4 进行中）

| 来源 | 我们采纳的 |
|------|-----------|
| [吴师兄: 《鹅厂面试官追问：你的 RAG 能跑通 Demo？那让它在 5000 份文档里稳定答对，试试看》](https://mp.weixin.qq.com/s/YNcfSN0uv1c1LsLPzgB0jw) | 8 个生产工程化杠杆；具体 J1（chunk 路径前缀）、J3（显式阈值调优 0.65/0.72/0.78）、J5（强约束 prompt + 置信度）、J6（公开召回率 + 答非所问率作为 KPI）。量化锚：召回 0.62→0.91、幻觉 18%→7% |
| [explodinggradients/ragas](https://github.com/explodinggradients/ragas)（Apache-2.0） | J6 metric 名 + 公式：Faithfulness / Answer Relevancy / Context Precision / Context Recall — 用业内标准而非自创 |
| [CRAG paper（arXiv:2401.15884）](https://arxiv.org/abs/2401.15884) | J5 三分类检索门控：correct / incorrect / ambiguous → 分支动作（降阈值二次检索） |
| [Self-RAG paper（Asai 等）](https://arxiv.org/abs/2310.11511) | J5 token 化置信度（attune 用 1-5 分简化版，更适合 chat） |
| [stanfordnlp/dspy](https://github.com/stanfordnlp/dspy)（MIT） | 仅作离线阈值调优的灵感来源。我们**不**在生产跑 DSPy compile（per attune 成本契约） |

### K 系列 — 开源生态借鉴（W5-W10 计划）

| 功能 | 来源 | 我们采纳的 |
|------|------|-----------|
| **K1 Sleeptime 进化代理** | [letta-ai/letta](https://github.com/letta-ai/letta)（Apache-2.0）— "sleeptime agent"模式；[noahshinn/reflexion](https://github.com/noahshinn/reflexion)（MIT）— verbal feedback 长期 buffer | primary chat agent 不为 memory 压缩阻塞；独立后台 agent 跑跨会话反思 |
| **K2 Parse Golden Set** | [Readwise Reader](https://blog.readwise.io/the-next-chapter-of-reader-public-beta/)（商业，仅设计） | 200 篇真实页面 parse benchmark 方法论；CI 回归 < 95% 准确率不准发版 |
| **K3 AGENTS.md 兼容** | [continuedev/continue](https://github.com/continuedev/continue)（Apache-2.0）— `.continue/checks/*.md` + `create_rule_block`；[PatrickJS/awesome-cursorrules](https://github.com/PatrickJS/awesome-cursorrules)（MIT） | Plugin SDK 同时读 attune `plugin.yaml` 和社区 `AGENTS.md` / `.continue/rules/*.md` — 零成本生态接入 |
| **K4 CRDT 多端同步** | [anytype-io/any-sync](https://github.com/anytype-io/any-sync)（协议层 MIT；Anytype 客户端走 Anytype Tech License） | AnySync 架构作 v0.7+ 探索参照 — 尚未承诺实现 |
| **K5 Items Key 撤销** | [standardnotes/app](https://github.com/standardnotes/app)（AGPL-3.0）— 004 spec 分层密钥模型 | master key 加密 items keys，items key 加密数据；按 Project / Note 独立 key，云备份可按项撤销 |

### Hybrid Intelligence（C 系列）

| 来源 | 我们采纳的 |
|------|-----------|
| [searxng/searxng](https://github.com/searxng/searxng)（AGPL-3.0） | C1 自托管 meta search 后端（用户启用 web 增强时）— 不重造搜索聚合器 |
| [assafelovic/gpt-researcher](https://github.com/assafelovic/gpt-researcher)（MIT） | C3 wrap 现成 MCP server，不重写自动研究 |

### 行业插件生态（E 系列）

| 来源 | 我们采纳的 |
|------|-----------|
| [langchain-ai/langgraph](https://github.com/langchain-ai/langgraph)（MIT） | E2 plugin SDK：StateGraph + Node/Edge + checkpointing 概念（自实现迷你版，不依赖 langgraph） |
| [All-Hands-AI/OpenHands](https://github.com/All-Hands-AI/OpenHands)（MIT） | E1 marketplace plugin manifest YAML schema |

---

## 反例（明确避开）

这些项目教了我们**不该做什么** — 同样宝贵：

| 反模式 | 来源 | 我们的对策 |
|--------|------|-----------|
| memory 全交给 LLM function call（小模型崩） | Letta v1 agent loop | attune 保留确定性 indexer，仅 sleeptime / 显式分析阶段调 LLM |
| 编译期 prompt 优化每次 $20-50 | DSPy 默认 `compile()` | DSPy 仅作开发期灵感；运行期用手调 + RAGAS 验证 |
| 默认 deep research 模式（每 query 5-20 次 LLM） | Perplexica / gpt-researcher 自动模式 | attune 任何昂贵多 LLM 动作必须用户显式触发（成本契约 §2） |
| 安装包捆绑 LLM 模型（3GB+） | jan.ai 默认安装 | attune 仅捆绑 embedding/rerank/ASR/OCR 底座；LLM 默认走远端 token |
| 用户活跃时跑索引重建 | Logseq、Obsidian | 每任务 H1 governor 在系统 CPU > 阈值时暂停后台 |

---

## 如何在你的项目里 cite attune

如果 attune 影响了你的项目，我们感谢相同级别的归属。建议格式：

```
Inspired by Attune (https://github.com/qiurui144/attune) —
specifically the [feature name] design from [SHA or release tag].
```

---

## 本文件维护规则

- **每个采纳外部模式的 PR 在合入前必须加条目**
- Commit message 加 `Inspired-by: <project>(<URL>)` 行作为平行记录
- 本文件每季度复查 license URL 与死链
- 双语：英文版为权威；`ACKNOWLEDGMENTS.zh.md` 为翻译

最近更新：2026-04-27（W2 batch 1 进行中）。
