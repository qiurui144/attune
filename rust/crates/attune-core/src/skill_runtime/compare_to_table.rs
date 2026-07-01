//! `compare_to_table` agent — the LLM step of the `compare-to-table` skill (用户例 3).
//!
//! User need (原话): "一个文档中两个设备参数的不同 → 输出表格并下载".
//!
//! Unlike [`crate::document_intelligence::compare`] (which diffs **two whole documents** by
//! section), this agent **extracts the parameters of two named entities from one (or two)
//! document(s)** and produces a per-parameter diff. The output is a [`ParamComparison`] —
//! one row per parameter, with entity-A / entity-B values and whether they differ — which the
//! `render` step turns into an [`crate::export::Table`] Artifact (param / A / B / 差异).
//!
//! ## §4.5 LLM-fallback hardening (CLAUDE.md §Agent 模型兜底原则)
//!
//! - **A. Schema-guided**: the extraction prompt demands a strict JSON object and is passed a
//!   JSON schema via [`crate::llm::LlmProvider::chat_with_format_json`] so OpenAI-compatible
//!   providers (DeepSeek) enforce it server-side via `response_format`.
//! - **B. Retry-validate (≤3)**: the response is validated (parseable + every value traces to
//!   the source text); a failed attempt feeds the error back to the model.
//! - **C. Few-shot**: two worked examples (incl. an absent-parameter edge) are baked into the
//!   system prompt so weak models lock onto the shape.
//! - **Grounding (§4.5)**: every extracted value must appear **verbatim** in the source text
//!   (CJK-safe substring check). A value that does not trace back is dropped to `None` and the
//!   row is flagged `ungrounded` rather than allowed to be a fabrication — the cell shows
//!   "（未在文档中找到）" instead of a hallucinated number.
//! - **E. Degrade**: if every attempt fails, the agent returns an **empty but valid**
//!   comparison (no rows) rather than panicking, so the skill still produces a (degraded)
//!   downloadable artifact + a warning.

use crate::document_intelligence::token_bill::TokenBill;
use crate::llm::LlmProvider;
use crate::usage::TokenUsage;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// One compared parameter row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamRow {
    /// The parameter name (e.g. "分辨率", "功耗").
    pub name: String,
    /// Entity-A value, or `None` if the document does not state it for A.
    pub value_a: Option<String>,
    /// Entity-B value, or `None` if the document does not state it for B.
    pub value_b: Option<String>,
    /// Whether A and B differ on this parameter (both present and unequal).
    pub differs: bool,
    /// True iff a value the model proposed could not be traced back to the source text and was
    /// therefore dropped to `None` (grounding guard). Surfaced as an `unverified_span`.
    #[serde(default)]
    pub ungrounded: bool,
}

/// The structured comparison of two entities' parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamComparison {
    /// Entity A's display label (e.g. "设备 A" / "iPhone 15").
    pub entity_a: String,
    /// Entity B's display label.
    pub entity_b: String,
    /// One row per parameter, in stable (model-proposed) order.
    pub rows: Vec<ParamRow>,
    /// Token accounting for the extraction call(s).
    pub token_bill: TokenBill,
    /// Non-fatal warnings (degrade path, dropped ungrounded values).
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl ParamComparison {
    /// Number of rows where the two entities differ.
    pub fn diff_count(&self) -> usize {
        self.rows.iter().filter(|r| r.differs).count()
    }
}

const EXTRACT_SYSTEM_PROMPT: &str = "你是设备参数抽取与比对器。给你一段文档文本和两个实体名称（A 与 B），\
从文档中抽取这两个实体各自的参数，并逐项对照。\
只输出一个 JSON 对象，不要任何前后缀、不要 markdown 代码块。\
对象字段：`rows` 是一个数组，每个元素 `{\"name\": 参数名, \"value_a\": A的值或null, \"value_b\": B的值或null}`。\
要求：(1) 参数值必须是文档中**原文出现**的字符串，不得自行换算或编造；\
(2) 文档没有说明某实体的某参数时，对应值填 null，不要猜测；\
(3) 只抽取文档中确有依据的参数。\
示例：\n\
输入文档【设备 A 分辨率 1080p，功耗 5W。设备 B 分辨率 4K，功耗 12W。】实体 A=设备 A，实体 B=设备 B\n\
输出 {\"rows\":[{\"name\":\"分辨率\",\"value_a\":\"1080p\",\"value_b\":\"4K\"},{\"name\":\"功耗\",\"value_a\":\"5W\",\"value_b\":\"12W\"}]}\n\
输入文档【甲型号重量 2kg。乙型号未给出重量，价格 999 元。】实体 A=甲型号，实体 B=乙型号\n\
输出 {\"rows\":[{\"name\":\"重量\",\"value_a\":\"2kg\",\"value_b\":null},{\"name\":\"价格\",\"value_a\":null,\"value_b\":\"999 元\"}]}";

/// JSON schema for the extraction object (§4.5.A). Passed to `chat_with_format_json`.
fn extract_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "rows": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "value_a": { "type": ["string", "null"] },
                        "value_b": { "type": ["string", "null"] }
                    },
                    "required": ["name"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["rows"],
        "additionalProperties": false
    })
}

/// Extract + compare the parameters of `entity_a` and `entity_b` from `text`.
///
/// Never returns `Err`: a hard LLM failure degrades to an empty comparison + a warning so the
/// enclosing skill always produces a (possibly degraded) downloadable artifact (§4.5.E).
/// `model` is the logical model name (for the token bill).
pub fn compare_to_table(
    text: &str,
    entity_a: &str,
    entity_b: &str,
    llm: &dyn LlmProvider,
    model: &str,
) -> ParamComparison {
    let mut bill = TokenBill::default();
    let mut warnings = Vec::new();

    let schema = extract_schema();
    let user = format!(
        "文档文本：\n{text}\n\n实体 A={entity_a}\n实体 B={entity_b}\n\n\
         请抽取这两个实体的参数并按上述 JSON 格式输出。"
    );

    let (raw, usage) = extract_call(llm, EXTRACT_SYSTEM_PROMPT, &user, &schema);
    account(&mut bill, &usage, &user, &raw, model);

    let mut rows = match parse_rows(&raw) {
        Some(r) => r,
        None => {
            // §4.5.E degrade — every attempt failed to produce a parseable object.
            warnings.push("参数抽取失败（模型未返回可解析结果），已生成空表格".to_string());
            Vec::new()
        }
    };

    // ── Grounding guard (§4.5): every value must appear verbatim in the source text ──
    for row in &mut rows {
        if let Some(v) = &row.value_a {
            if !grounded(text, v) {
                row.ungrounded = true;
                warnings.push(format!("参数「{}」的 A 值未在文档中找到，已置空", row.name));
                row.value_a = None;
            }
        }
        if let Some(v) = &row.value_b {
            if !grounded(text, v) {
                row.ungrounded = true;
                warnings.push(format!("参数「{}」的 B 值未在文档中找到，已置空", row.name));
                row.value_b = None;
            }
        }
        row.differs = match (&row.value_a, &row.value_b) {
            (Some(a), Some(b)) => a.trim() != b.trim(),
            _ => false,
        };
    }

    ParamComparison {
        entity_a: entity_a.to_string(),
        entity_b: entity_b.to_string(),
        rows,
        token_bill: bill,
        warnings,
    }
}

/// Schema-guided extraction call with a ≤3-attempt validate-retry loop (§4.5.B). Always returns
/// a `(text, usage)` pair — a hard transport failure yields empty text (caller then degrades).
fn extract_call(
    llm: &dyn LlmProvider,
    system: &str,
    user: &str,
    schema: &serde_json::Value,
) -> (String, TokenUsage) {
    let validator = |raw: &str| -> std::result::Result<(), String> {
        match parse_rows(raw) {
            Some(_) => Ok(()),
            None => Err(
                "输出不是可解析的 JSON 对象（需含 rows 数组，每项有 name + value_a/value_b）"
                    .into(),
            ),
        }
    };
    // Strict schema-guided path first (real response_format on DeepSeek/OpenAI).
    if let Ok((raw, usage)) = llm.chat_with_format_json(system, user, Some(schema)) {
        if validator(&raw).is_ok() {
            return (raw, usage);
        }
    }
    // Retry loop feeds the validation error back to the model (≤3 attempts).
    match llm.chat_with_retry(system, user, 3, &validator) {
        Ok((raw, usage)) => (raw, usage),
        // Every attempt failed: one last plain call so we at least bill it; caller degrades.
        Err(_) => llm
            .chat(system, user)
            .unwrap_or_else(|_| (String::new(), TokenUsage::empty("compare_to_table", ""))),
    }
}

/// Parse the `rows` array out of a (possibly fenced / prose-wrapped) LLM response.
/// Returns `None` when no parseable object with a `rows` array is found.
fn parse_rows(resp: &str) -> Option<Vec<ParamRow>> {
    let candidate = extract_json_object(resp)?;
    let val: serde_json::Value = serde_json::from_str(&candidate).ok()?;
    let arr = val.get("rows")?.as_array()?;
    let mut rows = Vec::with_capacity(arr.len());
    for item in arr {
        let obj = item.as_object()?;
        let name = obj.get("name").and_then(|v| v.as_str())?.trim().to_string();
        if name.is_empty() {
            continue;
        }
        let value_a = obj.get("value_a").and_then(json_opt_str);
        let value_b = obj.get("value_b").and_then(json_opt_str);
        rows.push(ParamRow {
            name,
            value_a,
            value_b,
            differs: false, // computed after grounding
            ungrounded: false,
        });
    }
    Some(rows)
}

/// Read an optional non-empty string from a JSON value (`null` / `""` → `None`).
fn json_opt_str(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        // Tolerate a model that emits a number instead of a string.
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// CJK-safe grounding check: does `needle` appear (trimmed) verbatim in `haystack`?
/// `String::contains` is byte-based but correct for UTF-8 substrings.
fn grounded(haystack: &str, needle: &str) -> bool {
    let n = needle.trim();
    !n.is_empty() && haystack.contains(n)
}

/// Scan `s` for the first balanced top-level `{...}` substring (string-aware), recovering a JSON
/// object even when the model wraps it in ```json fences or adds prose. (Mirrors compare.rs.)
fn extract_json_object(s: &str) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    let start = chars.iter().position(|&c| c == '{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &c) in chars.iter().enumerate().skip(start) {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(chars[start..=i].iter().collect());
                }
            }
            _ => {}
        }
    }
    None
}

/// Account an LLM call into the map leg (mock reports 0 → approximate from text length).
fn account(bill: &mut TokenBill, usage: &TokenUsage, user: &str, resp: &str, model: &str) {
    use crate::context_compress::estimate_tokens;
    let leg = &mut bill.map_llm_tokens;
    if usage.tokens_in == 0 {
        leg.r#in = leg.r#in.saturating_add(estimate_tokens(user) as u32);
        leg.out = leg.out.saturating_add(estimate_tokens(resp) as u32);
        if leg.model.is_empty() {
            leg.model = model.to_string();
        }
    } else {
        leg.add(usage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::RecordingMockLlm;

    fn doc() -> &'static str {
        "本文档对比两款设备。设备 A：分辨率 1080p，功耗 5W，重量 2kg。\
         设备 B：分辨率 4K，功耗 12W，重量 2kg。"
    }

    #[test]
    fn extracts_and_diffs_two_devices() {
        let llm = RecordingMockLlm::new("deepseek").with_response(
            r#"{"rows":[
                {"name":"分辨率","value_a":"1080p","value_b":"4K"},
                {"name":"功耗","value_a":"5W","value_b":"12W"},
                {"name":"重量","value_a":"2kg","value_b":"2kg"}
            ]}"#,
        );
        let cmp = compare_to_table(doc(), "设备 A", "设备 B", &llm, "deepseek");
        assert_eq!(cmp.rows.len(), 3);
        // 分辨率 differs, 功耗 differs, 重量 same.
        assert_eq!(cmp.diff_count(), 2);
        let res = cmp.rows.iter().find(|r| r.name == "分辨率").unwrap();
        assert!(res.differs);
        assert_eq!(res.value_a.as_deref(), Some("1080p"));
        let weight = cmp.rows.iter().find(|r| r.name == "重量").unwrap();
        assert!(!weight.differs, "same value → no diff");
        assert!(cmp.warnings.is_empty());
    }

    #[test]
    fn ungrounded_value_dropped_to_none() {
        // Model hallucinates "8K" for B which is NOT in the doc → grounding guard nulls it.
        let llm = RecordingMockLlm::new("deepseek")
            .with_response(r#"{"rows":[{"name":"分辨率","value_a":"1080p","value_b":"8K"}]}"#);
        let cmp = compare_to_table(doc(), "设备 A", "设备 B", &llm, "deepseek");
        let row = &cmp.rows[0];
        assert_eq!(row.value_a.as_deref(), Some("1080p"), "grounded value kept");
        assert_eq!(row.value_b, None, "ungrounded 8K dropped");
        assert!(row.ungrounded);
        assert!(!row.differs, "one side null → not a diff");
        assert!(cmp.warnings.iter().any(|w| w.contains("未在文档中找到")));
    }

    #[test]
    fn absent_param_is_null_not_invented() {
        let llm = RecordingMockLlm::new("deepseek")
            .with_response(r#"{"rows":[{"name":"价格","value_a":null,"value_b":null}]}"#);
        let cmp = compare_to_table(doc(), "设备 A", "设备 B", &llm, "deepseek");
        assert_eq!(cmp.rows[0].value_a, None);
        assert_eq!(cmp.rows[0].value_b, None);
        assert!(!cmp.rows[0].differs);
    }

    #[test]
    fn fenced_json_recovered() {
        let llm = RecordingMockLlm::new("deepseek").with_response(
            "```json\n{\"rows\":[{\"name\":\"功耗\",\"value_a\":\"5W\",\"value_b\":\"12W\"}]}\n```",
        );
        let cmp = compare_to_table(doc(), "设备 A", "设备 B", &llm, "deepseek");
        assert_eq!(cmp.rows.len(), 1);
        assert!(cmp.rows[0].differs);
    }

    #[test]
    fn garbage_degrades_to_empty_with_warning() {
        let mut llm = RecordingMockLlm::new("deepseek");
        for _ in 0..6 {
            llm = llm.with_response("完全无关的散文，没有任何 JSON。");
        }
        let cmp = compare_to_table(doc(), "设备 A", "设备 B", &llm, "deepseek");
        assert!(cmp.rows.is_empty(), "no rows on total parse failure");
        assert!(cmp.warnings.iter().any(|w| w.contains("抽取失败")));
        assert!(
            llm.call_count() >= 3,
            "retry loop attempted before degrading"
        );
    }

    #[test]
    fn serde_roundtrip() {
        let llm = RecordingMockLlm::new("deepseek")
            .with_response(r#"{"rows":[{"name":"功耗","value_a":"5W","value_b":"12W"}]}"#);
        let cmp = compare_to_table(doc(), "设备 A", "设备 B", &llm, "deepseek");
        let js = serde_json::to_string(&cmp).unwrap();
        let back: ParamComparison = serde_json::from_str(&js).unwrap();
        assert_eq!(back, cmp);
    }

    #[test]
    fn empty_doc_no_panic() {
        let llm = RecordingMockLlm::new("deepseek").with_response(r#"{"rows":[]}"#);
        let cmp = compare_to_table("", "A", "B", &llm, "deepseek");
        assert!(cmp.rows.is_empty());
    }
}
