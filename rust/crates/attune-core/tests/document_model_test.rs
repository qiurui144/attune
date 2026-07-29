use attune_core::document_model::{DocumentNode, NodeKind, SourceSpan};

#[test]
fn document_node_preserves_structure_and_provenance() {
    let node = DocumentNode {
        node_id: "doc-1:n-1".to_string(),
        parent_id: Some("doc-1:s-4".to_string()),
        kind: NodeKind::ProcedureStep,
        section_path: vec!["Module API".to_string(), "Read Flow".to_string()],
        text: "Step 1: initialize the controller.".to_string(),
        span: SourceSpan {
            source_path: Some("/manuals/example.pdf".to_string()),
            page_start: Some(12),
            page_end: Some(12),
            char_start: 1200,
            char_end: 1238,
        },
        quality_flags: vec!["text_extracted".to_string()],
    };

    assert_eq!(node.kind, NodeKind::ProcedureStep);
    assert_eq!(node.section_path.join(" > "), "Module API > Read Flow");
    assert_eq!(node.span.page_start, Some(12));
}
