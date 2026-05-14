//! PII (Personally Identifiable Information) 脱敏 — 出网中间件
//!
//! 设计：3 层流水线（per 用户决策 2026-04-28）
//!
//! - **L1 正则 + 词典**：OSS 免费层，所有 tier 必跑（本模块）
//! - **L2 ONNX NER**：OSS 免费层，Tier T1+ 可下载（见 `pii::ner`，待实现）
//! - **L3 LLM 脱敏**：高端硬件 (T3+T4+K3) 增值层（见 `pii::llm`，v0.7+）
//!
//! ## 核心承诺
//!
//! - **格式化 PII** (身份证/手机/邮箱/IP/案号/API key/银行卡/...): L1 ≥ 99% 召回，0 幻觉
//! - **placeholder 可逆**: `redact()` → 云端 LLM → `restore()` 答案中 placeholder 还原回原值
//! - **同值同标签**: 文本中 "张三" 出现 N 次共享同一 `[PERSON_1]`，保持语义一致
//!
//! ## 与 `entities` 模块的区别
//!
//! - `entities`: 通用语义实体（Person / Money / Date / Org），用于 Project 推荐归类
//! - `pii`: 敏感字段闭合清单，用于出网前脱敏（输出可逆 placeholder）

pub mod patterns;
pub mod dictionary;
pub mod ner;

// ── F-17 LLM call wrapper helpers ──────────────────────────────────────────
//
// 给非 ChatEngine 的 LLM call sites (context_compress / ai_annotator /
// web_search_browser query 等) 提供统一的 redact + LLM call + restore 包装，
// 避免每个 caller 重复 redact_batch + restore 模板代码。

/// Wrap a single-message LLM `chat(system, user)` call with PII redact +
/// restore. Use for non-chat LLM call sites where a stable [system, user]
/// API is already in place.
///
/// 行为：
/// 1. `redact_batch` 处理 [system, user]，placeholder 全局唯一
/// 2. 调 LLM 用 redacted_system + redacted_user
/// 3. `restore` 反向 LLM 响应里的所有 placeholder
///
/// audit log 用 `log::info!(target: "outbound_audit", ...)` 输出，与
/// `ChatEngine::run_llm_once` 一致。
///
/// 适用 call sites:
/// - `context_compress::compress_chunk` — chunk → summary (chunk 含 PII 不出云)
/// - `context_compress::generate_summary` — 同上 cacheless 版本
/// - `ai_annotator::generate_annotations` — chunk → 4 角度批注 JSON
pub fn llm_chat_redacted(
    llm: &dyn crate::llm::LlmProvider,
    redactor: &Redactor,
    system: &str,
    user: &str,
    call_site: &str,
) -> crate::error::Result<String> {
    let (redacted, mappings) = redactor.redact_batch(&[system, user]);

    if !mappings.is_empty() {
        let mut by_kind: HashMap<String, usize> = HashMap::new();
        for m in &mappings {
            let prefix = m.kind.placeholder_prefix().to_string().to_uppercase();
            *by_kind.entry(prefix).or_insert(0) += 1;
        }
        log::info!(
            target: "outbound_audit",
            "F-17: PII redacted in {} outbound — kinds={:?} total={} model={}",
            call_site,
            by_kind,
            mappings.len(),
            llm.model_name()
        );
    }

    let raw = llm.chat(&redacted[0], &redacted[1])?;
    Ok(redactor.restore(&raw, &mappings))
}

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// PII 类别。`Custom` 来自用户词典；`PluginProvided` 来自 vertical plugin
/// (如 law-pro 的 case_no、medical-pro 的 medical_id)。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiiKind {
    IdCard,
    Phone,
    Email,
    Ipv4,
    Ipv6,
    CreditCard,
    BankCard,
    PlateNumber,
    ApiKey,
    Url,
    MacAddress,
    Coordinate,
    Custom(String),
    PluginProvided(String),
}

impl PiiKind {
    /// placeholder 前缀。设计目标：LLM 容易识别，反向替换不冲突。
    pub fn placeholder_prefix(&self) -> &str {
        match self {
            Self::IdCard => "ID",
            Self::Phone => "PHONE",
            Self::Email => "EMAIL",
            Self::Ipv4 | Self::Ipv6 => "IP",
            Self::CreditCard | Self::BankCard => "CARD",
            Self::PlateNumber => "PLATE",
            Self::ApiKey => "APIKEY",
            Self::Url => "URL",
            Self::MacAddress => "MAC",
            Self::Coordinate => "GPS",
            Self::Custom(name) | Self::PluginProvided(name) => name.as_str(),
        }
    }
}

/// 单条 PII 命中记录。`restore()` 时按 (placeholder → original) 反向替换。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiiMatch {
    pub kind: PiiKind,
    pub original: String,
    pub placeholder: String,
    pub byte_start: usize,
    pub byte_end: usize,
}

/// 一次脱敏的完整结果：处理后文本 + 可逆映射 + 统计。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactionResult {
    pub redacted_text: String,
    pub mappings: Vec<PiiMatch>,
    pub stats: RedactionStats,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RedactionStats {
    /// 按类别统计命中数（key 为 placeholder 前缀，如 "PHONE"）
    pub by_kind: HashMap<String, usize>,
    /// 总命中字段数（去重前）
    pub total_matches: usize,
    /// 脱敏前敏感字段总字符数
    pub total_chars_redacted: usize,
}

/// vertical plugin 注册的自定义 PII 抽取器。
///
/// 例如 attune-pro/law-pro 提供 CaseNoExtractor，识别 `(2023)京01民终123号`。
pub trait PiiExtractor: Send + Sync {
    /// 抽取器名字（用于 placeholder 前缀，如 "case_no" → `[case_no_1]`）
    fn name(&self) -> &str;

    /// 类别标记
    fn kind(&self) -> PiiKind {
        PiiKind::PluginProvided(self.name().to_string())
    }

    /// 在文本中找出所有命中的 (byte_start, byte_end) 区间。
    fn extract(&self, text: &str) -> Vec<(usize, usize)>;
}

/// 主脱敏器：管理用户词典 + 插件 extractor + 内置正则。
#[derive(Default)]
pub struct Redactor {
    user_dict: Vec<dictionary::DictEntry>,
    plugin_extractors: Vec<Box<dyn PiiExtractor>>,
}

/// 内部用：从 patterns / dict / plugin 收集到的原始命中
#[derive(Debug, Clone)]
struct RawMatch {
    kind: PiiKind,
    start: usize,
    end: usize,
    value: String,
}

impl Redactor {
    pub fn new() -> Self {
        Self::default()
    }

    /// 加载用户词典 YAML 文件（`.attune/pii_dict.yaml`）。
    /// 文件不存在不报错（返回空词典 Redactor）。
    pub fn with_dictionary_file(path: &Path) -> std::io::Result<Self> {
        let mut r = Self::new();
        if path.exists() {
            r.user_dict = dictionary::load(path)?;
        }
        Ok(r)
    }

    pub fn register_plugin(&mut self, ext: Box<dyn PiiExtractor>) {
        self.plugin_extractors.push(ext);
    }

    /// 添加一个词典项（字面量 + / 或正则）。
    pub fn add_dict_entry(&mut self, entry: dictionary::DictEntry) {
        self.user_dict.push(entry);
    }

    /// 便利方法：注册一个由 (name, regex) 描述的 PII 模式。
    /// vertical plugin 提供的行业 PII 用这条路径（PluginRegistry::all_pii_patterns 聚合后批量注入）。
    pub fn add_pattern(&mut self, name: &str, regex: &str) -> std::io::Result<()> {
        self.user_dict.push(dictionary::DictEntry::from_regex(name, regex)?);
        Ok(())
    }

    pub fn dictionary_len(&self) -> usize {
        self.user_dict.len()
    }

    pub fn plugin_count(&self) -> usize {
        self.plugin_extractors.len()
    }

    /// 主入口：对文本做脱敏，返回 (redacted_text, mappings, stats)。
    pub fn redact(&self, text: &str) -> RedactionResult {
        if text.is_empty() {
            return RedactionResult {
                redacted_text: String::new(),
                mappings: Vec::new(),
                stats: RedactionStats::default(),
            };
        }

        let raw = self.collect_all(text);
        let deduped = dedupe_overlaps(raw);
        let mappings = assign_placeholders(deduped);
        let redacted_text = apply_replacements(text, &mappings);
        let stats = compute_stats(&mappings);

        RedactionResult {
            redacted_text,
            mappings,
            stats,
        }
    }

    /// 批量 redact：对多段文本分别 redact，**placeholder 全局唯一**。
    ///
    /// 用 separator-based 方案：把所有段 join 成一个 megastring 再 redact，
    /// 让内部 `assign_placeholders` 自然产生全局连续的 [KIND_N] 索引；
    /// 然后按 separator split 回各段。Separator 选用极低冲突字符串
    /// `<<<ATTUNE_PII_SEPARATOR_NONCE_42424242>>>`（含等号 / 大写 / 高熵 nonce），
    /// 用户输入命中概率可忽略。
    ///
    /// 返回 `(redacted_segments, all_mappings)`：
    /// - `redacted_segments[i]` 对应输入 `segments[i]` 的 redacted 版本
    /// - `all_mappings` 是全局 mappings，可直接 `restore()` 任何含 placeholder 的文本
    ///
    /// 用例：F-17-PRIVACY 多段出网内容（user_message + history + knowledge）
    /// 需要全局唯一 placeholder 索引，否则 [PHONE_1] 在 user 和 knowledge 中
    /// 指向不同原值，restore 时无法区分。
    ///
    /// 边界：如果某段含 separator 字面量（极不可能），split 会切错。当前不防御
    /// 这种攻击 — F-17 redact 是出网安全增强，不是恶意输入抗性层。
    pub fn redact_batch<S: AsRef<str>>(&self, segments: &[S]) -> (Vec<String>, Vec<PiiMatch>) {
        const SEP: &str = "<<<ATTUNE_PII_SEPARATOR_NONCE_42424242>>>";

        if segments.is_empty() {
            return (Vec::new(), Vec::new());
        }

        let joined: String = segments
            .iter()
            .map(|s| s.as_ref())
            .collect::<Vec<_>>()
            .join(SEP);

        let result = self.redact(&joined);
        let redacted_segments: Vec<String> = result
            .redacted_text
            .split(SEP)
            .map(String::from)
            .collect();

        // mappings 中的 byte_start/byte_end 是相对 joined 字符串的，对调用方来说
        // 通常只用于 restore（按 placeholder 字面量替换，不依赖 offset），所以
        // 即使 offset 跨段也不影响 restore 正确性。
        (redacted_segments, result.mappings)
    }

    /// 反向替换：把 LLM 返回的答案中的 placeholder 还原回原值。
    /// 普通字符串替换（按 placeholder 长度降序避免 prefix 冲突，如
    /// `[PERSON_10]` 必须先于 `[PERSON_1]` 替换）。
    pub fn restore(&self, text: &str, mappings: &[PiiMatch]) -> String {
        // 收集 (placeholder → original)，相同 placeholder 只保留一份
        let mut pairs: HashMap<&str, &str> = HashMap::new();
        for m in mappings {
            pairs.insert(&m.placeholder, &m.original);
        }
        // 按 placeholder 长度降序，避免 [PERSON_1] 误吃 [PERSON_10] 前缀
        let mut sorted: Vec<_> = pairs.into_iter().collect();
        sorted.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));

        let mut out = text.to_string();
        for (placeholder, original) in sorted {
            out = out.replace(placeholder, original);
        }
        out
    }

    fn collect_all(&self, text: &str) -> Vec<RawMatch> {
        let mut raw = Vec::new();

        // 内置 patterns（顺序：长 → 短，减少 overlap 时短的吞掉长的）
        push_matches(&mut raw, PiiKind::Url, patterns::detect_url(text), text);
        push_matches(&mut raw, PiiKind::Email, patterns::detect_email(text), text);
        push_matches(&mut raw, PiiKind::IdCard, patterns::detect_id_card(text), text);
        push_matches(&mut raw, PiiKind::CreditCard, patterns::detect_credit_card(text), text);
        push_matches(&mut raw, PiiKind::BankCard, patterns::detect_bank_card(text), text);
        push_matches(&mut raw, PiiKind::ApiKey, patterns::detect_api_key(text), text);
        push_matches(&mut raw, PiiKind::Ipv6, patterns::detect_ipv6(text), text);
        push_matches(&mut raw, PiiKind::Ipv4, patterns::detect_ipv4(text), text);
        push_matches(&mut raw, PiiKind::Phone, patterns::detect_phone(text), text);
        push_matches(&mut raw, PiiKind::PlateNumber, patterns::detect_plate_number(text), text);
        push_matches(&mut raw, PiiKind::MacAddress, patterns::detect_mac(text), text);
        push_matches(&mut raw, PiiKind::Coordinate, patterns::detect_gps(text), text);

        // 用户词典
        for entry in &self.user_dict {
            for (s, e) in entry.find_all(text) {
                raw.push(RawMatch {
                    kind: PiiKind::Custom(entry.name.clone()),
                    start: s,
                    end: e,
                    value: text[s..e].to_string(),
                });
            }
        }

        // 插件
        for ext in &self.plugin_extractors {
            let kind = ext.kind();
            for (s, e) in ext.extract(text) {
                raw.push(RawMatch {
                    kind: kind.clone(),
                    start: s,
                    end: e,
                    value: text[s..e].to_string(),
                });
            }
        }

        raw
    }
}

fn push_matches(raw: &mut Vec<RawMatch>, kind: PiiKind, spans: Vec<(usize, usize)>, text: &str) {
    for (s, e) in spans {
        if e <= text.len() && s < e {
            raw.push(RawMatch {
                kind: kind.clone(),
                start: s,
                end: e,
                value: text[s..e].to_string(),
            });
        }
    }
}

/// 贪心去 overlap：start 升序 + 长度降序，扫描时跳过被覆盖的。
fn dedupe_overlaps(mut raw: Vec<RawMatch>) -> Vec<RawMatch> {
    raw.sort_by_key(|m| (m.start, std::cmp::Reverse(m.end)));
    let mut result = Vec::new();
    let mut cursor = 0usize;
    for m in raw {
        if m.start >= cursor {
            cursor = m.end;
            result.push(m);
        }
    }
    result
}

/// 同值共享同 placeholder（per-kind 计数）。例如 "张三" 出现 3 次都得 [PERSON_1]。
fn assign_placeholders(raw: Vec<RawMatch>) -> Vec<PiiMatch> {
    let mut counters: HashMap<String, usize> = HashMap::new();
    let mut value_to_placeholder: HashMap<(String, String), String> = HashMap::new();
    let mut result = Vec::with_capacity(raw.len());

    for m in raw {
        let prefix = m.kind.placeholder_prefix().to_string().to_uppercase();
        let key = (prefix.clone(), m.value.clone());
        let placeholder = value_to_placeholder
            .entry(key)
            .or_insert_with(|| {
                let n = counters.entry(prefix.clone()).or_insert(0);
                *n += 1;
                format!("[{}_{}]", prefix, *n)
            })
            .clone();

        result.push(PiiMatch {
            kind: m.kind,
            original: m.value,
            placeholder,
            byte_start: m.start,
            byte_end: m.end,
        });
    }
    result
}

fn apply_replacements(text: &str, matches: &[PiiMatch]) -> String {
    if matches.is_empty() {
        return text.to_string();
    }
    let mut sorted = matches.to_vec();
    sorted.sort_by_key(|m| m.byte_start);

    let mut out = String::with_capacity(text.len());
    let mut last = 0usize;
    for m in &sorted {
        if m.byte_start < last {
            // 不应发生（dedupe 已去 overlap），保险跳过
            continue;
        }
        out.push_str(&text[last..m.byte_start]);
        out.push_str(&m.placeholder);
        last = m.byte_end;
    }
    out.push_str(&text[last..]);
    out
}

fn compute_stats(matches: &[PiiMatch]) -> RedactionStats {
    let mut by_kind: HashMap<String, usize> = HashMap::new();
    let mut chars = 0usize;
    for m in matches {
        let prefix = m.kind.placeholder_prefix().to_string().to_uppercase();
        *by_kind.entry(prefix).or_insert(0) += 1;
        chars += m.original.chars().count();
    }
    RedactionStats {
        by_kind,
        total_matches: matches.len(),
        total_chars_redacted: chars,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_redactor() -> Redactor {
        Redactor::new()
    }

    #[test]
    fn empty_text_returns_empty() {
        let r = make_redactor();
        let result = r.redact("");
        assert!(result.redacted_text.is_empty());
        assert!(result.mappings.is_empty());
        assert_eq!(result.stats.total_matches, 0);
    }

    #[test]
    fn no_pii_returns_text_unchanged() {
        let r = make_redactor();
        let text = "今天天气很好，适合出去散步";
        let result = r.redact(text);
        assert_eq!(result.redacted_text, text);
        assert_eq!(result.stats.total_matches, 0);
    }

    #[test]
    fn redact_phone_then_restore() {
        let r = make_redactor();
        let text = "联系电话 13812345678 立即拨打";
        let result = r.redact(text);
        assert!(!result.redacted_text.contains("13812345678"));
        assert!(result.redacted_text.contains("[PHONE_1]"));
        assert_eq!(result.mappings.len(), 1);

        let restored = r.restore(&result.redacted_text, &result.mappings);
        assert_eq!(restored, text);
    }

    #[test]
    fn same_value_shares_placeholder() {
        let r = make_redactor();
        let text = "邮箱 a@b.com 备用 a@b.com 紧急 a@b.com";
        let result = r.redact(text);
        assert_eq!(result.mappings.len(), 3);
        // 三次出现，但都映射到同一个 placeholder
        let placeholders: std::collections::HashSet<_> =
            result.mappings.iter().map(|m| &m.placeholder).collect();
        assert_eq!(placeholders.len(), 1);
        assert!(result.redacted_text.contains("[EMAIL_1]"));
        assert!(!result.redacted_text.contains("a@b.com"));
    }

    #[test]
    fn different_values_get_different_placeholders() {
        let r = make_redactor();
        let text = "主邮箱 a@b.com 备用 c@d.com";
        let result = r.redact(text);
        assert_eq!(result.mappings.len(), 2);
        assert!(result.redacted_text.contains("[EMAIL_1]"));
        assert!(result.redacted_text.contains("[EMAIL_2]"));
    }

    #[test]
    fn mixed_pii_types() {
        let r = make_redactor();
        let text = "我是 13812345678，邮箱 user@example.com，IP 192.168.1.1";
        let result = r.redact(text);
        assert!(result.stats.total_matches >= 3);
        assert!(result.redacted_text.contains("[PHONE_1]"));
        assert!(result.redacted_text.contains("[EMAIL_1]"));
        assert!(result.redacted_text.contains("[IP_1]"));

        let restored = r.restore(&result.redacted_text, &result.mappings);
        assert!(restored.contains("13812345678"));
        assert!(restored.contains("user@example.com"));
        assert!(restored.contains("192.168.1.1"));
    }

    #[test]
    fn restore_with_long_index_does_not_collide() {
        // 模拟有 [PERSON_1] 和 [PERSON_10] 同时存在时的还原
        let mappings = vec![
            PiiMatch {
                kind: PiiKind::Custom("PERSON".into()),
                original: "Alice".into(),
                placeholder: "[PERSON_1]".into(),
                byte_start: 0,
                byte_end: 5,
            },
            PiiMatch {
                kind: PiiKind::Custom("PERSON".into()),
                original: "Bob".into(),
                placeholder: "[PERSON_10]".into(),
                byte_start: 0,
                byte_end: 3,
            },
        ];
        let r = make_redactor();
        let answer = "[PERSON_10] 比 [PERSON_1] 高";
        let restored = r.restore(answer, &mappings);
        assert_eq!(restored, "Bob 比 Alice 高");
    }

    #[test]
    fn stats_reflect_match_counts() {
        let r = make_redactor();
        let text = "phone 13812345678 alt 13987654321 mail a@b.com";
        let result = r.redact(text);
        assert_eq!(result.stats.by_kind.get("PHONE").copied().unwrap_or(0), 2);
        assert_eq!(result.stats.by_kind.get("EMAIL").copied().unwrap_or(0), 1);
    }

    #[test]
    fn vertical_plugin_pattern_via_add_pattern() {
        // 模拟 attune-pro/law-pro 注册案号 PII：
        // (2023)京01民终123号
        // 中间是中文+数字混排，所以字符类合并为 [一-龥\d]+
        let mut r = make_redactor();
        r.add_pattern("case_no", r"\(\d{4}\)[一-龥\d]+号").unwrap();

        let text = "本院审理(2023)京01民终123号一案";
        let result = r.redact(text);

        // 应命中案号
        assert_eq!(result.stats.by_kind.get("CASE_NO").copied().unwrap_or(0), 1);
        assert!(result.redacted_text.contains("[CASE_NO_1]"));
        assert!(!result.redacted_text.contains("(2023)"));

        let restored = r.restore(&result.redacted_text, &result.mappings);
        assert_eq!(restored, text);
    }

    #[test]
    fn placeholder_uppercase_normalization() {
        // name 里有大小写 / 下划线 → 统一升 upper
        let mut r = make_redactor();
        r.add_pattern("custom_thing", r"FOO\d+").unwrap();
        let result = r.redact("see FOO123 here");
        assert!(result.redacted_text.contains("[CUSTOM_THING_1]"));
    }

    // ── redact_batch (F-17 全路径接入) ──────────────────────────────────────

    #[test]
    fn redact_batch_empty_input_returns_empty() {
        let r = make_redactor();
        let segments: Vec<&str> = Vec::new();
        let (redacted, mappings) = r.redact_batch(&segments);
        assert!(redacted.is_empty());
        assert!(mappings.is_empty());
    }

    #[test]
    fn redact_batch_single_segment_equivalent_to_redact() {
        let r = make_redactor();
        let one = "phone 13812345678";
        let (redacted, mappings) = r.redact_batch(&[one]);
        assert_eq!(redacted.len(), 1);
        assert!(redacted[0].contains("[PHONE_1]"));
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].original, "13812345678");
    }

    #[test]
    fn redact_batch_global_unique_placeholders_across_segments() {
        // 关键不变量: user_message 中的 [PHONE_1] 与 knowledge 中的 [PHONE_2]
        // 必须指向不同原值, 否则 restore 时 ambiguous.
        let r = make_redactor();
        let user_msg = "my phone 13812345678";
        let knowledge = "客户联系方式 13987654321";
        let history = "alt phone 13755554444";

        let (redacted, mappings) = r.redact_batch(&[user_msg, history, knowledge]);

        assert_eq!(redacted.len(), 3, "3 segments → 3 redacted strings");

        // 各段都被 redact (含 placeholder)
        for (i, seg) in redacted.iter().enumerate() {
            assert!(
                seg.contains("[PHONE_"),
                "segment {} should contain PHONE placeholder, got: {}",
                i,
                seg
            );
        }

        // 全局共 3 个不同 phone → 3 个 mappings (placeholder 1/2/3)
        let phone_mappings: Vec<&PiiMatch> = mappings
            .iter()
            .filter(|m| matches!(m.kind, PiiKind::Phone))
            .collect();
        assert_eq!(phone_mappings.len(), 3);
        let placeholders: std::collections::HashSet<&str> = phone_mappings
            .iter()
            .map(|m| m.placeholder.as_str())
            .collect();
        assert_eq!(
            placeholders.len(),
            3,
            "3 distinct phones → 3 unique placeholders, got: {:?}",
            placeholders
        );

        // 每个 phone 对应正确的 placeholder (按 segments order)
        let originals: std::collections::HashMap<String, String> = phone_mappings
            .iter()
            .map(|m| (m.placeholder.clone(), m.original.clone()))
            .collect();
        assert!(originals.values().any(|v| v == "13812345678"));
        assert!(originals.values().any(|v| v == "13987654321"));
        assert!(originals.values().any(|v| v == "13755554444"));
    }

    #[test]
    fn redact_batch_same_value_in_different_segments_shares_placeholder() {
        // 设计语义: 同一 PHONE 在多段中出现也共享 placeholder (per assign_placeholders
        // value_to_placeholder 缓存语义). 这是有意的 — 让 LLM 看到同值时知道是同人.
        let r = make_redactor();
        let segments = vec!["alice 13812345678", "13812345678 confirmed"];
        let (redacted, mappings) = r.redact_batch(&segments);

        let phone_mappings: Vec<&PiiMatch> = mappings
            .iter()
            .filter(|m| matches!(m.kind, PiiKind::Phone))
            .collect();
        // 2 个命中 (一个 phone 在两段各出现一次), 但 placeholder 只 1 个 (同值共享)
        assert_eq!(phone_mappings.len(), 2);
        let placeholders: std::collections::HashSet<&str> = phone_mappings
            .iter()
            .map(|m| m.placeholder.as_str())
            .collect();
        assert_eq!(placeholders.len(), 1, "same value → same placeholder");

        // 两段都含同一 placeholder
        for seg in &redacted {
            assert!(seg.contains("[PHONE_1]"));
        }
    }

    #[test]
    fn redact_batch_restore_works_across_all_segments() {
        // 端到端验证: redact_batch 产出的 mappings 能 restore 全部段落.
        let r = make_redactor();
        let user = "phone 13812345678 email alice@example.com";
        let history = "earlier message with 13987654321";
        let knowledge = "context: api_key sk-1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF";

        let (redacted, mappings) = r.redact_batch(&[user, history, knowledge]);

        // 模拟 LLM 把所有 placeholder 都 echo 回来
        let llm_response = format!(
            "User reports {} and {}. Earlier said {}. Key={}",
            redacted[0].split_whitespace().nth(1).unwrap_or(""),  // [PHONE_1]
            redacted[0].split_whitespace().last().unwrap_or(""),   // [EMAIL_1]
            redacted[1].split_whitespace().last().unwrap_or(""),   // [PHONE_2]
            redacted[2].split_whitespace().last().unwrap_or(""),   // [APIKEY_1]
        );

        let restored = r.restore(&llm_response, &mappings);

        // 所有原始 PII 都应该在 restored 中
        assert!(restored.contains("13812345678"), "user phone restored: {}", restored);
        assert!(restored.contains("alice@example.com"), "user email restored: {}", restored);
        assert!(restored.contains("13987654321"), "history phone restored: {}", restored);
        assert!(restored.contains("sk-1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF"),
            "api_key restored: {}", restored);
    }

    // ── R14 v0.6.4 fuzz-style robustness for Redactor::redact ──────────────

    #[test]
    fn redact_does_not_panic_on_pathological_inputs() {
        // 防御 catastrophic regex backtracking + binary data + 超大输入.
        // regex crate (linear-time guarantee) 应该免疫 ReDoS, 但 Redactor 包装
        // 层有 panic 风险 (utf-8 boundary / overlap dedupe / placeholder collision).
        let r = make_redactor();
        let cases: Vec<String> = vec![
            "".to_string(),                                    // empty
            "a".repeat(100_000),                               // 100KB plain
            "a".repeat(1_000_000),                             // 1MB plain (regex linear time check)
            "@".repeat(10_000),                                // pathological email-like
            "1".repeat(50_000),                                // 50K digits (id card / phone / bank / credit prefixes)
            "https://".to_string() + &"a".repeat(50_000),      // long URL
            "中".repeat(10_000),                               // 10K cn chars
            "🌿".repeat(5_000),                                  // 5K emoji
            (0u8..=255).map(|b| b as char).collect::<String>(), // every byte as char
            "13800138000\n".repeat(10_000),                    // 10K phone numbers
            "tel://13800138000 ".repeat(1_000),                // mixed real PII
        ];
        for (i, input) in cases.iter().enumerate() {
            let _result = r.redact(input);  // must not panic
            // restore must also be panic-safe even with empty mappings
            let _restored = r.restore(input, &[]);
            // round-trip on text WITHOUT PII: redacted_text == original
            let r2 = r.redact(input);
            if r2.mappings.is_empty() {
                assert_eq!(r2.redacted_text, *input, "case {i}: PII-free input should pass through");
            }
        }
    }

    #[test]
    fn redact_handles_overlapping_pii_correctly() {
        // 重叠的 pattern (邮箱 + URL 中的 @): 不应导致 panic 或重复替换
        let r = make_redactor();
        let cases = vec![
            "邮箱 a@example.com 和 URL https://b@example.com 一起",
            "phone in URL https://13800138000.example.com/path",
            "13800138000也是13987654321的旁边",
        ];
        for input in cases {
            let result = r.redact(input);
            // dedup: 不应有 overlap match 被同时保留导致 redacted_text 字符乱
            let restored = r.restore(&result.redacted_text, &result.mappings);
            // 不变量: restore 后所有 placeholder 都被还原
            assert!(
                !restored.contains("[PHONE_") && !restored.contains("[EMAIL_") && !restored.contains("[URL_"),
                "restore should remove all placeholders, got: {}",
                restored
            );
        }
    }

    #[test]
    fn redact_invalid_utf8_boundary_safe() {
        // 字节级正则可能落在 multi-byte char 中间. Redactor 必须 char-boundary safe.
        let cases = vec![
            "前缀 13800138000 中文",                  // CJK bordering ASCII
            "🌿邮箱alice@example.com🌿",              // emoji + email
            "测试①13800138000测试②13987654321测试",   // CJK + numbered + phone
        ];
        let r = make_redactor();
        for input in cases {
            let result = r.redact(input);
            // restore 不应破坏 UTF-8
            let restored = r.restore(&result.redacted_text, &result.mappings);
            assert!(restored.is_char_boundary(0));
            assert!(restored.is_char_boundary(restored.len()));
            assert!(std::str::from_utf8(restored.as_bytes()).is_ok(), "valid UTF-8");
        }
    }

    #[test]
    fn redact_batch_empty_strings_preserved() {
        // 空字符串作为 segment 也要保留 (chat history 可能含空 message)
        let r = make_redactor();
        let segments = vec!["phone 13812345678", "", "another no pii"];
        let (redacted, _) = r.redact_batch(&segments);
        assert_eq!(redacted.len(), 3);
        assert!(redacted[0].contains("[PHONE_1]"));
        assert_eq!(redacted[1], "", "empty segment preserved as empty");
        assert_eq!(redacted[2], "another no pii", "no-PII segment unchanged");
    }
}
