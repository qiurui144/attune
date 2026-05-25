//! `id_card_cn_v1` / `bank_card_v1` / `business_license_v1` — 3 子类型卡证抽取.
//!
//! Spec §4.2: subtype 由调用方显式指定, OCR 不猜 (per "不确定就问律师"原则).
//! 高准确度红线 (95%, 卡证字段位置固定).
//!
//! 三个子类型各自独立实现, 共用 normalize_date + luhn_check + id_card_cn_check +
//! business_license_check 校验.

use super::normalize;
use super::{find_value_after_anchor, FieldValue, StructuredFields};
use crate::ocr::RawLine;
use serde::{Deserialize, Serialize};

// ─── 居民身份证 (id_card_cn_v1) ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IdCardCnFields {
    pub name: FieldValue,
    pub gender: FieldValue,
    pub nationality: FieldValue,
    pub birth_date: FieldValue,
    pub address: FieldValue,
    pub id_number: FieldValue,
}

// ─── 银行卡 (bank_card_v1) ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BankCardFields {
    pub card_number: FieldValue,
    pub bank_name: FieldValue,
    pub card_type: FieldValue,
    pub valid_thru: FieldValue,
}

// ─── 营业执照 (business_license_v1) ──────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BusinessLicenseFields {
    pub registration_no: FieldValue,
    pub company_name: FieldValue,
    pub legal_rep: FieldValue,
    pub registered_capital: FieldValue,
    pub established_date: FieldValue,
    pub scope: FieldValue,
}

/// 入口路由 — 按 subtype 选 schema.
pub fn extract(lines: &[RawLine], subtype: &str) -> Option<StructuredFields> {
    match subtype {
        "id_card_cn" => Some(extract_id_card_cn(lines)),
        "bank_card" => Some(extract_bank_card(lines)),
        "business_license" => Some(extract_business_license(lines)),
        _ => None,
    }
}

// ─── id_card_cn 实施 ────────────────────────────────────────────────────

fn extract_id_card_cn(lines: &[RawLine]) -> StructuredFields {
    let mut fields = IdCardCnFields::default();
    let mut unrecognized: Vec<&'static str> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // 姓名
    let anchor_name = regex::Regex::new(r"^\s*姓\s*名").unwrap();
    if let Some((idx, raw, conf)) = find_value_after_anchor(lines, &anchor_name, 1) {
        fields.name = FieldValue::from_line(raw.trim().to_string(), conf.min(0.95), &lines[idx], idx);
    } else {
        unrecognized.push("name");
    }

    // 性别 — anchor 优先; 若 anchor value 未含 男/女 (OCR 把 label+value 串到一行时
    // find_value_after_anchor 可能取到"民族"标签文本), 退回全文正则扫描.
    let anchor_gender = regex::Regex::new(r"性\s*别").unwrap();
    let gender_fallback_re = regex::Regex::new(r"性\s*别\s*[:：]?\s*(男|女)").unwrap();
    let gender_val = if let Some((idx, raw, conf)) = find_value_after_anchor(lines, &anchor_gender, 1) {
        if raw.contains('男') {
            Some(FieldValue::from_line("男".to_string(), conf.min(0.95), &lines[idx], idx))
        } else if raw.contains('女') {
            Some(FieldValue::from_line("女".to_string(), conf.min(0.95), &lines[idx], idx))
        } else {
            // anchor found but value doesn't contain 男/女 — try full-text fallback
            lines.iter().enumerate().find_map(|(i, l)| {
                gender_fallback_re.captures(&l.text).map(|cap| {
                    FieldValue::from_line(cap[1].to_string(), l.confidence.min(0.92), l, i)
                })
            })
        }
    } else {
        // no anchor match at all — try full-text fallback
        lines.iter().enumerate().find_map(|(i, l)| {
            gender_fallback_re.captures(&l.text).map(|cap| {
                FieldValue::from_line(cap[1].to_string(), l.confidence.min(0.92), l, i)
            })
        })
    };
    if let Some(fv) = gender_val {
        fields.gender = fv;
    } else {
        unrecognized.push("gender");
    }

    // 民族
    let anchor_nat = regex::Regex::new(r"民\s*族").unwrap();
    if let Some((idx, raw, conf)) = find_value_after_anchor(lines, &anchor_nat, 1) {
        fields.nationality =
            FieldValue::from_line(raw.trim().to_string(), conf.min(0.95), &lines[idx], idx);
    } else {
        unrecognized.push("nationality");
    }

    // 出生日期
    let anchor_birth = regex::Regex::new(r"出\s*生").unwrap();
    if let Some((idx, raw, conf)) = find_value_after_anchor(lines, &anchor_birth, 1) {
        if let Some(iso) = normalize::normalize_date(&raw) {
            fields.birth_date = FieldValue::from_line(iso, conf.min(0.95), &lines[idx], idx);
        } else {
            fields.birth_date =
                FieldValue::from_line(raw.clone(), conf * 0.5, &lines[idx], idx);
            warnings.push(format!("birth_date raw '{raw}' failed parse"));
        }
    } else {
        unrecognized.push("birth_date");
    }

    // 住址
    let anchor_addr = regex::Regex::new(r"住\s*址|地\s*址").unwrap();
    if let Some((idx, raw, conf)) = find_value_after_anchor(lines, &anchor_addr, 2) {
        fields.address =
            FieldValue::from_line(raw.trim().to_string(), conf.min(0.92), &lines[idx], idx);
    } else {
        unrecognized.push("address");
    }

    // 身份证号
    let anchor_id =
        regex::Regex::new(r"公\s*民\s*身\s*份\s*号\s*码|身\s*份\s*证\s*号").unwrap();
    let mut id_found = false;
    if let Some((idx, raw, conf)) = find_value_after_anchor(lines, &anchor_id, 1) {
        let cleaned: String = raw.chars().filter(|c| c.is_alphanumeric()).collect();
        if cleaned.len() == 18 {
            let mut conf_adj = conf.min(0.95);
            if !normalize::id_card_cn_check(&cleaned) {
                warnings.push(format!(
                    "id_number {} 校验位不符 (GB 11643)",
                    cleaned
                ));
                conf_adj *= 0.5;
            }
            fields.id_number =
                FieldValue::from_line(cleaned, conf_adj, &lines[idx], idx);
            id_found = true;
        }
    }
    if !id_found {
        // 兜底: 全文扫 18 位数字+X 模式 (有时 OCR 把 anchor 分割)
        let id_re = regex::Regex::new(r"\d{17}[\dXx]").unwrap();
        for (i, l) in lines.iter().enumerate() {
            if let Some(m) = id_re.find(&l.text) {
                let cleaned = m.as_str().to_uppercase();
                let mut conf_adj = l.confidence.min(0.93);
                if !normalize::id_card_cn_check(&cleaned) {
                    warnings.push(format!("id_number {cleaned} 校验位不符"));
                    conf_adj *= 0.5;
                }
                fields.id_number = FieldValue::from_line(cleaned, conf_adj, l, i);
                id_found = true;
                break;
            }
        }
    }
    if !id_found {
        unrecognized.push("id_number");
    }

    StructuredFields::IdCardCnV1 {
        fields,
        unrecognized_fields: unrecognized.iter().map(|s| s.to_string()).collect(),
        validation_warnings: warnings,
    }
}

// ─── bank_card 实施 ─────────────────────────────────────────────────────

fn extract_bank_card(lines: &[RawLine]) -> StructuredFields {
    let mut fields = BankCardFields::default();
    let mut unrecognized: Vec<&'static str> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // card_number: 16-19 位数字 (容许空格 / 分隔)
    let card_re = regex::Regex::new(r"(\d[\d\s\-]{14,22}\d)").unwrap();
    let mut card_found = false;
    for (i, l) in lines.iter().enumerate() {
        if let Some(cap) = card_re.captures(&l.text) {
            let raw = cap.get(1).unwrap().as_str();
            let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
            if !(13..=19).contains(&digits.len()) {
                continue;
            }
            let mut conf = l.confidence.min(0.95);
            if !normalize::luhn_check(&digits) {
                warnings.push(format!("card_number {digits} Luhn 校验位错误"));
                conf *= 0.5;
            }
            // 保留 4 位一组的可读格式
            let formatted: String = digits
                .chars()
                .collect::<Vec<_>>()
                .chunks(4)
                .map(|c| c.iter().collect::<String>())
                .collect::<Vec<_>>()
                .join(" ");
            fields.card_number = FieldValue::from_line(formatted, conf, l, i);
            card_found = true;
            break;
        }
    }
    if !card_found {
        unrecognized.push("card_number");
    }

    // bank_name: 常见银行关键词
    let bank_keywords = [
        "中国工商银行", "工商银行", "工行",
        "中国农业银行", "农业银行", "农行",
        "中国建设银行", "建设银行", "建行",
        "中国银行",
        "交通银行", "交行",
        "招商银行", "招行",
        "中信银行", "民生银行", "光大银行", "华夏银行",
        "兴业银行", "浦发银行", "平安银行", "邮政储蓄",
        "ICBC", "ABC", "BOC", "CCB", "BoCom",
    ];
    for (i, l) in lines.iter().enumerate() {
        if let Some(k) = bank_keywords.iter().find(|k| l.text.contains(*k)) {
            fields.bank_name =
                FieldValue::from_line((*k).to_string(), l.confidence.min(0.93), l, i);
            break;
        }
    }
    if fields.bank_name.value.is_none() {
        unrecognized.push("bank_name");
    }

    // card_type: 借记卡 / 信用卡 / 储蓄卡
    let type_keywords = [
        "借记卡", "信用卡", "储蓄卡", "贷记卡", "准贷记卡", "Debit", "Credit",
    ];
    for (i, l) in lines.iter().enumerate() {
        if let Some(k) = type_keywords.iter().find(|k| l.text.contains(*k)) {
            fields.card_type =
                FieldValue::from_line((*k).to_string(), l.confidence.min(0.92), l, i);
            break;
        }
    }
    if fields.card_type.value.is_none() {
        unrecognized.push("card_type");
    }

    // valid_thru: MM/YY or VALID THRU MM/YY
    let valid_re = regex::Regex::new(r"(\d{2})\s*/\s*(\d{2,4})").unwrap();
    // 跳过身份证号或卡号那种长数字串 (≥ 8 位连续数字) — 编译一次,循环复用
    let long_digit = regex::Regex::new(r"\d{8,}").unwrap();
    for (i, l) in lines.iter().enumerate() {
        if long_digit.is_match(&l.text) {
            continue;
        }
        if let Some(cap) = valid_re.captures(&l.text) {
            let mm = cap.get(1).unwrap().as_str();
            let yy = cap.get(2).unwrap().as_str();
            let mm_n: u32 = mm.parse().unwrap_or(0);
            if (1..=12).contains(&mm_n) {
                fields.valid_thru =
                    FieldValue::from_line(format!("{mm}/{yy}"), l.confidence.min(0.9), l, i);
                break;
            }
        }
    }
    if fields.valid_thru.value.is_none() {
        unrecognized.push("valid_thru");
    }

    StructuredFields::BankCardV1 {
        fields,
        unrecognized_fields: unrecognized.iter().map(|s| s.to_string()).collect(),
        validation_warnings: warnings,
    }
}

// ─── business_license 实施 ──────────────────────────────────────────────

fn extract_business_license(lines: &[RawLine]) -> StructuredFields {
    let mut fields = BusinessLicenseFields::default();
    let mut unrecognized: Vec<&'static str> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // 统一社会信用代码 (18 位)
    let reg_re = regex::Regex::new(r"[0-9A-HJ-NP-RT-UWXY]{18}").unwrap();
    let mut reg_found = false;
    for (i, l) in lines.iter().enumerate() {
        if let Some(m) = reg_re.find(&l.text.to_uppercase()) {
            let raw = m.as_str().to_string();
            let mut conf = l.confidence.min(0.95);
            if !normalize::business_license_check(&raw) {
                warnings.push(format!("registration_no {raw} 校验位不符 (GB 32100)"));
                conf *= 0.5;
            }
            fields.registration_no = FieldValue::from_line(raw, conf, l, i);
            reg_found = true;
            break;
        }
    }
    if !reg_found {
        unrecognized.push("registration_no");
    }

    // 名称
    let anchor_name = regex::Regex::new(r"名\s*称|公\s*司\s*名\s*称|Company\s*Name").unwrap();
    if let Some((idx, raw, conf)) = find_value_after_anchor(lines, &anchor_name, 1) {
        fields.company_name =
            FieldValue::from_line(raw.trim().to_string(), conf.min(0.93), &lines[idx], idx);
    } else {
        unrecognized.push("company_name");
    }

    // 法定代表人
    let anchor_legal = regex::Regex::new(r"法定代表人|法\s*人\s*代\s*表").unwrap();
    if let Some((idx, raw, conf)) = find_value_after_anchor(lines, &anchor_legal, 1) {
        fields.legal_rep =
            FieldValue::from_line(raw.trim().to_string(), conf.min(0.93), &lines[idx], idx);
    } else {
        unrecognized.push("legal_rep");
    }

    // 注册资本
    let anchor_cap = regex::Regex::new(r"注\s*册\s*资\s*本").unwrap();
    if let Some((idx, raw, conf)) = find_value_after_anchor(lines, &anchor_cap, 1) {
        fields.registered_capital =
            FieldValue::from_line(raw.trim().to_string(), conf.min(0.9), &lines[idx], idx);
    } else {
        unrecognized.push("registered_capital");
    }

    // 成立日期
    let anchor_est = regex::Regex::new(r"成\s*立\s*日\s*期|设\s*立\s*日\s*期").unwrap();
    if let Some((idx, raw, conf)) = find_value_after_anchor(lines, &anchor_est, 1) {
        if let Some(iso) = normalize::normalize_date(&raw) {
            fields.established_date =
                FieldValue::from_line(iso, conf.min(0.93), &lines[idx], idx);
        } else {
            fields.established_date =
                FieldValue::from_line(raw.clone(), conf * 0.5, &lines[idx], idx);
            warnings.push(format!("established_date raw '{raw}' failed parse"));
        }
    } else {
        unrecognized.push("established_date");
    }

    // 经营范围
    let anchor_scope = regex::Regex::new(r"经\s*营\s*范\s*围").unwrap();
    if let Some((idx, raw, conf)) = find_value_after_anchor(lines, &anchor_scope, 3) {
        fields.scope =
            FieldValue::from_line(raw.trim().to_string(), conf.min(0.88), &lines[idx], idx);
    } else {
        unrecognized.push("scope");
    }

    StructuredFields::BusinessLicenseV1 {
        fields,
        unrecognized_fields: unrecognized.iter().map(|s| s.to_string()).collect(),
        validation_warnings: warnings,
    }
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

    // ── id_card_cn ──

    #[test]
    fn id_card_cn_unknown_subtype_returns_none() {
        assert!(extract(&[], "totally_unknown").is_none());
    }

    #[test]
    fn id_card_cn_extracts_name_gender_nationality() {
        let lines = vec![
            rl("姓名 张三", 0),
            rl("性别 男 民族 汉", 40),
        ];
        let Some(StructuredFields::IdCardCnV1 { fields, .. }) = extract(&lines, "id_card_cn") else {
            unreachable!()
        };
        assert_eq!(fields.name.value.as_deref(), Some("张三"));
        assert_eq!(fields.gender.value.as_deref(), Some("男"));
    }

    #[test]
    fn id_card_cn_extracts_birth_date_iso() {
        let lines = vec![rl("出生 1990年01月01日", 0)];
        let Some(StructuredFields::IdCardCnV1 { fields, .. }) = extract(&lines, "id_card_cn") else {
            unreachable!()
        };
        assert_eq!(fields.birth_date.value.as_deref(), Some("1990-01-01"));
    }

    #[test]
    fn id_card_cn_id_number_with_gb_check_valid() {
        // 110101199001010015 is GB 11643 valid (per normalize::tests).
        let lines = vec![rl("公民身份号码 110101199001010015", 0)];
        let Some(StructuredFields::IdCardCnV1 { fields, validation_warnings, .. }) =
            extract(&lines, "id_card_cn") else { unreachable!() };
        assert_eq!(fields.id_number.value.as_deref(), Some("110101199001010015"));
        assert!(fields.id_number.confidence > 0.8);
        assert!(validation_warnings.is_empty());
    }

    #[test]
    fn id_card_cn_id_number_invalid_check_warns_and_downgrades() {
        let lines = vec![rl("公民身份号码 110101199001010019", 0)]; // 末位错
        let Some(StructuredFields::IdCardCnV1 { fields, validation_warnings, .. }) =
            extract(&lines, "id_card_cn") else { unreachable!() };
        assert_eq!(fields.id_number.value.as_deref(), Some("110101199001010019"));
        assert!(fields.id_number.confidence < 0.6);
        assert!(validation_warnings.iter().any(|w| w.contains("校验位")));
    }

    #[test]
    fn id_card_cn_id_fallback_scan_finds_18_digits() {
        // No anchor — should still find 18-digit ID via fallback scan.
        let lines = vec![rl("110101199001010015", 0)];
        let Some(StructuredFields::IdCardCnV1 { fields, .. }) = extract(&lines, "id_card_cn")
        else { unreachable!() };
        assert_eq!(fields.id_number.value.as_deref(), Some("110101199001010015"));
    }

    // ── bank_card ──

    #[test]
    fn bank_card_visa_test_number_luhn_valid() {
        let lines = vec![rl("4111 1111 1111 1111", 0)];
        let Some(StructuredFields::BankCardV1 { fields, validation_warnings, .. }) =
            extract(&lines, "bank_card") else { unreachable!() };
        assert!(fields.card_number.value.is_some());
        assert!(fields.card_number.value.as_ref().unwrap().contains("4111"));
        assert!(validation_warnings.is_empty(), "no Luhn warning for valid card");
    }

    #[test]
    fn bank_card_invalid_luhn_warns() {
        // 16 digits but bad checksum (last digit flipped)
        let lines = vec![rl("4111 1111 1111 1112", 0)];
        let Some(StructuredFields::BankCardV1 { fields, validation_warnings, .. }) =
            extract(&lines, "bank_card") else { unreachable!() };
        assert!(fields.card_number.confidence < 0.6);
        assert!(validation_warnings.iter().any(|w| w.contains("Luhn")));
    }

    #[test]
    fn bank_card_extracts_bank_name_and_type() {
        let lines = vec![
            rl("中国工商银行", 0),
            rl("4111 1111 1111 1111", 30),
            rl("借记卡", 60),
            rl("VALID THRU 12/28", 90),
        ];
        let Some(StructuredFields::BankCardV1 { fields, .. }) =
            extract(&lines, "bank_card") else { unreachable!() };
        assert_eq!(fields.bank_name.value.as_deref(), Some("中国工商银行"));
        assert_eq!(fields.card_type.value.as_deref(), Some("借记卡"));
        assert_eq!(fields.valid_thru.value.as_deref(), Some("12/28"));
    }

    // ── business_license ──

    #[test]
    fn business_license_extracts_company_legal_rep_dates() {
        let lines = vec![
            rl("名称: ABC 科技有限公司", 0),
            rl("法定代表人: 张三", 30),
            rl("注册资本: 100 万元人民币", 60),
            rl("成立日期: 2020年01月15日", 90),
            rl("经营范围: 技术开发、技术咨询", 120),
        ];
        let Some(StructuredFields::BusinessLicenseV1 { fields, .. }) =
            extract(&lines, "business_license") else { unreachable!() };
        assert!(fields.company_name.value.as_deref().unwrap().contains("ABC"));
        assert_eq!(fields.legal_rep.value.as_deref(), Some("张三"));
        assert!(fields.registered_capital.value.as_deref().unwrap().contains("100"));
        assert_eq!(fields.established_date.value.as_deref(), Some("2020-01-15"));
        assert!(fields.scope.value.as_deref().unwrap().contains("技术开发"));
    }

    #[test]
    fn business_license_registration_no_gb32100_invalid_warns() {
        // 18-char string, all digits (no letters), but checksum is bound to fail random one.
        let lines = vec![rl("911100000000000000", 0)];
        let Some(StructuredFields::BusinessLicenseV1 { fields, validation_warnings, .. }) =
            extract(&lines, "business_license") else { unreachable!() };
        // either found (with low conf + warning) or not found at all
        if fields.registration_no.value.is_some() {
            assert!(fields.registration_no.confidence < 0.6);
            assert!(validation_warnings.iter().any(|w| w.contains("校验")));
        }
    }

    // ── Fix #62: gender label/value confusion regression ────────────────

    /// OCR layout: "性别 男 民族 汉" on one line — anchor finds the whole line as
    /// value; 男 is present so direct extraction works.
    #[test]
    fn id_card_cn_gender_same_line_with_nationality() {
        let lines = vec![
            rl("姓名 李四", 0),
            rl("性别 男 民族 汉", 40),
        ];
        let Some(StructuredFields::IdCardCnV1 { fields, .. }) = extract(&lines, "id_card_cn")
        else { unreachable!() };
        assert_eq!(fields.gender.value.as_deref(), Some("男"), "should extract 男 from combined line");
    }

    /// Regression for Finding-B: OCR mis-lines "性别" label with "民族" value text.
    /// E.g. anchor returns "汉" (民族 value) which has neither 男 nor 女 — the fallback
    /// regex must scan raw lines and find "性别：女" elsewhere.
    #[test]
    fn id_card_cn_gender_fallback_when_anchor_grabs_wrong_value() {
        let lines = vec![
            rl("姓名 王五", 0),
            // anchor "性别" grabs next line "汉" (nationality mis-OCR'd adjacent)
            rl("性别", 40),
            rl("汉", 60),
            // but the full-text fallback regex matches this line
            rl("性别：女", 80),
        ];
        let Some(StructuredFields::IdCardCnV1 { fields, .. }) = extract(&lines, "id_card_cn")
        else { unreachable!() };
        assert_eq!(fields.gender.value.as_deref(), Some("女"), "fallback regex should recover 女");
    }
}
