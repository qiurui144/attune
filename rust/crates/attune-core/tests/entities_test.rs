//! 实体抽取端到端测试 — 固定语料（CLAUDE.md "零随机数据"原则）。

use attune_core::entities::{extract_entities, Entity, EntityKind};

#[test]
fn extract_money_simple() {
    let text = "借款金额为人民币壹拾万元整（¥100,000.00）";
    let ents = extract_entities(text);
    let monies: Vec<&Entity> = ents
        .iter()
        .filter(|e| e.kind == EntityKind::Money)
        .collect();
    assert!(!monies.is_empty(), "should detect money");
    // 至少一个金额匹配 ¥100,000.00 或 壹拾万
    assert!(monies
        .iter()
        .any(|e| e.value.contains("100,000") || e.value.contains("壹拾万")));
}

#[test]
fn extract_chinese_dates() {
    let text = "本合同于 2024 年 3 月 15 日签订，至 2026-01-31 到期。";
    let ents = extract_entities(text);
    let dates: Vec<&Entity> = ents.iter().filter(|e| e.kind == EntityKind::Date).collect();
    assert_eq!(dates.len(), 2, "应抽两个日期");
    assert!(dates
        .iter()
        .any(|e| e.value.contains("2024") && e.value.contains("3") && e.value.contains("15")));
    assert!(dates.iter().any(|e| e.value.contains("2026-01-31")));
}

#[test]
fn extract_organization_suffix() {
    let text = "甲方：北京云麓科技有限公司，乙方：上海某某有限责任公司，研究方：清华大学。";
    let ents = extract_entities(text);
    let orgs: Vec<&Entity> = ents
        .iter()
        .filter(|e| e.kind == EntityKind::Organization)
        .collect();
    assert!(orgs.len() >= 2, "至少应抽两个机构");
    assert!(orgs
        .iter()
        .any(|e| e.value.contains("北京云麓科技有限公司")));
}

#[test]
fn extract_chinese_person_heuristic() {
    let text = "甲方代表张三，乙方代表李四（法定代表人王小明）。";
    let ents = extract_entities(text);
    let persons: Vec<&Entity> = ents
        .iter()
        .filter(|e| e.kind == EntityKind::Person)
        .collect();
    // 启发式 — 至少应抽 1 个，理想 2-3 个
    assert!(!persons.is_empty(), "至少抽一个人名");
    let names: Vec<&str> = persons.iter().map(|e| e.value.as_str()).collect();
    // 简单姓 + 1-2 字名 — "张三" / "李四" / "王小明" 应该在
    assert!(names
        .iter()
        .any(|n| n == &"张三" || n == &"李四" || n == &"王小明"));
}

#[test]
fn empty_text_returns_empty() {
    let ents = extract_entities("");
    assert!(ents.is_empty());
}

#[test]
fn no_entities_text_returns_empty() {
    let ents = extract_entities("the quick brown fox jumps over the lazy dog");
    let chinese_kinds: Vec<&Entity> = ents
        .iter()
        .filter(|e| matches!(e.kind, EntityKind::Person | EntityKind::Organization))
        .collect();
    assert!(chinese_kinds.is_empty(), "纯英文不应误抽中文实体");
}
