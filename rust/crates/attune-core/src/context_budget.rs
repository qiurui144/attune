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

    BudgetPlan {
        window,
        response_reserve,
        knowledge_tokens,
        history_keep: keep,
        history_dropped,
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
}
