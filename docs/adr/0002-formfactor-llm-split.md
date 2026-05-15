# ADR 0002: FormFactor 形态感知 + LLM 默认路径分裂

- **Status**: Accepted
- **Date**: 2026-04-30

## Context

attune 部署形态多样:
- Laptop (Win/Linux/macOS) — 用户笔电, 主流
- Server (headless / NAS) — 多用户
- K3Appliance — 我们家的 RISC-V 一体机, 内置推理服务
- Unknown — fallback

不同形态的 LLM 默认策略不一样:
- Laptop / Server: 本地 LLM 安装维护成本高 (模型选型 / GPU 配置 / ROCm bug),
  默认走云端 token 更友好 (用户付月费即用)
- K3Appliance: 出厂预装 Ollama + 模型 (镜像构建时打入), 默认走本地

之前一刀切默认本地 Ollama, 笔电用户 wizard 卡在 "ollama 没装" 错误.

## Decision

attune-core 加 `FormFactor` enum + `HardwareProfile::detect()` 启动一次检测.
LLM 构造逻辑分支:

```rust
match form_factor {
    Laptop | Server | Unknown => 远端 token (settings.llm.endpoint 配)
    K3Appliance => 本地 Ollama auto-detect
}
```

用户可在 settings.llm.endpoint 手动覆盖. K3Appliance 检测条件: RISC-V arch
+ 特定 PCI device id (rv-spacemit AI accelerator). 错检风险低.

## Consequences

**好处**:
- Laptop 用户 wizard 流程顺 (不需要装 Ollama)
- K3 一体机出厂即可对话 (符合"零配置"承诺)
- LLM 配置成本透明: cloud token 用户买 quota, K3 用户买硬件

**代价**:
- Laptop 用户必须配 cloud token 才能 chat (没有 demo mode)
- detect 错时 fallback: K3 误判 Laptop 走 cloud (浪费 token); Laptop 误判
  K3 试 Ollama (报 503). 用户可手 override
- 测试矩阵: 4 form factor × N 配置组合

## Implementation 落地

- v0.6.1 (commit 461c4c7): FormFactor 形态感知 + 8 unit test 端到端覆盖
- attune-server::state::build_llm_from_settings 实施分支
- routes/status.rs 暴露 form_factor 到 wizard 让 UI 决策
