// npu-vault/crates/vault-core/src/skill_evolution.rs
//
// 技能自动进化模块（SkillClaw 启发）
//
// 工作流：
//   1. 收集本地搜索失败信号（knowledge_count == 0）
//   2. 当累积信号 >= EVOLVE_THRESHOLD 时触发一次进化周期
//   3. LLM 分析失败查询 → 提炼主题和同义词扩展词组
//   4. 将扩展词组写入 app_settings["search"]["learned_expansions"]
//   5. Chat 检索前自动扩展查询，命中率随时间提升

use crate::error::{Result, VaultError};
use crate::llm::LlmProvider;
use crate::store::{SkillSignal, Store};

/// 累积多少条失败信号后触发一次进化（可调）
pub const EVOLVE_THRESHOLD: usize = 10;

/// 单次进化最多处理的信号数（避免 LLM prompt 过长）
const MAX_SIGNALS_PER_CYCLE: usize = 30;

/// app_settings 中存储扩展词的键路径
const SETTINGS_KEY: &str = "app_settings";

// ── 核心进化周期 ──────────────────────────────────────────────────────────────

/// 执行一次技能进化周期。
///
/// 返回本次新增/更新的扩展词条数（0 = 无变化或信号不足）。
pub fn run_evolution_cycle(store: &Store, llm: &dyn LlmProvider) -> Result<usize> {
    // 1. 检查累积信号是否达到阈值
    let count = store.count_unprocessed_signals()?;
    if count < EVOLVE_THRESHOLD {
        return Ok(0);
    }

    // 2. 读取未处理信号
    let signals = store.get_unprocessed_signals(MAX_SIGNALS_PER_CYCLE)?;
    if signals.is_empty() {
        return Ok(0);
    }

    // 3. 构建 LLM prompt
    let prompt = build_evolution_prompt(&signals);
    let messages = vec![crate::llm::ChatMessage::user(&prompt)];

    let raw_response = llm.chat_with_history(&messages).map_err(|e| {
        VaultError::LlmUnavailable(format!("skill evolution LLM call: {e}"))
    })?;

    // 4. 解析 LLM 返回的 JSON 扩展词组
    let expansions = parse_expansion_response(&raw_response);
    if expansions.is_empty() {
        // LLM 返回无法解析，标记信号已处理避免重复
        let ids: Vec<i64> = signals.iter().map(|s| s.id).collect();
        let _ = store.mark_signals_processed(&ids);
        return Ok(0);
    }

    // 5. 合并到现有 app_settings
    let merged = merge_expansions_into_settings(store, &expansions)?;

    // 6. 标记信号为已处理
    let ids: Vec<i64> = signals.iter().map(|s| s.id).collect();
    store.mark_signals_processed(&ids)?;

    eprintln!(
        "[skill_evolution] processed {} signals, {merged} expansion entries updated",
        signals.len()
    );

    Ok(merged)
}

// ── 工具函数 ──────────────────────────────────────────────────────────────────

/// 构建给 LLM 的进化分析 prompt
fn build_evolution_prompt(signals: &[SkillSignal]) -> String {
    let queries: Vec<&str> = signals.iter().map(|s| s.query.as_str()).collect();
    let query_list = queries
        .iter()
        .enumerate()
        .map(|(i, q)| format!("{}. {}", i + 1, q))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"以下是用户在知识库中搜索但本地无结果的查询列表：

{query_list}

请分析这些查询，识别其中的主题聚类，并为每个主题生成搜索扩展词（同义词、相关术语、上位词）。

要求：
1. 每个主题最多 5 个扩展词
2. 扩展词必须是中文或英文关键词，不是完整句子
3. 只返回 JSON，格式如下：

```json
{{
  "expansions": [
    {{"topic": "主题关键词", "terms": ["扩展词1", "扩展词2", "扩展词3"]}},
    {{"topic": "另一主题", "terms": ["扩展词A", "扩展词B"]}}
  ]
}}
```

只返回 JSON，不要任何解释。"#
    )
}

/// 解析 LLM 返回文本中的 JSON 扩展词组
fn parse_expansion_response(response: &str) -> Vec<(String, Vec<String>)> {
    // 从 ```json ... ``` 或直接 JSON 块中提取
    let json_str = extract_json_block(response);

    let value: serde_json::Value = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let arr = match value.get("expansions").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return vec![],
    };

    arr.iter()
        .filter_map(|entry| {
            let topic = entry.get("topic")?.as_str()?.to_string();
            let terms: Vec<String> = entry
                .get("terms")?
                .as_array()?
                .iter()
                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                .filter(|s| !s.is_empty())
                .take(5)
                .collect();
            if topic.is_empty() || terms.is_empty() {
                None
            } else {
                Some((topic, terms))
            }
        })
        .collect()
}

/// 从 LLM 响应中提取 JSON 内容（处理 markdown 代码块）
fn extract_json_block(text: &str) -> String {
    // 尝试 ```json ... ```
    if let Some(start) = text.find("```json") {
        let after = &text[start + 7..];
        if let Some(end) = after.find("```") {
            return after[..end].trim().to_string();
        }
    }
    // 尝试 ``` ... ```
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        if let Some(end) = after.find("```") {
            return after[..end].trim().to_string();
        }
    }
    // 尝试直接提取 { ... }
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            if end > start {
                return text[start..=end].to_string();
            }
        }
    }
    text.trim().to_string()
}

/// 将新扩展词合并写入 app_settings，返回实际新增/更新条数
fn merge_expansions_into_settings(
    store: &Store,
    expansions: &[(String, Vec<String>)],
) -> Result<usize> {
    // 读取现有 settings
    let mut settings: serde_json::Value = store
        .get_meta(SETTINGS_KEY)?
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    // 确保 search 对象存在
    if settings.get("search").is_none() {
        settings["search"] = serde_json::json!({});
    }

    // 读取现有扩展词 map
    let existing_map = settings["search"]
        .get("learned_expansions")
        .cloned()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

    let mut map = existing_map;
    let mut changed = 0usize;

    for (topic, terms) in expansions {
        let key = topic.to_lowercase();
        let new_val = serde_json::Value::Array(
            terms.iter().map(|t| serde_json::Value::String(t.clone())).collect(),
        );
        // 合并：已有的保留，新词追加（避免 LLM 幻觉覆盖有效词）
        let existing_terms: Vec<String> = map
            .get(&key)
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|t| t.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();

        let mut merged_terms = existing_terms;
        for t in terms {
            if !merged_terms.contains(t) {
                merged_terms.push(t.clone());
            }
        }
        // 最多保留 8 个扩展词/主题（防止膨胀）
        merged_terms.truncate(8);

        let merged_val = serde_json::Value::Array(
            merged_terms.iter().map(|t| serde_json::Value::String(t.clone())).collect(),
        );

        if map.get(&key) != Some(&new_val) {
            changed += 1;
        }
        map.insert(key, merged_val);
    }

    settings["search"]["learned_expansions"] = serde_json::Value::Object(map);

    let data = serde_json::to_vec(&settings)?;
    store.set_meta(SETTINGS_KEY, &data)?;

    Ok(changed)
}

// ── 查询扩展工具（供 Chat 路由调用）────────────────────────────────────────────

/// 从 app_settings 中读取 learned_expansions，对查询进行语义扩展。
///
/// 扩展规则：查询中包含某主题词时，将其扩展词追加到查询末尾。
/// 例：原始查询 "ipc分类" → 扩展后 "ipc分类 IPC分类号 专利分类 技术领域分类"
pub fn expand_query(query: &str, settings: &serde_json::Value) -> String {
    let expansions = match settings
        .get("search")
        .and_then(|s| s.get("learned_expansions"))
        .and_then(|v| v.as_object())
    {
        Some(m) => m,
        None => return query.to_string(),
    };

    let query_lower = query.to_lowercase();
    let mut extra_terms: Vec<String> = vec![];

    for (topic, terms_val) in expansions {
        // 查询中包含主题词（大小写不敏感）
        if query_lower.contains(topic.as_str()) {
            if let Some(arr) = terms_val.as_array() {
                for t in arr {
                    if let Some(term) = t.as_str() {
                        // 扩展词不在原始查询中才追加
                        if !query_lower.contains(&term.to_lowercase()) {
                            extra_terms.push(term.to_string());
                        }
                    }
                }
            }
        }
    }

    if extra_terms.is_empty() {
        return query.to_string();
    }

    format!("{} {}", query, extra_terms.join(" "))
}

// ── 单元测试 ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_expansion_response_valid_json() {
        let resp = r#"```json
{"expansions": [{"topic": "专利检索", "terms": ["IPC分类", "专利数据库", "先行技术"]}]}
```"#;
        let result = parse_expansion_response(resp);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "专利检索");
        assert_eq!(result[0].1.len(), 3);
    }

    #[test]
    fn parse_expansion_response_bare_json() {
        let resp = r#"{"expansions": [{"topic": "法律", "terms": ["合同法", "民法典"]}]}"#;
        let result = parse_expansion_response(resp);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "法律");
    }

    #[test]
    fn parse_expansion_response_invalid_returns_empty() {
        let result = parse_expansion_response("sorry, I cannot do that");
        assert!(result.is_empty());
    }

    #[test]
    fn expand_query_matches_topic() {
        let settings = serde_json::json!({
            "search": {
                "learned_expansions": {
                    "专利": ["IPC分类", "权利要求", "说明书"]
                }
            }
        });
        let expanded = expand_query("专利检索方法", &settings);
        assert!(expanded.contains("IPC分类"), "should append expansion terms");
        assert!(expanded.starts_with("专利检索方法"), "original query preserved");
    }

    #[test]
    fn expand_query_no_match_returns_original() {
        let settings = serde_json::json!({
            "search": {"learned_expansions": {"patent": ["USPTO", "claims"]}}
        });
        let q = "Python programming";
        assert_eq!(expand_query(q, &settings), q);
    }

    #[test]
    fn expand_query_no_expansions_returns_original() {
        let settings = serde_json::json!({"search": {}});
        assert_eq!(expand_query("test query", &settings), "test query");
    }

    #[test]
    fn merge_expansions_into_settings_deduplicates() {
        let store = Store::open_memory().unwrap();
        // 第一次写入
        let expansions1 = vec![("专利".to_string(), vec!["IPC分类".to_string(), "权利要求".to_string()])];
        let n = merge_expansions_into_settings(&store, &expansions1).unwrap();
        assert_eq!(n, 1);

        // 第二次写入相同主题，追加新词
        let expansions2 = vec![("专利".to_string(), vec!["权利要求".to_string(), "说明书".to_string()])];
        merge_expansions_into_settings(&store, &expansions2).unwrap();

        // 验证合并后词表包含所有词，无重复
        let settings: serde_json::Value = serde_json::from_slice(
            &store.get_meta("app_settings").unwrap().unwrap()
        ).unwrap();
        let terms = settings["search"]["learned_expansions"]["专利"].as_array().unwrap();
        assert!(terms.len() == 3, "should have 3 unique terms, got {}", terms.len());
    }

    #[test]
    fn store_skill_signal_roundtrip() {
        let store = Store::open_memory().unwrap();
        store.record_skill_signal("专利检索", 0, false).unwrap();
        store.record_skill_signal("合同纠纷", 0, true).unwrap();

        assert_eq!(store.count_unprocessed_signals().unwrap(), 2);

        let sigs = store.get_unprocessed_signals(10).unwrap();
        assert_eq!(sigs.len(), 2);
        assert_eq!(sigs[0].query, "专利检索");
        assert!(!sigs[0].web_used);
        assert!(sigs[1].web_used);

        store.mark_signals_processed(&[sigs[0].id, sigs[1].id]).unwrap();
        assert_eq!(store.count_unprocessed_signals().unwrap(), 0);
    }
}
