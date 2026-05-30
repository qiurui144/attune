# Architecture Decision Records (ADR)

`docs/adr/` 记录 attune 项目重大设计决策. 每个 ADR 一个 markdown 文件, 命名:
`NNNN-kebab-case-title.md`, NNNN 单调递增.

ADR 格式: status (proposed / accepted / superseded) + context + decision + consequences.

参考: [Michael Nygard ADR template](https://github.com/joelparkerhenderson/architecture-decision-record).

## 索引

| # | 标题 | Status | 日期 |
|---|------|--------|------|
| [0001](0001-oss-pro-boundary.md) | OSS × Pro 边界 (三产品矩阵) | Accepted | 2026-04-27 |
| [0002](0002-formfactor-llm-split.md) | FormFactor 形态感知 + LLM 默认路径分裂 | Accepted | 2026-04-30 |
| [0003](0003-gitflow-lite-double-tag.md) | GitFlow Lite + 双 tag (server / desktop 独立) | Accepted | 2026-04-30 |
| [0004](0004-app-error-accessor-pattern.md) | AppError + State accessor (lock-clone Arc) | Accepted | 2026-05-14 |
| [0005](0005-f17-full-path-pii-redact.md) | F-17 全路径 PII redact (ChatEngine + outbound audit) | Accepted | 2026-05-14 |
| [0006](0006-resource-governor-cost-tier.md) | Resource Governor + 三层成本治理契约 | Accepted | 2026-04-27 |
| [0007](0007-model-agnostic-llm-infra.md) | Model-Agnostic LLM Infra (弱模型兜底加固) | Accepted | 2026-05-22 |
