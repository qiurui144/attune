//! Local zero-LLM extractive pre-cut (spec §3.2 STAGE 1).
//!
//! "First throw away half the tokens, for free." Scores each sentence in a block by
//! TF + position prior + heading-word hit + length-norm, keeps the top-K, and returns the
//! kept sentences joined. This is the first of three independent token-savings levers
//! (extractive pre-cut + cache reuse + cheap/reasoning split) and the only one that costs
//! zero LLM tokens. **No `LlmProvider` reference appears in this file** — it is strictly
//! local/zero-cost.

/// A sentence scorer. Default is [`TfPositionTitleScorer`]; a stronger local
/// embedding-based scorer can be plugged in later without touching the pipeline.
pub trait ExtractiveScorer {
    /// Score one sentence given its 0-based position, total sentence count, and the
    /// heading words of the enclosing block. Higher = more salient.
    fn score(&self, sentence: &str, position: usize, total: usize, heading_words: &[String]) -> f32;
}

/// TF + position prior + heading-word hit + length-norm (default scorer).
#[derive(Debug, Clone, Default)]
pub struct TfPositionTitleScorer;

impl ExtractiveScorer for TfPositionTitleScorer {
    fn score(&self, sentence: &str, position: usize, total: usize, heading_words: &[String]) -> f32 {
        let words = tokenize(sentence);
        if words.is_empty() {
            return 0.0;
        }
        // Term frequency: reward sentences with more distinct content words (length-normalized
        // so a very long sentence is not auto-winner).
        let distinct = {
            let mut v: Vec<&str> = words.iter().map(|s| s.as_str()).collect();
            v.sort_unstable();
            v.dedup();
            v.len()
        };
        let tf = (distinct as f32) / (1.0 + (words.len() as f32).sqrt());

        // Position prior: lead sentences carry topic; first/last get a bump.
        let pos_norm = if total <= 1 {
            1.0
        } else {
            position as f32 / (total - 1) as f32
        };
        let position_prior = 1.0 - 0.5 * pos_norm // earlier is better
            + if position == total.saturating_sub(1) { 0.15 } else { 0.0 }; // small last-sentence bump

        // Heading-word hit: a sentence echoing the chapter heading is on-topic.
        let lower = sentence.to_lowercase();
        let heading_hits = heading_words
            .iter()
            .filter(|h| !h.is_empty() && lower.contains(&h.to_lowercase()))
            .count() as f32;
        let heading_bonus = (heading_hits * 0.4).min(1.2);

        // Length-norm: penalize extreme-short fragments (often headers/noise).
        let len_penalty = if words.len() < 2 { 0.5 } else { 1.0 };

        (tf + position_prior + heading_bonus) * len_penalty
    }
}

/// Split a block into top-K sentences by score and return them re-joined in original order.
///
/// `keep_ratio` ∈ (0,1] is the fraction of sentences to keep (clamped to [0.05, 1.0]); at
/// least one sentence is kept for a non-empty block. Output is guaranteed never larger than
/// input (it is a subset of the original sentences in original order).
pub fn extract_candidates(block: &str, keep_ratio: f32, heading_words: &[String]) -> String {
    extract_candidates_with(&TfPositionTitleScorer, block, keep_ratio, heading_words)
}

/// As [`extract_candidates`] but with a custom scorer.
pub fn extract_candidates_with<S: ExtractiveScorer>(
    scorer: &S,
    block: &str,
    keep_ratio: f32,
    heading_words: &[String],
) -> String {
    let sentences = split_sentences(block);
    if sentences.is_empty() {
        return String::new();
    }
    let total = sentences.len();
    let ratio = keep_ratio.clamp(0.05, 1.0);
    let keep_n = ((total as f32) * ratio).ceil().max(1.0) as usize;
    let keep_n = keep_n.min(total);

    // Score every sentence (keep original index so we can restore order).
    let mut scored: Vec<(usize, f32)> = sentences
        .iter()
        .enumerate()
        .map(|(i, (_, text))| (i, scorer.score(text, i, total, heading_words)))
        .collect();
    // Pick top-K by score; ties broken by earlier position (stable enough).
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    let mut keep_idx: Vec<usize> = scored.into_iter().take(keep_n).map(|(i, _)| i).collect();
    keep_idx.sort_unstable();

    // Re-emit kept sentences in original order, with their original trailing punctuation.
    keep_idx
        .iter()
        .map(|&i| sentences[i].1.as_str())
        .collect::<Vec<_>>()
        .join("")
}

/// Sentence segmentation that handles CJK terminators (。！？；) and ASCII (.!?;\n).
/// Returns `(byte_range_unused, sentence_text_including_its_terminator)`.
fn split_sentences(text: &str) -> Vec<((), String)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        cur.push(ch);
        let is_term = matches!(ch, '。' | '！' | '？' | '；' | '.' | '!' | '?' | ';' | '\n');
        if is_term {
            let trimmed = cur.trim();
            if !trimmed.is_empty() {
                out.push(((), cur.clone()));
            }
            cur.clear();
        }
    }
    let tail = cur.trim();
    if !tail.is_empty() {
        out.push(((), cur));
    }
    out
}

/// Lowercase ASCII-and-CJK-aware word split for TF. CJK chars each count as a "word".
fn tokenize(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut buf = String::new();
    for ch in s.chars() {
        if is_cjk(ch) {
            if !buf.is_empty() {
                words.push(std::mem::take(&mut buf).to_lowercase());
            }
            words.push(ch.to_string());
        } else if ch.is_alphanumeric() {
            buf.push(ch);
        } else if !buf.is_empty() {
            words.push(std::mem::take(&mut buf).to_lowercase());
        }
    }
    if !buf.is_empty() {
        words.push(buf.to_lowercase());
    }
    words
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_compress::estimate_tokens;

    #[test]
    fn test_keeps_top_sentences() {
        // First sentence (topic) + a heading-echoing sentence should survive a 0.5 keep.
        let block = "Rust ownership prevents data races. The weather is nice today. \
                     Ownership is the core memory-safety mechanism. I had coffee.";
        let heading = vec!["ownership".to_string()];
        let kept = extract_candidates(block, 0.5, &heading);
        assert!(kept.contains("Ownership is the core"), "heading-echo kept: {kept}");
        assert!(kept.len() < block.len(), "output strictly smaller");
    }

    #[test]
    fn test_cjk_sentence_split() {
        let block = "所有权防止数据竞争。今天天气很好。所有权是核心内存安全机制。我喝了咖啡。";
        let heading = vec!["所有权".to_string()];
        let kept = extract_candidates(block, 0.5, &heading);
        // keeps ~2 of 4; the heading-echoing sentence must be present.
        assert!(kept.contains("所有权是核心"), "cjk heading-echo kept: {kept}");
        assert!(kept.chars().count() < block.chars().count());
    }

    #[test]
    fn test_empty_block() {
        assert_eq!(extract_candidates("", 0.5, &[]), "");
        assert_eq!(extract_candidates("   \n  ", 0.5, &[]), "");
    }

    #[test]
    fn test_keep_ratio_bounds() {
        let block = "One. Two. Three. Four. Five. Six. Seven. Eight. Nine. Ten.";
        // ratio 1.0 keeps all (output == all sentences, same token count region)
        let all = extract_candidates(block, 1.0, &[]);
        assert_eq!(split_sentences(&all).len(), 10);
        // ratio below floor still keeps ≥1
        let one = extract_candidates(block, 0.0, &[]);
        assert!(!one.is_empty());
        assert!(split_sentences(&one).len() >= 1);
        // ratio above 1.0 clamps to all
        let over = extract_candidates(block, 5.0, &[]);
        assert_eq!(split_sentences(&over).len(), 10);
    }

    #[test]
    fn test_single_sentence_block() {
        let block = "Just one sentence here.";
        let kept = extract_candidates(block, 0.3, &[]);
        assert_eq!(kept.trim(), "Just one sentence here.");
    }

    #[test]
    fn test_pre_cut_lever_40pct_on_1000_cjk() {
        // AC: on a 1000-CJK-char block, extractive output estimate_tokens ≤ 0.6 × input.
        // Build a ~1000-char doc of 20 sentences (50 chars each), heading-relevant only a few.
        let mut block = String::new();
        for i in 0..20 {
            if i % 5 == 0 {
                block.push_str("内存安全是系统编程的核心所有权机制保证无数据竞争且零成本抽象非常重要。");
            } else {
                block.push_str("这是一段无关紧要的填充文字用来增加文档长度并稀释关键信息密度内容。");
            }
        }
        let input_tokens = estimate_tokens(&block);
        let kept = extract_candidates(&block, 0.5, &["所有权".to_string(), "内存安全".to_string()]);
        let kept_tokens = estimate_tokens(&kept);
        assert!(
            (kept_tokens as f32) <= 0.6 * (input_tokens as f32),
            "extractive pre-cut lever: kept {kept_tokens} tok must be ≤ 0.6 × input {input_tokens} tok"
        );
    }

    // proptest (§6.1 ≥3): output never larger than input; never panics on adversarial input.
    proptest::proptest! {
        #[test]
        fn proptest_output_token_le_input(s in ".{0,500}", ratio in 0.0f32..2.0f32) {
            let out = extract_candidates(&s, ratio, &[]);
            // Output is a subset of original sentences → token count never exceeds input.
            proptest::prop_assert!(estimate_tokens(&out) <= estimate_tokens(&s) + 1);
        }

        #[test]
        fn proptest_never_panics_on_emoji_mixed(s in "[\\p{Han}a-zA-Z0-9 .。!！?？\u{1F600}-\u{1F64F}]{0,300}") {
            let _ = extract_candidates(&s, 0.5, &["test".to_string()]);
        }

        #[test]
        fn proptest_keep_ratio_one_is_superset_size(s in "[a-zA-Z .]{0,300}") {
            // ratio=1.0 must keep at least as much as ratio=0.2
            let full = extract_candidates(&s, 1.0, &[]);
            let part = extract_candidates(&s, 0.2, &[]);
            proptest::prop_assert!(estimate_tokens(&full) >= estimate_tokens(&part));
        }
    }
}
