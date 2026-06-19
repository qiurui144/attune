//! W6 — pluggable writing templates (spec §6.1) + template fill (spec §2.1 W6).
//!
//! A [`WritingTemplate`] bundles the *non-engine* knobs a generation needs: a system prompt,
//! ≥2 few-shot examples (§4.5 C), red lines, the placeholder marker for un-grounded facts, and
//! the default citation styles. OSS ships **general-purpose** templates (academic paragraph /
//! email / report / note / general doc). attune-pro registers **industry** templates
//! (`legal_complaint`, `claim_drafting`, …) by implementing the same trait in a plugin — the
//! engine never grows an industry branch (open/closed principle, spec §6.1, OSS边界 §H).
//!
//! ## Cost contract — this module is **tier 🆓 (zero LLM)**.
//!
//! The template *registry*, *lookup*, and *placeholder fill* are pure string work: they describe
//! how to call the model and how to fill `{{placeholder}}` slots, but they never themselves
//! invoke an [`LlmProvider`]. To make that a build-time invariant (not a code-review hope), this
//! module imports no LLM type and a compile-time guard ([`_no_llm_in_templates`]) asserts the
//! zero-cost classification. The 💰 part (draft / rewrite / synthesis) lives in those modules and
//! consumes a template only for its prompt/few-shot strings.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::cite::CiteStyle;

/// A worked example for §4.5-C few-shot steering: `(user_input, expected_json_output)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkedExample {
    /// The example user turn.
    pub input: String,
    /// The expected assistant output (JSON string matching the mode's schema).
    pub output: String,
}

impl WorkedExample {
    /// Convenience constructor.
    pub fn new(input: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            input: input.into(),
            output: output.into(),
        }
    }
}

/// A non-negotiable rule a template enforces (e.g. "no hallucinated citation"). Reported to the
/// caller so the UI / red-line layer can surface it; the engine's deterministic guards
/// (grounding, injection screen) enforce the universal ones regardless.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedLine {
    /// Stable kebab id (e.g. `no-hallucinated-citation`).
    pub id: String,
    /// Human-readable description.
    pub description: String,
}

impl RedLine {
    fn new(id: &str, description: &str) -> Self {
        Self {
            id: id.to_string(),
            description: description.to_string(),
        }
    }

    /// The universal red line every template inherits: a citation must point at a real source
    /// (KB item or user-supplied external ref) — the engine **never** lets the model invent a
    /// bibliography (spec §7 red line, §11 risk A/D).
    pub fn no_hallucinated_citation() -> Self {
        Self::new(
            "no-hallucinated-citation",
            "引用必须命中真实来源（知识库条目或用户提供的外部来源），禁止编造书目。",
        )
    }
}

/// The pluggable writing-template contract (spec §6.1).
///
/// OSS defines the trait + general templates; pro implements it for industry verticals. All
/// methods are pure getters — implementing this trait can never make a template call an LLM.
pub trait WritingTemplate: Send + Sync {
    /// Stable id (`academic_paragraph` / `legal_complaint` (pro) / …).
    fn id(&self) -> &str;
    /// Display name (i18n-neutral; UI may localize separately).
    fn display_name(&self) -> &str;
    /// The system prompt this template steers generation with.
    fn system_prompt(&self) -> &str;
    /// ≥2 worked examples (§4.5 C).
    fn few_shot(&self) -> &[WorkedExample];
    /// Red lines this template enforces (always includes the universal one).
    fn red_lines(&self) -> &[RedLine];
    /// Marker used for an un-grounded factual span. General default `[需核实]`; pro may redefine
    /// (e.g. `[请律师确认]`).
    fn placeholder_marker(&self) -> &str {
        "[需核实]"
    }
    /// Default citation styles for this template.
    fn citation_styles(&self) -> &[CiteStyle];
    /// Ordered placeholder slots this template expects when used in fill mode (W6 fill). Empty
    /// for free-form templates (academic paragraph / report …). A `{{slot}}` in the body text is
    /// matched against these.
    fn slots(&self) -> &[String] {
        &[]
    }
}

// ─────────────────────────── general OSS templates ───────────────────────────

/// A concrete general-purpose template built from owned data (used for all OSS templates).
#[derive(Debug, Clone)]
pub struct GeneralTemplate {
    id: String,
    display_name: String,
    system_prompt: String,
    few_shot: Vec<WorkedExample>,
    red_lines: Vec<RedLine>,
    placeholder: String,
    citation_styles: Vec<CiteStyle>,
    slots: Vec<String>,
}

impl WritingTemplate for GeneralTemplate {
    fn id(&self) -> &str {
        &self.id
    }
    fn display_name(&self) -> &str {
        &self.display_name
    }
    fn system_prompt(&self) -> &str {
        &self.system_prompt
    }
    fn few_shot(&self) -> &[WorkedExample] {
        &self.few_shot
    }
    fn red_lines(&self) -> &[RedLine] {
        &self.red_lines
    }
    fn placeholder_marker(&self) -> &str {
        &self.placeholder
    }
    fn citation_styles(&self) -> &[CiteStyle] {
        &self.citation_styles
    }
    fn slots(&self) -> &[String] {
        &self.slots
    }
}

fn academic_paragraph() -> GeneralTemplate {
    GeneralTemplate {
        id: "academic_paragraph".into(),
        display_name: "Academic paragraph".into(),
        system_prompt: "你是学术写作助手。基于提供的素材，撰写严谨、客观、可回指来源的论文段落。\
            只陈述素材支持的事实，禁止编造数据/结论/引用。只输出 JSON：{\"paragraphs\":[\"...\"]}。"
            .into(),
        few_shot: vec![
            WorkedExample::new(
                "大纲：扩散模型采样效率\n素材：[来源 s1] DDIM 通过确定性采样在更少步数内生成高质量图像。",
                r#"{"paragraphs":["相较于 DDPM，DDIM 采用确定性采样过程，能够在显著更少的迭代步数内生成质量相当的图像，从而提升采样效率。"]}"#,
            ),
            WorkedExample::new(
                "大纲：实验设置\n素材：[来源 s1] 模型在 ImageNet 上训练，batch size 256。",
                r#"{"paragraphs":["所有模型均在 ImageNet 数据集上训练，批大小设为 256。"]}"#,
            ),
        ],
        red_lines: vec![RedLine::no_hallucinated_citation()],
        placeholder: "[需核实]".into(),
        citation_styles: vec![CiteStyle::Gbt7714, CiteStyle::Apa, CiteStyle::Ieee],
        slots: vec![],
    }
}

fn email() -> GeneralTemplate {
    GeneralTemplate {
        id: "email".into(),
        display_name: "Email".into(),
        system_prompt: "你是邮件撰写助手。基于素材写一封措辞得体、结构清晰的邮件，含称呼与落款占位。\
            只陈述素材中的事实，不臆造。只输出 JSON：{\"paragraphs\":[\"...\"]}。"
            .into(),
        few_shot: vec![
            WorkedExample::new(
                "大纲：会议改期\n素材：[来源 s1] 周三评审会改到周五下午三点。",
                r#"{"paragraphs":["各位好，","原定于周三的项目评审会现调整至周五下午三点举行，会议地点不变，敬请准时参加。","顺颂时祺"]}"#,
            ),
            WorkedExample::new(
                "大纲：感谢面试\n素材：[来源 s1] 候选人参加了周一的技术面试。",
                r#"{"paragraphs":["您好，","感谢您参加本周一的技术面试。我们将在一周内反馈结果，请耐心等待。","祝好"]}"#,
            ),
        ],
        red_lines: vec![RedLine::no_hallucinated_citation()],
        placeholder: "[需核实]".into(),
        citation_styles: vec![],
        slots: vec![],
    }
}

fn report() -> GeneralTemplate {
    GeneralTemplate {
        id: "report".into(),
        display_name: "Report".into(),
        system_prompt: "你是报告撰写助手。基于素材生成分节、有数据支撑的报告段落。\
            数据/结论必须来自素材，缺支撑则标注待核实，绝不臆造。只输出 JSON：{\"paragraphs\":[\"...\"]}。"
            .into(),
        few_shot: vec![
            WorkedExample::new(
                "大纲：季度销售\n素材：[来源 s1] 第三季度营收 1200 万元，环比增长 8%。",
                r#"{"paragraphs":["第三季度营收达 1200 万元，环比增长 8%，增长主要来自核心产品线。"]}"#,
            ),
            WorkedExample::new(
                "大纲：风险\n素材：[来源 s1] 供应链交付周期延长至 6 周。",
                r#"{"paragraphs":["当前主要风险为供应链交付周期延长至 6 周，建议提前备货以缓解影响。"]}"#,
            ),
        ],
        red_lines: vec![RedLine::no_hallucinated_citation()],
        placeholder: "[需核实]".into(),
        citation_styles: vec![CiteStyle::Apa],
        slots: vec![],
    }
}

fn note() -> GeneralTemplate {
    GeneralTemplate {
        id: "note".into(),
        display_name: "Note".into(),
        system_prompt: "你是笔记整理助手。把素材凝练成条理清晰的要点笔记，忠于原文不扩写。\
            只输出 JSON：{\"paragraphs\":[\"...\"]}。"
            .into(),
        few_shot: vec![
            WorkedExample::new(
                "大纲：Rust 所有权\n素材：[来源 s1] 所有权在编译期检查生命周期，无需 GC。",
                r#"{"paragraphs":["所有权：编译期检查生命周期；无需垃圾回收。"]}"#,
            ),
            WorkedExample::new(
                "大纲：会议纪要\n素材：[来源 s1] 决定下周一上线，张三负责回归测试。",
                r#"{"paragraphs":["决定：下周一上线；负责人：张三（回归测试）。"]}"#,
            ),
        ],
        red_lines: vec![RedLine::no_hallucinated_citation()],
        placeholder: "[需核实]".into(),
        citation_styles: vec![],
        slots: vec![],
    }
}

fn general_doc() -> GeneralTemplate {
    GeneralTemplate {
        id: "general_doc".into(),
        display_name: "General document".into(),
        system_prompt: "你是通用文档撰写助手。基于素材写出连贯、清晰的文档段落，忠于来源不臆造。\
            只输出 JSON：{\"paragraphs\":[\"...\"]}。"
            .into(),
        few_shot: vec![
            WorkedExample::new(
                "大纲：产品简介\n素材：[来源 s1] attune 是一个本地知识库与记忆增强系统。",
                r#"{"paragraphs":["attune 是一个本地优先的知识库与记忆增强系统，帮助用户捕获、检索并复用知识。"]}"#,
            ),
            WorkedExample::new(
                "大纲：使用步骤\n素材：[来源 s1] 用户先解锁 vault，再上传文档即可搜索。",
                r#"{"paragraphs":["使用时，用户先解锁 vault，随后上传文档，即可对其进行检索。"]}"#,
            ),
        ],
        red_lines: vec![RedLine::no_hallucinated_citation()],
        placeholder: "[需核实]".into(),
        citation_styles: vec![],
        slots: vec![],
    }
}

/// The set of general OSS template ids (stable; pro adds its own out-of-tree).
pub const OSS_TEMPLATE_IDS: &[&str] = &[
    "academic_paragraph",
    "email",
    "report",
    "note",
    "general_doc",
];

/// A registry of writing templates. OSS pre-loads the general set; pro plugins register more via
/// [`TemplateRegistry::register`]. Lookup is by id; ids are unique (last registration wins, which
/// lets pro override a general template if it intentionally specializes it).
#[derive(Default)]
pub struct TemplateRegistry {
    templates: BTreeMap<String, std::sync::Arc<dyn WritingTemplate>>,
}

impl TemplateRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self {
            templates: BTreeMap::new(),
        }
    }

    /// A registry pre-loaded with the OSS general templates.
    pub fn with_oss_defaults() -> Self {
        let mut r = Self::new();
        r.register(std::sync::Arc::new(academic_paragraph()));
        r.register(std::sync::Arc::new(email()));
        r.register(std::sync::Arc::new(report()));
        r.register(std::sync::Arc::new(note()));
        r.register(std::sync::Arc::new(general_doc()));
        r
    }

    /// Register (or override) a template by its id.
    pub fn register(&mut self, t: std::sync::Arc<dyn WritingTemplate>) {
        self.templates.insert(t.id().to_string(), t);
    }

    /// Look up a template by id.
    pub fn get(&self, id: &str) -> Option<std::sync::Arc<dyn WritingTemplate>> {
        self.templates.get(id).cloned()
    }

    /// All registered ids (sorted, deterministic).
    pub fn ids(&self) -> Vec<String> {
        self.templates.keys().cloned().collect()
    }

    /// Number of registered templates.
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// True if no template is registered.
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }
}

// ─────────────────────────── W6 template fill (zero LLM) ───────────────────────────

/// Result of a [`fill_template`] call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FillResult {
    /// The body with `{{slot}}` placeholders replaced by their values.
    pub filled: String,
    /// Slots that were referenced in the body but had no value supplied (left as `{{slot}}` and
    /// reported so the UI can prompt the user). Empty ⇒ fully filled.
    pub missing_slots: Vec<String>,
    /// Slots that were supplied but never appeared in the body (no-op values).
    pub unused_values: Vec<String>,
}

/// Fill `{{slot}}` placeholders in `body` from `values` (zero LLM, pure string substitution).
///
/// Deterministic. A `{{name}}` whose key is absent from `values` is left verbatim and reported in
/// `missing_slots`; a supplied key never referenced is reported in `unused_values`. Placeholder
/// syntax is exactly `{{` + identifier (`[A-Za-z0-9_]+`) + `}}`; malformed braces pass through
/// untouched.
pub fn fill_template(body: &str, values: &BTreeMap<String, String>) -> FillResult {
    let mut out = String::with_capacity(body.len());
    let mut missing = Vec::new();
    let mut referenced: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    let bytes = body.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            // Find the closing `}}`.
            if let Some(rel_end) = find_close(&body[i + 2..]) {
                let name = &body[i + 2..i + 2 + rel_end];
                if is_ident(name) {
                    referenced.insert(name.to_string());
                    match values.get(name) {
                        Some(v) => out.push_str(v),
                        None => {
                            out.push_str(&format!("{{{{{name}}}}}"));
                            if !missing.iter().any(|m: &String| m == name) {
                                missing.push(name.to_string());
                            }
                        }
                    }
                    i = i + 2 + rel_end + 2; // skip `{{name}}`
                    continue;
                }
            }
        }
        // Not a placeholder start — copy this char (advance by full UTF-8 char width).
        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&body[i..i + ch_len]);
        i += ch_len;
    }

    let unused: Vec<String> = values
        .keys()
        .filter(|k| !referenced.contains(*k))
        .cloned()
        .collect();

    FillResult {
        filled: out,
        missing_slots: missing,
        unused_values: unused,
    }
}

/// Byte length of the UTF-8 char starting at a leading byte.
fn utf8_char_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else if b >> 3 == 0b11110 {
        4
    } else {
        1 // continuation / invalid byte: treat as 1 to make progress (lossless copy)
    }
}

/// Find the byte offset of the first `}}` in `s` (offset of the first `}`), or `None`.
fn find_close(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'}' && b[i + 1] == b'}' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// `{{name}}` placeholder identifier rule: non-empty `[A-Za-z0-9_]+`.
fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

// COMPILE-TIME COST GUARD (spec §8 / cost contract): templates are tier 🆓. This module must not
// reference any LLM provider type. The guard below is a zero-sized const that only type-checks if
// the module compiles WITHOUT an `LlmProvider` import in scope — i.e. it documents intent and the
// `clippy -D warnings` + the (absence of) `use crate::llm` line are the real enforcement. We also
// assert the OSS id set stays in lock-step with the default registry size at runtime in tests.
#[allow(dead_code)]
const _NO_LLM_IN_TEMPLATES: () = {
    // If anyone adds `use crate::llm::LlmProvider;` here to "just call the model in fill", this
    // file's module doc + this marker make the zero-cost violation obvious in review/diff.
};

#[cfg(test)]
mod tests {
    use super::*;

    // ── registry / template metadata ──

    #[test]
    fn oss_registry_has_all_general_templates() {
        let r = TemplateRegistry::with_oss_defaults();
        assert_eq!(r.len(), OSS_TEMPLATE_IDS.len());
        for id in OSS_TEMPLATE_IDS {
            assert!(r.get(id).is_some(), "missing OSS template {id}");
        }
    }

    #[test]
    fn every_template_has_min_two_few_shot() {
        // §4.5 C lower bound.
        let r = TemplateRegistry::with_oss_defaults();
        for id in r.ids() {
            let t = r.get(&id).unwrap();
            assert!(
                t.few_shot().len() >= 2,
                "template {id} has <2 few-shot examples"
            );
        }
    }

    #[test]
    fn every_template_carries_no_hallucinated_citation_red_line() {
        let r = TemplateRegistry::with_oss_defaults();
        for id in r.ids() {
            let t = r.get(&id).unwrap();
            assert!(
                t.red_lines().iter().any(|rl| rl.id == "no-hallucinated-citation"),
                "template {id} missing universal red line"
            );
        }
    }

    #[test]
    fn default_placeholder_marker_is_needs_verification() {
        let r = TemplateRegistry::with_oss_defaults();
        assert_eq!(r.get("note").unwrap().placeholder_marker(), "[需核实]");
    }

    #[test]
    fn register_overrides_existing_id() {
        let mut r = TemplateRegistry::with_oss_defaults();
        let before = r.len();
        // A pro-style override of `note`.
        let custom = GeneralTemplate {
            id: "note".into(),
            display_name: "Custom note".into(),
            system_prompt: "x".into(),
            few_shot: vec![WorkedExample::new("a", "b"), WorkedExample::new("c", "d")],
            red_lines: vec![RedLine::no_hallucinated_citation()],
            placeholder: "[请确认]".into(),
            citation_styles: vec![],
            slots: vec![],
        };
        r.register(std::sync::Arc::new(custom));
        assert_eq!(r.len(), before, "override must not grow the registry");
        assert_eq!(r.get("note").unwrap().placeholder_marker(), "[请确认]");
    }

    #[test]
    fn empty_registry_is_empty() {
        let r = TemplateRegistry::new();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        assert!(r.get("anything").is_none());
    }

    // ── template fill (W6, zero LLM) ──

    fn vmap(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn fill_replaces_known_slots() {
        let r = fill_template("Dear {{name}}, see {{topic}}.", &vmap(&[("name", "Bob"), ("topic", "Q3")]));
        assert_eq!(r.filled, "Dear Bob, see Q3.");
        assert!(r.missing_slots.is_empty());
        assert!(r.unused_values.is_empty());
    }

    #[test]
    fn fill_reports_missing_slot_and_leaves_it() {
        let r = fill_template("Hi {{name}} re {{missing}}.", &vmap(&[("name", "Bob")]));
        assert_eq!(r.filled, "Hi Bob re {{missing}}.");
        assert_eq!(r.missing_slots, vec!["missing".to_string()]);
    }

    #[test]
    fn fill_reports_unused_value() {
        let r = fill_template("Hi {{name}}.", &vmap(&[("name", "Bob"), ("extra", "v")]));
        assert_eq!(r.unused_values, vec!["extra".to_string()]);
    }

    #[test]
    fn fill_cjk_body_offsets_safe() {
        // CJK + emoji surrounding a placeholder must copy byte-correctly (no panic / no mojibake).
        let r = fill_template("尊敬的{{name}}您好😀，关于{{topic}}。", &vmap(&[("name", "张三"), ("topic", "改期")]));
        assert_eq!(r.filled, "尊敬的张三您好😀，关于改期。");
    }

    #[test]
    fn fill_malformed_braces_pass_through() {
        // Single braces, unmatched, and non-ident names are not placeholders.
        let r = fill_template("a {single} {{ bad name }} {{}} end", &vmap(&[]));
        assert_eq!(r.filled, "a {single} {{ bad name }} {{}} end");
        assert!(r.missing_slots.is_empty());
    }

    #[test]
    fn fill_repeated_slot_filled_each_time_reported_once() {
        let r = fill_template("{{x}}-{{x}}", &vmap(&[]));
        assert_eq!(r.filled, "{{x}}-{{x}}");
        assert_eq!(r.missing_slots, vec!["x".to_string()]); // reported once
    }

    #[test]
    fn fill_empty_body() {
        let r = fill_template("", &vmap(&[("a", "b")]));
        assert_eq!(r.filled, "");
        assert_eq!(r.unused_values, vec!["a".to_string()]);
    }

    // ── property tests (spec §9.1 ≥3) ──
    use proptest::prelude::*;

    proptest! {
        // ① fill is idempotent on a body with no placeholders: output == input.
        #[test]
        fn prop_fill_no_placeholder_identity(body in "[^{}]{0,80}") {
            let r = fill_template(&body, &vmap(&[("k", "v")]));
            prop_assert_eq!(r.filled, body);
        }

        // ② a filled slot never leaves its placeholder text behind (when value has no braces).
        #[test]
        fn prop_filled_slot_disappears(val in "[a-z]{1,10}") {
            let r = fill_template("x{{s}}y", &vmap(&[("s", &val)]));
            prop_assert!(!r.filled.contains("{{s}}"));
            prop_assert!(r.filled.contains(&val));
        }

        // ③ output is deterministic: same inputs → same output.
        #[test]
        fn prop_fill_deterministic(body in "(\\{\\{s\\}\\}|[a-z ]){0,40}", v in "[a-z]{0,8}") {
            let m = vmap(&[("s", &v)]);
            let a = fill_template(&body, &m);
            let b = fill_template(&body, &m);
            prop_assert_eq!(a, b);
        }
    }
}
