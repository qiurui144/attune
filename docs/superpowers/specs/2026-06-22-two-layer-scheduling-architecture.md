# 两层调度架构 — 模型调度(K3) × 硬件调度(本地) 规划

> 2026-06-22 · 架构规划(草案,待评审)· **impl 等 vlm-llm-bench 结果落地后按验证数据开发**。
> 用户拍板的职责切分:**K3 调度层 = 走哪个模型;本地调度层 = 走哪个硬件更优(含系统功耗)**。

---

## 0. 两层调度 — 职责切分(SSOT)

| 层 | 决策**什么** | 输入 | 实现位置 | 状态 |
|:--|:--|:--|:--|:--|
| **L_model 模型调度** | 走哪个模型 / 本地 vs 云 / 大小档 | 能力图 + capacity/load + 隐私级 + 账户配额/成本 | **K3**: k3-scheduler :8090(+ A100/X100/IME2 仲裁);attune governor 经 edge-cloud Model 1 协同 | ✅ 已建 |
| **L_hw 硬件调度** | 走哪个加速器更优(NPU/iGPU/dGPU/CPU)+ 是否并行 | **bench 实测 perf/质量/功耗** + 硬件在场 + **功耗/热/电池/争用** | **本地**: attune `infer/accel`(PC 自管);K3 这层在 scheduler 的设备仲裁 | 🟡 现状=单 EP/task,**缺多加速器并行 + 功耗感知** |

**两层垂直组合**:`L_model 选出模型(+本地/云)` → 若本地 → `L_hw 为该能力选加速器`。
- **K3**:两层都在设备侧(k3-scheduler 同时管"哪个模型"+"A100/X100/IME2 哪个跑");attune 只 submit 到 :8090。
- **PC(个人版)**:`L_model` 退化为"本地 vs 云会员"(简单);`L_hw` 由 attune **本地自管**(无独立 scheduler,attune infer 层就是本地硬件调度器)。← **本规划的主体是 PC 的 L_hw**。

---

## 1. 现状(grounded)与缺口

- `infer/accel.rs::recommend_ep_chain(hardware, task)` → **每 task 选 1 个最优 EP**(EpChoice: Cuda/DirectMl/OpenVino{Cpu,Gpu,Npu}/Rocm/VitisAi/Cpu,末位 CPU 兜底)。
- `model-catalog.default.yaml` 每能力 1 个 `ep:`(per tier 单选;OCR EP 修复=Intel→OpenVino/AMD→DirectMl 已落)。
- **缺口**(本规划要补):
  1. **多加速器并行分配**:一台机有 NPU+iGPU+dGPU → 可 OCR(NPU) ∥ embedding(iGPU) ∥ LLM(dGPU) **同时跑**;现在是"每 task 单选 1 EP",无"把不同能力铺到不同加速器"的概念。
  2. **功耗/热/电池感知**:recon 确认 attune **零** power/thermal/battery 感知。用户"走哪个硬件更优(考虑系统功耗)"硬需这层。
  3. **争用感知**:两能力都想要 iGPU → 排队/或溢到次优加速器(embedding→iGPU 忙→CPU)。
  4. **bench 数据驱动**:(能力×加速器) 的偏好 + perf/功耗数字应来自 vlm-bench 实测,编码进 catalog,不靠直觉。

---

## 2. 本地硬件调度器(L_hw)设计

### 2.1 核心抽象:能力→加速器分配(非单 EP)
`recommend_ep_chain(hw, task)` → **`assign_accelerators(hw, requested_capabilities[], power_state) -> Map<capability, AcceleratorPlan>`**
- 输入:本机在场加速器集 + 本次要跑的能力集 + 当前电源/热状态。
- 输出:每能力分到哪个加速器(+ 次优 fallback 链),**目标:铺开到不同加速器并行 + 各能力落到其性价比/功耗最优档**。
- 例(Intel Core Ultra:NPU+Arc iGPU):OCR→NPU、ASR→NPU(NPU 串行队列)、embedding→iGPU、reranker→iGPU、LLM→iGPU 或云。
- 例(AMD Ryzen AI:XDNA NPU + RDNA iGPU + 可选 dGPU):OCR/ASR→XDNA(VitisAI)、embedding/reranker→RDNA(DirectML/ROCm)、LLM→dGPU 或云。

### 2.2 加速器亲和表(bench 驱动,catalog 扩展)
扩展 INT-3 catalog:每 **(能力 × 加速器)** 带 `{latency_p50, throughput, quality(CER/WER/F1), power_w, energy_per_inf_j, source}`(bench 实测,§6.3 有源)。
- 选择函数按 **目标(perf-optimal / energy-optimal)** + 质量 floor 排序:perf 模式取最快达 floor;energy 模式取 energy/inf 最低达 floor。
- 末位永远 CPU 兜底(任何加速器不可用)。

### 2.3 功耗/热/电池策略(新,平台层)
新增 `platform::power` 探测:`{on_battery, power_profile(perf/balanced/saver), thermal_pressure}`(Win: powercfg/WMI;Linux: /sys/class/power_supply + thermal_zone + upower)。
- **AC + 热有余** → perf-optimal(最快加速器)。
- **电池 / saver** → energy-optimal(NPU 通常 energy/inf 最低 > iGPU > dGPU;tiny 任务可 CPU)。
- **热节流** → 降到低功耗加速器 / 或串行化降并发。
- **dGPU 唤醒成本**:dGPU 高吞吐但唤醒+功耗高;短/小任务偏 NPU/iGPU,长/大任务才上 dGPU。

### 2.4 争用感知(每加速器迷你队列)
- 每加速器一个轻量队列;同加速器多请求串行(避免 OOM/降速,per bianbu "串行+队列"教训,PC 版同理但跨加速器并行)。
- 能力 A 首选加速器忙 → 看次优 fallback(亲和表第 2)是否空闲 → 溢出过去(load-aware,本地版的 edge-cloud Model 1 思想下沉到加速器粒度)。

### 2.5 与 L_model / 既有的组合
- L_model(edge-cloud Model 1)在**上**:先定模型+本地/云;本地 → 交 L_hw 选加速器。
- L_hw 复用 accel.rs 的 EpChoice + catalog;**新增** = 多能力分配函数 + 功耗探测 + 亲和表(带 power 维) + 加速器队列。
- K3:L_hw 不在 attune(在 scheduler 的 A100/X100/IME2 仲裁);attune K3 形态只走 L_model→:8090。**所以 L_hw 仅 PC/Server 形态激活**(FormFactor 分叉,个人版 0 回退到现状=单 EP 仍可作 L_hw 的退化实现)。

---

## 3. bench-results 契约(等 vlm-llm-bench 落地的"验证结果")

**impl 前需 vlm-bench 产出的矩阵**(用户"依验证结果开发"的输入):
每 **(能力 × 模型 × 加速器 × 设备 × 电源态)**:
1. **延迟** p50/p95 + **吞吐**。
2. **质量** — OCR=CER · ASR=WER · embedding/rerank=F1/recall · LLM=任务指标。
3. **功耗(新维度,用户硬需)** — **平均功率 W + 单次能耗 J/inf**(多数 bench 只测延迟不测功耗;power-aware 路由必须有此列)。
4. **并发干扰** — 两能力跑在不同加速器**同时** → 是否互扰(共享内存带宽/总功耗预算/热)。

→ 这张矩阵直接填进 catalog 的 (能力×加速器) 亲和表 + 驱动 L_hw 选择函数。**bench 没测功耗这列前,功耗策略只能用保守默认(NPU<iGPU<dGPU 能效经验序),标 PENDING-bench。**

---

## 4. 切片(等 bench 后开发)
- **S1** platform::power 探测(on_battery/profile/thermal,Win+Linux)+ 测试(纯平台,可先于 bench)。
- **S2** catalog (能力×加速器) 亲和表 schema 扩展(加 power_w/energy_per_inf 列,bench 填值)。
- **S3** `assign_accelerators()` 多能力分配 + 争用队列(替/扩 recommend_ep_chain)。
- **S4** 功耗策略接入(perf/energy 模式按电源态切)。
- **S5** 真机验证(Intel Core Ultra / AMD Ryzen AI):OCR∥embedding 并行铺开 + 电池态切 energy 模式实测。
- 个人版 L_hw 激活;K3 不激活(走 scheduler 设备仲裁);0 回退 guard。

## 5. 待评审决策
1. ⏳ 功耗策略默认档:开箱 perf 还是 balanced?电池态是否强制 energy?
2. ⏳ 亲和表 SSOT:同 INT-3 catalog(bench 导出)还是独立 hw-affinity 表?(倾向并入 catalog,统一 bench-SSOT)
3. ⏳ dGPU 唤醒/功耗阈值:多大任务才上 dGPU(需 bench energy 数据定)。
4. ⏳ 并发上限:同时铺几个加速器(共享内存带宽/总功耗预算约束)。

## CHANGELOG
- 2026-06-22: 初版。两层调度职责切分(L_model 走哪个模型@K3-scheduler/edge-cloud · L_hw 走哪个加速器@本地 attune,功耗感知)+ L_hw 设计(多加速器并行分配/功耗热电池策略/争用队列/bench 驱动亲和表)+ bench-results 契约(加功耗 W/J 维度)。impl 等 bench 落地。
