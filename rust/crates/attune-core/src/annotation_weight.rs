// 批注加权 —— Batch B.2
//
// ## 设计哲学
//
// 批注是用户对文档的"思考痕迹"—— 应直接影响 RAG 排名。用户手动/AI 添加 ⭐ 重点 的
// chunk 下次查询更优先出现；标了 🗑 过时 的 chunk 直接从候选里剔除。
//
// ## 成本/触发契约
//
// 这层是 🆓 零成本：只读已持久化的批注表 + 算数。不调 LLM、不建索引、不走网络。
// 调用时机同 RAG：chat 路径 / search_relevant 路径。**建库管道不用。**
//
// ## 权重规则（取 MAX，不累乘）
//
// | Label     | 作用               | Multiplier |
// |-----------|--------------------|-----------:|
// | 🗑 过时   | 剔除（从候选移除）  | None (=drop) |
// | ⭐ 重点   | 强 boost            | × 1.5      |
// | 📍 待深入 | 中 boost            | × 1.2      |
// | 🤔 存疑   | 中 boost            | × 1.2      |
// | ❓ 不懂   | 中 boost            | × 1.2      |
// | 其他     | 不影响              | × 1.0      |
//
// 同一 item 多个批注时取 MAX（不累乘），避免"5 个 ⭐ 批注叠出 ×7.6 分"的病态情况。
// 过时是 **强制剔除**：任一批注带 过时 → 整个 item 丢弃，即便另一批注是 ⭐。
// （用户既然标了过时，说明该文档已失效，重点只是历史参考。）

use crate::store::Annotation;

/// Item 级评分调整指令
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScoreAdjust {
    /// 从结果里完全剔除（🗑 过时）
    Drop,
    /// 乘以系数（≥ 1.0，默认 1.0 无影响）
    Multiply(f32),
}

impl ScoreAdjust {
    /// 把调整应用到原始 score。Drop 返回 0.0，调用方自行剔除。
    pub fn apply(self, score: f32) -> f32 {
        match self {
            Self::Drop => 0.0,
            Self::Multiply(m) => score * m,
        }
    }
}

/// 识别为 Drop 的 label 白名单（精确匹配，trim 后对比）。
/// **不用 substring**：用户自由输入的 "非过时" / "过时信息的反例" 会触发
/// 反向 drop，属于静默丢数据 bug。见 annotation_weight Round 1+2 review。
const DROP_LABELS: &[&str] = &[
    "过时",
    "🗑过时",
    "🗑 过时", // user 批注（popup 预设）
    "🕰过时",
    "🕰 过时", // AI Outdated angle
];

/// 强 boost（×1.5）：用户的"重点" / AI 的"要点" 和"风险"。
const STRONG_BOOST_LABELS: &[&str] = &[
    "重点",
    "⭐重点",
    "⭐ 重点", // user
    "要点",
    "⭐要点",
    "⭐ 要点", // AI Highlights angle
    "风险",
    "⚠️风险",
    "⚠️ 风险", // AI Risk angle
];

/// 中 boost（×1.2）：用户的思考类标签 + AI 的疑点。
const MEDIUM_BOOST_LABELS: &[&str] = &[
    "待深入",
    "📍待深入",
    "📍 待深入", // user
    "存疑",
    "🤔存疑",
    "🤔 存疑", // user
    "不懂",
    "❓不懂",
    "❓ 不懂", // user
    "疑点",
    "🤔疑点",
    "🤔 疑点", // AI Questions angle
];

/// 根据一组批注计算 item 级评分调整。使用**精确 label 白名单**匹配，
/// 而不是子串 `.contains()` —— 后者会把 "非过时" 误判为 Drop，
/// 把 "非重点" 误判为 boost，静默丢数据或扭曲排序。
///
///   | 意图       | User 词    | AI 词      | 乘数  |
///   |-----------|-----------|-----------|------|
///   | 剔除       | 过时      | 过时      | drop |
///   | 强 boost   | 重点      | 要点 / 风险 | 1.5  |
///   | 中 boost   | 存疑 / 不懂 / 待深入 | 疑点 | 1.2  |
///
/// 白名单带 emoji 前缀 + 不带 emoji 两种写法都识别。用户要加新标签，必须
/// 在上面三个常量数组里显式登记。
pub fn compute_adjust(annotations: &[Annotation]) -> ScoreAdjust {
    let mut max_boost = 1.0f32;
    for a in annotations {
        let label = match &a.label {
            Some(l) => l.trim(),
            None => continue,
        };
        if DROP_LABELS.contains(&label) {
            return ScoreAdjust::Drop;
        }
        if STRONG_BOOST_LABELS.contains(&label) {
            max_boost = max_boost.max(1.5);
        } else if MEDIUM_BOOST_LABELS.contains(&label) {
            max_boost = max_boost.max(1.2);
        }
    }
    ScoreAdjust::Multiply(max_boost)
}

/// 应用统计 —— 便于 chat route 返给前端供 token chip 展开时显示。
///
/// 语义注意：
/// - `items_total`：加权前的候选条数（search_results 进入 weighting 时的数量）
/// - `items_dropped`：被 🗑 过时 剔除的条数
/// - `items_boosted`：被 boost 的条数（multiplier > 1.0）
/// - `items_kept`：`items_total - items_dropped` —— 最终注入到 chat 上下文的条数
///
/// 前端同时展示 total 和 kept，避免"检索 5 条但 chat 只看到 3 条"的歧义。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AnnotationWeightStats {
    pub items_total: usize,
    pub items_boosted: usize,
    pub items_dropped: usize,
    pub items_kept: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ann(label: &str) -> Annotation {
        Annotation {
            id: "x".into(),
            item_id: "i".into(),
            offset_start: 0,
            offset_end: 1,
            text_snippet: "s".into(),
            label: Some(label.into()),
            color: "yellow".into(),
            content: String::new(),
            source: "user".into(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn no_annotations_returns_neutral() {
        assert_eq!(compute_adjust(&[]), ScoreAdjust::Multiply(1.0));
    }

    #[test]
    fn outdated_drops_item() {
        let anns = vec![ann("🗑过时")];
        assert_eq!(compute_adjust(&anns), ScoreAdjust::Drop);
    }

    #[test]
    fn outdated_wins_over_boost() {
        // 即便有 ⭐ 重点，过时也必须剔除
        let anns = vec![ann("⭐重点"), ann("🗑过时")];
        assert_eq!(compute_adjust(&anns), ScoreAdjust::Drop);
    }

    #[test]
    fn important_boosts_1_5() {
        let anns = vec![ann("⭐重点")];
        assert_eq!(compute_adjust(&anns), ScoreAdjust::Multiply(1.5));
    }

    #[test]
    fn thinking_labels_boost_1_2() {
        for lbl in &["📍待深入", "🤔存疑", "❓不懂"] {
            let anns = vec![ann(lbl)];
            assert_eq!(
                compute_adjust(&anns),
                ScoreAdjust::Multiply(1.2),
                "label {lbl} should boost 1.2"
            );
        }
    }

    #[test]
    fn multiple_boosts_take_max_not_compound() {
        // ⭐ + 🤔 应取 1.5，而非 1.5*1.2 = 1.8
        let anns = vec![ann("⭐重点"), ann("🤔存疑")];
        assert_eq!(compute_adjust(&anns), ScoreAdjust::Multiply(1.5));
    }

    #[test]
    fn unknown_label_neutral() {
        let anns = vec![ann("自定义标签")];
        assert_eq!(compute_adjust(&anns), ScoreAdjust::Multiply(1.0));
    }

    #[test]
    fn empty_label_neutral() {
        let mut a = ann("重点");
        a.label = None; // 无 label 批注（纯高亮）
        assert_eq!(compute_adjust(&[a]), ScoreAdjust::Multiply(1.0));
    }

    #[test]
    fn apply_drop_zeros_score() {
        assert_eq!(ScoreAdjust::Drop.apply(0.75), 0.0);
    }

    #[test]
    fn apply_multiply_scales_score() {
        assert!((ScoreAdjust::Multiply(1.5).apply(0.5) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn user_label_with_plain_text_works() {
        // 用户精简版（无 emoji）
        let anns = vec![ann("重点")];
        assert_eq!(compute_adjust(&anns), ScoreAdjust::Multiply(1.5));
    }

    #[test]
    fn ai_label_highlights_recognized() {
        // AI 批注用"⭐ 要点"（Highlights angle）—— 视为强 boost
        assert_eq!(
            compute_adjust(&[ann("⭐ 要点")]),
            ScoreAdjust::Multiply(1.5)
        );
    }

    #[test]
    fn ai_label_risk_recognized_as_strong_boost() {
        // AI 的风险批注 —— 值得立即关注，强 boost
        assert_eq!(
            compute_adjust(&[ann("⚠️ 风险")]),
            ScoreAdjust::Multiply(1.5)
        );
    }

    #[test]
    fn ai_label_questions_recognized() {
        // AI 的疑点批注 —— 中 boost
        assert_eq!(
            compute_adjust(&[ann("🤔 疑点")]),
            ScoreAdjust::Multiply(1.2)
        );
    }

    #[test]
    fn ai_label_outdated_drops() {
        // AI 的过时批注 "🕰 过时" —— 和 user 的 "🗑过时" 一视同仁剔除
        assert_eq!(compute_adjust(&[ann("🕰 过时")]), ScoreAdjust::Drop);
    }

    // ==== 精确匹配回归测试（防子串误判 footgun）====

    #[test]
    fn nonoutdated_label_does_not_drop() {
        // "非过时" 明确是用户表达"不过时"的意思，绝不能触发 Drop
        assert_eq!(compute_adjust(&[ann("非过时")]), ScoreAdjust::Multiply(1.0));
    }

    #[test]
    fn descriptive_label_containing_outdated_does_not_drop() {
        // 用户自由输入包含"过时"二字的描述性 label 不应被误判
        assert_eq!(
            compute_adjust(&[ann("过时信息的反例")]),
            ScoreAdjust::Multiply(1.0)
        );
    }

    #[test]
    fn nonimportant_label_does_not_boost() {
        // "非重点" 是反向含义，不能 boost
        assert_eq!(compute_adjust(&[ann("非重点")]), ScoreAdjust::Multiply(1.0));
    }

    #[test]
    fn label_with_leading_trailing_whitespace_normalized() {
        // trim 兼容 user 输入前后的空格
        assert_eq!(
            compute_adjust(&[ann("  ⭐重点  ")]),
            ScoreAdjust::Multiply(1.5)
        );
        assert_eq!(compute_adjust(&[ann(" 🗑过时 ")]), ScoreAdjust::Drop);
    }

    #[test]
    fn custom_label_neutral() {
        // 任何未登记的自定义标签都视为中性，用户要扩展必须改常量
        assert_eq!(
            compute_adjust(&[ann("work_log")]),
            ScoreAdjust::Multiply(1.0)
        );
        assert_eq!(
            compute_adjust(&[ann("客户需求")]),
            ScoreAdjust::Multiply(1.0)
        );
    }
}
