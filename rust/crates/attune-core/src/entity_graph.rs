//! Entity 关系图数据结构（v0.7 sprint，F-EntityGraph）
//!
//! 设计目的：把通过 `entities::extract_entities()` 抽出来的零散 entity，
//! 跨多个 item 聚合成图——同一 item 内同时出现的两个 entity 之间加一条边，
//! 权重为共现次数。可用于 Project 推荐、相关 item 召回、知识地图可视化。
//!
//! 范围（v0.7 OSS）：纯内存 + JSON 导出。后续 attune-pro 可扩展持久化、
//! 多 hop 查询、向量化 entity embedding 等高级能力。

use crate::entities::Entity;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 图中节点：一个 entity（同 kind+name 唯一），并记录出现在哪些 item 中。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntityNode {
    /// 节点 id（约定：`<kind>:<name>`，便于跨 item 复用同一节点）
    pub id: String,
    /// 实体类别（person / money / date / organization 等）
    pub kind: String,
    /// 实体名（去重后的字面值）
    pub name: String,
    /// 哪些 item 提到过这个 entity
    pub item_ids: Vec<String>,
}

/// 图中边：两个 entity 共现关系。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntityEdge {
    /// 源节点 id
    pub from: String,
    /// 目标节点 id
    pub to: String,
    /// 关系类别（默认 "co_occurrence"，行业版可自定义 "represents"/"litigates"/"funds" 等）
    pub relation: String,
    /// 权重（默认即共现次数）
    pub weight: f32,
}

/// Entity 关系图主体。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityGraph {
    /// 节点表：id → node
    nodes: HashMap<String, EntityNode>,
    /// 边列表（允许重复 — 调用方决定是否去重）
    edges: Vec<EntityEdge>,
}

impl EntityGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: Vec::new(),
        }
    }

    /// 加节点；若同 id 已存在，合并 item_ids（去重）。
    pub fn add_node(&mut self, node: EntityNode) {
        let entry = self.nodes.entry(node.id.clone()).or_insert_with(|| EntityNode {
            id: node.id.clone(),
            kind: node.kind.clone(),
            name: node.name.clone(),
            item_ids: Vec::new(),
        });
        for iid in &node.item_ids {
            if !entry.item_ids.contains(iid) {
                entry.item_ids.push(iid.clone());
            }
        }
    }

    /// 加边；不做去重，调用方按需聚合。
    pub fn add_edge(&mut self, edge: EntityEdge) {
        self.edges.push(edge);
    }

    /// 按 id 查节点。
    pub fn find_node(&self, id: &str) -> Option<&EntityNode> {
        self.nodes.get(id)
    }

    /// 按 kind 列节点。
    pub fn nodes_by_kind(&self, kind: &str) -> Vec<&EntityNode> {
        self.nodes.values().filter(|n| n.kind == kind).collect()
    }

    /// 列从指定节点出发的所有边。
    pub fn edges_from(&self, node_id: &str) -> Vec<&EntityEdge> {
        self.edges.iter().filter(|e| e.from == node_id).collect()
    }

    /// 节点总数。
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// 边总数。
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// 导出 JSON（用于前端可视化或调试）。
    pub fn to_json(&self) -> serde_json::Value {
        let nodes: Vec<&EntityNode> = self.nodes.values().collect();
        serde_json::json!({
            "nodes": nodes,
            "edges": self.edges,
        })
    }
}

/// 从 (item_id, entities) 列表构建图：
/// - 每个 entity 变 node（id = `<kind>:<value>`），item_id 加入 node.item_ids
/// - 同一 item 内每对不同 entity 之间加一条无向边（约定 from < to 字典序），
///   relation = "co_occurrence"，weight 用共现次数累加。
pub fn build_from_items(items: &[(String, Vec<Entity>)]) -> EntityGraph {
    let mut g = EntityGraph::new();

    // 用 HashMap 聚合边权重，最后再 flush 到 g.edges
    let mut edge_weights: HashMap<(String, String), f32> = HashMap::new();

    for (item_id, ents) in items {
        // 1. 加节点
        for e in ents {
            let kind = format!("{:?}", e.kind).to_lowercase();
            let id = format!("{}:{}", kind, e.value);
            g.add_node(EntityNode {
                id,
                kind,
                name: e.value.clone(),
                item_ids: vec![item_id.clone()],
            });
        }

        // 2. 加边（同 item 内两两组合，去重后字典序排）
        let mut ids: Vec<String> = ents
            .iter()
            .map(|e| format!("{}:{}", format!("{:?}", e.kind).to_lowercase(), e.value))
            .collect();
        ids.sort();
        ids.dedup();

        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                let from = ids[i].clone();
                let to = ids[j].clone();
                *edge_weights.entry((from, to)).or_insert(0.0) += 1.0;
            }
        }
    }

    for ((from, to), weight) in edge_weights {
        g.add_edge(EntityEdge {
            from,
            to,
            relation: "co_occurrence".to_string(),
            weight,
        });
    }

    g
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{Entity, EntityKind};

    #[test]
    fn empty_graph() {
        let g = EntityGraph::new();
        assert_eq!(g.node_count(), 0);
        assert_eq!(g.edge_count(), 0);
        assert!(g.find_node("person:zhang").is_none());
        assert!(g.nodes_by_kind("person").is_empty());
        let json = g.to_json();
        assert_eq!(json["nodes"].as_array().unwrap().len(), 0);
        assert_eq!(json["edges"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn add_node_and_edge() {
        let mut g = EntityGraph::new();
        g.add_node(EntityNode {
            id: "person:Alice".into(),
            kind: "person".into(),
            name: "Alice".into(),
            item_ids: vec!["item1".into()],
        });
        // 同 id 再加一次 → item_ids 合并去重
        g.add_node(EntityNode {
            id: "person:Alice".into(),
            kind: "person".into(),
            name: "Alice".into(),
            item_ids: vec!["item1".into(), "item2".into()],
        });
        g.add_edge(EntityEdge {
            from: "person:Alice".into(),
            to: "org:ACME".into(),
            relation: "co_occurrence".into(),
            weight: 1.0,
        });
        assert_eq!(g.node_count(), 1);
        assert_eq!(g.edge_count(), 1);
        let node = g.find_node("person:Alice").unwrap();
        assert_eq!(node.item_ids, vec!["item1", "item2"]);
    }

    #[test]
    fn find_node() {
        let mut g = EntityGraph::new();
        g.add_node(EntityNode {
            id: "person:Bob".into(),
            kind: "person".into(),
            name: "Bob".into(),
            item_ids: vec![],
        });
        assert!(g.find_node("person:Bob").is_some());
        assert!(g.find_node("person:Eve").is_none());
    }

    #[test]
    fn nodes_by_kind() {
        let mut g = EntityGraph::new();
        g.add_node(EntityNode {
            id: "person:Alice".into(),
            kind: "person".into(),
            name: "Alice".into(),
            item_ids: vec![],
        });
        g.add_node(EntityNode {
            id: "person:Bob".into(),
            kind: "person".into(),
            name: "Bob".into(),
            item_ids: vec![],
        });
        g.add_node(EntityNode {
            id: "organization:ACME".into(),
            kind: "organization".into(),
            name: "ACME".into(),
            item_ids: vec![],
        });
        let persons = g.nodes_by_kind("person");
        assert_eq!(persons.len(), 2);
        let orgs = g.nodes_by_kind("organization");
        assert_eq!(orgs.len(), 1);
        assert!(g.nodes_by_kind("money").is_empty());
    }

    #[test]
    fn build_from_items_3items() {
        // item1: 张三 + ACME 公司
        // item2: 张三 + 李四
        // item3: 李四 + ACME 公司
        let mk = |kind, val: &str, start, end| Entity {
            kind,
            value: val.to_string(),
            byte_start: start,
            byte_end: end,
        };
        let items = vec![
            (
                "item1".to_string(),
                vec![mk(EntityKind::Person, "张三", 0, 6), mk(EntityKind::Organization, "ACME 公司", 7, 17)],
            ),
            (
                "item2".to_string(),
                vec![mk(EntityKind::Person, "张三", 0, 6), mk(EntityKind::Person, "李四", 8, 14)],
            ),
            (
                "item3".to_string(),
                vec![mk(EntityKind::Person, "李四", 0, 6), mk(EntityKind::Organization, "ACME 公司", 7, 17)],
            ),
        ];

        let g = build_from_items(&items);
        // 3 个不同节点：person:张三 / person:李四 / organization:ACME 公司
        assert_eq!(g.node_count(), 3);
        // 张三 出现在 item1+item2
        let zhang = g.find_node("person:张三").unwrap();
        assert_eq!(zhang.item_ids.len(), 2);
        assert!(zhang.item_ids.contains(&"item1".to_string()));
        assert!(zhang.item_ids.contains(&"item2".to_string()));
        // 李四 出现在 item2+item3
        let li = g.find_node("person:李四").unwrap();
        assert_eq!(li.item_ids.len(), 2);
        // ACME 出现在 item1+item3
        let acme = g.find_node("organization:ACME 公司").unwrap();
        assert_eq!(acme.item_ids.len(), 2);

        // 边：item1 → 张三-ACME, item2 → 张三-李四, item3 → 李四-ACME
        // 共 3 条无向边，每条 weight=1.0
        assert_eq!(g.edge_count(), 3);
        let edges_from_zhang = g.edges_from("person:张三");
        // 张三作为 from 出现在字典序在他后面的边里
        assert!(!edges_from_zhang.is_empty() || !g.edges_from("person:李四").is_empty());

        // 总边权重 = 3.0
        let total_weight: f32 = g.edges.iter().map(|e| e.weight).sum();
        assert!((total_weight - 3.0).abs() < 1e-6);

        // to_json 包含 nodes + edges
        let json = g.to_json();
        assert_eq!(json["nodes"].as_array().unwrap().len(), 3);
        assert_eq!(json["edges"].as_array().unwrap().len(), 3);
    }
}
