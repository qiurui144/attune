use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentOutline {
    pub document_id: String,
    pub title: String,
    pub nodes: Vec<DocumentNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentNode {
    pub node_id: String,
    pub parent_id: Option<String>,
    pub kind: NodeKind,
    pub section_path: Vec<String>,
    pub text: String,
    pub span: SourceSpan,
    pub quality_flags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Title,
    Section,
    Paragraph,
    Procedure,
    ProcedureStep,
    ApiReference,
    Table,
    TableRow,
    CodeBlock,
    CommandBlock,
    ConfigBlock,
    Troubleshooting,
    Toc,
    HeaderFooter,
    FigureCaption,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub source_path: Option<String>,
    pub page_start: Option<u32>,
    pub page_end: Option<u32>,
    pub char_start: usize,
    pub char_end: usize,
}
