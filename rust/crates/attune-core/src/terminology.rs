//! Terminology consistency — deterministic, zero-cost term-variant detection + unification.
//!
//! ## 定位
//!
//! 全库 / 选定范围内,把"同一术语的不同写法"收敛成一致写法。**规则层零成本优先**
//! (CLAUDE.md §成本契约 🆓 层):只用确定性规范化 (大小写 / 全半角 / 空白 / 标点),
//! **不调 LLM**。语义同义判断 (需理解上下文) 留给上层,本模块不做 —— 本模块是
//! 确定性地基,可单独可靠运行。
//!
//! ## 算法
//!
//! 1. 对每个候选术语算一个"规范键" (normalize key):大小写折叠 + 全角→半角 +
//!    空白折叠 + 去首尾标点。
//! 2. 规范键相同但原文不同的术语 = 一个"变体簇"。
//! 3. 每簇推荐一个"首选写法" (出现频次最高;并列时取字典序最小,保证确定性)。
//!
//! ## 成本契约守卫
//!
//! 本模块所有公开函数签名**不含 `LlmProvider`** —— 编译期即保证零 LLM 调用。
//! `terminology` 永远是 🆓 零成本层。若未来需要语义判断,应另开一个显式标 💰 的
//! 上层模块,**不污染本模块**。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 一个术语变体簇:同一规范键下的所有原文写法 + 推荐统一写法。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermCluster {
    /// 规范键 (内部用,也便于调试)。
    pub canonical_key: String,
    /// 推荐统一写法 (簇内首选)。
    pub preferred: String,
    /// 所有变体 (原文写法, 出现次数),按写法字典序稳定排序。
    pub variants: Vec<TermVariant>,
    /// 簇内总出现次数。
    pub total_count: usize,
}

impl TermCluster {
    /// 是否真的存在不一致 (>1 种写法)。只有这种簇才值得提示用户统一。
    pub fn is_inconsistent(&self) -> bool {
        self.variants.len() > 1
    }
}

/// 一个具体写法 + 其频次。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermVariant {
    pub text: String,
    pub count: usize,
}

/// 把一个术语原文规范化成"规范键"。确定性、零成本。
///
/// 步骤:
/// 1. 全角 ASCII / 标点 → 半角 (NFKC 风格的子集,手写覆盖常见 CJK 全角)。
/// 2. ASCII 大小写折叠 (lowercase)。
/// 3. 内部连续空白 (含全角空格) 折叠为单个普通空格;去首尾空白。
/// 4. 去首尾标点 (常见中英标点)。
///
/// 注意:**不**做 trad↔simp 转换 (那需要词表,且可能误伤);本层只做形态归一。
pub fn normalize_key(term: &str) -> String {
    let halfwidth: String = term.chars().map(fullwidth_to_half).collect();
    // 大小写折叠 (仅影响 ASCII / Latin)。
    let lowered = halfwidth.to_lowercase();
    // 空白折叠。
    let collapsed = collapse_whitespace(&lowered);
    // 去首尾标点。
    trim_punct(&collapsed).to_string()
}

/// 全角字符 → 半角 (覆盖 ASCII 区全角 U+FF01..U+FF5E 与全角空格 U+3000)。
fn fullwidth_to_half(c: char) -> char {
    match c {
        '\u{3000}' => ' ', // 全角空格
        '\u{FF01}'..='\u{FF5E}' => {
            // 全角 ASCII 区:偏移 0xFEE0 映射回半角。
            char::from_u32(c as u32 - 0xFEE0).unwrap_or(c)
        }
        _ => c,
    }
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !in_ws && !out.is_empty() {
                out.push(' ');
            }
            in_ws = true;
        } else {
            out.push(ch);
            in_ws = false;
        }
    }
    // 去尾随折叠产生的空格。
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// 去首尾常见标点 (中英)。中间标点保留 (如 "GB/T 7714" 的 '/')。
///
/// 注意:本函数在 `normalize_key` 中**在 `fullwidth_to_half` 之后**调用,故全角
/// ASCII 区标点 (如 `（` `）` `，`) 此时已被转成半角 —— 这里只需列半角 ASCII 标点
/// + 全角 ASCII 区**之外**的 CJK 专属标点 (、。「」【】《》 与弯引号)。
fn trim_punct(s: &str) -> &str {
    let is_edge_punct = |c: char| {
        matches!(
            c,
            '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' | '(' | ')' | '[' | ']'
                | '{' | '}'
                // CJK 专属标点 (不在全角 ASCII 区 U+FF01..FF5E)
                | '、' | '。' | '「' | '」' | '【' | '】' | '《' | '》'
                | '\u{201C}' | '\u{201D}' | '\u{2018}' | '\u{2019}'
        ) || c.is_whitespace()
    };
    s.trim_matches(is_edge_punct)
}

/// 从一组术语候选 (原文, 出现次数) 中收集变体簇。
///
/// 输入:同一术语可能以不同原文多次出现 —— 调用方先把候选术语抽出来 (例如分词 /
/// NER / 用户指定的术语表),连同各自频次传入。本函数按规范键聚类。
///
/// 输出:**只返回存在不一致的簇** (>1 种写法),按总频次降序、规范键升序稳定排序。
/// 完全一致的术语不需要用户处理,不返回 (减少噪声)。
pub fn collect_variants(candidates: &[(String, usize)]) -> Vec<TermCluster> {
    // key -> (variant_text -> count)
    let mut buckets: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for (text, count) in candidates {
        let trimmed = text.trim();
        if trimmed.is_empty() || *count == 0 {
            continue;
        }
        let key = normalize_key(trimmed);
        if key.is_empty() {
            continue;
        }
        *buckets
            .entry(key)
            .or_default()
            .entry(trimmed.to_string())
            .or_insert(0) += count;
    }

    let mut clusters: Vec<TermCluster> = buckets
        .into_iter()
        .filter(|(_, variants)| variants.len() > 1) // 只保留不一致簇
        .map(|(key, variant_map)| {
            let total_count: usize = variant_map.values().sum();
            // 变体列表:按写法字典序 (BTreeMap 已序),稳定。
            let variants: Vec<TermVariant> = variant_map
                .iter()
                .map(|(text, count)| TermVariant {
                    text: text.clone(),
                    count: *count,
                })
                .collect();
            // 首选写法:频次最高;并列取字典序最小 (确定性)。
            let preferred = variant_map
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
                .map(|(text, _)| text.clone())
                .unwrap_or_default();
            TermCluster {
                canonical_key: key,
                preferred,
                variants,
                total_count,
            }
        })
        .collect();

    // 排序:总频次降序,规范键升序 (稳定)。
    clusters.sort_by(|a, b| {
        b.total_count
            .cmp(&a.total_count)
            .then_with(|| a.canonical_key.cmp(&b.canonical_key))
    });
    clusters
}

/// 对一段文本应用统一:把某簇内的所有非首选变体替换成首选写法。
///
/// 确定性纯文本替换 (整词替换的近似:子串替换)。返回 (新文本, 替换次数)。
/// 注意:子串替换可能误伤更长术语的一部分 —— 调用方应在确认簇是"独立术语"
/// (非其他术语子串) 时使用,或先按长度降序处理。本函数按变体长度降序替换以
/// 降低误伤 (先替长的)。
pub fn apply_unification(text: &str, cluster: &TermCluster) -> (String, usize) {
    let mut variants: Vec<&TermVariant> = cluster
        .variants
        .iter()
        .filter(|v| v.text != cluster.preferred)
        .collect();
    // 长变体先替,降低短变体误吃长变体的风险。
    variants.sort_by_key(|v| std::cmp::Reverse(v.text.chars().count()));

    let mut out = text.to_string();
    let mut replaced = 0usize;
    for v in variants {
        if v.text.is_empty() {
            continue;
        }
        let occurrences = out.matches(&v.text).count();
        if occurrences > 0 {
            out = out.replace(&v.text, &cluster.preferred);
            replaced += occurrences;
        }
    }
    (out, replaced)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(pairs: &[(&str, usize)]) -> Vec<(String, usize)> {
        pairs.iter().map(|(s, c)| (s.to_string(), *c)).collect()
    }

    // ---- normalize_key 单元 (边界 ≥5) ----

    #[test]
    fn normalize_case_fold() {
        assert_eq!(normalize_key("OpenAI"), normalize_key("openai"));
        assert_eq!(normalize_key("HTTP"), "http");
    }

    #[test]
    fn normalize_fullwidth_to_half() {
        // 全角字母数字 → 半角。
        assert_eq!(normalize_key("ＡＰＩ"), "api");
        assert_eq!(normalize_key("ＧＰＴ４"), "gpt4");
    }

    #[test]
    fn normalize_whitespace_collapse() {
        assert_eq!(normalize_key("GB/T  7714"), "gb/t 7714");
        assert_eq!(normalize_key("  spaced  out  "), "spaced out");
        // 全角空格。
        assert_eq!(normalize_key("a\u{3000}b"), "a b");
    }

    #[test]
    fn normalize_trims_edge_punct_keeps_inner() {
        assert_eq!(normalize_key("(API)"), "api");
        assert_eq!(normalize_key("“术语”"), "术语");
        // 内部 '/' 保留。
        assert_eq!(normalize_key("GB/T"), "gb/t");
    }

    #[test]
    fn normalize_empty_and_punct_only() {
        assert_eq!(normalize_key(""), "");
        assert_eq!(normalize_key("，。！"), "");
        assert_eq!(normalize_key("   "), "");
    }

    // ---- collect_variants happy / golden 类 (≥10 场景) ----

    #[test]
    fn collect_detects_case_variant() {
        let c = collect_variants(&cand(&[("OpenAI", 3), ("openai", 2), ("OPENAI", 1)]));
        assert_eq!(c.len(), 1);
        assert!(c[0].is_inconsistent());
        assert_eq!(c[0].total_count, 6);
        // 首选 = 频次最高 = OpenAI。
        assert_eq!(c[0].preferred, "OpenAI");
        assert_eq!(c[0].variants.len(), 3);
    }

    #[test]
    fn collect_ignores_consistent_terms() {
        // 单一写法 → 不返回 (无不一致)。
        let c = collect_variants(&cand(&[("Rust", 10)]));
        assert!(c.is_empty());
    }

    #[test]
    fn collect_fullwidth_variant() {
        let c = collect_variants(&cand(&[("API", 5), ("ＡＰＩ", 2)]));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].preferred, "API");
        assert_eq!(c[0].total_count, 7);
    }

    #[test]
    fn collect_whitespace_variant() {
        let c = collect_variants(&cand(&[("GB/T 7714", 4), ("GB/T  7714", 1)]));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].variants.len(), 2);
    }

    #[test]
    fn collect_multiple_clusters_sorted_by_frequency() {
        let c = collect_variants(&cand(&[
            ("api", 1),
            ("API", 1), // cluster api: total 2
            ("OpenAI", 50),
            ("openai", 40), // cluster openai: total 90
        ]));
        assert_eq!(c.len(), 2);
        // 高频簇在前。
        assert_eq!(c[0].canonical_key, "openai");
        assert_eq!(c[1].canonical_key, "api");
    }

    #[test]
    fn collect_preferred_tiebreak_is_deterministic() {
        // 两写法频次相同 → 取字典序最小。
        let c = collect_variants(&cand(&[("Beta", 5), ("beta", 5)]));
        assert_eq!(c.len(), 1);
        // 'B' (0x42) < 'b' (0x62) → "Beta" 字典序更小。
        assert_eq!(c[0].preferred, "Beta");
    }

    #[test]
    fn collect_skips_empty_and_zero_count() {
        let c = collect_variants(&cand(&[("", 5), ("X", 0), ("Real", 2), ("real", 1)]));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].canonical_key, "real");
    }

    #[test]
    fn collect_punct_only_skipped() {
        let c = collect_variants(&cand(&[("，", 3), ("。", 2)]));
        assert!(c.is_empty());
    }

    #[test]
    fn collect_three_way_variant_counts() {
        let c = collect_variants(&cand(&[
            ("DeepSeek", 2),
            ("deepseek", 2),
            ("ＤＥＥＰＳＥＥＫ", 1),
        ]));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].variants.len(), 3);
        assert_eq!(c[0].total_count, 5);
    }

    #[test]
    fn collect_variants_list_is_sorted_stable() {
        let c = collect_variants(&cand(&[("zeta", 1), ("Zeta", 1), ("ZETA", 1)]));
        assert_eq!(c.len(), 1);
        // 变体按写法字典序: ZETA < Zeta < zeta (ASCII)
        let texts: Vec<&str> = c[0].variants.iter().map(|v| v.text.as_str()).collect();
        assert_eq!(texts, vec!["ZETA", "Zeta", "zeta"]);
    }

    // ---- apply_unification ----

    #[test]
    fn apply_replaces_non_preferred() {
        let cluster = TermCluster {
            canonical_key: "openai".into(),
            preferred: "OpenAI".into(),
            variants: vec![
                TermVariant {
                    text: "OpenAI".into(),
                    count: 3,
                },
                TermVariant {
                    text: "openai".into(),
                    count: 2,
                },
            ],
            total_count: 5,
        };
        let (out, n) = apply_unification("我用 openai 和 openai 的 API", &cluster);
        assert_eq!(n, 2);
        assert_eq!(out, "我用 OpenAI 和 OpenAI 的 API");
    }

    #[test]
    fn apply_no_match_returns_zero() {
        let cluster = TermCluster {
            canonical_key: "x".into(),
            preferred: "X".into(),
            variants: vec![
                TermVariant {
                    text: "X".into(),
                    count: 1,
                },
                TermVariant {
                    text: "x".into(),
                    count: 1,
                },
            ],
            total_count: 2,
        };
        let (out, n) = apply_unification("no terms here", &cluster);
        assert_eq!(n, 0);
        assert_eq!(out, "no terms here");
    }

    // ---- 异常 / 边界 ----

    #[test]
    fn collect_empty_input() {
        assert!(collect_variants(&[]).is_empty());
    }

    #[test]
    fn collect_all_consistent_returns_empty() {
        let c = collect_variants(&cand(&[("Same", 3), ("Same", 2)]));
        // 同写法合并成一个变体 → 不算不一致。
        assert!(c.is_empty());
    }

    // ---- proptest (≥3) ----
    use proptest::prelude::*;

    proptest! {
        /// normalize_key 幂等:normalize(normalize(x)) == normalize(x)。
        #[test]
        fn prop_normalize_idempotent(s in ".{0,40}") {
            let once = normalize_key(&s);
            let twice = normalize_key(&once);
            prop_assert_eq!(once, twice);
        }

        /// 大小写不影响规范键 (ASCII)。
        #[test]
        fn prop_case_insensitive(s in "[a-zA-Z]{1,20}") {
            prop_assert_eq!(normalize_key(&s), normalize_key(&s.to_uppercase()));
        }

        /// collect 返回的每个簇 total_count == 各变体 count 之和,且仅含不一致簇。
        #[test]
        fn prop_cluster_count_invariant(
            terms in prop::collection::vec(("[a-zA-Z]{1,8}", 1usize..10), 0..30)
        ) {
            let cands: Vec<(String, usize)> = terms;
            let clusters = collect_variants(&cands);
            for cl in &clusters {
                let sum: usize = cl.variants.iter().map(|v| v.count).sum();
                prop_assert_eq!(sum, cl.total_count);
                prop_assert!(cl.variants.len() > 1);
            }
        }

        /// apply_unification 后,首选写法仍在 (若原文含任一变体) 且替换计数非负确定。
        #[test]
        fn prop_apply_deterministic(n in 0usize..5) {
            let cluster = TermCluster {
                canonical_key: "t".into(),
                preferred: "T".into(),
                variants: vec![
                    TermVariant { text: "T".into(), count: 1 },
                    TermVariant { text: "t".into(), count: 1 },
                ],
                total_count: 2,
            };
            let text = "t ".repeat(n);
            let (out1, c1) = apply_unification(&text, &cluster);
            let (out2, c2) = apply_unification(&text, &cluster);
            prop_assert_eq!(out1, out2);
            prop_assert_eq!(c1, c2);
            prop_assert_eq!(c1, n);
        }
    }
}
