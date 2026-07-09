//! Edge-native retrieval planning.
//!
//! This module is intentionally pure: it decides which local index partitions,
//! retrieval channels, rerank bounds, and SRAS scoring coefficients should be
//! used before the existing BM25/vector/RRF path runs.

use crate::search::{detect_lang, Lang, SearchParams};
use crate::store::audit::PrivacyTier;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalTarget {
    LocalScheduler,
    LocalWorkstation,
    Cloud,
    Hybrid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalLatencyClass {
    Interactive,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalChannel {
    Metadata,
    Entity,
    Bm25,
    Vector,
    Summary,
    Recency,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalChannelPlan {
    pub channel: RetrievalChannel,
    pub top_k: usize,
    pub weight: f32,
    pub min_score: Option<f32>,
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexPartitionField {
    Vault,
    CorpusDomain,
    PrivacyTier,
    Modality,
    Language,
    SourceType,
    TimeBucket,
    EmbeddingModel,
    EmbeddingDim,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexPartitionFilter {
    pub field: IndexPartitionField,
    pub value: String,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexPartitionPlan {
    pub filters: Vec<IndexPartitionFilter>,
    pub local_only: bool,
    pub shard_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrasWeights {
    pub base: f32,
    pub exact_match: f32,
    pub entity_match: f32,
    pub same_domain: f32,
    pub same_language: f32,
    pub privacy_fit: f32,
    pub recency: f32,
    pub citation_span: f32,
    pub vector_similarity: f32,
    pub bm25_rank: f32,
    pub chunk_level: f32,
}

impl SrasWeights {
    pub fn local_scheduler_interactive() -> Self {
        Self {
            base: 1.0,
            exact_match: 1.20,
            entity_match: 0.70,
            same_domain: 0.55,
            same_language: 0.35,
            privacy_fit: 0.60,
            recency: 0.20,
            citation_span: 0.45,
            vector_similarity: 0.70,
            bm25_rank: 0.55,
            chunk_level: 0.20,
        }
    }

    pub fn local_workstation() -> Self {
        Self {
            vector_similarity: 0.85,
            bm25_rank: 0.50,
            ..Self::local_scheduler_interactive()
        }
    }

    pub fn cloud() -> Self {
        Self {
            exact_match: 1.00,
            entity_match: 0.60,
            same_domain: 0.45,
            same_language: 0.25,
            privacy_fit: 0.35,
            recency: 0.20,
            citation_span: 0.50,
            vector_similarity: 0.75,
            bm25_rank: 0.45,
            chunk_level: 0.12,
            base: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrasCandidateSignal {
    pub base_score: f32,
    pub exact_match: bool,
    pub entity_match: bool,
    pub same_domain: bool,
    pub same_language: bool,
    pub privacy_allowed: bool,
    pub has_citation_span: bool,
    pub vector_score: Option<f32>,
    pub bm25_rank: Option<usize>,
    pub recency_days: Option<u32>,
    pub chunk_level: Option<u8>,
}

impl Default for SrasCandidateSignal {
    fn default() -> Self {
        Self {
            base_score: 0.0,
            exact_match: false,
            entity_match: false,
            same_domain: false,
            same_language: false,
            privacy_allowed: true,
            has_citation_span: false,
            vector_score: None,
            bm25_rank: None,
            recency_days: None,
            chunk_level: None,
        }
    }
}

pub struct SrasSelector {
    weights: SrasWeights,
}

impl SrasSelector {
    pub fn new(weights: SrasWeights) -> Self {
        Self { weights }
    }

    pub fn score(&self, signal: &SrasCandidateSignal) -> f32 {
        score_sras_candidate(signal, &self.weights)
    }

    pub fn rank<'a, T, F>(
        &self,
        candidates: &'a [T],
        mut signal_for: F,
        limit: usize,
    ) -> Vec<RankedCandidate<'a, T>>
    where
        F: FnMut(&T) -> SrasCandidateSignal,
    {
        let mut ranked: Vec<_> = candidates
            .iter()
            .map(|candidate| RankedCandidate {
                candidate,
                score: self.score(&signal_for(candidate)),
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ranked.truncate(limit);
        ranked
    }
}

pub struct RankedCandidate<'a, T> {
    pub candidate: &'a T,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryFeatures {
    pub language: Lang,
    pub exactish: bool,
    pub entityish: bool,
    pub long_or_broad: bool,
    pub asks_summary: bool,
    pub token_estimate: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalPlan {
    pub target: RetrievalTarget,
    pub latency_class: RetrievalLatencyClass,
    pub final_top_k: usize,
    pub evidence_token_budget: usize,
    pub rerank_candidate_cap: usize,
    pub channels: Vec<RetrievalChannelPlan>,
    pub partitions: IndexPartitionPlan,
    pub sras_weights: SrasWeights,
    pub query_features: QueryFeatures,
    pub domain_hint: Option<String>,
}

impl RetrievalPlan {
    pub fn to_search_params(&self) -> SearchParams {
        let mut params = SearchParams::with_defaults_for_rag(self.final_top_k);
        let max_channel_top_k = self
            .channels
            .iter()
            .filter(|c| matches!(c.channel, RetrievalChannel::Bm25 | RetrievalChannel::Vector))
            .map(|c| c.top_k)
            .max()
            .unwrap_or(params.initial_k);
        params.initial_k = max_channel_top_k.max(self.final_top_k);
        params.intermediate_k = self.rerank_candidate_cap.max(self.final_top_k).min(200);
        params.min_score = self
            .channels
            .iter()
            .find(|c| c.channel == RetrievalChannel::Vector)
            .and_then(|c| c.min_score);
        params.domain_hint = self.domain_hint.clone();
        params
    }
}

#[derive(Debug, Clone)]
pub struct RetrievalPlanRequest<'a> {
    pub query: &'a str,
    pub top_k: usize,
    pub privacy_tier: PrivacyTier,
    pub target: RetrievalTarget,
    pub latency_class: RetrievalLatencyClass,
    pub vault_id: Option<&'a str>,
    pub corpus_domain: Option<&'a str>,
    pub modality: Option<&'a str>,
    pub source_type: Option<&'a str>,
    pub time_bucket: Option<&'a str>,
    pub embedding_model: Option<&'a str>,
    pub embedding_dim: Option<usize>,
}

impl<'a> RetrievalPlanRequest<'a> {
    pub fn local_scheduler_interactive(query: &'a str) -> Self {
        Self {
            query,
            top_k: 6,
            privacy_tier: PrivacyTier::L1,
            target: RetrievalTarget::LocalScheduler,
            latency_class: RetrievalLatencyClass::Interactive,
            vault_id: None,
            corpus_domain: None,
            modality: None,
            source_type: None,
            time_bucket: None,
            embedding_model: Some("bge-m3"),
            embedding_dim: Some(1024),
        }
    }

    pub fn with_privacy_tier(mut self, privacy_tier: PrivacyTier) -> Self {
        self.privacy_tier = privacy_tier;
        self
    }

    pub fn with_corpus_domain(mut self, domain: &'a str) -> Self {
        self.corpus_domain = Some(domain);
        self
    }

    pub fn with_vault(mut self, vault_id: &'a str) -> Self {
        self.vault_id = Some(vault_id);
        self
    }

    pub fn background(mut self) -> Self {
        self.latency_class = RetrievalLatencyClass::Background;
        self
    }
}

pub fn plan_retrieval(req: RetrievalPlanRequest<'_>) -> RetrievalPlan {
    let features = analyze_query(req.query);
    let final_top_k = normalize_top_k(req.top_k, req.target, req.latency_class);
    let rerank_candidate_cap = rerank_cap(req.target, req.latency_class);
    let evidence_token_budget = evidence_budget(req.target, req.latency_class, &features);
    let domain_hint = normalize_domain(req.corpus_domain);

    RetrievalPlan {
        target: req.target,
        latency_class: req.latency_class,
        final_top_k,
        evidence_token_budget,
        rerank_candidate_cap,
        channels: build_channels(
            req.target,
            req.latency_class,
            final_top_k,
            rerank_candidate_cap,
            &features,
        ),
        partitions: build_partitions(&req, &features),
        sras_weights: match req.target {
            RetrievalTarget::LocalScheduler => SrasWeights::local_scheduler_interactive(),
            RetrievalTarget::LocalWorkstation | RetrievalTarget::Hybrid => {
                SrasWeights::local_workstation()
            }
            RetrievalTarget::Cloud => SrasWeights::cloud(),
        },
        query_features: features,
        domain_hint,
    }
}

pub fn score_sras_candidate(signal: &SrasCandidateSignal, weights: &SrasWeights) -> f32 {
    if !signal.privacy_allowed {
        return -1_000_000.0;
    }

    let mut score = weights.base + signal.base_score.max(0.0);
    if signal.exact_match {
        score += weights.exact_match;
    }
    if signal.entity_match {
        score += weights.entity_match;
    }
    if signal.same_domain {
        score += weights.same_domain;
    }
    if signal.same_language {
        score += weights.same_language;
    }
    score += weights.privacy_fit;
    if signal.has_citation_span {
        score += weights.citation_span;
    }
    if let Some(vector_score) = signal.vector_score {
        score += weights.vector_similarity * vector_score.clamp(0.0, 1.0);
    }
    if let Some(rank) = signal.bm25_rank {
        score += weights.bm25_rank / ((rank + 1) as f32).sqrt();
    }
    if let Some(days) = signal.recency_days {
        score += weights.recency / (1.0 + days as f32 / 30.0);
    }
    if let Some(level) = signal.chunk_level {
        let level_reward = match level {
            1 => 0.45,
            2 => 1.0,
            _ => 0.20,
        };
        score += weights.chunk_level * level_reward;
    }
    score
}

pub fn analyze_query(query: &str) -> QueryFeatures {
    let language = detect_lang(query);
    let token_estimate = query
        .split_whitespace()
        .map(|s| if s.is_ascii() { 1 } else { 2 })
        .sum::<usize>()
        .max(query.chars().filter(|c| !c.is_whitespace()).count() / 3);
    let lower = query.to_lowercase();
    let asks_summary = [
        "summary",
        "summarize",
        "overview",
        "compare",
        "synthesis",
        "总结",
        "概括",
        "对比",
        "综合",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let exactish = query_has_exact_marker(query);
    let entityish = exactish || query_has_entity_shape(query);
    let long_or_broad = token_estimate >= 32 || asks_summary;

    QueryFeatures {
        language,
        exactish,
        entityish,
        long_or_broad,
        asks_summary,
        token_estimate,
    }
}

fn build_channels(
    target: RetrievalTarget,
    latency_class: RetrievalLatencyClass,
    final_top_k: usize,
    rerank_candidate_cap: usize,
    features: &QueryFeatures,
) -> Vec<RetrievalChannelPlan> {
    let foreground = latency_class == RetrievalLatencyClass::Interactive;
    let initial_k = match (target, foreground) {
        (RetrievalTarget::LocalScheduler, true) => 80,
        (RetrievalTarget::LocalScheduler, false) => 160,
        (_, true) => 120,
        (_, false) => 240,
    };
    let mut channels = Vec::new();

    if features.entityish {
        channels.push(RetrievalChannelPlan {
            channel: RetrievalChannel::Metadata,
            top_k: final_top_k.saturating_mul(2).max(8),
            weight: 1.15,
            min_score: None,
            required: features.exactish,
        });
        channels.push(RetrievalChannelPlan {
            channel: RetrievalChannel::Entity,
            top_k: final_top_k.saturating_mul(2).max(8),
            weight: 1.05,
            min_score: None,
            required: false,
        });
    }

    channels.push(RetrievalChannelPlan {
        channel: RetrievalChannel::Bm25,
        top_k: initial_k,
        weight: if features.exactish { 0.55 } else { 0.40 },
        min_score: None,
        required: false,
    });
    channels.push(RetrievalChannelPlan {
        channel: RetrievalChannel::Vector,
        top_k: initial_k,
        weight: if features.exactish { 0.45 } else { 0.60 },
        min_score: Some(if target == RetrievalTarget::LocalScheduler {
            0.65
        } else {
            0.60
        }),
        required: false,
    });

    if features.long_or_broad {
        channels.push(RetrievalChannelPlan {
            channel: RetrievalChannel::Summary,
            top_k: rerank_candidate_cap.min(40),
            weight: 0.35,
            min_score: None,
            required: false,
        });
    }

    channels.push(RetrievalChannelPlan {
        channel: RetrievalChannel::Recency,
        top_k: final_top_k,
        weight: 0.10,
        min_score: None,
        required: false,
    });

    channels
}

fn build_partitions(
    req: &RetrievalPlanRequest<'_>,
    features: &QueryFeatures,
) -> IndexPartitionPlan {
    let mut filters = Vec::new();

    push_filter(&mut filters, IndexPartitionField::Vault, req.vault_id, true);
    push_filter(
        &mut filters,
        IndexPartitionField::PrivacyTier,
        Some(privacy_label(req.privacy_tier)),
        true,
    );
    push_filter(
        &mut filters,
        IndexPartitionField::CorpusDomain,
        normalize_domain(req.corpus_domain).as_deref(),
        req.corpus_domain.is_some(),
    );
    push_filter(
        &mut filters,
        IndexPartitionField::Modality,
        req.modality.or(Some("text")),
        false,
    );
    push_filter(
        &mut filters,
        IndexPartitionField::Language,
        Some(lang_label(features.language)),
        false,
    );
    push_filter(
        &mut filters,
        IndexPartitionField::SourceType,
        req.source_type,
        false,
    );
    push_filter(
        &mut filters,
        IndexPartitionField::TimeBucket,
        req.time_bucket,
        false,
    );
    push_filter(
        &mut filters,
        IndexPartitionField::EmbeddingModel,
        req.embedding_model,
        true,
    );
    let embedding_dim = req.embedding_dim.map(|d| d.to_string());
    push_filter(
        &mut filters,
        IndexPartitionField::EmbeddingDim,
        embedding_dim.as_deref(),
        req.embedding_dim.is_some(),
    );

    let domain = normalize_domain(req.corpus_domain).unwrap_or_else(|| "general".to_string());
    let shard_key = format!(
        "{}:{}:{}:{}",
        req.vault_id.unwrap_or("default"),
        privacy_label(req.privacy_tier),
        domain,
        req.embedding_model.unwrap_or("embedding-default")
    );
    IndexPartitionPlan {
        filters,
        local_only: req.privacy_tier == PrivacyTier::L0
            || req.target == RetrievalTarget::LocalScheduler,
        shard_key,
    }
}

fn push_filter(
    filters: &mut Vec<IndexPartitionFilter>,
    field: IndexPartitionField,
    value: Option<&str>,
    required: bool,
) {
    if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
        filters.push(IndexPartitionFilter {
            field,
            value: value.to_string(),
            required,
        });
    }
}

fn normalize_top_k(
    requested: usize,
    target: RetrievalTarget,
    latency_class: RetrievalLatencyClass,
) -> usize {
    let requested = requested.max(1);
    match (target, latency_class) {
        (RetrievalTarget::LocalScheduler, RetrievalLatencyClass::Interactive) => requested.min(8),
        (RetrievalTarget::LocalScheduler, RetrievalLatencyClass::Background) => requested.min(16),
        (_, RetrievalLatencyClass::Interactive) => requested.min(12),
        (_, RetrievalLatencyClass::Background) => requested.min(32),
    }
}

fn rerank_cap(target: RetrievalTarget, latency_class: RetrievalLatencyClass) -> usize {
    match (target, latency_class) {
        (RetrievalTarget::LocalScheduler, RetrievalLatencyClass::Interactive) => 20,
        (RetrievalTarget::LocalScheduler, RetrievalLatencyClass::Background) => 40,
        (_, RetrievalLatencyClass::Interactive) => 40,
        (_, RetrievalLatencyClass::Background) => 80,
    }
}

fn evidence_budget(
    target: RetrievalTarget,
    latency_class: RetrievalLatencyClass,
    features: &QueryFeatures,
) -> usize {
    match (target, latency_class, features.long_or_broad) {
        (RetrievalTarget::LocalScheduler, RetrievalLatencyClass::Interactive, _) => 2048,
        (RetrievalTarget::LocalScheduler, RetrievalLatencyClass::Background, true) => 4096,
        (RetrievalTarget::LocalScheduler, RetrievalLatencyClass::Background, false) => 3072,
        (_, RetrievalLatencyClass::Interactive, true) => 4096,
        (_, RetrievalLatencyClass::Interactive, false) => 3072,
        (_, RetrievalLatencyClass::Background, _) => 8192,
    }
}

fn query_has_exact_marker(query: &str) -> bool {
    if query.contains('"') || query.contains('`') {
        return true;
    }
    query
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .any(|token| {
            let token = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
            token.len() >= 4
                && token.chars().any(|c| c.is_ascii_digit())
                && token
                    .chars()
                    .any(|c| c.is_ascii_uppercase() || c == '-' || c == '_')
        })
}

fn query_has_entity_shape(query: &str) -> bool {
    let compact_len = query.chars().filter(|c| !c.is_whitespace()).count();
    compact_len <= 24
        || query
            .split_whitespace()
            .any(|token| token.chars().all(|c| c.is_ascii_uppercase()) && token.len() >= 2)
}

fn normalize_domain(domain: Option<&str>) -> Option<String> {
    domain
        .map(str::trim)
        .filter(|d| !d.is_empty() && *d != "general")
        .map(ToString::to_string)
}

fn privacy_label(privacy_tier: PrivacyTier) -> &'static str {
    match privacy_tier {
        PrivacyTier::L0 => "L0",
        PrivacyTier::L1 => "L1",
        PrivacyTier::L3 => "L3",
    }
}

fn lang_label(lang: Lang) -> &'static str {
    match lang {
        Lang::Zh => "zh",
        Lang::En => "en",
        Lang::Mixed => "mixed",
    }
}
