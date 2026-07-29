use attune_core::document_model::NodeKind;
use attune_core::document_transform::{transform_document, TransformInput};

#[test]
fn transform_detects_api_procedure_commands_and_toc_noise() {
    let input = TransformInput {
        document_id: "manual-1".to_string(),
        title: "Controller Manual".to_string(),
        source_path: Some("/docs/controller.pdf".to_string()),
        text: "\
Table of Contents
3.1 open_device ................ 7
3.2 close_device ............... 8

3 API Reference
3.1 open_device
Prototype: int open_device(struct device *dev)
Purpose: initialize the device.

4 Operation Flow
Step 1 Initialize the device.
open_device(dev);
Step 2 Start transfer.
start_transfer(dev);

5 Troubleshooting
If transfer returns zero bytes, verify the allocated buffer.
"
        .to_string(),
    };

    let outline = transform_document(input);
    assert!(outline.nodes.iter().any(|n| n.kind == NodeKind::Toc));
    assert!(outline
        .nodes
        .iter()
        .any(|n| { n.kind == NodeKind::ApiReference && n.text.contains("open_device") }));
    assert!(outline
        .nodes
        .iter()
        .any(|n| n.kind == NodeKind::ProcedureStep && n.text.contains("Step 1")));
    assert!(outline
        .nodes
        .iter()
        .any(|n| { n.kind == NodeKind::CommandBlock && n.text.contains("start_transfer") }));
    assert!(outline
        .nodes
        .iter()
        .any(|n| { n.kind == NodeKind::Troubleshooting && n.text.contains("zero bytes") }));
}

#[test]
fn transform_does_not_treat_plain_numbered_operations_as_sections() {
    let outline = transform_document(TransformInput {
        document_id: "manual-2".to_string(),
        title: "Manual".to_string(),
        source_path: None,
        text: "\
3 API Reference
1 Call open_device with a valid handle
2 Check the return value
"
        .to_string(),
    });

    assert!(outline
        .nodes
        .iter()
        .any(|n| n.kind == NodeKind::Section && n.text == "3 API Reference"));
    assert!(outline.nodes.iter().any(|n| {
        n.kind != NodeKind::Section && n.text == "1 Call open_device with a valid handle"
    }));
}

#[test]
fn transform_detects_markdown_atx_headings_as_sections() {
    let outline = transform_document(TransformInput {
        document_id: "markdown-manual".to_string(),
        title: "Manual".to_string(),
        source_path: None,
        text: "\
# Heading One

First body paragraph.

## Heading Two

Second body paragraph.
"
        .to_string(),
    });

    assert!(outline
        .nodes
        .iter()
        .any(|n| n.kind == NodeKind::Section && n.text == "# Heading One"));
    assert!(outline
        .nodes
        .iter()
        .any(|n| n.kind == NodeKind::Section && n.text == "## Heading Two"));
}
