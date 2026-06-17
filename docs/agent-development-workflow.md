# Agent / Plugin 开发工作流与分工（SSOT）

> 本文档是 attune + attune-pro **插件 / agent 开发**的端到端流水线 + 角色分工 SSOT。
> 配套文档：插件**形态 / 打包 / 签名 / 装载** 见 [`plugin-development.md`](plugin-development.md)（本文不重复）；
> 测试矩阵 SSOT 见 [`TESTING.md`](TESTING.md)；agent 清单见 [`wiki/agents.md`](wiki/agents.md)。
> 规则来源：项目 `CLAUDE.md`（Agent 验证铁律 / 成本契约）+ 全局 `~/.claude/CLAUDE.md`
> §3.1（spec-first）/ §4.5（LLM 兜底）/ §5.2（review）/ §6.1（测试矩阵）/ §7.2（RC 四门）。
> 本文档只描述**流程与分工**，不重述代码实现细节。

## 0. 目录（TOC）

- [1. 总览：七阶段流水线](#1-总览七阶段流水线)
- [2. 各阶段入口 / 出口判据](#2-各阶段入口--出口判据)
- [3. test-fix-verify 闭环（核心）](#3-test-fix-verify-闭环核心)
- [4. LLM agent 兜底契约（§4.5）](#4-llm-agent-兜底契约45)
- [5. 视觉流水线（VLM）](#5-视觉流水线vlm)
- [6. 输出模式契约](#6-输出模式契约)
- [7. 角色矩阵与分工](#7-角色矩阵与分工)
- [8. 双岗位双验收 + 一票否决（§5.2.0b）](#8-双岗位双验收--一票否决5200b)
- [9. OSS vs Pro 同纪律](#9-oss-vs-pro-同纪律)
- [10. 反模式速查](#10-反模式速查)

---

## 1. 总览：七阶段流水线

任何 attune / attune-pro 的**新 agent / 新 plugin / 改 prompt / 改 LLM call** 都必须走完整流水线。
小 bug fix / typo 走轻量路径（§5.1 变更决策树），但**任何进主分支的 agent 代码无例外走全链**。

```
 ① spec (11 节)        ② plan (TDD)          ③ impl
  spec-analyst   ──G1──▶  architect    ──G2──▶  implementer
  （起草）        Challenger（写 plan）  Challenger（按 plan TDD）
                                                  │
                                                  ▼
 ④ test-fix-verify 闭环 ──────────────────────────┤  (Agent 验证铁律)
   implementer + tester （六类矩阵 + real-LLM N=3）  │
                                                  ▼
 ⑤ 双岗位 review        ⑥ G3 / G4              ⑦ ship
  spec-reviewer ∥        tester(G3) +        releaser
  code-reviewer ∥        challenger(G4)      （RC 四门 + 本机部署）
  adversarial-reviewer
   （独立交叉，一票否决）
```

阶段编号与 SDLC gate 对应：spec→**G1**→plan→**G2**→impl→review→**G3**→test→**G4**→RC→GA。
每个 gate 走 Challenger Panel 或双岗位验收（§5.2.0b 一票否决）。状态落
`docs/superpowers/handoffs/<sprint>_state.yaml`（SSOT）。

---

## 2. 各阶段入口 / 出口判据

| 阶段 | 入口判据（开始前必备） | 出口判据（通过才进下阶段） | 产物 |
|------|----------------------|--------------------------|------|
| ① spec | 用户痛点明确；与产品定位对齐 | 11 节齐全（无 TBD）；G1 Challenger ≥ 4/5 五项 | `docs/superpowers/specs/<date>-<feature>.md` |
| ② plan | spec 已评审通过 | 文件清单 + commit 分批 + 每任务 acceptance_judges + 风险登记；G2 通过 | `docs/superpowers/plans/<date>-<feature>.md`（ship 后删） |
| ③ impl | plan 已评审；worktree 隔离（大 feature） | 严格按 plan；偏离回头改 plan（不 silent drift）；每任务 1 commit + TDD 步式 | 源码 + 测试（同 commit） |
| ④ test-fix-verify | impl 代码可编译 | 六类下限全到 + real-LLM F1 ≥ 0.85 + golden gate 1.00（确定性） | golden set + reproducer fixtures |
| ⑤ review | 闭环已过 | 双岗位（spec / quality / adversarial）独立验收，Critical/Important 全修齐复验 | review 结论落盘 |
| ⑥ G3/G4 | review 全绿 | G3 功能预期对齐（真跑过，非"应该可以"）；G4 缺口登记齐 | 证据归档 `docs/screenshots/<release>-verification/` |
| ⑦ ship | G3/G4 通过 | RC 四门全过（§7.2）+ 本机部署验证（§7.3） | tag + RELEASE.md 版本节 |

**硬约束**：
- spec / plan / impl 三层任一变更，上层必须同步（三者不漂移，§3.1）。
- 涉及 agent 的 PR commit msg 必须含 `test-fix-verify` + `agent_golden_gate.rs` 在 PR 上
  1.00 pass rate（确定性）或 F1 ≥ 0.85（LLM）。

---

## 3. test-fix-verify 闭环（核心）

> Agent 是 attune 生态最重要的附加功能与核心功能来源之一。任何 agent ship 前必须完成
> **闭环验证（test → fix → verify）**，不可仅满足"有 tests"或"clippy 干净"。
> 铁律全文：项目 `CLAUDE.md`「Agent 验证铁律」+ attune-pro
> `plugins/law-pro/docs/agent-skill-training-methodology.md`。

### 3.1 闭环 = 4 步全过，缺一即不合格

1. **覆盖测试**：每 agent ≥ 10 真实 golden case（YAML in `tests/golden/<agent>/N.yaml`）+ 1 sentinel。
2. **真实测试发现 bug**：首跑全过 = 八成 ground truth 由 agent 自己生成 / 测的不是 invariant
   → 必须深挖（"测试不够难，继续加 case"）。
3. **修复迭代**：bug → 写 reproducer fixture（**GT 独立计算，绝不调 `agent.calculate()`**）
   → 修 agent → 跑 fixture 过 → reproducer + fix 同 commit 入库。
4. **验证锁定**：fixture 进 golden set；阈值 **ratchet 只升不降**；`agent_golden_gate.rs` 是
   日常 CI 硬门。

### 3.2 六类测试覆盖下限（强制，§6.1）

| 类型 | 下限 | 工具 / 落点 |
|------|------|------------|
| Golden case | ≥ 10 真实 + 1 sentinel | YAML fixture `tests/golden/<agent>/` |
| 属性测试 | ≥ 3 per agent | `proptest` |
| 边界 case | ≥ 5 `#[test]` | inline `#[cfg(test)]`（空 / 超长 / 零 / 负 / Unicode） |
| 异常 / 错误 | ≥ 3 case | YAML `expected_error`（非法输入 / 服务挂 / 鉴权失败） |
| 集成 E2E | ≥ 1 subprocess | `tests/<agent>_subprocess.rs` |
| 回归 fixture | 每修一个 bug 加 1 | golden set 永久保留 |

未到下限 → 禁止 PR merge。OSS attune-core 已落地此 harness（`*_golden_gate.rs` +
`tests/golden/<agent>/*.yaml`），新增 agent 复用同一套。

### 3.3 GT 纪律

- GT 必须**独立于 `agent.calculate()`**（reproducer 自己算 / 查表 / 第三方核对）。
  否则 agent 自检自己，gate 形同虚设。
- **domain GT 不确定 → 标 `PENDING-EXPERT`**，不臆造法律 / 医学公式去猜（law-pro 经验：
  口径不确定提示律师填写，不编码复杂公式硬猜）。expert 签字后才转正式 GT。
- 阈值下调绕过失败 = 反模式（ratchet rule，只升不降）。

---

## 4. LLM agent 兜底契约（§4.5）

任何 LLM-driven agent（extractor / classifier / judge）ship 前必过 A–G：

| 项 | 要求 |
|----|------|
| **A schema-guided** | Ollama `format: <schema>` / OpenAI `response_format: json_schema`；禁止自由文本 + 自己 regex parse |
| **B retry-validate** | LLM call → validator（JSON valid / 字段全 / grounding）→ fail 把 error 反馈重 call，**最多 3 次**，退避避免 burst |
| **C few-shot** | ≥ 2 worked examples 进 system/user（含 1 个 edge case：空字段 / 转义 / 多值） |
| **D 多模型矩阵** | 至少在 3 tier 真跑 10+ case：弱本地（qwen2.5:3b）/ 弱云（gemini-flash / gpt-4o-mini）/ 强云（Sonnet 作上限对照）；F1 三 tier 差 ≤ 0.15 算兼容 |
| **E 弱模型 fallback** | 全 tier 通 = all-model OK；仅中+强云通 → RELEASE.md 标 `Requires ≥ gpt-4o-mini`，本地 3B 自动 disable；仅强云通 → `Pro plan only` |
| **F 失败 telemetry** | record `agent_id / model / error_kind / retry_count`；(agent×model) 失败率 > 30% → UI 提示切高 tier；不 panic，走 graceful `Result::Err` |
| **G multi-turn + cache** | sequential agent（N ≥ 2 call）必 multi-turn（传真 prior turns）+ prompt cache + 监控 cache hit rate ≥ 50% on step ≥ 3 |

### real-LLM 验收门

- **N = 3 multi-seed**，报 mean ± std；**F1 ≥ 0.85** 才算通过 Phase 3。
- 多 seed 排名翻转 → 撤回 SOTA claim；改进 < 2σ 不算改进。
- **mock 全过 ≠ ready**：必须 real-LLM 端到端跑过真模型 + grounding 校验
  （attune v1.0 `defamation_extractor` mock 全过但 qwen2.5:3b real F1 = 0.09 的踩坑根因
  即缺 A + B + C 三项）。
- **生产模型选型默认**：文本 / 大模型任务 `deepseek-v4`；多模态 `qwen-3.6 / qwen-3.7`
  （qwen-vl 已下架禁用）。不掺 claude / gpt token 做生产后端（env 可切，默认值锁这两者）。

### grounding + injection guard

- extractor 输出每个字段必须带**原文依据锚点**（offset / 引文），validator 校验 grounding
  （字段值在源文本中可定位），无依据视为 fail 触发 retry。
- prompt injection guard：用户内容与指令分隔；validator 拒绝越权 / 偏离 schema 的输出。

---

## 5. 视觉流水线（VLM）

视觉理解（OCR 之上的语义 / 版面 / 图表理解）走独立 VLM 流水线，作为 agent 的上游能力：

```
图片 / 扫描件 / 视频帧
   │
   ▼  VLM（默认 qwen-3.6 / qwen-3.7，多模态）
   │  schema-guided 输出 + retry-validate(≤3) + grounding（区域 / bbox 锚点）
   ▼
 稳定结构化结果（JSON）
   │
   ├──▶ OSS 消费：doc-intel 插件（文档智能）/ Office helper（OCR + ASR 管线）
   └──▶ pro 消费：law-pro 等 vertical 的事实抽取 / 卷宗结构化
```

- VLM 走 DashScope 多模态接口；同样遵 §4.5 A–G（schema / retry / few-shot / grounding /
  多模型矩阵 / telemetry）。
- VLM 输出的不确定区域标 `PENDING-EXPERT` 或低置信，不强行下结论。
- 文档智能维度矩阵（A–K 全维：ZIP/XML 格式对抗面 = office 解压炸弹 / 路径穿越 / XML 实体
  P0 安全；tokenizer 变更 reindex 迁移）是 doc-intel 的 release 硬门，进 `TESTING.md` 主大纲，
  每轮 RC/GA 验收硬性要求。
- 若后续落地 `docs/wiki/vision-understanding-pipeline.md`，本节作为其上层入口指针。

---

## 6. 输出模式契约

> **结果非终点，精准适配输出才是。** 输出模式是 agent 的一等公民契约 —— spec 第 5 节
> （API 契约）必须明确声明本 agent 用哪种输出模式，下游据此消费。

| 模式 | 形态 | 适用场景 | 下游消费 |
|------|------|---------|---------|
| **结构化结果** | JSON（字段 + 值 + 置信） | 计算 / 抽取 / 分类（确定性 agent / extractor） | 直接入库 / 程序化处理 |
| **标记报告（带 offset）** | 原文 + 区间锚点标注（offset / span） | 风险点 / 引用命中 / 批注定位 | Reader 高亮 / 跳转回原文 |
| **批阅模式** | 逐段 inline 批注 + 修改建议 | 文书审阅 / 合同批改 / 文档校对 | 编辑器 inline 显示 |
| **叙述** | 自然语言总结 | chat 问答 / 深度分析 | 对话面板展示 |

- doc-intel 插件的输出层必须套用本契约（按场景选模式，不是一律返回纯文本）。
- 标记报告 / 批阅模式的 offset 必须与源文本对齐（reindex / tokenizer 变更时需迁移校验）。

---

## 7. 角色矩阵与分工

> 谁产出什么、谁验收什么。每个角色对应一个 SDLC agent（`sdlc-orchestrator:*`）或
> review agent。**产出者与验收者必须是不同 agent**（§5.2.0b）。

| 角色 | 阶段 | 产出 | 验收对象 | 模型 tier |
|------|------|------|---------|----------|
| **spec-analyst** | ① | 11 节 spec（拒绝在 intent 模糊时起草，拒绝留 TBD） | — | opus |
| **architect** | G1 + ② | (a) G1 Challenger 独立打分 spec；(b) 写 TDD plan（invoke `writing-plans`，不 freehand） | spec（G1 验收） | opus |
| **implementer** | ③ + ④ | 按 plan TDD 执行（write failing test → impl → commit）；test-fix-verify 闭环；偏离上报 architect | — | sonnet |
| **spec-reviewer** | ⑤ | 验"实现是否符合 spec 描述行为"（规约符合度 / 静态正确性） | impl（证明它对） | sonnet |
| **code-quality-reviewer** | ⑤ | §5.2 两轮清单（正确性 / 边界 / 错误处理 / 安全 / 测试覆盖 / 约定 / 性能） | impl | sonnet |
| **adversarial-reviewer** | ⑤ | 对抗破坏视角（证明它会挂；运行时 / 边界 / 安全）；安全 / 迁移 / 密钥改动**第二岗必含** | impl（证明它会挂） | opus |
| **tester** | ④ + G3 | 六类矩阵真跑 + real-LLM N=3；每 PASS 引 `reports/runs/<ts>/<file>:<line>` | 功能预期对齐（G3） | sonnet |
| **releaser** | ⑦ | RC 四门 + 本机部署验证 + RELEASE.md 4 节 + tag | 整体可装可用（GA 门） | opus |

**分工铁律**：
- spec-analyst 不自己验收 spec（architect 做 G1）；implementer 不自己宣布闭环过
  （tester 独立跑，PASS 必有 disk 上的 log，不信 agent 自报）。
- 两个 review 岗位**检查维度交叉 + 判定逻辑完全不同**（一个证明它对 / 一个证明它会挂），
  不允许同一逻辑跑两遍冒充双验收。

---

## 8. 双岗位双验收 + 一票否决（§5.2.0b）

**触发**：版本验收 / push 前最严审查 + 项目全局审核。

- **双岗位 + 双验收**：同一产物至少 2 个独立 agent 验收，**必须不同 agent + 检查维度交叉 +
  判定逻辑完全不同**。双岗不得共享同一 prompt / reviewer 上下文（防 monoculture 盲点）。
- **一票否决**：任一岗位判不合格 → **整体打回**（返工重审），不取平均、不"多数通过"放行。
  打回须附具体 finding + 复验闭环。
- **安全 / 认证 / 迁移 / 密钥改动**：第二岗必须是对抗式安全 review。
- **全局审核自动多组**：项目级 review（intake / 全仓 audit）自动派多组 agent 并行审
  （按维度 / 组件分组，组间逻辑互异），任一组 BLOCK → 该范围打回。

**最严审查硬门**（任何 git push / tag push 之前 + 任何版本验收之前）：
全测试套件 PASS + 静态检查零警告（clippy / lint）+ §5.2 两轮 review + 对抗式 review。
Critical / Important 全修齐复验后才能 push；有未决 → 不推。证据落盘，不靠"我看过了"。

---

## 9. OSS vs Pro 同纪律

- OSS attune 与 attune-pro 的 agent **共用同一套 reliability framework**，
  **不存在"free 因为是 OSS 所以纪律松"的合理化空间**。
- `agent_golden_gate.rs` 等价 harness 在 OSS attune-core 已落地（confirmed：
  `tests/*_golden_gate.rs` + `tests/golden/<agent>/*.yaml`）；attune-pro vertical agent
  复用同一 harness 形态。
- free / pro 同走七阶段流水线 + 六类下限 + LLM 兜底 A–G + real-LLM F1 ≥ 0.85。
- 成本契约对两者同样硬：LLM 调用（第三层"时间/金钱"）**必须用户显式触发，永不后台偷跑**；
  UI 显示预估 token / 费用；摘要按 chunk_hash 缓存避免重复消费。

---

## 10. 反模式速查

- ❌ 先写 agent 代码后补 spec（= rationalization，不是设计）。
- ❌ "我加了 agent 还没写测试" / "测试都过了没发现 bug"（测不够难，继续加 case）。
- ❌ ground truth 用 LLM / `agent.calculate()` 生成（自检自己，gate 形同虚设）。
- ❌ 阈值下调绕过失败 / "暂跳过这个失败 case"（ratchet 只升不降；要么修 agent 要么 expert 签字改 GT）。
- ❌ "我在 Claude 上测过 OK，弱模型应该也行"（没数据 = 没测；必跑 3 tier 矩阵）。
- ❌ mock 全过就当 LLM 路径 ready（mock ≠ 真 LLM 端到端 + grounding）。
- ❌ 同一 agent 自审两轮冒充双验收 / 两岗位用同一清单（逻辑没差异）。
- ❌ 不合格取平均放行 / 安全改动跳过对抗式安全 review。
- ❌ agent 一律返回纯文本（忽略输出模式契约，下游无法精准消费）。
- ❌ "这是 free agent 不用走 framework"（free 也要走）。
- ❌ 产品内嵌 agent 默认调 claude / gpt token（应 deepseek-v4 文本 + qwen-3.6/3.7 多模态）。
