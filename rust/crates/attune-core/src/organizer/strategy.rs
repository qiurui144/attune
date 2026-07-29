use super::types::{ClusterView, ItemView, LabelSource, RoleAssignment};
use crate::error::Result;
use crate::llm::LlmProvider;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ClusterLabel {
    pub name: String,
    pub summary: String,
    pub confidence: f32,
    pub source: LabelSource,
}

pub struct LabelCtx<'a> {
    pub llm: Option<&'a dyn LlmProvider>,
}

/// 领域无关的整理策略。命名/角色/kind 的领域语义由各实现定义;attune-core 不解释取值。
pub trait OrganizationStrategy: Send + Sync {
    /// 适用的 corpus_domain 标签值。
    fn domain(&self) -> &str;
    /// 给一组(已聚好)item 命名 + 摘要。每组仅 1 次 LLM(失败时由引擎兜底,不强制调)。
    fn label_cluster(&self, cluster: &ClusterView, ctx: &LabelCtx) -> Result<ClusterLabel>;
    /// 给组内某 item 分配角色;无角色概念返回 None。
    fn assign_role(&self, item: &ItemView, label: &ClusterLabel) -> Option<RoleAssignment>;
    /// 建议新建 Project 的 kind 字符串(取值由策略定义)。
    fn suggest_project_kind(&self, label: &ClusterLabel) -> String;
}

/// attune-core 唯一内置策略:主题命名,无角色,kind=collection。
pub struct GenericStrategy;
impl OrganizationStrategy for GenericStrategy {
    fn domain(&self) -> &str {
        "general"
    }
    fn label_cluster(&self, cluster: &ClusterView, ctx: &LabelCtx) -> Result<ClusterLabel> {
        // 优先 LLM 命名;无 LLM → extractive(取首 item 标题前若干词)。
        if let Some(llm) = ctx.llm {
            let titles: Vec<&str> = cluster
                .items
                .iter()
                .take(8)
                .map(|i| i.title.as_str())
                .collect();
            let user = format!("为以下同类文件起一个简短中文主题名(≤8字)和一句摘要,返回 JSON {{\"name\":..,\"summary\":..}}:\n{}", titles.join("\n"));
            if let Ok((raw, _)) = llm.chat("你是文件归类助手。", &user) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim()) {
                    if let Some(name) = v.get("name").and_then(|x| x.as_str()) {
                        return Ok(ClusterLabel {
                            name: name.into(),
                            summary: v
                                .get("summary")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .into(),
                            confidence: 0.8,
                            source: LabelSource::Llm,
                        });
                    }
                }
            }
        }
        Ok(extractive_label(cluster))
    }
    fn assign_role(&self, _item: &ItemView, _label: &ClusterLabel) -> Option<RoleAssignment> {
        None
    }
    fn suggest_project_kind(&self, _label: &ClusterLabel) -> String {
        "collection".into()
    }
}

/// 引擎兜底命名:簇内标题词频 top1 作名。供策略复用。
pub fn extractive_label(cluster: &ClusterView) -> ClusterLabel {
    let mut freq: HashMap<String, usize> = HashMap::new();
    for it in cluster.items {
        for w in it
            .title
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() >= 2)
        {
            *freq.entry(w.to_lowercase()).or_default() += 1;
        }
    }
    let name = freq
        .into_iter()
        .max_by_key(|(_, c)| *c)
        .map(|(w, _)| w)
        .unwrap_or_else(|| "未命名分组".into());
    ClusterLabel {
        name,
        summary: String::new(),
        confidence: 0.4,
        source: LabelSource::Extractive,
    }
}

pub struct StrategyRegistry {
    map: HashMap<String, Arc<dyn OrganizationStrategy>>,
    default: Arc<dyn OrganizationStrategy>,
}
impl StrategyRegistry {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            default: Arc::new(GenericStrategy),
        }
    }
    pub fn register(&mut self, s: Arc<dyn OrganizationStrategy>) {
        self.map.insert(s.domain().to_string(), s);
    }
    /// 精确 domain 命中 → 否则 default(Generic)。
    pub fn resolve(&self, domain: Option<&str>) -> Arc<dyn OrganizationStrategy> {
        domain
            .and_then(|d| self.map.get(d))
            .cloned()
            .unwrap_or_else(|| self.default.clone())
    }
}
impl Default for StrategyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn registry_resolves_known_else_default() {
        let mut reg = StrategyRegistry::new();
        assert_eq!(reg.resolve(Some("legal")).domain(), "general"); // 未注册 → default
        reg.register(Arc::new(GenericStrategy) as Arc<dyn OrganizationStrategy>);
        assert_eq!(reg.resolve(None).domain(), "general");
    }
    #[test]
    fn generic_strategy_no_role_collection_kind() {
        let s = GenericStrategy;
        let label = ClusterLabel {
            name: "topic".into(),
            summary: "".into(),
            confidence: 0.8,
            source: super::super::types::LabelSource::Extractive,
        };
        assert!(s.assign_role(&dummy_item(), &label).is_none());
        assert_eq!(s.suggest_project_kind(&label), "collection");
    }
    fn dummy_item() -> super::super::types::ItemView {
        super::super::types::ItemView {
            item_id: "i".into(),
            title: "t".into(),
            content_snippet: "".into(),
            dir: "".into(),
            embedding: None,
        }
    }
}
