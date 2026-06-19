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

        // 跨源核实：有 LLM → 语义聚类 + 冲突检测（§2-G，schema-guided + 重试 + grounding，
        // 失败保守回退确定性）；无 LLM → 确定性精确归并。
        let claims = match llm {
            Some(llm) => self.verify_claims_llm(&docs, opts.max_claims, llm),
            None => self.verify_claims(&docs, opts.max_claims),
        };

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

    /// 跨源交叉验证（**LLM 语义步**，spec §2-G + §9.2 floor F1≥0.80）。
    ///
    /// 确定性 `verify_claims` 只能并同**精确同标题**的多源材料；现实里"同一事实"常以不同
    /// 措辞出现在不同源（RSS vs 云盘 vs web）。本方法让 LLM 把编号材料聚成 claim 簇，并对每簇
    /// 标 confirmed / single / conflicting：
    /// - **grounding 强制**（spec §11 R6 准确性北极星）：每簇 `doc_indices` 必须是真实材料下标
    ///   1..=N，且每条断言 trace 回这些下标对应的 reference —— **不编造源**。validator 拒绝
    ///   越界 / 空 / 重复下标；越界即重试（≤3）。
    /// - **保守裁决**：只有 ≥2 个**独立 reference**（去重后）覆盖同一簇才标 confirmed；LLM 说
    ///   confirmed 但去重后独立源 < 2 → 降级 single（不被 LLM 的乐观说法带偏，防误标）。
    /// - **失败兜底**（§4.5.E + §7）：JSON 不可解析 / 重试耗尽 / LLM 报错 → 回退确定性
    ///   `verify_claims`（保守精确归并），**绝不**因 LLM 不稳而误标或 panic。
    fn verify_claims_llm(
        &self,
        docs: &[&ResearchDoc],
        max_claims: usize,
        llm: &dyn LlmProvider,
    ) -> Vec<VerifiedClaim> {
        let deterministic = self.verify_claims(docs, max_claims);
        if docs.len() < 2 {
            // 单材料无跨源可言，确定性已足（且省一次 LLM 调用）。
            return deterministic;
        }

        let n = docs.len();
        let mut ctx = String::new();
        for (i, d) in docs.iter().enumerate() {
            let kind = match d.kind {
                SourceKind::Vault => "本地",
                SourceKind::Web => "网络",
            };
            ctx.push_str(&format!("[{}] ({kind}) {} — {}\n", i + 1, d.title, d.snippet));
        }

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "claims": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "text": {"type": "string"},
                            "doc_indices": {"type": "array", "items": {"type": "integer"}},
                            "verdict": {"type": "string", "enum": ["confirmed", "single", "conflicting"]}
                        },
                        "required": ["text", "doc_indices", "verdict"]
                    }
                }
            },
            "required": ["claims"]
        });
        let system = "你是跨源事实核实助手。给定若干编号材料（每条带来源类别 [n]），\
            把表达**同一事实主张**的材料聚成一个 claim 簇（即便措辞不同）。规则：\
            (1) 每个 claim 的 doc_indices 必须是材料编号（1 起），且只能包含真实出现的编号，不得编造；\
            (2) verdict: 同一主张被 ≥2 条**不同**材料覆盖=confirmed；仅 1 条=single；\
            不同材料对同一主张说法**相互矛盾**=conflicting；\
            (3) text 用中文简述该主张；(4) 不要把无关材料硬塞进一个簇。";
        let schema_system = format!(
            "{system}\n\n输出必须是符合此 schema 的合法 JSON（不要 markdown 围栏，不要多余文字）：\n{schema}"
        );
        let user = format!("材料：\n{ctx}\n\n请输出 JSON 格式的跨源核实结果。");

        // §4.5-B validator: JSON 可解析 + 每个 doc_index 在 1..=n（grounding）+ 非空。
        let validator = move |raw: &str| -> std::result::Result<(), String> {
            let parsed = parse_claim_clusters(raw).map_err(|e| format!("JSON parse: {e}"))?;
            if parsed.claims.is_empty() {
                return Err("claims empty".into());
            }
            for c in &parsed.claims {
                if c.doc_indices.is_empty() {
                    return Err("a claim has no doc_indices (grounding)".into());
                }
                for idx in &c.doc_indices {
                    if *idx < 1 || *idx as usize > n {
                        return Err(format!("doc_index {idx} out of range 1..={n}"));
                    }
                }
            }
            Ok(())
        };

        match llm.chat_with_retry(&schema_system, &user, 3, &validator) {
            Ok((raw, _usage)) => match parse_claim_clusters(&raw) {
                Ok(parsed) => self.build_claims_from_clusters(docs, &parsed.claims, max_claims),
                Err(e) => {
                    log::warn!("cross-source verify: parse after retry failed, fallback: {e}");
                    deterministic
                }
            },
            Err(e) => {
                log::warn!("cross-source verify LLM failed, fallback to deterministic: {e}");
                deterministic
            }
        }
    }

    /// 把 LLM 聚出的簇转成 [`VerifiedClaim`]，**确定性地重算 verdict**（防 LLM 误标）：
    /// 独立 reference 去重计数 ≥2 → confirmed；LLM 标 conflicting 且确有 ≥2 独立源 → 保留
    /// conflicting；其余 → single。源引用全部 trace 回真实材料（grounding）。
    fn build_claims_from_clusters(
        &self,
        docs: &[&ResearchDoc],
        clusters: &[ClaimCluster],
        max_claims: usize,
    ) -> Vec<VerifiedClaim> {
        let mut out: Vec<VerifiedClaim> = Vec::new();
        for c in clusters.iter().take(max_claims) {
            // 去重 doc_indices → 去重 reference（同一 reference 多次不算多源）。
            let mut seen_idx: std::collections::HashSet<usize> = std::collections::HashSet::new();
            let mut seen_ref: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut sources: Vec<ClaimSource> = Vec::new();
            for idx in &c.doc_indices {
                let i = *idx as usize;
                if i < 1 || i > docs.len() || !seen_idx.insert(i) {
                    continue;
                }
                let d = docs[i - 1];
                if seen_ref.insert(d.reference.clone()) {
                    sources.push(ClaimSource {
                        kind: d.kind,
                        reference: d.reference.clone(),
                    });
                }
            }
            if sources.is_empty() {
                continue; // grounding：簇无任何真实源 → 丢弃，绝不产无源 claim。
            }
            // 确定性重算 verdict：独立源 ≥2 才可能 confirmed/conflicting；否则 single。
            let verification = if sources.len() >= 2 {
                match c.verdict.as_str() {
                    "conflicting" => Verification::Conflicting,
                    _ => Verification::MultiSourceConfirmed,
                }
            } else {
                Verification::SingleSource
            };
            out.push(VerifiedClaim {
                text: if c.text.trim().is_empty() {
                    docs[(c.doc_indices[0] as usize).clamp(1, docs.len()) - 1]
                        .title
                        .clone()
                } else {
                    c.text.trim().to_string()
                },
                verification,
                sources,
            });
        }
        if out.is_empty() {
            // LLM 簇全被 grounding 丢弃 → 回退确定性，保证非空有结论。
            return self.verify_claims(docs, max_claims);
        }
        out
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

// ── 跨源验证 LLM schema 解析（schema-guided JSON）─────────────────────────────

#[derive(Debug, Deserialize)]
struct ClaimClusters {
    claims: Vec<ClaimCluster>,
}

#[derive(Debug, Deserialize)]
struct ClaimCluster {
    #[serde(default)]
    text: String,
    doc_indices: Vec<i64>,
    #[serde(default)]
    verdict: String,
}

/// 容错解析跨源验证 JSON（剥 markdown 围栏 + 截首个 `{`..末个 `}`，对齐 digest 解析）。
fn parse_claim_clusters(raw: &str) -> std::result::Result<ClaimClusters, String> {
    let cleaned = strip_json_fence(raw);
    serde_json::from_str::<ClaimClusters>(&cleaned).map_err(|e| e.to_string())
}

fn strip_json_fence(raw: &str) -> String {
    let s = raw.trim();
    let s = s
        .strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))
        .unwrap_or(s);
    let s = s.strip_suffix("```").unwrap_or(s);
    let s = s.trim();
    if let (Some(start), Some(end)) = (s.find('{'), s.rfind('}')) {
        if end >= start {
            return s[start..=end].to_string();
        }
    }
    s.to_string()
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
        // run() with ≥2 docs now calls the LLM twice: (1) cross-source verify, (2) synthesize.
        // Mock is FIFO → queue verify JSON first, then the narrative.
        llm.push_response(r#"{"claims":[{"text":"a","doc_indices":[1],"verdict":"single"},{"text":"b","doc_indices":[2],"verdict":"single"}]}"#);
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
        // two distinct sources, same fact → LLM clusters them → confirmed.
        let docs = vec![
            doc(SourceKind::Vault, "item-1", "RISC-V RVA23 ratified", "a"),
            doc(SourceKind::Web, "https://lwn.net", "risc-v rva23 ratified", "b"),
        ];
        let llm = MockLlmProvider::new("mock");
        llm.push_response(r#"{"claims":[{"text":"RVA23 ratified","doc_indices":[1,2],"verdict":"confirmed"}]}"#);
        llm.push_response("ok [1][2]"); // synthesize
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
        // even if the LLM clusters both indices, they map to the same reference → 1 independent
        // source → single (the conservative re-verdict must not be fooled).
        llm.push_response(r#"{"claims":[{"text":"dup","doc_indices":[1,2],"verdict":"confirmed"}]}"#);
        llm.push_response("ok [1]"); // synthesize
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

    // ── cross-source verification LLM agent (§2-G) ──────────────────────────
    //
    // These exercise the new LLM semantic clustering step that groups same-fact
    // materials across DIFFERENT wordings (which the deterministic exact-title path
    // cannot), with grounding + conservative re-verdict + graceful fallback.

    fn docrefs() -> Vec<ResearchDoc> {
        vec![
            doc(SourceKind::Vault, "item-1", "RVV 1.0 ratified", "The RVV vector ext was ratified."),
            doc(SourceKind::Web, "https://lwn.net/x", "Vector extension finalized", "RISC-V finalized its vector spec."),
            doc(SourceKind::Vault, "item-2", "Unrelated note", "Something else entirely."),
        ]
    }

    /// happy: LLM clusters two differently-worded sources into one confirmed claim
    /// (the deterministic path would have kept them separate as two single-source claims).
    #[test]
    fn xsource_llm_clusters_synonymous_sources_confirmed() {
        let docs = docrefs();
        let llm = MockLlmProvider::new("mock");
        // [1] and [2] are the same fact in different words; [3] is its own single source.
        llm.push_response(r#"{"claims":[
            {"text":"RISC-V vector extension was ratified","doc_indices":[1,2],"verdict":"confirmed"},
            {"text":"unrelated","doc_indices":[3],"verdict":"single"}
        ]}"#);
        // synthesize is also called by run(); queue a second response for it.
        llm.push_response("综述 [1][2][3]");
        let r = DeepResearch.run("topic", &docs, &ResearchOpts::default(), Some(&llm));
        let confirmed: Vec<_> = r.claims.iter()
            .filter(|c| c.verification == Verification::MultiSourceConfirmed).collect();
        assert_eq!(confirmed.len(), 1, "synonymous cross-source merged & confirmed");
        assert_eq!(confirmed[0].sources.len(), 2);
    }

    /// adversarial: LLM optimistically labels a single-source cluster "confirmed".
    /// Our deterministic re-verdict MUST downgrade it to single (never overclaim, §11 R6).
    #[test]
    fn xsource_llm_overclaim_downgraded_to_single() {
        let docs = docrefs();
        let llm = MockLlmProvider::new("mock");
        llm.push_response(r#"{"claims":[
            {"text":"lone fact","doc_indices":[3],"verdict":"confirmed"}
        ]}"#);
        llm.push_response("综述 [3]");
        let r = DeepResearch.run("t", &docs, &ResearchOpts::default(), Some(&llm));
        assert!(r.claims.iter().all(|c| c.verification != Verification::MultiSourceConfirmed),
            "single-source cluster cannot be confirmed even if LLM says so");
    }

    /// conflicting: LLM marks conflicting + there really are ≥2 independent sources → kept.
    #[test]
    fn xsource_llm_conflicting_preserved_when_multisource() {
        let docs = docrefs();
        let llm = MockLlmProvider::new("mock");
        llm.push_response(r#"{"claims":[
            {"text":"sources disagree on the date","doc_indices":[1,2],"verdict":"conflicting"}
        ]}"#);
        llm.push_response("综述 [1][2]");
        let r = DeepResearch.run("t", &docs, &ResearchOpts::default(), Some(&llm));
        assert!(r.claims.iter().any(|c| c.verification == Verification::Conflicting));
    }

    /// grounding (adversarial): LLM hallucinates an out-of-range doc index. The validator
    /// rejects all 3 attempts → graceful fallback to the deterministic claims (no fabricated source).
    #[test]
    fn xsource_llm_out_of_range_index_falls_back() {
        let docs = docrefs();
        let llm = MockLlmProvider::new("mock");
        // doc_index 9 does not exist (only 3 docs) → rejected each attempt.
        llm.push_response(r#"{"claims":[{"text":"x","doc_indices":[9],"verdict":"confirmed"}]}"#);
        llm.push_response(r#"{"claims":[{"text":"y","doc_indices":[8],"verdict":"confirmed"}]}"#);
        llm.push_response(r#"{"claims":[{"text":"z","doc_indices":[7],"verdict":"confirmed"}]}"#);
        llm.push_response("综述"); // synthesize
        let r = DeepResearch.run("t", &docs, &ResearchOpts::default(), Some(&llm));
        // fell back to deterministic: each distinct title is its own single-source claim,
        // and crucially every source ref is real (in the docs).
        let all_refs: Vec<&str> = r.claims.iter().flat_map(|c| c.sources.iter()).map(|s| s.reference.as_str()).collect();
        assert!(all_refs.iter().all(|rf| docs.iter().any(|d| d.reference == *rf)),
            "no fabricated source after fallback");
        assert!(!r.claims.is_empty());
    }

    /// error: LLM returns garbage 3× → fallback to deterministic, no panic.
    #[test]
    fn xsource_llm_garbage_falls_back() {
        let docs = docrefs();
        let llm = MockLlmProvider::new("mock");
        llm.push_response("not json");
        llm.push_response("still not json");
        llm.push_response("nope");
        llm.push_response("综述");
        let r = DeepResearch.run("t", &docs, &ResearchOpts::default(), Some(&llm));
        assert!(!r.claims.is_empty(), "deterministic fallback produced claims");
    }

    /// edge: single doc → no LLM cross-source call needed, deterministic single-source.
    #[test]
    fn xsource_single_doc_no_llm_clustering() {
        let docs = vec![doc(SourceKind::Vault, "item-1", "Solo", "x")];
        let llm = MockLlmProvider::new("mock");
        // only synthesize should consume a response; verify_claims_llm short-circuits at <2 docs.
        llm.push_response("综述 [1]");
        let r = DeepResearch.run("t", &docs, &ResearchOpts::default(), Some(&llm));
        assert_eq!(r.claims.len(), 1);
        assert_eq!(r.claims[0].verification, Verification::SingleSource);
    }

    /// grounding: same reference cited twice within one cluster must not count as multi-source.
    #[test]
    fn xsource_duplicate_reference_in_cluster_is_single() {
        let docs = vec![
            doc(SourceKind::Vault, "item-1", "A", "first wording"),
            doc(SourceKind::Vault, "item-1", "A again", "second wording, same item"),
        ];
        let llm = MockLlmProvider::new("mock");
        llm.push_response(r#"{"claims":[{"text":"same fact","doc_indices":[1,2],"verdict":"confirmed"}]}"#);
        llm.push_response("综述 [1][2]");
        let r = DeepResearch.run("t", &docs, &ResearchOpts::default(), Some(&llm));
        assert_eq!(r.claims[0].sources.len(), 1, "same reference deduped → single independent source");
        assert_eq!(r.claims[0].verification, Verification::SingleSource);
    }
}
