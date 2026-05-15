# ADR 0001: OSS × Pro 边界 (三产品矩阵)

- **Status**: Accepted
- **Date**: 2026-04-27

## Context

attune 初期方向不明: 是只做开源个人知识库, 还是行业 SaaS, 还是企业部署?
- 律师 / 医生 / 学者等垂直场景需求很具体 (法律案号 / 病历术语)
- 但通用个人用户 (学生 / 开发者 / 写作者) 不需要这些
- lawcontrol (另一仓) 已做 B2B 律所小团队 SaaS

混在一起会拖累三方: OSS 用户嫌行业绑定臃肿, 行业用户嫌 OSS 不够深,
B2B 律所小团队需要的协作/审批与个人产品冲突.

## Decision

**三产品矩阵**, 技术独立 + 战略协作:

| 产品 | 用户群 | 形态 |
|------|--------|------|
| **attune (OSS)** | 个人通用 | 桌面/扩展, 零行业绑定 |
| **attune-pro** | 个人行业增强 | Plugin pack 装载到 attune (律师/医生/学者/售前/工程师/专利) |
| **lawcontrol** | 律所 B2B 小团队 | Django + Vue + 19 容器 SaaS |

OSS attune 完全独立 — 不调 lawcontrol API / 不复用 lawcontrol 代码 / 数据完全隔离.
可参考 lawcontrol plugin 设计模式但实现独立.

## Consequences

**好处**:
- OSS 用户友好 (零行业绑定, 通用)
- 行业用户可选 plugin pack 升级 (商业化路径)
- 律所小团队走独立 SaaS (协作场景)

**代价**:
- attune-pro 与 attune 是配套关系, 跨仓 release coordination 复杂
- 律所同时用 lawcontrol + attune 需要 export/import 手动桥接
- 同一团队三个产品, dev 资源稀释

## Implementation 落地

- v0.6.0-rc.2 (commit ee859a4): 边界瘦身 — 删 OSS attune 的 4 个 builtin 行业 yaml +
  EntityKind::CaseNo + CHAT_TRIGGER_KEYWORDS 律师专属 const
- 全部迁到 attune-pro/plugins/&lt;vertical&gt;-pro/
- CLAUDE.md §"OSS attune 边界规则" 固化判定: feature 进 OSS 当且仅当对任何
  领域个人通用用户都有价值
