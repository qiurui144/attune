//! 深度研究 orchestrator —— 用户**显式触发**的多源搜索 + 抽取 + 综合 + 跨源核实。
//!
//! spec §2-F/§2-G。本模块是**编排层**，不重造底座：
//! - 多源搜：调用方传入 vault RAG 命中 + 可选 web 命中（[`ResearchSource`]）。
//! - 综合：`&dyn LlmProvider` 把命中 reduce 成带引用的研究报告（显式 LLM）。
//! - 跨源核实：claim → 源 set 计数 **确定性**；判定两段是否同一 claim 的语义步走 LLM
//!   （失败保守标 `single_source`，不误标 confirmed —— spec §11 R6）。
//!
//! 成本契约：本模块**仅在用户显式发起深研时调用**，不在监控/digest 默认路径出现。

use serde::{Deserialize, Serialize};

use crate::llm::LlmProvider;

/// 一条研究材料（来自 vault 或 web）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchDoc {
    /// 来源类别。
    pub kind: SourceKind,
    /// 引用标识（vault: item_id；web: url）。
    pub reference: String,
    pub title: String,
    /// 已抽取 / 预裁的正文片段（调用方用 extractive 预裁省 token）。
    pub snippet: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Vault,
    Web,
}

/// 跨源核实状态（spec §5 DeepResearchResponse.claims）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    /// ≥2 独立源覆盖同一 claim。
    MultiSourceConfirmed,
    /// 仅单源。
    SingleSource,
    /// 多源冲突。
    Conflicting,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiedClaim {
    pub text: String,
    pub verification: Verification,
    /// 支撑该 claim 的源引用（去重后）。
    pub sources: Vec<ClaimSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimSource {
    pub kind: SourceKind,
    pub reference: String,
}

/// 一份研究报告（叙述 + 引用 + 跨源核实标注）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchReport {
    pub topic: String,
    /// 叙述式报告正文（markdown，带 [n] 引用）。
    pub report_markdown: String,
    pub claims: Vec<VerifiedClaim>,
    /// 是否因 LLM 不可用退化为"仅检索列表"（无综合段）。
    pub degraded: bool,
}

/// 深研选项。
#[derive(Debug, Clone)]
pub struct ResearchOpts {
    /// 是否启用 web 源（OutboundGate WebSearch 被禁时调用方传 false → 仅 vault）。
    pub use_web: bool,
    /// claim 跨源核实的源数上限（防失控）。
    pub max_claims: usize,
}

impl Default for ResearchOpts {
    fn default() -> Self {
        Self {
            use_web: true,
            max_claims: 12,
        }
    }
}

/// 深研 orchestrator。
#[derive(Debug, Default, Clone)]
pub struct DeepResearch;

impl DeepResearch {
    /// 执行深研：用已抽取的 `docs`（vault RAG + web 命中）综合成报告。
    ///
    /// `llm` 显式传入（用户已点"深度研究"）。LLM 不可用 → 退化为检索列表报告（degraded=true，
    /// 不 panic，spec §7）。跨源核实：claim → 源 set 由确定性聚合（[`Self::verify_claims`]），
    /// 同义合并的 LLM 步留给调用方/未来（本 sprint 保守：精确 reference 去重 + 多源计数）。
    pub fn run(
        &self,
        topic: &str,
        docs: &[ResearchDoc],
        opts: &ResearchOpts,
        llm: Option<&dyn LlmProvider>,
    ) -> ResearchReport {
        let docs: Vec<&ResearchDoc> = docs
            .iter()
            .filter(|d| opts.use_web || d.kind != SourceKind::Web)
            .collect();

        // 没有任何材料 → 空报告（非错误）。
        if docs.is_empty() {
            return ResearchReport {
                topic: topic.to_string(),
                report_markdown: format!("# {topic}\n\n（未找到相关材料）"),
                claims: vec![],
                degraded: llm.is_none(),
            };
        }

        // 跨源核实（确定性聚合）。
        let claims = self.verify_claims(&docs, opts.max_claims);

        // 综合：有 LLM → 叙述报告；无 LLM → 退化为检索列表。
        match llm {
            Some(llm) => {
                let body = self.synthesize(topic, &docs, llm);
                ResearchReport {
                    topic: topic.to_string(),
                    report_markdown: body,
                    claims,
                    degraded: false,
                }
            }
            None => ResearchReport {
                topic: topic.to_string(),
                report_markdown: self.degraded_report(topic, &docs),
                claims,
                degraded: true,
            },
        }
    }

    /// 跨源核实（确定性）：把材料按 reference 归并；多个不同源覆盖 → multi_source_confirmed。
    /// 本 sprint 的 claim 粒度 = 每篇材料一条概要 claim（spec 保守版：精确 reference 计数，
    /// 同义语义合并留 v.next / 调用方 LLM 子步）。
    fn verify_claims(&self, docs: &[&ResearchDoc], max_claims: usize) -> Vec<VerifiedClaim> {
        // 以 normalize(title) 作 claim 键：同标题多源 → 多源确认。
        let mut by_key: std::collections::HashMap<String, Vec<&ResearchDoc>> =
            std::collections::HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for d in docs {
            let key = normalize(&d.title);
            if !by_key.contains_key(&key) {
                order.push(key.clone());
            }
            by_key.entry(key).or_default().push(d);
        }
        let mut claims: Vec<VerifiedClaim> = Vec::new();
        for key in order.iter().take(max_claims) {
            let group = &by_key[key];
            // 去重源 reference（同一 reference 多次不算多源）。
            let mut sources: Vec<ClaimSource> = Vec::new();
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            for d in group {
                if seen.insert(d.reference.clone()) {
                    sources.push(ClaimSource {
                        kind: d.kind,
                        reference: d.reference.clone(),
                    });
                }
            }
            // 独立源数 = 去重 reference 数。≥2 → confirmed；否则 single。
            // 保守：不臆造 conflicting（需语义判定，本 sprint 不误标）。
            let verification = if sources.len() >= 2 {
                Verification::MultiSourceConfirmed
            } else {
                Verification::SingleSource
            };
            claims.push(VerifiedClaim {
                text: group[0].title.clone(),
                verification,
                sources,
            });
        }
        claims
    }

    /// LLM 综合：把材料 reduce 成带 [n] 引用的叙述报告。schema-free 但要求带引用。
    fn synthesize(&self, topic: &str, docs: &[&ResearchDoc], llm: &dyn LlmProvider) -> String {
        let mut ctx = String::new();
        for (i, d) in docs.iter().enumerate() {
            let kind = match d.kind {
                SourceKind::Vault => "本地",
                SourceKind::Web => "网络",
            };
            ctx.push_str(&format!("[{}] ({kind}) {} — {}\n", i + 1, d.title, d.snippet));
        }
        let system = "你是研究助手。基于给定的编号材料，就用户主题写一份简明研究综述。\
            规则：(1) 每个论断后用 [n] 标注来源编号；(2) 只用材料中出现的信息，不编造；\
            (3) 用中文 markdown，含简短结论。";
        let user = format!("主题：{topic}\n\n材料：\n{ctx}\n\n请写出带引用的研究综述。");
        match llm.chat(system, &user) {
            Ok((answer, _usage)) => format!("# {topic}\n\n{}", answer.trim()),
            Err(e) => {
                log::warn!("deep research synthesize failed: {e}");
                self.degraded_report(topic, docs)
            }
        }
    }

    /// 退化报告：LLM 不可用时只列检索命中（明示无综合段，spec §7）。
    fn degraded_report(&self, topic: &str, docs: &[&ResearchDoc]) -> String {
        let mut s = format!(
            "# {topic}\n\n（LLM 不可用，仅提供检索结果，无综合段）\n\n"
        );
        for (i, d) in docs.iter().enumerate() {
            s.push_str(&format!("{}. **{}** — {}\n", i + 1, d.title, d.snippet));
        }
        s
    }
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlmProvider;

    fn doc(kind: SourceKind, reference: &str, title: &str, snippet: &str) -> ResearchDoc {
        ResearchDoc {
            kind,
            reference: reference.to_string(),
            title: title.to_string(),
            snippet: snippet.to_string(),
        }
    }

    // ── happy: synthesize with LLM, claims verified ────────────────────────

    #[test]
    fn happy_synthesize_with_llm() {
        let llm = MockLlmProvider::new("mock");
        llm.push_response("综述结论 [1][2]。");
        let docs = vec![
            doc(SourceKind::Vault, "item-1", "Topic A", "vault snippet"),
            doc(SourceKind::Web, "https://x.com", "Topic B", "web snippet"),
        ];
        let r = DeepResearch.run("my topic", &docs, &ResearchOpts::default(), Some(&llm));
        assert!(!r.degraded);
        assert!(r.report_markdown.contains("综述结论"));
        assert!(r.report_markdown.starts_with("# my topic"));
    }

    #[test]
    fn happy_multi_source_confirmed() {
        // two distinct sources, same normalized title → confirmed.
        let docs = vec![
            doc(SourceKind::Vault, "item-1", "RISC-V RVA23 ratified", "a"),
            doc(SourceKind::Web, "https://lwn.net", "risc-v rva23 ratified", "b"),
        ];
        let llm = MockLlmProvider::new("mock");
        llm.push_response("ok [1][2]");
        let r = DeepResearch.run("t", &docs, &ResearchOpts::default(), Some(&llm));
        assert_eq!(r.claims.len(), 1, "same claim merged across two sources");
        assert_eq!(r.claims[0].verification, Verification::MultiSourceConfirmed);
        assert_eq!(r.claims[0].sources.len(), 2);
    }

    #[test]
    fn single_source_not_overclaimed() {
        let docs = vec![doc(SourceKind::Vault, "item-1", "Lone claim", "x")];
        let llm = MockLlmProvider::new("mock");
        llm.push_response("ok [1]");
        let r = DeepResearch.run("t", &docs, &ResearchOpts::default(), Some(&llm));
        assert_eq!(r.claims[0].verification, Verification::SingleSource, "never overclaim as confirmed");
    }

    #[test]
    fn same_reference_twice_is_single_source() {
        // same reference appearing twice must NOT count as multi-source.
        let docs = vec![
            doc(SourceKind::Vault, "item-1", "Dup ref claim", "first"),
            doc(SourceKind::Vault, "item-1", "dup ref claim", "second"),
        ];
        let llm = MockLlmProvider::new("mock");
        llm.push_response("ok [1]");
        let r = DeepResearch.run("t", &docs, &ResearchOpts::default(), Some(&llm));
        assert_eq!(r.claims[0].sources.len(), 1, "deduped reference");
        assert_eq!(r.claims[0].verification, Verification::SingleSource);
    }

    // ── edge: empty docs ───────────────────────────────────────────────────

    #[test]
    fn edge_no_docs_empty_report() {
        let llm = MockLlmProvider::new("mock");
        let r = DeepResearch.run("topic", &[], &ResearchOpts::default(), Some(&llm));
        assert!(r.claims.is_empty());
        assert!(r.report_markdown.contains("未找到相关材料"));
    }

    // ── degraded: web disabled / LLM unavailable ───────────────────────────

    #[test]
    fn degraded_web_disabled_filters_web_docs() {
        let docs = vec![
            doc(SourceKind::Vault, "item-1", "Vault", "v"),
            doc(SourceKind::Web, "https://x", "Web", "w"),
        ];
        let opts = ResearchOpts { use_web: false, ..Default::default() };
        let llm = MockLlmProvider::new("mock");
        llm.push_response("ok [1]");
        let r = DeepResearch.run("t", &docs, &opts, Some(&llm));
        // only the vault claim survives.
        assert_eq!(r.claims.len(), 1);
        assert!(r.claims[0].sources.iter().all(|s| s.kind == SourceKind::Vault));
    }

    #[test]
    fn degraded_no_llm_returns_retrieval_list() {
        let docs = vec![doc(SourceKind::Vault, "item-1", "Some doc", "snippet here")];
        let r = DeepResearch.run("t", &docs, &ResearchOpts::default(), None);
        assert!(r.degraded);
        assert!(r.report_markdown.contains("LLM 不可用"));
        assert!(r.report_markdown.contains("Some doc"));
        // claims still computed deterministically.
        assert_eq!(r.claims.len(), 1);
    }

    #[test]
    fn error_llm_fails_falls_back_to_degraded_report() {
        let docs = vec![doc(SourceKind::Vault, "item-1", "Doc", "snip")];
        let llm = MockLlmProvider::new("mock"); // no responses queued → chat() errors
        let r = DeepResearch.run("t", &docs, &ResearchOpts::default(), Some(&llm));
        // synthesize falls back to degraded_report on LLM error (no panic).
        assert!(r.report_markdown.contains("Doc"));
    }

    // ── resource: max_claims cap ───────────────────────────────────────────

    #[test]
    fn resource_max_claims_cap() {
        let docs: Vec<ResearchDoc> = (0..50)
            .map(|i| doc(SourceKind::Vault, &format!("item-{i}"), &format!("Title {i}"), "s"))
            .collect();
        let opts = ResearchOpts { max_claims: 5, use_web: true };
        let r = DeepResearch.run("t", &docs, &opts, None);
        assert_eq!(r.claims.len(), 5, "capped at max_claims");
    }
}
