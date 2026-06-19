//! Batch processing — run one capability over many items, with progress + cost preflight.
//!
//! ## 定位
//!
//! 把单文档能力 (摘要 / 改写 / 提取) 批量套用到一组 item 上。**复用编排层**,
//! 不引入新 LLM agent —— 逐项调用既有 `skills::summarize_text` / 内联改写 prompt /
//! 确定性 `skills::extract_entities`。批量本身只负责:
//!   - 逐项执行 + 聚合 (per-item 成功/失败,单项失败不中断整批)
//!   - 进度反馈 (已完成 N / 总 M,经回调)
//!   - 成本预估 (批前算总 token + 预估花费,§成本契约 UI 显示)
//!
//! ## 成本契约 (CLAUDE.md "Cost & Trigger Contract")
//!
//! 批处理是 💰 **第三层 (LLM)**,**必须用户显式触发**,绝不后台偷跑。调用前先用
//! [`BatchPlan::estimate`] 算总成本让用户确认,再调 [`run_batch`]。**例外**:
//! `Extract` 能力是确定性正则 (🆓 零成本层),不调 LLM —— `estimate` 对它返回零成本。
//!
//! ## 为何不需 N=3 真 LLM gate
//!
//! 批处理不是新 agent,它的"输出质量"完全由底层既有能力 (`summarize` 等) 决定 ——
//! 那些能力各自有自己的可靠性纪律。批处理新增的逻辑是**编排正确性**:进度计数、
//! 聚合、部分失败 graceful —— 这些用 mock LLM 即可确定性测试。

use crate::cost;
use crate::error::{Result, VaultError};
use crate::llm::LlmProvider;
use crate::skills::{extract_entities, summarize_text};
use serde::{Deserialize, Serialize};

/// 一项批处理输入:item 标识 + 待处理文本。
///
/// `id` 由调用方提供 (通常是 item_id);批处理只透传,用于在结果里定位是哪一项。
#[derive(Debug, Clone)]
pub struct BatchItem {
    pub id: String,
    pub text: String,
}

/// 可批量套用的能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchCapability {
    /// 批量摘要 (LLM,💰)。
    Summarize,
    /// 批量改写 (LLM,💰) —— 用 `params.rewrite_instruction` 指定改写目标。
    Rewrite,
    /// 批量实体提取 (确定性正则,🆓 零成本,不调 LLM)。
    Extract,
}

impl BatchCapability {
    /// 该能力是否需要 LLM (决定成本层级)。
    ///
    /// `Extract` 走确定性正则,属 🆓 零成本层 —— 这是成本契约编译期/运行期守卫的依据。
    pub fn uses_llm(self) -> bool {
        match self {
            BatchCapability::Summarize | BatchCapability::Rewrite => true,
            BatchCapability::Extract => false,
        }
    }
}

/// 批处理参数。
#[derive(Debug, Clone, Default)]
pub struct BatchParams {
    /// Rewrite 能力的改写指令 (如 "改写为正式书面语")。其他能力忽略。
    pub rewrite_instruction: Option<String>,
}

/// 单项结果 (成功或失败,均记录 —— 部分失败 graceful)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchItemResult {
    pub id: String,
    /// 成功时为能力输出 (摘要 / 改写文本 / 提取的 JSON);失败时 None。
    pub output: Option<String>,
    /// 失败时为人类可读错误信息;成功时 None。
    pub error: Option<String>,
}

impl BatchItemResult {
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

/// 整批结果聚合。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchOutcome {
    pub capability: BatchCapability,
    pub results: Vec<BatchItemResult>,
    /// 成功项数。
    pub succeeded: usize,
    /// 失败项数。
    pub failed: usize,
}

impl BatchOutcome {
    /// 整批是否全部成功。
    pub fn all_ok(&self) -> bool {
        self.failed == 0 && !self.results.is_empty()
    }
}

/// 批前成本预估 (§成本契约:批量前算总 token + 预估花费,UI 显示让用户确认)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchEstimate {
    pub capability: BatchCapability,
    pub item_count: usize,
    /// 是否调 LLM (false = 🆓 零成本层,total_cost_usd 必为 0)。
    pub uses_llm: bool,
    /// 预估总输入 token (所有项之和)。
    pub total_input_tokens: usize,
    /// 预估总输出 token (摘要/改写按经验比例;Extract 为 0)。
    pub total_output_tokens: usize,
    /// 预估总花费 (USD);本地模型 / Extract / 未知定价 → None。
    pub total_cost_usd: Option<f64>,
}

/// 批处理计划:能力 + 参数 + 模型名 (用于成本估算)。
pub struct BatchPlan<'a> {
    pub capability: BatchCapability,
    pub params: BatchParams,
    /// 用于 token/价格估算的模型名 (与实际 LLM 一致)。
    pub model: &'a str,
}

impl<'a> BatchPlan<'a> {
    pub fn new(capability: BatchCapability, model: &'a str) -> Self {
        Self {
            capability,
            params: BatchParams::default(),
            model,
        }
    }

    pub fn with_params(mut self, params: BatchParams) -> Self {
        self.params = params;
        self
    }

    /// 批前成本预估。**确定性、零 LLM 调用** —— 纯启发式 token 计数 + 价格表查询。
    ///
    /// 成本契约守卫:`Extract` 能力 `uses_llm=false` 且 `total_cost_usd=Some(0.0)`,
    /// 永不上 LLM。空批返回零成本。
    pub fn estimate(&self, items: &[BatchItem]) -> BatchEstimate {
        let uses_llm = self.capability.uses_llm();

        if !uses_llm {
            // 零成本层:确定性正则,无 token / 无花费。
            return BatchEstimate {
                capability: self.capability,
                item_count: items.len(),
                uses_llm: false,
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_cost_usd: Some(0.0),
            };
        }

        let total_input_tokens: usize = items
            .iter()
            .map(|it| cost::estimate_tokens(&it.text, self.model))
            .sum();

        // 输出 token 经验估算:摘要约压缩到输入的 1/8 (100-200 字摘要);
        // 改写约等长 (1:1)。给 UI 一个保守上界即可。
        let total_output_tokens = match self.capability {
            BatchCapability::Summarize => total_input_tokens / 8,
            BatchCapability::Rewrite => total_input_tokens,
            BatchCapability::Extract => 0,
        };

        let total_cost_usd =
            cost::estimate_cost_usd(total_input_tokens, total_output_tokens, self.model);

        BatchEstimate {
            capability: self.capability,
            item_count: items.len(),
            uses_llm: true,
            total_input_tokens,
            total_output_tokens,
            total_cost_usd,
        }
    }
}

const REWRITE_SYSTEM_PREFIX: &str =
    "你是文本改写助手。请严格按用户给的改写要求改写输入文本,不要添加原文之外的事实信息。\
输出纯文本,不要 markdown,不要解释。改写要求:";

/// 执行批处理。逐项跑同一能力,单项失败不中断整批 (记录 per-item 状态)。
///
/// `progress` 回调在**每项完成后**调用,参数为 `(已完成, 总数)` —— 供 UI 显示
/// "已完成 N/总 M"。回调不影响执行流程 (即使项失败也回调,计数 += 1)。
///
/// ## 成本契约
///
/// 这是 💰 触发点 —— 调用方**必须**在 UI 上经用户显式确认 (展示 [`BatchPlan::estimate`]
/// 的总成本) 后才调本函数;本函数不做后台偷跑判断 (那是调用方的触发契约)。
///
/// ## 错误语义
///
/// 整个函数只在**输入非法** (空批 / Rewrite 缺指令) 时返回 `Err`;能力执行期的
/// 单项错误 (LLM 不可用、空文本等) 落到 `BatchItemResult.error`,不向上抛。
pub fn run_batch(
    llm: &dyn LlmProvider,
    items: &[BatchItem],
    capability: BatchCapability,
    params: &BatchParams,
    mut progress: impl FnMut(usize, usize),
) -> Result<BatchOutcome> {
    if items.is_empty() {
        return Err(VaultError::InvalidInput("empty batch (no items)".into()));
    }

    // Rewrite 必须有改写指令,否则无法定义"改写成什么" —— 输入非法,早失败。
    let rewrite_system = if capability == BatchCapability::Rewrite {
        let instr = params
            .rewrite_instruction
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                VaultError::InvalidInput(
                    "Rewrite batch requires non-empty rewrite_instruction".into(),
                )
            })?;
        Some(format!("{REWRITE_SYSTEM_PREFIX}{instr}"))
    } else {
        None
    };

    let total = items.len();
    let mut results = Vec::with_capacity(total);
    let mut succeeded = 0usize;
    let mut failed = 0usize;

    for (idx, item) in items.iter().enumerate() {
        let outcome = run_one(llm, item, capability, rewrite_system.as_deref());
        match outcome {
            Ok(output) => {
                succeeded += 1;
                results.push(BatchItemResult {
                    id: item.id.clone(),
                    output: Some(output),
                    error: None,
                });
            }
            Err(e) => {
                failed += 1;
                results.push(BatchItemResult {
                    id: item.id.clone(),
                    output: None,
                    error: Some(format!("{e:?}")),
                });
            }
        }
        // 每项后报进度 (含失败项),计数从 1 开始。
        progress(idx + 1, total);
    }

    Ok(BatchOutcome {
        capability,
        results,
        succeeded,
        failed,
    })
}

/// 跑单项 —— 分派到对应底层能力。
fn run_one(
    llm: &dyn LlmProvider,
    item: &BatchItem,
    capability: BatchCapability,
    rewrite_system: Option<&str>,
) -> Result<String> {
    match capability {
        BatchCapability::Summarize => summarize_text::summarize(llm, &item.text),
        BatchCapability::Rewrite => {
            if item.text.trim().is_empty() {
                return Err(VaultError::InvalidInput("empty text to rewrite".into()));
            }
            // rewrite_system 在 run_batch 入口已构造 (Rewrite 路径必非 None)。
            let system = rewrite_system
                .ok_or_else(|| VaultError::InvalidInput("missing rewrite system prompt".into()))?;
            llm.chat(system, &item.text).map(|(s, _u)| s)
        }
        BatchCapability::Extract => {
            // 确定性正则,🆓 零成本层 —— 不碰 llm。
            let entities = extract_entities::extract(&item.text);
            serde_json::to_string(&entities).map_err(VaultError::from)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::TokenUsage;

    /// Mock LLM:回声 user 文本前缀,可配置成对特定内容失败 (测部分失败)。
    struct MockLlm {
        /// 若 user 文本含此子串则返回 Err (模拟单项失败)。
        fail_on: Option<String>,
    }
    impl MockLlm {
        fn ok() -> Self {
            Self { fail_on: None }
        }
        fn fail_on(s: &str) -> Self {
            Self {
                fail_on: Some(s.to_string()),
            }
        }
    }
    impl LlmProvider for MockLlm {
        fn chat(&self, _system: &str, user: &str) -> Result<(String, TokenUsage)> {
            if let Some(f) = &self.fail_on {
                if user.contains(f.as_str()) {
                    return Err(VaultError::LlmUnavailable("mock forced failure".into()));
                }
            }
            Ok((
                format!("OUT:{}", user.chars().take(10).collect::<String>()),
                TokenUsage::empty("mock", "mock"),
            ))
        }
        fn is_available(&self) -> bool {
            true
        }
        fn model_name(&self) -> &str {
            "mock"
        }
    }

    fn items(n: usize) -> Vec<BatchItem> {
        (0..n)
            .map(|i| BatchItem {
                id: format!("item-{i}"),
                text: format!("文档内容编号 {i} 这是一段测试正文"),
            })
            .collect()
    }

    // ---- 成本契约守卫 ----

    #[test]
    fn extract_is_zero_cost_layer() {
        // 编译期/运行期守卫:Extract 永不上 LLM,成本必为 0。
        assert!(!BatchCapability::Extract.uses_llm());
        let plan = BatchPlan::new(BatchCapability::Extract, "deepseek-v4-flash");
        let est = plan.estimate(&items(5));
        assert!(!est.uses_llm);
        assert_eq!(est.total_input_tokens, 0);
        assert_eq!(est.total_output_tokens, 0);
        assert_eq!(est.total_cost_usd, Some(0.0));
    }

    #[test]
    fn summarize_and_rewrite_use_llm() {
        assert!(BatchCapability::Summarize.uses_llm());
        assert!(BatchCapability::Rewrite.uses_llm());
    }

    #[test]
    fn estimate_summarize_has_positive_tokens() {
        let plan = BatchPlan::new(BatchCapability::Summarize, "gpt-4o-mini");
        let est = plan.estimate(&items(3));
        assert!(est.uses_llm);
        assert!(est.total_input_tokens > 0);
        // 摘要输出约 1/8 输入。
        assert_eq!(est.total_output_tokens, est.total_input_tokens / 8);
    }

    // ---- happy path ----

    #[test]
    fn batch_summarize_all_succeed() {
        let llm = MockLlm::ok();
        let its = items(4);
        let mut last_progress = (0, 0);
        let out = run_batch(
            &llm,
            &its,
            BatchCapability::Summarize,
            &BatchParams::default(),
            |done, total| last_progress = (done, total),
        )
        .expect("batch ok");
        assert_eq!(out.succeeded, 4);
        assert_eq!(out.failed, 0);
        assert!(out.all_ok());
        assert_eq!(out.results.len(), 4);
        assert_eq!(last_progress, (4, 4));
        for (i, r) in out.results.iter().enumerate() {
            assert_eq!(r.id, format!("item-{i}"));
            assert!(r.is_ok());
            assert!(r.output.as_ref().unwrap().starts_with("OUT:"));
        }
    }

    #[test]
    fn batch_extract_deterministic_no_llm() {
        // 用一个一旦被调就 panic-ish 的 LLM 证明 Extract 不碰 llm。
        struct NeverLlm;
        impl LlmProvider for NeverLlm {
            fn chat(&self, _s: &str, _u: &str) -> Result<(String, TokenUsage)> {
                panic!("Extract must not call LLM");
            }
            fn is_available(&self) -> bool {
                true
            }
            fn model_name(&self) -> &str {
                "never"
            }
        }
        let its = vec![BatchItem {
            id: "a".into(),
            text: "联系电话 13800138000 金额 1000 元".into(),
        }];
        let out = run_batch(
            &NeverLlm,
            &its,
            BatchCapability::Extract,
            &BatchParams::default(),
            |_, _| {},
        )
        .expect("extract ok");
        assert_eq!(out.succeeded, 1);
        // 输出是合法 JSON。
        let v: serde_json::Value = serde_json::from_str(out.results[0].output.as_ref().unwrap())
            .expect("valid json");
        assert!(v.is_object());
    }

    #[test]
    fn batch_rewrite_with_instruction() {
        let llm = MockLlm::ok();
        let its = items(2);
        let params = BatchParams {
            rewrite_instruction: Some("改写为正式书面语".into()),
        };
        let out = run_batch(&llm, &its, BatchCapability::Rewrite, &params, |_, _| {})
            .expect("rewrite ok");
        assert_eq!(out.succeeded, 2);
    }

    // ---- 部分失败 graceful ----

    #[test]
    fn batch_partial_failure_does_not_abort() {
        // 让含 "编号 1" 的项失败,其余成功。
        let llm = MockLlm::fail_on("编号 1 ");
        let its = items(4);
        let mut progress_calls = Vec::new();
        let out = run_batch(
            &llm,
            &its,
            BatchCapability::Summarize,
            &BatchParams::default(),
            |done, total| progress_calls.push((done, total)),
        )
        .expect("batch ok even with failures");
        assert_eq!(out.succeeded, 3);
        assert_eq!(out.failed, 1);
        assert!(!out.all_ok());
        // 进度回调对所有 4 项都触发 (含失败项)。
        assert_eq!(progress_calls, vec![(1, 4), (2, 4), (3, 4), (4, 4)]);
        // 失败项 id = item-1,有 error 无 output。
        let failed: Vec<_> = out.results.iter().filter(|r| !r.is_ok()).collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].id, "item-1");
        assert!(failed[0].output.is_none());
        assert!(failed[0].error.is_some());
    }

    // ---- 异常 / 边界 case ----

    #[test]
    fn empty_batch_errors() {
        let llm = MockLlm::ok();
        let r = run_batch(
            &llm,
            &[],
            BatchCapability::Summarize,
            &BatchParams::default(),
            |_, _| {},
        );
        assert!(r.is_err());
    }

    #[test]
    fn rewrite_without_instruction_errors() {
        let llm = MockLlm::ok();
        let r = run_batch(
            &llm,
            &items(1),
            BatchCapability::Rewrite,
            &BatchParams::default(),
            |_, _| {},
        );
        assert!(r.is_err());
    }

    #[test]
    fn rewrite_blank_instruction_errors() {
        let llm = MockLlm::ok();
        let params = BatchParams {
            rewrite_instruction: Some("   \t  ".into()),
        };
        let r = run_batch(&llm, &items(1), BatchCapability::Rewrite, &params, |_, _| {});
        assert!(r.is_err());
    }

    #[test]
    fn rewrite_empty_item_text_records_per_item_error() {
        let llm = MockLlm::ok();
        let its = vec![
            BatchItem {
                id: "good".into(),
                text: "正常正文".into(),
            },
            BatchItem {
                id: "blank".into(),
                text: "   ".into(),
            },
        ];
        let params = BatchParams {
            rewrite_instruction: Some("正式化".into()),
        };
        let out = run_batch(&llm, &its, BatchCapability::Rewrite, &params, |_, _| {})
            .expect("batch ok");
        assert_eq!(out.succeeded, 1);
        assert_eq!(out.failed, 1);
        assert_eq!(out.results[1].id, "blank");
        assert!(!out.results[1].is_ok());
    }

    #[test]
    fn estimate_empty_batch_is_zero() {
        let plan = BatchPlan::new(BatchCapability::Summarize, "gpt-4o-mini");
        let est = plan.estimate(&[]);
        assert_eq!(est.item_count, 0);
        assert_eq!(est.total_input_tokens, 0);
    }

    #[test]
    fn large_batch_progress_monotonic() {
        let llm = MockLlm::ok();
        let its = items(200);
        let mut prev = 0usize;
        let out = run_batch(
            &llm,
            &its,
            BatchCapability::Summarize,
            &BatchParams::default(),
            |done, total| {
                assert_eq!(total, 200);
                assert_eq!(done, prev + 1, "progress must be monotonic +1");
                prev = done;
            },
        )
        .expect("ok");
        assert_eq!(out.succeeded, 200);
        assert_eq!(prev, 200);
    }

    #[test]
    fn all_failures_still_returns_outcome() {
        // 全部失败也不抛 Err,聚合 failed=total。
        let llm = MockLlm::fail_on("文档"); // 所有项都含 "文档"
        let its = items(3);
        let out = run_batch(
            &llm,
            &its,
            BatchCapability::Summarize,
            &BatchParams::default(),
            |_, _| {},
        )
        .expect("returns outcome even if all fail");
        assert_eq!(out.failed, 3);
        assert_eq!(out.succeeded, 0);
        assert!(!out.all_ok());
    }

    // ---- proptest (≥3) ----

    use proptest::prelude::*;

    proptest! {
        /// 任意非空批 + Summarize (mock 全成功): succeeded == 项数,results 顺序保持。
        #[test]
        fn prop_all_succeed_count_matches(n in 1usize..50) {
            let llm = MockLlm::ok();
            let its = items(n);
            let out = run_batch(&llm, &its, BatchCapability::Summarize, &BatchParams::default(), |_, _| {}).unwrap();
            prop_assert_eq!(out.succeeded, n);
            prop_assert_eq!(out.failed, 0);
            prop_assert_eq!(out.results.len(), n);
            for (i, r) in out.results.iter().enumerate() {
                prop_assert_eq!(&r.id, &format!("item-{i}"));
            }
        }

        /// succeeded + failed 恒等于项数 (无论失败模式)。
        #[test]
        fn prop_succeeded_plus_failed_invariant(n in 1usize..40, fail_seed in 0usize..40) {
            // 让 index == fail_seed % n 的项失败。
            let target = fail_seed % n;
            let llm = MockLlm::fail_on(&format!("编号 {target} "));
            let its = items(n);
            let out = run_batch(&llm, &its, BatchCapability::Summarize, &BatchParams::default(), |_, _| {}).unwrap();
            prop_assert_eq!(out.succeeded + out.failed, n);
        }

        /// 进度回调次数恒等于项数,最后一次 done==total。
        #[test]
        fn prop_progress_called_once_per_item(n in 1usize..60) {
            let llm = MockLlm::ok();
            let its = items(n);
            let mut count = 0usize;
            let mut last = (0,0);
            run_batch(&llm, &its, BatchCapability::Summarize, &BatchParams::default(), |d, t| { count += 1; last = (d, t); }).unwrap();
            prop_assert_eq!(count, n);
            prop_assert_eq!(last, (n, n));
        }

        /// 成本估算确定性:同输入两次 estimate 结果相等;Extract 恒零成本。
        #[test]
        fn prop_estimate_deterministic_and_extract_zero(n in 0usize..30) {
            let its = items(n);
            let plan_s = BatchPlan::new(BatchCapability::Summarize, "gpt-4o-mini");
            prop_assert_eq!(plan_s.estimate(&its), plan_s.estimate(&its));
            let plan_e = BatchPlan::new(BatchCapability::Extract, "gpt-4o-mini");
            let e = plan_e.estimate(&its);
            prop_assert_eq!(e.total_cost_usd, Some(0.0));
            prop_assert_eq!(e.total_input_tokens, 0);
        }
    }
}
