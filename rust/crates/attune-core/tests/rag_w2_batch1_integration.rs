// W2 batch 1 集成测试：J1 + J3 + J5 + B1 backend 端到端
//
// 测试场景：
// 1. J1: extract_sections_with_path 在真实多级 markdown 文档上的路径正确性 +
//    breadcrumb prefix 注入后的内容形态
// 2. J3: SearchParams 默认 0.65 阈值；显式 None 时关闭
// 3. J5: parse_confidence 在多种 LLM 响应格式下都能正确解析；strip 干净
// 4. B1: Citation 含 offset/breadcrumb 字段时序列化往返
//
// 设计来源：docs/superpowers/specs/2026-04-27-w2-rag-quality-batch1-design.md
// 上游参照：吴师兄 RAG 文章 + CRAG arXiv:2401.15884 + Self-RAG arXiv:2310.11511

use attune_core::{parse_confidence, strip_confidence_marker, Citation, ChatResponse};
use attune_core::chunker::{extract_sections_with_path, SectionWithPath};
use attune_core::search::SearchParams;

// ── J1 集成 ────────────────────────────────────────────────────────────

#[test]
fn j1_realistic_multi_level_doc_path_correct() {
    // 模拟一份保险产品手册（吴师兄文章原型场景）
    let doc = r#"# XX 重疾险产品说明书

本产品由 XX 保险公司承保。

## 第一章 投保规则

### 1.1 投保年龄

18-65 周岁。

### 1.2 投保职业

1-4 类职业可投。

## 第二章 保险责任

### 2.1 重疾保障

确诊即赔。

### 2.2 等待期

#### 2.2.1 等待期定义

本产品等待期为 90 天。

#### 2.2.2 等待期内出险

不承担给付责任。
"#;

    let sections = extract_sections_with_path(doc);
    // 验证: "等待期内出险"那段的 path = [产品说明书, 第二章, 2.2 等待期, 2.2.2 等待期内出险]
    let target = sections
        .iter()
        .find(|s| s.content.contains("不承担给付责任"))
        .expect("找不到等待期内出险章节");

    assert_eq!(target.path.len(), 4, "应有 4 层路径，得到 {:?}", target.path);
    assert!(target.path[0].contains("XX 重疾险"));
    assert!(target.path[1].contains("第二章"));
    assert!(target.path[2].contains("2.2 等待期"));
    assert!(target.path[3].contains("2.2.2 等待期内出险"));

    // 注入面包屑后的内容应有清晰的 path 前缀
    let prefixed = target.with_breadcrumb_prefix();
    assert!(prefixed.starts_with("> XX 重疾险"));
    assert!(prefixed.contains("> 第二章"));
    assert!(prefixed.contains("不承担给付责任"));
}

#[test]
fn j1_dedent_across_chapters() {
    // 跨章节 dedent：第一章下的内容不应混进第二章的 path
    let doc = "# 文档\n\n## 第一章\n\n### 1.1 节\n\nA\n\n## 第二章\n\nB";
    let sections = extract_sections_with_path(doc);
    let b_section = sections.iter().find(|s| s.content.contains("\nB")).expect("missing B section");
    // B 的 path 应是 [文档, 第二章]，不应包含 "1.1 节"
    assert!(
        b_section.path.iter().all(|p| !p.contains("1.1")),
        "dedent 失败，B 的 path = {:?}",
        b_section.path
    );
    assert!(b_section.path.iter().any(|p| p.contains("第二章")));
}

// ── J3 集成（per reviewer S2 路径分离后）────────────────────────────────

#[test]
fn j3_general_search_default_no_threshold() {
    // 通用 search 路径默认不启用 J3 阈值（保持 W2 前 Chrome 扩展契约）
    let p = SearchParams::with_defaults(5);
    assert_eq!(p.min_score, None);
}

#[test]
fn j3_rag_path_defaults_065() {
    // chat / RAG 路径才启用 J3 0.65（吴师兄经验值，保守端）
    let rag = SearchParams::with_defaults_for_rag(5);
    assert_eq!(rag.min_score, Some(0.65));
}

// ── J5 集成 ────────────────────────────────────────────────────────────

#[test]
fn j5_parse_confidence_realistic_llm_outputs() {
    // 模拟真实 LLM 的几种输出形式
    let cases = [
        ("根据知识库，等待期为 90 天。\n\n【置信度: 5/5】", 5),
        ("根据知识库，等待期为 90 天。\n【置信度：4/5】", 4),
        ("可能是 30-90 天，建议查阅条款。\n\n【置信度: 1/5】", 1),
        // LLM 用英文输出（用户问英文 query 时常见）
        ("The waiting period is 90 days.\n\n[Confidence: 5/5]", 5),
        ("Possibly 30-90 days.\n\n(confidence: 2/5)", 2),
        // 缺失 marker → 默认 3
        ("没标 confidence", 3),
    ];
    for (input, expected) in cases {
        assert_eq!(
            parse_confidence(input),
            expected,
            "input: {input:?}"
        );
    }
}

#[test]
fn j5_strip_then_parse_round_trip() {
    // 用户最终看到的 content 应已剥离 marker；前端不需要再处理
    let raw = "等待期为 90 天。\n\n【置信度: 5/5】";
    let stripped = strip_confidence_marker(raw);
    assert_eq!(stripped, "等待期为 90 天。");
    // parse_confidence 仍能从原 raw 正确解析（chat.rs 流程是先 parse 再 strip）
    assert_eq!(parse_confidence(raw), 5);
}

// ── B1 集成 ────────────────────────────────────────────────────────────

#[test]
fn b1_citation_serializes_with_breadcrumb_offsets() {
    // Citation 序列化包含 B1 新字段（前端能拿到 offset + breadcrumb）
    let citation = Citation {
        item_id: "item-uuid".into(),
        title: "XX 重疾险产品说明书".into(),
        relevance: 0.92,
        chunk_offset_start: Some(1024),
        chunk_offset_end: Some(1280),
        breadcrumb: vec![
            "XX 重疾险产品说明书".into(),
            "第二章 保险责任".into(),
            "2.2 等待期".into(),
        ],
    };
    let json = serde_json::to_string(&citation).unwrap();
    assert!(json.contains("\"chunk_offset_start\":1024"));
    assert!(json.contains("\"chunk_offset_end\":1280"));
    assert!(json.contains("\"breadcrumb\":["));
    assert!(json.contains("第二章"));
}

#[test]
fn b1_citation_web_source_has_no_offsets() {
    // Web 搜索结果无源 item，offset 必须 None
    // W3 batch A reviewer S2: skip_serializing_if 让 None 不出现在 JSON（前端不必处理 null）
    let citation = Citation {
        item_id: "web:https://example.com/article".into(),
        title: "Web Article".into(),
        relevance: 0.55,
        chunk_offset_start: None,
        chunk_offset_end: None,
        breadcrumb: vec![],
    };
    let json = serde_json::to_string(&citation).unwrap();
    assert!(!json.contains("chunk_offset_start"), "None offset 字段不应出现在 JSON: {json}");
    assert!(!json.contains("chunk_offset_end"), "None offset 字段不应出现在 JSON: {json}");
    assert!(!json.contains("breadcrumb"), "空 breadcrumb 不应出现在 JSON: {json}");
}

#[test]
fn chat_response_w2_fields_in_serde() {
    let r = ChatResponse {
        content: "answer".into(),
        citations: vec![],
        knowledge_count: 3,
        web_search_used: false,
        confidence: 4,
        secondary_retrieval_used: true,
    };
    let json = serde_json::to_string(&r).unwrap();
    assert!(json.contains("\"confidence\":4"));
    assert!(json.contains("\"secondary_retrieval_used\":true"));
}

// ── 跨 feature 综合 ────────────────────────────────────────────────────

#[test]
fn end_to_end_breadcrumb_in_prefix_then_used_as_citation() {
    // 模拟生产路径：
    //   doc → extract_sections_with_path → 拿到 path → 创建 Citation 时填 breadcrumb
    let doc = "# 公司手册\n\n## 第三章 福利\n\n### 3.2 假期\n\n年假 15 天。";
    let sections = extract_sections_with_path(doc);
    let target = sections.iter().find(|s| s.content.contains("年假")).unwrap();

    let citation = Citation {
        item_id: "doc-1".into(),
        title: "公司手册".into(),
        relevance: 0.88,
        chunk_offset_start: Some(0),
        chunk_offset_end: Some(target.content.len()),
        breadcrumb: target.path.clone(),
    };

    assert_eq!(
        citation.breadcrumb,
        vec!["公司手册".to_string(), "第三章 福利".to_string(), "3.2 假期".to_string()]
    );
    let json = serde_json::to_string(&citation).unwrap();
    assert!(json.contains("3.2 假期"));
}

#[test]
fn section_with_path_clone_and_eq() {
    // 防止未来给 SectionWithPath 加字段时漏改 PartialEq
    let a = SectionWithPath {
        section_idx: 0,
        path: vec!["A".into()],
        content: "x".into(),
    };
    let b = a.clone();
    assert_eq!(a, b);
}
