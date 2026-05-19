//! 实体抽取：从中文 / 英文文本中抽出 Person / Money / Date / Organization 等结构化实体。
//!
//! Sprint 1 Phase B 用这些实体计算 Project 推荐归类的"实体重叠度"
//! （spec §2.3 的 0.6 阈值）。
//!
//! 设计：纯函数 + 正则 + 中文启发式。无外部 API、无模型推理。
//!
//! ## 范围说明（v0.6 OSS 边界瘦身后）
//!
//! 本模块只提供**通用领域**实体类型（Person / Money / Date / Organization）。
//! 行业专属实体（如律师案号 CaseNo / 病历号 / 商标号）由各 vertical plugin
//! 实现自己的 extractor，注册到 attune-core 的 plugin loader。详见
//! attune-pro 仓 `INTEGRATION.md` §13 委托清单。

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Person,       // 中文 2-4 字人名
    Money,        // ¥xxx / 人民币 X 元 / 数额单位
    Date,         // YYYY-MM-DD / YYYY 年 M 月 D 日
    Organization, // 含"有限公司"/"研究所"/"事务所"/"学校"等通用机构后缀
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub kind: EntityKind,
    pub value: String,
    /// 在原文中的字节起止（UTF-8 byte offset）— 上层可截取上下文
    pub byte_start: usize,
    pub byte_end: usize,
}

/// 从给定文本抽出所有实体。返回顺序：按出现位置升序。
///
/// 对中文 + 英文混合文本鲁棒。空文本返回空 Vec。
pub fn extract_entities(text: &str) -> Vec<Entity> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();

    extract_money(text, &mut out);
    extract_dates(text, &mut out);
    extract_organization(text, &mut out);
    extract_chinese_person(text, &mut out);

    out.sort_by_key(|e| e.byte_start);
    out
}

fn push(out: &mut Vec<Entity>, kind: EntityKind, m: regex::Match<'_>) {
    out.push(Entity {
        kind,
        value: m.as_str().to_string(),
        byte_start: m.start(),
        byte_end: m.end(),
    });
}

fn extract_money(text: &str, out: &mut Vec<Entity>) {
    // ¥xxx / ¥xxx.xx / xxx 元 / 人民币 xxx 元 / 壹拾万元 等
    static MONEY_RE_PAT: &str = r"(?x)
        ( ¥ \s* \d[\d,]*(?:\.\d+)?
        | (?:人民币|RMB|CNY) \s* \d[\d,]*(?:\.\d+)? \s*(?:元|圆|万|亿)?
        | \d[\d,]*(?:\.\d+)? \s* (?:元|圆|万元|亿元|万|亿)
        | (?:壹|贰|叁|肆|伍|陆|柒|捌|玖|拾|佰|仟|万|亿|零|整|圆|元|角|分){2,}
        )
    ";
    let re = Regex::new(MONEY_RE_PAT).expect("money regex compile");
    for m in re.find_iter(text) {
        push(out, EntityKind::Money, m);
    }
}

fn extract_dates(text: &str, out: &mut Vec<Entity>) {
    // 2024-03-15 / 2024/3/15 / 2024 年 3 月 15 日
    // 顺序：先长 pattern（年月日），再短 pattern（年月 / ISO），后面被前者覆盖的丢弃。
    static DATE_PATS: &[&str] = &[
        r"\d{4}\s*年\s*\d{1,2}\s*月\s*\d{1,2}\s*日",
        r"\b\d{4}[-/]\d{1,2}[-/]\d{1,2}\b",
        r"\d{4}\s*年\s*\d{1,2}\s*月",
    ];
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    for pat in DATE_PATS {
        let re = Regex::new(pat).expect("date regex compile");
        for m in re.find_iter(text) {
            // 若新匹配区间被已有匹配覆盖（subset），跳过；否则收录。
            let covered = spans
                .iter()
                .any(|(s, e, _)| *s <= m.start() && m.end() <= *e);
            if covered {
                continue;
            }
            spans.push((m.start(), m.end(), m.as_str().to_string()));
        }
    }
    for (s, e, v) in spans {
        out.push(Entity {
            kind: EntityKind::Date,
            value: v,
            byte_start: s,
            byte_end: e,
        });
    }
}

fn extract_organization(text: &str, out: &mut Vec<Entity>) {
    // 通用机构后缀：有限公司 / 股份有限公司 / 有限责任公司 / 研究所 / 事务所 / 学校 / 大学
    // 这些是跨行业通用的（律师 / 医生 / 学者 / 售前 都需要识别"机构"）。
    static ORG_PAT: &str = r"[一-鿿（）()]{2,15}(?:有限公司|股份有限公司|有限责任公司|研究所|事务所|律师事务所|科技公司|分公司|大学|学院|医院|银行)";
    let re = Regex::new(ORG_PAT).expect("organization regex compile");
    for m in re.find_iter(text) {
        push(out, EntityKind::Organization, m);
    }
}

/// 中文人名启发式：百家姓单字姓 + 1-3 字名（拒绝公司/案号片段）
fn extract_chinese_person(text: &str, out: &mut Vec<Entity>) {
    let common_surnames =
        "李王张刘陈杨赵黄周吴徐孙朱马胡郭林何高梁郑罗宋谢唐韩曹许邓萧冯曾程蔡彭潘袁于董余苏叶吕魏蒋田杜丁沈姜范江";
    let surnames_chars: std::collections::HashSet<char> = common_surnames.chars().collect();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len();
    let mut consumed = vec![false; n];

    for i in 0..n {
        if consumed[i] {
            continue;
        }
        let (byte_start, c0) = chars[i];
        if !surnames_chars.contains(&c0) {
            continue;
        }
        let mut name_end_idx = i;
        for j in 1..=3 {
            if i + j >= n {
                break;
            }
            let (_, cj) = chars[i + j];
            if is_chinese_name_char(cj) {
                name_end_idx = i + j;
            } else {
                break;
            }
        }
        if name_end_idx == i {
            continue; // 单字姓没有名 — 跳过
        }
        let byte_end = chars[name_end_idx].0 + chars[name_end_idx].1.len_utf8();
        let value: String = chars[i..=name_end_idx].iter().map(|(_, c)| *c).collect();
        // 拒绝结尾在公司/职务字
        if value.ends_with('总') || value.ends_with('司') || value.ends_with('厂') {
            continue;
        }
        out.push(Entity {
            kind: EntityKind::Person,
            value,
            byte_start,
            byte_end,
        });
        for flag in consumed.iter_mut().take(name_end_idx + 1).skip(i) {
            *flag = true;
        }
    }
}

fn is_chinese_name_char(c: char) -> bool {
    let code = c as u32;
    (0x4E00..=0x9FFF).contains(&code) || (0x3400..=0x4DBF).contains(&code)
}

/// 计算两组实体的 Jaccard 相似度：|A ∩ B| / |A ∪ B|。
///
/// 用 (kind, value) 二元组作为去重 key — 同字面值不同 kind 视为不同实体。
/// 空输入返回 0.0。Sprint 1 Phase B 用 0.6 阈值判断"是否推荐归类"（spec §2.3）。
pub fn entity_overlap_score(a: &[Entity], b: &[Entity]) -> f32 {
    use std::collections::HashSet;

    if a.is_empty() && b.is_empty() {
        return 0.0;
    }

    let set_a: HashSet<(EntityKind, &str)> =
        a.iter().map(|e| (e.kind, e.value.as_str())).collect();
    let set_b: HashSet<(EntityKind, &str)> =
        b.iter().map(|e| (e.kind, e.value.as_str())).collect();

    let inter = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();

    if union == 0 {
        0.0
    } else {
        inter as f32 / union as f32
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn money_basic() {
        let v = extract_entities("¥1,000");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].kind, EntityKind::Money);
    }

    #[test]
    fn date_basic() {
        let v = extract_entities("2024-03-15");
        assert_eq!(v[0].kind, EntityKind::Date);
        assert_eq!(v[0].value, "2024-03-15");
    }

    #[test]
    fn organization_basic() {
        let v = extract_entities("某某科技有限公司发布新产品");
        assert!(v.iter().any(|e| e.kind == EntityKind::Organization));
    }

    #[test]
    fn ordering_by_position() {
        let text = "2024-03-15 张三借款 ¥10,000";
        let v = extract_entities(text);
        let kinds: Vec<EntityKind> = v.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, vec![EntityKind::Date, EntityKind::Person, EntityKind::Money]);
    }

    #[test]
    fn overlap_score_identical() {
        let a = extract_entities("张三借款 ¥10000，2024-03-15 到期");
        let b = extract_entities("张三借款 ¥10000，2024-03-15 到期");
        let s = entity_overlap_score(&a, &b);
        assert!((s - 1.0).abs() < 1e-6, "完全相同应 1.0，got {s}");
    }

    #[test]
    fn overlap_score_disjoint() {
        let a = extract_entities("张三借款 ¥10000");
        let b = extract_entities("李四签约 ¥50000");
        let s = entity_overlap_score(&a, &b);
        assert!(s < 0.01, "无重叠应 ~0，got {s}");
    }

    #[test]
    fn overlap_score_partial() {
        // 用通用实体（人名 + 金额 + 公司）测试 Jaccard 相似度
        // 共享：Person 张三 + Organization 某科技有限公司；不共享：¥10000 / ¥20000
        // intersect = 2, union = 4 → 0.5
        let a = extract_entities("张三 借款 ¥10000，签约方某科技有限公司");
        let b = extract_entities("张三 还款 ¥20000，签约方某科技有限公司");
        let s = entity_overlap_score(&a, &b);
        assert!((s - 0.5).abs() < 0.05, "应 ~0.5（Jaccard），got {s}");
    }

    #[test]
    fn overlap_score_empty_inputs() {
        assert_eq!(entity_overlap_score(&[], &[]), 0.0);
        let a = extract_entities("张三");
        assert_eq!(entity_overlap_score(&a, &[]), 0.0);
        assert_eq!(entity_overlap_score(&[], &a), 0.0);
    }
}
