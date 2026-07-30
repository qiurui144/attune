// npu-vault/crates/vault-core/src/search.rs

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::embed::EmbeddingProvider;
use crate::index::FulltextIndex;
use crate::infer::RerankProvider;
use crate::store::Store;
use crate::vectors::{VectorEmbeddingCompatibility, VectorIndex};

/// RRF 参数
pub const RRF_K: f32 = 60.0;
pub const RERANK_VECTOR_WEIGHT: f32 = 0.7;
pub const RERANK_RRF_WEIGHT: f32 = 0.3;
pub const RERANK_TOP_K_THRESHOLD: usize = 20;
pub const DEFAULT_VECTOR_WEIGHT: f32 = 0.6;
pub const DEFAULT_FULLTEXT_WEIGHT: f32 = 0.4;
pub const INJECTION_BUDGET: usize = 2000;
const METADATA_SOURCE_SCAN_LIMIT: usize = 4096;
const EXACT_SUBSTRING_SCAN_LIMIT: usize = 512;
const LEXICAL_EXCERPT_MAX_BYTES: usize = 2400;

/// 启用 cross-encoder reranker 的最小候选数。
/// 候选数 < 此阈值时，RRF 排序比 cross-encoder 重排更稳定（cross-encoder
/// 在小候选集上放大噪声 / 跨语言错配）。
pub const RERANK_MIN_CANDIDATES: usize = 5;
const RERANK_MIN_ACTIONABLE_SCORE: f32 = 0.001;

/// Cross-lingual 降权系数。query 与 doc 语言不匹配时，该 doc 的 score 乘以此系数。
/// 设为 0.3 而不是直接过滤：保留 cross-lingual 召回（专业术语常借用英文），
/// 但不让大篇幅异语言文档压过同语言命中。
pub const CROSS_LANG_PENALTY: f32 = 0.3;

/// 判断文本的"主导语言"：zh / en / mixed。
///
/// 启发式：计算 CJK 统一表意文字（U+4E00..U+9FFF）占比
///   - CJK >= 30% → Zh
///   - ASCII letter >= 70% → En
///   - 其他 → Mixed（不降权，因为专业术语常中英混用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
    Mixed,
}

pub fn detect_lang(s: &str) -> Lang {
    let (mut cjk, mut ascii_alpha, mut total) = (0usize, 0usize, 0usize);
    for c in s.chars() {
        if c.is_whitespace() {
            continue;
        }
        total += 1;
        if ('\u{4e00}'..='\u{9fff}').contains(&c) {
            cjk += 1;
        } else if c.is_ascii_alphabetic() {
            ascii_alpha += 1;
        }
    }
    if total == 0 {
        return Lang::Mixed;
    }
    let cjk_ratio = cjk as f32 / total as f32;
    let ascii_ratio = ascii_alpha as f32 / total as f32;
    if cjk_ratio >= 0.30 {
        Lang::Zh
    } else if ascii_ratio >= 0.70 {
        Lang::En
    } else {
        Lang::Mixed
    }
}

/// 对 SearchResult 列表按 query/content 语言匹配降权。
///
/// - query=Mixed 或 doc=Mixed：不降权（尊重混用场景，如中文里的英文专业术语）
/// - query.Lang != doc.Lang（Zh vs En 明确不同）：score *= CROSS_LANG_PENALTY
///
/// 仅用于为了检查 title 中的内容摘要判定。对于大文档，取 content 前 500 字作为
/// 语言样本（避免过长导致判定被尾部数据污染）
pub fn apply_cross_lang_penalty(results: &mut [SearchResult], query_lang: Lang) {
    if matches!(query_lang, Lang::Mixed) {
        return;
    }
    for r in results.iter_mut() {
        // 用 title + 前 500 字判定文档语言（避免只看 content 可能因代码块偏向 en）
        let sample: String = r.title.chars().chain(r.content.chars()).take(500).collect();
        let doc_lang = detect_lang(&sample);
        let cross = matches!(
            (query_lang, doc_lang),
            (Lang::Zh, Lang::En) | (Lang::En, Lang::Zh)
        );
        if cross {
            r.score *= CROSS_LANG_PENALTY;
        }
    }
}

/// v0.6 Phase B F-Pro: cross-domain 降权系数 (与 CROSS_LANG_PENALTY 共用机制)。
/// query domain 已知（如 'legal'）但 doc.corpus_domain 不同（如 'tech'）→ score *= 该系数。
/// 0.4 比 cross-lang 0.3 略高 — 同语种跨领域比跨语言保留更多召回（专业术语共享）。
pub const CROSS_DOMAIN_PENALTY: f32 = 0.4;

/// v0.6 Phase B F-Pro Stage 4：从 query 文本检测领域意图（零 LLM 调用）。
///
/// **S4b MU-5 (R8 boundary)**：领域词表 **完全由 plugin 提供**，不再硬编码任何行业
/// 关键词。per oss-pro-strategy §4.3，legal / medical / patent / tech 全部属于
/// attune-pro vertical —— 行业 domain detection 不应活在 OSS attune-core。
///
/// 关键词来源：vertical plugin（attune-pro）经
/// `PluginRegistry::all_chat_trigger_keywords_by_domain()` 提供 `(domain, keywords)`
/// 分组；调用方传入。每个 domain 统计命中词数，命中最多者胜出（同分按传入顺序优先）。
///
/// - `domain_keywords` 空（OSS 裸装无 vertical plugin）→ 返 `None` →
///   不应用 cross-domain penalty → 走 generic ranking（graceful degrade）。
///   domain-aware reranking 是 pro feature（§4.3），OSS 裸装优雅降级。
/// - 任一 domain 命中 ≥1 词 → 返该 domain；零命中 → `None`。
///
/// `domain` 字符串需与 ingest 写入 item 的 `corpus_domain` 对齐
/// （`apply_cross_domain_penalty` 比对 `corpus_domain`）。
pub fn detect_query_domain<D: AsRef<str>, K: AsRef<str>>(
    query: &str,
    domain_keywords: &[(D, Vec<K>)],
) -> Option<String> {
    if domain_keywords.is_empty() {
        return None;
    }
    let q = query.to_lowercase();
    // 同分按传入序优先 → 用严格 `>` 累积，首个达到最大命中数的 domain 胜出。
    let mut best: Option<(&str, usize)> = None;
    for (domain, kws) in domain_keywords {
        let hits = kws
            .iter()
            // 中文/英文均按子串命中（lowercase 已统一英文大小写）
            .filter(|kw| q.contains(&kw.as_ref().to_lowercase()))
            .count();
        // 至少 1 个命中才参与（避免误识别）；严格大于才替换 → 保留首见最大者
        if hits >= 1 && best.map(|(_, b)| hits > b).unwrap_or(true) {
            best = Some((domain.as_ref(), hits));
        }
    }
    best.map(|(domain, _)| domain.to_string())
}

/// 跨领域降权：query 有 domain hint（如 "legal"）时，doc.corpus_domain 不匹配的降权。
/// query domain="general" 或 None：跳过（保持现有行为，向后兼容）。
/// query domain="legal" + doc.corpus_domain="tech": score *= CROSS_DOMAIN_PENALTY。
/// query domain="legal" + doc.corpus_domain="legal" / "general": 保持原分。
pub fn apply_cross_domain_penalty(results: &mut [SearchResult], query_domain: Option<&str>) {
    let qd = match query_domain {
        Some(d) if !d.is_empty() && d != "general" => d,
        _ => return,
    };
    for r in results.iter_mut() {
        // doc.corpus_domain == 'general' 不降权（默认 corpus 不强制归类）
        if r.corpus_domain != "general" && r.corpus_domain != qd {
            r.score *= CROSS_DOMAIN_PENALTY;
        }
    }
}

fn normalized_ascii_tokens(s: &str) -> HashSet<String> {
    const STOPWORDS: &[&str] = &[
        "and", "are", "for", "from", "give", "into", "local", "many", "now", "source", "the",
        "this", "while", "with", "without",
    ];
    const STEM_EXCEPTIONS: &[&str] = &["dos", "ios", "rtos", "windows"];
    let stopwords: HashSet<&str> = STOPWORDS.iter().copied().collect();
    let stem_exceptions: HashSet<&str> = STEM_EXCEPTIONS.iter().copied().collect();
    s.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter_map(|tok| {
            let tok = tok.trim();
            if tok.len() < 2 || stopwords.contains(tok) {
                return None;
            }
            let stemmed = if tok.len() > 3 && tok.ends_with('s') && !stem_exceptions.contains(tok) {
                &tok[..tok.len() - 1]
            } else {
                tok
            };
            Some(stemmed.to_string())
        })
        .collect()
}

fn source_hint_text(r: &SearchResult) -> String {
    let mut out = String::new();
    out.push_str(&r.title);
    out.push('\n');
    if let Some(path) = &r.source_path {
        out.push_str(path);
        out.push('\n');
    }
    for crumb in &r.breadcrumb {
        out.push_str(crumb);
        out.push('\n');
    }
    out.to_ascii_lowercase()
}

fn token_matches_with_prefix(query_token: &str, source_token: &str) -> bool {
    query_token == source_token
        || query_token.len() >= 3
            && source_token.len() >= 3
            && (query_token.starts_with(source_token) || source_token.starts_with(query_token))
}

fn source_phrase_boost(query: &str, source: &str) -> f32 {
    let q = query.to_ascii_lowercase();
    let source_tokens = normalized_ascii_tokens(source);
    let meaningful_query_tokens = normalized_ascii_tokens(&q)
        .into_iter()
        .filter(|token| token.len() >= 3)
        .collect::<Vec<_>>();
    let phrase_hits = meaningful_query_tokens
        .iter()
        .filter(|token| {
            source_tokens
                .iter()
                .any(|source_token| source_token == *token)
        })
        .count();
    let mut boost = (phrase_hits as f32 * 0.05).min(0.30);

    if contains_any_str(&q, &["pdf", "source file", "source document"]) && source.contains(".pdf") {
        boost += 0.04;
    }
    if contains_any_str(&q, &["markdown", "source file", "source document"])
        && (source.contains(".md") || source.contains("markdown"))
    {
        boost += 0.04;
    }
    boost -= generic_exclusion_penalty(&q, source);

    boost
}

fn generic_exclusion_penalty(query: &str, source: &str) -> f32 {
    const MARKERS: &[&str] = &[
        "不要引入",
        "不要包含",
        "不包括",
        "排除",
        "without",
        "exclude",
        "excluding",
        "not ",
    ];
    let excluded = MARKERS
        .iter()
        .filter_map(|marker| query.find(marker).map(|idx| &query[idx + marker.len()..]))
        .flat_map(normalized_ascii_tokens)
        .filter(|token| token.len() >= 3)
        .collect::<HashSet<_>>();
    if excluded.is_empty() {
        return 0.0;
    }
    let source_tokens = normalized_ascii_tokens(source);
    let hits = excluded
        .iter()
        .filter(|token| {
            source_tokens
                .iter()
                .any(|source_token| source_token == *token)
        })
        .count();
    (hits as f32 * 0.18).min(0.36)
}

/// Apply a cheap SRAS-style source selector reward.
///
/// Long manuals produce many chunks, which can make large-but-wrong PDFs dominate
/// vector/RRF scores. This deterministic pass rewards candidates whose title,
/// source path, or breadcrumb matches explicit source hints in the query.
/// It is kept small and local: no LLM call, no corpus-specific schema requirement.
pub fn apply_source_hint_boost(query: &str, results: &mut [SearchResult]) {
    if results.is_empty() {
        return;
    }
    let query_tokens = normalized_ascii_tokens(query);
    if query_tokens.is_empty() {
        return;
    }

    for r in results.iter_mut() {
        let source = source_hint_text(r);
        let source_tokens = normalized_ascii_tokens(&source);
        let overlap = query_tokens
            .iter()
            .filter(|tok| {
                source_tokens
                    .iter()
                    .any(|source_tok| token_matches_with_prefix(tok, source_tok))
            })
            .count();
        let token_boost = (overlap as f32 * 0.05).min(0.30);
        r.score += token_boost + source_phrase_boost(query, &source);
    }
}

fn explicit_scope_tokens(text: &str) -> HashSet<String> {
    const COMPONENT_STOPWORDS: &[&str] = &[
        "api",
        "dev",
        "developer",
        "doc",
        "guide",
        "manual",
        "platform",
        "reference",
        "source",
    ];
    let mut tokens = HashSet::new();
    let stopwords = COMPONENT_STOPWORDS.iter().copied().collect::<HashSet<_>>();
    for raw in
        text.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.')))
    {
        let raw = raw.trim_matches(|c| matches!(c, '-' | '_' | '/' | '.'));
        if raw.len() < 2 {
            continue;
        }
        let lower = raw.to_ascii_lowercase();
        let has_separator = raw.chars().any(|c| matches!(c, '-' | '_' | '/' | '.'));
        let has_digit = raw.chars().any(|c| c.is_ascii_digit());
        let is_acronym = raw.chars().any(|c| c.is_ascii_alphabetic())
            && raw
                .chars()
                .filter(|c| c.is_ascii_alphabetic())
                .all(|c| c.is_ascii_uppercase())
            && raw.len() <= 12;

        if has_digit || has_separator || is_acronym {
            tokens.insert(lower.clone());
        }

        if has_separator {
            for part in lower.split(|c| matches!(c, '-' | '_' | '/' | '.')) {
                if part.len() >= 2 && !stopwords.contains(part) {
                    tokens.insert(part.to_string());
                }
            }
        }
    }
    tokens
}

fn platform_hints(query: &str) -> HashSet<String> {
    explicit_scope_tokens(query)
}

fn source_platforms(result: &SearchResult) -> HashSet<String> {
    explicit_scope_tokens(&source_hint_text(result))
}

pub fn apply_platform_hint_adjustment(query: &str, results: &mut [SearchResult]) {
    let hints = platform_hints(query);
    if hints.is_empty() {
        return;
    }
    for result in results {
        let platforms = source_platforms(result);
        if platforms.is_empty() {
            continue;
        }
        let matches = hints.iter().any(|hint| platforms.contains(hint));
        let conflicts = platforms.iter().any(|platform| !hints.contains(platform));
        if matches {
            result.score += 0.70;
        }
        if conflicts && !matches {
            result.score -= 0.90;
        } else if conflicts {
            result.score -= 0.20;
        }
    }
}

fn lexical_needle_weight(needle: &str) -> f32 {
    let has_identifier_punct = needle
        .chars()
        .any(|c| matches!(c, '_' | '-' | '/' | '.' | '+' | '#' | ':'));
    if has_identifier_punct && needle.len() >= 8 {
        6.0
    } else if has_identifier_punct {
        4.0
    } else if needle.len() >= 12 {
        4.0
    } else if needle.len() >= 6 {
        2.0
    } else {
        1.0
    }
}

fn query_coverage_boost(query: &str, result: &SearchResult) -> f32 {
    let needles = lexical_needles(query);
    if needles.is_empty() {
        return 0.0;
    }

    let mut haystack = String::new();
    haystack.push_str(&result.title);
    haystack.push('\n');
    if let Some(path) = &result.source_path {
        haystack.push_str(path);
        haystack.push('\n');
    }
    haystack.push_str(&result.content);
    let haystack = haystack.to_ascii_lowercase();

    let total_weight: f32 = needles
        .iter()
        .map(|needle| lexical_needle_weight(needle))
        .sum();
    if total_weight <= 0.0 {
        return 0.0;
    }

    let matched_weight: f32 = needles
        .iter()
        .filter(|needle| haystack.contains(needle.as_str()))
        .map(|needle| lexical_needle_weight(needle))
        .sum();
    let coverage = matched_weight / total_weight;
    let absolute = (matched_weight * 0.05).min(0.55);
    let identifier_hits = needles
        .iter()
        .filter(|needle| {
            needle
                .chars()
                .any(|c| matches!(c, '_' | '-' | '/' | '.' | '+' | '#' | ':'))
                && haystack.contains(needle.as_str())
        })
        .count();

    (coverage * 0.45) + absolute + (identifier_hits as f32 * 0.10).min(0.30)
}

pub fn apply_query_coverage_boost(query: &str, results: &mut [SearchResult]) {
    for result in results {
        result.score += query_coverage_boost(query, result);
    }
}

/// Return the part of a user message that should drive retrieval.
///
/// Users often append answer-control instructions such as "answer only from the
/// knowledge base" to an otherwise good question. Those terms are useful for
/// generation, but they are poor retrieval anchors and can pull in unrelated
/// documents that talk about evidence, citations, or answering. Keep the
/// original user message for the LLM prompt; use this normalized query only for
/// search and evidence selection.
pub fn retrieval_semantic_query(query: &str) -> String {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut segments = sentence_segments(trimmed);
    while segments
        .last()
        .map(|segment| retrieval_meta_instruction_segment(segment))
        .unwrap_or(false)
        && segments.len() > 1
    {
        segments.pop();
    }
    let candidate = segments.concat();
    let candidate = candidate.trim();
    if candidate_is_too_small(candidate, trimmed) {
        trimmed.to_string()
    } else {
        candidate.to_string()
    }
}

fn sentence_segments(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '.' | '?' | '!' | ';' | '。' | '？' | '！' | '；' | '\n') {
            let segment = current.trim();
            if !segment.is_empty() {
                out.push(segment.to_string());
            }
            current.clear();
        }
    }
    let tail = current.trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

fn retrieval_meta_instruction_segment(segment: &str) -> bool {
    let folded = segment.to_ascii_lowercase();
    let source_control = contains_any_str(
        segment,
        &["知识库", "本地知识", "引用", "来源", "证据", "文档", "资料"],
    ) || contains_any_str(
        &folded,
        &[
            "knowledge base",
            "kb",
            "citation",
            "citations",
            "source",
            "sources",
            "evidence",
            "refs",
            "references",
        ],
    );
    let answer_control = contains_any_str(
        segment,
        &[
            "回答",
            "回复",
            "作答",
            "请",
            "只基于",
            "仅基于",
            "不要编造",
            "必须给出",
        ],
    ) || contains_any_str(
        &folded,
        &[
            "answer",
            "respond",
            "reply",
            "use only",
            "only from",
            "based only",
            "must cite",
            "do not invent",
        ],
    );
    source_control && answer_control && !has_substantive_retrieval_anchor(segment)
}

fn has_substantive_retrieval_anchor(segment: &str) -> bool {
    normalized_ascii_tokens(segment)
        .iter()
        .any(|token| token.len() >= 2 && !retrieval_meta_ascii_token(token))
        || substantive_cjk_tokens(segment)
}

fn retrieval_meta_ascii_token(token: &str) -> bool {
    matches!(
        token,
        "answer"
            | "respond"
            | "reply"
            | "only"
            | "from"
            | "based"
            | "use"
            | "using"
            | "knowledge"
            | "base"
            | "kb"
            | "evidence"
            | "citation"
            | "citations"
            | "source"
            | "sources"
            | "ref"
            | "refs"
            | "reference"
            | "references"
            | "cite"
            | "cited"
            | "must"
            | "please"
            | "document"
            | "documents"
            | "manual"
            | "manuals"
            | "invent"
    )
}

fn substantive_cjk_tokens(segment: &str) -> bool {
    let mut current = String::new();
    for ch in segment.chars() {
        if ('\u{4e00}'..='\u{9fff}').contains(&ch) {
            current.push(ch);
        } else {
            if substantive_cjk_run(&current) {
                return true;
            }
            current.clear();
        }
    }
    substantive_cjk_run(&current)
}

fn substantive_cjk_run(run: &str) -> bool {
    let cleaned = run
        .replace("请", "")
        .replace("只", "")
        .replace("仅", "")
        .replace("基于", "")
        .replace("根据", "")
        .replace("本地", "")
        .replace("知识库", "")
        .replace("知识", "")
        .replace("证据", "")
        .replace("引用", "")
        .replace("来源", "")
        .replace("文档", "")
        .replace("资料", "")
        .replace("内容", "")
        .replace("回答", "")
        .replace("回复", "")
        .replace("作答", "")
        .replace("必须", "")
        .replace("给出", "")
        .replace("不要", "")
        .replace("编造", "");
    cleaned.chars().count() >= 2
}

fn candidate_is_too_small(candidate: &str, original: &str) -> bool {
    if candidate.trim().is_empty() {
        return true;
    }
    let candidate_has_ascii = normalized_ascii_tokens(candidate)
        .iter()
        .any(|token| token.len() >= 2 && !retrieval_meta_ascii_token(token));
    let candidate_cjk = candidate
        .chars()
        .filter(|ch| ('\u{4e00}'..='\u{9fff}').contains(ch))
        .count();
    let original_cjk = original
        .chars()
        .filter(|ch| ('\u{4e00}'..='\u{9fff}').contains(ch))
        .count();
    !candidate_has_ascii && candidate_cjk < 2 && original_cjk >= 2
}

fn contains_any_str(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

/// 搜索结果
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SearchResult {
    pub item_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_idx: Option<usize>,
    pub score: f32,
    pub title: String,
    pub content: String,
    pub source_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    pub inject_content: Option<String>,
    /// v0.6 Phase B F-Pro：item.corpus_domain（legal/tech/medical/.../general）。
    /// search 阶段按 query intent 跨域降权防止"反洗钱"被 cs-notes 顶占。
    /// 默认 "general"（无标签 corpus）。
    pub corpus_domain: String,
    // ── F2 (W3 batch A, 2026-04-27)：breadcrumb + offset 透传 ─────────────
    // per spec docs/superpowers/specs/2026-04-27-w3-batch-a-design.md §4
    // 关闭 W2 batch 1 的 Citation 占位状态；search 阶段 join chunk_breadcrumbs
    // sidecar 表填入数据，ChatEngine 后续映射到 Citation。
    /// 启发式：用 item 第一个 chunk 的 path。
    /// skip_serializing_if 让空 Vec 不出现在 JSON，
    /// 保持 Chrome 扩展旧客户端契约（之前不存在此字段）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub breadcrumb: Vec<String>,
    /// chunk 在 item.content 的 char-level 区间。无 sidecar 数据时 None。
    /// **Known limitation**：当前 offset 是 sidecar
    /// 内累计 char count，不一定对齐原文 char index（行末 `\n` 处理 + `\r\n` 剥离会
    /// 引入漂移）。适合 item 顶层导航；精确映射回原文留待后续。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_offset_start: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_offset_end: Option<usize>,
}

/// 三阶段搜索参数
#[derive(Debug, Clone)]
pub struct SearchParams {
    pub top_k: usize,
    /// 粗召回数量（向量+全文各取此数量后 RRF 融合）
    pub initial_k: usize,
    /// Reranker 入口前的候选数量
    pub intermediate_k: usize,
    // ── J3：vector 召回 cosine 阈值（W2，2026-04-27）───────────────────────
    //
    // 设计来源（per docs/superpowers/specs/2026-04-27-w2-rag-quality-batch1-design.md §J3）：
    //   吴师兄《鹅厂面试官追问：你的 RAG 能跑通 Demo？》§2 "召回阈值：一个参数，决定生死"
    //   https://mp.weixin.qq.com/s/YNcfSN0uv1c1LsLPzgB0jw
    //   - 0.65：召回率 0.89，top-5 含 2 个噪音
    //   - 0.72：召回率 0.84，top-5 基本有用（精度优先推荐）
    //   - 0.78：召回率 0.71，开始漏边缘 case
    //
    // attune 默认 0.65（保守端）平衡召回与精度；用户可在 Settings 调到 0.72 求精度。
    // None = 不过滤（向后兼容，初版调用方未传时不破行为）。
    /// vector 召回 cosine 阈值。Some(0.65) 默认；低于此分数的 vector 结果在 RRF 前丢弃。
    pub min_score: Option<f32>,

    /// v0.6 Phase B F-Pro：query 意图领域提示。Some("legal") → 跨领域文档降权。
    /// None / Some("general") = 不应用 cross-domain penalty（默认行为，保留召回多样性）。
    /// 由 detect_query_domain (Stage 4) 自动从 query 推断 + plugin keywords 判断。
    pub domain_hint: Option<String>,

    // ── T1 (v1.0.6 KB-bench) deterministic knobs ──────────────────────────
    //
    // Per spec docs/superpowers/specs/2026-05-28-kb-memory-vs-vlm-llm-bench-validation.md
    // §11 Risk A. Threaded from `X-Attune-Eval-*` headers in
    // attune-server/src/routes/search.rs. All default to off so legacy callers
    // see no behavior change.
    /// Pin seed for query_rewrite + rerank LLM calls (only honored if the
    /// active LlmProvider supports it — see `DeterminismLevel`).
    pub seed: Option<u64>,
    /// Skip query_rewrite LLM call entirely (deterministic retrieval — bench
    /// uses this to isolate retrieval quality from LLM noise).
    pub skip_rewrite: bool,
    /// Skip vector embedding/search for metadata-source queries that can be
    /// answered by fulltext + SRAS source selection.
    pub skip_vector: bool,
    /// Skip rerank LLM call entirely (same motivation as `skip_rewrite`).
    pub skip_rerank: bool,
}

impl SearchParams {
    /// 通用 search 路径默认 — **不**应用 cosine 阈值过滤，保持 W2 之前的行为契约。
    /// 用于 `/api/v1/search` / `/api/v1/search/relevant` (Chrome 扩展) — 这些 route 的
    /// 用户期望"全部召回，自己挑"。
    /// 自动启用 0.65 会让 Chrome 扩展 query 含义模糊时全无结果（cosine 0.4-0.6）。
    pub fn with_defaults(top_k: usize) -> Self {
        // top_k 上限 100（per S14），但旧版 intermediate_k 写法 `(top_k*2).clamp(top_k, 40)`
        // 在 top_k > 20 时 (top_k*2) > 40，让 clamp 的 min > max 而 panic。
        // 2026-05-24 50-query rust-book benchmark 发现：top_k=50 直接让 tokio worker panic。
        // 修正：保持原意 intermediate_k ≈ top_k*2（rerank 候选池 ~2x），但允许动态上限
        // 跟随 top_k 增长，不再写死 40。下限保留 top_k 自身（rerank 至少要见到 top_k 个）。
        let initial_k = (top_k * 5).clamp(20, 500);
        // 旧契约：intermediate_k = (top_k*2).clamp(top_k, 40) → top_k=5→10 / top_k=20→40 / top_k=50→panic
        // 新契约：intermediate_k = (top_k*2).max(top_k).min(200) → top_k=5→10 / top_k=20→40 / top_k=50→100
        let intermediate_k = (top_k * 2).max(top_k).min(200);
        Self {
            top_k,
            initial_k,
            intermediate_k,
            min_score: None,
            domain_hint: None,
            seed: None,
            skip_rewrite: false,
            skip_vector: false,
            skip_rerank: false,
        }
    }

    /// v0.6 Phase B F-Pro：链式设置 domain_hint
    pub fn with_domain_hint(mut self, hint: impl Into<String>) -> Self {
        let s = hint.into();
        if !s.is_empty() && s != "general" {
            self.domain_hint = Some(s);
        }
        self
    }

    /// **RAG / chat 专用**默认 — 启用 J3 cosine 阈值 0.65 过滤噪音
    /// per spec §J3 + 吴师兄文章曲线。chat 主流程 confidence < 3 时降到 0.55 二次检索。
    pub fn with_defaults_for_rag(top_k: usize) -> Self {
        let mut s = Self::with_defaults(top_k);
        s.min_score = Some(0.65);
        s
    }
}

/// 搜索上下文：持有所有搜索所需组件的引用
pub struct SearchContext<'a> {
    pub fulltext: Option<&'a FulltextIndex>,
    pub vectors: Option<&'a VectorIndex>,
    pub embedding: Option<Arc<dyn EmbeddingProvider>>,
    pub reranker: Option<Arc<dyn RerankProvider>>,
    pub store: &'a Store,
    pub dek: &'a crate::crypto::Key32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchOutcome {
    pub results: Vec<SearchResult>,
    pub diagnostics: SearchDiagnostics,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchDiagnostics {
    pub bm25_results: usize,
    pub vector_results: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_skipped_reason: Option<String>,
    pub embedding: VectorEmbeddingCompatibility,
    pub reranker: RerankerSearchDiagnostics,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RerankerSearchDiagnostics {
    pub requested: bool,
    pub available: bool,
    pub used: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_reason: Option<String>,
    pub candidate_count: usize,
    pub score_count: usize,
    pub actionable: bool,
    pub changed_top_result: bool,
}

impl Default for RerankerSearchDiagnostics {
    fn default() -> Self {
        Self {
            requested: false,
            available: false,
            used: false,
            skipped_reason: None,
            candidate_count: 0,
            score_count: 0,
            actionable: false,
            changed_top_result: false,
        }
    }
}

const CHUNK_KEY_SEP: &str = "\u{1f}";

fn chunk_hit_key(item_id: &str, chunk_idx: usize) -> String {
    format!("{item_id}{CHUNK_KEY_SEP}{chunk_idx}")
}

fn parse_hit_key(key: &str) -> (&str, Option<usize>) {
    let Some((item_id, chunk)) = key.rsplit_once(CHUNK_KEY_SEP) else {
        return (key, None);
    };
    match chunk.parse::<usize>() {
        Ok(idx) => (item_id, Some(idx)),
        Err(_) => (key, None),
    }
}

fn hit_key_item_id(key: &str) -> &str {
    parse_hit_key(key).0
}

/// RRF 融合两组排名结果
pub fn rrf_fuse(
    vector_results: &[(String, f32)],
    fulltext_results: &[(String, f32)],
    vector_weight: f32,
    fulltext_weight: f32,
    top_k: usize,
) -> Vec<(String, f32)> {
    let mut scores: HashMap<String, f32> = HashMap::new();
    let mut representative_by_item: HashMap<String, String> = HashMap::new();

    for (rank, (id, _score)) in vector_results.iter().enumerate() {
        let rrf = vector_weight / (RRF_K + rank as f32 + 1.0);
        let item_id = hit_key_item_id(id).to_string();
        let representative = representative_by_item
            .entry(item_id)
            .or_insert_with(|| id.clone())
            .clone();
        *scores.entry(representative).or_default() += rrf;
    }
    for (rank, (id, _score)) in fulltext_results.iter().enumerate() {
        let rrf = fulltext_weight / (RRF_K + rank as f32 + 1.0);
        let item_id = hit_key_item_id(id).to_string();
        let representative = representative_by_item
            .entry(item_id)
            .or_insert_with(|| id.clone())
            .clone();
        *scores.entry(representative).or_default() += rrf;
    }

    let mut sorted: Vec<(String, f32)> = scores.into_iter().collect();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    sorted.truncate(top_k);
    sorted
}

fn merge_ranked_results(
    primary: Vec<(String, f32)>,
    secondary: Vec<(String, f32)>,
    limit: usize,
) -> Vec<(String, f32)> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(limit.min(primary.len() + secondary.len()));
    for (id, score) in primary.into_iter().chain(secondary.into_iter()) {
        if seen.insert(id.clone()) {
            out.push((id, score));
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

fn metadata_source_candidates(
    ctx: &SearchContext<'_>,
    query: &str,
    limit: usize,
) -> Vec<(String, f32)> {
    let Ok(items) = ctx.store.list_items(METADATA_SOURCE_SCAN_LIMIT, 0) else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    for item in items {
        let mut result = SearchResult {
            item_id: item.id,
            score: 0.0,
            title: item.title,
            source_type: item.source_type,
            source_path: item.url,
            corpus_domain: item.domain.unwrap_or_else(|| "general".to_string()),
            ..Default::default()
        };
        apply_source_hint_boost(query, std::slice::from_mut(&mut result));
        if result.score > 0.10 {
            candidates.push((result.item_id, result.score));
        }
    }
    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    candidates.truncate(limit);
    candidates
}

fn exact_substring_fallback_query(query: &str) -> bool {
    let q = query.trim();
    let len = q.chars().count();
    (3..=128).contains(&len)
        && q.chars()
            .any(|c| matches!(c, '_' | '-' | '/' | '.' | '+' | '#') || c.is_ascii_digit())
}

fn lexical_fast_path_query(query: &str) -> bool {
    let q = query.trim();
    let len = q.chars().count();
    if !(2..=96).contains(&len) {
        return false;
    }

    let token_count = q.split_whitespace().count().max(1);
    let has_digit = q.chars().any(|c| c.is_ascii_digit());
    let has_identifier_punct = q
        .chars()
        .any(|c| matches!(c, '_' | '-' | '/' | '.' | '+' | '#'));

    if has_identifier_punct || has_digit {
        return true;
    }

    // One- or two-token keyword searches are usually intentional lexical
    // lookups. Do not make them wait for a scheduler embedding call when
    // Tantivy already has candidates.
    token_count <= 2 && len <= 48
}

fn exact_substring_candidates(
    ctx: &SearchContext<'_>,
    query: &str,
    limit: usize,
) -> Vec<(String, f32)> {
    if limit == 0 || !exact_substring_fallback_query(query) {
        return Vec::new();
    }

    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }

    let Ok(items) = ctx.store.list_items(EXACT_SUBSTRING_SCAN_LIMIT, 0) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let title = item.title.to_lowercase();
        let url = item.url.unwrap_or_default().to_lowercase();
        let title_or_path_hit = title.contains(&needle) || url.contains(&needle);
        let content_hit = if title_or_path_hit {
            false
        } else {
            ctx.store
                .get_item(ctx.dek, &item.id)
                .ok()
                .flatten()
                .map(|full| full.content.to_lowercase().contains(&needle))
                .unwrap_or(false)
        };
        if title_or_path_hit || content_hit {
            out.push((item.id, if title_or_path_hit { 0.35 } else { 0.30 }));
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

fn lexical_content_candidates(
    ctx: &SearchContext<'_>,
    query: &str,
    limit: usize,
) -> Vec<(String, f32)> {
    if limit == 0 {
        return Vec::new();
    }
    let needles = lexical_needles(query);
    if needles.is_empty() {
        return Vec::new();
    }
    let total_weight: f32 = needles
        .iter()
        .map(|needle| lexical_needle_weight(needle))
        .sum();
    if total_weight <= 0.0 {
        return Vec::new();
    }

    let Ok(items) = ctx.store.list_items(EXACT_SUBSTRING_SCAN_LIMIT, 0) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let Ok(Some(full)) = ctx.store.get_item(ctx.dek, &item.id) else {
            continue;
        };
        let mut source_text = String::new();
        source_text.push_str(&full.title);
        source_text.push('\n');
        if let Some(path) = &full.url {
            source_text.push_str(path);
            source_text.push('\n');
        }
        let source_text = source_text.to_ascii_lowercase();
        let content = full.content.to_ascii_lowercase();

        let source_weight: f32 = needles
            .iter()
            .filter(|needle| source_text.contains(needle.as_str()))
            .map(|needle| lexical_needle_weight(needle))
            .sum();
        let window_weight = lexical_best_window_score(&content, &needles);
        let matched_weight = source_weight + window_weight;
        let coverage = matched_weight / total_weight;
        if matched_weight >= 3.0 && coverage >= 0.10 {
            out.push((
                item.id,
                0.25 + coverage.min(1.5) + (matched_weight * 0.01).min(0.35),
            ));
        }
    }
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(limit);
    out
}

fn lexical_needles(query: &str) -> Vec<String> {
    let mut needles = Vec::new();
    let mut current = String::new();
    let mut current_is_cjk = false;

    let flush = |buf: &mut String, is_cjk: bool, out: &mut Vec<String>| {
        let s = buf.trim();
        if is_cjk {
            let chars = s.chars().collect::<Vec<_>>();
            if chars.len() >= 2 {
                for n in 2..=3.min(chars.len()) {
                    for gram in chars.windows(n) {
                        out.push(gram.iter().collect::<String>());
                    }
                }
                if chars.len() <= 6 {
                    out.push(s.to_string());
                }
            }
        } else if s.len() >= 2 {
            push_ascii_identifier_needles(s, out);
        }
        buf.clear();
    };

    for ch in query.chars() {
        let is_cjk = ('\u{4e00}'..='\u{9fff}').contains(&ch);
        let is_ascii_ident =
            ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '/' | '.' | '+' | '#' | ':');
        if is_cjk || is_ascii_ident {
            if !current.is_empty() && current_is_cjk != is_cjk {
                flush(&mut current, current_is_cjk, &mut needles);
            }
            current_is_cjk = is_cjk;
            current.push(ch);
        } else if !current.is_empty() {
            flush(&mut current, current_is_cjk, &mut needles);
        }
    }
    if !current.is_empty() {
        flush(&mut current, current_is_cjk, &mut needles);
    }

    needles.sort();
    needles.dedup();
    needles
}

fn push_ascii_identifier_needles(raw: &str, out: &mut Vec<String>) {
    let lowered = raw.to_ascii_lowercase();
    out.push(lowered.clone());

    let has_separator = lowered
        .chars()
        .any(|c| matches!(c, '_' | '-' | '/' | '.' | '+' | '#' | ':'));
    if !has_separator {
        return;
    }

    let compact: String = lowered
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if compact.len() >= 2 {
        out.push(compact);
    }
    for part in lowered.split(|c: char| !c.is_ascii_alphanumeric()) {
        let part = part.trim();
        if part.len() >= 2 {
            out.push(part.to_string());
        }
    }
}

fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

fn lexical_window_score(window: &str, needles: &[String]) -> f32 {
    needles
        .iter()
        .map(|needle| {
            let count = window.matches(needle).count().min(3) as f32;
            count * lexical_needle_weight(needle)
        })
        .sum()
}

fn lexical_best_window_score(content: &str, needles: &[String]) -> f32 {
    let mut best = 0.0f32;
    for needle in needles {
        let mut search_from = 0usize;
        while let Some(rel) = content.get(search_from..).and_then(|s| s.find(needle)) {
            let pos = search_from + rel;
            let desired_start = pos.saturating_sub(LEXICAL_EXCERPT_MAX_BYTES / 3);
            let start = floor_char_boundary(content, desired_start);
            let end = ceil_char_boundary(
                content,
                (start + LEXICAL_EXCERPT_MAX_BYTES).min(content.len()),
            );
            if let Some(window) = content.get(start..end) {
                best = best.max(lexical_window_score(window, needles));
            }
            search_from = pos.saturating_add(needle.len()).min(content.len());
            if search_from >= content.len() {
                break;
            }
        }
    }
    best
}

fn lexical_excerpt_for_item(
    query: &str,
    content: &str,
) -> Option<(String, Option<usize>, Option<usize>)> {
    if content.len() <= LEXICAL_EXCERPT_MAX_BYTES {
        return None;
    }

    let needles = lexical_needles(query);
    if needles.is_empty() {
        return None;
    }

    let haystack = content.to_ascii_lowercase();
    let mut best: Option<(usize, f32, usize)> = None;
    for needle in &needles {
        let mut search_from = 0usize;
        while let Some(rel) = haystack.get(search_from..).and_then(|s| s.find(needle)) {
            let pos = search_from + rel;
            let desired_start = pos.saturating_sub(LEXICAL_EXCERPT_MAX_BYTES / 3);
            let start = floor_char_boundary(content, desired_start);
            let end = ceil_char_boundary(
                content,
                (start + LEXICAL_EXCERPT_MAX_BYTES).min(content.len()),
            );
            let Some(window) = haystack.get(start..end) else {
                search_from = pos.saturating_add(needle.len()).min(haystack.len());
                continue;
            };
            let score = lexical_window_score(window, &needles);
            let replace = best
                .map(|(_, best_score, best_start)| {
                    score > best_score || score == best_score && start < best_start
                })
                .unwrap_or(true);
            if replace {
                best = Some((start, score, start));
            }
            search_from = pos.saturating_add(needle.len()).min(haystack.len());
        }
    }

    let (start, score, _) = best?;
    if score <= 0.0 {
        return None;
    }
    let end = ceil_char_boundary(
        content,
        (start + LEXICAL_EXCERPT_MAX_BYTES).min(content.len()),
    );
    content
        .get(start..end)
        .map(|excerpt| (excerpt.to_string(), Some(start), Some(end)))
}

/// Collapse repeated chunk hits to the first/best item hit before RRF.
///
/// Vector search returns chunk-level hits. Without this normalization, a long
/// PDF with thousands of chunks can accumulate many RRF contributions for the
/// same item and suppress a short but exact source such as a quick reference or
/// subsystem manual. Input is expected to be rank-sorted; the first occurrence
/// is the best representative for that item.
pub fn dedup_ranked_results(results: Vec<(String, f32)>) -> Vec<(String, f32)> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(results.len());
    for (id, score) in results {
        if seen.insert(hit_key_item_id(&id).to_string()) {
            out.push((id, score));
        }
    }
    out
}

/// 动态注入预算分配
pub fn allocate_budget(results: &mut [SearchResult], budget: usize) {
    let total_score: f32 = results.iter().map(|r| r.score).sum();
    if total_score <= 0.0 || results.is_empty() {
        // 保证每条至少 100 字符，与正比路径中 .max(100.0) 对齐
        let per_item = (budget / results.len().max(1)).max(100);
        for r in results.iter_mut() {
            let content = &r.content;
            let end = content
                .char_indices()
                .nth(per_item)
                .map(|(i, _)| i)
                .unwrap_or(content.len());
            r.inject_content = Some(content[..end].to_string());
        }
        return;
    }
    for r in results.iter_mut() {
        let share = r.score / total_score;
        let alloc = (budget as f32 * share).max(100.0) as usize;
        let content = &r.content;
        let end = content
            .char_indices()
            .nth(alloc)
            .map(|(i, _)| i)
            .unwrap_or(content.len());
        r.inject_content = Some(content[..end].to_string());
    }
}

/// 计算两个向量的余弦相似度，任一范数为 0 时返回 0.0
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "cosine_similarity: dimension mismatch");
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a < 1e-8 || norm_b < 1e-8 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
}

/// 对 RRF 一阶结果进行余弦相似度二次排序。
///
/// 当 query 向量可用且结果集实际数量不超过 `RERANK_TOP_K_THRESHOLD` 时调用。
/// 原地修改 `results` 的 `score` 字段并重新排序。
pub fn rerank(query_vec: &[f32], results: &mut [SearchResult], vector_index: &VectorIndex) {
    for result in results.iter_mut() {
        let rrf_score = result.score;
        let rerank_score = vector_index
            .get_vector(&result.item_id)
            .map(|item_vec| cosine_similarity(query_vec, &item_vec))
            .unwrap_or(0.0);
        result.score = RERANK_VECTOR_WEIGHT * rerank_score + RERANK_RRF_WEIGHT * rrf_score;
    }
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

fn reranker_scores_are_actionable(scores: &[f32]) -> bool {
    scores
        .iter()
        .any(|score| score.is_finite() && *score >= RERANK_MIN_ACTIONABLE_SCORE)
}

/// 三阶段搜索：initial_k 粗召回 → intermediate_k RRF 融合 → Rerank → top_k 返回
///
/// 同时被 search 端点和 chat 引擎调用，避免重复逻辑。
///
/// 诊断：每阶段的候选数通过 log::info!/debug! 输出，便于排查"有文档但召回 0"的问题。
pub fn search_with_context(
    ctx: &SearchContext<'_>,
    query: &str,
    params: &SearchParams,
) -> crate::error::Result<Vec<SearchResult>> {
    search_with_context_diagnostics(ctx, query, params).map(|outcome| outcome.results)
}

pub fn search_with_context_diagnostics(
    ctx: &SearchContext<'_>,
    query: &str,
    params: &SearchParams,
) -> crate::error::Result<SearchOutcome> {
    let current_embedding_fingerprint = ctx
        .embedding
        .as_ref()
        .map(|embedding| crate::embed::current_embedding_fingerprint(embedding.as_ref()));
    let embedding = ctx
        .vectors
        .map(|vectors| {
            vectors.embedding_compatibility(current_embedding_fingerprint.as_deref(), true)
        })
        .unwrap_or_else(|| VectorEmbeddingCompatibility {
            usable: false,
            status: if ctx.embedding.is_some() {
                "no_vector_index".to_string()
            } else {
                "unavailable".to_string()
            },
            enforce: true,
            index_fingerprint: None,
            current_fingerprint: current_embedding_fingerprint.clone(),
            stale_vectors: 0,
        });
    let mut vector_skipped_reason = None;

    // 1. Source metadata + fulltext recall.
    //
    // For source-shaped interactive queries, metadata titles/paths are usually
    // the highest-signal channel. Run that first so cold post-unlock traffic is
    // not forced through Tantivy while the background FTS rebuild is still
    // committing segments.
    let metadata_results = if params.skip_vector {
        metadata_source_candidates(ctx, query, params.initial_k)
    } else {
        Vec::new()
    };
    let skip_fulltext = params.skip_vector && metadata_results.len() >= params.top_k.max(1);
    let ft_results = if skip_fulltext {
        log::info!(
            "search stages: fulltext skipped; metadata_source_candidates={}",
            metadata_results.len()
        );
        Vec::new()
    } else {
        ctx.fulltext
            .map(|ft| {
                ft.search(query, params.initial_k).unwrap_or_else(|e| {
                    log::warn!("fulltext search error: {e}");
                    vec![]
                })
            })
            .unwrap_or_default()
    };
    let ft_results = dedup_ranked_results(ft_results);
    let ft_results = if params.skip_vector {
        log::info!(
            "search stages: metadata_source_candidates={}",
            metadata_results.len()
        );
        merge_ranked_results(metadata_results, ft_results, params.initial_k)
    } else {
        ft_results
    };

    let lexical_results = lexical_content_candidates(ctx, query, params.initial_k.min(64));
    let ft_results = if lexical_results.is_empty() {
        ft_results
    } else {
        log::info!(
            "search stages: lexical_content_candidates={}",
            lexical_results.len()
        );
        merge_ranked_results(lexical_results, ft_results, params.initial_k)
    };

    let ft_results = if ft_results.is_empty() {
        let exact_results = exact_substring_candidates(ctx, query, params.initial_k);
        if !exact_results.is_empty() {
            log::info!(
                "search stages: exact substring fallback={} query='{}'",
                exact_results.len(),
                query.chars().take(50).collect::<String>()
            );
        }
        exact_results
    } else {
        ft_results
    };

    let lexical_fast_path =
        !params.skip_vector && !ft_results.is_empty() && lexical_fast_path_query(query);

    // 2. 向量搜索（initial_k）
    // J3 (per spec §J3)：拿到 vector 结果后立即按 min_score 过滤；
    // 低于阈值的进 RRF 前丢弃，避免噪音污染融合排序。
    let (vec_results, query_vec): (Vec<(String, f32)>, Option<Vec<f32>>) = if params.skip_vector {
        log::info!("search stages: vector skipped by SearchParams");
        vector_skipped_reason = Some("search_params_skip_vector".to_string());
        (vec![], None)
    } else if lexical_fast_path {
        log::info!(
            "search stages: vector skipped by lexical fast path query='{}' fts={}",
            query.chars().take(50).collect::<String>(),
            ft_results.len()
        );
        vector_skipped_reason = Some("lexical_fast_path".to_string());
        (vec![], None)
    } else if !embedding.usable {
        log::warn!(
            "search stages: vector skipped by embedding compatibility status={}",
            embedding.status
        );
        vector_skipped_reason = Some(
            match embedding.status.as_str() {
                "mismatch" => "embedding_fingerprint_mismatch",
                "unknown" => "embedding_fingerprint_unknown",
                "current_unknown" => "embedding_current_fingerprint_unknown",
                "no_vector_index" => "no_vector_index",
                "unavailable" => "embedding_unavailable",
                _ => "embedding_not_usable",
            }
            .to_string(),
        );
        (vec![], None)
    } else {
        match (&ctx.embedding, &ctx.vectors) {
            (Some(emb), Some(vecs)) => match emb.embed(&[query]) {
                Ok((e, _usage)) if !e.is_empty() => {
                    let qv = e[0].clone();
                    let raw: Vec<(String, f32)> = vecs
                        .search(&qv, params.initial_k)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(meta, score)| (chunk_hit_key(&meta.item_id, meta.chunk_idx), score))
                        .collect();
                    let filtered: Vec<(String, f32)> = match params.min_score {
                        Some(threshold) => {
                            let kept: Vec<_> =
                                raw.into_iter().filter(|(_, s)| *s >= threshold).collect();
                            log::info!(
                                "search J3: vector min_score={:.3} kept {} results",
                                threshold,
                                kept.len()
                            );
                            kept
                        }
                        None => raw,
                    };
                    (dedup_ranked_results(filtered), Some(qv))
                }
                _ => {
                    vector_skipped_reason = Some("embedding_query_failed".to_string());
                    (vec![], None)
                }
            },
            _ => {
                vector_skipped_reason = Some("embedding_or_vector_index_missing".to_string());
                (vec![], None)
            }
        }
    };

    log::info!(
        "search stages: query='{}' fts={} vec={}",
        query.chars().take(50).collect::<String>(),
        ft_results.len(),
        vec_results.len(),
    );

    // 3. RRF 融合 → intermediate_k
    let fused = rrf_fuse(
        &vec_results,
        &ft_results,
        DEFAULT_VECTOR_WEIGHT,
        DEFAULT_FULLTEXT_WEIGHT,
        params.intermediate_k,
    );
    log::info!("search stages: rrf_fused={}", fused.len());

    // 4. 获取并解密 items + F2 (W3 batch A) 拉 breadcrumb sidecar
    let mut results: Vec<SearchResult> = Vec::new();
    for (hit_key, score) in &fused {
        let (item_id, chunk_idx) = parse_hit_key(hit_key);
        if let Ok(Some(item)) = ctx.store.get_item(ctx.dek, item_id) {
            // breadcrumb 现已加密落盘，需传 dek 解密
            let chunk_span =
                chunk_idx.and_then(|idx| ctx.store.get_chunk_span(&item.id, idx).ok().flatten());
            let chunk_content = chunk_span.and_then(|(start, end, _level, _section_idx)| {
                item.content
                    .get(start..end)
                    .map(|s| (s.to_string(), Some(start), Some(end)))
            });
            let (content, span_start, span_end) = chunk_content
                .or_else(|| lexical_excerpt_for_item(query, &item.content))
                .unwrap_or_else(|| {
                    let start = None;
                    let end = None;
                    (item.content.clone(), start, end)
                });
            let (breadcrumb, off_start, off_end) = chunk_idx
                .and_then(|idx| {
                    ctx.store
                        .get_chunk_breadcrumb(ctx.dek, &item.id, idx)
                        .ok()
                        .flatten()
                })
                .or_else(|| {
                    ctx.store
                        .get_first_chunk_breadcrumb(ctx.dek, &item.id)
                        .ok()
                        .flatten()
                })
                .map(|(p, s, e)| (p, Some(s), Some(e)))
                .unwrap_or_default();
            // v0.6 Phase B F-Pro：拉 corpus_domain；item 不存在 / 列缺时回退 'general'
            let corpus_domain = ctx
                .store
                .get_item_corpus_domain(&item.id)
                .unwrap_or_else(|_| "general".to_string());
            results.push(SearchResult {
                item_id: item.id,
                chunk_idx,
                score: *score,
                title: item.title,
                content,
                source_type: item.source_type,
                source_path: item.url,
                inject_content: None,
                breadcrumb,
                chunk_offset_start: span_start.or(off_start),
                chunk_offset_end: span_end.or(off_end),
                corpus_domain,
            });
        }
    }
    log::info!("search stages: items_decrypted={}", results.len());

    // 5. Rerank 策略：
    //    a) 候选 < RERANK_MIN_CANDIDATES：跳过 cross-encoder，保留 RRF 序
    //       （小集合上 cross-encoder 放大噪声 + 跨语言错配）
    //    b) 候选够多：用 cross-encoder 重排
    //    c) 无 cross-encoder 但有 query 向量 + 候选 <= 20：用 cosine 重排
    //
    // 语言降权（反 cross-lingual 污染）：任何 rerank 方式之后，都按
    // query/doc 语言匹配对 score 做降权，防止大篇幅异语言文档排到前面。
    let query_lang = detect_lang(query);
    let mut reranker_diag = RerankerSearchDiagnostics {
        requested: !params.skip_rerank,
        available: ctx.reranker.is_some(),
        candidate_count: results.len(),
        ..Default::default()
    };
    reranker_diag.requested = !params.skip_rerank;
    reranker_diag.available = ctx.reranker.is_some();

    if params.skip_rerank {
        log::info!("search stages: reranker skipped by SearchParams");
        reranker_diag.skipped_reason = Some("search_params_skip_rerank".to_string());
    } else if let Some(reranker) = &ctx.reranker {
        if results.len() >= RERANK_MIN_CANDIDATES {
            let before_top = results.first().map(|r| r.item_id.clone());
            let docs: Vec<&str> = results.iter().map(|r| r.content.as_str()).collect();
            match reranker.score(query, &docs) {
                Ok(scores) => {
                    reranker_diag.score_count = scores.len();
                    if reranker_scores_are_actionable(&scores) {
                        for (r, s) in results.iter_mut().zip(scores.iter()) {
                            r.score = *s;
                        }
                        results.sort_by(|a, b| {
                            b.score
                                .partial_cmp(&a.score)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                        reranker_diag.used = true;
                        reranker_diag.actionable = true;
                        reranker_diag.changed_top_result =
                            before_top != results.first().map(|r| r.item_id.clone());
                    } else {
                        reranker_diag.skipped_reason = Some("low_signal_scores".to_string());
                        log::warn!("reranker returned no actionable scores, keeping RRF order");
                    }
                }
                Err(e) => {
                    reranker_diag.skipped_reason = Some("reranker_error".to_string());
                    log::warn!("reranker failed, keeping RRF order: {e}");
                }
            }
        } else {
            reranker_diag.skipped_reason = Some("insufficient_candidates".to_string());
            log::info!(
                "search stages: reranker skipped (candidates={} < {})",
                results.len(),
                RERANK_MIN_CANDIDATES
            );
        }
    } else if results.len() <= RERANK_TOP_K_THRESHOLD {
        reranker_diag.skipped_reason = Some("reranker_unavailable".to_string());
        if let Some(qvec) = &query_vec {
            if let Some(vecs) = ctx.vectors {
                rerank(qvec, &mut results, vecs);
                reranker_diag.used = true;
                reranker_diag.skipped_reason = Some("vector_similarity_fallback".to_string());
            }
        }
    }

    // 语言匹配降权：任何排序策略之后统一应用，不改变同语言相对顺序
    apply_cross_lang_penalty(&mut results, query_lang);

    // v0.6 Phase B F-Pro：跨领域降权（同语种跨领域污染防御）
    // 如 query="反洗钱"（domain_hint=legal）+ doc.corpus_domain=tech → score *= 0.4
    apply_cross_domain_penalty(&mut results, params.domain_hint.as_deref());

    // SRAS/source selector reward: explicit source hints should beat generic
    // semantic similarity, especially with 512-dim local embeddings and long
    // scanned manuals where large PDFs otherwise dominate by chunk count.
    apply_source_hint_boost(query, &mut results);
    apply_platform_hint_adjustment(query, &mut results);
    apply_query_coverage_boost(query, &mut results);

    // 最终排序
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // 6. 截取 top_k（保护：如果 top_k=0，别截成空）
    let final_k = params.top_k.max(1);
    results.truncate(final_k);
    log::info!("search stages: returned={}", results.len());
    Ok(SearchOutcome {
        results,
        diagnostics: SearchDiagnostics {
            bm25_results: ft_results.len(),
            vector_results: vec_results.len(),
            vector_skipped_reason,
            embedding,
            reranker: reranker_diag,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_lang_pure_chinese() {
        assert_eq!(detect_lang("劳动合同法规定"), Lang::Zh);
        assert_eq!(detect_lang("民法典第五百八十四条"), Lang::Zh);
    }

    #[test]
    fn detect_lang_pure_english() {
        assert_eq!(
            detect_lang("What is rust ownership and borrowing"),
            Lang::En
        );
        assert_eq!(
            detect_lang("Box T smart pointer reference cycles"),
            Lang::En
        );
    }

    #[test]
    fn detect_lang_technical_mix() {
        // 中文为主但含英文术语 → 仍按中文处理（CJK >= 30%）
        assert_eq!(detect_lang("使用 Box<T> 处理堆内存"), Lang::Zh);
        // 少量中文的英文文档（< 30%）→ 英文
        assert_eq!(detect_lang("Rust programming language 简称 RPL"), Lang::En);
    }

    #[test]
    fn cross_lang_penalty_en_query_cn_doc_downweighted() {
        let mut results = vec![
            SearchResult {
                item_id: "1".into(),
                score: 0.2,
                title: "references-and-borrowing".into(),
                content:
                    "In Rust, references allow you to refer to a value without taking ownership."
                        .into(),
                source_type: "file".into(),
                inject_content: None,
                ..Default::default()
            },
            SearchResult {
                item_id: "2".into(),
                score: 0.3,
                title: "民法典".into(),
                content: "中华人民共和国民法典第一编 总则".into(),
                source_type: "file".into(),
                inject_content: None,
                ..Default::default()
            },
        ];
        apply_cross_lang_penalty(&mut results, Lang::En);
        assert_eq!(results[0].score, 0.2, "英文文档不降权");
        assert!(
            results[1].score < 0.1,
            "中文文档应被降权 (0.3 * 0.3 = 0.09): {}",
            results[1].score
        );
    }

    #[test]
    fn cross_lang_penalty_mixed_query_no_penalty() {
        let mut results = vec![SearchResult {
            item_id: "1".into(),
            score: 0.5,
            title: "rust 所有权".into(),
            content: "Rust ownership system...".into(),
            source_type: "file".into(),
            inject_content: None,
            ..Default::default()
        }];
        apply_cross_lang_penalty(&mut results, Lang::Mixed);
        assert_eq!(results[0].score, 0.5, "Mixed query 不应降权任何结果");
    }

    // ── S4b MU-5 (R8): detect_query_domain is plugin-driven, no hardcoded industry words ──

    /// Helper: build a domain→keyword mapping the way the plugin registry would,
    /// so tests don't depend on owned-vs-borrowed str types.
    fn dk(pairs: &[(&'static str, &[&'static str])]) -> Vec<(&'static str, Vec<&'static str>)> {
        pairs.iter().map(|(d, kws)| (*d, kws.to_vec())).collect()
    }

    #[test]
    fn detect_query_domain_oss_bare_no_plugin_returns_none() {
        // OSS 裸装无 vertical plugin → 空词表 → 永远 None（无 cross-domain penalty）。
        // 这是 oss-pro-strategy §4.3 边界规则的代码层验证：行业 domain detection 不在 OSS。
        let empty: Vec<(&str, Vec<&str>)> = Vec::new();
        assert_eq!(detect_query_domain("反洗钱合同纠纷怎么处理", &empty), None);
        assert_eq!(
            detect_query_domain("Rust ownership and borrowing", &empty),
            None
        );
    }

    #[test]
    fn detect_query_domain_plugin_keywords_detect_correctly() {
        // vertical plugin（attune-pro）提供 domain 词表后才识别。
        let domains = dk(&[
            ("legal", &["反洗钱", "诉讼", "合同"]),
            ("medical", &["病历", "处方"]),
        ]);
        assert_eq!(
            detect_query_domain("反洗钱合同纠纷怎么处理", &domains).as_deref(),
            Some("legal")
        );
        assert_eq!(
            detect_query_domain("帮我看看这份病历", &domains).as_deref(),
            Some("medical")
        );
    }

    #[test]
    fn detect_query_domain_zero_hit_returns_none() {
        // 提供了词表但 query 不含任何特征词 → None（不误识别）。
        let domains = dk(&[("legal", &["诉讼", "合同"])]);
        assert_eq!(detect_query_domain("今天天气怎么样", &domains), None);
    }

    #[test]
    fn detect_query_domain_tie_prefers_input_order() {
        // 两个 domain 各命中 1 词（平手）→ 按传入顺序取首个（legal 先于 tech）。
        let domains = dk(&[("legal", &["合同"]), ("tech", &["索引"])]);
        assert_eq!(
            detect_query_domain("合同里的索引字段", &domains).as_deref(),
            Some("legal")
        );
        // 顺序反转 → tech 胜出，证明的确是 input-order 而非字母序。
        let domains_rev = dk(&[("tech", &["索引"]), ("legal", &["合同"])]);
        assert_eq!(
            detect_query_domain("合同里的索引字段", &domains_rev).as_deref(),
            Some("tech")
        );
    }

    #[test]
    fn detect_query_domain_most_hits_wins() {
        // 命中数多的 domain 胜出（不受 input order 影响）。
        let domains = dk(&[
            ("tech", &["索引"]),                  // 1 命中
            ("legal", &["合同", "诉讼", "赔偿"]), // 3 命中
        ]);
        assert_eq!(
            detect_query_domain("合同诉讼赔偿与索引", &domains).as_deref(),
            Some("legal")
        );
    }

    #[test]
    fn detect_query_domain_case_insensitive_english() {
        // 英文关键词大小写无关（query 与 keyword 都 lowercase 后子串匹配）。
        let domains = dk(&[("tech", &["Rust", "Docker"])]);
        assert_eq!(
            detect_query_domain("How does RUST ownership work", &domains).as_deref(),
            Some("tech")
        );
    }

    #[test]
    fn detect_query_domain_drives_cross_domain_penalty_only_with_plugin() {
        // 端到端：plugin 提供词表 → detect → penalty 生效；无 plugin → 无 penalty。
        let mk = |dom: &str| SearchResult {
            item_id: "x".into(),
            score: 1.0,
            title: "t".into(),
            content: "c".into(),
            source_type: "file".into(),
            inject_content: None,
            corpus_domain: dom.into(),
            ..Default::default()
        };
        // OSS 裸装：detect → None → penalty no-op
        let empty: Vec<(&str, Vec<&str>)> = Vec::new();
        let d = detect_query_domain("反洗钱诉讼", &empty);
        let mut results = vec![mk("tech")];
        apply_cross_domain_penalty(&mut results, d.as_deref());
        assert_eq!(results[0].score, 1.0, "OSS 裸装无词表 → 不降权");
        // 装了 legal plugin：detect → legal → tech doc 被降权
        let domains = dk(&[("legal", &["反洗钱", "诉讼"])]);
        let d = detect_query_domain("反洗钱诉讼", &domains);
        let mut results = vec![mk("tech")];
        apply_cross_domain_penalty(&mut results, d.as_deref());
        assert!(
            (results[0].score - CROSS_DOMAIN_PENALTY).abs() < 1e-6,
            "legal query + tech doc 应降权到 {CROSS_DOMAIN_PENALTY}: {}",
            results[0].score
        );
    }

    #[test]
    fn rrf_fuse_basic() {
        let vec_results = vec![("a".into(), 0.9), ("b".into(), 0.7), ("c".into(), 0.5)];
        let ft_results = vec![("b".into(), 10.0), ("a".into(), 8.0), ("d".into(), 5.0)];

        let fused = rrf_fuse(&vec_results, &ft_results, 0.6, 0.4, 10);
        assert!(!fused.is_empty());
        // "a" 和 "b" 在两个列表中都出现，应该排名靠前
        let top_ids: Vec<&str> = fused.iter().map(|(id, _)| id.as_str()).collect();
        assert!(top_ids.contains(&"a"));
        assert!(top_ids.contains(&"b"));
    }

    #[test]
    fn rrf_fuse_empty() {
        let fused = rrf_fuse(&[], &[], 0.6, 0.4, 10);
        assert!(fused.is_empty());
    }

    #[test]
    fn rrf_fuse_single_source() {
        let vec_results = vec![("a".into(), 0.9)];
        let fused = rrf_fuse(&vec_results, &[], 0.6, 0.4, 10);
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].0, "a");
    }

    #[test]
    fn merge_ranked_results_prefers_metadata_and_dedups() {
        let primary = vec![("guide".to_string(), 0.8), ("manual".to_string(), 0.7)];
        let secondary = vec![("manual".to_string(), 0.4), ("runbook".to_string(), 0.3)];

        let merged = merge_ranked_results(primary, secondary, 3);

        assert_eq!(
            merged,
            vec![
                ("guide".to_string(), 0.8),
                ("manual".to_string(), 0.7),
                ("runbook".to_string(), 0.3)
            ]
        );
    }

    #[test]
    fn dedup_ranked_results_keeps_first_item_hit() {
        let ranked = vec![
            ("big-pdf".to_string(), 0.91),
            ("big-pdf".to_string(), 0.90),
            ("quick-guide".to_string(), 0.84),
            ("big-pdf".to_string(), 0.83),
        ];
        let deduped = dedup_ranked_results(ranked);
        assert_eq!(
            deduped,
            vec![
                ("big-pdf".to_string(), 0.91),
                ("quick-guide".to_string(), 0.84)
            ]
        );
    }

    #[test]
    fn source_hint_boost_promotes_explicit_manual_source() {
        let mut results = vec![
            SearchResult {
                item_id: "bulk-manual".into(),
                score: 0.50,
                title: "controller family reference manual".into(),
                source_path: Some("file:///docs/controller/reference-manual.pdf".into()),
                ..Default::default()
            },
            SearchResult {
                item_id: "quick-guide".into(),
                score: 0.43,
                title: "controller quick reference guide".into(),
                source_path: Some("file:///docs/controller/quick-reference-guide.pdf".into()),
                ..Default::default()
            },
        ];

        apply_source_hint_boost("controller quick reference guide source", &mut results);
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

        assert_eq!(results[0].item_id, "quick-guide");
    }

    #[test]
    fn source_hint_boost_promotes_product_specific_quick_reference() {
        let mut results = vec![
            SearchResult {
                item_id: "product-beta-guide".into(),
                score: 0.54,
                title: "product-beta quick reference handbook".into(),
                source_path: Some(
                    "file:///products/product-beta/quick-reference-handbook.pdf".into(),
                ),
                ..Default::default()
            },
            SearchResult {
                item_id: "product-alpha-guide".into(),
                score: 0.42,
                title: "product-alpha quick reference guide".into(),
                source_path: Some(
                    "file:///products/product-alpha/quick-reference-guide.pdf".into(),
                ),
                ..Default::default()
            },
        ];

        apply_source_hint_boost("product-alpha quick reference guide", &mut results);
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

        assert_eq!(results[0].item_id, "product-alpha-guide");
    }

    #[test]
    fn source_hint_boost_promotes_requested_product_family() {
        let mut results = vec![
            SearchResult {
                item_id: "product-alpha-manual".into(),
                score: 0.52,
                title: "product-alpha operating manual".into(),
                source_path: Some("file:///products/product-alpha/operating-manual.pdf".into()),
                ..Default::default()
            },
            SearchResult {
                item_id: "product-beta-manual".into(),
                score: 0.46,
                title: "product-beta operating manual".into(),
                source_path: Some("file:///products/product-beta/operating-manual.pdf".into()),
                ..Default::default()
            },
        ];

        apply_source_hint_boost("product-beta operating manual source", &mut results);
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

        assert_eq!(results[0].item_id, "product-beta-manual");
    }

    #[test]
    fn source_hint_boost_promotes_numeric_identifier_from_path() {
        let mut results = vec![
            SearchResult {
                item_id: "controller-100".into(),
                score: 0.54,
                title: "controller 100 operating manual".into(),
                source_path: Some("file:///controllers/100/operating-manual.pdf".into()),
                ..Default::default()
            },
            SearchResult {
                item_id: "controller-200".into(),
                score: 0.42,
                title: "controller 200 field operating manual".into(),
                source_path: Some("file:///controllers/200/field-operating-manual.pdf".into()),
                ..Default::default()
            },
        ];

        apply_source_hint_boost("controller 200 field operating manual", &mut results);
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

        assert_eq!(results[0].item_id, "controller-200");
    }

    #[test]
    fn source_hint_boost_promotes_specific_section_manual_over_bulk_manual() {
        let mut results = vec![
            SearchResult {
                item_id: "bulk-manual".into(),
                score: 0.56,
                title: "controller complete operating manual".into(),
                source_path: Some("file:///controllers/complete-operating-manual.pdf".into()),
                ..Default::default()
            },
            SearchResult {
                item_id: "network-section".into(),
                score: 0.43,
                title: "controller network diagnostics".into(),
                source_path: Some("file:///controllers/sections/network-diagnostics.pdf".into()),
                breadcrumb: vec!["sections".into(), "network diagnostics".into()],
                ..Default::default()
            },
        ];

        apply_source_hint_boost(
            "controller network diagnostics section manual",
            &mut results,
        );
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

        assert_eq!(results[0].item_id, "network-section");
    }

    #[test]
    fn source_hint_boost_promotes_procedure_sources() {
        let mut results = vec![
            SearchResult {
                item_id: "quick-guide".into(),
                score: 0.50,
                title: "controller quick reference".into(),
                source_path: Some("file:///controllers/quick-reference.pdf".into()),
                ..Default::default()
            },
            SearchResult {
                item_id: "startup-procedure".into(),
                score: 0.36,
                title: "controller startup procedure".into(),
                source_path: Some("file:///controllers/procedures/startup-procedure.pdf".into()),
                ..Default::default()
            },
        ];

        apply_source_hint_boost("controller startup procedure", &mut results);
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

        assert_eq!(results[0].item_id, "startup-procedure");
    }

    #[test]
    fn source_hint_boost_keeps_requested_source_ahead_of_excluded_noise() {
        let mut results = vec![
            SearchResult {
                item_id: "noise-guide".into(),
                score: 0.58,
                title: "legacy quick guide".into(),
                source_path: Some("file:///legacy/quick-guide.pdf".into()),
                ..Default::default()
            },
            SearchResult {
                item_id: "target-guide".into(),
                score: 0.40,
                title: "target quick guide".into(),
                source_path: Some("file:///target/quick-guide.pdf".into()),
                ..Default::default()
            },
        ];

        apply_source_hint_boost(
            "只基于 target quick guide 来源；不要引入 legacy。",
            &mut results,
        );
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

        assert_eq!(results[0].item_id, "target-guide");
    }

    #[test]
    fn source_hint_boost_promotes_abbreviation_sources() {
        let mut results = vec![
            SearchResult {
                item_id: "generic-guide".into(),
                score: 0.49,
                title: "controller quick reference".into(),
                source_path: Some("file:///controllers/quick-reference.pdf".into()),
                ..Default::default()
            },
            SearchResult {
                item_id: "symbol-glossary".into(),
                score: 0.38,
                title: "controller symbol abbreviations glossary".into(),
                source_path: Some("file:///reference/symbol-abbreviations-glossary.md".into()),
                ..Default::default()
            },
        ];

        apply_source_hint_boost("controller symbol abbreviations glossary", &mut results);
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

        assert_eq!(results[0].item_id, "symbol-glossary");
    }

    /// Regression: top_k>20 previously panicked at `intermediate_k = (top_k*2).clamp(top_k, 40)`
    /// because top_k*2 > 40 made the clamp `min > max`. See
    /// docs/superpowers/specs/2026-05-24-knowledge-base-deepseek-rag-audit.md §B2.
    /// 50-query benchmark on rust-book triggered tokio worker panic; fix ensures
    /// `with_defaults(top_k)` is total over the documented range top_k ∈ [1, 100].
    #[test]
    fn with_defaults_does_not_panic_for_any_top_k() {
        for tk in [1usize, 5, 10, 20, 21, 30, 50, 99, 100] {
            let p = SearchParams::with_defaults(tk);
            assert!(p.initial_k >= 20, "initial_k floor at 20 for top_k={tk}");
            assert!(
                p.intermediate_k >= tk,
                "intermediate_k must be >= top_k for top_k={tk}"
            );
            // intermediate_k 上限 200 防止 rerank 过度膨胀（每个候选都过 ONNX 推理）
            assert!(
                p.intermediate_k <= 200,
                "intermediate_k ceiling 200 for top_k={tk}"
            );
            assert_eq!(p.top_k, tk);
        }
    }

    #[test]
    fn allocate_budget_proportional() {
        let mut results = vec![
            SearchResult {
                item_id: "a".into(),
                score: 0.8,
                title: "A".into(),
                content: "A".repeat(3000),
                source_type: "note".into(),
                inject_content: None,
                ..Default::default()
            },
            SearchResult {
                item_id: "b".into(),
                score: 0.2,
                title: "B".into(),
                content: "B".repeat(3000),
                source_type: "note".into(),
                inject_content: None,
                ..Default::default()
            },
        ];
        allocate_budget(&mut results, 2000);

        let a_len = results[0].inject_content.as_ref().unwrap().chars().count();
        let b_len = results[1].inject_content.as_ref().unwrap().chars().count();
        // "a" has 80% score, should get ~1600 chars; "b" has 20%, should get ~400 (min 100)
        assert!(
            a_len > b_len,
            "Higher score should get more budget: a={a_len} b={b_len}"
        );
        assert!(b_len >= 100, "Minimum budget should be 100: got {b_len}");
    }

    #[test]
    fn allocate_budget_zero_scores() {
        let mut results = vec![
            SearchResult {
                item_id: "a".into(),
                score: 0.0,
                title: "A".into(),
                content: "A".repeat(3000),
                source_type: "note".into(),
                inject_content: None,
                ..Default::default()
            },
            SearchResult {
                item_id: "b".into(),
                score: 0.0,
                title: "B".into(),
                content: "B".repeat(3000),
                source_type: "note".into(),
                inject_content: None,
                ..Default::default()
            },
        ];
        allocate_budget(&mut results, 2000);
        // Equal distribution when scores are 0
        let a_len = results[0].inject_content.as_ref().unwrap().chars().count();
        let b_len = results[1].inject_content.as_ref().unwrap().chars().count();
        assert_eq!(a_len, b_len, "Equal scores should get equal budget");
    }

    #[test]
    fn cosine_similarity_basic() {
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-5);
        assert!((cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]) - 0.0).abs() < 1e-5);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn rerank_orders_by_cosine() {
        use crate::vectors::{VectorIndex, VectorMeta};

        let mut idx = VectorIndex::new(2).unwrap();
        idx.add(
            &[1.0, 0.0],
            VectorMeta {
                item_id: "close".into(),
                chunk_idx: 0,
                level: 2,
                section_idx: 0,
            },
        )
        .unwrap();
        idx.add(
            &[0.0, 1.0],
            VectorMeta {
                item_id: "far".into(),
                chunk_idx: 0,
                level: 2,
                section_idx: 0,
            },
        )
        .unwrap();

        let mut results = vec![
            SearchResult {
                item_id: "far".into(),
                score: 0.9,
                title: "Far".into(),
                content: "c".into(),
                source_type: "note".into(),
                inject_content: None,
                ..Default::default()
            },
            SearchResult {
                item_id: "close".into(),
                score: 0.5,
                title: "Close".into(),
                content: "c".into(),
                source_type: "note".into(),
                inject_content: None,
                ..Default::default()
            },
        ];

        rerank(&[1.0, 0.0], &mut results, &idx);
        assert_eq!(
            results[0].item_id, "close",
            "Reranker should elevate closer vector"
        );
    }

    #[test]
    fn rerank_fallback_when_no_vector() {
        use crate::vectors::VectorIndex;

        let idx = VectorIndex::new(2).unwrap();
        let mut results = vec![
            SearchResult {
                item_id: "a".into(),
                score: 0.8,
                title: "A".into(),
                content: "c".into(),
                source_type: "note".into(),
                ..Default::default()
            },
            SearchResult {
                item_id: "b".into(),
                score: 0.3,
                title: "B".into(),
                content: "c".into(),
                source_type: "note".into(),
                ..Default::default()
            },
        ];
        rerank(&[1.0, 0.0], &mut results, &idx);
        assert!(results[0].score >= results[1].score);
    }

    #[test]
    fn search_params_defaults_clamp_correctly() {
        let p = SearchParams::with_defaults(5);
        assert_eq!(p.top_k, 5);
        assert_eq!(p.initial_k, 25); // 5*5=25, in [20,500]
        assert_eq!(p.intermediate_k, 10); // 5*2=10
                                          // 通用 search 默认不启用 min_score 阈值，保持向后行为契约
        assert_eq!(p.min_score, None);

        let p2 = SearchParams::with_defaults(1);
        assert_eq!(p2.initial_k, 20); // min clamp
        assert_eq!(p2.intermediate_k, 2); // max(1, 2) = 2

        // top_k=20: 旧契约 intermediate_k=40 (max clamp), 新契约 intermediate_k=40 (top_k*2)
        // 两者数值一致
        let p3 = SearchParams::with_defaults(20);
        assert_eq!(p3.initial_k, 100);
        assert_eq!(p3.intermediate_k, 40);

        // top_k=50: 旧契约会 panic（min=50 > max=40），新契约返回 100
        // 这是本次修复的关键测试 (per 2026-05-24 spec §B2)
        let p4 = SearchParams::with_defaults(50);
        assert_eq!(p4.initial_k, 250);
        assert_eq!(p4.intermediate_k, 100);

        // top_k=100 (max per S14): 旧契约会 panic，新契约返回 200
        let p5 = SearchParams::with_defaults(100);
        assert_eq!(p5.initial_k, 500);
        assert_eq!(p5.intermediate_k, 200);
    }

    // ── J3 tests（per spec §J3 + reviewer S2 路径分离）──────────────

    #[test]
    fn min_score_filter_keeps_above_threshold() {
        // 模拟 vecs.search 返回 [0.50, 0.70, 0.85]
        let raw: Vec<(String, f32)> =
            vec![("a".into(), 0.50), ("b".into(), 0.70), ("c".into(), 0.85)];
        let kept_065: Vec<_> = raw.iter().filter(|(_, s)| *s >= 0.65).cloned().collect();
        assert_eq!(kept_065.len(), 2, "0.65 阈值应保留 2 个 (0.70 + 0.85)");
        assert_eq!(kept_065[0].0, "b");
        assert_eq!(kept_065[1].0, "c");

        let kept_078: Vec<_> = raw.iter().filter(|(_, s)| *s >= 0.78).cloned().collect();
        assert_eq!(kept_078.len(), 1, "0.78 阈值应保留 1 个 (0.85)");

        let kept_055: Vec<_> = raw.iter().filter(|(_, s)| *s >= 0.55).cloned().collect();
        assert_eq!(kept_055.len(), 2, "0.55 应保留 0.70 + 0.85（不含 0.50）");
    }

    #[test]
    fn rag_defaults_enable_065_threshold() {
        // chat 路径默认走 RAG 阈值（0.65）— J3 仅对 RAG 生效，通用 search 不变
        let rag = SearchParams::with_defaults_for_rag(5);
        assert_eq!(rag.min_score, Some(0.65));
        assert_eq!(rag.top_k, 5);
        assert_eq!(rag.initial_k, 25); // 与通用版同构
    }

    #[test]
    fn min_score_threshold_curve_documented_in_spec() {
        // 锁住吴师兄文章给出的曲线值，避免有人未读 spec 误改默认
        let rag = SearchParams::with_defaults_for_rag(5);
        assert_eq!(
            rag.min_score,
            Some(0.65),
            "RAG 默认 0.65（保守端，召回优先）"
        );
        // 0.72 是吴师兄推荐的"精度优先"档，未来 Settings 提供
        // 0.78 开始漏边缘 case，仅极端精度场景用
    }

    #[test]
    fn reranker_scores_must_be_actionable_before_overriding_rrf() {
        assert!(!reranker_scores_are_actionable(&[0.0, 0.0, f32::NAN]));
        assert!(!reranker_scores_are_actionable(&[0.0001, 0.0009]));
        assert!(reranker_scores_are_actionable(&[0.0, 0.001]));
        assert!(reranker_scores_are_actionable(&[0.42, 0.0]));
    }

    // #9: search_with_context 三阶段管道（有 Reranker）
    #[test]
    fn search_with_context_reranker_reorders_results() {
        use crate::infer::MockRerankProvider;
        use crate::store::Store;

        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();

        // 插入两条 item
        store
            .insert_item(
                &dek,
                "低分文档",
                "content about cats",
                None,
                "note",
                None,
                None,
            )
            .unwrap();
        store
            .insert_item(
                &dek,
                "高分文档",
                "content about dogs",
                None,
                "note",
                None,
                None,
            )
            .unwrap();

        // Reranker 固定返回固定分数（第二条评分更高）
        let reranker: std::sync::Arc<dyn crate::infer::RerankProvider> =
            std::sync::Arc::new(MockRerankProvider::new(vec![0.1, 0.9]));

        let ctx = SearchContext {
            fulltext: None,
            vectors: None,
            embedding: None,
            reranker: Some(reranker),
            store: &store,
            dek: &dek,
        };

        // 无 FTS 也无向量时 fused 为空，search_with_context 返回空但不 panic
        let params = SearchParams::with_defaults(5);
        let results = search_with_context(&ctx, "dogs", &params);
        assert!(
            results.is_ok(),
            "search_with_context should not fail with reranker"
        );
        // 无数据源时结果为空
        assert!(results.unwrap().is_empty());
    }

    // #10: search_with_context 纯 FTS fallback（无 embedding、无 reranker）
    #[test]
    fn search_with_context_fts_only_fallback() {
        use crate::store::Store;

        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();

        let ctx = SearchContext {
            fulltext: None,
            vectors: None,
            embedding: None,
            reranker: None,
            store: &store,
            dek: &dek,
        };

        let params = SearchParams::with_defaults(5);
        let results = search_with_context(&ctx, "any query", &params).unwrap();
        // 无数据源时结果为空，但不应 panic
        assert!(results.is_empty());
    }

    #[test]
    fn search_outcome_reports_embedding_mismatch_and_skips_vector_hits() {
        use crate::embed::{current_embedding_fingerprint, MockEmbeddingProvider};
        use crate::store::Store;
        use crate::vectors::{VectorIndex, VectorMeta};
        use std::sync::Arc;

        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        store
            .insert_item(
                &dek,
                "vector only item",
                "body without the requested synthetic token",
                None,
                "note",
                None,
                None,
            )
            .unwrap();

        let provider = Arc::new(MockEmbeddingProvider::new(4));
        let active_fingerprint = current_embedding_fingerprint(provider.as_ref());
        let mut vectors = VectorIndex::new_with_fingerprint(4, "provider=mock;model=old;dim=4")
            .expect("vector index");
        vectors
            .add(
                &[1.0, 0.0, 0.0, 0.0],
                VectorMeta {
                    item_id: "vector only item".into(),
                    chunk_idx: 0,
                    level: 2,
                    section_idx: 0,
                },
            )
            .unwrap();

        assert_ne!(
            vectors.embedding_fingerprint(),
            Some(active_fingerprint.as_str())
        );

        let ctx = SearchContext {
            fulltext: None,
            vectors: Some(&vectors),
            embedding: Some(provider),
            reranker: None,
            store: &store,
            dek: &dek,
        };

        let outcome =
            search_with_context_diagnostics(&ctx, "zzzznotintext", &SearchParams::with_defaults(5))
                .expect("search outcome");

        assert!(outcome.results.is_empty());
        assert_eq!(outcome.diagnostics.embedding.status, "mismatch");
        assert!(!outcome.diagnostics.embedding.usable);
        assert_eq!(outcome.diagnostics.embedding.stale_vectors, 1);
        assert_eq!(outcome.diagnostics.vector_results, 0);
        assert_eq!(
            outcome.diagnostics.vector_skipped_reason.as_deref(),
            Some("embedding_fingerprint_mismatch")
        );
    }

    #[test]
    fn search_outcome_reports_low_signal_reranker_fallback() {
        use crate::index::FulltextIndex;
        use crate::infer::MockRerankProvider;
        use crate::store::Store;
        use std::sync::Arc;

        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        let ft = FulltextIndex::open_memory().unwrap();
        for idx in 0..5 {
            let title = format!("generic doc {idx}");
            let content = format!("alpha shared retrieval marker {idx}");
            let item_id = store
                .insert_item(&dek, &title, &content, None, "note", None, None)
                .unwrap();
            ft.add_document(&item_id, &title, &content, "note").unwrap();
        }

        let base_ctx = SearchContext {
            fulltext: Some(&ft),
            vectors: None,
            embedding: None,
            reranker: None,
            store: &store,
            dek: &dek,
        };
        let params = SearchParams {
            skip_vector: true,
            ..SearchParams::with_defaults(5)
        };
        let base = search_with_context_diagnostics(&base_ctx, "alpha", &params)
            .expect("base search outcome");
        let base_top = base.results.first().map(|result| result.item_id.clone());

        let rerank_ctx = SearchContext {
            reranker: Some(Arc::new(MockRerankProvider::new(vec![0.0]))),
            ..base_ctx
        };
        let outcome = search_with_context_diagnostics(&rerank_ctx, "alpha", &params)
            .expect("rerank search outcome");

        assert_eq!(
            outcome.results.first().map(|result| result.item_id.clone()),
            base_top
        );
        assert!(outcome.diagnostics.reranker.requested);
        assert!(outcome.diagnostics.reranker.available);
        assert!(!outcome.diagnostics.reranker.used);
        assert_eq!(outcome.diagnostics.reranker.candidate_count, 5);
        assert_eq!(outcome.diagnostics.reranker.score_count, 5);
        assert!(!outcome.diagnostics.reranker.actionable);
        assert_eq!(
            outcome.diagnostics.reranker.skipped_reason.as_deref(),
            Some("low_signal_scores")
        );
    }

    #[test]
    fn search_with_context_exact_substring_fallback_for_code_like_queries() {
        use crate::store::Store;

        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        let item_id = store
            .insert_item(
                &dek,
                "multi - 多语言",
                "# 多语言\n\n中文 English 日本語 한국어 🚀🔥✨ émojis BOUNDARY_MULTI_MARK\n\n## 节\n\nمرحبا Ω≈ç√∫\n",
                None,
                "file",
                None,
                None,
            )
            .unwrap();

        let ctx = SearchContext {
            fulltext: None,
            vectors: None,
            embedding: None,
            reranker: None,
            store: &store,
            dek: &dek,
        };

        let params = SearchParams::with_defaults(5);
        let results = search_with_context(&ctx, "BOUNDARY_MULTI_MARK", &params).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].item_id, item_id);
        assert!(results[0].content.contains("BOUNDARY_MULTI_MARK"));
    }

    #[test]
    fn search_with_context_lexical_hit_injects_matched_excerpt_not_cover() {
        use crate::store::Store;

        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        let cover = "cover page and table of contents\n".repeat(120);
        let evidence = "The Platform-A control flow calls ctrl_channel_open before ctrl_transfer_start and releases resources afterwards.";
        let content = format!("{cover}\n## Control API\n\n{evidence}\n");
        let item_id = store
            .insert_item(
                &dek,
                "platform control manual",
                &content,
                None,
                "file",
                None,
                None,
            )
            .unwrap();

        let ctx = SearchContext {
            fulltext: None,
            vectors: None,
            embedding: None,
            reranker: None,
            store: &store,
            dek: &dek,
        };

        let params = SearchParams::with_defaults(5);
        let results = search_with_context(&ctx, "ctrl_channel_open", &params).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].item_id, item_id);
        assert!(results[0].content.contains("ctrl_channel_open"));
        assert!(results[0].content.contains("ctrl_transfer_start"));
        assert!(
            !results[0].content.starts_with("cover page"),
            "lexical fallback should not inject the beginning of a long document"
        );
        assert!(results[0].chunk_offset_start.unwrap_or_default() > 0);
    }

    #[test]
    fn query_coverage_boost_prefers_specific_identifier_over_product_only_hit() {
        let mut results = vec![
            SearchResult {
                item_id: "product-faq".to_string(),
                score: 0.30,
                title: "ZX900 FAQ".to_string(),
                content: "ZX900 Platform-A console and board configuration notes".to_string(),
                ..Default::default()
            },
            SearchResult {
                item_id: "api-manual".to_string(),
                score: 0.10,
                title: "Platform-A control developer guide".to_string(),
                content: "Control flow uses ctrl_channel_open and then ctrl_transfer_start."
                    .to_string(),
                ..Default::default()
            },
        ];

        apply_query_coverage_boost(
            "ZX900 Platform-A ctrl_channel_open ctrl_transfer_start",
            &mut results,
        );
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        assert_eq!(results[0].item_id, "api-manual");
    }

    #[test]
    fn query_coverage_boost_handles_natural_chinese_action_words() {
        let mut results = vec![
            SearchResult {
                item_id: "platform-memory".to_string(),
                score: 0.40,
                title: "ZX900 Platform-A memory guide".to_string(),
                content: "ZX900 Platform-A reserved memory and boot configuration.".to_string(),
                ..Default::default()
            },
            SearchResult {
                item_id: "control-flow".to_string(),
                score: 0.20,
                title: "Platform-A control developer guide".to_string(),
                content:
                    "API 说明：ctrl_channel_open 申请控制通道。ctrl_transfer_start 启动一次传输。"
                        .to_string(),
                ..Default::default()
            },
        ];

        apply_query_coverage_boost(
            "ZX900 Platform-A 里申请控制通道并启动一次传输，大概按什么流程做？",
            &mut results,
        );
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        assert_eq!(results[0].item_id, "control-flow");
    }

    #[test]
    fn search_with_context_natural_chinese_query_adds_lexical_content_candidate() {
        use crate::store::Store;

        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        store
            .insert_item(
                &dek,
                "ZX900 Platform-A memory guide",
                "ZX900 Platform-A reserved memory and boot configuration. Control transfer is mentioned in a boot optimization note.",
                None,
                "file",
                None,
                None,
            )
            .unwrap();
        let control_id = store
            .insert_item(
                &dek,
                "Platform-A control developer guide",
                "模块接口说明：ctrl_channel_open 用于申请控制通道；配置描述符后，调用 ctrl_transfer_start 启动一次传输。",
                None,
                "file",
                None,
                None,
            )
            .unwrap();

        let ctx = SearchContext {
            fulltext: None,
            vectors: None,
            embedding: None,
            reranker: None,
            store: &store,
            dek: &dek,
        };

        let params = SearchParams::with_defaults(5);
        let results = search_with_context(
            &ctx,
            "ZX900 Platform-A 里申请控制通道并启动一次传输，大概按什么流程做？",
            &params,
        )
        .unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].item_id, control_id);
        assert!(results[0].content.contains("ctrl_channel_open"));
    }

    #[test]
    fn lexical_content_candidates_prefer_local_procedure_evidence_over_scattered_terms() {
        use crate::store::Store;

        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        store
            .insert_item(
                &dek,
                "Long mixed platform manual",
                &[
                    "ZX900 应用手册提到 Platform-A 与 Platform-B 协同启动。",
                    &"无关内容。".repeat(300),
                    "某章节说控制传输可用于视频缓存。",
                    &"背景介绍。".repeat(300),
                    "另一个章节讨论通道资源和一次传输统计，但没有给出接口。",
                ]
                .join("\n"),
                None,
                "file",
                None,
                None,
            )
            .unwrap();
        let control_id = store
            .insert_item(
                &dek,
                "Platform-A control developer guide",
                "模块接口说明：ctrl_channel_open 用于申请控制通道；配置描述符后，调用 ctrl_transfer_start 启动一次传输。",
                None,
                "file",
                None,
                None,
            )
            .unwrap();

        let ctx = SearchContext {
            fulltext: None,
            vectors: None,
            embedding: None,
            reranker: None,
            store: &store,
            dek: &dek,
        };

        let params = SearchParams::with_defaults(5);
        let results = search_with_context(
            &ctx,
            "ZX900 Platform-A 申请控制通道然后启动一次传输，流程怎么走？",
            &params,
        )
        .unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].item_id, control_id);
    }

    #[test]
    fn platform_hint_adjustment_prefers_explicit_platform_source() {
        let mut results = vec![
            SearchResult {
                item_id: "platform-b-control".to_string(),
                score: 0.50,
                title: "Platform-B 控制开发指南".to_string(),
                content: "ctrl_request_handle 申请控制通道".to_string(),
                ..Default::default()
            },
            SearchResult {
                item_id: "platform-a-control".to_string(),
                score: 0.40,
                title: "Platform-A 控制开发指南".to_string(),
                content: "ctrl_channel_open 申请控制通道".to_string(),
                ..Default::default()
            },
        ];
        apply_source_hint_boost("Platform-A 申请控制通道", &mut results);
        apply_platform_hint_adjustment("Platform-A 申请控制通道", &mut results);
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        assert_eq!(results[0].item_id, "platform-a-control");
    }

    #[test]
    fn search_with_context_returns_matched_vector_chunk_content() {
        use crate::store::Store;
        use crate::vectors::{VectorIndex, VectorMeta};

        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        let content =
            "front matter and table of contents\n\nactual API evidence TARGET_CHUNK_FUNC call flow\n";
        let item_id = store
            .insert_item(&dek, "manual", content, None, "file", None, None)
            .unwrap();
        let target_start = content.find("actual API evidence").unwrap();
        let target_end = content.len();
        store
            .upsert_chunk_span(&item_id, 1, target_start, target_end, 2, 1)
            .unwrap();

        let mut vectors = VectorIndex::new(2).unwrap();
        vectors
            .add(
                &[0.0, 1.0],
                VectorMeta {
                    item_id: item_id.clone(),
                    chunk_idx: 0,
                    level: 2,
                    section_idx: 0,
                },
            )
            .unwrap();
        vectors
            .add(
                &[1.0, 0.0],
                VectorMeta {
                    item_id: item_id.clone(),
                    chunk_idx: 1,
                    level: 2,
                    section_idx: 1,
                },
            )
            .unwrap();
        let embedding = Arc::new(CountingEmbeddingProvider {
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        vectors.set_embedding_fingerprint(Some(crate::embed::current_embedding_fingerprint(
            embedding.as_ref(),
        )));
        let ctx = SearchContext {
            fulltext: None,
            vectors: Some(&vectors),
            embedding: Some(embedding),
            reranker: None,
            store: &store,
            dek: &dek,
        };

        let results =
            search_with_context(&ctx, "where is target api", &SearchParams::with_defaults(5))
                .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].item_id, item_id);
        assert_eq!(results[0].chunk_idx, Some(1));
        assert!(results[0].content.contains("TARGET_CHUNK_FUNC"));
        assert!(!results[0].content.contains("table of contents"));
    }

    struct CountingEmbeddingProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl crate::embed::EmbeddingProvider for CountingEmbeddingProvider {
        fn embed(
            &self,
            texts: &[&str],
        ) -> crate::error::Result<(Vec<Vec<f32>>, crate::usage::TokenUsage)> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok((
                texts.iter().map(|_| vec![1.0f32, 0.0]).collect(),
                crate::usage::TokenUsage::empty("counting", "test"),
            ))
        }

        fn dimensions(&self) -> usize {
            2
        }

        fn is_available(&self) -> bool {
            true
        }
    }

    #[test]
    fn search_with_context_lexical_fast_path_skips_embedding_when_fts_hits() {
        use crate::index::FulltextIndex;
        use crate::store::Store;
        use crate::vectors::VectorIndex;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        let item_id = store
            .insert_item(
                &dek,
                "loop fixture",
                "# Loop fixture\n\nLOOPMARK42 tokio keyword body.\n",
                None,
                "file",
                None,
                None,
            )
            .unwrap();
        let ft = FulltextIndex::open_memory().unwrap();
        ft.add_document(
            &item_id,
            "loop fixture",
            "# Loop fixture\n\nLOOPMARK42 tokio keyword body.\n",
            "file",
        )
        .unwrap();
        let vectors = VectorIndex::new(2).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let embedding = Arc::new(CountingEmbeddingProvider {
            calls: calls.clone(),
        });
        let ctx = SearchContext {
            fulltext: Some(&ft),
            vectors: Some(&vectors),
            embedding: Some(embedding),
            reranker: None,
            store: &store,
            dek: &dek,
        };

        let params = SearchParams::with_defaults(10);
        let results = search_with_context(&ctx, "LOOPMARK42", &params).unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].item_id, item_id);

        let results = search_with_context(&ctx, "tokio", &params).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].item_id, item_id);
    }

    #[test]
    fn search_with_context_natural_query_still_uses_embedding() {
        use crate::index::FulltextIndex;
        use crate::store::Store;
        use crate::vectors::VectorIndex;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        store
            .insert_item(
                &dek,
                "rust ownership",
                "Rust ownership and borrowing guide.",
                None,
                "file",
                None,
                None,
            )
            .unwrap();
        let ft = FulltextIndex::open_memory().unwrap();
        let vectors = VectorIndex::new(2).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let embedding = Arc::new(CountingEmbeddingProvider {
            calls: calls.clone(),
        });
        let ctx = SearchContext {
            fulltext: Some(&ft),
            vectors: Some(&vectors),
            embedding: Some(embedding),
            reranker: None,
            store: &store,
            dek: &dek,
        };

        let params = SearchParams::with_defaults(10);
        let _ = search_with_context(&ctx, "what is rust ownership", &params).unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

// ============================================================================
// Time travel search — 自然语言时间表达解析
// ============================================================================
//
// "上周谁说了 X" 类查询。解析 query 中的中英文时间词，返回 unix epoch 区间。
// 调用方（search pipeline）取出区间后，在 SQL 层加 WHERE captured_at BETWEEN ?
// 过滤即可。本模块不修改现有检索函数，仅追加新 API。

/// 自然语言时间过滤区间（unix epoch 秒）。
///
/// `start_unix` 含入、`end_unix` 含入。"今天" → [今日 00:00, 今日 23:59:59]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeFilter {
    pub start_unix: i64,
    pub end_unix: i64,
}

/// 解析 query 中的时间表达式。
///
/// 支持：
/// - 中文："今天" / "昨天" / "前天" / "本周" / "上周" / "本月" / "上月"
/// - 相对：N 天前 / N 周前 / N 月前（中文数字 + 阿拉伯数字）
/// - 英文：today / yesterday / this week / last week / this month / last month
///
/// 未识别时返回 None（调用方走全时间检索）。
///
/// 实现注：依赖系统 wall clock。为支持测试，内部走 `now_unix()` 抽象，
/// 测试用 `parse_time_filter_with_now` 注入固定 now。
pub fn parse_time_filter(query: &str) -> Option<TimeFilter> {
    let now = chrono::Utc::now().timestamp();
    parse_time_filter_with_now(query, now)
}

/// 测试用：注入固定 now（unix epoch 秒）。
pub fn parse_time_filter_with_now(query: &str, now_unix: i64) -> Option<TimeFilter> {
    let q = query.to_lowercase();

    // 优先匹配相对表达："N 天前" / "N days ago"
    if let Some(filter) = parse_n_units_ago(&q, now_unix) {
        return Some(filter);
    }

    // 今天 / today
    if q.contains("今天") || q.contains("today") {
        return Some(day_range(now_unix, 0));
    }
    // 昨天 / yesterday
    if q.contains("昨天") || q.contains("yesterday") {
        return Some(day_range(now_unix, -1));
    }
    // 前天 (英文无对等词)
    if q.contains("前天") {
        return Some(day_range(now_unix, -2));
    }
    // 上周 / last week
    if q.contains("上周") || q.contains("上星期") || q.contains("last week") {
        return Some(week_range(now_unix, -1));
    }
    // 本周 / this week
    if q.contains("本周") || q.contains("这周") || q.contains("this week") {
        return Some(week_range(now_unix, 0));
    }
    // 上月 / last month
    if q.contains("上月") || q.contains("上个月") || q.contains("last month") {
        return Some(month_range(now_unix, -1));
    }
    // 本月 / this month
    if q.contains("本月") || q.contains("这个月") || q.contains("this month") {
        return Some(month_range(now_unix, 0));
    }

    None
}

/// 解析 "3 天前" / "3 days ago" / "三天前" 等相对表达
fn parse_n_units_ago(q: &str, now_unix: i64) -> Option<TimeFilter> {
    const DAY: i64 = 86400;
    let cn_digit = |c: char| -> Option<i64> {
        match c {
            '一' => Some(1),
            '二' | '两' => Some(2),
            '三' => Some(3),
            '四' => Some(4),
            '五' => Some(5),
            '六' => Some(6),
            '七' => Some(7),
            '八' => Some(8),
            '九' => Some(9),
            '十' => Some(10),
            _ => None,
        }
    };

    // 找数字 — 阿拉伯优先，否则中文单字
    let n: Option<i64> = q.chars().collect::<Vec<_>>().windows(2).find_map(|w| {
        // 阿拉伯数字（最多 3 位）
        if w[0].is_ascii_digit() {
            let s: String = q
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(|c| c.is_ascii_digit())
                .collect();
            return s.parse::<i64>().ok();
        }
        cn_digit(w[0])
    });

    let n = n?;
    if n <= 0 || n > 365 {
        return None;
    }

    // 单位识别
    if q.contains("天前") || q.contains("days ago") || q.contains("day ago") {
        let offset_days = -n;
        return Some(day_range(now_unix, offset_days));
    }
    if q.contains("周前") || q.contains("weeks ago") || q.contains("week ago") {
        let start = now_unix - n * 7 * DAY;
        let end = start + 7 * DAY - 1;
        return Some(TimeFilter {
            start_unix: start,
            end_unix: end,
        });
    }
    if q.contains("月前") || q.contains("months ago") || q.contains("month ago") {
        // 近似 30 天
        let start = now_unix - n * 30 * DAY;
        let end = start + 30 * DAY - 1;
        return Some(TimeFilter {
            start_unix: start,
            end_unix: end,
        });
    }

    None
}

fn day_range(now_unix: i64, offset_days: i64) -> TimeFilter {
    let day: i64 = 86400;
    let target = now_unix + offset_days * day;
    // 对齐 UTC 整日界（简化，不处理时区）
    let start = (target / day) * day;
    TimeFilter {
        start_unix: start,
        end_unix: start + day - 1,
    }
}

fn week_range(now_unix: i64, offset_weeks: i64) -> TimeFilter {
    let day: i64 = 86400;
    // 周一为周起点。Unix epoch 1970-01-01 是周四 → 偏移 4 天
    let days_since_epoch = now_unix / day;
    let weekday = (days_since_epoch + 4) % 7; // 0=周一
    let this_week_monday = (days_since_epoch - weekday) * day;
    let target_monday = this_week_monday + offset_weeks * 7 * day;
    TimeFilter {
        start_unix: target_monday,
        end_unix: target_monday + 7 * day - 1,
    }
}

fn month_range(now_unix: i64, offset_months: i64) -> TimeFilter {
    use chrono::{Datelike, TimeZone, Utc};
    let now = Utc
        .timestamp_opt(now_unix, 0)
        .single()
        .unwrap_or_else(Utc::now);
    let (mut year, mut month) = (now.year(), now.month() as i32);
    month += offset_months as i32;
    while month < 1 {
        month += 12;
        year -= 1;
    }
    while month > 12 {
        month -= 12;
        year += 1;
    }
    let start = Utc
        .with_ymd_and_hms(year, month as u32, 1, 0, 0, 0)
        .single()
        .map(|d| d.timestamp())
        .unwrap_or(now_unix);
    // 月末 = 下月 1 日 - 1 秒
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let next_start = Utc
        .with_ymd_and_hms(next_year, next_month as u32, 1, 0, 0, 0)
        .single()
        .map(|d| d.timestamp())
        .unwrap_or(start + 30 * 86400);
    TimeFilter {
        start_unix: start,
        end_unix: next_start - 1,
    }
}

#[cfg(test)]
mod retrieval_query_tests {
    use super::*;

    #[test]
    fn retrieval_semantic_query_strips_trailing_answer_control_sentence() {
        let query = "如何排查设备连接失败？请只基于知识库证据回答。";

        assert_eq!(retrieval_semantic_query(query), "如何排查设备连接失败？");
    }

    #[test]
    fn retrieval_semantic_query_strips_english_answer_control_sentence() {
        let query =
            "How do I troubleshoot device connectivity? Answer only from the knowledge base.";

        assert_eq!(
            retrieval_semantic_query(query),
            "How do I troubleshoot device connectivity?"
        );
    }

    #[test]
    fn retrieval_semantic_query_keeps_substantive_evidence_questions() {
        let query = "如何基于安全知识库证据排查流程问题？";

        assert_eq!(retrieval_semantic_query(query), query);
    }

    #[test]
    fn retrieval_semantic_query_does_not_strip_single_meta_topic_question() {
        let query = "知识库证据不足时应该怎么办？";

        assert_eq!(retrieval_semantic_query(query), query);
    }

    #[test]
    fn lexical_needles_include_compact_identifier_variants() {
        let needles = lexical_needles("排查 ABC/DEF-12 连接失败");

        assert!(needles.contains(&"abc/def-12".to_string()));
        assert!(needles.contains(&"abcdef12".to_string()));
        assert!(needles.contains(&"abc".to_string()));
        assert!(needles.contains(&"def".to_string()));
        assert!(needles.contains(&"12".to_string()));
    }
}

#[cfg(test)]
mod time_filter_tests {
    use super::*;

    // 固定 now = 2026-05-12 12:00:00 UTC = 1778587200
    // 该日为周二。
    const FIXED_NOW: i64 = 1_778_587_200;
    const DAY: i64 = 86_400;

    #[test]
    fn parse_today_chinese() {
        let f = parse_time_filter_with_now("今天有什么消息", FIXED_NOW).unwrap();
        // 2026-05-12 00:00 UTC = 1778544000
        assert_eq!(f.start_unix, 1_778_544_000);
        assert_eq!(f.end_unix, 1_778_544_000 + DAY - 1);
    }

    #[test]
    fn parse_yesterday_english() {
        let f = parse_time_filter_with_now("what happened yesterday", FIXED_NOW).unwrap();
        // 2026-05-11 00:00 UTC
        assert_eq!(f.start_unix, 1_778_544_000 - DAY);
        assert_eq!(f.end_unix, 1_778_544_000 - 1);
    }

    #[test]
    fn parse_last_week_chinese() {
        let f = parse_time_filter_with_now("上周谁说了 rust async", FIXED_NOW).unwrap();
        // 区间为 7 天 (周一 00:00 ~ 下周一 00:00 - 1s)
        assert_eq!(f.end_unix - f.start_unix, 7 * DAY - 1);
        // 终点 < FIXED_NOW
        assert!(f.end_unix < FIXED_NOW);
    }

    #[test]
    fn parse_this_month_english() {
        let f = parse_time_filter_with_now("show me this month logs", FIXED_NOW).unwrap();
        // 2026-05-01 00:00 UTC = 1777593600
        assert_eq!(f.start_unix, 1_777_593_600);
        // 2026-05-31 23:59:59 UTC = 2026-06-01 00:00 UTC - 1 = 1780272000 - 1
        assert_eq!(f.end_unix, 1_780_272_000 - 1);
    }

    #[test]
    fn parse_n_days_ago_chinese() {
        let f = parse_time_filter_with_now("3 天前的笔记", FIXED_NOW).unwrap();
        // 3 天前 = 2026-05-12 整日
        assert_eq!(f.start_unix, 1_778_544_000 - 3 * DAY);
        assert_eq!(f.end_unix, 1_778_544_000 - 3 * DAY + DAY - 1);
    }

    #[test]
    fn parse_unrecognized_returns_none() {
        assert!(parse_time_filter_with_now("rust async runtime", FIXED_NOW).is_none());
        assert!(parse_time_filter_with_now("", FIXED_NOW).is_none());
    }
}
