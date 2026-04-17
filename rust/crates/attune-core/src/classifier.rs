use crate::error::{Result, VaultError};
use crate::llm::LlmProvider;
use crate::taxonomy::{ClassificationResult, Taxonomy};
use std::collections::HashMap;
use std::sync::Arc;

pub struct Classifier {
    taxonomy: Arc<Taxonomy>,
    llm: Arc<dyn LlmProvider>,
    batch_size: usize,
}

impl Classifier {
    pub fn new(taxonomy: Arc<Taxonomy>, llm: Arc<dyn LlmProvider>) -> Self {
        Self { taxonomy, llm, batch_size: 5 }
    }

    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size.max(1);
        self
    }

    /// 分类单条
    pub fn classify_one(&self, title: &str, content: &str) -> Result<ClassificationResult> {
        let items = vec![(title.to_string(), content.to_string())];
        let mut results = self.classify_batch(&items)?;
        results.pop()
            .ok_or_else(|| VaultError::Classification("empty result".into()))
    }

    /// 批量分类（一次 LLM 调用处理 batch_size 条）
    pub fn classify_batch(&self, items: &[(String, String)]) -> Result<Vec<ClassificationResult>> {
        if items.is_empty() {
            return Ok(vec![]);
        }

        let mut all_results = Vec::with_capacity(items.len());
        for chunk in items.chunks(self.batch_size) {
            let batch_results = self.classify_one_llm_call(chunk)?;
            all_results.extend(batch_results);
        }
        Ok(all_results)
    }

    fn classify_one_llm_call(&self, items: &[(String, String)]) -> Result<Vec<ClassificationResult>> {
        let system = self.taxonomy.build_system_prompt();
        let user = self.taxonomy.build_user_prompt(items);
        let raw = self.llm.chat(&system, &user)?;
        self.parse_response(&raw, items.len())
    }

    fn parse_response(&self, raw: &str, expected_count: usize) -> Result<Vec<ClassificationResult>> {
        let trimmed = raw.trim();
        let json_str = extract_json_block(trimmed).unwrap_or_else(|| trimmed.to_string());

        let parsed: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| VaultError::Classification(format!("invalid JSON: {e}. raw: {}", &json_str.chars().take(200).collect::<String>())))?;

        let items_array: Vec<serde_json::Value> = if expected_count == 1 && parsed.is_object() {
            vec![parsed]
        } else if let Some(arr) = parsed.as_array() {
            arr.clone()
        } else if parsed.is_object() {
            vec![parsed]
        } else {
            return Err(VaultError::Classification("expected object or array".into()));
        };

        let mut results = Vec::with_capacity(items_array.len());
        for obj in items_array {
            let result = self.parse_single(&obj)?;
            results.push(result);
        }
        Ok(results)
    }

    fn parse_single(&self, obj: &serde_json::Value) -> Result<ClassificationResult> {
        let mut result = ClassificationResult::empty();
        result.model = self.llm.model_name().to_string();
        result.plugins_used = self.taxonomy.plugins.iter().map(|p| p.id.clone()).collect();

        if let Some(core) = obj.get("core").and_then(|v| v.as_object()) {
            for (k, v) in core {
                let values = json_to_string_vec(v);
                result.core.insert(k.clone(), values);
            }
        }

        if let Some(universal) = obj.get("universal").and_then(|v| v.as_object()) {
            for (k, v) in universal {
                if let Some(s) = v.as_str() {
                    result.universal.insert(k.clone(), s.to_string());
                } else {
                    let values = json_to_string_vec(v);
                    if let Some(first) = values.into_iter().next() {
                        result.universal.insert(k.clone(), first);
                    }
                }
            }
        }

        if let Some(plugin) = obj.get("plugin").and_then(|v| v.as_object()) {
            for (plugin_id, dims_val) in plugin {
                if let Some(dims_obj) = dims_val.as_object() {
                    let mut plugin_tags: HashMap<String, Vec<String>> = HashMap::new();
                    for (dim, values) in dims_obj {
                        plugin_tags.insert(dim.clone(), json_to_string_vec(values));
                    }
                    result.plugin.insert(plugin_id.clone(), plugin_tags);
                }
            }
        }

        Ok(result)
    }
}

fn json_to_string_vec(v: &serde_json::Value) -> Vec<String> {
    if let Some(arr) = v.as_array() {
        arr.iter().filter_map(|e| e.as_str().map(String::from)).collect()
    } else if let Some(s) = v.as_str() {
        vec![s.to_string()]
    } else {
        vec![]
    }
}

/// 从可能包含 ```json ... ``` 或其他修饰的文本中提取 JSON
fn extract_json_block(s: &str) -> Option<String> {
    if let Some(start) = s.find("```json") {
        let after = &s[start + 7..];
        if let Some(end) = after.find("```") {
            return Some(after[..end].trim().to_string());
        }
    }
    if let Some(start) = s.find("```") {
        let after = &s[start + 3..];
        if let Some(end) = after.find("```") {
            return Some(after[..end].trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlmProvider;

    fn make_classifier() -> (Classifier, Arc<MockLlmProvider>) {
        let mock = Arc::new(MockLlmProvider::new("mock-model"));
        let taxonomy = Arc::new(Taxonomy::default());
        let classifier = Classifier::new(taxonomy, mock.clone());
        (classifier, mock)
    }

    const SAMPLE_RESPONSE: &str = r#"{
        "core": {
            "domain": ["技术"],
            "topic": ["Rust 加密"],
            "purpose": ["参考资料"],
            "project": ["npu-vault"],
            "entities": ["rustls", "aes-gcm"]
        },
        "universal": {
            "difficulty": "进阶",
            "freshness": "常青",
            "action_type": "学习"
        },
        "plugin": {}
    }"#;

    #[test]
    fn classify_one_parses_response() {
        let (classifier, mock) = make_classifier();
        mock.push_response(SAMPLE_RESPONSE);
        let result = classifier.classify_one("标题", "内容").unwrap();
        assert_eq!(result.core["domain"], vec!["技术"]);
        assert_eq!(result.core["topic"], vec!["Rust 加密"]);
        assert_eq!(result.universal["difficulty"], "进阶");
        assert_eq!(result.model, "mock-model");
    }

    #[test]
    fn classify_batch_multiple() {
        let mock = Arc::new(MockLlmProvider::new("mock-model"));
        let taxonomy = Arc::new(Taxonomy::default());
        let classifier = Classifier::new(taxonomy, mock.clone()).with_batch_size(2);
        let batch_response = format!("[{}, {}]", SAMPLE_RESPONSE, SAMPLE_RESPONSE);
        mock.push_response(&batch_response);

        let items = vec![
            ("a".to_string(), "c".to_string()),
            ("b".to_string(), "c".to_string()),
        ];
        let results = classifier.classify_batch(&items).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn classify_extracts_json_from_code_block() {
        let (classifier, mock) = make_classifier();
        let wrapped = format!("```json\n{}\n```", SAMPLE_RESPONSE);
        mock.push_response(&wrapped);
        let result = classifier.classify_one("t", "c").unwrap();
        assert_eq!(result.core["domain"], vec!["技术"]);
    }

    #[test]
    fn classify_invalid_json_errors() {
        let (classifier, mock) = make_classifier();
        mock.push_response("not json at all");
        let result = classifier.classify_one("t", "c");
        assert!(result.is_err());
    }

    #[test]
    fn classify_empty_batch_returns_empty() {
        let (classifier, _mock) = make_classifier();
        let results = classifier.classify_batch(&[]).unwrap();
        assert!(results.is_empty());
    }
}
