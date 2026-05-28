//! 上下文预算管理器
//!
//! **修复的问题**：`search::INJECTION_BUDGET`（RAG 知识注入）与 chat 路由的
//! `MAX_HISTORY_DEPTH`（历史深度）此前都是写死常量，不感知 LLM 的真实上下文窗口，
//! 也从不把 `system + 知识 + 历史 + 消息` 四段加总对照窗口。后果：
//!   - 小模型（qwen2.5:3b ~32K）：四段上限之和溢出 → provider 截断 → 丢证据
//!   - 大模型（gemini-2.5 ~1M）：注入预算才 2000 字符 → 浪费 99% 窗口
//!
//! 本模块按 model 名查窗口，预留回答空间后，在剩余预算里给「知识注入」与「历史」
//! 各分配份额。token 估算复用 [`crate::context_compress::estimate_tokens`]（CJK 感知）。

use crate::context_compress::estimate_tokens;

/// 按模型名估算上下文窗口（token）。子串匹配；**未知模型保守取小窗口**避免溢出。
pub fn context_window(model: &str) -> usize {
    let m = model.to_lowercase();
    if m.contains("gemini") && (m.contains("2.5") || m.contains("2.0") || m.contains("1.5")) {
        1_000_000
    } else if m.contains("claude") {
        200_000
    } else if m.contains("gpt-4o") || m.contains("gpt-4.1") || m.contains("gpt-4-turbo")
        || m.contains("glm-4") || m.contains("moonshot") || m.contains("kimi") {
        128_000
    } else if m.contains("deepseek") {
        64_000
    } else if m.contains("gpt-3.5") {
        16_000
    } else if m.contains("qwen") || m.contains("gemini") || m.contains("llama") {
        // 本地常见小模型 / 未标注的 gemini 变体
        32_000
    } else {
        // 未知模型（含本地小模型）→ 最保守，宁可少注入也别溢出
        8_000
    }
}

/// 上下文预算分配结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetPlan {
    /// 模型上下文窗口（token）
    pub window: usize,
    /// 为 LLM 回答预留的 token
    pub response_reserve: usize,
    /// RAG 知识注入预算（token）
    pub knowledge_tokens: usize,
    /// 应保留的历史消息条数（从最新往回数）
    pub history_keep: usize,
    /// 因预算不足应丢弃的历史条数（最旧的若干条）
    pub history_dropped: usize,
    /// Spec §4.2 — pre-flight input token estimate for the UsageEvent path.
    ///
    /// Covers `system + user + retained history` (the parts `plan_context` can
    /// account for deterministically). Excludes the actual knowledge tokens
    /// that downstream RAG injection ends up using — the caller (chat.rs)
    /// adds the realized knowledge slice before emitting `UsageEvent`.
    pub tokens_in_used: usize,
}

impl BudgetPlan {
    /// 知识注入预算换算成字符数 —— `search::allocate_budget` 按字符切。
    /// `estimate_tokens` 对 CJK 取 1.2 token/字 → 字符 ≈ token × 5/6（保守）。
    pub fn knowledge_chars(&self) -> usize {
        self.knowledge_tokens * 5 / 6
    }
}

/// 规划上下文预算。
///
/// - `model`：目标 LLM 名（决定窗口）
/// - `system_text` / `user_text`：已定段（不可压缩，先扣掉）
/// - `history`：多轮历史，**从旧到新**排列，每项 `(role, content)`
///
/// 策略：`window − reserve − system − user = 可分配`；知识与历史平分；
/// 历史从最新往回累加直到塞满其份额，更旧的计入 `history_dropped`。
pub fn plan_context(
    model: &str,
    system_text: &str,
    user_text: &str,
    history: &[(String, String)],
) -> BudgetPlan {
    let window = context_window(model);
    // 预留回答空间：窗口的 1/4，封顶 4096，保底 512
    let response_reserve = (window / 4).clamp(512, 4096);
    let system_tok = estimate_tokens(system_text);
    let user_tok = estimate_tokens(user_text);

    let fixed = response_reserve + system_tok + user_tok;
    let available = window.saturating_sub(fixed);

    // 知识与历史平分可分配预算
    let knowledge_tokens = available / 2;
    let history_share = available - knowledge_tokens;

    // 历史从最新往回累加，塞进 history_share；+8/条 计结构开销
    let mut used = 0usize;
    let mut keep = 0usize;
    for (role, content) in history.iter().rev() {
        let t = estimate_tokens(role) + estimate_tokens(content) + 8;
        if used + t > history_share {
            break;
        }
        used += t;
        keep += 1;
    }
    let history_dropped = history.len().saturating_sub(keep);

    // Spec §4.2 — populate tokens_in_used = system + user + retained history.
    // Downstream call sites add realized knowledge tokens before emitting UsageEvent.
    let tokens_in_used = system_tok + user_tok + used;

    BudgetPlan {
        window,
        response_reserve,
        knowledge_tokens,
        history_keep: keep,
        history_dropped,
        tokens_in_used,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_window_known_models() {
        assert_eq!(context_window("gemini-2.5-flash"), 1_000_000);
        assert_eq!(context_window("claude-sonnet-4-6"), 200_000);
        assert_eq!(context_window("gpt-4o-mini"), 128_000);
        assert_eq!(context_window("deepseek-chat"), 64_000);
        assert_eq!(context_window("qwen2.5:3b"), 32_000);
        assert_eq!(context_window("glm-4-plus"), 128_000);
    }

    #[test]
    fn context_window_unknown_is_conservative() {
        // 未知模型 → 8K，宁可少注入不溢出
        assert_eq!(context_window("some-unknown-model"), 8_000);
        assert_eq!(context_window(""), 8_000);
    }

    #[test]
    fn plan_reserves_response_space() {
        let plan = plan_context("gpt-4o-mini", "sys", "hi", &[]);
        assert!(plan.response_reserve >= 512);
        assert!(plan.response_reserve <= 4096);
        // system + user + reserve + knowledge*2 大致 ≤ window
        assert!(plan.knowledge_tokens * 2 + plan.response_reserve <= plan.window);
    }

    /// Spec §4.2: plan_context must populate `tokens_in_used` so the UsageEvent at
    /// the call site can read the pre-flight token budget without re-counting the
    /// history. The value covers system + user + retained history; the knowledge
    /// budget is excluded because the actual injected knowledge size is decided
    /// downstream (search::allocate_budget can return less than the reserved
    /// `knowledge_tokens`), and the caller (chat.rs) will add the realized
    /// knowledge tokens before emitting the UsageEvent.
    #[test]
    fn plan_context_reports_tokens_used() {
        let plan = plan_context("gpt-4o-mini", "system text", "user question", &[]);
        assert!(
            plan.tokens_in_used > 0,
            "tokens_in_used should reflect system+user (+history when present)"
        );
        let cap = plan.window - plan.response_reserve;
        assert!(
            plan.tokens_in_used <= cap,
            "tokens_in_used ({}) must not exceed window - response_reserve ({})",
            plan.tokens_in_used,
            cap
        );
    }

    /// Adding history must increase `tokens_in_used` proportionally to the
    /// `history_keep` count (subject to truncation when over budget).
    #[test]
    fn plan_context_tokens_used_grows_with_kept_history() {
        let base = plan_context("gpt-4o-mini", "sys", "q", &[]);
        let history: Vec<(String, String)> = (0..5)
            .map(|i| ("user".into(), format!("turn {i} content")))
            .collect();
        let with_hist = plan_context("gpt-4o-mini", "sys", "q", &history);
        assert!(
            with_hist.tokens_in_used > base.tokens_in_used,
            "history kept ({}) should raise tokens_in_used (base={}, with={})",
            with_hist.history_keep,
            base.tokens_in_used,
            with_hist.tokens_in_used
        );
    }

    #[test]
    fn plan_scales_with_window() {
        // 同样输入，大窗口模型分到的知识预算应远大于小模型
        let big = plan_context("gemini-2.5-flash", "sys", "question", &[]);
        let small = plan_context("local-tiny-model", "sys", "question", &[]);
        assert!(
            big.knowledge_tokens > small.knowledge_tokens * 10,
            "big={} small={}",
            big.knowledge_tokens,
            small.knowledge_tokens
        );
    }

    #[test]
    fn plan_trims_history_on_small_window() {
        // 8K 窗口 + 大量长历史 → 必然丢弃最旧的若干条
        let long = "这是一段很长的对话内容".repeat(80); // ~880 字 → ~1000+ token
        let history: Vec<(String, String)> = (0..30)
            .map(|i| {
                let role = if i % 2 == 0 { "user" } else { "assistant" };
                (role.to_string(), long.clone())
            })
            .collect();
        let plan = plan_context("local-tiny-model", "sys", "q", &history);
        assert!(plan.history_dropped > 0, "小窗口长历史必须丢弃");
        assert_eq!(plan.history_keep + plan.history_dropped, history.len());
    }

    #[test]
    fn plan_keeps_all_history_when_it_fits() {
        // 大窗口 + 短历史 → 全部保留，零丢弃
        let history: Vec<(String, String)> = (0..6)
            .map(|i| ("user".to_string(), format!("短消息 {i}")))
            .collect();
        let plan = plan_context("gemini-2.5-flash", "sys", "q", &history);
        assert_eq!(plan.history_dropped, 0);
        assert_eq!(plan.history_keep, 6);
    }

    // ── 增强覆盖 ────────────────────────────────────────────────────────────

    // knowledge_chars: token × 5/6
    #[test]
    fn knowledge_chars_uses_5_6_ratio() {
        let plan = BudgetPlan {
            window: 128_000,
            response_reserve: 4_096,
            knowledge_tokens: 6000,
            history_keep: 0,
            history_dropped: 0,
            tokens_in_used: 0,
        };
        // 6000 × 5/6 = 5000
        assert_eq!(plan.knowledge_chars(), 5_000);
    }

    // 边界: knowledge_tokens=0 → knowledge_chars=0
    #[test]
    fn knowledge_chars_zero() {
        let plan = BudgetPlan {
            window: 8_000, response_reserve: 2_000, knowledge_tokens: 0,
            history_keep: 0, history_dropped: 0,
            tokens_in_used: 0,
        };
        assert_eq!(plan.knowledge_chars(), 0);
    }

    // 历史空 → keep=0, dropped=0
    #[test]
    fn plan_empty_history() {
        let plan = plan_context("gpt-4o", "sys", "u", &[]);
        assert_eq!(plan.history_keep, 0);
        assert_eq!(plan.history_dropped, 0);
    }

    // 超长 user/system 导致 fixed > window → available 用 saturating_sub 为 0
    #[test]
    fn plan_oversized_inputs_no_panic() {
        let huge = "x".repeat(200_000);
        let plan = plan_context("local-tiny-model", &huge, &huge, &[]);
        // 不 panic, knowledge=0 也合理
        assert!(plan.knowledge_tokens <= plan.window);
    }

    // moonshot / kimi → 128K
    #[test]
    fn context_window_moonshot_kimi() {
        assert_eq!(context_window("moonshot-v1-8k"), 128_000);
        assert_eq!(context_window("kimi-k2"), 128_000);
    }

    // claude variants 都 200K
    #[test]
    fn context_window_claude_variants() {
        assert_eq!(context_window("claude-3-opus"), 200_000);
        assert_eq!(context_window("claude-3-5-sonnet-20241022"), 200_000);
        assert_eq!(context_window("claude-3-haiku"), 200_000);
    }

    // gpt-3.5 → 16K
    #[test]
    fn context_window_gpt_3_5() {
        assert_eq!(context_window("gpt-3.5-turbo"), 16_000);
    }

    // llama 走 32K 分支
    #[test]
    fn context_window_llama() {
        assert_eq!(context_window("llama3.2:3b"), 32_000);
    }

    // model lookup 大小写不敏感 (lowercases)
    #[test]
    fn context_window_case_insensitive() {
        assert_eq!(context_window("CLAUDE-SONNET-4-6"), 200_000);
        assert_eq!(context_window("GPT-4O"), 128_000);
        assert_eq!(context_window("DeepSeek-Chat"), 64_000);
    }

    // 历史从"最新→最旧"保留: rev iter 保证最新 N 条留下
    #[test]
    fn plan_keeps_newest_history_when_trimming() {
        let history: Vec<(String, String)> = (0..50)
            .map(|i| ("user".to_string(), format!("msg{i} ").repeat(500)))
            .collect();
        let plan = plan_context("local-tiny-model", "sys", "q", &history);
        assert_eq!(plan.history_keep + plan.history_dropped, history.len());
        // 至少要 drop 一部分 (8K window vs 50 条长消息)
        assert!(plan.history_dropped > 0);
    }

    // response_reserve clamp: 大窗口不超 4096
    #[test]
    fn plan_response_reserve_clamped_to_4096() {
        let plan = plan_context("gemini-2.5-flash", "", "q", &[]);
        // 1M / 4 = 250K, clamp 到 4096
        assert_eq!(plan.response_reserve, 4_096);
    }

    // response_reserve clamp: 小窗口不低于 512
    #[test]
    fn plan_response_reserve_clamped_to_512() {
        // 8K / 4 = 2000, 介于 512 和 4096 之间, 应保留 2000
        let plan = plan_context("unknown-model", "", "q", &[]);
        assert!(plan.response_reserve >= 512);
        assert!(plan.response_reserve <= 4_096);
    }
}
