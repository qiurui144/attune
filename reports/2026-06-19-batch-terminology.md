# 弱腿波 E:批处理 + 引用/术语一致性层

**日期**:2026-06-20 (任务标 2026-06-19)
**分支**:`worktree-agent-a0dc31500c8111a9d` (基于 develop b8c99a5 / merge tip 3a6c7c5)
**任务**:#102 roadmap 余项 — 批处理 + 术语层

## 1. spec / 既有能力的实际情况(校准)

任务 prompt 引用的 `docs/superpowers/specs/2026-06-19-user-need-capability-roadmap.md`
**在本仓不存在**;引用的 `writing/`(draft/rewrite/outline/synthesis/cite/templates)与
`document_intelligence/` 模块**也不在 attune-core**(它们在 attune-pro,本仓是 OSS 边界)。

attune-core 实际可复用的能力地基:
- `skills/summarize_text.rs` — 单文档摘要(调 LLM 一次)
- `skills/extract_entities.rs` — 确定性正则实体提取(电话/金额等,🆓 零成本)
- `cost.rs` — token / USD 估算(`estimate_tokens` / `estimate_cost_usd`)
- `llm.rs::LlmProvider` — LLM 抽象 + mock

→ 据此把"批处理"实现为**复用既有能力的循环编排**;"引用/术语一致性"中,引用规范化
依赖的 `writing/cite` 不在 OSS,故本波**只交付术语层**(确定性规则层,符合"规则层零成本优先"),
引用规范化层留待 attune-pro 或后续 OSS cite 落地后接入(已在已知限制登记)。

## 2. 改 / 新增文件

| 文件 | 类型 | 说明 |
|------|------|------|
| `rust/crates/attune-core/src/batch.rs` | 新增 | 批处理编排:`run_batch` + `BatchPlan::estimate` + 成本契约 |
| `rust/crates/attune-core/src/terminology.rs` | 新增 | 术语变体收集 + 统一(确定性,零 LLM) |
| `rust/crates/attune-core/src/lib.rs` | 改(mod 注册) | `pub mod batch; pub mod terminology;` |

只新增 + mod 注册,**未改任何无关模块行为**。

## 3. 批处理:复用编排,非新 agent → 不需 N=3

**结论:批处理是复用编排,不引入新 LLM agent,故不需真 N=3 gate。**

- `BatchCapability::{Summarize, Rewrite, Extract}` 逐项分派到既有能力:
  - Summarize → `skills::summarize_text::summarize`(既有 LLM 能力,自带可靠性纪律)
  - Rewrite → `llm.chat` + 用户改写指令(薄封装,无新 schema/judge)
  - Extract → `skills::extract_entities::extract`(确定性,**不碰 LLM**)
- 批处理新增的逻辑仅是**编排正确性**:进度计数 / 聚合 / 部分失败 graceful ——
  这些用 mock LLM 即可确定性测试。输出质量由底层能力决定,不是批处理的新职责。
- 因此**未引入新 agent**,`nightly-real-llm.yml` 无需新增 gate。

(若未来批处理引入新 batch-专属 LLM agent(如"批量去重摘要"需跨项语义判断),
届时再按 §4.5 + N=3 floor F1≥0.85 补 gate。本波不涉及。)

## 4. 成本契约符合(§Cost & Trigger Contract)

- 批处理是 💰 第三层:`run_batch` 注释明确"调用方必须经 UI 用户显式确认成本后才调",
  本函数不后台偷跑判断(触发契约属调用方)。
- 批前成本预估 `BatchPlan::estimate`:确定性、零 LLM 调用,纯 `cost::estimate_tokens` +
  价格表;给 UI 显示总 token + 预估 USD 让用户确认。
- **零成本层守卫**:`Extract` 能力 `uses_llm()==false`,estimate 恒返回
  `total_cost_usd=Some(0.0)` + `total_input_tokens=0`;`batch_extract_deterministic_no_llm`
  用一个"被调即 panic 的 NeverLlm" provider 证明 Extract 路径不碰 LLM(运行期守卫)。
- 术语层 `terminology.rs`:**所有公开函数签名不含 `LlmProvider`** —— 编译期即保证
  零 LLM 调用,永远 🆓 零成本层(模块 doc 已注明此约束)。

## 5. 六类测试矩阵

### batch.rs(18 tests)
| 类别 | 用例 |
|------|------|
| 成本契约守卫 | extract_is_zero_cost_layer / summarize_and_rewrite_use_llm / estimate_summarize_has_positive_tokens |
| happy | batch_summarize_all_succeed / batch_extract_deterministic_no_llm / batch_rewrite_with_instruction |
| 部分失败 graceful | batch_partial_failure_does_not_abort(失败项不中断,进度照常计数) / all_failures_still_returns_outcome |
| 异常/错误(≥3) | empty_batch_errors / rewrite_without_instruction_errors / rewrite_blank_instruction_errors / rewrite_empty_item_text_records_per_item_error |
| 边界 | estimate_empty_batch_is_zero / large_batch_progress_monotonic(200 项进度单调) |
| proptest(4) | prop_all_succeed_count_matches / prop_succeeded_plus_failed_invariant / prop_progress_called_once_per_item / prop_estimate_deterministic_and_extract_zero |

### terminology.rs(23 tests)
| 类别 | 用例 |
|------|------|
| 边界(normalize,≥5) | case_fold / fullwidth_to_half / whitespace_collapse / trims_edge_punct_keeps_inner / empty_and_punct_only |
| golden/happy(≥10) | collect_detects_case_variant / ignores_consistent_terms / fullwidth_variant / whitespace_variant / multiple_clusters_sorted_by_frequency / preferred_tiebreak_is_deterministic / skips_empty_and_zero_count / punct_only_skipped / three_way_variant_counts / variants_list_is_sorted_stable + apply_replaces_non_preferred / apply_no_match_returns_zero |
| 异常/边界 | collect_empty_input / collect_all_consistent_returns_empty |
| proptest(4) | prop_normalize_idempotent / prop_case_insensitive / prop_cluster_count_invariant / prop_apply_deterministic |

**集成(≥1)**:批处理的"集成"即 run_batch 端到端跑既有 skill(summarize/extract)路径,
已由 batch_summarize_all_succeed / batch_extract_deterministic_no_llm 覆盖(跨 skills 模块调用)。
术语层是纯函数库,无 subprocess 集成面。

**回归 fixture(test-fix-verify 闭环)**:首跑真发现 2 个 bug,各加/保留为永久回归用例:
1. `normalize_trims_edge_punct_keeps_inner` —— 抓出 `trim_punct` 漏了弯引号
   `"…"`(U+201C/201D),导致 `"术语"` 没被去引号。修:加 U+2018/2019/201C/201D。
2. clippy `-D warnings` 抓出 `trim_punct` 中全角 ASCII 区标点 `（）` 在 `fullwidth_to_half`
   之后**不可达**(已先转半角)→ 删冗余分支 + 注释说明调用顺序契约;另修
   `sort_by`→`sort_by_key(Reverse)` + 删 identity map + 删 unused import。

## 6. 真 LLM 数字

**无** —— 本波不引入新 LLM agent(批处理=复用编排),按 prompt 约定不需真 N=3。

## 7. clippy / i18n / 跨平台

- `cargo clippy -p attune-core --lib --all-targets -- -D warnings`:**干净**(0 warning)。
- i18n:**本波未加 UI**(纯 core capability 模块);UI 路由 + 前端卡片是后续独立任务,
  届时按 §i18n 双守卫处理。故本波无 i18n 守卫需求。
- 跨平台:纯 Rust + serde + std,无平台特定调用 / 无文件路径硬编码 / 无 LLM 硬依赖。

## 8. 测试结果

- `cargo test -p attune-core --lib -- batch::tests terminology::tests`:**41 passed, 0 failed**。
- `cargo test -p attune-core --lib`(全量):**1597 passed, 0 failed, 1 ignored**(+41 新增,无回归)。
- `cargo build -p attune-core -p attune-server`:**通过**(server 无破坏)。

**踩坑记录(环境)**:共享 `CARGO_TARGET_DIR` 在"仅新增 mod 文件"时未失效 lib 指纹,
导致过滤测试跑到 0 个的旧 binary;`touch lib.rs` 强制 rebuild 后正常。验证测试通过性
必须确认 binary 是最新编译(`Compiling attune-core` 出现)而非缓存。

## 9. 已知限制

- **引用规范化层未交付**:依赖 `writing/cite`(4 引用风格),该模块在 attune-pro 不在 OSS
  attune-core;本波只交付术语一致性层。OSS cite 落地或迁入后,术语层可与之组合成
  完整"引用/术语一致性层"。
- **术语层是确定性形态归一**(大小写/全半角/空白/标点),**不做语义同义判断**(那需词表/LLM,
  属上层 💰,不污染本零成本模块)+ **不做繁简转换**(需词表,可能误伤)。
- **批处理无 UI / 无 HTTP route**:本波是 core capability 增量;route + 前端进度条/成本确认卡
  是后续任务(届时复用 `BatchPlan::estimate` 显示总成本)。
- `apply_unification` 用子串替换(按变体长度降序降低误伤),调用方应在"簇是独立术语"
  时使用 —— 已在 doc 注明。
