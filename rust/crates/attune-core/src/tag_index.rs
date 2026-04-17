use crate::crypto::Key32;
use crate::error::Result;
use crate::store::Store;
use crate::taxonomy::ClassificationResult;
use std::collections::{HashMap, HashSet};

/// 标签反向索引
/// forward: (dimension, value) -> {item_ids}
/// reverse: item_id -> [(dimension, value)]
pub struct TagIndex {
    forward: HashMap<(String, String), HashSet<String>>,
    reverse: HashMap<String, Vec<(String, String)>>,
}

impl TagIndex {
    pub fn new() -> Self {
        Self {
            forward: HashMap::new(),
            reverse: HashMap::new(),
        }
    }

    /// 从 store 构建索引（解密所有 items 的 tags）
    pub fn build(store: &Store, dek: &Key32) -> Result<Self> {
        let mut index = Self::new();
        let ids = store.list_all_item_ids()?;
        for id in ids {
            if let Some(tags_json) = store.get_tags_json(dek, &id)? {
                if let Ok(result) = serde_json::from_str::<ClassificationResult>(&tags_json) {
                    index.upsert(&id, &result);
                }
            }
        }
        Ok(index)
    }

    /// 插入或更新一个 item 的标签集合
    pub fn upsert(&mut self, item_id: &str, tags: &ClassificationResult) {
        self.remove(item_id);

        let mut pairs: Vec<(String, String)> = Vec::new();

        for (dim, values) in &tags.core {
            for v in values {
                pairs.push((dim.clone(), v.clone()));
            }
        }
        for (dim, value) in &tags.universal {
            pairs.push((dim.clone(), value.clone()));
        }
        for plugin_dims in tags.plugin.values() {
            for (dim, values) in plugin_dims {
                for v in values {
                    pairs.push((dim.clone(), v.clone()));
                }
            }
        }

        for pair in &pairs {
            self.forward.entry(pair.clone())
                .or_default()
                .insert(item_id.to_string());
        }
        self.reverse.insert(item_id.to_string(), pairs);
    }

    /// 删除一个 item 的所有标签
    pub fn remove(&mut self, item_id: &str) {
        if let Some(pairs) = self.reverse.remove(item_id) {
            for pair in pairs {
                if let Some(set) = self.forward.get_mut(&pair) {
                    set.remove(item_id);
                    if set.is_empty() {
                        self.forward.remove(&pair);
                    }
                }
            }
        }
    }

    /// 查询: 某维度某值的所有 item_id
    pub fn query(&self, dimension: &str, value: &str) -> Vec<String> {
        self.forward
            .get(&(dimension.to_string(), value.to_string()))
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// AND 组合查询
    pub fn query_and(&self, filters: &[(String, String)]) -> Vec<String> {
        if filters.is_empty() {
            return vec![];
        }
        let mut sets: Vec<&HashSet<String>> = Vec::new();
        for (dim, val) in filters {
            match self.forward.get(&(dim.clone(), val.clone())) {
                Some(s) => sets.push(s),
                None => return vec![],
            }
        }
        sets.sort_by_key(|s| s.len());
        let mut result: HashSet<String> = sets[0].clone();
        for s in &sets[1..] {
            result.retain(|id| s.contains(id));
        }
        result.into_iter().collect()
    }

    /// OR 组合查询
    pub fn query_or(&self, filters: &[(String, String)]) -> Vec<String> {
        let mut result: HashSet<String> = HashSet::new();
        for (dim, val) in filters {
            if let Some(set) = self.forward.get(&(dim.clone(), val.clone())) {
                result.extend(set.iter().cloned());
            }
        }
        result.into_iter().collect()
    }

    /// 某维度的所有值 + count 直方图
    pub fn histogram(&self, dimension: &str) -> Vec<(String, usize)> {
        let mut counts: Vec<(String, usize)> = self.forward
            .iter()
            .filter(|((dim, _), _)| dim == dimension)
            .map(|((_, val), set)| (val.clone(), set.len()))
            .collect();
        counts.sort_by(|a, b| b.1.cmp(&a.1));
        counts
    }

    /// 所有出现过的维度名
    pub fn all_dimensions(&self) -> Vec<String> {
        let dims: HashSet<String> = self.forward.keys().map(|(d, _)| d.clone()).collect();
        let mut sorted: Vec<String> = dims.into_iter().collect();
        sorted.sort();
        sorted
    }

    pub fn item_count(&self) -> usize {
        self.reverse.len()
    }
}

impl Default for TagIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tags(domain: &str, topic: &str) -> ClassificationResult {
        let mut tags = ClassificationResult::empty();
        tags.core.insert("domain".into(), vec![domain.into()]);
        tags.core.insert("topic".into(), vec![topic.into()]);
        tags.universal.insert("difficulty".into(), "进阶".into());
        tags
    }

    #[test]
    fn build_empty_index() {
        let idx = TagIndex::new();
        assert_eq!(idx.item_count(), 0);
        assert!(idx.query("domain", "技术").is_empty());
    }

    #[test]
    fn upsert_and_query() {
        let mut idx = TagIndex::new();
        idx.upsert("item1", &sample_tags("技术", "Rust"));
        idx.upsert("item2", &sample_tags("技术", "Python"));
        idx.upsert("item3", &sample_tags("法律", "合同"));

        let tech = idx.query("domain", "技术");
        assert_eq!(tech.len(), 2);

        let rust = idx.query("topic", "Rust");
        assert_eq!(rust, vec!["item1".to_string()]);
    }

    #[test]
    fn query_and_intersects() {
        let mut idx = TagIndex::new();
        idx.upsert("a", &sample_tags("技术", "Rust"));
        idx.upsert("b", &sample_tags("技术", "Python"));
        idx.upsert("c", &sample_tags("法律", "Rust"));

        let filters = vec![("domain".into(), "技术".into()), ("topic".into(), "Rust".into())];
        let results = idx.query_and(&filters);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "a");
    }

    #[test]
    fn query_or_unions() {
        let mut idx = TagIndex::new();
        idx.upsert("a", &sample_tags("技术", "Rust"));
        idx.upsert("b", &sample_tags("法律", "合同"));

        let filters = vec![("domain".into(), "技术".into()), ("domain".into(), "法律".into())];
        let mut results = idx.query_or(&filters);
        results.sort();
        assert_eq!(results, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn remove_cleans_all_entries() {
        let mut idx = TagIndex::new();
        idx.upsert("a", &sample_tags("技术", "Rust"));
        assert_eq!(idx.item_count(), 1);

        idx.remove("a");
        assert_eq!(idx.item_count(), 0);
        assert!(idx.query("domain", "技术").is_empty());
    }

    #[test]
    fn upsert_replaces_old_values() {
        let mut idx = TagIndex::new();
        idx.upsert("a", &sample_tags("技术", "Rust"));
        idx.upsert("a", &sample_tags("法律", "合同"));

        assert!(idx.query("domain", "技术").is_empty());
        assert_eq!(idx.query("domain", "法律"), vec!["a".to_string()]);
    }

    #[test]
    fn histogram_counts_correctly() {
        let mut idx = TagIndex::new();
        idx.upsert("a", &sample_tags("技术", "Rust"));
        idx.upsert("b", &sample_tags("技术", "Rust"));
        idx.upsert("c", &sample_tags("技术", "Python"));
        idx.upsert("d", &sample_tags("法律", "合同"));

        let hist = idx.histogram("domain");
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].0, "技术");
        assert_eq!(hist[0].1, 3);
        assert_eq!(hist[1].0, "法律");
        assert_eq!(hist[1].1, 1);
    }
}
