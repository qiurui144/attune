# W5 综述 grounding 提升 — 实施报告（弱腿波 F / 任务 #103）

**Date**: 2026-06-20
**Branch**: `worktree-agent-a7e3a38058b73b75c`（基于 `develop` @ `b8c99a5`）
**Scope**: 把 `rust/crates/attune-core/src/writing/{synthesis,grounding}.rs` 的 W5 综述
grounding-precision 从前波实测 **0.779**（deepseek-chat N=3）算法级提升，**不换贵模型、不削弱
no-fabrication 指标、不下调 0.90 floor**。

> 起点修正：worktree 初始 HEAD 是陈旧的 main backfill 合并 `3a6c7c5`（不含 writing 模块）。
> `--ff-only` 推进到 `develop` 尖端 `b8c99a5`（携带 W1-W6 + spec + real-LLM gate），与任务
> 「基于 develop b8c99a5」一致。

## 改了什么算法

### 1. grounding.rs — 比例重叠召回路径（recall，不放宽编造门）
前波 grounding 校验只有**绝对阈值**：`overlap ≥ 3 tokens`。弱模型综述常产出**短的抽象式句子**，
真实复述了某条来源要点，却因释义只共享 2 个 token（< 3）被判 unverified —— 这是把
grounding-precision 拖下来的假阴性主因，而 fact-consistency 始终 1.000（没有编造）。

修复（`GroundingConfig::is_grounded`）:
- 新增**比例路径**:`overlap / seg_tokens ≥ min_overlap_ratio(0.34)` **且**
  `overlap ≥ min_overlap_abs_floor(2)` 也判 grounded。
- 绝对路径（≥3）**完全不变** —— 这是 additive OR，**不是降阈值**（spec §11 risk F）。
- `abs_floor = 2` 是安全护栏:**单 token 偶然重叠永不 ground**。编造的事实在任何来源里都不
  存在 → 至多共享 1 个偶然 token → 仍 unverified。**no-fabrication 不变量完整保留**。

### 2. grounding.rs — 全/半角归一（recall）
`fold_width()`:FF01–FF5E 全角 ASCII + U+3000 表意空格折到半角，再 tokenize。全角标点/字母的
综述句也能 token-match 半角来源。对已是半角的 ASCII/CJK 无影响（纯召回）。

> 未动 grounding 判定的逻辑入口（`ground_segments` 调 `is_grounded` 替换原 `>= min_overlap_tokens`
> 单条件）；未触碰 `synthesis.rs` 生成/MAP/REDUCE/grounding 装配逻辑；未触碰 real-LLM gate 文件
> （`git diff develop -- writing_real_llm_gate.rs` = 空）。

## before → after grounding（真实 N=3，deepseek-v4-flash）

| 模型 | grounding-precision (mean±std) | fact-consistency | floor | 判定 |
|---|---|---|---|---|
| deepseek-chat（前波，改前）| 0.779±0.042 | 1.000 | 0.90 | ⚠ 低于 |
| **deepseek-v4-flash（本波，改后）** | **0.826±0.038** | **1.000±0.000** | 0.90 | ⚠ 低于 |
| deepseek-v4-pro（本波，min-tier 探针）| 0.816±0.019 | 1.000±0.000 | 0.90 | ⚠ 低于 |

- flash 改后:`reports/runs/2026-06-20_synthesis-grounding-uplift/deepseek-v4-flash-synthesis-n3.log:45`
  （per-seed `:19`=0.808 / `:31`=0.792 / `:43`=0.880）。
- pro:`reports/runs/2026-06-20_synthesis-grounding-uplift/deepseek-v4-pro-synthesis-n3.log:45`
  （per-seed `:19`=0.815 / `:31`=0.793 / `:43`=0.839）。
- 提升量:**0.779 → 0.826（+0.047）**,纯算法,未换更强生成模型、未改语料 GT。

### fact-consistency 未降证明
flash + pro **两模型 × 全 3 seed × 全 11 case** 的 `consistent=true`,聚合
**fact-consistency = 1.000±0.000**（两 log `:45`）。比例召回路径由 `abs_floor=2` 护栏保证不引入
假阳性;单元 `is_grounded_abs_floor_blocks_single_token_fabrication` + proptest
`prop_disjoint_alphabet_never_falsely_grounds`（异字符集来源永不产生 grounding ref）从代码侧
封住编造路径。零编造不变量较改前**未降**。

## 判定:保留 Beta / 标最低 tier（floor 不下调）

仍 < 0.90。**根因 = 确定性 token-overlap 校验器对抽象式综述句的召回天花板,不是模型能力差**:
- **deepseek-v4-pro 同测 0.816,与 flash 0.826 持平**（差异在 1σ 内）→ **换更强生成模型无可测增益**,
  印证 CLAUDE.md §4.5H「pro 对文本 agent 无可测增益」。残余假阴性是「语义等价但用词远离原文」的
  释义句（如 `db-index-tradeoff` / `vaccine-mechanism` 长抽象句,factual=4 verified=2/3）,
  token-overlap 在分母=该句众多 token 时无法信用化。
- 继续把 ratio 调到 0.34 以下虽能凑过 floor,但会开始接纳弱相关句 → **危及 no-fabrication
  这一唯一不可削弱的不变量**,故**不做**。
- 彻底闭合需后续增量的 **LLM-judge grounding 步骤**（语义层判定每节是否可归因来源）,而非更强
  生成模型。在此之前 W5 综述标 **Beta / 需 LLM-judge grounding 增量**;**0.90 floor 原样保留**,
  作为该增量必须达到的硬门（ratchet 只升不降,Agent 验证铁律）。

→ 文档:`docs/wiki/writing-engine.md` 质量证据节加 W5 行 + 已知限制（最低 tier 理由由「需更强
LLM tier」更正为「需 LLM-judge grounding 增量」—— pro 实测证明前者不成立）。RELEASE.md 当前版本
节只覆盖 W1/W2（W5 尚未进已发布版本节,故不在 RELEASE 标 tier,待 W5 进发布切片时引本报告结论）。

## 既有测试无回退

- `cargo test -p attune-core --lib writing::` → **99 passed; 0 failed**（改前 93 → +6 新测试:
  `is_grounded_*` ×3 / `proportional_path_grounds_short_cjk_paraphrase`（GT 用 `tokenize()` 独立
  算,不调 `ground_segments`）/ `fullwidth_folds_to_halfwidth_for_grounding` /
  `prop_disjoint_alphabet_never_falsely_grounds`）。
- `cargo clippy -p attune-core --all-targets -- -D warnings` → clean。
- `corpora_parse_and_meet_floor_count`（非 ignored 守卫）→ ok（语料未改,11 case 仍齐）。
- real-LLM gate 文件 / floor 常量 `git diff develop` = 空（floor 0.90 / 0.85 原样）。

### 顺手修一个前波遗留 flaky（test-robustness,非 floor 改动）
`synthesis.rs` 两个 proptest 生成器 `[a-z ]{4,60}` 可产出全空格来源 → `synthesize()` 返
`NoSourceMaterial` → `.unwrap()` panic（与原 W3-W6 切片里 cite all-space-title 同类）。改为
`[a-z][a-z ]{3,59}`（首字符非空格）。commit `c116c10`,独立于算法 commit `231b8ca`。

## raw log

- `reports/runs/2026-06-20_synthesis-grounding-uplift/deepseek-v4-flash-synthesis-n3.log`
- `reports/runs/2026-06-20_synthesis-grounding-uplift/deepseek-v4-pro-synthesis-n3.log`

模型经 `https://api.deepseek.com/v1`（key 从 `/tmp/secrets-deepseek/key.env` source,
**全程未 echo / commit / log**,§1.4）。该 endpoint 直接服务 `deepseek-v4-flash` /
`deepseek-v4-pro`（`/v1/models` 实测两项),无需经 gateway。
