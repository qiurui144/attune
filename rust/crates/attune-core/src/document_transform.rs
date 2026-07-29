use crate::document_model::{DocumentNode, DocumentOutline, NodeKind, SourceSpan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformInput {
    pub document_id: String,
    pub title: String,
    pub source_path: Option<String>,
    pub text: String,
}

pub fn transform_document(input: TransformInput) -> DocumentOutline {
    let mut nodes = Vec::new();
    let mut section_path = Vec::new();
    let mut cursor = 0usize;

    nodes.push(DocumentNode {
        node_id: format!("{}:title", input.document_id),
        parent_id: None,
        kind: NodeKind::Title,
        section_path: Vec::new(),
        text: input.title.clone(),
        span: SourceSpan {
            source_path: input.source_path.clone(),
            page_start: None,
            page_end: None,
            char_start: 0,
            char_end: 0,
        },
        quality_flags: vec!["structure_inferred".to_string()],
    });

    for raw_line in input.text.split_inclusive('\n') {
        let line_start = cursor;
        cursor += raw_line.len();

        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        let kind = classify_line(line, &section_path);
        if kind == NodeKind::Section {
            section_path = update_section_path(&section_path, line);
        }

        let text = line.to_string();
        let node_id = format!("{}:n-{}", input.document_id, nodes.len());
        nodes.push(DocumentNode {
            node_id,
            parent_id: None,
            kind,
            section_path: section_path.clone(),
            text,
            span: SourceSpan {
                source_path: input.source_path.clone(),
                page_start: None,
                page_end: None,
                char_start: line_start + raw_line.len().saturating_sub(raw_line.trim_start().len()),
                char_end: line_start + raw_line.trim_end().len(),
            },
            quality_flags: vec!["structure_inferred".to_string()],
        });
    }

    DocumentOutline {
        document_id: input.document_id,
        title: input.title,
        nodes,
    }
}

fn classify_line(line: &str, section_path: &[String]) -> NodeKind {
    if is_toc_line(line) {
        return NodeKind::Toc;
    }
    if is_heading_line(line) {
        return NodeKind::Section;
    }
    if is_troubleshooting_context(section_path) || is_troubleshooting_line(line) {
        return NodeKind::Troubleshooting;
    }
    if is_api_reference_line(line, section_path) {
        return NodeKind::ApiReference;
    }
    if is_procedure_step_line(line) {
        return NodeKind::ProcedureStep;
    }
    if is_command_line(line) {
        return NodeKind::CommandBlock;
    }
    if is_config_line(line) {
        return NodeKind::ConfigBlock;
    }
    if looks_like_table_row(line) {
        return NodeKind::TableRow;
    }
    NodeKind::Paragraph
}

fn is_toc_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if lower == "table of contents" || lower == "contents" {
        return true;
    }
    let has_dot_leader = line.contains("....");
    let ends_with_page_number = line
        .split_whitespace()
        .last()
        .is_some_and(|last| last.chars().all(|c| c.is_ascii_digit()));
    has_dot_leader && ends_with_page_number
}

fn is_heading_line(line: &str) -> bool {
    if line.len() > 120 || line.ends_with(';') || line.ends_with('{') || line.ends_with('}') {
        return false;
    }
    if is_markdown_atx_heading(line) {
        return true;
    }
    if starts_with_numbered_heading(line) {
        return true;
    }
    let lower = line.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "api reference"
            | "operation flow"
            | "troubleshooting"
            | "faq"
            | "configuration"
            | "build"
            | "overview"
    )
}

fn is_markdown_atx_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    (1..=6).contains(&hashes)
        && trimmed
            .chars()
            .nth(hashes)
            .is_some_and(|c| c.is_whitespace())
        && trimmed[hashes..].trim().chars().count() <= 100
}

fn starts_with_numbered_heading(line: &str) -> bool {
    let mut chars = line.chars().peekable();
    let mut saw_digit = false;
    while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
        saw_digit = true;
        chars.next();
    }
    if !saw_digit {
        return false;
    }
    if chars.peek().is_some_and(|c| c.is_whitespace()) {
        let rest = chars.collect::<String>();
        return looks_like_plain_numbered_heading(rest.trim());
    }
    let mut saw_dot = false;
    while chars
        .peek()
        .is_some_and(|c| *c == '.' || c.is_ascii_digit())
    {
        if chars.peek() == Some(&'.') {
            saw_dot = true;
        }
        chars.next();
    }
    saw_dot && chars.peek().is_some_and(|c| c.is_whitespace())
}

fn looks_like_plain_numbered_heading(rest: &str) -> bool {
    if rest.is_empty() || rest.len() > 60 {
        return false;
    }
    let lower = rest.to_ascii_lowercase();
    if [
        "api",
        "reference",
        "overview",
        "configuration",
        "build",
        "operation",
        "flow",
        "troubleshooting",
        "faq",
    ]
    .iter()
    .any(|word| lower.contains(word))
    {
        return true;
    }
    if rest.contains("接口")
        || rest.contains("配置")
        || rest.contains("编译")
        || rest.contains("流程")
        || rest.contains("排查")
        || rest.contains("概述")
        || rest.contains("前言")
    {
        return true;
    }
    rest.chars().count() <= 12
        && !rest.chars().any(|c| {
            matches!(
                c,
                '，' | ',' | '。' | '.' | ';' | '；' | ':' | '：' | '(' | ')'
            )
        })
}

fn update_section_path(current: &[String], heading: &str) -> Vec<String> {
    let level = heading_level(heading).unwrap_or(1).max(1);
    let mut next = current
        .iter()
        .take(level.saturating_sub(1))
        .cloned()
        .collect::<Vec<_>>();
    next.push(heading.to_string());
    next
}

fn heading_level(heading: &str) -> Option<usize> {
    let prefix = heading.split_whitespace().next()?;
    let numbered = prefix.chars().all(|c| c.is_ascii_digit() || c == '.')
        && prefix.chars().any(|c| c.is_ascii_digit());
    if !numbered {
        return Some(1);
    }
    Some(prefix.split('.').filter(|part| !part.is_empty()).count())
}

fn is_troubleshooting_context(section_path: &[String]) -> bool {
    section_path.iter().any(|section| {
        let lower = section.to_ascii_lowercase();
        lower.contains("troubleshooting")
            || lower.contains("faq")
            || section.contains("排查")
            || section.contains("常见问题")
            || section.contains("问题")
            || section.contains("解决")
    })
}

fn is_troubleshooting_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("troubleshoot")
        || lower.contains("problem:")
        || lower.contains("check:")
        || lower.contains("if ")
        || line.contains("问题现象")
        || line.contains("解决方法")
        || line.contains("排查")
}

fn is_api_reference_line(line: &str, section_path: &[String]) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("prototype:")
        || lower.starts_with("purpose:")
        || lower.starts_with("parameters:")
        || lower.starts_with("returns:")
        || line.starts_with("原型")
        || line.starts_with("作用")
        || line.starts_with("参数")
        || line.starts_with("返回")
        || (is_api_context(section_path) && looks_like_function_signature(line))
}

fn is_api_context(section_path: &[String]) -> bool {
    section_path.iter().any(|section| {
        let lower = section.to_ascii_lowercase();
        lower.contains("api")
            || lower.contains("interface")
            || lower.contains("reference")
            || section.contains("接口")
            || section.contains("函数")
    })
}

fn looks_like_function_signature(line: &str) -> bool {
    line.contains('(')
        && line.contains(')')
        && !line.contains("://")
        && !line
            .split_whitespace()
            .next()
            .unwrap_or("")
            .starts_with("http")
}

fn is_procedure_step_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("step ")
        || lower.starts_with("step:")
        || line.starts_with("步骤")
        || line.starts_with("第") && line.contains("步")
        || starts_with_ordered_list_marker(line)
}

fn starts_with_ordered_list_marker(line: &str) -> bool {
    let mut chars = line.chars().peekable();
    let mut saw_digit = false;
    while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
        saw_digit = true;
        chars.next();
    }
    saw_digit && matches!(chars.next(), Some('.') | Some(')'))
}

fn is_command_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    line.starts_with('$')
        || line.starts_with("# ")
        || lower.starts_with("sudo ")
        || lower.starts_with("source ")
        || lower.starts_with("make ")
        || lower.starts_with("./")
        || line.ends_with(';') && looks_like_function_signature(line)
}

fn is_config_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    line.contains('=') && !line.contains("==")
        || lower.starts_with("export ")
        || lower.starts_with("config_")
        || lower.starts_with("set ")
}

fn looks_like_table_row(line: &str) -> bool {
    line.matches('|').count() >= 2 || line.matches('\t').count() >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_heading_depth_updates_section_path() {
        let root = update_section_path(&[], "3 API Reference");
        assert_eq!(root, vec!["3 API Reference"]);
        let child = update_section_path(&root, "3.1 open_device");
        assert_eq!(child, vec!["3 API Reference", "3.1 open_device"]);
    }
}
