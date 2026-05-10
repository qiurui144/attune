//! parse_chinese_date — 中文日期文本 → ISO 8601 日期 (per protocol §2 OSS 内部 skill)
//!
//! 纯规则 (无 LLM), 可缓存. 用于 document_classifier_agent 内部 dates 字段抽取.
//!
//! 支持的输入格式:
//! - "2023年1月15日" → "2023-01-15"
//! - "2023年01月15日" → "2023-01-15"
//! - "2023/1/15" → "2023-01-15"
//! - "2023-01-15" (已是 ISO) → "2023-01-15"
//! - "二〇二三年一月十五日" → "2023-01-15"
//! - 含上下文文本: "签订日期为2023年1月15日上午" → 返第一个匹配
//!
//! 不支持 (返 None, 律师 Stage 3 人工填):
//! - 模糊日期 ("去年", "上个月")
//! - 农历日期
//! - 范围日期 ("2023年1月至3月")

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParsedDate {
    /// ISO 8601: "YYYY-MM-DD"
    pub iso: String,
    /// 原始字符串切片 (供 audit_trail)
    pub matched_text: String,
}

/// 主入口 — 从文本中抽出第一个能识别的中文日期
pub fn parse_first(text: &str) -> Option<ParsedDate> {
    // 优先级: 数字格式 (YYYY年MM月DD日, YYYY/MM/DD, YYYY-MM-DD) > 中文数字格式
    parse_numeric(text).or_else(|| parse_chinese_numerals(text))
}

/// 抽出文本中**所有**中文/数字日期 (用于扫长文档)
pub fn parse_all(text: &str) -> Vec<ParsedDate> {
    let mut out = Vec::new();
    out.extend(find_all_numeric(text));
    out.extend(find_all_chinese_numerals(text));
    out
}

/// 数字格式: 2023年1月15日 / 2023/1/15 / 2023-01-15
fn parse_numeric(text: &str) -> Option<ParsedDate> {
    find_all_numeric(text).into_iter().next()
}

fn find_all_numeric(text: &str) -> Vec<ParsedDate> {
    use regex::Regex;
    // 三种分隔: 年月日 / / / -
    let re = Regex::new(r"(\d{4})\s*[年/\-]\s*(\d{1,2})\s*[月/\-]\s*(\d{1,2})\s*日?").unwrap();
    let mut out = Vec::new();
    for cap in re.captures_iter(text) {
        let year: i32 = cap[1].parse().unwrap_or(0);
        let month: u32 = cap[2].parse().unwrap_or(0);
        let day: u32 = cap[3].parse().unwrap_or(0);
        if !is_valid_date(year, month, day) {
            continue;
        }
        let matched = cap.get(0).unwrap().as_str().to_string();
        out.push(ParsedDate {
            iso: format!("{year:04}-{month:02}-{day:02}"),
            matched_text: matched,
        });
    }
    out
}

/// 中文数字格式: 二〇二三年一月十五日
fn parse_chinese_numerals(text: &str) -> Option<ParsedDate> {
    find_all_chinese_numerals(text).into_iter().next()
}

fn find_all_chinese_numerals(text: &str) -> Vec<ParsedDate> {
    use regex::Regex;
    // 中文数字 + 月日 (年部分允许 4 字符全中文如 "二〇二三")
    let re = Regex::new(r"([〇零一二三四五六七八九十]{4})年\s*([一二三四五六七八九十]{1,3})月\s*([一二三四五六七八九十]{1,3})日").unwrap();
    let mut out = Vec::new();
    for cap in re.captures_iter(text) {
        let year_str = &cap[1];
        let month_str = &cap[2];
        let day_str = &cap[3];

        let year = chinese_to_year(year_str);
        let month = chinese_to_number(month_str);
        let day = chinese_to_number(day_str);

        if let (Some(y), Some(m), Some(d)) = (year, month, day) {
            if is_valid_date(y, m, d) {
                out.push(ParsedDate {
                    iso: format!("{y:04}-{m:02}-{d:02}"),
                    matched_text: cap.get(0).unwrap().as_str().to_string(),
                });
            }
        }
    }
    out
}

fn chinese_to_year(s: &str) -> Option<i32> {
    // 4 字符: "二〇二三" / "一九九九"
    let mut digits = String::new();
    for c in s.chars() {
        let d = match c {
            '〇' | '零' => '0',
            '一' => '1',
            '二' => '2',
            '三' => '3',
            '四' => '4',
            '五' => '5',
            '六' => '6',
            '七' => '7',
            '八' => '8',
            '九' => '9',
            _ => return None,
        };
        digits.push(d);
    }
    digits.parse().ok()
}

fn chinese_to_number(s: &str) -> Option<u32> {
    // 月 / 日: "一" / "十" / "十二" / "二十" / "二十五" / "三十"
    let chars: Vec<char> = s.chars().collect();
    let val = |c: char| -> Option<u32> {
        match c {
            '〇' | '零' => Some(0),
            '一' => Some(1),
            '二' => Some(2),
            '三' => Some(3),
            '四' => Some(4),
            '五' => Some(5),
            '六' => Some(6),
            '七' => Some(7),
            '八' => Some(8),
            '九' => Some(9),
            _ => None,
        }
    };
    match chars.as_slice() {
        [c] if *c == '十' => Some(10),
        [c] => val(*c),
        ['十', c] => val(*c).map(|n| 10 + n),
        [c, '十'] => val(*c).map(|n| n * 10),
        [c1, '十', c2] => match (val(*c1), val(*c2)) {
            (Some(a), Some(b)) => Some(a * 10 + b),
            _ => None,
        },
        _ => None,
    }
}

fn is_valid_date(year: i32, month: u32, day: u32) -> bool {
    if !(1900..=2100).contains(&year) {
        return false;
    }
    if !(1..=12).contains(&month) {
        return false;
    }
    if !(1..=31).contains(&day) {
        return false;
    }
    // chrono 校验闰年 / 月长度
    chrono::NaiveDate::from_ymd_opt(year, month, day).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_year_month_day() {
        let r = parse_first("签订日期为2023年1月15日上午").expect("parse");
        assert_eq!(r.iso, "2023-01-15");
        assert!(r.matched_text.contains("2023年1月15日"));
    }

    #[test]
    fn numeric_with_zero_padding() {
        let r = parse_first("2023年01月15日").expect("parse");
        assert_eq!(r.iso, "2023-01-15");
    }

    #[test]
    fn slash_format() {
        let r = parse_first("2023/1/15 入账").expect("parse");
        assert_eq!(r.iso, "2023-01-15");
    }

    #[test]
    fn iso_format_passes_through() {
        let r = parse_first("2023-01-15").expect("parse");
        assert_eq!(r.iso, "2023-01-15");
    }

    #[test]
    fn chinese_numeral_year() {
        let r = parse_first("立约日期为二〇二三年一月十五日").expect("parse");
        assert_eq!(r.iso, "2023-01-15");
    }

    #[test]
    fn chinese_numeral_compound_day() {
        let r = parse_first("二〇二三年十二月二十五日").expect("parse");
        assert_eq!(r.iso, "2023-12-25");
    }

    #[test]
    fn chinese_numeral_thirty() {
        let r = parse_first("二〇二三年十一月三十日").expect("parse");
        assert_eq!(r.iso, "2023-11-30");
    }

    #[test]
    fn invalid_date_rejected() {
        assert!(parse_first("2023年13月45日").is_none());
        assert!(parse_first("2023年2月30日").is_none()); // 2月没30
    }

    #[test]
    fn out_of_range_year_rejected() {
        assert!(parse_first("1800年1月1日").is_none());
        assert!(parse_first("2200年1月1日").is_none());
    }

    #[test]
    fn no_date_returns_none() {
        assert!(parse_first("没有日期的文本").is_none());
        assert!(parse_first("").is_none());
    }

    #[test]
    fn parse_all_collects_multiple() {
        let text = "合同签订于2023年1月15日, 履行日 2023/3/20, 解除日二〇二三年六月十日";
        let dates = parse_all(text);
        assert_eq!(dates.len(), 3);
        assert_eq!(dates[0].iso, "2023-01-15");
        assert_eq!(dates[1].iso, "2023-03-20");
        assert_eq!(dates[2].iso, "2023-06-10");
    }

    #[test]
    fn fuzzy_dates_not_supported() {
        // "去年" / "上个月" 等模糊日期不支持, 律师 Stage 3 人工填
        assert!(parse_first("去年签的合同").is_none());
        assert!(parse_first("上个月还款").is_none());
    }

    #[test]
    fn leap_year_validated() {
        assert_eq!(parse_first("2024年2月29日").map(|d| d.iso).as_deref(), Some("2024-02-29"));
        assert!(parse_first("2023年2月29日").is_none()); // 2023 非闰年
    }
}
