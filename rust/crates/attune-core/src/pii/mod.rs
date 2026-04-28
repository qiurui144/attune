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
}
