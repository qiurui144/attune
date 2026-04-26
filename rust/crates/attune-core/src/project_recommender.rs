//! Project 推荐归类引擎（spec §2.3）
//!
//! 两种触发：
//! - 文件上传成功 → recommend_for_file 算实体重叠度，> 0.6 推荐
//! - chat 用户消息含触发关键词 → recommend_for_chat 提示用户"是否要找/建 Project"
//!
//! 推荐结果**不持久化**：通过 WebSocket 推送给前端；前端如果错过，下次同样路径再算即可。

use crate::entities::{entity_overlap_score, Entity};
use crate::error::Result;
use crate::store::Store;
use serde::{Deserialize, Serialize};

/// 单条推荐候选（一个 Project 是否值得归到该 Project）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecommendationCandidate {
    pub project_id: String,
    pub project_title: String,
    /// Jaccard 相似度（0.0-1.0）
    pub score: f32,
    /// 触发的实体重叠（最相关的 top 5）
    pub overlapping_entities: Vec<String>,
}

/// chat 关键词触发结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTriggerHint {
    /// 命中的关键词
    pub matched_keywords: Vec<String>,
    /// 提示文案（前端可显示气泡）
    pub suggestion: String,
}

/// spec §2.3 阈值
pub const RECOMMEND_THRESHOLD: f32 = 0.6;

/// 触发关键词（中文常见关于"案件 / 客户 / 项目"语义）。
const CHAT_TRIGGER_KEYWORDS: &[&str] = &["案件", "案号", "客户", "项目", "诉讼", "案子"];

/// 给一份新文件（或新 chunk）算应该归到哪个 Project。
///
/// 参数 `project_entities` 是为了避免 recommender 在调用方代价巨大的 join：
/// route handler 调用前先从 items 表 + project_file 表组装好每个 active project 的
/// entities Vec，然后传入。如为 None，recommender fall back 走简化路径返回空。
///
/// 返回的 candidates 已按 score 降序排列，仅包含 score >= 阈值的项。
pub fn recommend_for_file(
    _store: &Store,
    _new_file_id: &str,
    new_file_entities: &[Entity],
    project_entities: Option<Vec<(&String, Vec<Entity>)>>,
) -> Result<Vec<RecommendationCandidate>> {
    let projects = match project_entities {
        Some(v) => v,
        None => return Ok(Vec::new()),
    };

    let mut out = Vec::new();
    for (pid, ents) in projects {
        let score = entity_overlap_score(new_file_entities, &ents);
        if score >= RECOMMEND_THRESHOLD {
            // 计算重叠的实体（最多 5 个，方便前端显示）
            use std::collections::HashSet;
            let new_set: HashSet<_> = new_file_entities
                .iter()
                .map(|e| (e.kind, e.value.clone()))
                .collect();
            let overlap: Vec<String> = ents
                .iter()
                .filter(|e| new_set.contains(&(e.kind, e.value.clone())))
                .take(5)
                .map(|e| format!("{:?}: {}", e.kind, e.value))
                .collect();

            // project_title 由调用方在 route 层补；这里给空，因为 recommender 不持有 store query 责任
            out.push(RecommendationCandidate {
                project_id: pid.clone(),
                project_title: String::new(),
                score,
                overlapping_entities: overlap,
            });
        }
    }

    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

/// 给一段 chat 用户消息检测是否含 Project 触发关键词。
///
/// 不调 LLM，纯关键词匹配。命中即返回 ChatTriggerHint。无命中返回 None。
pub fn recommend_for_chat(message: &str) -> Option<ChatTriggerHint> {
    let mut matched = Vec::new();
    for kw in CHAT_TRIGGER_KEYWORDS {
        if message.contains(kw) {
            matched.push(kw.to_string());
        }
    }
    if matched.is_empty() {
        None
    } else {
        Some(ChatTriggerHint {
            matched_keywords: matched.clone(),
            suggestion: format!(
                "看起来你提到了 {} — 是否要把当前对话或最近上传的文件归到一个 Project？",
                matched.join(" / ")
            ),
        })
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use crate::entities::extract_entities;

    #[test]
    fn threshold_constant() {
        assert!((RECOMMEND_THRESHOLD - 0.6).abs() < 1e-6);
    }

    #[test]
    fn chat_keyword_basic() {
        let h = recommend_for_chat("帮我整理这个案件的证据").expect("hit");
        assert!(h.matched_keywords.contains(&"案件".to_string()));
    }

    #[test]
    fn chat_keyword_multiple() {
        let h = recommend_for_chat("这个客户的项目我们整理一下").expect("hit");
        assert!(h.matched_keywords.contains(&"客户".to_string()));
        assert!(h.matched_keywords.contains(&"项目".to_string()));
    }

    #[test]
    fn chat_no_keyword() {
        assert!(recommend_for_chat("今天天气怎样").is_none());
    }

    #[test]
    fn recommend_for_file_empty_projects() {
        let store = Store::open_memory().expect("open");
        let new_ents = extract_entities("test");
        let r = recommend_for_file(&store, "f1", &new_ents, None).expect("ok");
        assert!(r.is_empty());

        let r = recommend_for_file(&store, "f1", &new_ents, Some(vec![])).expect("ok");
        assert!(r.is_empty());
    }
}
