//! `receipt_v1` — 发票/收据 7 字段抽取.
//!
//! Spec §4.2 字段集合: invoice_no, issue_date, seller, buyer,
//! amount_total, tax_amount, amount_chinese.
//!
//! 路径: 正则锚点 + bbox 邻近 + 校验函数 (normalize_date /
//! normalize_amount / 大写金额数字解析). 零 LLM.

use super::normalize;
use super::{find_value_after_anchor, FieldValue, StructuredFields};
use crate::ocr::RawLine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReceiptFields {
    pub invoice_no: FieldValue,
    pub issue_date: FieldValue,
    pub seller: FieldValue,
    pub buyer: FieldValue,
    pub amount_total: FieldValue,
    pub tax_amount: FieldValue,
    pub amount_chinese: FieldValue,
}

/// 抽取主入口.
pub fn extract(lines: &[RawLine]) -> StructuredFields {
    let mut fields = ReceiptFields::default();
    let mut unrecognized: Vec<&'static str> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // ─── invoice_no ─────────────────────────────────────────────────────
    // 锚点优先级: "发票号码" > "号码" (避免误匹配 "电话号码")
    let anchor_invoice = regex::Regex::new(r"发票号码|发\s*票\s*号|Invoice\s*No\.?").unwrap();
    if let Some((idx, raw, conf)) = find_value_after_anchor(lines, &anchor_invoice, 1) {
        let cleaned: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
        if cleaned.len() >= 6 {
            fields.invoice_no =
                FieldValue::from_line(cleaned, conf.min(0.95), &lines[idx], idx);
        } else {
            unrecognized.push("invoice_no");
        }
    } else {
        unrecognized.push("invoice_no");
    }

    // ─── issue_date ─────────────────────────────────────────────────────
    let anchor_date = regex::Regex::new(r"开票日期|开\s*票\s*日\s*期|Issue\s*Date").unwrap();
    if let Some((idx, raw, conf)) = find_value_after_anchor(lines, &anchor_date, 1) {
        if let Some(iso) = normalize::normalize_date(&raw) {
            fields.issue_date =
                FieldValue::from_line(iso, conf.min(0.95), &lines[idx], idx);
        } else {
            fields.issue_date =
                FieldValue::from_line(raw.clone(), conf * 0.5, &lines[idx], idx);
            warnings.push(format!("issue_date raw value '{raw}' failed date parse"));
        }
    } else {
        unrecognized.push("issue_date");
    }

    // ─── seller / buyer ─────────────────────────────────────────────────
    let (seller, _) = extract_party(lines, "销售方");
    fields.seller = seller;
    if fields.seller.value.is_none() {
        unrecognized.push("seller");
    }
    let (buyer, _) = extract_party(lines, "购买方");
    fields.buyer = buyer;
    if fields.buyer.value.is_none() {
        unrecognized.push("buyer");
    }

    // ─── amount_total ───────────────────────────────────────────────────
    let anchor_total =
        regex::Regex::new(r"价税合计|合计金额|应付金额|应\s*付|Total\s*Amount").unwrap();
    if let Some((idx, raw, conf)) = find_value_after_anchor(lines, &anchor_total, 1) {
        if let Some(amt) = normalize::normalize_amount(&raw) {
            fields.amount_total =
                FieldValue::from_line(amt, conf.min(0.95), &lines[idx], idx);
        } else {
            unrecognized.push("amount_total");
        }
    } else {
        unrecognized.push("amount_total");
    }

    // ─── tax_amount ─────────────────────────────────────────────────────
    // 锚点 "税额" 必须排除 "税率"
    let anchor_tax = regex::Regex::new(r"税\s*额(?:[^率]|$)|Tax\s*Amount").unwrap();
    if let Some((idx, raw, conf)) = find_value_after_anchor(lines, &anchor_tax, 1) {
        if let Some(amt) = normalize::normalize_amount(&raw) {
            let mut conf_adjusted = conf.min(0.95);
            if let Some(total_str) = &fields.amount_total.value {
                if let (Ok(t), Ok(tot)) = (amt.parse::<f64>(), total_str.parse::<f64>()) {
                    if t > tot + 0.01 {
                        warnings.push(format!(
                            "tax_amount {amt} > amount_total {total_str} (校验失败)"
                        ));
                        conf_adjusted *= 0.5;
                    }
                }
            }
            fields.tax_amount =
                FieldValue::from_line(amt, conf_adjusted, &lines[idx], idx);
        } else {
            unrecognized.push("tax_amount");
        }
    } else {
        unrecognized.push("tax_amount");
    }

    // ─── amount_chinese ─────────────────────────────────────────────────
    if let Some((idx, line)) = find_chinese_amount_line(lines) {
        let parsed_f = parse_chinese_amount(&line.text);
        let mut conf = line.confidence.min(0.95);
        if let (Some(c), Some(t)) = (parsed_f, fields.amount_total.value.as_deref()) {
            if let Ok(tv) = t.parse::<f64>() {
                if (c - tv).abs() > 0.1 {
                    warnings.push(format!(
                        "amount_chinese decoded {c:.2} ≠ amount_total {tv:.2} (校验失败)"
                    ));
                    conf *= 0.5;
                }
            }
        }
        fields.amount_chinese = FieldValue::from_line(line.text.clone(), conf, line, idx);
    } else {
        unrecognized.push("amount_chinese");
    }

    StructuredFields::ReceiptV1 {
        fields,
        unrecognized_fields: unrecognized.iter().map(|s| s.to_string()).collect(),
        validation_warnings: warnings,
    }
}

/// 解析 "销售方/购买方" 区块的名称行.
fn extract_party(lines: &[RawLine], anchor: &str) -> (FieldValue, Option<usize>) {
    let name_re = regex::Regex::new(r"名\s*称\s*[:：]?\s*(.+)").unwrap();
    for (i, l) in lines.iter().enumerate() {
        if !l.text.contains(anchor) {
            continue;
        }
        for off in 0..=4 {
            let Some(line) = lines.get(i + off) else { break };
            if !line.text.contains("名称") {
                continue;
            }
            if let Some(cap) = name_re.captures(&line.text) {
                if let Some(m) = cap.get(1) {
                    let v = m.as_str().trim().to_string();
                    if !v.is_empty() {
                        return (
                            FieldValue::from_line(v, line.confidence.min(0.9), line, i + off),
                            Some(i + off),
                        );
                    }
                }
            }
            if let Some(next) = lines.get(i + off + 1) {
                let t = next.text.trim().to_string();
                if !t.is_empty() && !t.contains("名称") && !t.contains("税号") {
                    return (
                        FieldValue::from_line(
                            t,
                            next.confidence.min(0.85),
                            next,
                            i + off + 1,
                        ),
                        Some(i + off + 1),
                    );
                }
            }
        }
    }
    (FieldValue::none(), None)
}

fn find_chinese_amount_line(lines: &[RawLine]) -> Option<(usize, &RawLine)> {
    let chinese_digits = ['壹', '贰', '叁', '肆', '伍', '陆', '柒', '捌', '玖'];
    let units = ['元', '角', '分', '圆'];
    for (i, l) in lines.iter().enumerate() {
        let has_digit = chinese_digits.iter().any(|c| l.text.contains(*c));
        let has_unit = units.iter().any(|c| l.text.contains(*c));
        if has_digit && has_unit {
            return Some((i, l));
        }
    }
    None
}

/// 解析大写金额 → f64. 失败返 None.
fn parse_chinese_amount(s: &str) -> Option<f64> {
    let digit_map = |c: char| -> Option<u64> {
        match c {
            '零' => Some(0),
            '壹' | '一' => Some(1),
            '贰' | '二' => Some(2),
            '叁' | '三' => Some(3),
            '肆' | '四' => Some(4),
            '伍' | '五' => Some(5),
            '陆' | '六' => Some(6),
            '柒' | '七' => Some(7),
            '捌' | '八' => Some(8),
            '玖' | '九' => Some(9),
            _ => None,
        }
    };
    let unit_map = |c: char| -> Option<u64> {
        match c {
            '拾' | '十' => Some(10),
            '佰' | '百' => Some(100),
            '仟' | '千' => Some(1_000),
            _ => None,
        }
    };
    let section_map = |c: char| -> Option<u64> {
        match c {
            '万' | '萬' => Some(10_000),
            '亿' | '億' => Some(100_000_000),
            _ => None,
        }
    };

    let mut total_int: u64 = 0;
    let mut section_sum: u64 = 0;
    let mut current: u64 = 0;
    let mut fractional_jiao: u64 = 0;
    let mut fractional_fen: u64 = 0;
    let mut in_fractional = false;
    let mut saw_any_digit = false;

    for c in s.chars() {
        if let Some(d) = digit_map(c) {
            current = d;
            saw_any_digit = true;
        } else if let Some(u) = unit_map(c) {
            if current == 0 {
                current = 1;
            }
            section_sum += current * u;
            current = 0;
        } else if let Some(sec) = section_map(c) {
            section_sum += current;
            total_int += section_sum * sec;
            section_sum = 0;
            current = 0;
        } else if c == '元' || c == '圆' {
            section_sum += current;
            total_int += section_sum;
            section_sum = 0;
            current = 0;
            in_fractional = true;
        } else if c == '角' {
            fractional_jiao = current;
            current = 0;
        } else if c == '分' {
            fractional_fen = current;
            current = 0;
        }
    }
    if !in_fractional && total_int == 0 && (section_sum > 0 || current > 0) {
        total_int = section_sum + current;
    }
    if !saw_any_digit {
        return None;
    }
    let val = total_int as f64 + fractional_jiao as f64 * 0.1 + fractional_fen as f64 * 0.01;
    Some(val)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::BBox;

    fn rl(text: &str, y: u32) -> RawLine {
        RawLine {
            text: text.into(),
            bbox: BBox { x: 0, y, w: 200, h: 30 },
            confidence: 0.95,
        }
    }

    #[test]
    fn extracts_invoice_no_same_line() {
        let lines = vec![rl("发票号码: 12345678", 0)];
        let StructuredFields::ReceiptV1 { fields, .. } = extract(&lines) else {
            panic!("wrong schema");
        };
        assert_eq!(fields.invoice_no.value.as_deref(), Some("12345678"));
        assert!(fields.invoice_no.confidence > 0.5);
    }

    #[test]
    fn extracts_invoice_no_next_line() {
        let lines = vec![rl("发票号码", 0), rl("87654321", 40)];
        let StructuredFields::ReceiptV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert_eq!(fields.invoice_no.value.as_deref(), Some("87654321"));
    }

    #[test]
    fn extracts_issue_date_iso() {
        let lines = vec![rl("开票日期: 2026年05月18日", 0)];
        let StructuredFields::ReceiptV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert_eq!(fields.issue_date.value.as_deref(), Some("2026-05-18"));
    }

    #[test]
    fn extracts_amount_total_and_tax() {
        let lines = vec![
            rl("价税合计: ￥1234.56", 0),
            rl("税额: 137.17", 40),
        ];
        let StructuredFields::ReceiptV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert_eq!(fields.amount_total.value.as_deref(), Some("1234.56"));
        assert_eq!(fields.tax_amount.value.as_deref(), Some("137.17"));
    }

    #[test]
    fn tax_amount_excludes_tax_rate_line() {
        let lines = vec![
            rl("税率: 13%", 0),
            rl("税额: 50.00", 40),
            rl("价税合计: 500.00", 80),
        ];
        let StructuredFields::ReceiptV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert_eq!(fields.tax_amount.value.as_deref(), Some("50.00"));
    }

    #[test]
    fn validates_tax_le_total() {
        let lines = vec![
            rl("价税合计: 100.00", 0),
            rl("税额: 200.00", 40),
        ];
        let StructuredFields::ReceiptV1 { fields, validation_warnings, .. } = extract(&lines) else {
            unreachable!()
        };
        assert!(validation_warnings.iter().any(|w| w.contains("tax_amount")));
        assert!(fields.tax_amount.confidence < 0.6);
    }

    #[test]
    fn extracts_seller_and_buyer() {
        let lines = vec![
            rl("销售方", 0),
            rl("名称: ABC 科技有限公司", 40),
            rl("购买方", 200),
            rl("名称: XYZ 咨询有限公司", 240),
        ];
        let StructuredFields::ReceiptV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert_eq!(fields.seller.value.as_deref(), Some("ABC 科技有限公司"));
        assert_eq!(fields.buyer.value.as_deref(), Some("XYZ 咨询有限公司"));
    }

    #[test]
    fn extracts_amount_chinese_with_cross_validation() {
        let lines = vec![
            rl("价税合计: 1234.56", 0),
            rl("大写: 壹仟贰佰叁拾肆元伍角陆分", 40),
        ];
        let StructuredFields::ReceiptV1 { fields, validation_warnings, .. } = extract(&lines) else {
            unreachable!()
        };
        assert!(fields.amount_chinese.value.is_some());
        assert!(validation_warnings.is_empty(), "no validation warning expected, got {validation_warnings:?}");
        assert!(fields.amount_chinese.confidence > 0.6);
    }

    #[test]
    fn amount_chinese_mismatch_warns_and_downgrades() {
        let lines = vec![
            rl("价税合计: 9999.99", 0),
            rl("大写: 壹仟贰佰叁拾肆元伍角陆分", 40),
        ];
        let StructuredFields::ReceiptV1 { fields, validation_warnings, .. } = extract(&lines) else {
            unreachable!()
        };
        assert!(validation_warnings.iter().any(|w| w.contains("amount_chinese")));
        assert!(fields.amount_chinese.confidence < 0.6);
    }

    #[test]
    fn returns_unrecognized_for_empty_lines() {
        let StructuredFields::ReceiptV1 { unrecognized_fields, .. } = extract(&[]) else {
            unreachable!()
        };
        assert!(!unrecognized_fields.is_empty());
        assert!(unrecognized_fields.contains(&"invoice_no".to_string()));
        assert!(unrecognized_fields.contains(&"issue_date".to_string()));
    }

    #[test]
    fn invoice_no_strips_non_digits() {
        let lines = vec![rl("发票号码: No.12345-678", 0)];
        let StructuredFields::ReceiptV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert_eq!(fields.invoice_no.value.as_deref(), Some("12345678"));
    }

    #[test]
    fn chinese_amount_basic() {
        let v = parse_chinese_amount("壹仟贰佰叁拾肆元伍角陆分").unwrap();
        assert!((v - 1234.56).abs() < 0.001, "got {v}");
    }

    #[test]
    fn chinese_amount_integer_only() {
        assert_eq!(parse_chinese_amount("伍佰元整"), Some(500.0));
    }

    #[test]
    fn chinese_amount_with_zero_ten() {
        assert_eq!(parse_chinese_amount("壹佰零伍元"), Some(105.0));
    }

    #[test]
    fn chinese_amount_garbage_returns_none() {
        assert_eq!(parse_chinese_amount("hello"), None);
    }
}
