//! digest 构建 —— 被动可见的"你关注的新信息"摘要卡。
//!
//! 两态（spec §2-B）：
//! - **默认（零成本）**：标题列表 + 每条源引用 + extractive 抽取式一句话预览。
//!   [`DigestBuilder::build_default`] **签名无 LLM 句柄**（编译期反偷跑，spec §8 R1）。
//! - **LLM 摘要（可选）**：[`DigestBuilder::build_llm_summary`] 显式带 `&dyn LlmProvider`，
//!   套 §4.5（schema-guided + 重试-验证 + 源-grounded）。仅在 per-watch `llm_summary=1` +
//!   后台队列（可暂停）时调用。

use serde::{Deserialize, Serialize};

use crate::llm::LlmProvider;
use crate::monitoring::WatchHit;

/// digest 卡的一条条目（去重后；多源同内容合一）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestEntry {
    pub item_id: String,
    pub title: String,
    /// extractive 抽取式一句话预览（零 LLM）。
    pub preview: String,
    /// 该条涉及的源（dedup_group 内 > 1 = 多源）。
    pub sources: Vec<String>,
    pub score: f32,
    /// 可解释命中/排序理由（透传 WatchHit.reasons）。
    pub reasons: Vec<String>,
}

/// 一张 digest 卡（复用 suggestion card 范式：被动可见 + dismiss/mute）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestCard {
    pub watch_id: String,
    /// `kind="digest"`（接入 suggestion 卡体系时的 kind 字段）。
    pub kind: String,
    pub title: String,
    pub entries: Vec<DigestEntry>,
    /// 可选 LLM 摘要正文（默认路径为 None）；每条要点须源-grounded（带 [n] 引用）。
    pub llm_summary: Option<String>,
    pub created_at: String,
}

/// item 正文供给：digest 预览要读 item content（调用方解密后传入）。
/// 用 trait 让 core 不直接依赖 Store 解密，测试可注入内存映射。
pub trait ContentSource {
    /// 返回 item_id 的正文（解密后）。缺失返回 None（预览退化为标题）。
    fn content(&self, item_id: &str) -> Option<String>;
}

/// 内存 ContentSource（测试 / 调用方已聚合内容时用）。
#[derive(Debug, Default, Clone)]
pub struct MapContentSource(pub std::collections::HashMap<String, String>);

impl ContentSource for MapContentSource {
    fn content(&self, item_id: &str) -> Option<String> {
        self.0.get(item_id).cloned()
    }
}

/// digest 构建器。`preview_chars` 控制 extractive 预览长度。
#[derive(Debug, Clone)]
pub struct DigestBuilder {
    pub preview_chars: usize,
    pub max_entries: usize,
}

impl Default for DigestBuilder {
    fn default() -> Self {
        Self {
            preview_chars: 140,
            max_entries: 50,
        }
    }
}

impl DigestBuilder {
    /// 默认 digest：标题 + 源引用 + extractive 预览。**零 LLM**（签名无 LLM 句柄）。
    ///
    /// `hits` 应已按 triage 分降序（evaluate 已排好）。`content` 提供正文做 extractive 预览。
    /// dedup_group 合并：同组只保留分最高的一条，sources 汇总组内各 item 的源。
    // 参数较多是 spec §5 的固定 API 契约（watch_id/label/hits/content/sources/now），不抽 struct。
    #[allow(clippy::too_many_arguments)]
    pub fn build_default(
        &self,
        watch_id: &str,
        watch_label: &str,
        hits: &[WatchHit],
        content: &dyn ContentSource,
        hit_sources: &std::collections::HashMap<String, Vec<String>>,
        now_rfc3339: &str,
    ) -> DigestCard {
        let merged = merge_dedup_groups(hits);
        let mut entries: Vec<DigestEntry> = Vec::new();
        for hit in merged.iter().take(self.max_entries) {
            let preview = match content.content(&hit.item_id) {
                Some(body) => extractive_preview(&body, self.preview_chars),
                None => truncate_chars(&hit.title, self.preview_chars),
            };
            let sources = hit_sources
                .get(&hit.item_id)
                .cloned()
                .unwrap_or_default();
            entries.push(DigestEntry {
                item_id: hit.item_id.clone(),
                title: hit.title.clone(),
                preview,
                sources,
                score: hit.score,
                reasons: hit.reasons.clone(),
            });
        }
        DigestCard {
            watch_id: watch_id.to_string(),
            kind: "digest".to_string(),
            title: format!("「{watch_label}」新信息 {} 条", entries.len()),
            entries,
            llm_summary: None,
            created_at: now_rfc3339.to_string(),
        }
    }

    /// LLM 智能摘要 digest（显式开启 + 后台队列可暂停）。套 §4.5：schema-guided JSON +
    /// 重试-验证（≤3）+ 源-grounded（每条要点带 [n] item 引用）。失败 → 退化为默认 digest。
    ///
    /// **此方法是唯一带 `&dyn LlmProvider` 的 digest 入口** —— 编译期把"偷跑 LLM"挡在
    /// `build_default` 之外。
    #[allow(clippy::too_many_arguments)]
    pub fn build_llm_summary(
        &self,
        watch_id: &str,
        watch_label: &str,
        hits: &[WatchHit],
        content: &dyn ContentSource,
        hit_sources: &std::collections::HashMap<String, Vec<String>>,
        now_rfc3339: &str,
        llm: &dyn LlmProvider,
    ) -> DigestCard {
        // 始终先有零成本 digest 兜底（spec §7 graceful degradation）。
        let mut card =
            self.build_default(watch_id, watch_label, hits, content, hit_sources, now_rfc3339);
        if card.entries.is_empty() {
            return card;
        }

        // 构造带编号源的 context（每条 [n] = entry index，供 grounding 引用核验）。
        let mut ctx = String::new();
        for (i, e) in card.entries.iter().enumerate() {
            ctx.push_str(&format!("[{}] {} — {}\n", i + 1, e.title, e.preview));
        }

        let system = "你是一个信息监控助手。根据用户关注主题下的若干新条目（每条带编号 [n]），\
            产出简明的「本期要点」摘要。规则：(1) 每条要点必须基于给定条目，并在句末标注来源编号如 [1]；\
            (2) 不得编造未在条目中出现的事实；(3) 用中文，3-6 条要点。";
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "keypoints": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "text": {"type": "string"},
                            "refs": {"type": "array", "items": {"type": "integer"}}
                        },
                        "required": ["text", "refs"]
                    }
                }
            },
            "required": ["keypoints"]
        });
        let user = format!("关注主题：{watch_label}\n\n新条目：\n{ctx}\n\n请产出 JSON 格式的本期要点。");
        let n_entries = card.entries.len();

        // §4.5-B 重试-验证：JSON 可解析 + grounding（refs 落在 entry 范围内）。
        let validator = move |raw: &str| -> std::result::Result<(), String> {
            let parsed: KeyPoints = parse_keypoints(raw).map_err(|e| format!("JSON parse: {e}"))?;
            if parsed.keypoints.is_empty() {
                return Err("keypoints empty".into());
            }
            for kp in &parsed.keypoints {
                if kp.refs.is_empty() {
                    return Err("a keypoint has no source ref (grounding)".into());
                }
                for r in &kp.refs {
                    if *r < 1 || *r as usize > n_entries {
                        return Err(format!("ref {r} out of range 1..={n_entries}"));
                    }
                }
            }
            Ok(())
        };

        // schema-guided + 重试。chat_with_retry 把 validator error 反馈给 LLM 重试 ≤3。
        let schema_system = format!(
            "{system}\n\n输出必须是符合此 schema 的合法 JSON（不要 markdown 围栏，不要多余文字）：\n{schema}"
        );
        match llm.chat_with_retry(&schema_system, &user, 3, &validator) {
            Ok((raw, _usage)) => {
                if let Ok(kp) = parse_keypoints(&raw) {
                    let mut body = String::new();
                    for k in &kp.keypoints {
                        let refs: Vec<String> = k.refs.iter().map(|r| format!("[{r}]")).collect();
                        body.push_str(&format!("• {} {}\n", k.text.trim(), refs.join("")));
                    }
                    card.llm_summary = Some(body.trim_end().to_string());
                }
            }
            Err(e) => {
                log::warn!("digest LLM summary failed, falling back to extractive: {e}");
                // card.llm_summary 保持 None → 默认 digest 兜底（spec §9.2 fallback）。
            }
        }
        card
    }
}

#[derive(Debug, Deserialize)]
struct KeyPoints {
    keypoints: Vec<KeyPoint>,
}
#[derive(Debug, Deserialize)]
struct KeyPoint {
    text: String,
    refs: Vec<i64>,
}

/// 容错解析 keypoints JSON（剥 markdown 围栏 + 截首个 `{`..末个 `}`）。
fn parse_keypoints(raw: &str) -> std::result::Result<KeyPoints, String> {
    let cleaned = strip_json_fence(raw);
    serde_json::from_str::<KeyPoints>(&cleaned).map_err(|e| e.to_string())
}

fn strip_json_fence(raw: &str) -> String {
    let s = raw.trim();
    let s = s.strip_prefix("```json").or_else(|| s.strip_prefix("```")).unwrap_or(s);
    let s = s.strip_suffix("```").unwrap_or(s);
    let s = s.trim();
    if let (Some(start), Some(end)) = (s.find('{'), s.rfind('}')) {
        if end >= start {
            return s[start..=end].to_string();
        }
    }
    s.to_string()
}

/// 同 dedup_group 只保留分最高一条（已 triage 降序 → 取首遇）。无 group 的全保留。
fn merge_dedup_groups(hits: &[WatchHit]) -> Vec<WatchHit> {
    let mut seen_groups: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<WatchHit> = Vec::new();
    for h in hits {
        match &h.dedup_group {
            Some(g) => {
                if seen_groups.insert(g.clone()) {
                    out.push(h.clone());
                }
            }
            None => out.push(h.clone()),
        }
    }
    out
}

/// 零成本 extractive 预览：取首句 / 首段前 N 字（跳过空行 + markdown 标题符号）。
fn extractive_preview(body: &str, max_chars: usize) -> String {
    let first = body
        .lines()
        .map(|l| l.trim_start_matches(['#', '>', '-', '*', ' ']).trim())
        .find(|l| !l.is_empty())
        .unwrap_or("");
    // 截到首句号 / 句点（中英），否则截字数。
    let sentence_end = first
        .char_indices()
        .find(|(_, c)| matches!(c, '。' | '!' | '?' | '！' | '？'))
        .map(|(i, c)| i + c.len_utf8());
    let snippet = match sentence_end {
        Some(end) if end <= first.len() => &first[..end],
        _ => first,
    };
    truncate_chars(snippet, max_chars)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i >= max_chars {
            out.push('…');
            break;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlmProvider;
    use crate::monitoring::WatchHit;
    use std::collections::HashMap;

    const NOW: &str = "2026-06-19T00:00:00Z";

    fn hit(item_id: &str, title: &str, score: f32, group: Option<&str>) -> WatchHit {
        WatchHit {
            watch_id: "w".to_string(),
            item_id: item_id.to_string(),
            title: title.to_string(),
            score,
            reasons: vec!["关键词命中".to_string()],
            dedup_group: group.map(|g| g.to_string()),
            created_at: "2026-06-18T00:00:00Z".to_string(),
        }
    }

    fn content_map(pairs: &[(&str, &str)]) -> MapContentSource {
        MapContentSource(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
    }

    fn sources(pairs: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    // ── happy: default digest is zero-LLM, has previews + refs ──────────────

    #[test]
    fn happy_default_digest_extractive_preview() {
        let b = DigestBuilder::default();
        let hits = vec![hit("i1", "RVV news", 0.9, None), hit("i2", "More", 0.5, None)];
        let cm = content_map(&[
            ("i1", "RVV extension finalized today。 second sentence ignored."),
            ("i2", "Second item body here."),
        ]);
        let card = b.build_default("w", "RISC-V", &hits, &cm, &HashMap::new(), NOW);
        assert_eq!(card.kind, "digest");
        assert_eq!(card.entries.len(), 2);
        assert!(card.llm_summary.is_none(), "default path = zero LLM, no summary");
        assert!(card.entries[0].preview.contains("RVV extension finalized today"));
        assert!(!card.entries[0].preview.contains("second sentence"), "first sentence only");
    }

    #[test]
    fn happy_digest_carries_sources_multi() {
        let b = DigestBuilder::default();
        let hits = vec![hit("i1", "x", 0.9, None)];
        let src = sources(&[("i1", &["rss", "clouddrive"])]);
        let card = b.build_default("w", "L", &hits, &content_map(&[("i1", "body")]), &src, NOW);
        assert_eq!(card.entries[0].sources, vec!["rss", "clouddrive"]);
    }

    // ── edge: empty hits → empty card; missing content → title fallback ─────

    #[test]
    fn edge_no_hits_empty_card() {
        let card = DigestBuilder::default().build_default("w", "L", &[], &content_map(&[]), &HashMap::new(), NOW);
        assert!(card.entries.is_empty());
        assert!(card.llm_summary.is_none());
    }

    #[test]
    fn edge_missing_content_falls_back_to_title() {
        let hits = vec![hit("i1", "Title Only", 0.9, None)];
        let card = DigestBuilder::default().build_default("w", "L", &hits, &content_map(&[]), &HashMap::new(), NOW);
        assert!(card.entries[0].preview.contains("Title Only"));
    }

    // ── dedup: same group collapses to one entry ───────────────────────────

    #[test]
    fn dedup_group_collapses_to_one_entry() {
        // hits already triage-sorted desc; same group → keep highest only.
        let hits = vec![
            hit("a", "dup A", 0.9, Some("dg:a")),
            hit("b", "dup B", 0.5, Some("dg:a")),
            hit("c", "unique", 0.7, None),
        ];
        let cm = content_map(&[("a", "body a"), ("b", "body b"), ("c", "body c")]);
        let card = DigestBuilder::default().build_default("w", "L", &hits, &cm, &HashMap::new(), NOW);
        assert_eq!(card.entries.len(), 2, "group dg:a collapsed to 1 + unique = 2");
        assert!(card.entries.iter().any(|e| e.item_id == "a"), "kept highest-score group member");
        assert!(!card.entries.iter().any(|e| e.item_id == "b"));
    }

    // ── max_entries cap (resource) ─────────────────────────────────────────

    #[test]
    fn resource_max_entries_cap() {
        let b = DigestBuilder { max_entries: 3, ..Default::default() };
        let hits: Vec<WatchHit> = (0..20)
            .map(|i| hit(&format!("i{i}"), &format!("t{i}"), 1.0 - i as f32 * 0.01, None))
            .collect();
        let cm = content_map(&[]);
        let card = b.build_default("w", "L", &hits, &cm, &HashMap::new(), NOW);
        assert_eq!(card.entries.len(), 3);
    }

    // ── LLM summary: grounded keypoints with refs ──────────────────────────

    #[test]
    fn llm_summary_grounded_keypoints() {
        let b = DigestBuilder::default();
        let hits = vec![hit("i1", "Item one", 0.9, None), hit("i2", "Item two", 0.8, None)];
        let cm = content_map(&[("i1", "body one"), ("i2", "body two")]);
        let llm = MockLlmProvider::new("mock");
        llm.push_response(r#"{"keypoints":[{"text":"point about one","refs":[1]},{"text":"point about two","refs":[2]}]}"#);
        let card = b.build_llm_summary("w", "L", &hits, &cm, &HashMap::new(), NOW, &llm);
        let summary = card.llm_summary.expect("summary present");
        assert!(summary.contains("point about one"));
        assert!(summary.contains("[1]"));
        assert!(summary.contains("[2]"));
    }

    #[test]
    fn llm_summary_strips_markdown_fence() {
        let b = DigestBuilder::default();
        let hits = vec![hit("i1", "Item one", 0.9, None)];
        let cm = content_map(&[("i1", "body one")]);
        let llm = MockLlmProvider::new("mock");
        llm.push_response("```json\n{\"keypoints\":[{\"text\":\"pt\",\"refs\":[1]}]}\n```");
        let card = b.build_llm_summary("w", "L", &hits, &cm, &HashMap::new(), NOW, &llm);
        assert!(card.llm_summary.unwrap().contains("pt"));
    }

    // ── error: bad LLM output exhausts retry → graceful fallback to extractive ──

    #[test]
    fn error_llm_garbage_falls_back_to_default() {
        let b = DigestBuilder::default();
        let hits = vec![hit("i1", "Item one", 0.9, None)];
        let cm = content_map(&[("i1", "body one")]);
        let llm = MockLlmProvider::new("mock");
        // 3 garbage responses → retry exhausted.
        llm.push_response("not json at all");
        llm.push_response("still not json");
        llm.push_response("nope");
        let card = b.build_llm_summary("w", "L", &hits, &cm, &HashMap::new(), NOW, &llm);
        assert!(card.llm_summary.is_none(), "garbage → fall back to extractive (no summary)");
        assert_eq!(card.entries.len(), 1, "default digest entries still present");
    }

    #[test]
    fn error_llm_ref_out_of_range_rejected_then_falls_back() {
        let b = DigestBuilder::default();
        let hits = vec![hit("i1", "Only one item", 0.9, None)];
        let cm = content_map(&[("i1", "body")]);
        let llm = MockLlmProvider::new("mock");
        // ref 5 is out of range (only 1 entry) → validator rejects all 3 attempts.
        llm.push_response(r#"{"keypoints":[{"text":"hallucinated","refs":[5]}]}"#);
        llm.push_response(r#"{"keypoints":[{"text":"still bad","refs":[9]}]}"#);
        llm.push_response(r#"{"keypoints":[{"text":"bad again","refs":[3]}]}"#);
        let card = b.build_llm_summary("w", "L", &hits, &cm, &HashMap::new(), NOW, &llm);
        assert!(card.llm_summary.is_none(), "out-of-range ref = ungrounded → rejected");
    }

    #[test]
    fn error_llm_no_refs_rejected() {
        let b = DigestBuilder::default();
        let hits = vec![hit("i1", "Item", 0.9, None)];
        let cm = content_map(&[("i1", "body")]);
        let llm = MockLlmProvider::new("mock");
        llm.push_response(r#"{"keypoints":[{"text":"ungrounded claim","refs":[]}]}"#);
        llm.push_response(r#"{"keypoints":[{"text":"still ungrounded","refs":[]}]}"#);
        llm.push_response(r#"{"keypoints":[{"text":"again","refs":[]}]}"#);
        let card = b.build_llm_summary("w", "L", &hits, &cm, &HashMap::new(), NOW, &llm);
        assert!(card.llm_summary.is_none(), "empty refs = ungrounded → rejected");
    }
}
