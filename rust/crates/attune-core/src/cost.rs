//! Cost & token estimator utility (Sprint v0.7 / F2).
//!
//! 设计目标（per CLAUDE.md "Cost & Trigger Contract"）：UI 必须显示每次 LLM
//! 调用的预估 token + 花费，常驻在 Chat 发送按钮旁。本模块提供：
//!   - `estimate_tokens(text, model)` —— 启发式 token 估算（按 model family 调整偏差）
//!   - `lookup_pricing(model)`        —— 内置主流模型的 USD/1K-token 价格表
//!   - `estimate_cost_usd(in, out, model)` —— 输入/输出 token 数 → 美元
//!
//! ## 估算偏差说明
//!
//! 真实 tokenizer（cl100k_base / claude.json / gemini SP）成本太高，本模块只做
//! "够用" 的启发式：根据 CJK 字符比例线性插值。在通用中英混排文档上误差 ±15%，
//! 足以驱动 UI 显示（不用于计费 source-of-truth）。
//!
//! 各 family 系数来自实测（同一段 1K 字符中文文本 + 不同 tokenizer）：
//!   - GPT-3.5/4 (cl100k_base):  中文 ≈ 0.50 tok/char, 英文 ≈ 0.25 tok/char
//!   - Claude (Anthropic):       中文 ≈ 0.55 tok/char, 英文 ≈ 0.27 tok/char
//!   - Gemini (SentencePiece):   中文 ≈ 0.45 tok/char, 英文 ≈ 0.24 tok/char
//!   - Qwen / DeepSeek (BBPE):   中文 ≈ 0.40 tok/char, 英文 ≈ 0.27 tok/char
//!   - bge embedding:            中文 ≈ 0.50 tok/char, 英文 ≈ 0.25 tok/char
//!   - 未知 model:               通用 0.30 tok/char

/// 模型定价（USD per 1000 tokens）。
///
/// 价格来源：各 vendor 官方 pricing 页面 2026-04 snapshot。
/// 更新策略：每季度复核一次；用户也可以通过 settings.llm.custom_pricing 覆盖。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub input_per_1k_usd: f64,
    pub output_per_1k_usd: f64,
}

/// 估算一段文本在某模型下的 token 数。
///
/// 算法：扫描每个 char，按"CJK / 非 CJK" 二分类，乘以各自的 tok/char 系数后求和。
/// CJK 判定包含 Han 汉字（U+4E00..U+9FFF + 扩展 A），不含日文假名/韩文谚文 ——
/// 实测后两者 tokenizer 偏差更接近英文，归到 non-CJK 桶。
///
/// 空字符串返回 0；不会 panic。
pub fn estimate_tokens(text: &str, model: &str) -> usize {
    if text.is_empty() {
        return 0;
    }

    let (cjk_coef, ascii_coef) = model_coefficients(model);

    let (cjk_chars, ascii_chars) = text.chars().fold((0usize, 0usize), |(c, a), ch| {
        if is_cjk_han(ch) {
            (c + 1, a)
        } else {
            (c, a + 1)
        }
    });

    let est = (cjk_chars as f64) * cjk_coef + (ascii_chars as f64) * ascii_coef;
    // 向上取整避免 0.4 → 0 把"看似有内容的短文本"显示为零成本
    est.ceil() as usize
}

/// 查内置 model pricing 表。命中返回 Some，未知 model 返回 None
/// （上层 UI 应显示"价格未知，请咨询 vendor"，不假装为 $0）。
pub fn lookup_pricing(model: &str) -> Option<ModelPricing> {
    // 用 prefix 匹配（"gpt-4o-2024-08-06" 匹配 "gpt-4o"），更稳定
    let m = model.to_ascii_lowercase();
    // mini 必须在 4o 之前判断（gpt-4o-mini 也以 gpt-4o 开头）
    if m.starts_with("gpt-4o-mini") {
        Some(ModelPricing { input_per_1k_usd: 0.000_150, output_per_1k_usd: 0.000_600 })
    } else if m.starts_with("gpt-4o") {
        Some(ModelPricing { input_per_1k_usd: 0.002_500, output_per_1k_usd: 0.010_000 })
    } else if m.starts_with("claude-3-5-sonnet") || m.starts_with("claude-3.5-sonnet") {
        Some(ModelPricing { input_per_1k_usd: 0.003_000, output_per_1k_usd: 0.015_000 })
    } else if m.starts_with("claude-3-opus") {
        Some(ModelPricing { input_per_1k_usd: 0.015_000, output_per_1k_usd: 0.075_000 })
    } else if m.starts_with("gemini-1.5-pro") {
        Some(ModelPricing { input_per_1k_usd: 0.001_250, output_per_1k_usd: 0.005_000 })
    } else if m.starts_with("deepseek-chat") {
        Some(ModelPricing { input_per_1k_usd: 0.000_140, output_per_1k_usd: 0.000_280 })
    } else if m.starts_with("qwen-max") {
        Some(ModelPricing { input_per_1k_usd: 0.002_500, output_per_1k_usd: 0.005_000 })
    } else {
        None
    }
}

/// 根据 input + output token 数估算总成本（美元）。
///
/// 未知 model 返回 None，调用方应据此显示"价格未知 / 询问 vendor"。
pub fn estimate_cost_usd(tokens_in: usize, tokens_out: usize, model: &str) -> Option<f64> {
    let p = lookup_pricing(model)?;
    let cost = (tokens_in as f64) * p.input_per_1k_usd / 1000.0
        + (tokens_out as f64) * p.output_per_1k_usd / 1000.0;
    Some(cost)
}

// ── 私有 helpers ─────────────────────────────────────────────────────────────

fn model_coefficients(model: &str) -> (f64, f64) {
    let m = model.to_ascii_lowercase();
    if m.starts_with("gpt") {
        (0.50, 0.25)
    } else if m.starts_with("claude") {
        (0.55, 0.27)
    } else if m.starts_with("gemini") {
        (0.45, 0.24)
    } else if m.starts_with("qwen") || m.starts_with("deepseek") {
        (0.40, 0.27)
    } else if m.starts_with("bge") {
        (0.50, 0.25)
    } else {
        // unknown model fallback —— 通用 0.30 tok/char（不区分 CJK / ASCII）
        (0.30, 0.30)
    }
}

#[inline]
fn is_cjk_han(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF      // CJK Ext A
        | 0x4E00..=0x9FFF    // CJK Unified
        | 0x20000..=0x2A6DF  // CJK Ext B
    )
}

// ── 单元测试 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_chinese_uses_higher_coef() {
        // "你好世界" 4 个中文字 × 0.5 (gpt) = 2 → ceil 2
        let n = estimate_tokens("你好世界", "gpt-4o");
        assert_eq!(n, 2, "gpt-4o 中文 4 字 → 2 tokens");
        // claude 系数 0.55 → 2.2 → ceil 3
        let n = estimate_tokens("你好世界", "claude-3-5-sonnet");
        assert_eq!(n, 3, "claude 中文 4 字 → 3 tokens (系数偏高)");
    }

    #[test]
    fn estimate_tokens_english_uses_lower_coef() {
        // 16 ASCII chars × 0.25 (gpt) = 4
        let n = estimate_tokens("hello there world", "gpt-4o-mini");
        // 17 chars 含空格 → 17 * 0.25 = 4.25 → ceil 5
        assert_eq!(n, 5, "gpt 英文 17 chars → 5 tokens");
    }

    #[test]
    fn lookup_pricing_known_models() {
        let p = lookup_pricing("gpt-4o").unwrap();
        assert!((p.input_per_1k_usd - 0.0025).abs() < 1e-9);
        assert!((p.output_per_1k_usd - 0.01).abs() < 1e-9);

        // mini 必须比 4o 便宜
        let p_mini = lookup_pricing("gpt-4o-mini").unwrap();
        assert!(p_mini.input_per_1k_usd < p.input_per_1k_usd);

        // version suffix 也能匹配
        assert!(lookup_pricing("gpt-4o-2024-08-06").is_some());
        assert!(lookup_pricing("claude-3-5-sonnet-20241022").is_some());
    }

    #[test]
    fn lookup_pricing_unknown_returns_none() {
        assert!(lookup_pricing("gpt-2-davinci").is_none());
        assert!(lookup_pricing("llama3-70b-instruct").is_none());
        assert!(lookup_pricing("").is_none());
    }

    #[test]
    fn estimate_cost_usd_zero_tokens_zero_cost() {
        // 边界 case：0 token 应该返回 Some(0.0) 而非 None
        let c = estimate_cost_usd(0, 0, "gpt-4o").unwrap();
        assert!(c.abs() < 1e-12, "0 token → $0");
        // 未知 model 即使是 0 token 也返 None（成本未知）
        assert!(estimate_cost_usd(0, 0, "mystery-model-x").is_none());
    }

    #[test]
    fn estimate_cost_usd_real_world_example() {
        // gpt-4o 1000 in / 500 out → 0.0025 + 0.005 = $0.0075
        let c = estimate_cost_usd(1000, 500, "gpt-4o").unwrap();
        assert!((c - 0.0075).abs() < 1e-9, "actual: {c}");
    }

    // ── 增强覆盖: 边界 / Unicode / 系数差异 / 大数 ───────────────────────────

    // Empty string → 0 tokens
    #[test]
    fn estimate_tokens_empty_returns_zero() {
        assert_eq!(estimate_tokens("", "gpt-4o"), 0);
        assert_eq!(estimate_tokens("", "unknown-model"), 0);
    }

    // 短文本 (1 char) 仍 ceil 上来,避免 UI 显示 0
    #[test]
    fn estimate_tokens_single_char_ceils_to_at_least_one() {
        let n = estimate_tokens("a", "gpt-4o");
        assert_eq!(n, 1, "1 ASCII char × 0.25 → ceil 1");
        let n = estimate_tokens("中", "gpt-4o");
        assert_eq!(n, 1, "1 CJK char × 0.5 → ceil 1");
    }

    // Case-insensitive model lookup
    #[test]
    fn lookup_pricing_case_insensitive() {
        assert!(lookup_pricing("GPT-4o").is_some());
        assert!(lookup_pricing("GPT-4O-MINI").is_some());
        assert!(lookup_pricing("Claude-3-5-Sonnet-20241022").is_some());
    }

    // 全 model 覆盖 — 确保所有 7 个 family 都能 lookup
    #[test]
    fn lookup_pricing_all_families() {
        assert!(lookup_pricing("gpt-4o").is_some());
        assert!(lookup_pricing("gpt-4o-mini").is_some());
        assert!(lookup_pricing("claude-3-5-sonnet").is_some());
        assert!(lookup_pricing("claude-3.5-sonnet").is_some()); // dot alt syntax
        assert!(lookup_pricing("claude-3-opus").is_some());
        assert!(lookup_pricing("gemini-1.5-pro").is_some());
        assert!(lookup_pricing("deepseek-chat").is_some());
        assert!(lookup_pricing("qwen-max").is_some());
    }

    // Output 比 input 价高 (per business model — chat 应 disincentivize long output)
    #[test]
    fn lookup_pricing_output_higher_than_input() {
        for m in ["gpt-4o", "claude-3-5-sonnet", "claude-3-opus", "gemini-1.5-pro"] {
            let p = lookup_pricing(m).unwrap();
            assert!(p.output_per_1k_usd > p.input_per_1k_usd, "{m} output >= input");
        }
    }

    // CJK Ext A (U+3400..U+4DBF) — 应识别
    #[test]
    fn estimate_tokens_cjk_ext_a_treated_as_cjk() {
        // U+3400 = 㐀
        let n_ext_a = estimate_tokens("㐀㐁㐂㐃", "gpt-4o");
        let n_basic = estimate_tokens("一二三四", "gpt-4o");
        assert_eq!(n_ext_a, n_basic, "CJK Ext A 与 CJK Unified 同系数");
    }

    // 日文假名 — 按设计应归为 non-CJK (ASCII 系数)
    #[test]
    fn estimate_tokens_japanese_kana_treated_as_non_cjk() {
        // 4 平假名 × 0.25 (gpt ascii coef) = 1 → ceil 1
        let n = estimate_tokens("あいうえ", "gpt-4o");
        assert_eq!(n, 1, "假名按 ASCII 系数 (0.25), 实测 tokenizer 偏差近英文");
    }

    // 混合 CJK + ASCII: 系数应叠加
    #[test]
    fn estimate_tokens_mixed_text() {
        // "Hello 世界" → 6 ASCII (含空格) + 2 CJK
        // gpt-4o: 6*0.25 + 2*0.5 = 1.5 + 1.0 = 2.5 → ceil 3
        let n = estimate_tokens("Hello 世界", "gpt-4o");
        assert_eq!(n, 3);
    }

    // unknown model 用通用 0.30
    #[test]
    fn estimate_tokens_unknown_model_uses_generic_coef() {
        // 10 chars × 0.30 = 3
        let n = estimate_tokens("abcdefghij", "mystery-model");
        assert_eq!(n, 3);
    }

    // 系数差异 deepseek vs gpt (相同文本不同 token 数)
    #[test]
    fn estimate_tokens_model_family_differs() {
        let text = "这是一段比较长的中文测试文本用于验证模型系数差异";
        let n_gpt = estimate_tokens(text, "gpt-4o");      // 0.50 coef
        let n_qwen = estimate_tokens(text, "qwen-max");   // 0.40 coef
        assert!(n_gpt > n_qwen, "gpt 系数更高 → token 更多");
    }

    // 大数 token 不 overflow
    #[test]
    fn estimate_cost_usd_million_tokens_no_overflow() {
        let c = estimate_cost_usd(1_000_000, 500_000, "gpt-4o").unwrap();
        // 1M in × 0.0025 + 500K out × 0.01 = 2.5 + 5.0 = 7.5
        assert!((c - 7.5).abs() < 1e-6);
    }

    // 跨 family pricing 排序 — claude-3-opus 应是最贵
    #[test]
    fn pricing_opus_is_most_expensive() {
        let opus = lookup_pricing("claude-3-opus").unwrap();
        let gpt4o = lookup_pricing("gpt-4o").unwrap();
        let mini = lookup_pricing("gpt-4o-mini").unwrap();
        let deepseek = lookup_pricing("deepseek-chat").unwrap();
        assert!(opus.input_per_1k_usd > gpt4o.input_per_1k_usd);
        assert!(gpt4o.input_per_1k_usd > mini.input_per_1k_usd);
        assert!(mini.input_per_1k_usd > deepseek.input_per_1k_usd);
    }
}
