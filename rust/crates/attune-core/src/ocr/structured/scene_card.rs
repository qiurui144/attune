//! `card_v1` — 名片字段抽取 (6 字段: name/company/job_title/phone/email/address).
//!
//! Spec §4.2: 启发式 + 关键词字典 + 正则. 高标杆场景 (Z 方案, 92% 红线).
//!
//! 策略 (按字段独立处理):
//!   - phone: 手机号 / 座机号正则 (中国为主, 国际格式次之)
//!   - email: 标准 email regex
//!   - job_title: 关键词字典 (CEO/CTO/总监/经理/...)
//!   - company: 公司后缀关键词 ("有限公司"/"股份"/"集团"/"科技"/"Ltd"/"Inc"/...)
//!   - address: 地址关键词 ("路"/"街"/"号"/"区"/"市"/"省"/"Road"/...)
//!   - name: 字号最大 + 卡片上半部 + 2-4 汉字 / 2-3 英文词 (启发式, 排除已识别字段所在行)

use super::{FieldValue, StructuredFields};
use crate::ocr::RawLine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CardFields {
    pub name: FieldValue,
    pub company: FieldValue,
    pub job_title: FieldValue,
    pub phone: FieldValue,
    pub email: FieldValue,
    pub address: FieldValue,
}

/// 主入口.
pub fn extract(lines: &[RawLine]) -> StructuredFields {
    let mut fields = CardFields::default();
    let mut unrecognized: Vec<&'static str> = Vec::new();
    let warnings: Vec<String> = Vec::new();

    if lines.is_empty() {
        return StructuredFields::CardV1 {
            fields,
            unrecognized_fields: vec![
                "name".into(),
                "company".into(),
                "job_title".into(),
                "phone".into(),
                "email".into(),
                "address".into(),
            ],
            validation_warnings: warnings,
        };
    }

    // 已识别字段所在行索引集 (用于排除 name 启发式时不挑这些行)
    let mut consumed: std::collections::HashSet<usize> = std::collections::HashSet::new();

    // ─── phone ──────────────────────────────────────────────────────────
    // 中国手机号: 1[3-9]\d{9}; 带分隔符的座机: \d{3,4}[-\s]?\d{7,8}
    let phone_re = regex::Regex::new(
        r"(?:\+?86[- ]?)?(1[3-9]\d{9})|(\d{3,4})[-\s]?(\d{7,8})",
    )
    .unwrap();
    for (i, l) in lines.iter().enumerate() {
        if let Some(cap) = phone_re.captures(&l.text) {
            // 取最长的 capture group
            let raw = cap.iter()
                .skip(1)
                .flatten()
                .map(|m| m.as_str().to_string())
                .max_by_key(|s| s.len())
                .unwrap_or_default();
            let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.len() >= 7 {
                fields.phone = FieldValue::from_line(digits, l.confidence.min(0.93), l, i);
                consumed.insert(i);
                break;
            }
        }
    }
    if fields.phone.value.is_none() {
        unrecognized.push("phone");
    }

    // ─── email ──────────────────────────────────────────────────────────
    let email_re = regex::Regex::new(
        r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}",
    )
    .unwrap();
    for (i, l) in lines.iter().enumerate() {
        if let Some(m) = email_re.find(&l.text) {
            fields.email =
                FieldValue::from_line(m.as_str().to_string(), l.confidence.min(0.95), l, i);
            consumed.insert(i);
            break;
        }
    }
    if fields.email.value.is_none() {
        unrecognized.push("email");
    }

    // ─── job_title ──────────────────────────────────────────────────────
    let title_keywords = [
        "CEO", "CTO", "CFO", "COO", "CMO", "CIO", "VP", "President",
        "Director", "Manager", "Engineer", "Architect", "Lead", "Head",
        "总裁", "总经理", "总监", "经理", "主任", "主管", "组长",
        "工程师", "架构师", "顾问", "副总", "副总裁", "副总经理", "董事长",
    ];
    for (i, l) in lines.iter().enumerate() {
        if consumed.contains(&i) {
            continue;
        }
        let t = &l.text;
        if title_keywords.iter().any(|k| t.contains(k)) {
            // 通常 title 是独立短行 (≤ 20 字符) — 长行很可能是混到地址/公司
            if t.chars().count() <= 30 {
                fields.job_title =
                    FieldValue::from_line(t.trim().to_string(), l.confidence.min(0.88), l, i);
                consumed.insert(i);
                break;
            }
        }
    }
    if fields.job_title.value.is_none() {
        unrecognized.push("job_title");
    }

    // ─── company ────────────────────────────────────────────────────────
    let company_keywords = [
        "有限公司", "股份有限公司", "集团", "Ltd", "Ltd.", "Inc", "Inc.",
        "Corp", "Corporation", "LLC", "GmbH", "Co.", "Company",
        "科技", "信息", "咨询", "网络", "贸易", "实业",
    ];
    let mut best_company: Option<(usize, f32)> = None;
    for (i, l) in lines.iter().enumerate() {
        if consumed.contains(&i) {
            continue;
        }
        let t = &l.text;
        if company_keywords.iter().any(|k| t.contains(k)) {
            // 偏好更具体的: 长度合理 (≤ 40 字符) + 含关键词在末尾
            let score = if t.chars().count() <= 40 { 1.0 } else { 0.5 };
            match best_company {
                None => best_company = Some((i, score)),
                Some((_, s)) if score > s => best_company = Some((i, score)),
                _ => {}
            }
        }
    }
    if let Some((i, _)) = best_company {
        let l = &lines[i];
        fields.company =
            FieldValue::from_line(l.text.trim().to_string(), l.confidence.min(0.9), l, i);
        consumed.insert(i);
    } else {
        unrecognized.push("company");
    }

    // ─── address ────────────────────────────────────────────────────────
    let address_keywords = [
        "路", "街", "号", "室", "楼", "栋", "区", "市", "省", "县", "镇", "村",
        "Road", "Street", "Floor", "Suite", "Avenue", "Lane",
    ];
    for (i, l) in lines.iter().enumerate() {
        if consumed.contains(&i) {
            continue;
        }
        let t = &l.text;
        let hits = address_keywords.iter().filter(|k| t.contains(*k)).count();
        // 至少 2 个地址 token 才算 (避免 single "号" / "路" 误匹配公司名)
        if hits >= 2 && t.chars().count() >= 6 {
            fields.address =
                FieldValue::from_line(t.trim().to_string(), l.confidence.min(0.85), l, i);
            consumed.insert(i);
            break;
        }
    }
    if fields.address.value.is_none() {
        unrecognized.push("address");
    }

    // ─── name ───────────────────────────────────────────────────────────
    // 启发式: 取未被 consumed 的行中 — 字号 (bbox.h) 最大 + 内容短 (2-15 字符) + 在卡片上半部.
    let mut max_y = 0u32;
    for l in lines {
        max_y = max_y.max(l.bbox.y + l.bbox.h);
    }
    let half_y = max_y / 2;
    let mut best_name: Option<(usize, u32, f32)> = None;
    for (i, l) in lines.iter().enumerate() {
        if consumed.contains(&i) {
            continue;
        }
        let len = l.text.trim().chars().count();
        if !(2..=15).contains(&len) {
            continue;
        }
        // 跳过含明显非名字字符 (数字 / 邮件 / 公司词 / 长地址)
        if l.text.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        if company_keywords.iter().any(|k| l.text.contains(k)) {
            continue;
        }
        // 偏好上半部 (字号大者 + y 小者)
        let score_h = l.bbox.h;
        let y_bonus = if l.bbox.y < half_y { 10 } else { 0 };
        let total = score_h + y_bonus;
        match best_name {
            None => best_name = Some((i, total, l.confidence)),
            Some((_, s, _)) if total > s => best_name = Some((i, total, l.confidence)),
            _ => {}
        }
    }
    if let Some((i, _, _)) = best_name {
        let l = &lines[i];
        fields.name =
            FieldValue::from_line(l.text.trim().to_string(), l.confidence.min(0.82), l, i);
    } else {
        unrecognized.push("name");
    }

    StructuredFields::CardV1 {
        fields,
        unrecognized_fields: unrecognized.iter().map(|s| s.to_string()).collect(),
        validation_warnings: warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::BBox;

    fn rl(text: &str, x: u32, y: u32, h: u32) -> RawLine {
        RawLine {
            text: text.into(),
            bbox: BBox { x, y, w: 200, h },
            confidence: 0.95,
        }
    }

    #[test]
    fn empty_returns_all_unrecognized() {
        let StructuredFields::CardV1 { unrecognized_fields, .. } = extract(&[]) else {
            unreachable!()
        };
        assert_eq!(unrecognized_fields.len(), 6);
    }

    #[test]
    fn extracts_phone_mobile() {
        let lines = vec![rl("电话: 13800138000", 0, 0, 30)];
        let StructuredFields::CardV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert_eq!(fields.phone.value.as_deref(), Some("13800138000"));
    }

    #[test]
    fn extracts_phone_with_intl_prefix() {
        let lines = vec![rl("Tel: +86 13912345678", 0, 0, 30)];
        let StructuredFields::CardV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert_eq!(fields.phone.value.as_deref(), Some("13912345678"));
    }

    #[test]
    fn extracts_email() {
        let lines = vec![rl("alice@example.com", 0, 0, 30)];
        let StructuredFields::CardV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert_eq!(fields.email.value.as_deref(), Some("alice@example.com"));
    }

    #[test]
    fn extracts_job_title_chinese() {
        let lines = vec![
            rl("张三", 50, 10, 40),
            rl("技术总监", 50, 50, 25),
        ];
        let StructuredFields::CardV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert_eq!(fields.job_title.value.as_deref(), Some("技术总监"));
    }

    #[test]
    fn extracts_job_title_english() {
        let lines = vec![
            rl("Alice Wong", 50, 10, 40),
            rl("Senior Engineer", 50, 50, 25),
        ];
        let StructuredFields::CardV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert_eq!(fields.job_title.value.as_deref(), Some("Senior Engineer"));
    }

    #[test]
    fn extracts_company() {
        let lines = vec![rl("ABC 科技有限公司", 0, 0, 30)];
        let StructuredFields::CardV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert_eq!(fields.company.value.as_deref(), Some("ABC 科技有限公司"));
    }

    #[test]
    fn extracts_address_requires_two_tokens() {
        let lines = vec![rl("北京市朝阳区建国路88号 SOHO 1座 3楼", 0, 0, 25)];
        let StructuredFields::CardV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert!(fields.address.value.is_some());
        assert!(fields.address.value.as_ref().unwrap().contains("北京"));
    }

    #[test]
    fn address_rejects_single_token() {
        // 只含一个 "号" 不算地址 (可能是 "13 号文件")
        let lines = vec![rl("13 号文件", 0, 0, 25)];
        let StructuredFields::CardV1 { fields, unrecognized_fields, .. } = extract(&lines) else {
            unreachable!()
        };
        assert!(fields.address.value.is_none());
        assert!(unrecognized_fields.contains(&"address".to_string()));
    }

    #[test]
    fn extracts_name_largest_font_top_half() {
        let lines = vec![
            // name 在顶部, 字号大
            rl("张三", 50, 10, 50),
            rl("销售总监", 50, 70, 25),
            rl("ABC 有限公司", 50, 100, 25),
        ];
        let StructuredFields::CardV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert_eq!(fields.name.value.as_deref(), Some("张三"));
    }

    #[test]
    fn name_excludes_lines_containing_digits() {
        let lines = vec![
            rl("13800138000", 0, 5, 50), // 大字号但全数字, 应被 phone 吃掉
            rl("张三", 0, 60, 30),
        ];
        let StructuredFields::CardV1 { fields, .. } = extract(&lines) else { unreachable!() };
        assert_eq!(fields.phone.value.as_deref(), Some("13800138000"));
        assert_eq!(fields.name.value.as_deref(), Some("张三"));
    }

    #[test]
    fn full_business_card_end_to_end() {
        let lines = vec![
            rl("Alice Wong", 50, 10, 50),
            rl("Senior Architect", 50, 70, 25),
            rl("ABC Tech Inc.", 50, 100, 25),
            rl("alice@abc.com", 50, 130, 22),
            rl("+86 13912345678", 50, 160, 22),
            rl("100 Main Road, Suite 200, Beijing", 50, 190, 22),
        ];
        let StructuredFields::CardV1 { fields, unrecognized_fields, .. } = extract(&lines) else {
            unreachable!()
        };
        assert_eq!(fields.name.value.as_deref(), Some("Alice Wong"));
        assert_eq!(fields.job_title.value.as_deref(), Some("Senior Architect"));
        assert!(fields.company.value.as_deref().unwrap().contains("ABC Tech Inc"));
        assert_eq!(fields.email.value.as_deref(), Some("alice@abc.com"));
        assert_eq!(fields.phone.value.as_deref(), Some("13912345678"));
        assert!(fields.address.value.is_some());
        assert!(unrecognized_fields.is_empty(),
            "all 6 fields should be recognized; got unrecognized={unrecognized_fields:?}");
    }
}
