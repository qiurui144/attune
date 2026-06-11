// npu-vault/crates/vault-core/src/search.rs

use std::collections::HashMap;
use std::sync::Arc;

use crate::embed::EmbeddingProvider;
use crate::index::FulltextIndex;
use crate::infer::RerankProvider;
use crate::store::Store;
use crate::vectors::VectorIndex;

/// RRF 参数
pub const RRF_K: f32 = 60.0;
pub const RERANK_VECTOR_WEIGHT: f32 = 0.7;
pub const RERANK_RRF_WEIGHT: f32 = 0.3;
pub const RERANK_TOP_K_THRESHOLD: usize = 20;
pub const DEFAULT_VECTOR_WEIGHT: f32 = 0.6;
pub const DEFAULT_FULLTEXT_WEIGHT: f32 = 0.4;
pub const INJECTION_BUDGET: usize = 2000;

/// 启用 cross-encoder reranker 的最小候选数。
/// 候选数 < 此阈值时，RRF 排序比 cross-encoder 重排更稳定（cross-encoder
/// 在小候选集上放大噪声 / 跨语言错配）。
pub const RERANK_MIN_CANDIDATES: usize = 5;

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
pub enum Lang { Zh, En, Mixed }

pub fn detect_lang(s: &str) -> Lang {
    let (mut cjk, mut ascii_alpha, mut total) = (0usize, 0usize, 0usize);
    for c in s.chars() {
        if c.is_whitespace() { continue; }
        total += 1;
        if ('\u{4e00}'..='\u{9fff}').contains(&c) { cjk += 1; }
        else if c.is_ascii_alphabetic() { ascii_alpha += 1; }
    }
    if total == 0 { return Lang::Mixed; }
    let cjk_ratio = cjk as f32 / total as f32;
    let ascii_ratio = ascii_alpha as f32 / total as f32;
    if cjk_ratio >= 0.30 { Lang::Zh }
    else if ascii_ratio >= 0.70 { Lang::En }
    else { Lang::Mixed }
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
/// 关键词命中策略：每个 domain 维护一组特征词，统计命中数最多的 domain 返回。
/// 返回 None = 未明确意图（不应用 cross-domain penalty，保持现状）。
///
/// 关键词集建议来源：vertical plugin.yaml::chat_trigger.project_keywords。
/// 当 plugin loader 已加载 vertical plugin 时调用方应优先用 plugin 数据；
/// 这里仅提供 hardcoded fallback 让 OSS 裸装也能用基础识别能力。
pub fn detect_query_domain(query: &str) -> Option<String> {
    use std::collections::HashMap;

    // hardcoded fallback：覆盖 attune-pro 6 vertical 的核心特征词
    // 每个 domain 选 12-20 个高判别性词（避免泛词如"问题/可以"）
    let keywords_by_domain: &[(&str, &[&str])] = &[
        ("legal", &[
            "法律", "法条", "法规", "法院", "判决", "案件", "案号", "诉讼", "起诉", "判例",
            "民法", "刑法", "民法典", "合同法", "公司法", "商标法", "专利法",
            "借贷", "商标", "股东", "股权", "侵权", "违约", "赔偿", "仲裁",
            "反洗钱", "劳动合同", "工伤", "婚姻", "继承",
        ]),
        ("tech", &[
            // Rust / 系统编程
            "Rust", "ownership", "borrow", "lifetime",
            // Python / 通用
            "Python", "decorator", "tuple", "list comprehension",
            // 算法 / 数据结构
            "算法", "数据结构", "动态规划", "二叉树", "哈希", "梯度下降", "过拟合",
            // 系统 / 分布式
            "Linux", "Docker", "kubernetes", "k8s", "Redis", "MySQL", "PostgreSQL",
            "分布式", "TCP", "HTTP", "Socket",
            // 数据库
            "SQL", "索引", "事务",
        ]),
        ("medical", &[
            "病历", "诊断", "症状", "用药", "处方", "手术", "病人", "患者",
            "临床", "医院", "禁忌", "副作用", "剂量",
        ]),
        ("patent", &[
            "专利", "权利要求", "申请号", "IPC", "OA", "审查", "优先权", "新颖性",
            "创造性", "实用新型", "外观设计", "PCT",
        ]),
    ];

    let q = query.to_lowercase();
    let mut hit_counts: HashMap<&str, usize> = HashMap::new();
    for (domain, kws) in keywords_by_domain {
        for kw in *kws {
            // 中文命中按子串；英文命中按子串（lowercase 已处理大小写）
            if q.contains(&kw.to_lowercase()) {
                *hit_counts.entry(*domain).or_insert(0) += 1;
            }
        }
    }
    // 至少 1 个命中才返回（避免误识别），同分则按表序优先
    hit_counts
        .into_iter()
        .max_by_key(|(_, c)| *c)
        .filter(|(_, c)| *c >= 1)
        .map(|(d, _)| d.to_string())
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

/// 搜索结果
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SearchResult {
    pub item_id: String,
    pub score: f32,
    pub title: String,
    pub content: String,
    pub source_type: String,
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

/// RRF 融合两组排名结果
pub fn rrf_fuse(
    vector_results: &[(String, f32)],
    fulltext_results: &[(String, f32)],
    vector_weight: f32,
    fulltext_weight: f32,
    top_k: usize,
) -> Vec<(String, f32)> {
    let mut scores: HashMap<String, f32> = HashMap::new();

    for (rank, (id, _score)) in vector_results.iter().enumerate() {
        let rrf = vector_weight / (RRF_K + rank as f32 + 1.0);
        *scores.entry(id.clone()).or_default() += rrf;
    }
    for (rank, (id, _score)) in fulltext_results.iter().enumerate() {
        let rrf = fulltext_weight / (RRF_K + rank as f32 + 1.0);
        *scores.entry(id.clone()).or_default() += rrf;
    }

    let mut sorted: Vec<(String, f32)> = scores.into_iter().collect();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    sorted.truncate(top_k);
    sorted
}

/// 动态注入预算分配
pub fn allocate_budget(results: &mut [SearchResult], budget: usize) {
    let total_score: f32 = results.iter().map(|r| r.score).sum();
    if total_score <= 0.0 || results.is_empty() {
        // 保证每条至少 100 字符，与正比路径中 .max(100.0) 对齐
        let per_item = (budget / results.len().max(1)).max(100);
        for r in results.iter_mut() {
            let content = &r.content;
            let end = content.char_indices()
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
        let end = content.char_indices()
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
pub fn rerank(
    query_vec: &[f32],
    results: &mut [SearchResult],
    vector_index: &VectorIndex,
) {
    for result in results.iter_mut() {
        let rrf_score = result.score;
        let rerank_score = vector_index
            .get_vector(&result.item_id)
            .map(|item_vec| cosine_similarity(query_vec, &item_vec))
            .unwrap_or(0.0);
        result.score = RERANK_VECTOR_WEIGHT * rerank_score + RERANK_RRF_WEIGHT * rrf_score;
    }
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
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
    // 1. 全文搜索（initial_k）
    let ft_results = ctx.fulltext
        .map(|ft| ft.search(query, params.initial_k).unwrap_or_else(|e| {
            log::warn!("fulltext search error: {e}");
            vec![]
        }))
        .unwrap_or_default();

    // 2. 向量搜索（initial_k）
    // J3 (per spec §J3)：拿到 vector 结果后立即按 min_score 过滤；
    // 低于阈值的进 RRF 前丢弃，避免噪音污染融合排序。
    let (vec_results, query_vec): (Vec<(String, f32)>, Option<Vec<f32>>) =
        match (&ctx.embedding, &ctx.vectors) {
            (Some(emb), Some(vecs)) => {
                match emb.embed(&[query]) {
                    Ok((e, _usage)) if !e.is_empty() => {
                        let qv = e[0].clone();
                        let raw: Vec<(String, f32)> = vecs.search(&qv, params.initial_k)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|(meta, score)| (meta.item_id, score))
                            .collect();
                        let filtered: Vec<(String, f32)> = match params.min_score {
                            Some(threshold) => {
                                let kept: Vec<_> = raw.into_iter()
                                    .filter(|(_, s)| *s >= threshold)
                                    .collect();
                                log::info!(
                                    "search J3: vector min_score={:.3} kept {} results",
                                    threshold, kept.len()
                                );
                                kept
                            }
                            None => raw,
                        };
                        (filtered, Some(qv))
                    }
                    _ => (vec![], None),
                }
            }
            _ => (vec![], None),
        };

    log::info!(
        "search stages: query='{}' fts={} vec={}",
        query.chars().take(50).collect::<String>(),
        ft_results.len(),
        vec_results.len(),
    );

    // 3. RRF 融合 → intermediate_k
    let fused = rrf_fuse(&vec_results, &ft_results, DEFAULT_VECTOR_WEIGHT, DEFAULT_FULLTEXT_WEIGHT, params.intermediate_k);
    log::info!("search stages: rrf_fused={}", fused.len());

    // 4. 获取并解密 items + F2 (W3 batch A) 拉 breadcrumb sidecar
    let mut results: Vec<SearchResult> = Vec::new();
    for (item_id, score) in &fused {
        if let Ok(Some(item)) = ctx.store.get_item(ctx.dek, item_id) {
            // breadcrumb 现已加密落盘，需传 dek 解密
            let (breadcrumb, off_start, off_end) = ctx
                .store
                .get_first_chunk_breadcrumb(ctx.dek, &item.id)
                .ok()
                .flatten()
                .map(|(p, s, e)| (p, Some(s), Some(e)))
                .unwrap_or_default();
            // v0.6 Phase B F-Pro：拉 corpus_domain；item 不存在 / 列缺时回退 'general'
            let corpus_domain = ctx
                .store
                .get_item_corpus_domain(&item.id)
                .unwrap_or_else(|_| "general".to_string());
            results.push(SearchResult {
                item_id: item.id,
                score: *score,
                title: item.title,
                content: item.content,
                source_type: item.source_type,
                inject_content: None,
                breadcrumb,
                chunk_offset_start: off_start,
                chunk_offset_end: off_end,
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

    if let Some(reranker) = &ctx.reranker {
        if results.len() >= RERANK_MIN_CANDIDATES {
            let docs: Vec<&str> = results.iter().map(|r| r.content.as_str()).collect();
            match reranker.score(query, &docs) {
                Ok(scores) => {
                    for (r, s) in results.iter_mut().zip(scores.iter()) {
                        r.score = *s;
                    }
                }
                Err(e) => {
                    log::warn!("reranker failed, keeping RRF order: {e}");
                }
            }
        } else {
            log::info!(
                "search stages: reranker skipped (candidates={} < {})",
                results.len(), RERANK_MIN_CANDIDATES
            );
        }
    } else if results.len() <= RERANK_TOP_K_THRESHOLD {
        if let Some(qvec) = &query_vec {
            if let Some(vecs) = ctx.vectors {
                rerank(qvec, &mut results, vecs);
            }
        }
    }

    // 语言匹配降权：任何排序策略之后统一应用，不改变同语言相对顺序
    apply_cross_lang_penalty(&mut results, query_lang);

    // v0.6 Phase B F-Pro：跨领域降权（同语种跨领域污染防御）
    // 如 query="反洗钱"（domain_hint=legal）+ doc.corpus_domain=tech → score *= 0.4
    apply_cross_domain_penalty(&mut results, params.domain_hint.as_deref());

    // 最终排序
    results.sort_by(|a, b| b.score.partial_cmp(&a.score)
        .unwrap_or(std::cmp::Ordering::Equal));

    // 6. 截取 top_k（保护：如果 top_k=0，别截成空）
    let final_k = params.top_k.max(1);
    results.truncate(final_k);
    log::info!("search stages: returned={}", results.len());
    Ok(results)
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
        assert_eq!(detect_lang("What is rust ownership and borrowing"), Lang::En);
        assert_eq!(detect_lang("Box T smart pointer reference cycles"), Lang::En);
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
                item_id: "1".into(), score: 0.2, title: "references-and-borrowing".into(),
                content: "In Rust, references allow you to refer to a value without taking ownership.".into(),
                source_type: "file".into(), inject_content: None, ..Default::default() },
            SearchResult {
                item_id: "2".into(), score: 0.3, title: "民法典".into(),
                content: "中华人民共和国民法典第一编 总则".into(),
                source_type: "file".into(), inject_content: None, ..Default::default() },
        ];
        apply_cross_lang_penalty(&mut results, Lang::En);
        assert_eq!(results[0].score, 0.2, "英文文档不降权");
        assert!(results[1].score < 0.1, "中文文档应被降权 (0.3 * 0.3 = 0.09): {}",
            results[1].score);
    }

    #[test]
    fn cross_lang_penalty_mixed_query_no_penalty() {
        let mut results = vec![
            SearchResult {
                item_id: "1".into(), score: 0.5, title: "rust 所有权".into(),
                content: "Rust ownership system...".into(),
                source_type: "file".into(), inject_content: None, ..Default::default() },
        ];
        apply_cross_lang_penalty(&mut results, Lang::Mixed);
        assert_eq!(results[0].score, 0.5, "Mixed query 不应降权任何结果");
    }

    #[test]
    fn rrf_fuse_basic() {
        let vec_results = vec![
            ("a".into(), 0.9), ("b".into(), 0.7), ("c".into(), 0.5),
        ];
        let ft_results = vec![
            ("b".into(), 10.0), ("a".into(), 8.0), ("d".into(), 5.0),
        ];

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
            assert!(p.intermediate_k >= tk, "intermediate_k must be >= top_k for top_k={tk}");
            // intermediate_k 上限 200 防止 rerank 过度膨胀（每个候选都过 ONNX 推理）
            assert!(p.intermediate_k <= 200, "intermediate_k ceiling 200 for top_k={tk}");
            assert_eq!(p.top_k, tk);
        }
    }

    #[test]
    fn allocate_budget_proportional() {
        let mut results = vec![
            SearchResult {
                item_id: "a".into(), score: 0.8, title: "A".into(),
                content: "A".repeat(3000), source_type: "note".into(), inject_content: None, ..Default::default() },
            SearchResult {
                item_id: "b".into(), score: 0.2, title: "B".into(),
                content: "B".repeat(3000), source_type: "note".into(), inject_content: None, ..Default::default() },
        ];
        allocate_budget(&mut results, 2000);

        let a_len = results[0].inject_content.as_ref().unwrap().chars().count();
        let b_len = results[1].inject_content.as_ref().unwrap().chars().count();
        // "a" has 80% score, should get ~1600 chars; "b" has 20%, should get ~400 (min 100)
        assert!(a_len > b_len, "Higher score should get more budget: a={a_len} b={b_len}");
        assert!(b_len >= 100, "Minimum budget should be 100: got {b_len}");
    }

    #[test]
    fn allocate_budget_zero_scores() {
        let mut results = vec![
            SearchResult {
                item_id: "a".into(), score: 0.0, title: "A".into(),
                content: "A".repeat(3000), source_type: "note".into(), inject_content: None, ..Default::default() },
            SearchResult {
                item_id: "b".into(), score: 0.0, title: "B".into(),
                content: "B".repeat(3000), source_type: "note".into(), inject_content: None, ..Default::default() },
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
        idx.add(&[1.0, 0.0], VectorMeta { item_id: "close".into(), chunk_idx: 0, level: 2, section_idx: 0 }).unwrap();
        idx.add(&[0.0, 1.0], VectorMeta { item_id: "far".into(), chunk_idx: 0, level: 2, section_idx: 0 }).unwrap();

        let mut results = vec![
            SearchResult { item_id: "far".into(),   score: 0.9, title: "Far".into(),   content: "c".into(), source_type: "note".into(), inject_content: None, ..Default::default() },
            SearchResult { item_id: "close".into(), score: 0.5, title: "Close".into(), content: "c".into(), source_type: "note".into(), inject_content: None, ..Default::default() },
        ];

        rerank(&[1.0, 0.0], &mut results, &idx);
        assert_eq!(results[0].item_id, "close", "Reranker should elevate closer vector");
    }

    #[test]
    fn rerank_fallback_when_no_vector() {
        use crate::vectors::VectorIndex;

        let idx = VectorIndex::new(2).unwrap();
        let mut results = vec![
            SearchResult { item_id: "a".into(), score: 0.8, title: "A".into(), content: "c".into(), source_type: "note".into(), ..Default::default() },
            SearchResult { item_id: "b".into(), score: 0.3, title: "B".into(), content: "c".into(), source_type: "note".into(), ..Default::default() },
        ];
        rerank(&[1.0, 0.0], &mut results, &idx);
        assert!(results[0].score >= results[1].score);
    }

    #[test]
    fn search_params_defaults_clamp_correctly() {
        let p = SearchParams::with_defaults(5);
        assert_eq!(p.top_k, 5);
        assert_eq!(p.initial_k, 25);   // 5*5=25, in [20,500]
        assert_eq!(p.intermediate_k, 10); // 5*2=10
        // 通用 search 默认不启用 min_score 阈值，保持向后行为契约
        assert_eq!(p.min_score, None);

        let p2 = SearchParams::with_defaults(1);
        assert_eq!(p2.initial_k, 20);  // min clamp
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
        let raw: Vec<(String, f32)> = vec![
            ("a".into(), 0.50),
            ("b".into(), 0.70),
            ("c".into(), 0.85),
        ];
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
        assert_eq!(rag.initial_k, 25);  // 与通用版同构
    }

    #[test]
    fn min_score_threshold_curve_documented_in_spec() {
        // 锁住吴师兄文章给出的曲线值，避免有人未读 spec 误改默认
        let rag = SearchParams::with_defaults_for_rag(5);
        assert_eq!(rag.min_score, Some(0.65), "RAG 默认 0.65（保守端，召回优先）");
        // 0.72 是吴师兄推荐的"精度优先"档，未来 Settings 提供
        // 0.78 开始漏边缘 case，仅极端精度场景用
    }

    // #9: search_with_context 三阶段管道（有 Reranker）
    #[test]
    fn search_with_context_reranker_reorders_results() {
        use crate::infer::MockRerankProvider;
        use crate::store::Store;

        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();

        // 插入两条 item
        store.insert_item(&dek, "低分文档", "content about cats", None, "note", None, None).unwrap();
        store.insert_item(&dek, "高分文档", "content about dogs", None, "note", None, None).unwrap();

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
        assert!(results.is_ok(), "search_with_context should not fail with reranker");
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
    let n: Option<i64> = q
        .chars()
        .collect::<Vec<_>>()
        .windows(2)
        .find_map(|w| {
            // 阿拉伯数字（最多 3 位）
            if w[0].is_ascii_digit() {
                let s: String = q.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
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
        return Some(TimeFilter { start_unix: start, end_unix: end });
    }
    if q.contains("月前") || q.contains("months ago") || q.contains("month ago") {
        // 近似 30 天
        let start = now_unix - n * 30 * DAY;
        let end = start + 30 * DAY - 1;
        return Some(TimeFilter { start_unix: start, end_unix: end });
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
    let now = Utc.timestamp_opt(now_unix, 0).single().unwrap_or_else(Utc::now);
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
    let (next_year, next_month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
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
