//! 信息监控闭环 —— 主题/源监控（Watch）、确定性 triage、跨源/跨时间去重。
//!
//! 北极星诉求："对你关注的内容，尽快获取准确的信息"。本模块实装 spec
//! `docs/superpowers/specs/2026-06-19-info-monitoring-loop.md` 的 A/C/D 三部分：
//!
//! - **A 监控（Watch）**：用户声明关注主题（关键词 / 实体 / 语义 anchor）+ 可选绑源。
//!   新 item 入库后 [`WatchMatcher::evaluate`] 用**确定性匹配**（关键词 / 实体重叠 /
//!   已算好的向量相似度）判定命中，**不调 embedding / 不调 LLM**。
//! - **C triage**：[`WatchHit`] 按可解释分排序（匹配 × recency × 源权重 × 交互）。
//! - **D 去重**：`content_hash` O(1)（store 层已有）+ 近似去重（标题归一 + 向量阈值 →
//!   同 `dedup_group`）+ 跨时间 marker（`digested` 标志）。
//!
//! ## 成本契约编译期守卫（最高约束，spec §8 R1）
//!
//! [`WatchMatcher::evaluate`] 与 `digest::DigestBuilder::build_default` 的**签名不含
//! 任何 LLM provider 句柄** —— 监控 + 默认 digest 路径在编译期就无法偷跑 LLM。
//! LLM 只可能出现在 `digest::DigestBuilder::build_llm_summary`（显式开启 + 后台队列
//! 可暂停）、watch 问答、深度研究三个**显式**入口。
//! `tests/monitoring_cost_contract.rs` 用 panic-mock proptest 把这条立成 CI 硬门。

pub mod deep_research;
pub mod digest;

use serde::{Deserialize, Serialize};

use crate::entities::{entity_overlap_score, Entity};

/// 关注项（用户显式声明的监控主题 / 源），对齐 spec §3.2 `watches` 表的运行时投影。
///
/// `anchor_vec` 是建 watch 时一次性 embed 的语义 anchor 向量（spec §8：⚡ 本地算力一次性，
/// 之后匹配复用，**绝不**为每次匹配重新 embed）。`None` = 纯关键词/实体匹配。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Watch {
    pub id: String,
    pub label: String,
    pub keywords: Vec<String>,
    pub entities: Vec<Entity>,
    /// 语义 anchor 文本（仅展示 / 重建用；匹配走 `anchor_vec`）。
    pub anchor_text: String,
    /// 建 watch 时算好的 anchor 向量（与 item 向量同 embedding 模型）。
    pub anchor_vec: Option<Vec<f32>>,
    /// 绑定的 connector source id（空 = 全源）。匹配时用 item 的 `source_type` / 源标识过滤。
    pub source_ids: Vec<String>,
    /// 命中阈值（综合分 ≥ 此值才算命中），复用 search 已验证常量量级。
    pub match_threshold: f32,
    /// 可选 per-source 权重（source 标识 → 权重，默认 1.0）。
    pub source_weights: std::collections::HashMap<String, f32>,
    pub enabled: bool,
}

impl Watch {
    /// 是否有任何可匹配条件。三者全空 = `watch-no-criteria`（route 层 400）。
    pub fn has_criteria(&self) -> bool {
        !self.keywords.is_empty() || !self.entities.is_empty() || self.anchor_vec.is_some()
    }
}

/// 监控匹配引擎读取的 item 轻量元数据 —— **不含 LLM 句柄**，纯数据。
///
/// `vector` 是入库时已算好的 item 向量（spec §8：匹配复用，不重新 embed）。`None` =
/// 该 item 还没 embed（退化为纯关键词/实体匹配）。
#[derive(Debug, Clone)]
pub struct ItemMeta {
    pub id: String,
    pub title: String,
    /// item 正文（用于关键词字面量匹配 + extractive 预览；调用方解密后传入）。
    pub content: String,
    /// 源标识（item.source_type，如 `rss` / `webdav` / `git` / `email`）。
    pub source_type: String,
    /// 入库时已算好的 item 向量（与 watch.anchor_vec 同模型）。
    pub vector: Option<Vec<f32>>,
    /// 已抽取的实体（复用 entities.rs；调用方传入避免重抽）。
    pub entities: Vec<Entity>,
    /// RFC3339 创建时间（recency 计分用）。
    pub created_at: String,
    /// item content_hash（跨源去重 O(1) 防线，store 层已防一层，这里做近似归并）。
    pub content_hash: String,
}

/// 一次 watch 命中（去重后），供 digest 聚合 + triage 排序。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchHit {
    pub watch_id: String,
    pub item_id: String,
    pub title: String,
    /// triage 综合分（match × recency × source_w × interaction），越高越靠前。
    pub score: f32,
    /// 可解释命中 / 排序理由（"关键词 'RVV' 命中" / "向量相似 0.71" / "7天内"）。
    pub reasons: Vec<String>,
    /// 近似去重组：同组 = 多源同内容。`None` = 唯一来源。
    pub dedup_group: Option<String>,
    pub created_at: String,
}

/// 调用方可注入的交互信号（browse_signals / citation_hit 计数），spec §3 triage 一项。
/// 默认全 0 = 无历史交互（不放大也不降低）。
#[derive(Debug, Clone, Default)]
pub struct InteractionSignals {
    /// item 被点击 / dwell 的次数（已有 browse_signals 表可填）。
    pub interaction_count: u32,
}

/// 监控匹配 + triage + 去重 引擎。**确定性，零 LLM**（签名编译期无 LLM 句柄）。
#[derive(Debug, Clone)]
pub struct WatchMatcher {
    /// 近似去重的向量相似度阈值（≥ 此值且标题归一相同视为同内容）。
    pub dedup_vec_threshold: f32,
    /// recency 半衰期（天）—— 越新分越高。
    pub recency_halflife_days: f64,
}

impl Default for WatchMatcher {
    fn default() -> Self {
        Self {
            // 与 spec match_threshold 量级对齐；近似去重要求很高相似度避免误并。
            dedup_vec_threshold: 0.92,
            recency_halflife_days: 14.0,
        }
    }
}

impl WatchMatcher {
    /// 增量匹配：仅对 `new_items` × `enabled watches` 算命中（spec §11 R3，非全表）。
    ///
    /// **签名不含 LLM provider** —— 编译期保证监控路径不偷跑 LLM。向量相似度复用
    /// `new_items[*].vector` 与 `watches[*].anchor_vec`（均为入库 / 建 watch 时已算好的向量），
    /// 不在此重新 embed。
    ///
    /// 返回去重后的命中（同 watch+item 不重复；近似同内容标 `dedup_group`），按 triage 分降序。
    ///
    /// `signals` 给每个 item_id 提供交互计数（可空 map → 全 0）。`now_rfc3339` 注入当前时间
    /// （测试可固定，保证确定性）。
    pub fn evaluate(
        &self,
        new_items: &[ItemMeta],
        watches: &[Watch],
        signals: &std::collections::HashMap<String, InteractionSignals>,
        now_rfc3339: &str,
    ) -> Vec<WatchHit> {
        let now = parse_rfc3339(now_rfc3339);
        let mut hits: Vec<WatchHit> = Vec::new();
        // 单 watch 内 (item_id) 去重防线（防同批次重复 item）。
        for watch in watches.iter().filter(|w| w.enabled && w.has_criteria()) {
            let mut seen_items: std::collections::HashSet<&str> = std::collections::HashSet::new();
            let mut watch_hits: Vec<WatchHit> = Vec::new();
            for item in new_items {
                if seen_items.contains(item.id.as_str()) {
                    continue;
                }
                // 源过滤：watch 绑了 source_ids 则 item 必须属于其一。
                if !watch.source_ids.is_empty()
                    && !watch.source_ids.iter().any(|s| s == &item.source_type)
                {
                    continue;
                }
                if let Some(hit) = self.match_one(watch, item, signals, now) {
                    seen_items.insert(item.id.as_str());
                    watch_hits.push(hit);
                }
            }
            // 近似去重：同 watch 内标题归一相同 + 向量高相似 → 同 dedup_group。
            self.assign_dedup_groups(&mut watch_hits, new_items);
            hits.extend(watch_hits);
        }
        // triage：综合分降序；同分按 recency（created_at 降序）决胜。
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.created_at.cmp(&a.created_at))
        });
        hits
    }

    /// 对单个 (watch, item) 算命中 + 可解释分。不命中（综合分 < 阈值）返回 None。
    fn match_one(
        &self,
        watch: &Watch,
        item: &ItemMeta,
        signals: &std::collections::HashMap<String, InteractionSignals>,
        now: f64,
    ) -> Option<WatchHit> {
        let mut reasons: Vec<String> = Vec::new();

        // (1) 关键词字面量匹配（不当 regex；spec adversarial：正则元字符当字面量）。
        // 巨量重复同一词不放大分（用 distinct 命中关键词数 / 关键词总数）。
        let content_lc = item.content.to_lowercase();
        let title_lc = item.title.to_lowercase();
        let mut kw_hit = 0usize;
        for kw in &watch.keywords {
            let kl = kw.to_lowercase();
            if kl.is_empty() {
                continue;
            }
            if content_lc.contains(&kl) || title_lc.contains(&kl) {
                kw_hit += 1;
                reasons.push(format!("关键词 '{kw}' 命中"));
            }
        }
        let kw_score = if watch.keywords.is_empty() {
            0.0
        } else {
            kw_hit as f32 / watch.keywords.len() as f32
        };

        // (2) 实体重叠（复用 entities.rs entity_overlap_score）。
        let ent_score = if watch.entities.is_empty() || item.entities.is_empty() {
            0.0
        } else {
            let s = entity_overlap_score(&watch.entities, &item.entities);
            if s > 0.0 {
                reasons.push(format!("实体重叠 {s:.2}"));
            }
            s
        };

        // (3) 向量相似度（复用已算好的向量；缺向量则 0，不重新 embed）。
        let vec_score = match (&watch.anchor_vec, &item.vector) {
            (Some(a), Some(b)) if a.len() == b.len() && !a.is_empty() => {
                let s = cosine(a, b).max(0.0);
                if s > 0.0 {
                    reasons.push(format!("向量相似 {s:.2}"));
                }
                s
            }
            _ => 0.0,
        };

        // 匹配分 = 三信号取最大（任一强信号即命中），更直觉可解释。
        let match_score = kw_score.max(ent_score).max(vec_score);
        if match_score < watch.match_threshold {
            return None;
        }

        // recency 衰减：以半衰期指数衰减，∈ (0,1]。
        let recency = recency_factor(&item.created_at, now, self.recency_halflife_days);
        reasons.push(recency_reason(&item.created_at, now));

        // 源权重（默认 1.0）。
        let source_w = watch
            .source_weights
            .get(&item.source_type)
            .copied()
            .unwrap_or(1.0)
            .max(0.0);
        if (source_w - 1.0).abs() > f32::EPSILON {
            reasons.push(format!("来源权重 {source_w:.1}"));
        }

        // 交互加成：1 + log1p(count)*0.1，温和不喧宾夺主。
        let inter = signals
            .get(&item.id)
            .map(|s| s.interaction_count)
            .unwrap_or(0);
        let inter_factor = if inter > 0 {
            reasons.push(format!("历史交互 {inter} 次"));
            1.0 + (inter as f32 + 1.0).ln() * 0.1
        } else {
            1.0
        };

        let score = match_score * recency as f32 * source_w * inter_factor;

        Some(WatchHit {
            watch_id: watch.id.clone(),
            item_id: item.id.clone(),
            title: item.title.clone(),
            score,
            reasons,
            dedup_group: None,
            created_at: item.created_at.clone(),
        })
    }

    /// 近似去重：同 watch 内若两 hit 的归一标题相同 / content_hash 相同 / 向量相似 ≥ 阈值
    /// → 同组。
    ///
    /// 性能（spec §11 R3）：exact 键（content_hash + 归一标题）走 hash bucket O(n)；向量
    /// 相似只在**同标题前缀 bucket 内**两两比较，不做全量 O(n²) cosine（大批量同 watch 命中
    /// 时全量 cosine 会炸）。归一标题预计算一次，避免内层循环重复 alloc。
    // union-find + 多个并行 vec（metas/norm_titles/group）按下标对齐索引，迭代器写法反而更晦涩。
    #[allow(clippy::needless_range_loop)]
    fn assign_dedup_groups(&self, hits: &mut [WatchHit], items: &[ItemMeta]) {
        let n = hits.len();
        if n < 2 {
            return;
        }
        let item_by_id: std::collections::HashMap<&str, &ItemMeta> =
            items.iter().map(|i| (i.id.as_str(), i)).collect();

        // union-find。
        let mut group: Vec<usize> = (0..n).collect();
        fn find(group: &mut [usize], mut x: usize) -> usize {
            while group[x] != x {
                group[x] = group[group[x]];
                x = group[x];
            }
            x
        }
        fn union(group: &mut [usize], a: usize, b: usize) {
            let ra = find(group, a);
            let rb = find(group, b);
            if ra != rb {
                group[rb] = ra;
            }
        }

        // 预计算每个 hit 对应 item 的 (content_hash, normalized_title, vector)。
        let metas: Vec<Option<&ItemMeta>> = hits
            .iter()
            .map(|h| item_by_id.get(h.item_id.as_str()).copied())
            .collect();
        let norm_titles: Vec<String> = metas
            .iter()
            .map(|m| m.map(|i| normalize_title(&i.title)).unwrap_or_default())
            .collect();

        // (1) content_hash bucket — O(n)。
        let mut by_hash: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for i in 0..n {
            if let Some(m) = metas[i] {
                if m.content_hash.is_empty() {
                    continue;
                }
                match by_hash.get(m.content_hash.as_str()) {
                    Some(&first) => union(&mut group, first, i),
                    None => {
                        by_hash.insert(m.content_hash.as_str(), i);
                    }
                }
            }
        }
        // (2) normalized-title bucket — O(n)，同时把同 bucket 成员收集供向量复核。
        let mut by_title: std::collections::HashMap<&str, Vec<usize>> =
            std::collections::HashMap::new();
        for i in 0..n {
            if norm_titles[i].is_empty() {
                continue;
            }
            by_title.entry(norm_titles[i].as_str()).or_default().push(i);
        }
        for members in by_title.values() {
            for w in members.windows(2) {
                union(&mut group, w[0], w[1]);
            }
        }
        // (3) 向量相似只在标题不同但内容可能近似的小集合内做 —— 仅当总 hit 数较小时
        //     才做全量两两 cosine（避免大批量 O(n²)）。大批量场景下 exact 键已足够。
        const VEC_PAIRWISE_LIMIT: usize = 256;
        if n <= VEC_PAIRWISE_LIMIT {
            for i in 0..n {
                let va = metas[i].and_then(|m| m.vector.as_ref());
                let Some(va) = va else { continue };
                for j in (i + 1)..n {
                    if find(&mut group, i) == find(&mut group, j) {
                        continue; // already same group
                    }
                    if let Some(vb) = metas[j].and_then(|m| m.vector.as_ref()) {
                        if va.len() == vb.len()
                            && !va.is_empty()
                            && cosine(va, vb) >= self.dedup_vec_threshold
                        {
                            union(&mut group, i, j);
                        }
                    }
                }
            }
        }

        // 仅给真正成组（size ≥ 2）的成员赋稳定 group id（= 组内最小 item_id）。
        let mut members: std::collections::HashMap<usize, Vec<usize>> =
            std::collections::HashMap::new();
        for i in 0..n {
            let r = find(&mut group, i);
            members.entry(r).or_default().push(i);
        }
        for idxs in members.values() {
            if idxs.len() < 2 {
                continue;
            }
            let gid = idxs
                .iter()
                .map(|&i| hits[i].item_id.clone())
                .min()
                .unwrap_or_default();
            for &i in idxs {
                hits[i].dedup_group = Some(format!("dg:{gid}"));
                hits[i].reasons.push("多源同内容".to_string());
            }
        }
    }
}

/// 标题归一：小写 + 去标点/空白，用于近似去重。
pub fn normalize_title(t: &str) -> String {
    t.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// cosine 相似度（与 search.rs 同实现，避免跨模块私有依赖）。
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// RFC3339 → epoch 秒（解析失败回 0，确定性退化）。
fn parse_rfc3339(s: &str) -> f64 {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.timestamp() as f64)
        .unwrap_or(0.0)
}

/// recency 因子 ∈ (0,1]：以 half-life 指数衰减。created_at 解析失败 → 1.0（不惩罚）。
fn recency_factor(created_at: &str, now: f64, halflife_days: f64) -> f64 {
    let created = parse_rfc3339(created_at);
    if created <= 0.0 || now <= 0.0 {
        return 1.0;
    }
    let age_days = ((now - created) / 86_400.0).max(0.0);
    0.5f64.powf(age_days / halflife_days.max(0.001))
}

fn recency_reason(created_at: &str, now: f64) -> String {
    let created = parse_rfc3339(created_at);
    if created <= 0.0 || now <= 0.0 {
        return "时间未知".to_string();
    }
    let age_days = ((now - created) / 86_400.0).max(0.0) as i64;
    if age_days <= 1 {
        "1天内".to_string()
    } else if age_days <= 7 {
        format!("{age_days}天内")
    } else {
        format!("{age_days}天前")
    }
}

#[cfg(test)]
mod tests;
