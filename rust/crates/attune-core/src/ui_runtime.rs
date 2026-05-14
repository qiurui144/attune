//! UI runtime — 解析 plugin 提供的 ui_components yaml + 渲染基础表单 HTML.
//!
//! 范围:
//! - 解析 yaml form schema → 内部 FormSchema
//! - 渲染基础 HTML (input / select / textarea / submit)
//! - 收集表单提交 → JSON facts (供 capability_dispatch 调用)
//!
//! 不做:
//! - 复杂 layout (table / tab) — 调用方按需扩展
//! - 客户端 JS 校验 (可由 plugin 注入)

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormSchema {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub fields: Vec<FormField>,
    /// 提交后调用的 agent / capability id
    pub submit_target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormField {
    pub name: String,
    pub label: String,
    /// "text" | "number" | "date" | "select" | "textarea" | "checkbox"
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub placeholder: String,
    #[serde(default)]
    pub options: Vec<FormOption>,
    #[serde(default)]
    pub help: String,
    #[serde(default)]
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormOption {
    pub value: String,
    pub label: String,
}

/// 渲染 schema 为 HTML 字符串 (调用方嵌入 attune Web UI iframe)
pub fn render_html(schema: &FormSchema) -> String {
    let mut html = String::with_capacity(2048);
    html.push_str("<!DOCTYPE html><html lang=\"zh-CN\"><head><meta charset=\"UTF-8\"><title>");
    html.push_str(&escape_html(&schema.title));
    html.push_str("</title>");
    html.push_str("<style>body{font-family:-apple-system,sans-serif;max-width:640px;margin:24px auto;padding:0 16px}label{display:block;margin:8px 0 4px;font-size:13px;color:#555}input,select,textarea{width:100%;padding:6px 8px;box-sizing:border-box;font-size:13px;border:1px solid #ccc;border-radius:3px}.required{color:#d23}.help{font-size:11px;color:#888}button{margin-top:16px;padding:10px 20px;background:#1976d2;color:white;border:0;border-radius:3px;cursor:pointer}</style>");
    html.push_str("</head><body>");
    html.push_str("<h1>");
    html.push_str(&escape_html(&schema.title));
    html.push_str("</h1>");
    if !schema.description.is_empty() {
        html.push_str("<p>");
        html.push_str(&escape_html(&schema.description));
        html.push_str("</p>");
    }
    html.push_str("<form id=\"attune-form\" data-target=\"");
    html.push_str(&escape_html(&schema.submit_target));
    html.push_str("\">");

    for field in &schema.fields {
        render_field(&mut html, field);
    }

    html.push_str("<button type=\"submit\">提交 →</button>");
    html.push_str("</form></body></html>");
    html
}

fn render_field(html: &mut String, f: &FormField) {
    html.push_str("<label>");
    html.push_str(&escape_html(&f.label));
    if f.required {
        html.push_str(" <span class=\"required\">*</span>");
    }
    html.push_str("</label>");

    match f.field_type.as_str() {
        "textarea" => {
            html.push_str("<textarea name=\"");
            html.push_str(&escape_html(&f.name));
            html.push_str("\" rows=\"3\"");
            if f.required {
                html.push_str(" required");
            }
            if !f.placeholder.is_empty() {
                html.push_str(" placeholder=\"");
                html.push_str(&escape_html(&f.placeholder));
                html.push('"');
            }
            html.push('>');
            if let Some(d) = &f.default_value {
                html.push_str(&escape_html(d));
            }
            html.push_str("</textarea>");
        }
        "select" => {
            html.push_str("<select name=\"");
            html.push_str(&escape_html(&f.name));
            html.push('"');
            if f.required {
                html.push_str(" required");
            }
            html.push('>');
            html.push_str("<option value=\"\">-- 请选择 --</option>");
            for opt in &f.options {
                html.push_str("<option value=\"");
                html.push_str(&escape_html(&opt.value));
                html.push('"');
                if f.default_value.as_deref() == Some(opt.value.as_str()) {
                    html.push_str(" selected");
                }
                html.push('>');
                html.push_str(&escape_html(&opt.label));
                html.push_str("</option>");
            }
            html.push_str("</select>");
        }
        "checkbox" => {
            html.push_str("<input type=\"checkbox\" name=\"");
            html.push_str(&escape_html(&f.name));
            html.push_str("\" value=\"true\"");
            if f.default_value.as_deref() == Some("true") {
                html.push_str(" checked");
            }
            html.push('>');
        }
        _ => {
            // text / number / date / default
            let html_type = match f.field_type.as_str() {
                "number" => "number",
                "date" => "date",
                _ => "text",
            };
            html.push_str("<input type=\"");
            html.push_str(html_type);
            html.push_str("\" name=\"");
            html.push_str(&escape_html(&f.name));
            html.push('"');
            if f.required {
                html.push_str(" required");
            }
            if !f.placeholder.is_empty() {
                html.push_str(" placeholder=\"");
                html.push_str(&escape_html(&f.placeholder));
                html.push('"');
            }
            if let Some(d) = &f.default_value {
                html.push_str(" value=\"");
                html.push_str(&escape_html(d));
                html.push('"');
            }
            html.push('>');
        }
    }

    if !f.help.is_empty() {
        html.push_str("<div class=\"help\">");
        html.push_str(&escape_html(&f.help));
        html.push_str("</div>");
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_with_fields() -> FormSchema {
        FormSchema {
            id: "test_form".into(),
            title: "测试表单".into(),
            description: "示例".into(),
            submit_target: "agent:test_agent".into(),
            fields: vec![
                FormField {
                    name: "principal".into(),
                    label: "本金".into(),
                    field_type: "number".into(),
                    required: true,
                    placeholder: "500000".into(),
                    options: vec![],
                    help: "依据借条第 1 条".into(),
                    default_value: None,
                },
                FormField {
                    name: "rate_type".into(),
                    label: "利率类型".into(),
                    field_type: "select".into(),
                    required: true,
                    placeholder: "".into(),
                    options: vec![
                        FormOption { value: "day".into(), label: "日利率".into() },
                        FormOption { value: "month".into(), label: "月利率".into() },
                        FormOption { value: "year".into(), label: "年利率".into() },
                    ],
                    help: "".into(),
                    default_value: Some("month".into()),
                },
                FormField {
                    name: "loan_doc_exists".into(),
                    label: "借条原件".into(),
                    field_type: "checkbox".into(),
                    required: false,
                    placeholder: "".into(),
                    options: vec![],
                    help: "".into(),
                    default_value: None,
                },
                FormField {
                    name: "evidence".into(),
                    label: "证据描述".into(),
                    field_type: "textarea".into(),
                    required: false,
                    placeholder: "请说明借款依据".into(),
                    options: vec![],
                    help: "".into(),
                    default_value: None,
                },
            ],
        }
    }

    #[test]
    fn renders_form_with_all_field_types() {
        let html = render_html(&schema_with_fields());
        assert!(html.contains("<title>测试表单</title>"));
        assert!(html.contains(r#"name="principal""#));
        assert!(html.contains(r#"type="number""#));
        assert!(html.contains(r#"<select name="rate_type""#));
        assert!(html.contains(r#"<option value="month" selected"#));
        assert!(html.contains(r#"type="checkbox" name="loan_doc_exists""#));
        assert!(html.contains(r#"<textarea name="evidence""#));
        assert!(html.contains(r#"data-target="agent:test_agent""#));
    }

    #[test]
    fn required_fields_marked_with_asterisk() {
        let html = render_html(&schema_with_fields());
        // principal required → 含红星
        assert!(html.contains("<span class=\"required\">*</span>"));
        // checkbox + textarea 都不 required → 应只 2 个红星 (principal + rate_type)
        let count = html.matches("<span class=\"required\">").count();
        assert_eq!(count, 2);
    }

    #[test]
    fn html_escapes_special_chars() {
        let s = FormSchema {
            id: "x".into(),
            title: "测试 <script>alert(1)</script>".into(),
            description: r#"含 "引号" 和 &符号"#.into(),
            submit_target: "agent:x".into(),
            fields: vec![],
        };
        let html = render_html(&s);
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&quot;引号&quot;"));
    }

    #[test]
    fn yaml_roundtrip() {
        let yaml = r#"
id: test_form
title: 测试
description: ""
submit_target: agent:x
fields:
  - name: a
    label: A
    type: text
    required: true
"#;
        let s: FormSchema = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(s.id, "test_form");
        assert_eq!(s.fields.len(), 1);
        let html = render_html(&s);
        assert!(html.contains(r#"name="a""#));
    }
}
