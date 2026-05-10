//! 实体抽取 — 文本中识别 [人名 / 日期 / 金额 / 地点 / 组织] 等结构化字段.
//!
//! 纯规则版 (无 LLM, 可缓存). LLM 增强由调用方按需追加.
//!
//! 抽取范围:
//! - persons: 中文人名 (2-4 字, 百家姓启发式)
//! - dates: 复用 parse_chinese_date
//! - amounts: 阿拉伯数字金额 (含千分位 / 元万亿单位) + 中文大写数字 (壹贰叁...)
//! - locations: 省/市/区/县 + 街道/小区
//! - organizations: 公司 / 律师事务所 / 法院 / 银行 等
//!
//! 不抽:
//! - 邮箱 / 电话 / 身份证 (走单独 PII redactor 模块)
//! - 模糊指代 ("某某" / "甲方" / "出租人")

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Entities {
    pub persons: Vec<String>,
    pub dates: Vec<String>,
    pub amounts: Vec<Amount>,
    pub locations: Vec<String>,
    pub organizations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Amount {
    /// 标准化为元 (1 万 = 10000.0, 1 角 = 0.1)
    pub value: f64,
    /// 原始字符串
    pub raw: String,
}

/// 主入口
pub fn extract(text: &str) -> Entities {
    Entities {
        persons: extract_persons(text),
        dates: extract_dates(text),
        amounts: extract_amounts(text),
        locations: extract_locations(text),
        organizations: extract_organizations(text),
    }
}

// ── 人名 ──────────────────────────────────────────────

const COMMON_SURNAMES: &[&str] = &[
    "王", "李", "张", "刘", "陈", "杨", "黄", "赵", "吴", "周",
    "徐", "孙", "马", "朱", "胡", "郭", "何", "高", "林", "罗",
    "郑", "梁", "谢", "宋", "唐", "许", "韩", "冯", "邓", "曹",
    "彭", "曾", "肖", "田", "董", "袁", "潘", "于", "蒋", "蔡",
    "余", "杜", "叶", "程", "苏", "魏", "吕", "丁", "任", "沈",
    "姚", "卢", "姜", "崔", "钟", "谭", "陆", "汪", "范", "金",
    "石", "廖", "贾", "夏", "韦", "付", "方", "白", "邹", "孟",
    "熊", "秦", "邱", "江", "尹", "薛", "闫", "段", "雷", "侯",
    "龙", "史", "陶", "黎", "贺", "顾", "毛", "郝", "龚", "邵",
    "万", "覃", "武", "钱", "戴", "严", "莫", "孔", "向", "汤",
];

fn extract_persons(text: &str) -> Vec<String> {
    let surnames: std::collections::HashSet<&str> = COMMON_SURNAMES.iter().copied().collect();
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // stop chars: 出现在 given-name 末尾时退化到 1 字名
    // (常见动词 / 介词 / 助词 / 连词 — 不会作为人名收尾)
    const STOP_CHARS: &[char] = &[
        '同', '在', '是', '到', '与', '和', '已', '不', '了', '为',
        '的', '上', '说', '又', '却', '把', '将', '从', '向', '让',
        '签', '订', '约', '后', '前', '中', '里', '内', '外',
        '于', '由', '被', '对', '比', '给', '使', '叫', '令', '请',
        '来', '去', '入', '出', '回', '至', '及', '或', '而', '但',
        '即', '便', '才', '就', '该', '此', '这', '那', '其', '某',
        '会', '能', '可', '应', '需', '要', '想', '觉', '感', '认',
    ];

    let mut i = 0;
    while i < chars.len() {
        let surname_str = chars[i].to_string();
        if !surnames.contains(surname_str.as_str()) {
            i += 1;
            continue;
        }
        // 默认取 2 字名 (3 chars total: surname + 2 given).
        // 若 given 第 2 字是 STOP_CHAR 或非中文 → 退化为 1 字名.
        let mut chosen_len: Option<usize> = None;
        if i + 2 < chars.len()
            && is_cjk_char(&chars[i + 1])
            && is_cjk_char(&chars[i + 2])
            && !STOP_CHARS.contains(&chars[i + 2])
            && !surnames.contains(chars[i + 2].to_string().as_str())
        {
            chosen_len = Some(2);
        } else if i + 1 < chars.len() && is_cjk_char(&chars[i + 1]) {
            chosen_len = Some(1);
        }

        if let Some(given_len) = chosen_len {
            let candidate: String = chars[i..i + 1 + given_len].iter().collect();
            if !NON_PERSON_TOKENS.iter().any(|kw| candidate.contains(kw)) {
                if seen.insert(candidate.clone()) {
                    out.push(candidate);
                }
                i += 1 + given_len;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn is_cjk_char(c: &char) -> bool {
    matches!(*c, '\u{4e00}'..='\u{9fff}')
}

const NON_PERSON_TOKENS: &[&str] = &[
    "公司", "银行", "法院", "律所", "事务所", "学院", "大学", "中学", "小学",
    "省", "市", "区", "县", "街", "路", "号",
    "条", "款", "项", "章", "节",
    "元", "万", "亿", "角", "分",
];

// ── 日期 ──────────────────────────────────────────────

fn extract_dates(text: &str) -> Vec<String> {
    super::parse_chinese_date::parse_all(text)
        .into_iter()
        .map(|d| d.iso)
        .collect()
}

// ── 金额 ──────────────────────────────────────────────

fn extract_amounts(text: &str) -> Vec<Amount> {
    let mut out = Vec::new();
    out.extend(extract_arabic_amounts(text));
    out.extend(extract_chinese_capital_amounts(text));
    // 去重 (同一字符串重复抽)
    let mut seen = std::collections::HashSet::new();
    out.into_iter()
        .filter(|a| seen.insert(a.raw.clone()))
        .collect()
}

fn extract_arabic_amounts(text: &str) -> Vec<Amount> {
    use regex::Regex;
    // 支持 "1,000.00" / "10000" / "1.5万" / "2亿" / "500元" / "5角" / "3分"
    // 千分位分支必须含至少 1 个 `,\d{3}` 组, 否则走 plain `\d+` 匹配整段数字.
    let re = Regex::new(
        r"(\d{1,3}(?:,\d{3})+(?:\.\d+)?|\d+(?:\.\d+)?)\s*(亿|万|千|百|元|角|分)?"
    ).unwrap();
    let mut out = Vec::new();
    for cap in re.captures_iter(text) {
        let num_str = cap[1].replace(',', "");
        let unit = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let Ok(num) = num_str.parse::<f64>() else { continue };
        let multiplier = match unit {
            "亿" => 100_000_000.0,
            "万" => 10_000.0,
            "千" => 1_000.0,
            "百" => 100.0,
            "元" => 1.0,
            "角" => 0.1,
            "分" => 0.01,
            "" => continue, // 无单位的纯数字不抽 (避免抽到日期/编号/页码)
            _ => 1.0,
        };
        let value = num * multiplier;
        if value < 0.001 || value > 1e15 {
            continue;
        }
        let raw = cap.get(0).unwrap().as_str().trim().to_string();
        out.push(Amount { value, raw });
    }
    out
}

fn extract_chinese_capital_amounts(text: &str) -> Vec<Amount> {
    use regex::Regex;
    // 中文大写金额: 壹贰叁肆伍陆柒捌玖拾佰仟万亿元角分整
    let re = Regex::new(
        r"[壹贰叁肆伍陆柒捌玖拾佰仟万亿元角分整]{2,}"
    ).unwrap();
    let mut out = Vec::new();
    for m in re.find_iter(text) {
        let raw = m.as_str().to_string();
        if let Some(value) = parse_chinese_capital(&raw) {
            out.push(Amount { value, raw });
        }
    }
    out
}

fn parse_chinese_capital(s: &str) -> Option<f64> {
    let digit = |c: char| -> Option<u64> {
        match c {
            '零' | '〇' => Some(0),
            '壹' => Some(1),
            '贰' => Some(2),
            '叁' => Some(3),
            '肆' => Some(4),
            '伍' => Some(5),
            '陆' => Some(6),
            '柒' => Some(7),
            '捌' => Some(8),
            '玖' => Some(9),
            _ => None,
        }
    };
    let mut total: f64 = 0.0;
    let mut section: u64 = 0;
    let mut current: u64 = 0;
    let mut found_unit = false;

    for c in s.chars() {
        if let Some(d) = digit(c) {
            current = d;
            continue;
        }
        match c {
            '拾' => {
                section += if current == 0 { 10 } else { current * 10 };
                current = 0;
                found_unit = true;
            }
            '佰' => {
                section += current * 100;
                current = 0;
                found_unit = true;
            }
            '仟' => {
                section += current * 1000;
                current = 0;
                found_unit = true;
            }
            '万' => {
                section = (section + current) * 10_000;
                total += section as f64;
                section = 0;
                current = 0;
                found_unit = true;
            }
            '亿' => {
                section = (section + current) * 100_000_000;
                total += section as f64;
                section = 0;
                current = 0;
                found_unit = true;
            }
            '元' => {
                total += (section + current) as f64;
                section = 0;
                current = 0;
                found_unit = true;
            }
            '角' => {
                total += current as f64 * 0.1;
                current = 0;
                found_unit = true;
            }
            '分' => {
                total += current as f64 * 0.01;
                current = 0;
                found_unit = true;
            }
            '整' | '正' => {}
            _ => return None,
        }
    }
    if !found_unit {
        return None;
    }
    // 残留 (没遇到 元/角/分 等终结符)
    if current > 0 || section > 0 {
        total += (section + current) as f64;
    }
    Some(total)
}

// ── 地点 ──────────────────────────────────────────────

fn extract_locations(text: &str) -> Vec<String> {
    use regex::Regex;
    let re = Regex::new(
        r"[一-鿿]{2,8}(?:省|自治区|特别行政区|市|区|县|街道|乡|镇|村|路|街|号|大厦|小区|大道|巷)"
    ).unwrap();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for m in re.find_iter(text) {
        let s = m.as_str();
        if seen.insert(s.to_string()) {
            out.push(s.to_string());
        }
    }
    out
}

// ── 组织 ──────────────────────────────────────────────

fn extract_organizations(text: &str) -> Vec<String> {
    use regex::Regex;
    let re = Regex::new(
        r"[一-鿿]{2,12}(?:有限公司|股份公司|公司|集团|律师事务所|事务所|法院|检察院|银行|学校|大学|学院|医院|协会|商会|工作室)"
    ).unwrap();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for m in re.find_iter(text) {
        let s = m.as_str();
        if seen.insert(s.to_string()) {
            out.push(s.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_common_persons() {
        let e = extract("张三和李四签订了合同, 王小明是见证人");
        assert!(e.persons.contains(&"张三".to_string()));
        assert!(e.persons.contains(&"李四".to_string()));
        assert!(e.persons.contains(&"王小明".to_string()));
    }

    #[test]
    fn excludes_non_person_with_org_suffix() {
        let e = extract("张三同志在张三公司任职");
        // "张三公司" 有 "公司" 后缀, 但 "张三" 应被识别 (organization 单独抽)
        assert!(e.persons.contains(&"张三".to_string()));
        assert!(e.organizations.iter().any(|o| o.contains("张三公司")));
    }

    #[test]
    fn extracts_arabic_amounts_with_unit() {
        let e = extract("借款 500000 元, 利息 ¥1,200 元, 罚金 1.5万");
        assert!(e.amounts.iter().any(|a| (a.value - 500_000.0).abs() < 0.01));
        assert!(e.amounts.iter().any(|a| (a.value - 1_200.0).abs() < 0.01));
        assert!(e.amounts.iter().any(|a| (a.value - 15_000.0).abs() < 0.01));
    }

    #[test]
    fn skips_unitless_numbers() {
        let e = extract("第 12345 号文件");
        // "12345" 无 "元/万/亿" 等单位 → 不抽 (避免误识页码/编号)
        assert!(!e.amounts.iter().any(|a| (a.value - 12345.0).abs() < 0.01));
    }

    #[test]
    fn parses_chinese_capital_amount() {
        let v = parse_chinese_capital("伍拾万元整").expect("parse");
        assert!((v - 500_000.0).abs() < 0.01);
        let v = parse_chinese_capital("壹佰贰拾叁万肆仟伍佰陆拾柒元捌角玖分").expect("parse");
        assert!((v - 1_234_567.89).abs() < 0.01);
    }

    #[test]
    fn extract_chinese_capital_in_text() {
        let e = extract("借条第 1 条: 借款人民币伍拾万元整");
        assert!(e.amounts.iter().any(|a| (a.value - 500_000.0).abs() < 0.01));
    }

    #[test]
    fn extract_dates_via_parse_chinese_date() {
        let e = extract("签订日期 2023年1月15日, 履行日 2023/3/20");
        assert!(e.dates.contains(&"2023-01-15".to_string()));
        assert!(e.dates.contains(&"2023-03-20".to_string()));
    }

    #[test]
    fn extract_locations_basic() {
        let e = extract("住所地: 北京市海淀区中关村大街 5 号 1 单元");
        assert!(e.locations.iter().any(|l| l.contains("北京市")));
        assert!(e.locations.iter().any(|l| l.contains("海淀区")));
    }

    #[test]
    fn extract_orgs_basic() {
        let e = extract("案件由北京市第一中级人民法院审理, 阿里巴巴集团为被告, 张伟律师事务所代理");
        assert!(e.organizations.iter().any(|o| o.contains("法院")));
        assert!(e.organizations.iter().any(|o| o.contains("阿里巴巴集团") || o.contains("集团")));
        assert!(e.organizations.iter().any(|o| o.contains("律师事务所")));
    }

    #[test]
    fn empty_input_returns_empty() {
        let e = extract("");
        assert!(e.persons.is_empty());
        assert!(e.dates.is_empty());
        assert!(e.amounts.is_empty());
        assert!(e.locations.is_empty());
        assert!(e.organizations.is_empty());
    }

    #[test]
    fn integration_full_document_snippet() {
        let text = "原告张三 (住所地北京市海淀区) 与被告李四 (住所地上海市浦东新区) \
            因借款合同纠纷一案. 2023年1月15日 张三向李四出借人民币伍拾万元整 (¥500,000), \
            约定月利率1%. 案件由北京市第一中级人民法院受理.";
        let e = extract(text);
        assert!(e.persons.contains(&"张三".to_string()));
        assert!(e.persons.contains(&"李四".to_string()));
        assert!(e.dates.contains(&"2023-01-15".to_string()));
        assert!(e.amounts.iter().any(|a| (a.value - 500_000.0).abs() < 0.01));
        assert!(e.locations.iter().any(|l| l.contains("海淀区")));
        assert!(e.organizations.iter().any(|o| o.contains("法院")));
    }
}
